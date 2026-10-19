use live_archive_core::moderation::{
    bridge::ProbeBridge,
    fill::{FillOutcome, FillReason, FillReport, FillSession},
    journal::ProbeJournal,
    protocol::{Hello, Poll, ProbeScope},
};
use serde_json::Value;

const ROOM: &str = "https://live.douyin.com/139244611014";
fn pair(bridge: &mut ProbeBridge) -> Value {
    let script = bridge.bootstrap(&bridge.relay_token.clone()).unwrap();
    let config: Value = serde_json::from_str(
        script
            .split("const BOOT = ")
            .nth(1)
            .unwrap()
            .split(";\n")
            .next()
            .unwrap(),
    )
    .unwrap();
    bridge
        .hello(Hello {
            v: 1,
            ticket: config["ticket"].as_str().unwrap().into(),
            page_url: ROOM.into(),
            document_id: "doc".into(),
            adapter_id: "douyin.live_page".into(),
        })
        .unwrap()
}
fn poll(session: &Value) -> Poll {
    Poll {
        v: 1,
        connection_id: session["connection_id"].as_str().unwrap().into(),
        binding_id: session["binding_id"].as_str().unwrap().into(),
        page_url: ROOM.into(),
        input_ready: None,
    }
}
fn report(session: &Value) -> FillReport {
    FillReport {
        v: 1,
        connection_id: session["connection_id"].as_str().unwrap().into(),
        binding_id: session["binding_id"].as_str().unwrap().into(),
        page_url: ROOM.into(),
        action_id: "fill-test-1".into(),
        outcome: FillOutcome::Filled,
        reason: FillReason::InputFilled,
    }
}

#[test]
fn pairing_to_manual_command_and_authenticated_idempotent_receipt_audit() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("probe.sqlite3");
    let mut bridge = ProbeBridge::new(
        ProbeScope::douyin(ROOM.parse().unwrap()).unwrap(),
        ProbeJournal::open(&database).unwrap(),
    );
    bridge
        .enable_fill_preview(Some("场控填入测试（不会发送）".into()))
        .unwrap();
    let session = pair(&mut bridge);
    let token = session["token"].as_str().unwrap();
    assert!(
        bridge.fill_result(token, report(&session)).is_err(),
        "cannot report unissued command"
    );
    let response = bridge.poll(token, poll(&session)).unwrap();
    assert_eq!(response["send_chat"], false);
    assert_eq!(response["commands"][0]["command"], "fill_input");
    assert_eq!(response["commands"][0]["document_id"], "doc");
    assert!(
        bridge.poll(token, poll(&session)).unwrap()["commands"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(bridge.fill_result("wrong-token", report(&session)).is_err());
    let mut changed = report(&session);
    changed.page_url = "https://live.douyin.com/123".into();
    assert!(bridge.fill_result(token, changed).is_err());
    bridge.fill_result(token, report(&session)).unwrap();
    bridge.fill_result(token, report(&session)).unwrap();
    let mut conflict = report(&session);
    conflict.outcome = FillOutcome::Rejected;
    conflict.reason = FillReason::InputBusy;
    assert!(bridge.fill_result(token, conflict).is_err());
    let db = rusqlite::Connection::open(database).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM fill_audits WHERE json_extract(audit_json,'$.state')='filled'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn lost_handoff_and_late_receipt_never_reissue_the_fill_command() {
    let mut fill = FillSession::new(
        "conn".into(),
        "binding".into(),
        "doc".into(),
        ROOM.into(),
        false,
    );
    fill.enqueue(0, "action-1".into(), "测试".into(), 20000);
    assert_eq!(fill.issue(0, false).0.len(), 1);
    assert!(fill.issue(4999, false).0.is_empty());
    let (commands, logs) = fill.issue(5000, false);
    assert!(commands.is_empty());
    assert_eq!(logs[0].state, "unknown");
    let session = serde_json::json!({"connection_id":"conn","binding_id":"binding"});
    let mut late = report(&session);
    late.action_id = "action-1".into();
    fill.report(6000, &late).unwrap();
    assert_eq!(fill.status()["filled"], 1);
    assert!(fill.issue(7000, false).0.is_empty());
}

#[test]
fn queued_expiry_pause_and_stop_cancel_without_handoff() {
    let mut fill = FillSession::new(
        "conn".into(),
        "binding".into(),
        "doc".into(),
        ROOM.into(),
        false,
    );
    fill.enqueue(0, "expired".into(), "测试".into(), 1000);
    let (commands, logs) = fill.issue(1000, false);
    assert!(commands.is_empty());
    assert_eq!(logs[0].state, "expired");
    fill.enqueue(1000, "paused".into(), "测试".into(), 20000);
    assert!(fill.issue(1001, true).0.is_empty());
    assert_eq!(fill.status()["pending"], 0);
    fill.enqueue(2000, "stopped".into(), "测试".into(), 20000);
    fill.cancel(2001, "document_stopped");
    assert!(fill.issue(3000, false).0.is_empty());
}

#[test]
fn default_probe_never_issues_commands_and_fill_text_validation_precedes_pairing() {
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = ProbeBridge::new(
        ProbeScope::douyin(ROOM.parse().unwrap()).unwrap(),
        ProbeJournal::open(&dir.path().join("probe.sqlite3")).unwrap(),
    );
    assert!(
        bridge
            .enable_fill_preview(Some("包含\n换行".into()))
            .is_err()
    );
    let session = pair(&mut bridge);
    let response = bridge
        .poll(session["token"].as_str().unwrap(), poll(&session))
        .unwrap();
    assert!(response["commands"].as_array().unwrap().is_empty());
    assert_eq!(response["mode"], "observe");
    assert!(
        bridge
            .fill_result(session["token"].as_str().unwrap(), report(&session))
            .is_err()
    );
}

#[test]
fn unknown_attempt_and_lost_receipt_do_not_block_later_commands_or_retry_the_old_one() {
    for lost_receipt in [false, true] {
        let mut fill = FillSession::new(
            "conn".into(),
            "binding".into(),
            "doc".into(),
            ROOM.into(),
            false,
        );
        fill.enable_automatic_send(Default::default()).unwrap();
        fill.enqueue(0, "a".into(), "第一条".into(), 20000);
        fill.enqueue(0, "b".into(), "第二条".into(), 20000);
        assert_eq!(fill.issue(0, false).0[0].action_id, "a");
        if !lost_receipt {
            let mut r = report(&serde_json::json!({"connection_id":"conn","binding_id":"binding"}));
            r.action_id = "a".into();
            r.outcome = FillOutcome::Unknown;
            r.reason = FillReason::SubmissionUnconfirmed;
            fill.report(1, &r).unwrap();
            fill.report(2, &r).unwrap();
        }
        assert!(fill.issue(5000, false).0.is_empty());
        assert_eq!(fill.status()["blocked"], false);
        assert!(fill.issue(7999, false).0.is_empty());
        let commands = fill.issue(8000, false).0;
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].action_id, "b");
        assert_eq!(fill.status()["unknown"], 1);
        assert!(fill.issue(13000, false).0.is_empty());
        assert_eq!(fill.status()["blocked"], false);
    }
}
