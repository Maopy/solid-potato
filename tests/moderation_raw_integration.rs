use live_archive_core::moderation::{
    bridge::ProbeBridge, config::ModerationConfig, http::ProbeServer, journal::ProbeJournal,
};
use std::{path::Path, process::Command};
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wechat_raw_callbacks_survive_sampling_quota_duplicates_and_ack_loss() {
    exercise("wechat_channels", true, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn douyin_raw_frames_keep_unknown_json_and_binary_before_decoding() {
    exercise("douyin", true, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_capture_can_be_disabled_without_changing_observation() {
    exercise("wechat_channels", false, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controller_page_binding_stops_collection_on_native_room_join() {
    exercise("wechat_channels", true, true).await;
}
async fn exercise(platform: &str, enabled: bool, task: bool) {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("probe.sqlite3");
    let yaml = temp.path().join("config.yml");
    let (adapter, url) = if platform == "wechat_channels" {
        (
            "wechat_channels.desktop_probe",
            "https://channels.weixin.qq.com/web/pages/live",
        )
    } else {
        ("douyin.live_page", "https://live.douyin.com/123")
    };
    std::fs::write(&yaml,format!("version: 1\nroom:\n  platform: {platform}\n  adapter: {adapter}\n  url: {url}\nstorage:\n  raw_messages: {enabled}\n")).unwrap();
    let config = ModerationConfig::load(&yaml).unwrap();
    assert_eq!(config.probe_options().raw_messages, enabled);
    let mut bridge = ProbeBridge::new(
        live_archive_core::moderation::protocol::ProbeScope::from_room_url(config.room.url.clone())
            .unwrap(),
        ProbeJournal::open(&db).unwrap(),
    );
    if task {
        bridge
            .configure_task(
                "fixture",
                std::sync::Arc::new(std::sync::Mutex::new(
                    live_archive_core::moderation::account_budget::AccountBudget::new(
                        Default::default(),
                    )
                    .unwrap(),
                )),
            )
            .unwrap();
    }
    bridge
        .configure_retention(config.probe_options().retention)
        .unwrap();
    bridge.maintain_journal(true).unwrap();
    bridge
        .configure_raw_messages(config.probe_options().raw_messages)
        .unwrap();
    let bootstrap = temp.path().join("bootstrap.js");
    std::fs::write(
        &bootstrap,
        bridge.bootstrap(&bridge.relay_token.clone()).unwrap(),
    )
    .unwrap();
    let mut server = ProbeServer::bind(bridge).await.unwrap();
    let address = server.address.to_string();
    let p = platform.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        Command::new(std::env::var_os("MODERATION_NODE").unwrap_or_else(|| "node".into()))
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/support/moderation_raw_driver.cjs"),
            )
            .arg(bootstrap)
            .arg(address)
            .arg(p)
            .arg(enabled.to_string())
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    server.stop().await.unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let c = rusqlite::Connection::open(&db).unwrap();
    let records: i64 = c
        .query_row(
            "SELECT count(DISTINCT capture_id) FROM raw_messages",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if !enabled {
        assert_eq!(records, 0);
        return;
    }
    assert_eq!(records, if platform == "douyin" { 6 } else { 5 });
    if platform == "douyin" {
        let binary: String = c
            .query_row(
                "SELECT data FROM raw_messages WHERE encoding='binary_base64'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(binary, "AP8REg==");
    }
    let gaps: i64 = c
        .query_row(
            "SELECT count(*) FROM raw_messages WHERE encoding='gap'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(gaps, 2);
    let dupes:i64=c.query_row("SELECT count(*) FROM (SELECT run_id,connection_id,seq,count(*) n FROM raw_messages GROUP BY run_id,connection_id,seq HAVING n>1)",[],|r|r.get(0)).unwrap();
    assert_eq!(dupes, 0);
    let rows:Vec<(String,String,i64,i64)>=c.prepare("SELECT capture_id,group_concat(data,''),count(*),max(parts) FROM (SELECT * FROM raw_messages WHERE encoding='json_utf8_base64' ORDER BY seq) GROUP BY capture_id ORDER BY min(seq)").unwrap().query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(rows.len(), 3);
    assert!(rows[0].2 > 1);
    assert_eq!(
        rows[0].1, rows[1].1,
        "duplicate source callbacks must remain separate records"
    );
    for row in &rows {
        assert_eq!(row.2, row.3, "all chunks must be durable");
    }
    // Decode persisted chunks, rather than only checking the browser counters.
    let out = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .args(["-X", "utf8"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/read_moderation_raw.py"))
        .arg(&db)
        .arg("--contains")
        .arg("陌")
        .arg("--limit")
        .arg("10")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let decoded: Vec<serde_json::Value> = String::from_utf8(out.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(decoded.len(), 2);
    assert!(decoded.iter().all(|v| v["complete"] == true));
    let mut v = decoded[0]["message"].clone();
    if platform == "wechat_channels" {
        v = v[0].clone();
    } else {
        v = serde_json::from_str(v.as_str().unwrap()).unwrap();
    }
    assert_eq!(v["app_msg_list"].as_array().unwrap().len(), 80);
    assert_eq!(
        v["app_msg_list"][79]["from_user_contact"]["contact"]["nickname"],
        "陌**"
    );
    assert_eq!(v["unknown_message"]["large"].as_str().unwrap().len(), 70000);
    assert_eq!(v["cookie"], "[REDACTED]");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("do-not-store"));
    let raw_events:i64=c.query_row("SELECT count(*) FROM probe_events WHERE json_extract(event_json,'$.event_type')='raw.message'",[],|r|r.get(0)).unwrap();
    assert_eq!(raw_events, 0);
    let candidates:i64=c.query_row("SELECT count(*) FROM probe_events WHERE json_extract(event_json,'$.event_type')='gift.candidate'",[],|r|r.get(0)).unwrap();
    assert_eq!(
        candidates,
        if platform == "wechat_channels" { 80 } else { 0 }
    );
    let fills: i64 = c
        .query_row("SELECT count(*) FROM fill_audits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fills, 0);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_upload_through_shipped_same_origin_proxy_reaches_durable_journal() {
    use live_archive_core::moderation::{inject::render_addon, protocol::ProbeScope};
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("j.sqlite3");
    let scope = ProbeScope::from_room_url(
        "https://channels.weixin.qq.com/web/pages/live"
            .parse()
            .unwrap(),
    )
    .unwrap();
    let mut b = ProbeBridge::new(scope.clone(), ProbeJournal::open(&path).unwrap());
    b.configure_raw_messages(true).unwrap();
    let relay = b.relay_token.clone();
    let script = b.bootstrap(&relay).unwrap();
    let boot: serde_json::Value = serde_json::from_str(
        script
            .split("const BOOT = ")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap(),
    )
    .unwrap();
    let mut s = ProbeServer::bind(b).await.unwrap();
    let addon = t.path().join("addon.py");
    let cfg = t.path().join("boot.json");
    std::fs::write(&addon, render_addon(&scope, s.address, &relay)).unwrap();
    std::fs::write(&cfg, serde_json::to_vec(&boot).unwrap()).unwrap();
    let result = tokio::task::spawn_blocking(move || {
        Command::new(if cfg!(windows) { "python" } else { "python3" })
            .args(["-X", "utf8"])
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/support/moderation_proxy_route_driver.py"),
            )
            .arg(addon)
            .arg(cfg)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    s.stop().await.unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let c = rusqlite::Connection::open(path).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM raw_messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
