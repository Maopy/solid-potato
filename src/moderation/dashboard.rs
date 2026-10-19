//! Loopback-only unified controls. A per-run bearer stays in the URL fragment.
use super::{bridge::ProbeBridge, control::Controller};
use crate::download_task_bridge::DownloadTaskBridge;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
    task::JoinHandle,
};
struct Room {
    id: String,
    label: String,
    url: String,
    platform: String,
    bridge: Arc<Mutex<ProbeBridge>>,
}
pub struct Dashboard {
    pub url: String,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}
impl Dashboard {
    pub async fn bind(
        controller: &Controller,
        recordings: Option<DownloadTaskBridge>,
    ) -> Result<Self> {
        Self::bind_with_userscript(controller, recordings, None).await
    }
    pub async fn bind_with_userscript(
        controller: &Controller,
        recordings: Option<DownloadTaskBridge>,
        install_url: Option<String>,
    ) -> Result<Self> {
        let hub = controller.userscript.clone();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let rooms: Arc<Vec<Room>> = Arc::new(
            controller
                .tasks
                .values()
                .map(|t| Room {
                    id: t.id.clone(),
                    label: t.label.clone(),
                    url: t.scope.room_url.to_string(),
                    platform: t.scope.platform().into(),
                    bridge: t.server.bridge.clone(),
                })
                .collect(),
        );
        let (stop, mut stopped) = watch::channel(false);
        let auth = token.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {_=stopped.changed()=>break,accepted=listener.accept()=>{let Ok((socket,_))=accepted else{break;};let rooms=rooms.clone();let auth=auth.clone();let recordings=recordings.clone();let hub=hub.clone();let install_url=install_url.clone();tokio::spawn(async move{let _=tokio::time::timeout(std::time::Duration::from_secs(3),handle(socket,address,&auth,&rooms,recordings,hub,install_url)).await;});}}
            }
        });
        Ok(Self {
            url: format!("http://{address}/#{token}"),
            stop,
            task: Some(task),
        })
    }
    pub async fn stop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for Dashboard {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}
async fn handle(
    mut socket: TcpStream,
    address: SocketAddr,
    token: &str,
    rooms: &[Room],
    recordings: Option<DownloadTaskBridge>,
    hub: Option<super::userscript::UserscriptHub>,
    install_url: Option<String>,
) -> Result<()> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        data.extend_from_slice(&chunk[..n]);
        if data.len() > 16384 {
            bail!("dashboard request too large");
        }
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = std::str::from_utf8(&data[..end])?;
    let mut lines = head.split("\r\n");
    let first: Vec<_> = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    if first.len() != 3 {
        bail!("invalid request");
    }
    let (method, path) = (first[0].to_string(), first[1].to_string());
    let mut headers = std::collections::HashMap::new();
    for line in lines {
        let (k, v) = line.split_once(':').context("invalid header")?;
        if headers
            .insert(k.to_lowercase(), v.trim().to_string())
            .is_some()
        {
            bail!("duplicate header");
        }
    }
    let origin = format!("http://{address}");
    if headers.get("host") != Some(&address.to_string())
        || headers.contains_key("transfer-encoding")
    {
        return respond(&mut socket, 403, "application/json", b"{}").await;
    }
    if method == "GET" && path == "/" {
        return respond(
            &mut socket,
            200,
            "text/html; charset=utf-8",
            include_bytes!("../../assets/moderation/dashboard.html"),
        )
        .await;
    }
    if headers.get("authorization") != Some(&format!("Bearer {token}"))
        || headers.get("origin").is_some_and(|s| s != &origin)
    {
        return respond(&mut socket, 403, "application/json", b"{}").await;
    }
    let size: usize = headers.get("content-length").map_or(Ok(0), |s| s.parse())?;
    if size > 4096 {
        bail!("body too large");
    }
    while data.len() < end + 4 + size {
        let n = socket.read(&mut chunk).await?;
        if n == 0 {
            bail!("incomplete request");
        }
        data.extend_from_slice(&chunk[..n]);
    }
    if data.len() != end + 4 + size {
        bail!("extra bytes");
    }
    let result:Result<Value>=async {
        if method=="GET"&&path=="/api/state" {
            let mut rows:Vec<_>=rooms.iter().map(|r|json!({"id":r.id,"label":r.label,"url":r.url,"platform":r.platform,"control":r.bridge.lock().ok().map(|b|b.control_status())})).collect();
            if let Some(hub)=&hub {rows.extend(hub.rows());}
            return Ok(json!({"rooms":rows,"userscript_install_url":install_url,"downloads":if let Some(t)=&recordings{t.list().await}else{vec![]}}));
        }
        if method!="POST"||headers.get("content-type").map(String::as_str)!=Some("application/json"){bail!("invalid action");}
        let body:Value=serde_json::from_slice(&data[end+4..])?;
        let id=body["id"].as_str().context("id required")?;
        if path=="/api/download" {
            let tasks=recordings.as_ref().context("recording unavailable")?;
            let task=match body["action"].as_str(){Some("pause")=>tasks.pause(id).await,Some("start")=>tasks.start(id).await,Some("delete")=>tasks.delete(id).await,_=>None};
            return Ok(json!({"task":task.context("unknown recording task or action")?}));
        }
        if !rooms.iter().any(|r|r.id==id) {
            let hub=hub.as_ref().context("unknown room")?;
            return match path.as_str() {"/api/moderation"=>hub.action(id,Some(body["enabled"].as_bool().context("enabled required")?)),"/api/record"=>hub.action(id,None),_=>bail!("unknown action")};
        }
        let room=rooms.iter().find(|r|r.id==id).context("unknown room")?;
        let mut bridge=room.bridge.lock().map_err(|_|anyhow::anyhow!("bridge unavailable"))?;
        match path.as_str(){"/api/moderation"=>bridge.set_moderation_enabled(body["enabled"].as_bool().context("enabled required")?),"/api/record"=>bridge.request_recording(),_=>bail!("unknown action")}
    }.await;
    let (status, value) = match result {
        Ok(value) => (200, value),
        Err(error) => (422, json!({"error":error.to_string()})),
    };
    respond(
        &mut socket,
        status,
        "application/json",
        &serde_json::to_vec(&value)?,
    )
    .await
}
async fn respond(socket: &mut TcpStream, status: u16, kind: &str, body: &[u8]) -> Result<()> {
    socket.write_all(format!("HTTP/1.1 {status} OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\r\n",body.len()).as_bytes()).await?;
    socket.write_all(body).await?;
    Ok(())
}
