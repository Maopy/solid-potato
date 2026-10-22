//! Bounded HTTP/1.1 bridge. No CORS, arbitrary execution or chat submission.
use super::{bridge::ProbeBridge, protocol::*};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, watch},
    task::{JoinHandle, JoinSet},
};

pub struct ProbeServer {
    pub address: SocketAddr,
    pub bridge: Arc<Mutex<ProbeBridge>>,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl ProbeServer {
    pub async fn bind(bridge: ProbeBridge) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let bridge = Arc::new(Mutex::new(bridge));
        let state = bridge.clone();
        let (shutdown, mut stopping) = watch::channel(false);
        let task = tokio::spawn(async move {
            let limit = Arc::new(Semaphore::new(32));
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    _ = stopping.changed() => break,
                    Some(_) = clients.join_next(), if !clients.is_empty() => {},
                    accepted = listener.accept() => {
                        let Ok((mut socket, peer)) = accepted else { break; };
                        if !peer.ip().is_loopback() { continue; }
                        let Ok(permit) = limit.clone().try_acquire_owned() else { continue; };
                        let bridge = state.clone();
                        clients.spawn(async move {
                            let _permit = permit;
                            let _ = tokio::time::timeout(Duration::from_secs(3), handle(&mut socket, address, bridge)).await;
                        });
                    }
                }
            }
            // Each client already has a 3-second deadline. Let accepted
            // requests finish so the final durable ACK/goodbye response is
            // not cut off immediately after its session is removed.
            while clients.join_next().await.is_some() {}
        });
        Ok(Self {
            address,
            bridge,
            shutdown,
            task: Some(task),
        })
    }

    pub async fn stop(&mut self) -> Result<()> {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            task.await.context("probe server panicked")?;
        }
        Ok(())
    }

    /// Keep the authenticated bridge available while existing pages freeze
    /// collection, acknowledge buffered batches, and close their sessions.
    pub async fn drain(&mut self, timeout: Duration) -> Result<bool> {
        self.bridge
            .lock()
            .map_err(|_| anyhow::anyhow!("probe lock poisoned"))?
            .begin_drain();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self
                .bridge
                .lock()
                .map_err(|_| anyhow::anyhow!("probe lock poisoned"))?
                .drain_finished()
            {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for ProbeServer {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub bearer: String,
    pub origin: String,
    pub body: Vec<u8>,
}

pub(crate) async fn read_request(socket: &mut TcpStream, address: SocketAddr) -> Result<Request> {
    let mut bytes = Vec::new();
    let (header_end, length, headers, path, method) = loop {
        let mut chunk = [0u8; 4096];
        let count = socket.read(&mut chunk).await?;
        if count == 0 {
            bail!("incomplete request");
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_BODY + 8192 {
            bail!("request too large");
        }
        let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
            if bytes.len() > 8192 {
                bail!("headers too large");
            }
            continue;
        };
        if end > 8192 {
            bail!("headers too large");
        }
        let text = std::str::from_utf8(&bytes[..end])?;
        let mut lines = text.split("\r\n");
        let request: Vec<_> = lines
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .collect();
        if request.len() != 3 || !matches!(request[0], "POST" | "GET") || request[2] != "HTTP/1.1" {
            bail!("only POST HTTP/1.1 accepted");
        }
        let path = request[1].to_string();
        let mut headers = HashMap::new();
        for line in lines {
            let (key, value) = line.split_once(':').context("malformed header")?;
            let key = key.to_ascii_lowercase();
            if headers.insert(key, value.trim().to_string()).is_some() {
                bail!("duplicate header");
            }
        }
        if headers.get("host") != Some(&address.to_string())
            || headers.contains_key("transfer-encoding")
        {
            bail!("invalid host or framing");
        }
        if request[0] == "POST"
            && headers.get("content-type").map(|s| s.split(';').next())
                != Some(Some("application/json"))
        {
            bail!("JSON content type required");
        }
        let length: usize = headers
            .get("content-length")
            .map(String::as_str)
            .unwrap_or("0")
            .parse()?;
        if request[0] == "POST" && !headers.contains_key("content-length") {
            bail!("length required");
        }
        if length > MAX_BODY {
            bail!("body too large");
        }
        break (end, length, headers, path, request[0].to_string());
    };
    let target = header_end + 4 + length;
    while bytes.len() < target {
        let remaining = target - bytes.len();
        let mut chunk = [0u8; 4096];
        let count = socket.read(&mut chunk[..remaining.min(4096)]).await?;
        if count == 0 {
            bail!("incomplete body");
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    if bytes.len() != target {
        bail!("extra request bytes");
    }
    Ok(Request {
        method,
        path,
        bearer: headers
            .get("authorization")
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or_default()
            .to_owned(),
        origin: headers.get("origin").cloned().unwrap_or_default(),
        body: bytes[header_end + 4..].to_vec(),
    })
}

pub(crate) async fn respond(
    socket: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let headers = format!(
        "HTTP/1.1 {status} Bridge\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(headers.as_bytes()).await?;
    socket.write_all(body).await?;
    socket.shutdown().await?;
    Ok(())
}

async fn handle(
    socket: &mut TcpStream,
    address: SocketAddr,
    bridge: Arc<Mutex<ProbeBridge>>,
) -> Result<()> {
    let request = match read_request(socket, address).await {
        Ok(request) => request,
        Err(_) => {
            return respond(
                socket,
                400,
                "application/json",
                br#"{"error":"invalid_request"}"#,
            )
            .await;
        }
    };
    let (status, content_type, body) = {
        let mut bridge = bridge
            .lock()
            .map_err(|_| anyhow::anyhow!("bridge state poisoned"))?;
        if request.method != "POST" {
            (
                400,
                "application/json",
                br#"{"error":"invalid_method"}"#.to_vec(),
            )
        } else if request.path == "/v1/bootstrap" {
            match bridge.bootstrap(&request.bearer) {
                Ok(script) => (200, "application/javascript", script.into_bytes()),
                Err(_) => (
                    403,
                    "application/json",
                    br#"{"error":"bootstrap_denied"}"#.to_vec(),
                ),
            }
        } else if request.origin != bridge.scope.room_url.origin().ascii_serialization() {
            (
                403,
                "application/json",
                br#"{"error":"origin_denied"}"#.to_vec(),
            )
        } else {
            let result: Result<Value> = (|| match request.path.as_str() {
                "/v1/control" => {
                    bridge.control(&request.bearer, serde_json::from_slice(&request.body)?)
                }
                "/v1/speech" => {
                    bridge.speech(&request.bearer, serde_json::from_slice(&request.body)?)
                }
                "/v1/hello" => bridge.hello(serde_json::from_slice(&request.body)?),
                "/v1/events" => {
                    bridge.events(&request.bearer, serde_json::from_slice(&request.body)?)
                }
                "/v1/raw" => {
                    bridge.raw_messages(&request.bearer, serde_json::from_slice(&request.body)?)
                }
                "/v1/likes/claim" => bridge
                    .claim_like_period(&request.bearer, serde_json::from_slice(&request.body)?),
                "/v1/poll" => bridge.poll(&request.bearer, serde_json::from_slice(&request.body)?),
                "/v1/results" => {
                    bridge.fill_result(&request.bearer, serde_json::from_slice(&request.body)?)
                }
                "/v1/delivery" => {
                    bridge.delivery_result(&request.bearer, serde_json::from_slice(&request.body)?)
                }
                "/v1/goodbye" => {
                    bridge.goodbye(&request.bearer, serde_json::from_slice(&request.body)?)
                }
                _ => bail!("unsupported endpoint"),
            })();
            match result {
                Ok(value) => (200, "application/json", serde_json::to_vec(&value)?),
                Err(_) => (
                    422,
                    "application/json",
                    serde_json::to_vec(
                        &json!({"error":"rejected","retry_requires_new_pairing":false}),
                    )?,
                ),
            }
        }
    };
    respond(socket, status, content_type, &body).await
}
