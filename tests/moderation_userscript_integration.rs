use live_archive_core::moderation::{
    account_budget::AccountBudget,
    config::ModerationConfig,
    userscript::{UserscriptConfig, UserscriptHub, UserscriptServer},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
fn hub(dir: &std::path::Path) -> UserscriptHub {
    let p = dir.join("dy.yml");
    std::fs::write(&p,"version: 1\nmode: fill_only\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/111\nsending:\n  automatic: true\nkeyword_replies:\n  enabled: true\n  cooldown_seconds: 60\n  rules:\n    - keywords: ['胎记', '印记']\n      reply: 手臂是胎记\n").unwrap();
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
