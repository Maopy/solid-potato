//! YAML -> authenticated room bridge -> timed and gift queues -> SQLite.
use live_archive_core::moderation::{
    bridge::ProbeBridge, config::ModerationConfig, http::ProbeServer, journal::ProbeJournal,
    protocol::ProbeScope,
};
use serde_json::{Value, json};
use std::time::Duration;
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
    setup_threshold(dir, 0).await
}
async fn setup_threshold(dir: &std::path::Path, min_comments: u64) -> (ProbeServer, Client) {
    let path = dir.join("room.yml");
    std::fs::write(&path,"version: 1\nmode: candidate_fill_only\nroom:\n  platform: wechat_channels\n  adapter: wechat_channels.desktop_probe\n  url: https://channels.weixin.qq.com/web/pages/live\ncandidate_preview:\n  enabled: true\n  recipient_user_id: host\nsending:\n  automatic: true\ntimed_messages:\n  enabled: true\n  interval_seconds: 1\n  messages: ['定时甲', '定时乙']\nrules:\n  gift:\n    emojis: []\n").unwrap();
    let mut cfg = ModerationConfig::load(&path).unwrap();
    cfg.timed_messages.min_comments = min_comments;
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
    b.configure_timed(cfg.timed_messages).unwrap();
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
fn gift() -> Value {
    json!({"event_type":"gift.candidate","source_kind":"network","user_id":"user","native_event_id":"gift","nickname":"观众","payload":{"decoder":"wechat.app20009.reward.candidate.v1","message_type":20009,"gift_id":"rose","gift_name":"玫瑰","reward_product_count":1,"recipient_user_id":"host"}})
}
#[tokio::test]
async fn timed_messages_wait_for_collection_choose_nonrepeating_text_and_share_send_budget() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup(dir.path()).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let first = c.poll(true).await;
    assert_eq!(first["commands"], json!([]));
    assert_eq!(first["timed_messages"]["active"], false);
    c.active(true).await;
    assert_eq!(c.poll(true).await["commands"], json!([]));
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let first = c.poll(true).await;
    let a = first["commands"][0].clone();
    assert!(a["action_id"].as_str().unwrap().starts_with("timed-"));
    c.receipt(&a, false).await;
    assert_eq!(c.poll(true).await["commands"], json!([]));
    tokio::time::sleep(Duration::from_millis(8100)).await;
    let second = c.poll(true).await;
    let b = second["commands"][0].clone();
    assert_ne!(a["text"], b["text"]);
    c.receipt(&b, false).await;
    c.active(false).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(c.poll(true).await["timed_messages"]["active"], false);
    c.active(true).await;
    let resumed = c.poll(true).await;
    assert!(resumed["timed_messages"]["remaining_ms"].as_u64().unwrap() > 900);
    assert_eq!(resumed["commands"], json!([]));
    server.bridge.lock().unwrap().begin_drain();
    assert_eq!(c.poll(true).await["commands"], json!([]));
    server.stop().await.unwrap();
    let db = rusqlite::Connection::open(dir.path().join("journal.sqlite3")).unwrap();
    let n: i64 = db
        .query_row(
            "select count(*) from fill_audits where json_extract(audit_json,'$.state')='issued'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 2);
}
#[tokio::test]
async fn pending_gifts_take_precedence_and_busy_input_never_accumulates_timers_and_unknown_does_not_pause()
 {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup(dir.path()).await;
    c.active(true).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let busy = c.poll(false).await;
    assert_eq!(busy["commands"], json!([]));
    assert_eq!(busy["timed_messages"]["generated"], 0);
    c.events(vec![gift()]).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let waiting = c.poll(true).await;
    assert_eq!(waiting["commands"], json!([]));
    assert_eq!(waiting["timed_messages"]["generated"], 0);
    tokio::time::sleep(Duration::from_millis(4100)).await;
    let ready = c.poll(true).await;
    let cmd = &ready["commands"][0];
    assert_eq!(cmd["text"], "感谢观众送来的玫瑰");
    assert!(cmd["action_id"].as_str().unwrap().starts_with("candidate-"));
    c.receipt(cmd, true).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let blocked = c.poll(true).await;
    assert_eq!(blocked["fill"]["blocked"], false);
    assert_eq!(blocked["commands"], json!([]));
    assert_eq!(blocked["timed_messages"]["generated"], 0);
    assert_eq!(blocked["fill"]["pending"], 0);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn comment_threshold_uses_fresh_nonkeyword_chat_and_both_gates() {
    let dir = tempfile::tempdir().unwrap();
    let (mut server, mut c) = setup_threshold(dir.path(), 2).await;
    c.active(true).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(c.poll(true).await["commands"], json!([]));
    let comment = |id: &str| json!({"event_type":"chat.activity","source_kind":"network","user_id":"viewer","native_event_id":id,"payload":{"content":"随便聊聊","decoder":"wechat.msg_list.activity.v1","message_type":1,"keyword_live":true,"observed_at_ms":chrono::Utc::now().timestamp_millis()}});
    c.events(vec![comment("one"), comment("one")]).await;
    let waiting = c.poll(true).await;
    assert_eq!(waiting["commands"], json!([]));
    assert_eq!(waiting["timed_messages"]["comments_since_last"], 1);
    c.events(vec![comment("two")]).await;
    let ready = c.poll(true).await;
    assert_eq!(ready["commands"].as_array().unwrap().len(), 1);
    assert_eq!(ready["timed_messages"]["comments_since_last"], 0);
    assert_eq!(c.poll(true).await["commands"], json!([]));
    server.stop().await.unwrap();
}
