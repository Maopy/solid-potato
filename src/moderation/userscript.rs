//! Authenticated userscript transport. Each room owns its bridge; account budgets are shared.
use super::recording::RecordingRequest;
use super::{
    account_budget::AccountBudget,
    bridge::ProbeBridge,
    http,
    journal::ProbeJournal,
    protocol::{Poll, ProbeScope},
    service::ProbeOptions,
};
use crate::{download_task_bridge::DownloadTaskBridge, model::Platform};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, mpsc, watch},
    task::JoinHandle,
};

fn default_port() -> u16 {
    2025
}
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UserscriptConfig {
    pub enabled: bool,
    pub port: u16,
    pub key_file: Option<PathBuf>,
}
impl Default for UserscriptConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_port(),
            key_file: None,
        }
    }
}
struct Template {
    id: String,
    options: ProbeOptions,
    budget: Arc<Mutex<AccountBudget>>,
}
pub struct Room {
    pub id: String,
    pub bridge: Arc<Mutex<ProbeBridge>>,
}
struct State {
    templates: Vec<Template>,
    rooms: BTreeMap<String, Arc<Room>>,
    directory: PathBuf,
    recording: bool,
    stopping: bool,
}
#[derive(Clone)]
pub struct UserscriptHub(Arc<Mutex<State>>);
impl UserscriptHub {
    pub fn new(directory: PathBuf, recording: bool) -> Self {
        Self(Arc::new(Mutex::new(State {
            templates: vec![],
            rooms: BTreeMap::new(),
            directory,
            recording,
            stopping: false,
        })))
    }
    pub fn add_template(
        &self,
        id: &str,
        _account: &str,
        options: ProbeOptions,
        budget: Arc<Mutex<AccountBudget>>,
    ) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        // A browser profile has one logged-in Douyin account. Explicitly reject ambiguous templates.
        if !s.templates.is_empty() {
            bail!("userscript mode currently requires exactly one Douyin template/account");
        }
        s.templates.push(Template {
            id: id.into(),
            options,
            budget,
        });
        Ok(())
    }
    pub fn bootstrap(&self, page: &str) -> Result<Value> {
        let scope = ProbeScope::douyin(url::Url::parse(page)?)?;
        let key = scope.room_key.clone();
        let mut s = self.0.lock().unwrap();
        if s.stopping {
            bail!("userscript service stopping");
        }
        if !s.rooms.contains_key(&key) {
            if s.rooms.len() >= 32 {
                bail!("最多管理 32 个抖音直播间，请重启场控后重新连接");
            }
            let t = s
                .templates
                .first()
                .context("no Douyin template configured")?;
            let id = format!("{}-{}", &t.id[..t.id.len().min(30)], key);
            let dir = s.directory.join("userscript-rooms").join(&key);
            std::fs::create_dir_all(&dir)?;
            let mut b = ProbeBridge::new(
                scope,
                ProbeJournal::open(&dir.join("moderation-probe.sqlite3"))?,
            );
            configure_bridge(&mut b, &id, &t.options, t.budget.clone(), s.recording)?;
            s.rooms.insert(
                key.clone(),
                Arc::new(Room {
                    id,
                    bridge: Arc::new(Mutex::new(b)),
                }),
            );
        }
        let room = s.rooms[&key].clone();
        drop(s);
        let mut b = room.bridge.lock().unwrap();
        let relay = b.relay_token.clone();
        b.bootstrap_config(&relay)
    }
    pub fn room(&self, page: &str) -> Result<Arc<Room>> {
        let scope = ProbeScope::douyin(url::Url::parse(page)?)?;
        let s = self.0.lock().unwrap();
        s.rooms
            .get(&scope.room_key)
            .cloned()
            .context("room not paired")
    }
    pub fn rows(&self) -> Vec<Value> {
        let s = self.0.lock().unwrap();
        s.rooms.values().map(|r| { let b = r.bridge.lock().unwrap(); json!({"id":r.id,"label":format!("直播间 {}",b.scope.room_key),"url":b.scope.room_url,"platform":"douyin","transport":"userscript","control":b.control_status()}) }).collect()
    }
    pub fn action(&self, id: &str, enabled: Option<bool>) -> Result<Value> {
        let room = self
            .0
            .lock()
            .unwrap()
            .rooms
            .values()
            .find(|r| r.id == id)
            .cloned()
            .context("unknown room")?;
        let mut b = room.bridge.lock().unwrap();
        match enabled {
            Some(v) => b.set_moderation_enabled(v),
            None => b.request_recording(),
        }
    }
    pub fn dispatch(&self, page: &str, endpoint: &str, bearer: &str, body: Value) -> Result<Value> {
        let room = self.room(page)?;
        let mut b = room.bridge.lock().unwrap();
        if let Some(page) = body["page_url"]
            .as_str()
            .or_else(|| body["scope"]["page_url"].as_str())
        {
            if !b.scope.matches(page) {
                bail!("request room mismatch");
            }
        }
        match endpoint {
            "hello" => b.hello(serde_json::from_value(body)?),
            "control" => b.control(bearer, body),
            "speech" => b.speech(bearer, body),
            "likes/claim" => b.claim_like_period(bearer, serde_json::from_value(body)?),
            "events" => b.events(bearer, serde_json::from_value(body)?),
            "raw" => b.raw_messages(bearer, serde_json::from_value(body)?),
            "poll" => b.poll(bearer, serde_json::from_value(body)?),
            "results" => b.fill_result(bearer, serde_json::from_value(body)?),
            "delivery" => b.delivery_result(bearer, serde_json::from_value(body)?),
            "goodbye" => b.goodbye(bearer, serde_json::from_value(body)?),
            _ => bail!("unsupported userscript endpoint"),
        }
    }
    pub fn authorize_recording(&self, page: &str, bearer: &str, scope: Value) -> Result<()> {
        let room = self.room(page)?;
        let mut b = room.bridge.lock().unwrap();
        b.authorize_scope(bearer, serde_json::from_value::<Poll>(scope)?)
    }
    pub async fn stop(&self) -> Result<()> {
        let rooms = {
            let mut s = self.0.lock().unwrap();
            s.stopping = true;
            s.rooms.values().cloned().collect::<Vec<_>>()
        };
        for r in &rooms {
            r.bridge.lock().unwrap().begin_drain();
        }
        // Requests remain available for final ACKs during drain (dispatch uses the existing room).
        let until = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < until
            && rooms
                .iter()
                .any(|r| !r.bridge.lock().unwrap().drain_finished())
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for r in &rooms {
            r.bridge.lock().unwrap().maintain_journal(true)?;
        }
        Ok(())
    }
}
fn configure_bridge(
    b: &mut ProbeBridge,
    id: &str,
    o: &ProbeOptions,
    budget: Arc<Mutex<AccountBudget>>,
    recording: bool,
) -> Result<()> {
    b.configure_task(id, budget)?;
    b.configure_ui(recording, true);
    b.set_moderation_enabled(false)?;
    b.configure_delivery(o.delivery.clone())?;
    b.configure_timed(o.timed_messages.clone())?;
    b.configure_likes(o.likes.clone())?;
    b.configure_keywords(o.keyword_replies.clone())?;
    b.configure_retention(o.retention.clone())?;
    b.maintain_journal(true)?;
    b.configure_raw_messages(o.raw_messages)?;
    b.configure(o.rules_config.clone(), o.scheduler_config.clone())?;
    b.configure_candidate_preview(o.candidate_preview.clone())?;
    if o.candidate_fill_only {
        b.enable_candidate_fill_preview(o.fill_test_text.clone())?;
    } else if o.fill_preview {
        b.enable_fill_preview(o.fill_test_text.clone())?;
    } else if o.scheduler_preview {
        b.enable_scheduler_preview();
    } else if o.rules_preview {
        b.enable_rules_preview();
    }
    if o.automatic_send {
        b.enable_automatic_send()?;
    }
    Ok(())
}

pub struct UserscriptServer {
    pub install_url: String,
    stop: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}
impl UserscriptServer {
    pub async fn bind(
        config: &UserscriptConfig,
        directory: &Path,
        hub: UserscriptHub,
        tasks: Option<DownloadTaskBridge>,
        recording: Option<mpsc::Sender<RecordingRequest>>,
    ) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", config.port))
            .await
            .context("userscript port occupied")?;
        let address = listener.local_addr()?;
        let key = persistent_key(
            config
                .key_file
                .as_deref()
                .unwrap_or(&directory.join("userscript.key")),
        )?;
        let install = uuid::Uuid::new_v4().simple().to_string();
        let install_path = format!("/install/{install}/live-archive-douyin.user.js");
        let source = render_script(&format!("http://{address}"), &key);
        let install_url = format!("http://{address}{install_path}");
        let (stop, mut stopped) = watch::channel(false);
        let worker = tokio::spawn(async move {
            let limit = Arc::new(Semaphore::new(32));
            let mut clients = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = stopped.changed() => break,
                    Some(_) = clients.join_next(), if !clients.is_empty() => {},
                    accepted = listener.accept() => {
                        let Ok((mut socket, peer)) = accepted else { break; };
                        if !peer.ip().is_loopback() { continue; }
                        let Ok(permit) = limit.clone().try_acquire_owned() else { continue; };
                        let (key, path, source, hub, tasks, recording) = (key.clone(), install_path.clone(), source.clone(), hub.clone(), tasks.clone(), recording.clone());
                        clients.spawn(async move { let _permit = permit;
                            let _ = tokio::time::timeout(Duration::from_secs(4), async {
                                let r = match http::read_request(&mut socket, address).await { Ok(r)=>r, Err(_)=>return http::respond(&mut socket,400,"application/json",b"{}").await };
                                if r.method == "GET" && r.path == path {
                                    return http::respond(&mut socket,200,"text/javascript; charset=utf-8",source.as_bytes()).await;
                                }
                                if r.method != "POST" || r.path != "/v1/userscript" || r.bearer != key || (!r.origin.is_empty() && r.origin != "https://live.douyin.com" && !r.origin.starts_with("moz-extension://") && !r.origin.starts_with("chrome-extension://")) {
                                    return http::respond(&mut socket,403,"application/json",b"{\"error\":\"userscript_auth_denied\"}").await;
                                }
                                let result = request(&hub, tasks, recording, &r.body).await;
                                let (status,value) = match result { Ok(v)=>(200,v),Err(e)=>(422,json!({"error":e.to_string()})) };
                                http::respond(&mut socket,status,"application/json",&serde_json::to_vec(&value)?).await
                            }).await;
                        });
                    }
                }
            }
            while clients.join_next().await.is_some() {}
        });
        Ok(Self {
            install_url,
            stop,
            worker: Some(worker),
        })
    }
    pub async fn stop(mut self) {
        let _ = self.stop.send(true);
        if let Some(worker) = self.worker.take() {
            let _ = worker.await;
        }
    }
}
impl Drop for UserscriptServer {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}
fn persistent_key(path: &Path) -> Result<String> {
    if path.exists() {
        let key = std::fs::read_to_string(path)?;
        if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("invalid userscript pairing key");
        }
        return Ok(key);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let key = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    options.open(path)?.write_all(key.as_bytes())?;
    Ok(key)
}
async fn request(
    hub: &UserscriptHub,
    tasks: Option<DownloadTaskBridge>,
    recording: Option<mpsc::Sender<RecordingRequest>>,
    bytes: &[u8],
) -> Result<Value> {
    let v: Value = serde_json::from_slice(bytes)?;
    let page = v["room_url"].as_str().context("room_url required")?;
    let endpoint = v["endpoint"].as_str().context("endpoint required")?;
    if endpoint == "bootstrap" {
        return hub.bootstrap(page);
    }
    let bearer = v["session_token"].as_str().unwrap_or("");
    if endpoint != "recording" {
        return hub.dispatch(page, endpoint, bearer, v["body"].clone());
    }
    hub.authorize_recording(page, bearer, v["scope"].clone())?;
    let tasks = tasks.context("recording unavailable")?;
    match v["action"].as_str().context("recording action required")? {
        "list" => Ok(json!({"list":tasks.recording_list().await})),
        "create" => {
            if tasks.list().await.len() >= 512 {
                bail!("recording task limit reached");
            }
            let object = &v["body"]["objects"][0];
            if object["platform"] != "douyin"
                || object["content"]["profile"]["room_url"].as_str() != Some(page)
            {
                bail!("recording scope mismatch");
            }
            let stream_url = url::Url::parse(
                object["content"]["live"]["stream_url"]
                    .as_str()
                    .context("missing stream URL")?,
            )?;
            if !matches!(stream_url.scheme(), "http" | "https")
                || stream_url.host_str().is_none()
                || !stream_url.username().is_empty()
                || stream_url.password().is_some()
            {
                bail!("invalid media URL");
            }
            let title = object["content"]["profile"]["nickname"]
                .as_str()
                .unwrap_or("抖音直播")
                .chars()
                .take(128)
                .collect::<String>();
            let id = uuid::Uuid::new_v4().to_string();
            recording
                .context("recording unavailable")?
                .try_send(RecordingRequest {
                    id: id.clone(),
                    platform: Platform::Douyin,
                    page_url: url::Url::parse(page)?,
                    stream_url,
                    title: title.clone(),
                })
                .context("recording queue unavailable")?;
            // The worker publishes the task, so this response cannot overwrite
            // a fast failure or an already-started recording with "waiting".
            Ok(json!({"task_id":id}))
        }
        action @ ("pause" | "start" | "delete") => {
            let ids = v["body"]["task_ids"]
                .as_array()
                .context("task_ids required")?;
            if ids.len() > 64 {
                bail!("too many task IDs");
            }
            for id in ids {
                let id = id.as_str().context("invalid task ID")?;
                match action {
                    "pause" => {
                        tasks.pause(id).await;
                    }
                    "start" => {
                        tasks.start(id).await;
                    }
                    _ => {
                        tasks.delete(id).await;
                    }
                }
            }
            Ok(json!({}))
        }
        _ => bail!("unsupported recording action"),
    }
}
pub fn render_script(base: &str, key: &str) -> String {
    include_str!("../../assets/moderation/userscript/douyin.user.js")
        .replace("__BASE_JSON__", &serde_json::to_string(base).unwrap())
        .replace("__KEY_JSON__", &serde_json::to_string(key).unwrap())
        .replace("/*__SDK__*/", super::SDK)
        .replace("/*__RAW__*/", super::RAW_CAPTURE)
        .replace("/*__SUBMIT__*/", super::AUTO_SEND)
        .replace("/*__DECODER__*/", super::DOUYIN_DECODER)
        .replace("/*__ADAPTER__*/", super::DOUYIN_ADAPTER)
        .replace("/*__FILL__*/", super::DOUYIN_FILL)
        .replace("/*__RECORDING__*/", super::DOUYIN_RECORDING)
        .replace("/*__PANEL__*/", super::PANEL)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn recording_uses_the_authenticated_page_and_rejects_cross_room_submissions() {
        let dir = tempfile::tempdir().unwrap();
        let hub = UserscriptHub::new(dir.path().into(), true);
        hub.add_template(
            "dy",
            "account",
            ProbeOptions::default(),
            Arc::new(Mutex::new(AccountBudget::new(Default::default()).unwrap())),
        )
        .unwrap();
        let page = "https://live.douyin.com/123";
        let boot = hub.bootstrap(page).unwrap();
        let h=hub.dispatch(page,"hello","",json!({"v":1,"ticket":boot["ticket"],"document_id":"doc","page_url":page,"adapter_id":"douyin.live_page"})).unwrap();
        let scope = json!({"v":1,"connection_id":h["connection_id"],"binding_id":h["binding_id"],"page_url":page});
        let (tx, mut rx) = mpsc::channel(4);
        let tasks = DownloadTaskBridge::default();
        let mut body = json!({"room_url":page,"endpoint":"recording","action":"create","session_token":h["token"],"scope":scope,"body":{"objects":[{"platform":"douyin","content":{"profile":{"room_url":page,"nickname":"测试主播"},"live":{"stream_url":"https://media.example.test/live.flv"}}}]}});
        body["body"]["objects"][0]["content"]["profile"]["room_url"] =
            json!("https://live.douyin.com/456");
        assert!(
            request(
                &hub,
                Some(tasks.clone()),
                Some(tx.clone()),
                body.to_string().as_bytes()
            )
            .await
            .is_err()
        );
        assert!(rx.try_recv().is_err());
        body["body"]["objects"][0]["content"]["profile"]["room_url"] = json!(page);
        let value = request(
            &hub,
            Some(tasks.clone()),
            Some(tx.clone()),
            body.to_string().as_bytes(),
        )
        .await
        .unwrap();
        let recorded = rx.recv().await.unwrap();
        assert_eq!(recorded.id, value["task_id"]);
        assert_eq!(recorded.page_url.as_str(), page);
        assert_eq!(
            recorded.stream_url.as_str(),
            "https://media.example.test/live.flv"
        );
        assert!(tasks.list().await.is_empty());
        body["session_token"] = json!("wrong");
        assert!(
            request(&hub, Some(tasks), Some(tx), body.to_string().as_bytes())
                .await
                .is_err()
        );
        assert!(rx.try_recv().is_err());
    }
}
