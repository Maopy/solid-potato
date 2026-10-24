//! YAML -> authenticated local HTTP bridge -> candidate drafts -> SQLite.
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
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .header("Origin", "https://channels.weixin.qq.com")
            .header("Content-Type", "application/json")
            .bearer_auth(self.hello["token"].as_str().unwrap_or(""))
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            200,
            "{}",
            response.text().await.unwrap_or_default()
        );
        serde_json::from_str(&response.text().await.unwrap()).unwrap()
    }
    async fn events(&mut self, mut events: Vec<Value>) {
        let first = self.seq + 1;
        for event in &mut events {
            self.seq += 1;
            event["seq"] = json!(self.seq);
            event["event_id"] = json!(format!("e{}", self.seq));
        }
        let body = json!({"v":1,"connection_id":self.hello["connection_id"],"binding_id":self.hello["binding_id"],"first_seq":first,"events":events});
        assert_eq!(
            self.post("/v1/events", body.clone()).await["acked_seq"],
            self.seq
        );
        // Lost response retry must not reset deadlines or produce duplicates.
        assert_eq!(self.post("/v1/events", body).await["acked_seq"], self.seq);
    }
    async fn poll(&self) -> Value {
        let result=self.post("/v1/poll",json!({"v":1,"connection_id":self.hello["connection_id"],"binding_id":self.hello["binding_id"],"page_url":"https://channels.weixin.qq.com/web/pages/live"})).await;
        assert_eq!(result["commands"], json!([]));
        assert_eq!(result["previews"], json!([]));
        assert_eq!(result["send_chat"], false);
        result
    }
}
fn gift(user: &str, id: &str, name: &str, target: Option<&str>) -> Value {
    json!({"seq":1,"event_id":"fixture","event_type":"gift.candidate","source_kind":"network","user_id":user,"native_event_id":id,"nickname":user,
        "payload":{"decoder":"wechat.app20009.reward.candidate.v1","message_type":20009,"gift_id":"product","gift_name":name,"gift_name_truncated":false,"reward_product_count":1,"recipient_user_id":target}})
}
async fn setup(path: &std::path::Path) -> (ProbeServer, Client) {
    setup_with_rules(path, "", false).await
}
async fn setup_with_rules(
    path: &std::path::Path,
    rules: &str,
    fill: bool,
) -> (ProbeServer, Client) {
    let config_path = path.join("preview.yml");
    std::fs::write(&config_path,"version: 1\nmode: observe\nroom:\n  platform: wechat_channels\n  adapter: wechat_channels.desktop_probe\n  url: https://channels.weixin.qq.com/web/pages/live\ncandidate_preview:\n  enabled: true\n  recipient_user_id: selected-host\n").unwrap();
    if !rules.is_empty() {
        let source = std::fs::read_to_string(&config_path).unwrap() + rules;
        std::fs::write(&config_path, source).unwrap();
    }
    let config = ModerationConfig::load(&config_path).unwrap();
    let mut bridge = ProbeBridge::new(
        ProbeScope::from_room_url(config.room.url.clone()).unwrap(),
        ProbeJournal::open(&path.join("probe.sqlite3")).unwrap(),
    );
    if !rules.is_empty() {
        bridge
            .configure(config.rules_config(), config.scheduler.clone())
            .unwrap();
    }
    bridge.configure_room_profiles(config.resolved_room_profiles.clone());
    bridge
        .configure_candidate_preview(config.candidate_preview)
        .unwrap();
    if fill {
        bridge.enable_candidate_fill_preview(None).unwrap();
    }
    let script = bridge.bootstrap(&bridge.relay_token.clone()).unwrap();
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
    let server = ProbeServer::bind(bridge).await.unwrap();
    let mut client = Client {
        http: reqwest::Client::builder().no_proxy().build().unwrap(),
        base: format!("http://{}", server.address),
        hello: json!({}),
        seq: 0,
    };
    client.hello=client.post("/v1/hello",json!({"v":1,"ticket":boot["ticket"],"document_id":"fixture-document","adapter_id":"wechat_channels.desktop_probe","page_url":"https://channels.weixin.qq.com/web/pages/live"})).await;
    (server, client)
}
fn drafts(path: &std::path::Path) -> Vec<Value> {
    let db = rusqlite::Connection::open(path.join("probe.sqlite3")).unwrap();
    for table in ["preview_audits", "fill_audits"] {
        let count: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    let mut stmt = db
        .prepare("SELECT audit_json FROM candidate_draft_audits WHERE json_extract(audit_json,'$.kind')='gift_candidate_draft' ORDER BY id")
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect()
}
#[tokio::test]
async fn target_filter_user_debounce_overflow_revisions_and_durable_non_actionable_drafts() {
    let temp = tempfile::tempdir().unwrap();
    let (mut server, mut client) = setup(temp.path()).await;
    let long = "字".repeat(30);
    client
        .events(vec![
            gift("甲", "a1", "小星星", Some("selected-host")),
            gift("乙", "b1", "鲜花", Some("selected-host")),
            gift("丙", "c1", &long, Some("selected-host")),
            gift("旁人", "x1", "礼物", Some("pk-host")),
            gift("未知", "x2", "礼物", None),
        ])
        .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let mut revision = gift("乙", "b1", "鲜花", Some("selected-host"));
    revision["event_type"] = json!("gift.candidate_revision");
    revision["payload"]["reward_product_count"] = json!(2);
    client
        .events(vec![
            gift("甲", "a2", "鲜花", Some("selected-host")),
            gift("丙", "c2", "星", Some("selected-host")),
            revision,
            gift("乙", "b1", "鲜花", Some("selected-host")),
        ])
        .await;
    tokio::time::sleep(Duration::from_millis(2700)).await;
    client.poll().await;
    let logs = drafts(temp.path());
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["text"], "感谢乙送来的鲜花");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    client.poll().await;
    let logs = drafts(temp.path());
    assert_eq!(logs.len(), 3);
    let status = client.poll().await["candidate_preview"].clone();
    assert_eq!(status["recipient_user_id"], "selected-host");
    assert_eq!(status["pending_users"], 0);
    assert_eq!(status["counters"]["accepted"], 5);
    assert_eq!(status["counters"]["target_missing"], 1);
    assert_eq!(status["counters"]["target_mismatch"], 1);
    assert_eq!(status["counters"]["duplicate"], 1);
    assert_eq!(status["counters"]["revision_only"], 1);
    assert_eq!(status["counters"]["draft_generated"], 3);
    assert!(
        status["recent"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v.get("source_messages").is_none())
    );
    assert!(logs.iter().any(|v| v["text"] == "感谢甲送来的小星星、鲜花"));
    assert!(logs.iter().any(|v| v["text"] == "感谢礼物飘屏"));
    for log in logs {
        assert_eq!(
            log["source_messages"].as_array().unwrap().len() as u64,
            log["candidate_messages"].as_u64().unwrap()
        );
        assert_eq!(
            log["due_at_ms"].as_u64().unwrap() - log["last_candidate_at_ms"].as_u64().unwrap(),
            5000
        );
        assert!(log["at_ms"].as_u64().unwrap() >= log["due_at_ms"].as_u64().unwrap());
        assert_eq!(log["live_eligible"], false);
        assert_eq!(log["room_verified"], false);
        assert_eq!(log["quantity_mode"], "unknown");
        assert_eq!(log["fill_input"], false);
        assert_eq!(log["send_chat"], false);
    }
    // Separate explicit listing exposes target IDs only, not donor/name/content.
    let targets = live_archive_core::moderation::gift_report::inspect_targets(
        &temp.path().join("probe.sqlite3"),
    )
    .unwrap();
    assert_eq!(targets["targets"].as_array().unwrap().len(), 2);
    assert_eq!(targets["room_verified"], false);
    let output = targets.to_string();
    assert!(!output.contains("小星星"));
    assert!(!output.contains("旁人"));
    server.stop().await.unwrap();
}
#[tokio::test]
async fn pause_target_change_and_shutdown_discard_pending() {
    let temp = tempfile::tempdir().unwrap();
    let (mut server, mut client) = setup(temp.path()).await;
    client.events(vec![gift("甲","a1","星",Some("selected-host")),json!({"event_type":"probe.status","source_kind":"diagnostic","payload":{"stage":"wechat_collection_paused"}})]).await;
    client.events(vec![json!({"event_type":"probe.status","source_kind":"diagnostic","payload":{"stage":"wechat_collection_started"}})]).await;
    client
        .events(vec![gift("乙", "b1", "星", Some("selected-host"))])
        .await;
    let mut revision = gift("乙", "b1", "星", Some("other-host"));
    revision["event_type"] = json!("gift.candidate_revision");
    client.events(vec![revision]).await;
    tokio::time::sleep(Duration::from_millis(5100)).await;
    client.poll().await;
    assert!(drafts(temp.path()).is_empty());
    client
        .events(vec![gift("丙", "c1", "星", Some("selected-host"))])
        .await;
    server.bridge.lock().unwrap().begin_drain();
    tokio::time::sleep(Duration::from_millis(5100)).await;
    assert_eq!(client.poll().await["draining"], true);
    assert!(drafts(temp.path()).is_empty());
    server.stop().await.unwrap();
}
#[test]
fn config_fails_closed_for_missing_target_and_action_or_non_injection_modes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.yml");
    let base = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/moderation.wechat.example.yml"
    ))
    .unwrap()
    .replace("enabled: false", "enabled: true");
    std::fs::write(&path, &base).unwrap();
    assert!(ModerationConfig::load(&path).is_err());
    let valid = format!("{base}\n  recipient_user_id: selected-host\n");
    std::fs::write(&path, &valid).unwrap();
    assert!(ModerationConfig::load(&path).is_ok());
    for source in [
        valid.replace("mode: observe", "mode: fill_only"),
        valid.replace("system_proxy: true", "system_proxy: true\n  tls_only: true"),
    ] {
        std::fs::write(&path, source).unwrap();
        assert!(ModerationConfig::load(&path).is_err());
    }
}

#[tokio::test]
async fn delayed_poll_separates_silent_windows_and_exact_30_chars_fit() {
    let temp = tempfile::tempdir().unwrap();
    let (mut server, mut client) = setup(temp.path()).await;
    let name = "字".repeat(30);
    client
        .events(vec![gift("甲", "a1", &name, Some("selected-host"))])
        .await;
    tokio::time::sleep(Duration::from_millis(5100)).await;
    let mut truncated = gift("甲", "a2", &name, Some("selected-host"));
    truncated["payload"]["gift_name_truncated"] = json!(true);
    client.events(vec![truncated]).await;
    let logs = drafts(temp.path());
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["text"], format!("感谢甲送来的{name}"));
    tokio::time::sleep(Duration::from_millis(5100)).await;
    client.poll().await;
    let logs = drafts(temp.path());
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[1]["text"], "感谢礼物飘屏");
    // An old source ID cannot create another draft after the first window closed.
    client
        .events(vec![gift("甲", "a1", &name, Some("selected-host"))])
        .await;
    client.poll().await;
    assert_eq!(drafts(temp.path()).len(), 2);
    server.stop().await.unwrap();
}

#[test]
fn anonymized_recorded_pk_candidates_replay_to_durable_name_only_drafts() {
    use live_archive_core::moderation::{
        candidate_preview::CandidatePreview,
        protocol::{ProbeEvent, sanitize},
    };
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/wechat-gift-candidates-recorded.json"
    ))
    .unwrap();
    assert_eq!(fixture["room_verified"], false);
    let rows = fixture["events"].as_array().unwrap();
    assert_eq!(rows.len(), 17);
    let temp = tempfile::tempdir().unwrap();
    let mut journal = ProbeJournal::open(&temp.path().join("probe.sqlite3")).unwrap();
    let mut preview = CandidatePreview::new(fixture["selected_target"].as_str().unwrap().into());
    let mut logs = vec![];
    let mut last = 0;
    for row in rows {
        assert_eq!(row["document"], "document-1");
        let event: ProbeEvent = serde_json::from_value(row["event"].clone()).unwrap();
        event.validate().unwrap();
        let event = sanitize(&event);
        last = row["at_ms"].as_u64().unwrap();
        journal
            .append("recorded-replay", "document-1", &[event.clone()])
            .unwrap();
        logs.extend(preview.ingest(last, &event));
        logs.extend(preview.advance(last));
    }
    logs.extend(preview.advance(last + 5000));
    assert!(!logs.is_empty());
    assert_eq!(
        logs.iter()
            .map(|v| v["candidate_messages"].as_u64().unwrap())
            .sum::<u64>(),
        13
    );
    for log in &logs {
        assert_eq!(log["recipient_user_id"], "target-selected");
        assert_eq!(log["quantity_mode"], "unknown");
        assert_eq!(log["live_eligible"], false);
        assert_eq!(log["room_verified"], false);
        assert_eq!(log["fill_input"], false);
        assert_eq!(log["send_chat"], false);
        assert!(log["text"].as_str().unwrap().starts_with("感谢"));
    }
    journal
        .append_candidate_drafts("recorded-replay", "document-1", &logs)
        .unwrap();
    assert_eq!(drafts(temp.path()), logs);
    // Replay does not re-emit an already processed source message.
    let first: ProbeEvent = serde_json::from_value(rows[0]["event"].clone()).unwrap();
    assert!(preview.ingest(last + 10000, &sanitize(&first)).is_empty());
    assert!(preview.advance(last + 15000).is_empty());
    println!(
        "recorded replay: 17 candidates, 13 selected, {} drafts; send_chat=false",
        logs.len()
    );
}

fn cancellations(path: &std::path::Path) -> Vec<Value> {
    let db = rusqlite::Connection::open(path.join("probe.sqlite3")).unwrap();
    let mut stmt=db.prepare("SELECT audit_json FROM candidate_draft_audits WHERE json_extract(audit_json,'$.kind')='gift_candidate_cancelled' ORDER BY id").unwrap();
    stmt.query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect()
}
#[tokio::test]
async fn pending_countdown_pause_resume_gap_and_goodbye_have_traceable_cancellations() {
    let temp = tempfile::tempdir().unwrap();
    let (mut server, mut client) = setup(temp.path()).await;
    client
        .events(vec![gift("甲", "a1", "星", Some("selected-host"))])
        .await;
    let first = client.poll().await["candidate_preview"].clone();
    assert_eq!(first["pending_users"], 1);
    let remaining = first["pending"][0]["remaining_ms"].as_u64().unwrap();
    assert!(remaining > 0 && remaining <= 5000);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        client.poll().await["candidate_preview"]["pending"][0]["remaining_ms"]
            .as_u64()
            .unwrap()
            < remaining
    );
    client.events(vec![json!({"event_type":"probe.status","source_kind":"diagnostic","payload":{"stage":"wechat_collection_paused"}}),gift("甲","a2","星",Some("selected-host"))]).await;
    let status = client.poll().await["candidate_preview"].clone();
    assert_eq!(status["paused"], true);
    assert_eq!(status["pending_users"], 0);
    assert_eq!(status["counters"]["observation_paused_candidate"], 1);
    client.events(vec![json!({"event_type":"probe.status","source_kind":"diagnostic","payload":{"stage":"wechat_collection_started"}}),gift("甲","a1","星",Some("selected-host")),gift("乙","b1","花",Some("selected-host"))]).await;
    assert_eq!(
        client.poll().await["candidate_preview"]["counters"]["duplicate"],
        1
    );
    client.events(vec![json!({"event_type":"probe.gap","source_kind":"diagnostic","payload":{"reason":"page_buffer_full"}})]).await;
    assert_eq!(client.poll().await["candidate_preview"]["pending_users"], 0);
    client
        .events(vec![gift("丙", "c1", "花", Some("selected-host"))])
        .await;
    client.post("/v1/goodbye",json!({"v":1,"connection_id":client.hello["connection_id"],"binding_id":client.hello["binding_id"],"page_url":"https://channels.weixin.qq.com/web/pages/live"})).await;
    let records = cancellations(temp.path());
    assert_eq!(records.len(), 3);
    assert_eq!(
        records
            .iter()
            .map(|v| v["reason"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["observation_paused", "observation_gap", "document_stopped"]
    );
    for record in records {
        if record["reason"] != "document_stopped" {
            let db = rusqlite::Connection::open(temp.path().join("probe.sqlite3")).unwrap();
            let stored: String = db
                .query_row(
                    "SELECT event_json FROM probe_events WHERE seq=?1",
                    [record["trigger_event"]["seq"].as_u64().unwrap() as i64],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&stored).unwrap()["event_id"],
                record["trigger_event"]["event_id"]
            );
        }
        assert_eq!(record["source_messages"].as_array().unwrap().len(), 1);
        let source = &record["source_messages"][0];
        let db = rusqlite::Connection::open(temp.path().join("probe.sqlite3")).unwrap();
        let stored: String = db
            .query_row(
                "SELECT event_json FROM probe_events WHERE seq=?1",
                [source["seq"].as_u64().unwrap() as i64],
                |r| r.get(0),
            )
            .unwrap();
        let event: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(event["event_id"], source["event_id"]);
        assert_eq!(event["native_event_id"], source["native_event_id"]);
        assert_eq!(record["quantity_mode"], "unknown");
        assert_eq!(record["confidence"], "candidate");
        assert_eq!(record["send_chat"], false);
        assert_eq!(record["fill_input"], false);
    }
    assert!(drafts(temp.path()).is_empty());
    server.stop().await.unwrap();
}
#[tokio::test]
async fn expired_connection_cancels_pending_and_new_document_starts_fresh() {
    let temp = tempfile::tempdir().unwrap();
    let (mut server, mut client) = setup(temp.path()).await;
    client
        .events(vec![gift("甲", "a1", "星", Some("selected-host"))])
        .await;
    // No polls after a connection disappears: do not produce delayed thanks.
    tokio::time::sleep(Duration::from_millis(30100)).await;
    let script = {
        let mut bridge = server.bridge.lock().unwrap();
        assert_eq!(bridge.summary()["active_connections"], 0);
        let relay = bridge.relay_token.clone();
        bridge.bootstrap(&relay).unwrap()
    };
    assert!(drafts(temp.path()).is_empty());
    assert_eq!(
        cancellations(temp.path())[0]["reason"],
        "connection_expired"
    );
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
    let old = client.hello["connection_id"].clone();
    client.hello=client.post("/v1/hello",json!({"v":1,"ticket":boot["ticket"],"document_id":"new-document","adapter_id":"wechat_channels.desktop_probe","page_url":"https://channels.weixin.qq.com/web/pages/live"})).await;
    assert_ne!(old, client.hello["connection_id"]);
    client.seq = 0;
    client
        .events(vec![gift("甲", "a1", "星", Some("selected-host"))])
        .await;
    let status = client.poll().await["candidate_preview"].clone();
    assert_eq!(status["pending_users"], 1);
    assert_eq!(status["counters"]["accepted"], 1);
    assert!(status["recent"].as_array().unwrap().is_empty());
    server.bridge.lock().unwrap().begin_drain();
    assert_eq!(cancellations(temp.path()).len(), 2);
    assert_eq!(cancellations(temp.path())[1]["reason"], "service_stopped");
    server.stop().await.unwrap();
}

#[tokio::test]
async fn oversized_custom_suffix_draft_does_not_cancel_another_users_queued_thanks() {
    let temp = tempfile::tempdir().unwrap();
    let(mut server,mut client)=setup_with_rules(temp.path(),"rules:\n  gift:\n    template: '{nickname}{gift_list}'\n    overflow_template: '很长的溢出文案{nickname}'\n    emojis: ['🎁']\nlimits:\n  max_text_chars: 10\n",true).await;
    client
        .events(vec![gift("甲", "a1", "星", Some("selected-host"))])
        .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    client
        .events(vec![gift(
            "很长很长的观众昵称",
            "b1",
            "星",
            Some("selected-host"),
        )])
        .await;
    tokio::time::sleep(Duration::from_millis(2700)).await;
    assert_eq!(client.poll().await["fill"]["pending"], 1);
    tokio::time::sleep(Duration::from_millis(2600)).await;
    assert_eq!(client.poll().await["fill"]["pending"], 1);
    let c = rusqlite::Connection::open(temp.path().join("probe.sqlite3")).unwrap();
    let skipped:i64=c.query_row("select count(*) from candidate_draft_audits where json_extract(audit_json,'$.kind')='gift_candidate_skipped'",[],|r|r.get(0)).unwrap();
    assert_eq!(skipped, 1);
    let cancelled: i64 = c
        .query_row(
            "select count(*) from fill_audits where json_extract(audit_json,'$.state')='cancelled'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cancelled, 0);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn confirmed_same_message_growth_restarts_silence_window_but_display_summary_does_not() {
    let temp = tempfile::tempdir().unwrap();
    let (mut server, mut client) = setup(temp.path()).await;
    let mut first = gift("陌**", "gift-1", "玫瑰", Some("selected-host"));
    first["payload"]["combo_id"] = json!("combo-1");
    client.events(vec![first.clone()]).await;
    tokio::time::sleep(Duration::from_millis(2800)).await;
    let mut revision = first.clone();
    revision["event_type"] = json!("gift.candidate_revision");
    revision["payload"]["previous_gift_id"] = json!("product");
    revision["payload"]["previous_reward_product_count"] = json!(1);
    revision["payload"]["reward_product_count"] = json!(2);
    client.events(vec![revision]).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let mut summary = first.clone();
    summary["event_type"] = json!("gift.summary");
    summary["native_event_id"] = json!("display-1");
    summary["payload"]["message_type"] = json!(20013);
    summary["payload"]["decoder"] = json!("wechat.app20013.display.v1");
    client.events(vec![summary]).await;
    client.poll().await;
    assert!(
        drafts(temp.path()).is_empty(),
        "growth must postpone thanks beyond the original deadline"
    );
    tokio::time::sleep(Duration::from_millis(2700)).await;
    let status = client.poll().await;
    assert_eq!(
        status["candidate_preview"]["counters"]["revision_activity"],
        1
    );
    let logs = drafts(temp.path());
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["text"], "感谢陌**送来的玫瑰");
    assert_eq!(
        logs[0]["due_at_ms"].as_u64().unwrap() - logs[0]["last_candidate_at_ms"].as_u64().unwrap(),
        5000
    );
    server.stop().await.unwrap();
}

#[tokio::test]
async fn wechat_arrivals_reach_one_fill_command_after_thirty_seconds() {
    let temp = tempfile::tempdir().unwrap();
    let (mut server, mut client) =
        setup_with_rules(temp.path(), "rules:\n  welcome:\n    enabled: true\n", true).await;
    let arrival = |id: &str, user: &str| {
        json!({"seq":1,"event_id":"fixture","event_type":"member.candidate",
        "source_kind":"network","native_event_id":id,"user_id":user,"nickname":user,
        "payload":{"decoder":"wechat.msg_list.arrival.candidate.v1","message_type":10005,
            "arrival_text_match":true,"observed_at_ms":chrono::Utc::now().timestamp_millis()}})
    };
    client
        .events(vec![
            arrival("a", "甲"),
            arrival("b", "乙"),
            arrival("c", "甲"),
        ])
        .await;
    assert_eq!(
        client.poll().await["candidate_preview"]["pending_welcome_users"],
        2
    );
    // Keep the bound document alive while waiting for the actual bridge clock.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_secs(9)).await;
        assert_eq!(
            client.poll().await["candidate_preview"]["pending_welcome_users"],
            2
        );
    }
    tokio::time::sleep(Duration::from_millis(3200)).await;
    let poll = json!({"v":1,"connection_id":client.hello["connection_id"],"binding_id":client.hello["binding_id"],
        "page_url":"https://channels.weixin.qq.com/web/pages/live","input_ready":true});
    let response = client.post("/v1/poll", poll.clone()).await;
    assert_eq!(response["commands"].as_array().unwrap().len(), 1);
    assert_eq!(response["commands"][0]["text"], "欢迎甲、乙来到直播间");
    assert_eq!(client.post("/v1/poll", poll).await["commands"], json!([]));
    server.stop().await.unwrap();
}

#[tokio::test]
async fn supporter_arrival_fills_immediately_and_different_message_or_title_does_not_bypass_cooldown()
 {
    let temp = tempfile::tempdir().unwrap();
    let (mut server, mut client) = setup_with_rules(
        temp.path(),
        "rules:\n  supporter_welcome:\n    template: '{title}：欢迎{nickname}'\n",
        true,
    )
    .await;
    let entrance = |id: &str, title: &str| {
        json!({"seq":1,"event_id":"fixture","event_type":"member.candidate",
        "source_kind":"network","native_event_id":id,"user_id":"same-user","nickname":"甲",
        "payload":{"decoder":"wechat.msg_list.arrival.candidate.v1","message_type":10005,
            "arrival_text_match":true,"supporter_title":title,"observed_at_ms":chrono::Utc::now().timestamp_millis()}})
    };
    client
        .events(vec![
            entrance("a", "super_fan"),
            entrance("b", "star_guardian"),
        ])
        .await;
    let poll = json!({"v":1,"connection_id":client.hello["connection_id"],"binding_id":client.hello["binding_id"],
        "page_url":"https://channels.weixin.qq.com/web/pages/live","input_ready":true});
    let response = client.post("/v1/poll", poll.clone()).await;
    assert_eq!(response["commands"].as_array().unwrap().len(), 1);
    assert_eq!(response["commands"][0]["text"], "超粉：欢迎甲");
    assert_eq!(response["candidate_preview"]["pending_welcome_users"], 0);
    client.events(vec![entrance("c", "super_fan")]).await;
    assert_eq!(client.post("/v1/poll", poll).await["commands"], json!([]));
    let c = rusqlite::Connection::open(temp.path().join("probe.sqlite3")).unwrap();
    let count:i64=c.query_row("select count(*) from candidate_draft_audits where json_extract(audit_json,'$.reason')='supporter_arrival'",[],|r|r.get(0)).unwrap();
    assert_eq!(count, 1);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn native_upgrade_uses_configured_template_and_enters_fill_queue_once() {
    let temp = tempfile::tempdir().unwrap();
    let (mut server, mut client) = setup_with_rules(temp.path(),
        "rules:\n  welcome:\n    enabled: false\n  upgrade:\n    enabled: true\n    template: '恭喜{nickname}升到{new_level}级'\nnickname_mapping:\n  users:\n    upgrade-user: 测试哥\n", true).await;
    let notice = |id: &str| json!({"event_type":"level.candidate","source_kind":"network",
        "native_event_id":id,"user_id":"upgrade-user","nickname":"長**",
        "payload":{"decoder":"wechat.app20031.upgrade.candidate.v1","message_type":20031,
        "upgrade_type":1,"previous_level":0,"new_level":1,"recipient_user_id":"selected-host",
        "observed_at_ms":chrono::Utc::now().timestamp_millis(),"live_eligible":true,"room_verified":true}});
    let mut wrong = notice("wrong-host"); wrong["payload"]["recipient_user_id"] = json!("other-host");
    client.events(vec![wrong, notice("upgrade-a"), notice("upgrade-a")]).await;
    let poll = json!({"v":1,"connection_id":client.hello["connection_id"],"binding_id":client.hello["binding_id"],
        "page_url":"https://channels.weixin.qq.com/web/pages/live","input_ready":true});
    let response = client.post("/v1/poll",poll).await;
    assert_eq!(response["commands"].as_array().unwrap().len(),1);
    assert_eq!(response["commands"][0]["text"],"恭喜测试哥升到1级");
    let c=rusqlite::Connection::open(temp.path().join("probe.sqlite3")).unwrap();
    let count:i64=c.query_row("select count(*) from candidate_draft_audits where json_extract(audit_json,'$.kind')='upgrade_candidate_draft'",[],|r|r.get(0)).unwrap();
    assert_eq!(count,1);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn native_upgrade_congratulates_every_distinct_notice_without_level_comparisons() {
    let temp = tempfile::tempdir().unwrap();
    let (mut server, mut client) = setup_with_rules(temp.path(),
        "rules:\n  welcome:\n    enabled: false\n  upgrade:\n    enabled: true\n    template: '恭喜{nickname}升到{new_level}级'\n", true).await;
    // Captured notices for the same viewer: 13 -> 16, then 0 -> 10.
    let notice = |id: &str, before: u64, after: u64| json!({
        "event_type":"level.candidate","source_kind":"network",
        "native_event_id":id,"user_id":"viewer","nickname":"言**",
        "payload":{"decoder":"wechat.app20031.upgrade.candidate.v1","message_type":20031,
        "upgrade_type":1,"previous_level":before,"new_level":after,
        "recipient_user_id":"selected-host","observed_at_ms":chrono::Utc::now().timestamp_millis()}});
    client.events(vec![notice("first", 13, 16)]).await;
    let mut no_previous = notice("no-previous-level", 0, 0);
    no_previous["payload"].as_object_mut().unwrap().remove("previous_level");
    no_previous["payload"]["observed_at_ms"] = json!(1);
    client.events(vec![notice("second", 0, 10), notice("second", 0, 10),
        notice("duplicate-level", 0, 16), notice("decrease", 16, 10),
        notice("unchanged", 10, 10), no_previous]).await;
    let c = rusqlite::Connection::open(temp.path().join("probe.sqlite3")).unwrap();
    let drafts: Vec<String> = c.prepare(
        "select json_extract(audit_json,'$.text') from candidate_draft_audits where json_extract(audit_json,'$.kind')='upgrade_candidate_draft' order by id"
    ).unwrap().query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(drafts, vec!["恭喜言**升到16级", "恭喜言**升到10级",
        "恭喜言**升到16级", "恭喜言**升到10级", "恭喜言**升到10级", "恭喜言**升到0级"]);
    // Every distinct notice reaches the fill queue; transport retries do not.
    let pending: i64 = c.query_row("select count(*) from fill_audits where json_extract(audit_json,'$.state')='queued'", [], |row| row.get(0)).unwrap();
    assert_eq!(pending, 6);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn wechat_bootstrap_tickets_keep_room_profiles_isolated_and_fallback_to_defaults() {
    let temp=tempfile::tempdir().unwrap();
    let (mut server,client)=setup_with_rules(temp.path(),
        "rules:\n  upgrade:\n    template: '默认{nickname}到{new_level}级'\nroom_profiles:\n  special-host:\n    rules:\n      upgrade:\n        template: '专属{nickname}到{new_level}级'\n    timed_messages:\n      min_comments: 19\n",true).await;
    let mut boots=Vec::new();
    for host in ["special-host","other-host"] {
        let mut bridge=server.bridge.lock().unwrap();let relay=bridge.relay_token.clone();
        let script=bridge.bootstrap_for_room(&relay,Some(host)).unwrap();
        let boot:Value=serde_json::from_str(script.split("const BOOT = ").nth(1).unwrap().split(";\n").next().unwrap()).unwrap();
        assert_eq!(boot["room_profile"],if host=="special-host" {"special-host"} else {"default"});
        if host=="special-host" {assert_eq!(boot["timed_messages"]["min_comments"],19);}
        boots.push((host,boot));
    }
    for (host,boot) in boots {
        let mut page=Client {http:client.http.clone(),base:client.base.clone(),hello:json!({}),seq:0};
        page.hello=page.post("/v1/hello",json!({"v":1,"ticket":boot["ticket"],"document_id":host,
            "adapter_id":"wechat_channels.desktop_probe","page_url":"https://channels.weixin.qq.com/web/pages/live"})).await;
        page.events(vec![json!({"event_type":"level.candidate","source_kind":"network","native_event_id":"same-notice",
            "user_id":"user","nickname":"测试哥","payload":{"decoder":"wechat.app20031.upgrade.candidate.v1",
            "message_type":20031,"upgrade_type":1,"previous_level":0,"new_level":1,"recipient_user_id":host,
            "observed_at_ms":chrono::Utc::now().timestamp_millis()}})]).await;
        let response=page.post("/v1/poll",json!({"v":1,"connection_id":page.hello["connection_id"],"binding_id":page.hello["binding_id"],
            "page_url":"https://channels.weixin.qq.com/web/pages/live","input_ready":true})).await;
        assert_eq!(response["commands"][0]["text"],if host=="special-host" {"专属测试哥到1级"} else {"默认测试哥到1级"});
    }
    server.stop().await.unwrap();
}
