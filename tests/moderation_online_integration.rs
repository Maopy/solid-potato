use live_archive_core::moderation::{
    bridge::ProbeBridge, config::ModerationConfig, http::ProbeServer, journal::ProbeJournal,
    protocol::ProbeScope,
};
use std::{path::PathBuf, process::Command};

/// Exercises injected assets and a real HTTP server, gzip, exact 64-bit IDs,
/// durable journal, sliding rules and gap cancellation, with no real room.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn injected_decoder_to_http_bridge_and_rules_preview() {
    exercise_online(false, false, false, false, false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn injected_decoder_to_rate_limited_preview_and_journal() {
    exercise_online(true, false, false, false, false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gifts_to_fill_only_executor_receipt_retry_and_busy_input() {
    exercise_online(true, true, false, false, false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn name_only_gifts_fill_and_wait_for_manual_send_without_overwriting() {
    exercise_online(true, true, true, false, false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn name_only_gifts_submit_once_with_receipt_retry_and_rate_limit() {
    exercise_online(true, true, true, true, false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn douyin_custom_format_and_emoji_submit_unchanged_through_receipt_retry() {
    exercise_online(true, true, true, true, true, false, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn douyin_user_id_nickname_table_reaches_input_and_preserves_receipt_retry_text() {
    exercise_online(true, true, true, true, false, true, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn douyin_periodic_text_submits_without_gifts_or_platform_requests() {
    exercise_online(true, true, true, true, false, false, true).await;
}
async fn exercise_online(
    scheduled: bool,
    filling: bool,
    names_only: bool,
    automatic: bool,
    emoji: bool,
    aliases: bool,
    timed: bool,
) {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("probe.sqlite3");
    let config_path = directory.path().join("moderation.yml");
    let mode = if filling {
        "fill_only"
    } else if scheduled {
        "scheduler_preview"
    } else {
        "rules_preview"
    };
    std::fs::write(&config_path, format!("version: 1\nmode: {mode}\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/123\nrules:\n  gift:\n    emojis: []\n    template: '配置：感谢{{nickname}}送来的{{gift_list}}'\n")).unwrap();
    if names_only {
        let source = std::fs::read_to_string(&config_path).unwrap().replace(
            "  gift:\n    emojis: []\n",
            "  gift:\n    emojis: []\n    show_quantity: false\n",
        );
        std::fs::write(&config_path, source).unwrap();
    }
    if automatic {
        let source = std::fs::read_to_string(&config_path).unwrap()
            + "sending:\n  automatic: true\ndelivery:\n  actor_user_id: '9007199254740993999'\n";
        std::fs::write(&config_path, source).unwrap();
    }
    if emoji {
        let source = std::fs::read_to_string(&config_path)
            .unwrap()
            .replace("emojis: []", "emojis: ['👩‍💻']")
            .replace(
                "配置：感谢{nickname}送来的{gift_list}",
                "谢谢{nickname}的{gift_list}",
            );
        std::fs::write(&config_path, source).unwrap();
    }
    if aliases {
        std::fs::write(directory.path().join("names.yml"),"version: 1\nplatforms:\n  douyin:\n    '9007199254740993123': 霸气哥\n  wechat_channels:\n    '9007199254740993123': 不应使用的跨平台昵称\n").unwrap();
        let source = std::fs::read_to_string(&config_path).unwrap()
            + "nickname_mapping:\n  file: names.yml\n";
        std::fs::write(&config_path, source).unwrap();
    }
    if timed {
        let source = std::fs::read_to_string(&config_path).unwrap()
            + "timed_messages:\n  enabled: true\n  interval_seconds: 1\n  messages: ['定时文案甲','定时文案乙']\n";
        std::fs::write(&config_path, source).unwrap();
    }
    let config = ModerationConfig::load(&config_path).unwrap();
    let options = config.probe_options();
    let mut bridge = ProbeBridge::new(
        ProbeScope::douyin("https://live.douyin.com/123".parse().unwrap()).unwrap(),
        ProbeJournal::open(&database).unwrap(),
    );
    bridge
        .configure_retention(options.retention.clone())
        .unwrap();
    bridge.maintain_journal(true).unwrap();
    bridge.configure_raw_messages(options.raw_messages).unwrap();
    bridge.configure_delivery(options.delivery.clone()).unwrap();
    bridge
        .configure_timed(options.timed_messages.clone())
        .unwrap();
    bridge
        .configure(config.rules_config(), config.scheduler.clone())
        .unwrap();
    assert!(options.rules_preview);
    bridge.enable_rules_preview();
    if options.scheduler_preview {
        bridge.enable_scheduler_preview();
    }
    if options.fill_preview {
        bridge.enable_fill_preview(options.fill_test_text).unwrap();
    }
    if options.automatic_send {
        bridge.enable_automatic_send().unwrap();
    }
    let script = bridge.bootstrap(&bridge.relay_token.clone()).unwrap();
    let bootstrap = directory.path().join("bootstrap.js");
    std::fs::write(&bootstrap, script).unwrap();
    let mut server = ProbeServer::bind(bridge).await.unwrap();
    let address = server.address.to_string();
    let driver = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/support/moderation_online_driver.cjs");
    let node = std::env::var_os("MODERATION_NODE").unwrap_or_else(|| "node".into());
    let result = tokio::task::spawn_blocking(move || {
        Command::new(node)
            .arg(driver)
            .arg(bootstrap)
            .arg(address)
            .arg(if timed {
                "auto_timed"
            } else if aliases {
                "auto_alias"
            } else if emoji {
                "auto_emoji"
            } else if automatic {
                "auto"
            } else if names_only {
                "fill_names"
            } else if filling {
                "fill"
            } else if scheduled {
                "scheduled"
            } else {
                "immediate"
            })
            .output()
    })
    .await
    .unwrap();
    server.stop().await.unwrap();
    let result = result.expect("Node.js 22+ required; set MODERATION_NODE to its executable path");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    if timed {
        assert_eq!(summary["submissions"], 2);
        assert_eq!(summary["input_writes"], 2);
        let c = rusqlite::Connection::open(database).unwrap();
        let n:i64=c.query_row("select count(*) from fill_audits where json_extract(audit_json,'$.state')='submitted'",[],|r|r.get(0)).unwrap();
        assert_eq!(n, 2);
        return;
    }
    assert_eq!(summary["send_chat"], automatic);
    assert_eq!(summary["scheduler_preview"], scheduled);
    assert_eq!(summary["fill_only"], filling);
    let connection = rusqlite::Connection::open(database).unwrap();
    if automatic {
        let count:i64=connection.query_row("select count(*) from delivery_audits where json_extract(audit_json,'$.state')='echo_observed' and json_extract(audit_json,'$.source')='douyin_ws'",[],|r|r.get(0)).unwrap();
        assert_eq!(
            count, 0,
            "automatic submission no longer waits for platform echoes"
        );
    }
    let prices:i64=connection.query_row("select count(*) from probe_events where json_extract(event_json,'$.event_type')='gift.observed' and json_extract(event_json,'$.payload.unit_value')=1000",[],|r|r.get(0)).unwrap();
    assert!(prices >= 1);
    let planned: i64 = connection.query_row("SELECT COUNT(*) FROM preview_audits WHERE json_extract(audit_json, '$.kind')='planned'", [], |row| row.get(0)).unwrap();
    // Arrival stays in the 30-second collection window and is cancelled by upgrade.
    assert_eq!(planned, 3);
    let batched:i64=connection.query_row("SELECT COUNT(*) FROM preview_audits WHERE json_extract(audit_json,'$.reason')='welcome_batched'",[],|row|row.get(0)).unwrap();
    assert_eq!(batched, 1);
    let cancelled: i64 = connection.query_row("SELECT COUNT(*) FROM preview_audits WHERE json_extract(audit_json, '$.kind')='cancelled'", [], |row| row.get(0)).unwrap();
    assert_eq!(cancelled, if scheduled { 3 } else { 2 });
    if scheduled {
        let dispatches: i64 = connection.query_row("SELECT COUNT(*) FROM preview_audits WHERE json_extract(audit_json, '$.kind')='dispatch_preview'", [], |row| row.get(0)).unwrap();
        assert_eq!(dispatches, 2);
    }
    let filled: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM fill_audits WHERE json_extract(audit_json, '$.state')='filled'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(filled, if filling && !automatic { 1 } else { 0 });
    if automatic {
        let submitted:i64=connection.query_row("SELECT COUNT(*) FROM fill_audits WHERE json_extract(audit_json,'$.state')='submitted'",[],|r|r.get(0)).unwrap();
        assert_eq!(submitted, 2);
    }
    if filling {
        let busy: i64 = connection.query_row("SELECT COUNT(*) FROM fill_audits WHERE json_extract(audit_json, '$.reason')='input_busy'", [], |row| row.get(0)).unwrap();
        assert_eq!(busy, if names_only { 0 } else { 1 });
    }
    let gift: String = connection.query_row("SELECT event_json FROM probe_events WHERE json_extract(event_json, '$.event_type')='gift.observed' LIMIT 1", [], |row| row.get(0)).unwrap();
    assert!(gift.contains("9007199254740993123"));
    assert!(!gift.contains("cookie"));
}
