//! YAML -> rules -> scheduler, plus CLI preflight without proxy/platform access.
use live_archive_core::moderation::{
    config::{Mode, ModerationConfig},
    engine::Engine,
    events::{CoreEvent, EventKind, Evidence, Gift, Level, QuantityMode, Scope, User},
    scheduler::PreviewScheduler,
};
use std::{fs, process::Command};

const BASE: &str = "version: 1\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/139244611014\n";

fn event(id: &str, user: &str, kind: EventKind) -> CoreEvent {
    CoreEvent {
        event_id: id.into(),
        native_event_id: Some(id.into()),
        source: "integration".into(),
        evidence: Evidence::Verified,
        scope: Scope {
            platform: "douyin".into(),
            room_id: "139244611014".into(),
            live_session_id: "session".into(),
            actor_user_id: "assistant".into(),
            recipient_user_id: "anchor".into(),
        },
        user: Some(User {
            id: Some(user.into()),
            nickname: user.into(),
        }),
        kind,
    }
}
fn gift(id: &str, user: &str, name: &str) -> CoreEvent {
    event(
        id,
        user,
        EventKind::Gift(Gift {
            unit_value: None,
            gift_id: name.into(),
            name: name.into(),
            recipient_user_id: "anchor".into(),
            quantity_mode: QuantityMode::Incremental,
            quantity: Some(1),
            combo_id: None,
            combo_end: false,
        }),
    )
}

#[test]
fn minimal_yaml_and_cli_check_are_observation_only_and_do_not_create_storage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moderation.yml");
    fs::write(&path, BASE).unwrap();
    let config = ModerationConfig::load(&path).unwrap();
    assert_eq!(config.mode, Mode::Observe);
    assert!(!config.probe_options().rules_preview);
    assert!(!config.probe_options().system_proxy);
    assert_eq!(
        config.storage.directory,
        dir.path()
            .canonicalize()
            .unwrap()
            .join("data/moderation-probe")
    );
    let result = Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
        .args(["moderation", "check", "--config"])
        .arg(&path)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("send_chat=false"));
    assert!(!dir.path().join("data").exists());
}

#[test]
fn invalid_yaml_run_fails_before_creating_storage_or_launching_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moderation.yml");
    for suffix in [
        "mode: execute\n",
        "injection:\n  system_proxy: true\n",
        "unknown: true\n",
        "mode: observe\nmode: fill_only\n",
        "rules:\n  gift:\n    emojis: []\n    template: '感谢{unknown}'\n",
        "rules:\n  welcome:\n    enable: false\n",
        "scheduler:\n  min_interval_ms: 7999\n",
        "scheduler:\n  account_per_minute: 7\n",
        "mode: fill_only\nlimits:\n  max_text_chars: 81\n",
        "fill_test_text: '测试'\n",
        "duration_seconds: 3601\n",
        "version: 2\n",
    ] {
        fs::write(&path, format!("{BASE}{suffix}")).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
            .args(["moderation", "run", "--config"])
            .arg(&path)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(!result.status.success(), "accepted {suffix}");
        assert!(!dir.path().join("data").exists());
    }
    fs::write(
        &path,
        BASE.replace("douyin.live_page", "wechat_channels.assistant"),
    )
    .unwrap();
    assert!(ModerationConfig::load(&path).is_err());
    fs::write(&path, format!("{BASE}#{}", "x".repeat(65536))).unwrap();
    assert!(ModerationConfig::load(&path).is_err());
}

#[test]
fn yaml_templates_switches_sliding_gifts_and_custom_budget_reach_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moderation.yml");
    fs::write(&path, format!("{BASE}mode: scheduler_preview\ninjection:\n  mitmdump: ./bin/mitmdump\nrules:\n  gift:\n    emojis: []\n    template: '{{nickname}}：{{gift_list}}，谢谢！'\n    overflow_template: '{{nickname}}：感谢礼物飘屏'\n  welcome:\n    enabled: false\n  fansclub_joined:\n    template: '入团欢迎{{nickname}}'\n  upgrade:\n    enabled: false\nscheduler:\n  min_interval_ms: 9000\n  account_per_minute: 1\n")).unwrap();
    let config = ModerationConfig::load(&path).unwrap();
    assert!(config.injection.mitmdump.is_absolute());
    let mut engine = Engine::new(config.rules_config()).unwrap();
    let mut scheduler = PreviewScheduler::new(config.scheduler.clone()).unwrap();
    assert!(
        engine
            .ingest(0, event("welcome", "甲", EventKind::Welcome))
            .unwrap()
            .iter()
            .any(|a| a.reason == "rule_disabled")
    );
    engine.ingest(0, gift("a", "甲", "玫瑰")).unwrap();
    engine.ingest(4000, gift("b", "甲", "玫瑰")).unwrap();
    assert!(
        scheduler
            .advance(5000, &mut engine)
            .unwrap()
            .iter()
            .all(|a| a.action.is_none())
    );
    let first = scheduler.advance(9000, &mut engine).unwrap();
    assert!(
        first.iter().any(|a| a.kind == "dispatch_preview"
            && a.action.as_ref().unwrap().text == "甲：玫瑰×2，谢谢！")
    );
    engine
        .ingest(9001, event("join", "乙", EventKind::FansclubJoined))
        .unwrap();
    assert!(scheduler.advance(9001, &mut engine).unwrap().is_empty());
    assert_eq!(scheduler.status(&engine)["account_per_minute"], 1);
    assert_eq!(scheduler.status(&engine)["min_interval_ms"], 9000);
    // Budget delays beyond the welcome TTL: expiry rather than a stale draft.
    assert!(
        scheduler
            .advance(19001, &mut engine)
            .unwrap()
            .iter()
            .any(|a| a.kind == "expired")
    );
    engine
        .ingest(65000, gift("c", "丙", &"礼".repeat(31)))
        .unwrap();
    let second = scheduler.advance(70000, &mut engine).unwrap();
    assert!(
        second.iter().any(|a| a.kind == "dispatch_preview"
            && a.action.as_ref().unwrap().text == "丙：感谢礼物飘屏")
    );
}

#[test]
fn checked_in_example_is_valid_and_all_modes_map_to_safe_options() {
    let config = ModerationConfig::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("moderation.example.yml"),
    )
    .unwrap();
    assert_eq!(config.mode, Mode::SchedulerPreview);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moderation.yml");
    for (mode, rules, scheduler, fill) in [
        ("observe", false, false, false),
        ("rules_preview", true, false, false),
        ("scheduler_preview", true, true, false),
        ("fill_only", true, true, true),
    ] {
        fs::write(&path, format!("{BASE}mode: {mode}\n")).unwrap();
        let options = ModerationConfig::load(&path).unwrap().probe_options();
        assert_eq!(
            (
                options.rules_preview,
                options.scheduler_preview,
                options.fill_preview
            ),
            (rules, scheduler, fill)
        );
    }
}

#[test]
fn yaml_rule_switches_isolate_gifts_joined_and_each_upgrade_kind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("moderation.yml");
    fs::write(&path, format!("{BASE}rules:\n  gift:\n    emojis: []\n    enabled: false\n  fansclub_joined:\n    enabled: false\n  welcome:\n    template: '你好{{nickname}}'\n  upgrade:\n    fansclub: false\n    honor: true\n    wealth: false\n    template: '{{nickname}}/{{level_name}}/{{new_level}}'\n")).unwrap();
    let config = ModerationConfig::load(&path).unwrap();
    let mut engine = Engine::new(config.rules_config()).unwrap();
    let mut scheduler = PreviewScheduler::new(config.scheduler).unwrap();
    for e in [
        gift("gift", "甲", "玫瑰"),
        event("join", "乙", EventKind::FansclubJoined),
    ] {
        assert!(
            engine
                .ingest(0, e)
                .unwrap()
                .iter()
                .any(|a| a.reason == "rule_disabled")
        );
    }
    assert!(
        engine
            .ingest(0, event("welcome", "丙", EventKind::Welcome))
            .unwrap()
            .iter()
            .any(|a| a.reason == "welcome_batched")
    );
    for kind in ["fansclub", "honor", "wealth"] {
        let records = engine
            .ingest(
                0,
                event(
                    kind,
                    kind,
                    EventKind::LevelUpgraded(Level {
                        kind: kind.into(),
                        name: "等级".into(),
                        new_level: 3,
                    }),
                ),
            )
            .unwrap();
        if kind == "honor" {
            assert!(
                records
                    .iter()
                    .any(|a| a.kind == "planned"
                        && a.action.as_ref().unwrap().text == "honor/等级/3")
            );
        } else {
            assert!(records.iter().any(|a| a.reason == "rule_disabled"));
        }
    }
    assert!(
        scheduler
            .advance(0, &mut engine)
            .unwrap()
            .iter()
            .any(|a| a.kind == "dispatch_preview")
    );
    assert!(
        scheduler
            .advance(5000, &mut engine)
            .unwrap()
            .iter()
            .all(|a| a
                .action
                .as_ref()
                .is_none_or(|action| action.gift_batch.is_none()))
    );
    assert!(
        engine
            .advance(30_000)
            .unwrap()
            .iter()
            .any(|a| a.kind == "planned" && a.action.as_ref().is_some_and(|a| a.text == "你好丙"))
    );
}

#[test]
fn release_configs_enable_batched_welcomes_and_bind_wechat_page_target() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let douyin = ModerationConfig::load(&root.join("release/douyin.yml")).unwrap();
    assert_eq!(douyin.mode, Mode::FillOnly);
    assert!(douyin.rules.gift.enabled);
    assert!(!douyin.rules.gift.show_quantity);
    assert!(douyin.rules.welcome.enabled);
    assert!(!douyin.rules.fansclub_joined.enabled);
    assert!(!douyin.rules.upgrade.enabled);
    assert_eq!(
        douyin.rules.gift.overflow_template.as_deref(),
        Some("感谢礼物飘屏")
    );
    let wechat_template = root.join("release/wechat.yml");
    assert!(
        ModerationConfig::load(&wechat_template)
            .unwrap()
            .candidate_preview
            .needs_page_identity()
    );
    let dir = tempfile::tempdir().unwrap();
    let selected = dir.path().join("wechat.yml");
    fs::copy(
        root.join("release/nickname-mapping.yml"),
        dir.path().join("nickname-mapping.yml"),
    )
    .unwrap();
    fs::write(
        &selected,
        fs::read_to_string(wechat_template).unwrap().replace(
            "recipient_user_id: ''",
            "recipient_user_id: 'integration-target'",
        ),
    )
    .unwrap();
    let wechat = ModerationConfig::load(&selected).unwrap();
    assert_eq!(wechat.mode, Mode::CandidateFillOnly);
    let options = wechat.probe_options();
    assert!(options.candidate_fill_only);
    assert!(!options.rules_preview);
    assert!(!options.scheduler_preview);
    assert!(!wechat.rules.gift.show_quantity);
    assert!(wechat.rules.welcome.enabled);
    assert!(!wechat.rules.fansclub_joined.enabled);
    assert!(!wechat.rules.upgrade.enabled);
    let observe = ModerationConfig::load(&root.join("release/wechat-observe.yml")).unwrap();
    assert_eq!(observe.mode, Mode::Observe);
    assert!(!observe.probe_options().candidate_fill_only);
}

#[test]
fn automatic_send_is_opt_in_and_cannot_be_enabled_in_observe_or_fixed_test_runs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yml");
    for mode in ["observe", "rules_preview", "scheduler_preview"] {
        fs::write(
            &path,
            format!("{BASE}mode: {mode}\nsending:\n  automatic: true\n"),
        )
        .unwrap();
        assert!(ModerationConfig::load(&path).is_err());
    }
    fs::write(&path,format!("{BASE}mode: fill_only\nsending:\n  automatic: true\nfill_test_text: '固定测试不得发送'\n")).unwrap();
    assert!(ModerationConfig::load(&path).is_err());
    fs::write(&path, format!("{BASE}mode: fill_only\n")).unwrap();
    assert!(
        !ModerationConfig::load(&path)
            .unwrap()
            .probe_options()
            .automatic_send
    );
    fs::write(
        &path,
        format!("{BASE}mode: fill_only\nsending:\n  automatic: true\n"),
    )
    .unwrap();
    assert!(
        ModerationConfig::load(&path)
            .unwrap()
            .probe_options()
            .automatic_send
    );
}

#[test]
fn yaml_gift_format_default_pool_disable_and_length_budget_reach_engine() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yml");
    fs::write(&path,format!("{BASE}rules:\n  gift:\n    show_quantity: false\n    template: '谢谢{{nickname}}的{{gift_list}}'\n")).unwrap();
    let config = ModerationConfig::load(&path).unwrap();
    assert_eq!(
        config.rules.gift.emojis,
        live_archive_core::moderation::gift_text::default_emojis()
    );
    let mut engine = Engine::new(config.rules_config()).unwrap();
    engine.ingest(0, gift("gift-1", "观众", "玫瑰")).unwrap();
    let logs = engine.advance(5000).unwrap();
    let text = &logs.iter().find_map(|a| a.action.as_ref()).unwrap().text;
    let base = "谢谢观众的玫瑰";
    assert!(text.starts_with(base));
    assert!(
        config
            .rules
            .gift
            .emojis
            .contains(&text[base.len()..].to_string())
    );
    for (emoji, max, expected) in [
        ("[]", 80, "谢谢很长的观众昵称的玫瑰"),
        ("['👩‍💻']", 10, "感谢礼物飘屏👩‍💻"),
    ] {
        fs::write(&path,format!("{BASE}rules:\n  gift:\n    show_quantity: false\n    template: '谢谢{{nickname}}的{{gift_list}}'\n    overflow_template: '感谢礼物飘屏'\n    emojis: {emoji}\nlimits:\n  max_text_chars: {max}\n")).unwrap();
        let config = ModerationConfig::load(&path).unwrap();
        let mut engine = Engine::new(config.rules_config()).unwrap();
        engine
            .ingest(0, gift("gift-2", "很长的观众昵称", "玫瑰"))
            .unwrap();
        let logs = engine.advance(5000).unwrap();
        let text = &logs.iter().find_map(|a| a.action.as_ref()).unwrap().text;
        assert_eq!(text, expected);
        assert!(text.chars().count() <= max);
    }
}

#[test]
fn invalid_emoji_pools_fail_cli_preflight_before_storage_or_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yml");
    for pool in [
        "['']".to_string(),
        "[\"\\n\"]".to_string(),
        format!("['{}']", "🎁".repeat(17)),
        format!("[{}]", vec!["'🎁'"; 33].join(",")),
    ] {
        fs::write(
            &path,
            format!("{BASE}rules:\n  gift:\n    emojis: {pool}\n"),
        )
        .unwrap();
        assert!(ModerationConfig::load(&path).is_err());
        let result = Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
            .args(["moderation", "run", "--config"])
            .arg(&path)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!dir.path().join("data").exists());
    }
}

#[test]
fn delivery_actor_yaml_preflight_preserves_full_ids_and_rejects_invalid_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yml");
    for actor in [
        "'9007199254740993999'".to_string(),
        format!("'{}'", "u".repeat(148)),
    ] {
        fs::write(
            &path,
            format!("{BASE}delivery:\n  actor_user_id: {actor}\n"),
        )
        .unwrap();
        assert!(ModerationConfig::load(&path).is_ok());
    }
    for actor in [
        "''".to_string(),
        format!("'{}'", "u".repeat(257)),
        "{unexpected: mapping}".to_string(),
    ] {
        fs::write(
            &path,
            format!("{BASE}delivery:\n  actor_user_id: {actor}\n"),
        )
        .unwrap();
        assert!(ModerationConfig::load(&path).is_err());
        let output = Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
            .args(["moderation", "check", "--config"])
            .arg(&path)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!dir.path().join("data").exists());
    }
}

#[test]
fn value_wording_yaml_to_engine_and_sqlite_uses_highest_unit_price_and_nonrepeating_pools() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yml");
    fs::write(&path,format!("{BASE}rules:\n  gift:\n    emojis: []\n    show_quantity: false\n    variation:\n      enabled: true\n      unknown_templates: ['未知一{{nickname}}的{{gift_list}}', '未知二{{nickname}}的{{gift_list}}']\n      tiers:\n        - min_unit_value: 0\n          templates: ['普通一{{nickname}}的{{gift_list}}', '普通二{{nickname}}的{{gift_list}}']\n        - min_unit_value: 1000\n          templates: ['重点一{{nickname}}的{{gift_list}}', '重点二{{nickname}}的{{gift_list}}']\n")).unwrap();
    let config = ModerationConfig::load(&path).unwrap();
    let mut engine = Engine::new(config.rules_config()).unwrap();
    let db = dir.path().join("journal.sqlite3");
    let mut journal = live_archive_core::moderation::journal::ProbeJournal::open(&db).unwrap();
    let mut previous = None;
    for (i, price) in [
        Some(999),
        Some(999),
        Some(1000),
        Some(1000),
        None,
        None,
        Some(u64::MAX),
    ]
    .into_iter()
    .enumerate()
    {
        let at = i as u64 * 6000;
        let mut e = gift(&format!("gift-{i}"), "观众", "玫瑰");
        if let EventKind::Gift(ref mut g) = e.kind {
            g.unit_value = price;
            g.quantity = Some(9999);
        }
        engine.ingest(at, e).unwrap();
        let logs = engine.advance(at + 5000).unwrap();
        let action = logs.iter().find_map(|a| a.action.as_ref()).unwrap();
        let prefix = if i < 2 {
            "普通"
        } else if i < 4 {
            "重点"
        } else {
            "未知"
        };
        assert!(
            action.text.starts_with(prefix),
            "window {i}, value {price:?}: {}",
            action.text
        );
        assert!(!action.text.contains("9999"));
        if let Some(previous) = previous {
            assert_ne!(action.text, previous);
        }
        previous = Some(action.text.clone());
        journal.append_audits("run", "connection", &logs).unwrap();
        let completed = engine
            .simulate_dispatch(at + 5000, &action.action_id)
            .unwrap();
        journal
            .append_audits("run", "connection", &completed)
            .unwrap();
    }
    let c = rusqlite::Connection::open(db).unwrap();
    let stored: i64 = c
        .query_row(
            "select count(*) from preview_audits where json_extract(audit_json,'$.kind')='planned'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, 7);
    // Two low-price gifts, one high-price gift: choose by maximum unit price,
    // never multiply a gift's price by its quantity to determine the tier.
    let mut low = gift("mixed-low", "另一个观众", "星");
    if let EventKind::Gift(ref mut g) = low.kind {
        g.unit_value = Some(1);
        g.quantity = Some(99999);
    }
    let mut high = gift("mixed-high", "另一个观众", "花");
    if let EventKind::Gift(ref mut g) = high.kind {
        g.unit_value = Some(1000);
    }
    engine.ingest(50000, low).unwrap();
    engine.ingest(51000, high).unwrap();
    let logs = engine.advance(56000).unwrap();
    assert!(
        logs.iter()
            .find_map(|a| a.action.as_ref())
            .unwrap()
            .text
            .starts_with("重点")
    );
}

#[test]
fn invalid_value_template_pools_fail_preflight_and_legacy_wording_remains_available() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yml");
    for extra in [
        "      tiers: []\n",
        "      tiers:\n        - min_unit_value: 100\n          templates: ['感谢{nickname}']\n",
        "      unknown_templates: []\n",
        "      unknown_templates: ['{price}']\n",
        "      unknown_templates: ['重复', '重复']\n",
        "      tiers:\n        - min_unit_value: 0\n          templates: []\n",
    ] {
        fs::write(
            &path,
            format!("{BASE}rules:\n  gift:\n    variation:\n      enabled: true\n{extra}"),
        )
        .unwrap();
        assert!(ModerationConfig::load(&path).is_err());
    }
}

#[test]
fn shipped_wording_fragment_has_38_valid_editable_presets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yml");
    let fragment = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/release/gift-wording.example.yml"
    ))
    .unwrap();
    fs::write(&path, format!("{BASE}{fragment}")).unwrap();
    let config = ModerationConfig::load(&path).unwrap();
    let variation = &config.rules.gift.variation;
    assert!(variation.enabled);
    assert_eq!(variation.tiers.as_ref().unwrap().len(), 4);
    assert_eq!(
        variation
            .tiers
            .as_ref()
            .unwrap()
            .iter()
            .map(|t| t.templates.len())
            .sum::<usize>()
            + variation.unknown_templates.as_ref().unwrap().len(),
        38
    );
    Engine::new(config.rules_config()).unwrap();
}

#[test]
fn shared_nickname_table_is_platform_scoped_and_room_overrides_reach_gift_drafts_only() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("config");
    fs::create_dir(&config_dir).unwrap();
    fs::write(config_dir.join("nicknames.yml"),"version: 1\nplatforms:\n  douyin:\n    known: '共享称呼'\n    other: '另一个称呼'\n  wechat_channels:\n    known: '视频号称呼'\n").unwrap();
    let path = config_dir.join("room.yml");
    let base = format!(
        "{BASE}nickname_mapping:\n  file: nicknames.yml\n  users:\n    known: '房间称呼'\nrules:\n  gift:\n    emojis: []\n    show_quantity: false\n"
    );
    fs::write(&path, &base).unwrap();
    let config = ModerationConfig::load(&path).unwrap();
    assert_eq!(config.nickname_mapping.users["known"], "房间称呼");
    assert_eq!(config.nickname_mapping.users["other"], "另一个称呼");
    let mut engine = Engine::new(config.rules_config()).unwrap();
    let mut e = gift("alias-gift", "known", "玫瑰");
    e.user.as_mut().unwrap().nickname = "平台昵称".into();
    assert_eq!(e.user.as_ref().unwrap().nickname, "平台昵称");
    engine.ingest(0, e).unwrap();
    let logs = engine.advance(5000).unwrap();
    let action = logs.iter().find_map(|a| a.action.as_ref()).unwrap();
    assert_eq!(action.text, "感谢房间称呼送来的玫瑰");
    assert_eq!(action.user_id, "known");
    engine.simulate_dispatch(5000, &action.action_id).unwrap();
    let mut e = event("welcome", "known", EventKind::Welcome);
    e.user.as_mut().unwrap().nickname = "平台昵称".into();
    let mut logs = engine.ingest(6000, e).unwrap();
    logs.extend(engine.advance(36000).unwrap());
    assert!(
        logs.iter()
            .filter_map(|a| a.action.as_ref())
            .any(|a| a.text == "欢迎平台昵称来到直播间")
    );
    // The same stable ID in a different platform namespace has its own alias.
    let wx = base
        .replace("platform: douyin", "platform: wechat_channels")
        .replace(
            "adapter: douyin.live_page",
            "adapter: wechat_channels.desktop_probe",
        )
        .replace(
            "https://live.douyin.com/139244611014",
            "https://channels.weixin.qq.com/web/pages/live",
        )
        .replace("  users:\n    known: '房间称呼'\n", "");
    fs::write(&path, wx).unwrap();
    assert_eq!(
        ModerationConfig::load(&path)
            .unwrap()
            .nickname_mapping
            .users["known"],
        "视频号称呼"
    );
}

#[test]
fn invalid_nickname_tables_fail_preflight_before_storage_and_alias_braces_are_literal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("room.yml");
    for users in [
        "    '': 昵称\n".into(),
        "    ' user ': 昵称\n".into(),
        "    user: '  '\n".into(),
        format!("    user: '{}'\n", "名".repeat(33)),
        "    user: \"换\\n行\"\n".into(),
        format!("    '{}': 昵称\n", "u".repeat(257)),
    ] {
        fs::write(&path, format!("{BASE}nickname_mapping:\n  users:\n{users}")).unwrap();
        assert!(ModerationConfig::load(&path).is_err());
        let result = Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
            .args(["moderation", "check", "--config"])
            .arg(&path)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!dir.path().join("data").exists());
    }
    fs::write(
        &path,
        format!("{BASE}nickname_mapping:\n  file: absent.yml\n"),
    )
    .unwrap();
    assert!(ModerationConfig::load(&path).is_err());
    fs::write(dir.path().join("table.yml"), "version: 2\nplatforms: {}\n").unwrap();
    fs::write(
        &path,
        format!("{BASE}nickname_mapping:\n  file: table.yml\n"),
    )
    .unwrap();
    assert!(ModerationConfig::load(&path).is_err());
    fs::write(&path,format!("{BASE}nickname_mapping:\n  users:\n    known: '  星{{gift_list}}哥  '\nrules:\n  gift:\n    emojis: []\n    show_quantity: false\n")).unwrap();
    let mut engine = Engine::new(ModerationConfig::load(&path).unwrap().rules_config()).unwrap();
    engine.ingest(0, gift("gift", "known", "玫瑰")).unwrap();
    let logs = engine.advance(5000).unwrap();
    assert_eq!(
        logs.iter().find_map(|a| a.action.as_ref()).unwrap().text,
        "感谢星{gift_list}哥送来的玫瑰"
    );
}

#[test]
fn timed_message_yaml_requires_automatic_fill_and_valid_unique_bounded_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("room.yml");
    let base = format!("{BASE}mode: fill_only\nsending:\n  automatic: true\n");
    let valid = "timed_messages:\n  enabled: true\n  interval_seconds: 300\n  messages: ['欢迎来到直播间✨', '喜欢直播的朋友可以点点关注']\n";
    fs::write(&path, format!("{base}{valid}")).unwrap();
    let cfg = ModerationConfig::load(&path).unwrap();
    assert!(cfg.probe_options().timed_messages.enabled);
    assert_eq!(cfg.timed_messages.messages.len(), 2);
    for source in [
        format!("{BASE}{valid}"),
        format!(
            "{}{valid}",
            base.replace("automatic: true", "automatic: false")
        ),
        format!(
            "{base}{}",
            valid.replace("interval_seconds: 300", "interval_seconds: 0")
        ),
        format!(
            "{base}{}",
            valid.replace("interval_seconds: 300", "interval_seconds: 86401")
        ),
        format!("{base}timed_messages:\n  enabled: true\n  messages: []\n"),
        format!("{base}timed_messages:\n  enabled: true\n  messages: ['重复', '重复']\n"),
        format!("{base}timed_messages:\n  enabled: true\n  messages: ['  ']\n"),
        format!("{base}timed_messages:\n  enabled: true\n  messages: [\"换\\n行\"]\n"),
        format!(
            "{base}timed_messages:\n  enabled: true\n  messages: ['{}']\n",
            "文".repeat(81)
        ),
        format!("{base}fill_test_text: 不能与固定测试混用\n{valid}"),
        format!(
            "{base}limits:\n  max_text_chars: 10\ntimed_messages:\n  enabled: true\n  messages: ['{}']\n",
            "文".repeat(11)
        ),
    ] {
        fs::write(&path, source).unwrap();
        assert!(ModerationConfig::load(&path).is_err());
        let out = Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
            .args(["moderation", "check", "--config"])
            .arg(&path)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(!dir.path().join("data").exists());
    }
}
