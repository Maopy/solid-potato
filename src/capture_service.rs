//! Narrow loopback handoff for a consented WeChat TLS-capture sidecar.
//!
//! It intentionally accepts metadata only: no HTTP bodies, cookies, request
//! headers, or decryption keys are stored. A sidecar/proxy that has already
//! passed the native consent and certificate flow can POST a discovered media
//! URL to this service.
use crate::download_task_bridge::{DownloadTaskBridge, TaskSnapshot};
use crate::wechat::{CapturedWeChatStream, WeChatCaptureStore};
use crate::wechat_bridge::{ChannelsBridge, LiveInfoRequest};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
};
use uuid::Uuid;

// The reference frontend submits the complete Channels `profile + live`
// object when creating a live task. Those signed metadata objects routinely
// exceed 16 KiB, while a 1 MiB cap remains intentionally bounded for this
// loopback-only service.
const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct CapturePermit(String);

impl CapturePermit {
    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }
    pub fn as_bearer(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaptureEvent {
    pub page_url: url::Url,
    pub stream_url: url::Url,
    pub title: String,
    #[serde(default)]
    pub anchor_username: Option<String>,
    #[serde(default)]
    pub frontend_task_id: Option<String>,
}

/// Reference frontend request body for `POST /api/v1/download_task/create`.
/// We intentionally support only the one, explicit WeChat live-task shape;
/// this loopback service is not a general download API.
#[derive(Debug, Deserialize)]
struct ReferenceCreateRequest {
    objects: Vec<ReferenceCreateObject>,
}

#[derive(Debug, Deserialize)]
struct ReferenceCreateObject {
    platform: String,
    content: ReferenceLiveContent,
}

#[derive(Debug, Deserialize)]
struct ReferenceLiveContent {
    profile: Value,
    live: Value,
}

#[derive(Debug, Serialize)]
struct ReferenceCreateResponse {
    code: u8,
    data: ReferenceCreateData,
}

#[derive(Debug, Serialize)]
struct ReferenceCreateData {
    tasks: Vec<ReferenceCreateTask>,
}

#[derive(Debug, Serialize)]
struct ReferenceCreateTask {
    code: u8,
    data: ReferenceTaskData,
}

#[derive(Debug, Serialize)]
struct ReferenceTaskData {
    id: String,
    status: &'static str,
}

#[derive(Debug, Deserialize)]
struct ReferenceTaskIdsRequest {
    #[serde(default)]
    task_ids: Vec<String>,
}

pub struct CaptureIngestor {
    captures: Arc<WeChatCaptureStore>,
    permit: CapturePermit,
    bridge: Option<ChannelsBridge>,
    task_bridge: Option<DownloadTaskBridge>,
    recording: Option<tokio::sync::mpsc::Sender<crate::moderation::recording::RecordingRequest>>,
    recording_rooms: Vec<url::Url>,
}

impl CaptureIngestor {
    pub fn new(captures: Arc<WeChatCaptureStore>, permit: CapturePermit) -> Self {
        Self {
            captures,
            permit,
            bridge: None,
            task_bridge: None,
            recording: None,
            recording_rooms: vec![],
        }
    }
    pub fn with_bridge(mut self, bridge: ChannelsBridge) -> Self {
        self.bridge = Some(bridge);
        self
    }
    pub fn with_task_bridge(mut self, bridge: DownloadTaskBridge) -> Self {
        self.task_bridge = Some(bridge);
        self
    }

    pub fn with_recording_requests(
        mut self,
        sender: tokio::sync::mpsc::Sender<crate::moderation::recording::RecordingRequest>,
        rooms: Vec<url::Url>,
    ) -> Self {
        self.recording = Some(sender);
        self.recording_rooms = rooms;
        self
    }
    fn enqueue_recording(
        &self,
        event: CaptureEvent,
        platform: crate::model::Platform,
    ) -> Result<()> {
        if let Some(sender) = &self.recording {
            sender
                .try_send(crate::moderation::recording::RecordingRequest {
                    id: event
                        .frontend_task_id
                        .unwrap_or_else(|| Uuid::new_v4().to_string()),
                    platform,
                    page_url: event.page_url,
                    stream_url: event.stream_url,
                    title: event.title,
                })
                .map_err(|_| anyhow::anyhow!("recording queue unavailable or full"))?;
        }
        Ok(())
    }
    pub fn ingest(&self, peer: IpAddr, bearer: &str, event: CaptureEvent) -> Result<()> {
        self.ingest_with_stream_policy(peer, bearer, event.clone(), false)?;
        self.enqueue_recording(event, crate::model::Platform::WeChatChannels)
    }

    fn ingest_with_stream_policy(
        &self,
        peer: IpAddr,
        bearer: &str,
        event: CaptureEvent,
        allow_http_stream: bool,
    ) -> Result<()> {
        if !peer.is_loopback() {
            anyhow::bail!("capture event must originate from loopback");
        }
        if bearer != self.permit.as_bearer() {
            anyhow::bail!("invalid capture permit");
        }
        if !is_wechat_channels_page(&event.page_url) {
            anyhow::bail!("capture page URL is outside the WeChat Channels allowlist");
        }
        if event.stream_url.scheme() != "https"
            && !(allow_http_stream && event.stream_url.scheme() == "http")
        {
            anyhow::bail!("captured stream URL must use HTTPS");
        }
        tracing::info!(title = %event.title, anchor = ?event.anchor_username, "captured WeChat live stream metadata");
        self.captures
            .publish(CapturedWeChatStream {
                page_url: event.page_url,
                stream_url: event.stream_url,
                title: event.title,
                anchor_username: event.anchor_username,
                frontend_task_id: event.frontend_task_id,
                captured_at: Utc::now(),
            })
            .map_err(|error| anyhow::anyhow!(error))
    }

    fn ingest_diagnostic(&self, peer: IpAddr, bearer: &str, stage: &str) -> Result<()> {
        if !peer.is_loopback() {
            anyhow::bail!("capture diagnostic must originate from loopback");
        }
        if bearer != self.permit.as_bearer() {
            anyhow::bail!("invalid capture permit");
        }
        if !matches!(
            stage,
            "submitted"
                | "missing-payload"
                | "resolving"
                | "no-cdn"
                | "join-failed"
                | "awaiting-playback"
                | "profile-loaded"
                | "live-candidate-ready"
                | "live-data-no-cdn"
                | "live-info-hook-called"
                | "profile-hook-called"
                | "join-hook-called"
                | "missing-live-identifiers"
                | "bridge-request"
                | "bridge-unavailable"
                | "join-returned-no-cdn"
                | "reference-live-script-ready"
                | "reference-live-button-present"
        ) {
            anyhow::bail!("invalid capture diagnostic stage");
        }
        tracing::info!(stage, "WeChat live recording button status");
        Ok(())
    }

    async fn ingest_reference_create(
        &self,
        peer: IpAddr,
        bearer: &str,
        request: ReferenceCreateRequest,
    ) -> Result<ReferenceCreateResponse> {
        if !peer.is_loopback() {
            anyhow::bail!("reference task request must originate from loopback");
        }
        if bearer != self.permit.as_bearer() {
            anyhow::bail!("invalid capture permit");
        }
        if self.recording.is_some() {
            if let Some(bridge) = &self.task_bridge {
                if bridge.list().await.len() >= 512 {
                    anyhow::bail!("recording task limit reached; remove old tasks first");
                }
            }
        }
        let object = request
            .objects
            .into_iter()
            .next()
            .context("reference task request contains no objects")?;
        let (page_url, stream_url, title, username, platform) =
            if object.platform == "douyin" && self.recording.is_some() {
                let page = object.content.profile["room_url"]
                    .as_str()
                    .context("missing Douyin room_url")?;
                let page_url = url::Url::parse(page)?;
                if !self
                    .recording_rooms
                    .iter()
                    .any(|room| room == &page_url && room.host_str() == Some("live.douyin.com"))
                {
                    anyhow::bail!("Douyin recording room is outside configured moderation scopes");
                }
                let stream_url = url::Url::parse(
                    object.content.live["stream_url"]
                        .as_str()
                        .context("missing stream_url")?,
                )?;
                if !matches!(stream_url.scheme(), "http" | "https")
                    || stream_url.host_str().is_none()
                    || !stream_url.username().is_empty()
                    || stream_url.password().is_some()
                {
                    anyhow::bail!("invalid media URL");
                }
                let title = object.content.profile["nickname"]
                    .as_str()
                    .unwrap_or("抖音直播")
                    .chars()
                    .take(128)
                    .collect();
                (
                    page_url,
                    stream_url,
                    title,
                    None,
                    crate::model::Platform::Douyin,
                )
            } else {
                if object.platform != "wxchannels" {
                    anyhow::bail!("only wxchannels live tasks are accepted");
                }
                let stream_url = find_string(&object.content.live, "liveCdnUrl")
                    .context("live task does not include liveCdnUrl")?;
                let stream_url = url::Url::parse(stream_url).context("invalid liveCdnUrl")?;
                if !matches!(stream_url.scheme(), "http" | "https") {
                    anyhow::bail!("captured stream URL must use HTTP or HTTPS");
                }
                let username = find_string(&object.content.profile, "username")
                    .or_else(|| find_string(&object.content.live, "username"))
                    .map(ToOwned::to_owned);
                let title = find_string(&object.content.profile, "nickname")
                    .or_else(|| find_string(&object.content.live, "nickname"))
                    .or_else(|| find_string(&object.content.profile, "description"))
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| "微信视频号直播".to_owned());
                let page_url = match &username {
                    Some(username) if !username.is_empty() => {
                        let mut page_url =
                            url::Url::parse("https://channels.weixin.qq.com/live-archive/")?;
                        page_url
                            .path_segments_mut()
                            .map_err(|_| anyhow::anyhow!("constructing live archive page URL"))?
                            .push(username);
                        page_url
                    }
                    _ => url::Url::parse("https://channels.weixin.qq.com/web/pages/live")?,
                };
                (
                    page_url,
                    stream_url,
                    title,
                    username,
                    crate::model::Platform::WeChatChannels,
                )
            };
        let id = Uuid::new_v4().to_string();
        let event = CaptureEvent {
            page_url,
            stream_url,
            title: title.clone(),
            anchor_username: username,
            frontend_task_id: Some(id.clone()),
        };
        if platform == crate::model::Platform::WeChatChannels {
            self.ingest_with_stream_policy(peer, bearer, event.clone(), true)?;
        }
        if let Some(bridge) = &self.task_bridge {
            // Publish before returning the create response. The unchanged
            // upstream SDK opens its task socket immediately after receiving
            // that response, so spawning this work caused an observable race.
            bridge
                .publish(TaskSnapshot {
                    id: id.clone(),
                    status: "waiting".into(),
                    name: title.clone(),
                    error: None,
                })
                .await;
        }
        if let Err(error) = self.enqueue_recording(event, platform) {
            if let Some(bridge) = &self.task_bridge {
                bridge.delete(&id).await;
            }
            return Err(error);
        }
        Ok(ReferenceCreateResponse {
            code: 0,
            data: ReferenceCreateData {
                tasks: vec![ReferenceCreateTask {
                    code: 0,
                    data: ReferenceTaskData {
                        id,
                        status: "waiting",
                    },
                }],
            },
        })
    }
}

#[derive(Deserialize)]
struct LiveInfoBody {
    username: String,
    oid: String,
    nid: String,
    id: String,
}

fn find_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value {
        Value::Object(values) => values
            .get(key)
            .and_then(Value::as_str)
            .or_else(|| values.values().find_map(|nested| find_string(nested, key))),
        Value::Array(values) => values.iter().find_map(|nested| find_string(nested, key)),
        _ => None,
    }
}

/// A small localhost-only HTTP endpoint for a capture proxy sidecar. It accepts
/// `POST /capture`, `Authorization: Bearer <permit>`, and a compact JSON body.
pub struct LoopbackCaptureServer {
    pub address: SocketAddr,
    shutdown: watch::Sender<bool>,
}

impl LoopbackCaptureServer {
    pub async fn bind(ingestor: Arc<CaptureIngestor>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding capture endpoint")?;
        let address = listener.local_addr()?;
        let (shutdown, mut stopping) = watch::channel(false);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = stopping.changed() => { if changed.is_err() || *stopping.borrow() { break; } }
                    accepted = listener.accept() => match accepted { Ok((socket, peer)) => { let ingestor = Arc::clone(&ingestor); tokio::spawn(async move { let _ = handle(socket, peer, ingestor).await; }); }, Err(_) => break }
                }
            }
        });
        Ok(Self { address, shutdown })
    }

    pub fn stop(&self) -> Result<()> {
        self.shutdown
            .send(true)
            .map_err(|error| anyhow::anyhow!("stopping capture server: {error}"))
    }
}

async fn handle(
    mut socket: TcpStream,
    peer: SocketAddr,
    ingestor: Arc<CaptureIngestor>,
) -> Result<()> {
    if !peer.ip().is_loopback() {
        return Ok(());
    }
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= MAX_REQUEST_BYTES {
            write_response(&mut socket, 413, "payload too large").await?;
            return Ok(());
        }
        let mut chunk = [0_u8; 2048];
        let count = socket.read(&mut chunk).await?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header = std::str::from_utf8(&bytes[..header_end])
                .context("capture headers are not UTF-8")?;
            let length = header
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + length {
                break;
            }
        }
    }
    let request = std::str::from_utf8(&bytes).context("capture request is not UTF-8")?;
    let (head, body) = request
        .split_once("\r\n\r\n")
        .context("malformed capture request")?;
    let frontend_origin = head.lines().find_map(|line| line.strip_prefix("Origin: "))
        == Some("https://channels.weixin.qq.com");
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let request_target = request_line.split_whitespace().nth(1).unwrap_or_default();
    let method = request_line.split_whitespace().next().unwrap_or_default();
    let is_capture = request_target.starts_with("/capture?") || request_target == "/capture";
    let is_diagnostic =
        request_target.starts_with("/capture-status?") || request_target == "/capture-status";
    let is_reference_create = request_target.starts_with("/api/v1/download_task/create?")
        || request_target == "/api/v1/download_task/create";
    let task_path = request_target.split('?').next().unwrap_or_default();
    let is_task_list = task_path == "/api/v1/download_task/list";
    let is_task_detail = task_path == "/api/v1/download_task/detail";
    let task_action = matches!(
        task_path,
        "/api/v1/download_task/start"
            | "/api/v1/download_task/resume"
            | "/api/v1/download_task/pause"
            | "/api/v1/download_task/retry"
            | "/api/v1/download_task/delete"
    );
    let is_task_read = is_task_list || is_task_detail;
    let is_live_info = request_target.starts_with("/live-info?") || request_target == "/live-info";
    let is_live_profile = request_target.starts_with("/api/channels/live/profile?");
    let is_task_api = is_reference_create || is_task_read || task_action;
    if method == "OPTIONS" && is_task_api && frontend_origin {
        return write_cors_preflight(&mut socket).await;
    }
    if (!is_live_profile && !is_task_read && method != "POST")
        || (!is_capture
            && !is_diagnostic
            && !is_reference_create
            && !is_task_read
            && !task_action
            && !is_live_info
            && !is_live_profile)
    {
        write_response(&mut socket, 404, "not found").await?;
        return Ok(());
    }
    let header_token = lines
        .find_map(|line| line.strip_prefix("Authorization: Bearer "))
        .unwrap_or_default();
    let token = if header_token.is_empty() {
        request_line
            .split_whitespace()
            .nth(1)
            .and_then(|path| path.split_once("permit=").map(|(_, permit)| permit))
            .and_then(|permit| permit.split('&').next())
            .unwrap_or_default()
    } else {
        header_token
    };
    // This is the same trust boundary as the reference local API: the server
    // is loopback-only and accepts the Channels desktop origin directly. The
    // proxy permit remains required for capture metadata endpoints.
    let task_token = if frontend_origin {
        ingestor.permit.as_bearer()
    } else {
        token
    };
    if is_live_info || is_live_profile {
        let result = async {
            if !peer.ip().is_loopback() || token != ingestor.permit.as_bearer() {
                anyhow::bail!("rejected");
            }
            let body: LiveInfoBody = if is_live_profile {
                let query = request_target
                    .split_once('?')
                    .map(|(_, value)| value)
                    .unwrap_or_default();
                let values: std::collections::HashMap<_, _> =
                    url::form_urlencoded::parse(query.as_bytes())
                        .into_owned()
                        .collect();
                LiveInfoBody {
                    username: values.get("username").cloned().unwrap_or_default(),
                    oid: values.get("oid").cloned().unwrap_or_default(),
                    nid: values.get("nid").cloned().unwrap_or_default(),
                    id: values.get("id").cloned().unwrap_or_default(),
                }
            } else {
                serde_json::from_str(body)?
            };
            let bridge = ingestor
                .bridge
                .clone()
                .context("Channels bridge unavailable")?;
            bridge
                .request_live_info(LiveInfoRequest {
                    username: body.username,
                    object_id: body.oid,
                    object_nonce_id: body.nid,
                    live_id: body.id,
                })
                .await
        }
        .await;
        return match result {
            Ok(value) => write_json_response(&mut socket, 200, &value).await,
            Err(_) => {
                write_json_response(
                    &mut socket,
                    503,
                    &serde_json::json!({"code":503,"msg":"live API unavailable"}),
                )
                .await
            }
        };
    }
    if is_reference_create {
        let result =
            match serde_json::from_str(body).context("invalid reference download task JSON") {
                Ok(request) => {
                    ingestor
                        .ingest_reference_create(peer.ip(), task_token, request)
                        .await
                }
                Err(error) => Err(error),
            };
        return match result {
            Ok(response) => write_json_response(&mut socket, 200, &response).await,
            Err(error) => {
                write_json_response(
                    &mut socket,
                    422,
                    &serde_json::json!({"code": 422, "msg": error.to_string()}),
                )
                .await
            }
        };
    }
    if is_task_read || task_action {
        if task_token != ingestor.permit.as_bearer() {
            return write_json_response(
                &mut socket,
                403,
                &serde_json::json!({"code": 403, "msg": "rejected"}),
            )
            .await;
        }
        let Some(bridge) = ingestor.task_bridge.clone() else {
            return write_json_response(
                &mut socket,
                503,
                &serde_json::json!({"code": 503, "msg": "download task bridge unavailable"}),
            )
            .await;
        };
        if is_task_list {
            let list = bridge.list().await;
            return write_json_response(
                &mut socket,
                200,
                &serde_json::json!({
                    "code": 0,
                    "data": {"list": list, "total": list.len(), "page": 1, "page_size": list.len(), "stats": {}}
                }),
            )
            .await;
        }
        if is_task_detail {
            let id = request_target
                .split_once('?')
                .map(|(_, query)| {
                    url::form_urlencoded::parse(query.as_bytes())
                        .find(|(key, _)| key == "id")
                        .map(|(_, value)| value.into_owned())
                })
                .flatten()
                .unwrap_or_default();
            return match bridge.detail(&id).await {
                Some(task) => {
                    write_json_response(
                        &mut socket,
                        200,
                        &serde_json::json!({"code": 0, "data": task}),
                    )
                    .await
                }
                None => {
                    write_json_response(
                        &mut socket,
                        404,
                        &serde_json::json!({"code": 404, "msg": "task not found"}),
                    )
                    .await
                }
            };
        }
        let ids: ReferenceTaskIdsRequest = match serde_json::from_str(body) {
            Ok(request) => request,
            Err(_) => {
                return write_json_response(
                    &mut socket,
                    400,
                    &serde_json::json!({"code": 400, "msg": "invalid task request"}),
                )
                .await;
            }
        };
        let mut tasks = Vec::new();
        for id in ids.task_ids {
            let task = match task_path {
                "/api/v1/download_task/start" | "/api/v1/download_task/resume" => {
                    bridge.start(&id).await
                }
                "/api/v1/download_task/pause" => bridge.pause(&id).await,
                "/api/v1/download_task/retry" => bridge.retry(&id).await,
                "/api/v1/download_task/delete" => bridge.delete(&id).await,
                _ => None,
            };
            if let Some(task) = task {
                tasks.push(task);
            }
        }
        return write_json_response(
            &mut socket,
            200,
            &serde_json::json!({"code": 0, "data": {"tasks": tasks}}),
        )
        .await;
    }
    let result = if is_diagnostic {
        let stage = request_target
            .split_once("stage=")
            .map(|(_, stage)| stage.split('&').next().unwrap_or_default())
            .unwrap_or_default();
        ingestor.ingest_diagnostic(peer.ip(), token, stage)
    } else {
        let event = serde_json::from_str(body).context("invalid capture JSON")?;
        ingestor.ingest(peer.ip(), token, event)
    };
    match result {
        Ok(()) => write_response(&mut socket, 202, "accepted").await?,
        Err(_) => write_response(&mut socket, 403, "rejected").await?,
    }
    Ok(())
}

async fn write_json_response<T: Serialize>(
    socket: &mut TcpStream,
    status: u16,
    body: &T,
) -> Result<()> {
    let body = serde_json::to_vec(body)?;
    socket
        .write_all(
            format!(
                "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: https://channels.weixin.qq.com\r\nVary: Origin\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    socket.write_all(&body).await?;
    Ok(())
}

async fn write_cors_preflight(socket: &mut TcpStream) -> Result<()> {
    socket
        .write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: https://channels.weixin.qq.com\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nAccess-Control-Max-Age: 600\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await?;
    Ok(())
}

async fn write_response(socket: &mut TcpStream, status: u16, message: &str) -> Result<()> {
    socket
        .write_all(
            format!(
                "HTTP/1.1 {status} {message}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    Ok(())
}

fn is_wechat_channels_page(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            host == "channels.weixin.qq.com" || host.ends_with(".channels.weixin.qq.com")
        })
}
