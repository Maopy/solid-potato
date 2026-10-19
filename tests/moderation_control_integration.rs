use live_archive_core::moderation::control::{Controller, LoadedControl};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
fn setup(dir: &Path) -> std::path::PathBuf {
    let wx = |target: &str| {
        format!(
            "version: 1\nmode: candidate_fill_only\nroom:\n  platform: wechat_channels\n  adapter: wechat_channels.desktop_probe\n  url: https://channels.weixin.qq.com/web/pages/live\ncandidate_preview:\n  enabled: true\n  recipient_user_id: {target}\nsending:\n  automatic: true\nrules:\n  gift:\n    emojis: []\n"
        )
    };
    for (name, target) in [("a", "host-a"), ("b", "host-b"), ("c", "host-c")] {
        std::fs::write(dir.join(format!("{name}.yml")), wx(target)).unwrap();
    }
    std::fs::write(dir.join("d.yml"),"version: 1\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/139244611014\n").unwrap();
    let control = dir.join("control.yml");
    std::fs::write(&control,"version: 2\ndirectory: data\naccounts:\n  - id: wx-main\n    platform: wechat_channels\n  - id: wx-other\n    platform: wechat_channels\n  - id: dy-main\n    platform: douyin\ntasks:\n  - {id: a, label: '直播甲', account_id: wx-main, config: a.yml}\n  - {id: b, label: '直播乙', account_id: wx-main, config: b.yml}\n  - {id: c, label: '直播丙', account_id: wx-other, config: c.yml}\n  - {id: d, label: '抖音', account_id: dy-main, config: d.yml}\n").unwrap();
    control
}
struct Client {
    base: String,
    http: reqwest::Client,
    hello: Value,
    page: String,
    origin: String,
}
impl Client {
    async fn connect(controller: &Controller, id: &str) -> Self {
        let t = &controller.tasks[id];
        let source = {
            let mut b = t.server.bridge.lock().unwrap();
            let relay = b.relay_token.clone();
            b.bootstrap(&relay).unwrap()
        };
        let boot: Value = serde_json::from_str(
            source
                .split("const BOOT = ")
                .nth(1)
                .unwrap()
                .split(';')
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            boot["prefix"],
            format!("/__live_archive/moderation/v2/tasks/{id}/")
        );
        let mut c = Self {
            base: format!("http://{}", t.server.address),
            http: reqwest::Client::new(),
            hello: Value::Null,
            page: t.scope.room_url.to_string(),
            origin: t.scope.room_url.origin().ascii_serialization(),
        };
        c.hello=c.post("hello",json!({"v":1,"ticket":boot["ticket"],"page_url":c.page,"document_id":format!("document-{id}"),"adapter_id":boot["adapter_id"]})).await;
        c
    }
    async fn response(&self, path: &str, body: Value) -> reqwest::Response {
        self.http
            .post(format!("{}/v1/{path}", self.base))
            .header("Origin", &self.origin)
            .header("Content-Type", "application/json")
            .bearer_auth(self.hello["token"].as_str().unwrap_or(""))
            .body(body.to_string())
            .send()
            .await
            .unwrap()
    }
    async fn post(&self, path: &str, body: Value) -> Value {
        let r = self.response(path, body).await;
        assert!(
            r.status().is_success(),
            "{}",
            r.text().await.unwrap_or_default()
        );
        serde_json::from_str(&r.text().await.unwrap()).unwrap()
    }
    fn scope(&self) -> Value {
        json!({"v":1,"connection_id":self.hello["connection_id"],"binding_id":self.hello["binding_id"],"page_url":self.page,"input_ready":true})
    }
    async fn gift(&self, target: &str, name: &str) {
        let mut body = self.scope();
        body.as_object_mut().unwrap().remove("page_url");
        body.as_object_mut().unwrap().remove("input_ready");
        body["first_seq"] = json!(1);
        body["events"] = json!([
         {"seq":1,"event_id":"start","event_type":"probe.status","source_kind":"diagnostic","payload":{"stage":"wechat_collection_started"}},
         {"seq":2,"event_id":"gift","event_type":"gift.candidate","source_kind":"network","user_id":"same-user","native_event_id":"same-native-id","nickname":"观众","payload":{"decoder":"wechat.app20009.reward.candidate.v1","message_type":20009,"gift_id":"product","gift_name":name,"reward_product_count":1,"recipient_user_id":target}}
        ]);
        self.post("events", body).await;
    }
    async fn receipt(&self, command: &Value) {
        let mut body = self.scope();
        body.as_object_mut().unwrap().remove("input_ready");
        body["action_id"] = command["action_id"].clone();
        body["outcome"] = json!("submitted");
        body["reason"] = json!("input_submitted");
        self.post("results", body).await;
    }
    async fn goodbye(&self) {
        let mut b = self.scope();
        b.as_object_mut().unwrap().remove("input_ready");
        self.post("goodbye", b).await;
    }
}
#[test]
fn controller_preflight_rejects_conflicts_before_creating_data_or_proxy() {
    let temp = tempfile::tempdir().unwrap();
    let path = setup(temp.path());
    let original = std::fs::read_to_string(&path).unwrap();
    assert_eq!(LoadedControl::load(&path).unwrap().tasks.len(), 4);
    assert!(!temp.path().join("data").exists());
    for bad in [
        original.replace("id: b,", "id: a,"),
        original.replace(
            "account_id: wx-main, config: a.yml",
            "account_id: dy-main, config: a.yml",
        ),
        original.replace("id: b,", "id: ../bad,"),
        original.replace("config: b.yml", "config: a.yml"),
        original.replace("version: 2", "version: 1"),
    ] {
        std::fs::write(&path, bad).unwrap();
        let r = std::process::Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
            .args(["moderation", "check-controller", "--config"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(!r.status.success());
        assert!(!temp.path().join("data").exists());
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_tasks_isolate_events_tokens_and_share_only_account_send_budget() {
    let temp = tempfile::tempdir().unwrap();
    let loaded = LoadedControl::load(&setup(temp.path())).unwrap();
    let mut controller = Controller::bind(&loaded).await.unwrap();
    let (a, b, c, d) = tokio::join!(
        Client::connect(&controller, "a"),
        Client::connect(&controller, "b"),
        Client::connect(&controller, "c"),
        Client::connect(&controller, "d")
    );
    assert_eq!(b.response("poll", a.scope()).await.status(), 422);
    tokio::join!(
        a.gift("host-a", "玫瑰"),
        b.gift("host-b", "爱心"),
        c.gift("host-c", "墨镜")
    );
    tokio::time::sleep(Duration::from_millis(5200)).await;
    let pa = a.post("poll", a.scope()).await;
    assert_eq!(pa["commands"].as_array().unwrap().len(), 1);
    assert_eq!(pa["commands"][0]["text"], "感谢观众送来的玫瑰");
    a.receipt(&pa["commands"][0]).await;
    let pb = b.post("poll", b.scope()).await;
    assert_eq!(pb["commands"], json!([]));
    assert_eq!(pb["candidate_preview"]["counters"]["draft_generated"], 1);
    let pc = c.post("poll", c.scope()).await;
    assert_eq!(pc["commands"].as_array().unwrap().len(), 1);
    c.receipt(&pc["commands"][0]).await;
    let pd = d.post("poll", d.scope()).await;
    assert_eq!(pd["commands"], json!([]));
    assert_eq!(pd["rules"], Value::Null);
    tokio::time::sleep(Duration::from_millis(8100)).await;
    let pb = b.post("poll", b.scope()).await;
    assert_eq!(pb["commands"].as_array().unwrap().len(), 1);
    assert_eq!(pb["commands"][0]["text"], "感谢观众送来的爱心");
    b.receipt(&pb["commands"][0]).await;
    a.goodbye().await;
    controller.stop_task("a").await.unwrap();
    assert_eq!(b.post("poll", b.scope()).await["commands"], json!([]));
    for (id, name) in [("a", "玫瑰"), ("b", "爱心"), ("c", "墨镜")] {
        let db = rusqlite::Connection::open(
            temp.path()
                .join(format!("data/tasks/{id}/moderation-probe.sqlite3")),
        )
        .unwrap();
        let texts:Vec<String>=db.prepare("SELECT json_extract(audit_json,'$.text') FROM candidate_draft_audits WHERE json_extract(audit_json,'$.kind')='gift_candidate_draft'").unwrap().query_map([],|r|r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(texts, vec![format!("感谢观众送来的{name}")]);
    }
    tokio::join!(b.goodbye(), c.goodbye(), d.goodbye());
    controller.stop().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_proxy_routes_multiple_platforms_and_rejects_cross_task_auth() {
    let temp = tempfile::tempdir().unwrap();
    let loaded = LoadedControl::load(&setup(temp.path())).unwrap();
    let mut controller = Controller::bind(&loaded).await.unwrap();
    let addon = temp.path().join("controller.py");
    std::fs::write(&addon, controller.addon().unwrap()).unwrap();
    let result = tokio::task::spawn_blocking(move || {
        std::process::Command::new("python3")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/support/moderation_controller_proxy_driver.py"),
            )
            .arg(addon)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for id in ["a", "b", "c", "d"] {
        let c = rusqlite::Connection::open(
            temp.path()
                .join(format!("data/tasks/{id}/moderation-probe.sqlite3")),
        )
        .unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM raw_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    controller.stop().await.unwrap();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_real_proxy_loads_all_task_routes_and_cleans_up_its_process() {
    use live_archive_core::moderation::inject::ProbeSidecar;
    let temp = tempfile::tempdir().unwrap();
    let loaded = LoadedControl::load(&setup(temp.path())).unwrap();
    let mut controller = Controller::bind(&loaded).await.unwrap();
    let marker = temp.path().join("runtime.json");
    let source = format!(
        "{}\nfrom mitmproxy import ctx\ndef running():\n    with open({},'w') as f: json.dump({{'routes':len(ROUTES),'ignore_hosts':list(ctx.options.ignore_hosts)}},f)\n",
        controller.addon().unwrap(),
        serde_json::to_string(&marker).unwrap()
    );
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let scopes: Vec<_> = controller.tasks.values().map(|t| t.scope.clone()).collect();
    let proxy_dir = temp.path().join("proxy");
    let mut sidecar = ProbeSidecar::start_many(
        Path::new("mitmdump"),
        &proxy_dir,
        port,
        &source,
        &scopes,
        false,
        true,
    )
    .await
    .unwrap();
    for _ in 0..40 {
        if marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let result = std::fs::read_to_string(&marker);
    sidecar.stop().await.unwrap();
    let mut next = ProbeSidecar::start_many(
        Path::new("mitmdump"),
        &proxy_dir,
        port,
        &source,
        &scopes,
        false,
        true,
    )
    .await
    .unwrap();
    drop(sidecar);
    assert!(
        proxy_dir.join("moderation_probe.py").exists(),
        "old stopped handle cannot delete new instance's addon"
    );
    assert!(proxy_dir.join("moderation-probe.lock").exists());
    next.stop().await.unwrap();
    controller.stop().await.unwrap();
    let data: Value = serde_json::from_str(&result.unwrap()).unwrap();
    assert_eq!(data["routes"], 4);
    assert!(
        data["ignore_hosts"][0]
            .as_str()
            .unwrap()
            .contains("channels\\.weixin\\.qq\\.com")
    );
    assert!(
        data["ignore_hosts"][0]
            .as_str()
            .unwrap()
            .contains("live\\.douyin\\.com")
    );
    assert!(!proxy_dir.join("moderation-probe.lock").exists());
    assert!(!proxy_dir.join("moderation_probe.py").exists());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serve_cli_starts_all_tasks_once_and_exits_without_leaving_proxy_files() {
    let temp = tempfile::tempdir().unwrap();
    let path = setup(temp.path());
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let source = std::fs::read_to_string(&path).unwrap()
        + &format!(
            "duration_seconds: 1\ninjection:\n  system_proxy: false\n  proxy_port: {port}\n"
        );
    std::fs::write(&path, source).unwrap();
    let result = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
            .args(["moderation", "serve", "--config"])
            .arg(path)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Shared moderation proxy"));
    for id in ["a", "b", "c", "d"] {
        assert!(stdout.contains(&format!("\"id\": \"{id}\"")));
        assert!(
            temp.path()
                .join(format!("data/tasks/{id}/moderation-probe.sqlite3"))
                .exists()
        );
    }
    assert!(!temp.path().join("data/moderation-probe.lock").exists());
    assert!(!temp.path().join("data/moderation_probe.py").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn matching_durable_echo_recovers_only_its_task_and_retries_do_not_relatch_or_resend() {
    let temp = tempfile::tempdir().unwrap();
    let loaded = LoadedControl::load(&setup(temp.path())).unwrap();
    let mut controller = Controller::bind(&loaded).await.unwrap();
    let (a, c) = tokio::join!(
        Client::connect(&controller, "a"),
        Client::connect(&controller, "c")
    );
    tokio::join!(a.gift("host-a", "玫瑰"), c.gift("host-c", "爱心"));
    tokio::time::sleep(Duration::from_millis(5200)).await;
    let pa = a.post("poll", a.scope()).await;
    let pc = c.post("poll", c.scope()).await;
    let ca = &pa["commands"][0];
    let cc = &pc["commands"][0];
    assert!(!ca.is_null() && !cc.is_null());
    let mut uncertain = a.scope();
    uncertain.as_object_mut().unwrap().remove("input_ready");
    uncertain["action_id"] = ca["action_id"].clone();
    uncertain["outcome"] = json!("unknown");
    uncertain["reason"] = json!("submission_unconfirmed");
    a.post("results", uncertain.clone()).await;
    c.receipt(cc).await;
    let make_echo = |client: &Client, command: &Value| {
        let mut v = client.scope();
        v.as_object_mut().unwrap().remove("input_ready");
        v["action_id"] = command["action_id"].clone();
        v["state"] = json!("echo_observed");
        v["source"] = json!("wechat_native_poll");
        v["message_id"] = json!("server-echo");
        v["local_message_id"] = json!("local-correlation");
        v["sender_user_id"] = json!("fixture-own");
        v
    };
    let echo_c = make_echo(&c, cc);
    c.post("delivery", echo_c.clone()).await;
    assert_eq!(a.response("delivery", echo_c).await.status(), 422);
    assert_eq!(a.post("poll", a.scope()).await["fill"]["blocked"], false);
    let echo_a = make_echo(&a, ca);
    assert_eq!(
        a.post("delivery", echo_a.clone()).await["submission_resumed"],
        true
    );
    assert_eq!(a.post("delivery", echo_a).await["submission_resumed"], true);
    a.post("results", uncertain).await; // An old receipt retry must not re-latch the pause.
    let after = a.post("poll", a.scope()).await;
    assert_eq!(after["fill"]["blocked"], false);
    assert_eq!(after["fill"]["echo_confirmed"], 1);
    assert_eq!(after["commands"], json!([]));
    let db = rusqlite::Connection::open(temp.path().join("data/tasks/a/moderation-probe.sqlite3"))
        .unwrap();
    let count:i64=db.query_row("select count(*) from fill_audits where json_extract(audit_json,'$.state')='echo_confirmed'",[],|r|r.get(0)).unwrap();
    assert_eq!(count, 1);
    tokio::join!(a.goodbye(), c.goodbye());
    controller.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn room_timers_keep_their_own_text_and_share_only_account_send_budget() {
    let temp = tempfile::tempdir().unwrap();
    let control_path = setup(temp.path());
    for id in ["a", "b", "c"] {
        let p = temp.path().join(format!("{id}.yml"));
        let source = std::fs::read_to_string(&p).unwrap()
            + &format!(
                "timed_messages:\n  enabled: true\n  interval_seconds: 1\n  messages: ['房间{id}定时文案']\n"
            );
        std::fs::write(p, source).unwrap();
    }
    let loaded = LoadedControl::load(&control_path).unwrap();
    let mut controller = Controller::bind(&loaded).await.unwrap();
    let (a, b, c) = tokio::join!(
        Client::connect(&controller, "a"),
        Client::connect(&controller, "b"),
        Client::connect(&controller, "c")
    );
    for client in [&a, &b, &c] {
        let body = json!({"v":1,"connection_id":client.hello["connection_id"],"binding_id":client.hello["binding_id"],"first_seq":1,"events":[{"seq":1,"event_id":"start","event_type":"probe.status","source_kind":"diagnostic","payload":{"stage":"wechat_collection_started"}}]});
        client.post("events", body).await;
    }
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let pa = a.post("poll", a.scope()).await;
    assert_eq!(pa["commands"][0]["text"], "房间a定时文案");
    a.receipt(&pa["commands"][0]).await;
    let pb = b.post("poll", b.scope()).await;
    assert_eq!(pb["commands"], json!([]));
    assert_eq!(pb["timed_messages"]["generated"], 0);
    let pc = c.post("poll", c.scope()).await;
    assert_eq!(pc["commands"][0]["text"], "房间c定时文案");
    c.receipt(&pc["commands"][0]).await;
    tokio::time::sleep(Duration::from_millis(8100)).await;
    let pb = b.post("poll", b.scope()).await;
    assert_eq!(pb["commands"][0]["text"], "房间b定时文案");
    b.receipt(&pb["commands"][0]).await;
    tokio::join!(a.goodbye(), b.goodbye(), c.goodbye());
    controller.stop().await.unwrap();
}
