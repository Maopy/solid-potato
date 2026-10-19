use live_archive_core::moderation::{
    bridge::ProbeBridge, config::ModerationConfig, http::ProbeServer, journal::ProbeJournal,
    protocol::ProbeScope,
};
use std::{path::Path, process::Command};
#[tokio::test]
async fn wechat_gifts_to_textarea_wait_for_manual_send_and_pause_without_submission() {
    exercise("textarea", None).await;
}
#[tokio::test]
async fn wechat_gifts_to_contenteditable_wait_for_manual_send_and_pause_without_submission() {
    exercise("contenteditable", None).await;
}
#[tokio::test]
async fn wechat_textarea_submits_once_with_duplicate_command_and_lost_receipt() {
    exercise("textarea", Some("auto")).await;
}
#[tokio::test]
async fn wechat_contenteditable_submits_once_with_duplicate_command_and_lost_receipt() {
    exercise("contenteditable", Some("auto")).await;
}
#[tokio::test]
async fn wechat_unknown_submission_is_logged_without_halting_later_commands() {
    exercise("textarea", Some("unknown")).await;
}
#[tokio::test]
async fn wechat_disabled_comment_button_never_clicks_and_later_draft_can_submit() {
    exercise("textarea", Some("disabled")).await;
}
#[tokio::test]
async fn wechat_ambiguous_comment_controls_never_click() {
    exercise("contenteditable", Some("ambiguous")).await;
}
#[tokio::test]
async fn wechat_user_edit_after_fill_is_preserved_without_submission() {
    exercise("textarea", Some("changed")).await;
}
#[tokio::test]
async fn wechat_lost_command_response_is_not_retried_and_next_gift_continues() {
    exercise("textarea", Some("handoff")).await;
}
#[tokio::test]
async fn wechat_native_comment_button_inside_identical_text_wrappers_submits_and_continues() {
    exercise("textarea", Some("nested")).await;
}
#[tokio::test]
async fn wechat_custom_comment_label_bubbles_to_handler_without_clicking_wrapper() {
    exercise("contenteditable", Some("nested_custom")).await;
}
#[tokio::test]
async fn wechat_collapsed_editor_opens_for_each_pending_thanks_without_clicking_heading() {
    exercise("textarea", Some("collapsed")).await;
}
#[tokio::test]
async fn wechat_fixed_test_opens_and_fills_without_enabling_submission() {
    exercise("textarea", Some("open_only")).await;
}
#[tokio::test]
async fn wechat_yaml_custom_thanks_and_complete_zwj_emoji_reach_submit_and_retry() {
    exercise("textarea", Some("emoji")).await;
}
#[tokio::test]
async fn wechat_overflow_thanks_keep_one_complete_emoji() {
    exercise("contenteditable", Some("emoji_overflow")).await;
}
#[tokio::test]
async fn wechat_default_random_suffix_is_chosen_once_per_draft() {
    exercise("textarea", Some("emoji_defaults")).await;
}
#[tokio::test]
async fn wechat_148_character_user_ids_reach_thanks_and_submit_as_distinct_users() {
    exercise("textarea", Some("long_users")).await;
}

#[tokio::test]
async fn wechat_value_templates_follow_native_price_and_keep_one_choice_through_retry() {
    exercise("textarea", Some("variation")).await;
}

#[tokio::test]
async fn wechat_nickname_table_preserves_full_alias_and_original_identity_through_retry() {
    exercise("textarea", Some("aliases")).await;
}
#[tokio::test]
async fn wechat_nickname_table_matches_complete_148_character_user_ids() {
    exercise("contenteditable", Some("aliases_long")).await;
}
#[tokio::test]
async fn wechat_timed_messages_randomly_submit_from_collapsed_editor_without_gifts() {
    exercise("textarea", Some("timed")).await;
}
async fn exercise(editor: &str, scenario: Option<&str>) {
    let temp = tempfile::tempdir().unwrap();
    let db = temp.path().join("probe.sqlite3");
    let yaml = temp.path().join("config.yml");
    std::fs::write(&yaml,"version: 1\nmode: candidate_fill_only\nroom:\n  platform: wechat_channels\n  adapter: wechat_channels.desktop_probe\n  url: https://channels.weixin.qq.com/web/pages/live\ncandidate_preview:\n  enabled: true\n  recipient_user_id: fixture-recipient\n").unwrap();
    if scenario == Some("open_only") {
        let source =
            std::fs::read_to_string(&yaml).unwrap() + "fill_test_text: 仅展开填入测试，不发送\n";
        std::fs::write(&yaml, source).unwrap();
    } else if scenario.is_some() {
        let source = std::fs::read_to_string(&yaml).unwrap() + "sending:\n  automatic: true\n";
        std::fs::write(&yaml, source).unwrap();
    }
    let mut source = std::fs::read_to_string(&yaml).unwrap();
    if matches!(
        scenario,
        Some("emoji" | "emoji_overflow" | "emoji_defaults")
    ) {
        source.push_str("rules:\n  gift:\n    template: '谢谢{nickname}的{gift_list}'\n    overflow_template: '感谢礼物飘屏'\n");
        if scenario != Some("emoji_defaults") {
            source.push_str("    emojis: ['👩‍💻']\n");
        }
    } else {
        source.push_str("rules:\n  gift:\n    emojis: []\n");
    }
    if scenario.is_some_and(|s| s.starts_with("delivery_") && s != "delivery_local_marker") {
        source.push_str("delivery:\n  actor_user_id: fixture-own\n");
    }
    if scenario == Some("variation") {
        source=source.replace("rules:\n  gift:\n    emojis: []\n", "rules:\n  gift:\n    emojis: []\n    variation:\n      enabled: true\n      tiers:\n        - min_unit_value: 0\n          templates: ['普通{nickname}的{gift_list}']\n        - min_unit_value: 1000\n          templates: ['高档一{nickname}的{gift_list}', '高档二{nickname}的{gift_list}']\n");
    }
    if matches!(scenario, Some("aliases" | "aliases_long")) {
        let id = if scenario == Some("aliases_long") {
            format!("{}a", "u".repeat(147))
        } else {
            "first-user".into()
        };
        let alias = "专属昵称".repeat(8);
        std::fs::write(temp.path().join("names.yml"),format!("version: 1\nplatforms:\n  douyin:\n    '{id}': 不应使用的跨平台称呼\n  wechat_channels:\n    '{id}': '{alias}'\n")).unwrap();
        source.push_str("nickname_mapping:\n  file: names.yml\n");
    }
    if scenario == Some("timed") {
        source.push_str("timed_messages:\n  enabled: true\n  interval_seconds: 1\n  messages: ['定时文案甲', '定时文案乙', '定时文案丙']\n");
    }
    std::fs::write(&yaml, source).unwrap();
    let config = ModerationConfig::load(&yaml).unwrap();
    let options = config.probe_options();
    assert!(!options.rules_preview && !options.scheduler_preview && !options.fill_preview);
    assert!(options.candidate_fill_only);
    let mut bridge = ProbeBridge::new(
        ProbeScope::from_room_url(config.room.url.clone()).unwrap(),
        ProbeJournal::open(&db).unwrap(),
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
    bridge
        .configure_candidate_preview(config.candidate_preview)
        .unwrap();
    bridge
        .enable_candidate_fill_preview(options.fill_test_text)
        .unwrap();
    if options.automatic_send {
        bridge.enable_automatic_send().unwrap();
    }
    let bootstrap = temp.path().join("bootstrap.js");
    std::fs::write(
        &bootstrap,
        bridge.bootstrap(&bridge.relay_token.clone()).unwrap(),
    )
    .unwrap();
    let mut server = ProbeServer::bind(bridge).await.unwrap();
    let address = server.address.to_string();
    let editor = editor.to_owned();
    let scenario = scenario.map(str::to_owned);
    let scenario_driver = scenario.clone();
    let result = tokio::task::spawn_blocking(move || {
        Command::new(std::env::var_os("MODERATION_NODE").unwrap_or_else(|| "node".into()))
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR")).join(if scenario_driver.is_some() {
                    "tests/support/moderation_wechat_send_driver.cjs"
                } else {
                    "tests/support/moderation_wechat_fill_driver.cjs"
                }),
            )
            .arg(bootstrap)
            .arg(address)
            .arg(editor)
            .arg(scenario_driver.unwrap_or_default())
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
    let summary: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    if scenario.as_deref() == Some("timed") {
        assert_eq!(summary["submissions"], 2);
        assert_eq!(summary["writes"], 2);
        let c = rusqlite::Connection::open(&db).unwrap();
        let issued:i64=c.query_row("select count(*) from fill_audits where json_extract(audit_json,'$.state')='issued' and json_extract(audit_json,'$.action_id') like 'timed-%'",[],|r|r.get(0)).unwrap();
        assert_eq!(issued, 2);
        let gifts:i64=c.query_row("select count(*) from candidate_draft_audits where json_extract(audit_json,'$.kind')='gift_candidate_draft'",[],|r|r.get(0)).unwrap();
        assert_eq!(gifts, 0);
        return;
    }
    assert_eq!(
        summary["writes"],
        if scenario.as_deref() == Some("open_only") {
            1
        } else if scenario.as_deref() == Some("handoff") {
            1
        } else if matches!(scenario.as_deref(), Some("unknown" | "delivery_blocked")) {
            2
        } else {
            2
        }
    );
    assert_eq!(
        summary["send_chat"],
        scenario.is_some() && scenario.as_deref() != Some("open_only")
    );
    if matches!(
        scenario.as_deref(),
        Some("emoji" | "emoji_overflow" | "emoji_defaults")
    ) {
        let sent = summary["submitted_texts"].as_array().unwrap();
        assert_eq!(sent.len(), 2);
        for (i, t) in sent.iter().enumerate() {
            let text = t.as_str().unwrap();
            if scenario.as_deref() == Some("emoji_defaults") {
                let base = if i == 0 {
                    "谢谢集成测试观众的小星星"
                } else {
                    "谢谢集成测试观众的玫瑰"
                };
                assert!(text.starts_with(base));
                assert!(
                    live_archive_core::moderation::gift_text::default_emojis()
                        .contains(&text[base.len()..].to_string())
                );
            } else {
                assert_eq!(
                    text,
                    if i == 0 {
                        if scenario.as_deref() == Some("emoji_overflow") {
                            "感谢礼物飘屏👩‍💻"
                        } else {
                            "谢谢集成测试观众的小星星👩‍💻"
                        }
                    } else {
                        "谢谢集成测试观众的玫瑰👩‍💻"
                    }
                );
            }
        }
    }
    let c = rusqlite::Connection::open(db).unwrap();
    let filled: i64 = c
        .query_row(
            "select count(*) from fill_audits where json_extract(audit_json,'$.state')='filled'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        filled,
        if scenario.as_deref() == Some("open_only") {
            1
        } else if scenario.is_some() {
            0
        } else {
            2
        }
    );
    if scenario.is_some() && scenario.as_deref() != Some("open_only") {
        let submitted:i64=c.query_row("select count(*) from fill_audits where json_extract(audit_json,'$.state')='submitted'",[],|r|r.get(0)).unwrap();
        assert_eq!(
            submitted,
            if matches!(scenario.as_deref(), Some("unknown" | "delivery_blocked")) {
                0
            } else if matches!(
                scenario.as_deref(),
                Some(
                    "handoff"
                        | "disabled"
                        | "ambiguous"
                        | "changed"
                        | "delivery_recover"
                        | "delivery_crossing_poll"
                        | "delivery_crossing_zero_ack"
                )
            ) {
                1
            } else {
                2
            }
        );
        let unknown:i64=c.query_row("select count(*) from fill_audits where json_extract(audit_json,'$.state')='unknown'",[],|r|r.get(0)).unwrap();
        assert_eq!(
            unknown,
            if scenario.as_deref() == Some("unknown") {
                2
            } else if matches!(
                scenario.as_deref(),
                Some(
                    "unknown"
                        | "handoff"
                        | "delivery_blocked"
                        | "delivery_recover"
                        | "delivery_crossing_poll"
                        | "delivery_crossing_zero_ack"
                )
            ) {
                1
            } else {
                0
            }
        );
    }
    if scenario
        .as_deref()
        .is_some_and(|s| s.starts_with("delivery_"))
    {
        let rows: Vec<String> = c
            .prepare("select audit_json from delivery_audits")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            rows.len(),
            if scenario.as_deref() == Some("delivery_blocked") {
                1
            } else {
                2
            },
            "receipt retries must be idempotent"
        );
        for row in rows {
            let row: serde_json::Value = serde_json::from_str(&row).unwrap();
            assert_eq!(
                row["state"],
                if !matches!(
                    scenario.as_deref(),
                    Some("delivery_timeout" | "delivery_blocked")
                ) {
                    "echo_observed"
                } else {
                    "unconfirmed"
                }
            );
        }
    }
    if scenario.as_deref() == Some("variation") {
        let texts = summary["submitted_texts"].as_array().unwrap();
        assert!(
            texts
                .iter()
                .all(|t| t.as_str().unwrap().starts_with("高档"))
        );
        assert_ne!(
            texts[0]
                .as_str()
                .unwrap()
                .chars()
                .take(3)
                .collect::<String>(),
            texts[1]
                .as_str()
                .unwrap()
                .chars()
                .take(3)
                .collect::<String>()
        );
        let prices:i64=c.query_row("select count(*) from probe_events where json_extract(event_json,'$.event_type')='gift.candidate' and json_extract(event_json,'$.payload.unit_value')=1000",[],|r|r.get(0)).unwrap();
        assert!(prices >= 2);
    }
    if matches!(
        scenario.as_deref(),
        Some("delivery_recover" | "delivery_crossing_poll" | "delivery_crossing_zero_ack")
    ) {
        let confirmed:i64=c.query_row("select count(*) from fill_audits where json_extract(audit_json,'$.state')='echo_confirmed' and json_extract(audit_json,'$.reason')='network_echo_confirmed_no_resend'",[],|r|r.get(0)).unwrap();
        assert_eq!(
            confirmed, 1,
            "lost delivery ACK must not duplicate recovery audit"
        );
    }
    if matches!(scenario.as_deref(), Some("aliases" | "aliases_long")) {
        let texts = summary["submitted_texts"].as_array().unwrap();
        assert_eq!(
            texts[0],
            format!("感谢{}送来的小星星", "专属昵称".repeat(8))
        );
        assert_eq!(texts[1], "感谢集成测试观众送来的玫瑰");
        let count:i64=c.query_row("select count(*) from probe_events where json_extract(event_json,'$.event_type')='gift.candidate' and json_extract(event_json,'$.nickname')='集成测试观众'",[],|r|r.get(0)).unwrap();
        assert!(count >= 2, "raw platform nicknames must remain unchanged");
    }
    let ordinary: i64 = c
        .query_row("select count(*) from preview_audits", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ordinary, 0);
    let texts:Vec<String>=c.prepare("select json_extract(audit_json,'$.text') from candidate_draft_audits where json_extract(audit_json,'$.kind')='gift_candidate_draft'").unwrap().query_map([],|r|r.get(0)).unwrap().map(|r|r.unwrap()).collect();
    assert_eq!(
        texts.len(),
        if scenario.as_deref() == Some("open_only") {
            0
        } else {
            2
        }
    );
    assert!(texts.iter().all(|t| !t.contains('×')));
    if matches!(
        scenario.as_deref(),
        Some("emoji" | "emoji_overflow" | "emoji_defaults")
    ) {
        let sent = summary["submitted_texts"].as_array().unwrap();
        for (i, text) in texts.iter().enumerate() {
            assert_eq!(sent[i].as_str(), Some(text.as_str()));
        }
        let queued:Vec<String>=c.prepare("select json_extract(audit_json,'$.command.text') from fill_audits where json_extract(audit_json,'$.state')='queued' order by id").unwrap().query_map([],|r|r.get(0)).unwrap().map(|r|r.unwrap()).collect();
        assert_eq!(queued, texts);
    }
}
