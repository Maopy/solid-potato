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
                tokio::select! {_=stopped.changed()=>break,accepted=listener.accept()=>{let Ok((socket,_))=accepted else{break;};let rooms=rooms.clone();let auth=auth.clone();let recordings=recordings.clone();let hub=hub.clone();let install_url=install_url.clone();tokio::spawn(async move{let _=tokio::time::timeout(std::time::Duration::from_secs(35),handle(socket,address,&auth,&rooms,recordings,hub,install_url)).await;});}}
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
    if method == "GET" && (path == "/" || path.starts_with("/?")) {
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
            let mut rows:Vec<_>=rooms.iter().flat_map(|r| { let bridge=r.bridge.lock().ok(); if r.platform=="wechat_channels" { if let Some(b)=bridge.as_ref() { let active=b.dashboard_rooms(&r.id,&r.label,&r.url); if !active.is_empty(){return active;} } } let name=bridge.as_ref().and_then(|b|b.anchor_name()); vec![json!({"id":r.id,"label":name.unwrap_or(&r.label),"anchor_name":name,"url":r.url,"platform":r.platform,"control":bridge.as_ref().map(|b|b.control_status())})] }).collect();
            if let Some(hub)=&hub {rows.extend(hub.rows());}
            return Ok(json!({"rooms":rows,"userscript_install_url":install_url,"downloads":if let Some(t)=&recordings{t.list().await}else{vec![]}}));
        }
        if method=="GET"&&path=="/api/deepseek-key" { return super::deepseek::key_status(); }
        if method!="POST"||headers.get("content-type").map(String::as_str)!=Some("application/json"){bail!("invalid action");}
        let body:Value=serde_json::from_slice(&data[end+4..])?;
        if path=="/api/deepseek-key" {
            return match body["action"].as_str() {
                Some("save") => super::deepseek::save_key(body["key"].as_str().context("请输入 API Key")?),
                Some("delete") => super::deepseek::delete_key(),
                _ => bail!("invalid action"),
            };
        }
        let id=body["id"].as_str().context("id required")?;
        if path=="/api/deepseek-reply" {
            let room_exists=rooms.iter().any(|r|r.id==id || r.bridge.lock().ok().and_then(|b|b.dashboard_connection(&r.id,id)).is_some()) || hub.as_ref().is_some_and(|h|h.rows().iter().any(|r|r["id"]==id));
            if !room_exists { bail!("unknown room"); }
            return super::deepseek::suggest(&body).await;
        }
        if path=="/api/download" {
            let tasks=recordings.as_ref().context("recording unavailable")?;
            let task=match body["action"].as_str(){Some("pause")=>tasks.pause(id).await,Some("start")=>tasks.start(id).await,Some("delete")=>tasks.delete(id).await,_=>None};
            return Ok(json!({"task":task.context("unknown recording task or action")?}));
        }
        for room in rooms.iter().filter(|r|r.platform=="wechat_channels") {
            let mut bridge=room.bridge.lock().map_err(|_|anyhow::anyhow!("bridge unavailable"))?;
            if let Some(connection)=bridge.dashboard_connection(&room.id,id) {
                return match path.as_str(){
                    "/api/moderation"=>bridge.dashboard_set_enabled(&connection,body["enabled"].as_bool().context("enabled required")?),
                    "/api/timeline"=>bridge.dashboard_timeline(&connection),
                    "/api/reply-suggestions"=>bridge.dashboard_replies(&connection,body["entry_id"].as_str().context("entry_id required")?),
                    "/api/speech"=>bridge.dashboard_speech(&connection,body["enabled"].as_bool().context("enabled required")?),
                    "/api/record"=>bridge.dashboard_record(&connection),
                    "/api/send-message"=>bridge.dashboard_send(Some(&connection),body["text"].as_str().context("text required")?),
                    "/api/room-settings"=>bridge.dashboard_settings(&connection),
                    "/api/room-rules"=>{let identity=bridge.dashboard_identity(&connection).context("直播间身份尚未识别")?;if body["room_id"].as_str()!=Some(identity.as_str()){bail!("直播间身份不匹配");}bridge.patch_room_rules(&identity,serde_json::from_value(body["rules"].clone())?,body["revision"].as_str())},
                    _=>bail!("unknown action")
                };
            }
        }
        if !rooms.iter().any(|r|r.id==id) {
            let hub=hub.as_ref().context("unknown room")?;
            return match path.as_str() {"/api/moderation"=>hub.action(id,Some(body["enabled"].as_bool().context("enabled required")?)),"/api/record"=>hub.action(id,None),"/api/send-message"=>hub.send_message(id,body["text"].as_str().context("text required")?),"/api/timeline"=>hub.timeline(id),"/api/room-settings"=>hub.room_settings(id),"/api/room-rules"=>hub.patch_room_rules(id,body["room_id"].as_str().context("room_id required")?,serde_json::from_value(body["rules"].clone())?,body["revision"].as_str()),"/api/reply-suggestions"=>hub.request_replies(id,body["entry_id"].as_str().context("entry_id required")?),"/api/speech"=>hub.speech_action(id,body["enabled"].as_bool().context("enabled required")?),_=>bail!("unknown action")};
        }
        let room=rooms.iter().find(|r|r.id==id).context("unknown room")?;
        let mut bridge=room.bridge.lock().map_err(|_|anyhow::anyhow!("bridge unavailable"))?;
        if room.platform=="wechat_channels" && !bridge.dashboard_rooms(&room.id,&room.label,&room.url).is_empty() { bail!("请选择具体直播间"); }
        match path.as_str(){"/api/moderation"=>bridge.set_moderation_enabled(body["enabled"].as_bool().context("enabled required")?),"/api/record"=>bridge.request_recording(),"/api/send-message"=>bridge.dashboard_send(None,body["text"].as_str().context("text required")?),"/api/timeline"=>Ok(bridge.timeline()),"/api/room-settings"=>Ok(bridge.room_settings()),"/api/room-rules"=>bridge.patch_room_rules(body["room_id"].as_str().context("room_id required")?,serde_json::from_value(body["rules"].clone())?,body["revision"].as_str()),"/api/reply-suggestions"=>bridge.request_replies(body["entry_id"].as_str().context("entry_id required")?),"/api/speech"=>bridge.request_speech(body["enabled"].as_bool().context("enabled required")?),_=>bail!("unknown action")}
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
