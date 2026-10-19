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
    b.configure(cfg.rules_config(), cfg.scheduler.clone())
        .unwrap();
    b.configure_candidate_preview(cfg.candidate_preview)
        .unwrap();
    b.enable_candidate_fill_preview(None).unwrap();
    b.enable_automatic_send().unwrap();
    b.configure_keywords(cfg.keyword_replies).unwrap();
    let script = b.bootstrap(&b.relay_token.clone()).unwrap();
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
