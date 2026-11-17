//! YAML -> authenticated room bridge -> timed and gift queues -> SQLite.
use live_archive_core::moderation::{
    bridge::ProbeBridge, config::ModerationConfig, http::ProbeServer, journal::ProbeJournal,
    protocol::ProbeScope,
};
use serde_json::{Value, json};

struct Client {
    http: reqwest::Client,
    base: String,
    hello: Value,
    seq: u64,
}
impl Client {
    async fn post(&self, path: &str, body: Value) -> Value {
        let r = self
            .http
            .post(format!("{}{path}", self.base))
            .header("Origin", "https://channels.weixin.qq.com")
            .bearer_auth(self.hello["token"].as_str().unwrap_or(""))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());
        serde_json::from_str(&r.text().await.unwrap()).unwrap()
    }
    fn scope(&self) -> Value {
        json!({"v":1,"connection_id":self.hello["connection_id"],"binding_id":self.hello["binding_id"],"page_url":"https://channels.weixin.qq.com/web/pages/live","input_ready":true})
    }
    async fn poll(&self, ready: bool) -> Value {
        let mut v = self.scope();
        v["input_ready"] = json!(ready);
        self.post("/v1/poll", v).await
    }
    async fn events(&mut self, mut events: Vec<Value>) {
        let start = self.seq + 1;
        for e in &mut events {
            self.seq += 1;
            e["seq"] = json!(self.seq);
            e["event_id"] = json!(format!("e{}", self.seq));
        }
        let body = json!({"v":1,"connection_id":self.hello["connection_id"],"binding_id":self.hello["binding_id"],"first_seq":start,"events":events});
        self.post("/v1/events", body.clone()).await;
        self.post("/v1/events", body).await; // repeated ACK does not reset the clock
    }
    async fn active(&mut self, active: bool) {
        self.events(vec![json!({"event_type":"probe.status","source_kind":"diagnostic","payload":{"stage":if active{"wechat_collection_started"}else{"wechat_collection_paused"}}})]).await;
    }
    async fn receipt(&self, command: &Value, unknown: bool) {
        let mut v = self.scope();
        v.as_object_mut().unwrap().remove("input_ready");
        v["action_id"] = command["action_id"].clone();
        v["outcome"] = json!(if unknown { "unknown" } else { "submitted" });
        v["reason"] = json!(if unknown {
            "submission_unconfirmed"
        } else {
            "input_submitted"
        });
        self.post("/v1/results", v.clone()).await;
        self.post("/v1/results", v).await;
    }
}
async fn setup(dir: &std::path::Path) -> (ProbeServer, Client) {
    let path = dir.join("room.yml");
    std::fs::write(&path,"version: 1\nmode: candidate_fill_only\nroom:\n  platform: wechat_channels\n  adapter: wechat_channels.desktop_probe\n  url: https://channels.weixin.qq.com/web/pages/live\ncandidate_preview:\n  enabled: true\n  recipient_user_id: host\nsending:\n  automatic: true\nkeyword_replies:\n  enabled: true\n  cooldown_seconds: 60\n  rules:\n    - keywords: ['胎记', '印记']\n      reply: 手臂是胎记\nrules:\n  gift:\n    emojis: []\n").unwrap();
    let cfg = ModerationConfig::load(&path).unwrap();
    let mut b = ProbeBridge::new(
        ProbeScope::from_room_url(cfg.room.url.clone()).unwrap(),
        ProbeJournal::open(&dir.join("journal.sqlite3")).unwrap(),
    );
    b.set_config_path(path.clone());
    b.configure(cfg.rules_config(), cfg.scheduler.clone())
        .unwrap();
    b.configure_candidate_preview(cfg.candidate_preview)
        .unwrap();
    b.enable_candidate_fill_preview(None).unwrap();
    b.enable_automatic_send().unwrap();
    b.configure_keywords(cfg.keyword_replies).unwrap();
    let script = b
        .bootstrap_for_room(&b.relay_token.clone(), Some("host"))
        .unwrap();
    let boot: Value = serde_json::from_str(
        script
            .split("const BOOT = ")
            .nth(1)
            .unwrap()
            .split(";\n")
            .next()
            .unwrap(),
    )
    .unwrap();
    let server = ProbeServer::bind(b).await.unwrap();
    let mut c = Client {
        http: reqwest::Client::builder().no_proxy().build().unwrap(),
        base: format!("http://{}", server.address),
        hello: Value::Null,
        seq: 0,
    };
    c.hello=c.post("/v1/hello",json!({"v":1,"ticket":boot["ticket"],"document_id":"timer-page","adapter_id":"wechat_channels.desktop_probe","page_url":"https://channels.weixin.qq.com/web/pages/live"})).await;
    (server, c)
}

fn chat(id: &str, text: &str) -> Value {
    json!({"event_type":"chat.keyword_candidate","source_kind":"network","user_id":"viewer","native_event_id":id,"payload":{"content":text,"decoder":"wechat.msg_list.keyword.v1","keyword_live":true,"message_type":1,"observed_at_ms":chrono::Utc::now().timestamp_millis()}})
}
#[tokio::test]
async fn room_message_settings_update_existing_session() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup(dir.path()).await;
    c.active(true).await;
    let config = serde_json::from_value(
        json!({"enabled":true,"rules":[{"keywords":["价格"],"reply":"请看介绍"}]}),
    )
    .unwrap();
    let timed = Default::default();
    assert_eq!(
        server
            .bridge
            .lock()
            .unwrap()
            .update_room_messages("host", timed, config)
            .unwrap(),
        1
    );
    c.events(vec![chat("old", "胎记")]).await;
    assert_eq!(c.poll(true).await["commands"], json!([]));
    let mut fresh = chat("new", "价格多少");
    fresh["event_type"] = json!("chat.activity");
    fresh["payload"]["decoder"] = json!("wechat.msg_list.activity.v1");
    c.events(vec![fresh]).await;
    assert_eq!(c.poll(true).await["commands"][0]["text"], "请看介绍");
    server.stop().await.unwrap();
}
#[tokio::test]
async fn message_settings_save_and_reload_for_one_room() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup(dir.path()).await;
    c.active(true).await;
    let patch = serde_json::from_value(json!({"timed_messages":{"enabled":true,"interval_seconds":20,"min_comments":0,"count_scope":"comments","messages":["欢迎大家"]},"keyword_replies":{"enabled":true,"cooldown_seconds":60,"rules":[{"keywords":["价格"],"reply":"请看介绍"}]},"likes":{"enabled":true,"mode":"continuous","interval_ms":1300}})).unwrap();
    server
        .bridge
        .lock()
        .unwrap()
        .patch_room_rules("host", patch, None)
        .unwrap();
    assert_eq!(c.poll(true).await["timed_messages"]["interval_seconds"], 20);
    assert_eq!(c.poll(true).await["likes_config"]["interval_ms"], 1300);
    let cfg = ModerationConfig::load(&dir.path().join("room.yml")).unwrap();
    assert_eq!(
        cfg.resolved_room_profiles["host"].timed.messages,
        ["欢迎大家"]
    );
    assert_eq!(
        cfg.resolved_room_profiles["host"].keywords.rules[0].reply,
        "请看介绍"
    );
    assert_eq!(cfg.resolved_room_profiles["host"].likes.interval_ms, 1300);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn external_yaml_edits_apply_to_connected_room_and_invalid_edits_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup(dir.path()).await;
    c.active(true).await;
    let path = dir.path().join("room.yml");
    let original = std::fs::read_to_string(&path).unwrap();
    let changed = original
        .replace("['胎记', '印记']", "['价格']")
        .replace("reply: 手臂是胎记", "reply: 请看介绍");
    assert_ne!(changed, original);
    std::fs::write(&path, &changed).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    c.poll(true).await;
    c.events(vec![chat("new", "价格多少")]).await;
    let command = c.poll(true).await["commands"][0].clone();
    assert_eq!(command["text"], "请看介绍");
    c.receipt(&command, false).await;
    std::fs::write(&path, "not: [valid").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let status = c.poll(true).await;
    assert!(
        status["moderation_control"]["settings_error"]
            .as_str()
            .unwrap()
            .contains("配置未生效")
    );
    std::fs::write(&path, changed).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert!(c.poll(true).await["moderation_control"]["settings_error"].is_null());
    server.stop().await.unwrap();
}
#[tokio::test]
async fn backend_matches_live_activity_without_page_keyword_filter() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup(dir.path()).await;
    c.active(true).await;
    let activity = |id: &str, text: &str| json!({"event_type":"chat.activity","source_kind":"network","user_id":"viewer","native_event_id":id,"payload":{"content":text,"decoder":"wechat.msg_list.activity.v1","keyword_live":true,"message_type":1,"observed_at_ms":chrono::Utc::now().timestamp_millis()}});
    c.events(vec![activity("ordinary", "你好")]).await;
    assert_eq!(c.poll(true).await["commands"], json!([]));
    c.events(vec![activity("matching", "这里是胎记吗")]).await;
    assert_eq!(c.poll(true).await["commands"][0]["text"], "手臂是胎记");
    server.stop().await.unwrap();
}
#[tokio::test]
async fn ordinary_keyword_reply_uses_queue_shared_cooldown_and_echo_filter() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup(dir.path()).await;
    c.events(vec![chat("old", "胎记")]).await;
    assert_eq!(c.poll(true).await["commands"], json!([]));
    c.active(true).await;
    c.events(vec![chat("first", "这个是胎记吗")]).await;
    let p = c.poll(true).await;
    let cmd = &p["commands"][0];
    assert_eq!(cmd["text"], "手臂是胎记");
    assert!(cmd["action_id"].as_str().unwrap().starts_with("keyword-"));
    c.receipt(cmd, false).await;
    c.events(vec![chat("second", "印记"), chat("self", "手臂是胎记")])
        .await;
    let p = c.poll(true).await;
    assert_eq!(p["commands"], json!([]));
    assert_eq!(p["keyword_replies"]["generated"], 1);
    c.active(false).await;
    assert_eq!(c.poll(true).await["keyword_replies"]["active"], false);
    server.stop().await.unwrap();
}
#[tokio::test]
async fn pause_cancels_a_keyword_waiting_for_input() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup(dir.path()).await;
    c.active(true).await;
    c.events(vec![chat("first", "胎记")]).await;
    assert_eq!(c.poll(false).await["commands"], json!([]));
    c.active(false).await;
    c.active(true).await;
    let p = c.poll(true).await;
    assert_eq!(p["commands"], json!([]));
    assert_eq!(p["fill"]["pending"], 0);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn switch_filters_off_events_and_never_accumulates_for_resume() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup(dir.path()).await;
    c.active(true).await;
    let off = c
        .post("/v1/control", json!({"scope":c.scope(),"enabled":false}))
        .await;
    assert_eq!(off["enabled"], false);
    c.events(vec![chat("off", "胎记")]).await;
    assert_eq!(c.poll(true).await["commands"], json!([]));
    c.post("/v1/control", json!({"scope":c.scope(),"enabled":true}))
        .await;
    c.active(true).await;
    assert_eq!(c.poll(true).await["commands"], json!([]));
    let mut observed = chat("off-delivered-after-resume", "胎记");
    observed["event_type"] = json!("chat.observed");
    c.events(vec![observed]).await;
    assert_eq!(
        c.poll(true).await["commands"],
        json!([]),
        "off-period observations never become replies after resume"
    );
    c.events(vec![chat("new", "胎记")]).await;
    let p = c.poll(true).await;
    assert_eq!(p["commands"][0]["text"], "手臂是胎记");
    let cmd = p["commands"][0].clone();
    c.post("/v1/control", json!({"scope":c.scope(),"enabled":false}))
        .await;
    // A receipt for an already issued click remains valid after switching off.
    c.receipt(&cmd, false).await;
    assert_eq!(c.poll(true).await["commands"], json!([]));
    server.stop().await.unwrap();
}
#[tokio::test]
async fn switch_discards_waiting_commands_and_is_authenticated() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup(dir.path()).await;
    c.active(true).await;
    c.events(vec![chat("waiting", "胎记")]).await;
    assert_eq!(c.poll(false).await["fill"]["pending"], 1);
    let bad = c
        .http
        .post(format!("{}/v1/control", c.base))
        .header("Origin", "https://channels.weixin.qq.com")
        .header("Content-Type", "application/json")
        .bearer_auth("wrong")
        .body(json!({"scope":c.scope(),"enabled":false}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 422);
    c.post("/v1/control", json!({"scope":c.scope(),"enabled":false}))
        .await;
    assert_eq!(c.poll(true).await["fill"]["pending"], 0);
    c.post("/v1/control", json!({"scope":c.scope(),"enabled":true}))
        .await;
    assert_eq!(c.poll(true).await["commands"], json!([]));
    server.stop().await.unwrap();
}
