use live_archive_core::moderation::{journal::ProbeJournal, protocol::ProbeEvent};
use serde_json::{Value, json};
use std::{path::Path, process::Command};

fn gift(
    seq: u64,
    id: &str,
    user: &str,
    product: &str,
    count: u64,
    combo: Option<u64>,
) -> ProbeEvent {
    let mut payload = json!({"gift_id":product,"gift_name":"private-product-name","reward_product_count":count,
        "quantity_mode":"unknown","confidence":"candidate","live_eligible":false,"room_verified":false,
        "message_type":20009,"decoder":"wechat.app20009.reward.candidate.v1","is_floatmsg":0,"float_type":1});
    if let Some(c) = combo {
        payload["combo_product_count"] = json!(c);
    }
    ProbeEvent {
        seq,
        event_id: format!("private-event-{seq}"),
        event_type: "gift.candidate".into(),
        source_kind: "network".into(),
        native_event_id: Some(id.into()),
        user_id: Some(user.into()),
        nickname: Some("private-nickname".into()),
        payload,
    }
}
fn cli(path: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
        .args(["moderation", "inspect-gifts", "--journal"])
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}
fn report(output: std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("private-"));
    serde_json::from_str(&text).unwrap()
}

#[test]
fn cli_journal_sequences_are_anonymous_read_only_and_never_quantity_mappings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("probe.sqlite3");
    let mut journal = ProbeJournal::open(&path).unwrap();
    journal
        .append(
            "private-old-run",
            "private-old-doc",
            &[gift(
                1,
                "private-old-id",
                "private-user",
                "private-product",
                66,
                Some(66),
            )],
        )
        .unwrap();
    let mut duplicate = gift(
        4,
        "private-m2",
        "private-user",
        "private-product",
        2,
        Some(3),
    );
    duplicate.payload["is_floatmsg"] = json!(1);
    journal
        .append(
            "private-new-run",
            "private-doc-1",
            &[
                gift(
                    1,
                    "private-m1",
                    "private-user",
                    "private-product",
                    1,
                    Some(1),
                ),
                gift(
                    2,
                    "private-m2",
                    "private-user",
                    "private-product",
                    2,
                    Some(3),
                ),
                gift(
                    3,
                    "private-m3",
                    "private-user",
                    "private-product",
                    3,
                    Some(0),
                ),
                duplicate,
                gift(
                    5,
                    "private-m2",
                    "private-user",
                    "private-product",
                    4,
                    Some(7),
                ),
                gift(
                    6,
                    "private-m4",
                    "private-other-user",
                    "private-product",
                    66,
                    Some(66),
                ),
                gift(
                    7,
                    "private-m5",
                    "private-user",
                    "private-other-product",
                    1,
                    None,
                ),
            ],
        )
        .unwrap();
    let mut invalid = gift(
        2,
        "private-bad",
        "private-user",
        "private-product",
        1,
        Some(1),
    );
    invalid.payload["combo_product_count"] = json!(-1);
    let mut unsupported = gift(
        3,
        "private-unsupported",
        "private-user",
        "private-product",
        1,
        Some(1),
    );
    unsupported.payload["decoder"] = json!("private-other-decoder");
    journal
        .append(
            "private-new-run",
            "private-doc-2",
            &[
                gift(
                    1,
                    "private-m1",
                    "private-user",
                    "private-product",
                    1,
                    Some(1),
                ),
                invalid,
                unsupported,
            ],
        )
        .unwrap();
    drop(journal);
    let before = std::fs::read(&path).unwrap();
    let r = report(cli(&path, &[]));
    assert_eq!(r["available_rows"], 10);
    assert_eq!(r["scanned_rows"], 10);
    assert_eq!(r["rejected_rows"], 2);
    assert_eq!(r["unique_messages"], 6);
    assert_eq!(r["duplicate_messages"], 1);
    assert_eq!(r["conflicting_messages"], 1);
    assert_eq!(r["group_count"], 4);
    assert_eq!(r["quantity_mode"], "unknown");
    assert_eq!(r["live_eligible"], false);
    let g = &r["groups"][0];
    assert_eq!(g["messages"], 3);
    assert_eq!(g["matching_count_rises"], 1);
    assert_eq!(g["non_increases"], 1);
    assert_eq!(g["samples"][1]["combo_count_change"], 2);
    assert_eq!(g["samples"][2]["combo_count_change"], -3);
    assert_eq!(g["samples"][0]["float_type"], 1);
    assert_eq!(r["groups"][3]["document"], 2);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let limited = report(cli(&path, &["--limit", "2"]));
    assert_eq!(limited["scanned_rows"], 2);
    assert_eq!(limited["input_truncated"], true);
    assert_eq!(
        report(cli(&path, &["--run-id", "private-old-run"]))["unique_messages"],
        1
    );
    assert_eq!(
        report(cli(&path, &["--run-id", "private-no-run"]))["run_found"],
        false
    );
    let mut journal = ProbeJournal::open(&path).unwrap();
    let mut status = gift(1, "unused", "unused", "unused", 1, None);
    status.event_type = "probe.status".into();
    status.payload = json!({"stage":"ready"});
    journal
        .append("private-latest-empty", "private-doc", &[status])
        .unwrap();
    drop(journal);
    let empty = report(cli(&path, &[]));
    assert_eq!(empty["available_rows"], 0);
    assert_eq!(empty["run_found"], true);
}

#[test]
fn cli_caps_evidence_and_does_not_create_missing_journals() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("probe.sqlite3");
    let mut journal = ProbeJournal::open(&path).unwrap();
    let sequence = (1..=35)
        .map(|i| {
            gift(
                i,
                &format!("private-m{i}"),
                "private-user",
                "private-product",
                1,
                Some(i),
            )
        })
        .collect::<Vec<_>>();
    journal
        .append("private-run", "private-doc", &sequence)
        .unwrap();
    let groups = (36..=90)
        .map(|i| {
            gift(
                i,
                &format!("private-m{i}"),
                "private-user",
                &format!("private-product-{i}"),
                1,
                None,
            )
        })
        .collect::<Vec<_>>();
    journal
        .append("private-run", "private-doc", &groups)
        .unwrap();
    drop(journal);
    let r = report(cli(&path, &[]));
    assert_eq!(r["groups"].as_array().unwrap().len(), 50);
    assert_eq!(r["omitted_groups"], 6);
    assert_eq!(r["groups"][0]["samples"].as_array().unwrap().len(), 32);
    assert_eq!(r["groups"][0]["samples_truncated"], true);
    assert!(!cli(&path, &["--limit", "0"]).status.success());
    assert!(!cli(&path, &["--limit", "10001"]).status.success());
    let absent = dir.path().join("absent.sqlite3");
    assert!(!cli(&absent, &[]).status.success());
    assert!(!absent.exists());
}

#[test]
fn report_separates_source_targets_and_keeps_zero_count_patterns_unverified() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("probe.sqlite3");
    let mut a = gift(
        1,
        "private-m1",
        "private-user",
        "private-product",
        1,
        Some(1),
    );
    a.payload["recipient_user_id"] = json!("private-target-a");
    let mut b = gift(
        2,
        "private-m2",
        "private-user",
        "private-product",
        4,
        Some(4),
    );
    b.payload["recipient_user_id"] = json!("private-target-b");
    let mut c = gift(
        3,
        "private-m3",
        "private-user",
        "private-product",
        4,
        Some(5),
    );
    c.payload["recipient_user_id"] = json!("private-target-a");
    let mut tail = gift(
        4,
        "private-m4",
        "private-user",
        "private-product",
        5,
        Some(0),
    );
    tail.payload["recipient_user_id"] = json!("private-target-a");
    let unknown = gift(
        5,
        "private-m5",
        "private-user",
        "private-product",
        5,
        Some(0),
    );
    let mut changed = a.clone();
    changed.seq = 6;
    changed.event_id = "private-event-6".into();
    changed.payload["recipient_user_id"] = json!("private-target-b");
    let mut invalid = gift(
        7,
        "private-m7",
        "private-user",
        "private-product",
        1,
        Some(1),
    );
    invalid.payload["recipient_user_id"] = json!({"token":"private-token"});
    let mut journal = ProbeJournal::open(&path).unwrap();
    journal
        .append(
            "private-run",
            "private-doc",
            &[a, b, c, tail, unknown, changed, invalid],
        )
        .unwrap();
    drop(journal);
    let database = rusqlite::Connection::open(&path).unwrap();
    for (seq, time) in [
        (1, "2026-10-13T10:00:00.000Z"),
        (3, "2026-10-13T10:00:01.000Z"),
        (4, "2026-10-13T10:00:02.500Z"),
    ] {
        database
            .execute(
                "UPDATE probe_events SET received_at=?1 WHERE seq=?2",
                rusqlite::params![time, seq],
            )
            .unwrap();
    }
    drop(database);
    let r = report(cli(&path, &[]));
    assert_eq!(r["unique_messages"], 5);
    assert_eq!(r["conflicting_messages"], 1);
    assert_eq!(r["rejected_rows"], 1);
    assert_eq!(r["group_count"], 3);
    let g = &r["groups"][0];
    assert_eq!(g["recipient"], 1);
    assert_eq!(g["messages"], 3);
    assert_eq!(g["zero_matching_previous_total"], 1);
    assert_eq!(g["samples"][2]["receipt_gap_ms"], 1500);
    assert_eq!(
        g["samples"][2]["count_pattern"],
        "zero_matching_previous_total"
    );
    assert_eq!(r["groups"][1]["recipient"], 2);
    assert_eq!(
        r["groups"][1]["samples"][0]["combo_count_change"],
        Value::Null
    );
    assert_eq!(r["groups"][2]["recipient"], Value::Null);
    assert_eq!(r["groups"][2]["zero_other"], 1);
    assert_eq!(r["quantity_mode"], "unknown");
    assert_eq!(r["live_eligible"], false);
}

#[test]
fn revisions_keep_zero_count_evidence_without_counting_another_gift() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("probe.sqlite3");
    let base = gift(
        1,
        "private-id",
        "private-user",
        "private-product",
        1,
        Some(1),
    );
    let mut zero = gift(
        2,
        "private-id",
        "private-user",
        "private-product",
        0,
        Some(0),
    );
    zero.event_type = "gift.candidate_revision".into();
    zero.payload["previous_reward_product_count"] = json!(1);
    zero.payload["previous_combo_product_count"] = json!(1);
    zero.payload["reason"] = json!("same_message_changed");
    let mut update = zero.clone();
    update.seq = 3;
    update.event_id = "private-event-3".into();
    update.payload["reward_product_count"] = json!(2);
    update.payload["combo_product_count"] = json!(2);
    let mut invalid = update.clone();
    invalid.seq = 4;
    invalid.event_id = "private-event-4".into();
    invalid.payload["previous_reward_product_count"] = json!(-1);
    let mut journal = ProbeJournal::open(&path).unwrap();
    journal
        .append("private-run", "private-doc", &[base, zero, update, invalid])
        .unwrap();
    drop(journal);
    let c = rusqlite::Connection::open(&path).unwrap();
    c.execute(
        "UPDATE probe_events SET received_at='2026-10-13T10:00:00.000Z' WHERE seq=1",
        [],
    )
    .unwrap();
    c.execute(
        "UPDATE probe_events SET received_at='2026-10-13T10:00:02.000Z' WHERE seq=2",
        [],
    )
    .unwrap();
    drop(c);
    let r = report(cli(&path, &[]));
    assert_eq!(r["available_rows"], 4);
    assert_eq!(r["unique_messages"], 1);
    assert_eq!(r["revision_rows"], 2);
    assert_eq!(r["rejected_rows"], 1);
    assert_eq!(r["groups"][0]["messages"], 1);
    assert_eq!(r["revisions"][0]["prior_message_seen"], true);
    assert_eq!(
        r["revisions"][0]["source_message"],
        r["groups"][0]["samples"][0]["source_message"]
    );
    assert_eq!(r["revisions"][0]["reward_product_count"], 0);
    assert_eq!(r["revisions"][0]["since_first_candidate_ms"], 2000);
    assert_eq!(r["quantity_mode"], "unknown");
    assert_eq!(r["live_eligible"], false);
    let cut = report(cli(&path, &["--limit", "2"]));
    assert_eq!(cut["unique_messages"], 0);
    assert_eq!(cut["revision_rows"], 1);
    assert_eq!(cut["unmatched_revisions"], 1);
    assert_eq!(cut["revisions"][0]["since_first_candidate_ms"], Value::Null);
}
