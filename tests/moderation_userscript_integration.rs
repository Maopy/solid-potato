use live_archive_core::moderation::{
    account_budget::AccountBudget,
    config::ModerationConfig,
    userscript::{UserscriptConfig, UserscriptHub, UserscriptServer},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
fn hub(dir: &std::path::Path) -> UserscriptHub {
    let p = dir.join("dy.yml");
    std::fs::write(&p,"version: 1\nmode: fill_only\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/111\nsending:\n  automatic: true\nkeyword_replies:\n  enabled: true\n  cooldown_seconds: 60\n  rules:\n    - keywords: ['胎记', '印记']\n      reply: 手臂是胎记\nlikes:\n  enabled: true\n  mode: clock\n  every_minutes: 10\n  batch_count: 200\n").unwrap();
    let c = ModerationConfig::load(&p).unwrap();
    let hub = UserscriptHub::new(dir.join("data"), true);
    hub.add_template(
        "dy",
        "account",
        c.probe_options(),
        Arc::new(Mutex::new(AccountBudget::new(Default::default()).unwrap())),
    )
    .unwrap();
    hub
}
fn connect(hub: &UserscriptHub, page: &str, boot: &Value) -> Value {
    hub.dispatch(page,"hello","",json!({"v":1,"ticket":boot["ticket"],"page_url":page,"document_id":"doc","adapter_id":"douyin.live_page"})).unwrap()
}
fn scope(page: &str, hello: &Value) -> Value {
    json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":page,"input_ready":true})
}
#[test]
fn dashboard_manual_message_is_scoped_to_selected_douyin_room() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let a = "https://live.douyin.com/123";
    let b = "https://live.douyin.com/456";
    let boot = hub.bootstrap(a).unwrap();
    let ha=hub.dispatch(a,"hello","",json!({"v":1,"ticket":boot["ticket"],"page_url":a,"document_id":"manual-doc","adapter_id":"douyin.live_page","client_version":env!("CARGO_PKG_VERSION")})).unwrap();
    let hb = connect(&hub, b, &hub.bootstrap(b).unwrap());
    assert!(hub.send_message("dy-123", "仅发送到甲").is_ok());
    let pa = hub
        .dispatch(a, "poll", ha["token"].as_str().unwrap(), scope(a, &ha))
        .unwrap();
    let pb = hub
        .dispatch(b, "poll", hb["token"].as_str().unwrap(), scope(b, &hb))
        .unwrap();
    assert_eq!(pa["commands"][0]["text"], "仅发送到甲");
    assert_eq!(pb["commands"], json!([]));
    assert_eq!(
        hub.rows().iter().find(|r| r["id"] == "dy-123").unwrap()["control"]["manual_message"]["state"],
        "issued"
    );
}
#[test]
fn same_tab_refresh_replaces_old_binding_without_replaying_pending_commands() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let page = "https://live.douyin.com/123";
    let client = "6647e65f-df75-4f4c-a565-e076490400ee";
    let pair = |boot: &Value, tab: &str, document: &str| {
        hub.dispatch(page,"hello","",json!({"v":1,"ticket":boot["ticket"],"page_url":page,"document_id":document,"adapter_id":"douyin.live_page","client_id":tab}))
    };
    let boot = hub.bootstrap(page).unwrap();
    assert_eq!(boot["reload_handoff"], true);
    assert_eq!(boot["likes"]["enabled"], true);
    assert_eq!(boot["likes"]["mode"], "clock");
    assert_eq!(boot["likes"]["batch_count"], 200);
    let old = pair(&boot, client, "old-document").unwrap();
    hub.action("dy-123", Some(true)).unwrap();
    hub.dispatch(page,"events",old["token"].as_str().unwrap(),json!({"v":1,"connection_id":old["connection_id"],"binding_id":old["binding_id"],"first_seq":1,"events":[{"seq":1,"event_id":"pending-keyword","event_type":"chat.received","source_kind":"network","user_id":"viewer","native_event_id":"pending-native","payload":{"content":"胎记","decoder":"douyin.pb.v1","live_eligible":true,"observed_at_ms":chrono::Utc::now().timestamp_millis()}}]})).unwrap();
    let mut blocked = scope(page, &old);
    blocked["input_ready"] = json!(false);
    assert_eq!(
        hub.dispatch(page, "poll", old["token"].as_str().unwrap(), blocked)
            .unwrap()["fill"]["pending"],
        1
    );
    assert!(pair(&boot, client, "replayed-ticket").is_err());
    let other = hub.bootstrap(page).unwrap();
    assert!(pair(&other, "b82a4cd3-8d3b-4112-9d79-7473ec9bc42e", "other-tab").is_err());
    let refreshed = pair(&hub.bootstrap(page).unwrap(), client, "fresh-document").unwrap();
    assert_ne!(old["connection_id"], refreshed["connection_id"]);
    assert!(
        hub.dispatch(
            page,
            "poll",
            old["token"].as_str().unwrap(),
            scope(page, &old)
        )
        .is_err()
    );
    let result = hub
        .dispatch(
            page,
            "poll",
            refreshed["token"].as_str().unwrap(),
            scope(page, &refreshed),
        )
        .unwrap();
    assert_eq!(result["commands"], json!([]));
    assert_eq!(result["fill"]["pending"], 0);
}
#[tokio::test]
async fn arbitrary_rooms_are_independent_default_off_and_bound_to_their_own_ticket() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let a = "https://live.douyin.com/123";
    let b = "https://live.douyin.com/456";
    for bad in [
        "http://live.douyin.com/123",
        "https://evil.test/123",
        "https://live.douyin.com/abc",
        "https://live.douyin.com:2025/123",
    ] {
        assert!(hub.bootstrap(bad).is_err());
    }
    let ba = hub.bootstrap(&format!("{a}?from=test")).unwrap();
    let bb = hub.bootstrap(b).unwrap();
    assert_eq!(ba["room_url"], a);
    assert_eq!(ba["moderation_enabled"], false);
    assert_eq!(bb["moderation_enabled"], false);
    assert!(hub.dispatch(b,"hello","",json!({"v":1,"ticket":ba["ticket"],"page_url":b,"document_id":"bad","adapter_id":"douyin.live_page"})).is_err());
    let ha = connect(&hub, a, &ba);
    let hb = connect(&hub, b, &bb);
    assert_eq!(
        hub.rows().iter().find(|r| r["id"] == "dy-123").unwrap()["control"]["client_outdated"],
        true,
        "legacy scripts must show an update hint rather than silently missing public text"
    );
    let token = ha["token"].as_str().unwrap();
    assert!(hub.dispatch(b, "poll", token, scope(b, &ha)).is_err());
    assert!(hub.dispatch(a, "poll", token, scope(b, &ha)).is_err());
    let mut ctl = scope(a, &ha);
    ctl.as_object_mut().unwrap().remove("input_ready");
    hub.dispatch(a, "control", token, json!({"scope":ctl,"enabled":true}))
        .unwrap();
    let pa = hub.dispatch(a, "poll", token, scope(a, &ha)).unwrap();
    let pb = hub
        .dispatch(b, "poll", hb["token"].as_str().unwrap(), scope(b, &hb))
        .unwrap();
    assert_eq!(pa["moderation_control"]["enabled"], true);
    assert_eq!(pb["moderation_control"]["enabled"], false);
    assert!(pb["commands"].as_array().is_none_or(|v| v.is_empty()));
    // Repeated bootstrap/reload cannot reset a live room or create a duplicate sender.
    let again = hub.bootstrap(a).unwrap();
    assert_eq!(again["moderation_enabled"], true);
    assert!(hub.dispatch(a,"hello","",json!({"v":1,"ticket":again["ticket"],"page_url":a,"document_id":"second","adapter_id":"douyin.live_page"})).is_err());
    let rec = hub.action("dy-456", None).unwrap();
    assert!(rec["request_id"].is_string());
    assert!(hub.dispatch(a, "poll", token, scope(a, &ha)).unwrap()["recording_request"].is_null());
    assert!(
        hub.dispatch(b, "poll", hb["token"].as_str().unwrap(), scope(b, &hb))
            .unwrap()["recording_request"]
            .is_string()
    );
    assert!(hub.authorize_recording(b, token, scope(b, &ha)).is_err());
    hub.dispatch(a, "events", token, json!({"v":1,"connection_id":ha["connection_id"],"binding_id":ha["binding_id"],"first_seq":1,"events":[{"seq":1,"event_id":"host-name","event_type":"room.profile","source_kind":"diagnostic","payload":{"nickname":"测试主播"}}]})).unwrap();
    let rows = hub.rows();
    let named = rows.iter().find(|row| row["id"] == "dy-123").unwrap();
    assert_eq!(named["label"], "测试主播");
    assert_eq!(named["anchor_name"], "测试主播");
    assert_eq!(hub.rows().len(), 2);
}
#[tokio::test]
async fn transport_requires_pairing_key_has_no_cors_and_installs_static_code() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let config = UserscriptConfig {
        enabled: true,
        port: 0,
        key_file: Some(dir.path().join("key")),
    };
    let server = UserscriptServer::bind(&config, dir.path(), hub.clone(), None, None)
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let r = client.get(&server.install_url).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let script = r.text().await.unwrap();
    assert!(script.starts_with("// ==UserScript=="));
    assert!(script.contains("@run-at       document-start"));
    assert!(!script.contains("/*__SDK__*/"));
    assert!(!script.contains("eval("));
    let url = url::Url::parse(&server.install_url).unwrap();
    let endpoint = format!("http://{}/v1/userscript", url.authority());
    let body = json!({"room_url":"https://live.douyin.com/789","endpoint":"bootstrap"});
    let r = client
        .post(&endpoint)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert!(r.headers().get("access-control-allow-origin").is_none());
    let key = std::fs::read_to_string(dir.path().join("key")).unwrap();
    let r = client
        .post(&endpoint)
        .bearer_auth(&key)
        .header("Origin", "https://evil.test")
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    for origin in [
        "https://live.douyin.com",
        "https://www.douyin.com",
        "https://douyin.com",
    ] {
        let response = client
            .post(&endpoint)
            .bearer_auth(&key)
            .header("Origin", origin)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "supported origin {origin}");
    }
    let text = client
        .post(&endpoint)
        .bearer_auth(&key)
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let boot: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(boot["room_url"], "https://live.douyin.com/789");
    let unsupported = json!({"room_url":"https://live.douyin.com/789","endpoint":"arbitrary","session_token":"bad","body":{}});
    assert_eq!(
        client
            .post(&endpoint)
            .bearer_auth(&key)
            .header("Content-Type", "application/json")
            .body(unsupported.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    server.stop().await;
    let server = UserscriptServer::bind(&config, dir.path(), hub, None, None)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("key")).unwrap(),
        key
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.path().join("key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    server.stop().await;
}

#[tokio::test]
async fn fresh_keyword_replies_use_isolated_room_queues_and_shared_account_budget() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let a = "https://live.douyin.com/123";
    let b = "https://live.douyin.com/456";
    let ha = connect(&hub, a, &hub.bootstrap(a).unwrap());
    let hb = connect(&hub, b, &hub.bootstrap(b).unwrap());
    for (page, h) in [(a, &ha), (b, &hb)] {
        hub.action(if page == a { "dy-123" } else { "dy-456" }, Some(true))
            .unwrap();
        hub.dispatch(page,"events",h["token"].as_str().unwrap(),json!({"v":1,"connection_id":h["connection_id"],"binding_id":h["binding_id"],"first_seq":1,"events":[{"seq":1,"event_id":"same-local-event","event_type":"chat.received","source_kind":"network","user_id":"viewer","native_event_id":"same-native-message","nickname":"观众","payload":{"content":"胎记","decoder":"douyin.pb.v1","live_eligible":true,"observed_at_ms":chrono::Utc::now().timestamp_millis()}}]})).unwrap();
    }
    let pa = hub
        .dispatch(a, "poll", ha["token"].as_str().unwrap(), scope(a, &ha))
        .unwrap();
    assert_eq!(pa["commands"][0]["text"], "手臂是胎记");
    assert_eq!(pa["commands"][0]["page_url"], a);
    let pb = hub
        .dispatch(b, "poll", hb["token"].as_str().unwrap(), scope(b, &hb))
        .unwrap();
    assert_eq!(
        pb["commands"],
        json!([]),
        "same account cannot issue another room concurrently"
    );
    assert_eq!(
        pb["keyword_replies"]["generated"], 1,
        "other room keeps its own event dedupe"
    );
    assert_eq!(pb["fill"]["pending"], 1);
    hub.action("dy-456", Some(false)).unwrap();
    hub.action("dy-456", Some(true)).unwrap();
    let pb = hub
        .dispatch(b, "poll", hb["token"].as_str().unwrap(), scope(b, &hb))
        .unwrap();
    assert_eq!(pb["fill"]["pending"], 0, "toggle discards waiting messages");
}

#[tokio::test]
async fn unified_controller_proxies_only_wechat_and_dashboard_manages_dynamic_douyin_rooms() {
    use live_archive_core::moderation::{
        control::{Controller, LoadedControl},
        dashboard::Dashboard,
    };
    let dir = tempfile::tempdir().unwrap();
    let _ = hub(dir.path());
    std::fs::write(dir.path().join("wx.yml"),"version: 1\nroom:\n  platform: wechat_channels\n  adapter: wechat_channels.desktop_probe\n  url: https://channels.weixin.qq.com/web/pages/live\n").unwrap();
    let config = dir.path().join("control.yml");
    std::fs::write(&config,"version: 2\ndirectory: unified\ndashboard: true\nstart_enabled: false\ndouyin_userscript: {enabled: true}\naccounts:\n  - {id: dy, platform: douyin}\n  - {id: wx, platform: wechat_channels}\ntasks:\n  - {id: dy, account_id: dy, config: dy.yml}\n  - {id: wx, account_id: wx, config: wx.yml}\n").unwrap();
    let loaded = LoadedControl::load(&config).unwrap();
    let mut controller = Controller::bind(&loaded).await.unwrap();
    assert_eq!(controller.tasks.len(), 1);
    assert_eq!(controller.tasks["wx"].scope.platform(), "wechat_channels");
    assert!(!controller.addon().unwrap().contains("/v2/tasks/dy/"));
    let hub = controller.userscript.as_ref().unwrap();
    let page = "https://live.douyin.com/999";
    let h = connect(hub, page, &hub.bootstrap(page).unwrap());
    let mut dash = Dashboard::bind_with_userscript(
        &controller,
        None,
        Some("http://127.0.0.1/install/test.user.js".into()),
    )
    .await
    .unwrap();
    let url = url::Url::parse(&dash.url).unwrap();
    let base = format!("http://{}", url.authority());
    let token = url.fragment().unwrap();
    let client = reqwest::Client::new();
    let text = client
        .get(format!("{base}/api/state"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let state: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(state["rooms"].as_array().unwrap().len(), 2);
    assert!(state["userscript_install_url"].is_string());
    let r = client
        .post(format!("{base}/api/moderation"))
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .body(json!({"id":"dy-999","enabled":true}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(hub.rows()[0]["control"]["enabled"], true);
    hub.dispatch(
        page,
        "goodbye",
        h["token"].as_str().unwrap(),
        scope(page, &h),
    )
    .unwrap();
    dash.stop().await;
    controller.stop().await.unwrap();
}

#[test]
fn like_claim_is_authenticated_persistent_and_independent_of_moderation_and_recording() {
    let dir = tempfile::tempdir().unwrap();
    let page = "https://live.douyin.com/123";
    let client = "6647e65f-df75-4f4c-a565-e076490400ee";
    let pair = |h: &UserscriptHub, document: &str| {
        let boot = h.bootstrap(page).unwrap();
        assert_eq!(boot["like_period_ledger"], true);
        h.dispatch(page,"hello","",json!({"v":1,"ticket":boot["ticket"],"page_url":page,"document_id":document,"adapter_id":"douyin.live_page","client_id":client})).unwrap()
    };
    let h = hub(dir.path());
    let hello = pair(&h, "initial");
    h.room(page)
        .unwrap()
        .bridge
        .lock()
        .unwrap()
        .configure_ui(false, true);
    assert!(
        h.dispatch(page, "likes/claim", "bad-token", scope(page, &hello))
            .is_err()
    );
    let first = h
        .dispatch(
            page,
            "likes/claim",
            hello["token"].as_str().unwrap(),
            scope(page, &hello),
        )
        .unwrap();
    assert_eq!(first["claimed"], true);
    let action = h
        .dispatch(
            page,
            "likes/tick",
            hello["token"].as_str().unwrap(),
            scope(page, &hello),
        )
        .unwrap();
    assert_eq!(action["claimed"], true);
    let early = h
        .dispatch(
            page,
            "likes/tick",
            hello["token"].as_str().unwrap(),
            scope(page, &hello),
        )
        .unwrap();
    assert_eq!(early["claimed"], false);
    assert_eq!(
        first["period_end_ms"].as_i64().unwrap() - first["period_start_ms"].as_i64().unwrap(),
        600000
    );
    let repeated = h
        .dispatch(
            page,
            "likes/claim",
            hello["token"].as_str().unwrap(),
            scope(page, &hello),
        )
        .unwrap();
    if first["period_start_ms"] == repeated["period_start_ms"] {
        assert_eq!(repeated["claimed"], false);
    }
    let refreshed = pair(&h, "refreshed");
    let again = h
        .dispatch(
            page,
            "likes/claim",
            refreshed["token"].as_str().unwrap(),
            scope(page, &refreshed),
        )
        .unwrap();
    if repeated["period_start_ms"] == again["period_start_ms"] {
        assert_eq!(again["claimed"], false);
    }
    drop(h);
    let restarted = hub(dir.path());
    let hello = pair(&restarted, "restarted");
    let last = restarted
        .dispatch(
            page,
            "likes/claim",
            hello["token"].as_str().unwrap(),
            scope(page, &hello),
        )
        .unwrap();
    if last["period_start_ms"] == again["period_start_ms"] {
        assert_eq!(last["claimed"], false);
    }
}

#[test]
fn speech_accepts_bound_legacy_scope_without_page_url_but_rejects_wrong_room() {
    let dir = tempfile::tempdir().unwrap();
    let h = hub(dir.path());
    let page = "https://live.douyin.com/123";
    let boot = h.bootstrap(page).unwrap();
    let hello = h.dispatch(page, "hello", "", json!({"v":1,"ticket":boot["ticket"],"page_url":page,"document_id":"doc","adapter_id":"douyin.live_page"})).unwrap();
    let token = hello["token"].as_str().unwrap();
    let request = json!({"scope":{"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"]},"action":"status"});
    assert!(h.dispatch(page, "speech", token, request.clone()).is_ok());
    assert!(
        h.dispatch(page, "speech", "wrong-token", request.clone())
            .is_err()
    );
    let mut wrong = request;
    wrong["scope"]["page_url"] = json!("https://live.douyin.com/456");
    assert!(h.dispatch(page, "speech", token, wrong).is_err());
}

#[test]
fn scoped_userscript_actions_inherit_room_binding_without_cross_room_access() {
    let dir = tempfile::tempdir().unwrap();
    let h = hub(dir.path());
    let page = "https://live.douyin.com/123";
    let boot = h.bootstrap(page).unwrap();
    let hello = h.dispatch(page,"hello","",json!({"v":1,"ticket":boot["ticket"],"document_id":"doc","adapter_id":"douyin.live_page","client_id":"6647e65f-df75-4f4c-a565-e076490400ee"})).unwrap();
    let token = hello["token"].as_str().unwrap();
    let scope =
        json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"]});
    assert!(
        h.dispatch(
            page,
            "control",
            token,
            json!({"scope":scope,"enabled":false})
        )
        .is_ok()
    );
    assert!(
        h.dispatch(
            page,
            "speech",
            token,
            json!({"scope":scope,"action":"status"})
        )
        .is_ok()
    );
    assert!(h.dispatch(page, "poll", token, scope.clone()).is_ok());
    assert!(
        h.dispatch(page, "likes/claim", token, scope.clone())
            .is_ok()
    );
    assert!(h.dispatch(page, "likes/tick", token, scope.clone()).is_ok());
    let mut wrong = scope.clone();
    wrong["page_url"] = json!("https://live.douyin.com/456");
    assert!(h.dispatch(page, "poll", token, wrong).is_err());
    assert!(h.dispatch(page, "poll", "bad-token", scope).is_err());
}

#[tokio::test]
async fn current_script_public_text_is_visible_with_moderation_off_and_never_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub(dir.path());
    let page = "https://live.douyin.com/123";
    let boot = hub.bootstrap(page).unwrap();
    let hello=hub.dispatch(page,"hello","",json!({"v":1,"ticket":boot["ticket"],"page_url":page,"document_id":"new-client","adapter_id":"douyin.live_page","client_version":env!("CARGO_PKG_VERSION")})).unwrap();
    assert_eq!(hub.rows()[0]["control"]["client_outdated"], false);
    hub.dispatch(page,"events",hello["token"].as_str().unwrap(),json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"first_seq":1,"events":[{"seq":1,"event_id":"public-one","native_event_id":"native-one","event_type":"chat.observed","source_kind":"network","nickname":"小雨","payload":{"content":"这首歌叫什么？","live_eligible":false,"decoder":"douyin.pb.v1","observed_at_ms":chrono::Utc::now().timestamp_millis()}}]})).unwrap();
    let history = hub.timeline("dy-123").unwrap();
    assert_eq!(history["entries"][0]["text"], "这首歌叫什么？");
    assert_eq!(hub.rows()[0]["control"]["enabled"], false);
    hub.action("dy-123", Some(true)).unwrap();
    let poll = hub
        .dispatch(
            page,
            "poll",
            hello["token"].as_str().unwrap(),
            scope(page, &hello),
        )
        .unwrap();
    assert_eq!(
        poll["commands"],
        json!([]),
        "opening moderation never replies to display-only history"
    );
}
