use live_archive_core::moderation::{
    journal::ProbeJournal, protocol::RawMessageChunk, retention::RetentionPolicy,
};
use rusqlite::{Connection, params};
fn chunk(id: &str, seq: u64, part: u32, parts: u32, size: usize) -> RawMessageChunk {
    RawMessageChunk {
        seq,
        capture_id: id.into(),
        captured_at_ms: 1,
        channel: "native_bridge".into(),
        category: "dispatch".into(),
        encoding: "binary_base64".into(),
        part,
        parts,
        data: "x".repeat(size),
    }
}
fn seed(j: &mut ProbeJournal, run: &str, id: &str, parts: u32, size: usize) {
    let chunks: Vec<_> = (0..parts)
        .map(|part| chunk(id, part as u64 + 1, part, parts, size))
        .collect();
    j.append_raw(run, "connection", &chunks).unwrap();
}
#[test]
fn expiry_keeps_boundary_and_preserved_runs_with_different_audit_age() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("journal.sqlite3");
    let mut j = ProbeJournal::open(&path).unwrap();
    for run in ["expired", "boundary", "recent", "pinned"] {
        seed(&mut j, run, "capture", 2, 100);
    }
    let c = Connection::open(&path).unwrap();
    for (run, time) in [
        ("expired", "2026-10-06T23:59:59.999Z"),
        ("boundary", "2026-10-07T00:00:00.000Z"),
        ("recent", "2026-10-13T00:00:00.000Z"),
        ("pinned", "2026-01-01T00:00:00.000Z"),
    ] {
        c.execute(
            "UPDATE raw_messages SET received_at=?1 WHERE run_id=?2",
            params![time, run],
        )
        .unwrap();
        // Usage timestamps normally come from insert triggers; fixture dating updates both.
        c.execute(
            "UPDATE raw_capture_usage SET received_at=?1 WHERE run_id=?2",
            params![time, run],
        )
        .unwrap();
    }
    for table in [
        "probe_events",
        "preview_audits",
        "candidate_draft_audits",
        "fill_audits",
        "delivery_audits",
    ] {
        for (run, at) in [
            ("expired", "2026-09-13T23:59:59.999Z"),
            ("boundary", "2026-09-14T00:00:00.000Z"),
            ("recent", "2026-10-06T00:00:00.000Z"),
            ("pinned", "2026-01-01T00:00:00.000Z"),
        ] {
            let sql = if table == "probe_events" {
                format!(
                    "INSERT INTO {table}(run_id,connection_id,seq,event_json,received_at) VALUES(?1,'connection',1,'{{}}',?2)"
                )
            } else if table == "delivery_audits" {
                format!(
                    "INSERT INTO {table}(run_id,connection_id,action_id,audit_json,received_at) VALUES(?1,'connection','action','{{}}',?2)"
                )
            } else {
                format!(
                    "INSERT INTO {table}(run_id,connection_id,audit_json,received_at) VALUES(?1,'connection','{{}}',?2)"
                )
            };
            c.execute(&sql, params![run, at]).unwrap();
        }
    }
    let now = chrono::DateTime::parse_from_rfc3339("2026-10-14T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    let p = RetentionPolicy {
        preserved_runs: vec!["pinned".into()],
        ..Default::default()
    };
    let r = j.cleanup(&p, now, false).unwrap();
    assert_eq!(r.raw_captures_deleted, 1);
    assert_eq!(r.audit_rows_deleted, 5);
    assert_eq!(
        c.query_row("SELECT count(*) FROM raw_messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        6
    );
    for table in [
        "probe_events",
        "preview_audits",
        "candidate_draft_audits",
        "fill_audits",
        "delivery_audits",
    ] {
        assert_eq!(
            c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
    }
    assert_eq!(j.cleanup(&p, now, false).unwrap().raw_captures_deleted, 0);
    let disabled = RetentionPolicy {
        enabled: false,
        ..Default::default()
    };
    j.cleanup(&disabled, now + 400 * 86400000, false).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM raw_messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        6
    );
}
#[test]
fn capacity_evicts_whole_oldest_captures_and_reports_pinned_overage() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("j.sqlite3");
    let mut j = ProbeJournal::open(&path).unwrap();
    for run in ["oldest", "newer", "pinned"] {
        seed(&mut j, run, "c", 30, 24576);
    }
    j.append_raw(
        "inflight",
        "connection",
        &[chunk("inflight", 1, 0, 2, 24576)],
    )
    .unwrap();
    let c = Connection::open(&path).unwrap();
    c.execute(
        "UPDATE raw_capture_usage SET received_at='2026-10-12T00:00:00.000Z' WHERE run_id='oldest'",
        [],
    )
    .unwrap();
    c.execute(
        "UPDATE raw_capture_usage SET received_at='2026-10-13T00:00:00.000Z' WHERE run_id='newer'",
        [],
    )
    .unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let p = RetentionPolicy {
        raw_max_mib: 1,
        preserved_runs: vec!["pinned".into()],
        ..Default::default()
    };
    let r = j.cleanup(&p, now, false).unwrap();
    assert_eq!(r.raw_captures_deleted, 2);
    assert!(!r.over_limit);
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM raw_messages WHERE run_id='pinned'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        30
    );
    assert_eq!(
        c.query_row(
            "SELECT count(*) FROM raw_messages WHERE run_id='inflight'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    seed(&mut j, "pinned-other", "c", 30, 24576);
    let p = RetentionPolicy {
        preserved_runs: vec!["pinned".into(), "pinned-other".into()],
        ..p
    };
    assert!(j.cleanup(&p, now, false).unwrap().over_limit);
}
#[test]
fn old_database_migrates_usage_and_idle_reclamation_shrinks_the_file() {
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("j.sqlite3");
    let mut j = ProbeJournal::open(&path).unwrap();
    seed(&mut j, "old", "c", 900, 24576);
    drop(j);
    let c = Connection::open(&path).unwrap();
    c.execute_batch("DROP TRIGGER raw_usage_insert; DROP TRIGGER raw_usage_delete; DROP TABLE raw_capture_usage; UPDATE raw_messages SET received_at='2025-01-01T00:00:00.000Z'; PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
    drop(c);
    let before = std::fs::metadata(&path).unwrap().len();
    assert!(before > 16 * 1024 * 1024);
    let mut j = ProbeJournal::open(&path).unwrap();
    let c = Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("SELECT chunks FROM raw_capture_usage", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        900
    );
    drop(c);
    let report = j
        .cleanup(
            &RetentionPolicy::default(),
            chrono::Utc::now().timestamp_millis(),
            true,
        )
        .unwrap();
    assert_eq!(report.raw_captures_deleted, 1);
    let after = std::fs::metadata(&path).unwrap().len();
    assert!(after < before / 2);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_http_retry_cannot_resurrect_a_capture_removed_by_retention() {
    use live_archive_core::moderation::{
        bridge::ProbeBridge, http::ProbeServer, protocol::ProbeScope,
    };
    async fn post(
        c: &reqwest::Client,
        base: &str,
        path: &str,
        token: &str,
        body: &serde_json::Value,
    ) -> serde_json::Value {
        let r = c
            .post(format!("{base}/v1/{path}"))
            .header("Origin", "https://live.douyin.com")
            .header("Content-Type", "application/json")
            .bearer_auth(token)
            .body(serde_json::to_vec(body).unwrap())
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success());
        serde_json::from_str(&r.text().await.unwrap()).unwrap()
    }
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("j.sqlite3");
    let mut b = ProbeBridge::new(
        ProbeScope::from_room_url("https://live.douyin.com/123".parse().unwrap()).unwrap(),
        ProbeJournal::open(&path).unwrap(),
    );
    b.configure_raw_messages(true).unwrap();
    b.configure_retention(RetentionPolicy {
        raw_max_mib: 1,
        ..Default::default()
    })
    .unwrap();
    let js = b.bootstrap(&b.relay_token.clone()).unwrap();
    let cfg: serde_json::Value = serde_json::from_str(
        js.split("const BOOT = ")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap(),
    )
    .unwrap();
    let mut s = ProbeServer::bind(b).await.unwrap();
    let client = reqwest::Client::new();
    let base = format!("http://{}", s.address);
    let hello=post(&client,&base,"hello","",&serde_json::json!({"v":1,"ticket":cfg["ticket"],"page_url":"https://live.douyin.com/123","document_id":"fixture","adapter_id":"douyin.live_page"})).await;
    let mut last = serde_json::Value::Null;
    for start in (0..45).step_by(3) {
        let chunks: Vec<_> = (start..start + 3)
            .map(|i| chunk("big", i as u64 + 1, i, 45, 24576))
            .collect();
        last = serde_json::json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"chunks":chunks});
        post(
            &client,
            &base,
            "raw",
            hello["token"].as_str().unwrap(),
            &last,
        )
        .await;
    }
    s.bridge.lock().unwrap().maintain_journal(false).unwrap();
    let res = post(
        &client,
        &base,
        "raw",
        hello["token"].as_str().unwrap(),
        &last,
    )
    .await;
    assert_eq!(res["acked_seq"], 45);
    let c = Connection::open(&path).unwrap();
    assert_eq!(
        c.query_row("SELECT count(*) FROM raw_messages", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    s.stop().await.unwrap();
}
#[test]
fn yaml_retention_defaults_overrides_and_invalid_policy_preflight() {
    use live_archive_core::moderation::config::ModerationConfig;
    let t = tempfile::tempdir().unwrap();
    let cfg = t.path().join("config.yml");
    let base = "version: 1\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/123\n";
    std::fs::write(&cfg, base).unwrap();
    let p = ModerationConfig::load(&cfg)
        .unwrap()
        .probe_options()
        .retention;
    assert!(p.enabled);
    assert_eq!(p.raw_days, 7);
    assert_eq!(p.raw_max_mib, 2048);
    assert_eq!(p.audit_days, 30);
    std::fs::write(&cfg,format!("{base}storage:\n  retention:\n    enabled: false\n    raw_days: 2\n    raw_max_mib: 100\n    audit_days: 10\n    preserved_runs: ['fixture-run']\n")).unwrap();
    let p = ModerationConfig::load(&cfg)
        .unwrap()
        .probe_options()
        .retention;
    assert!(!p.enabled);
    assert_eq!(p.raw_days, 2);
    assert_eq!(p.raw_max_mib, 100);
    assert_eq!(p.audit_days, 10);
    assert_eq!(p.preserved_runs, vec!["fixture-run"]);
    for entry in [
        "raw_days: 0",
        "audit_days: 0",
        "raw_max_mib: 0",
        "preserved_runs: ['invalid run']",
        "unexpected: true",
    ] {
        std::fs::write(
            &cfg,
            format!("{base}storage:\n  directory: must-not-exist\n  retention:\n    {entry}\n"),
        )
        .unwrap();
        let res = std::process::Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
            .args(["moderation", "check", "--config"])
            .arg(&cfg)
            .output()
            .unwrap();
        assert!(!res.status.success());
        assert!(!t.path().join("must-not-exist").exists());
    }
}
