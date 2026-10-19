use live_archive_core::moderation::{
    candidate_preview::CandidatePreview,
    engine::{ActionKind, Audit, Engine, RulesConfig},
    events::{CoreEvent, EventKind, Evidence, Scope, User},
    protocol::{ProbeEvent, sanitize},
    scheduler::PreviewScheduler,
};
use serde_json::json;

fn arrival(id: &str, user: &str, platform: &str) -> CoreEvent {
    CoreEvent {
        event_id: id.into(),
        native_event_id: Some(id.into()),
        source: "fixture".into(),
        evidence: Evidence::Verified,
        scope: Scope {
            platform: platform.into(),
            room_id: "room".into(),
            live_session_id: "live".into(),
            actor_user_id: "assistant".into(),
            recipient_user_id: "host".into(),
        },
        user: Some(User {
            id: Some(user.into()),
            nickname: user.into(),
        }),
        kind: EventKind::Welcome,
    }
}
fn welcomed(logs: &[Audit]) -> Vec<String> {
    logs.iter()
        .filter(|a| a.kind == "planned")
        .filter_map(|a| a.action.as_ref())
        .filter(|a| a.kind == ActionKind::Welcome)
        .map(|a| a.text.clone())
        .collect()
}
fn candidate(id: &str, user: &str) -> ProbeEvent {
    serde_json::from_value(
        json!({"seq":1,"event_id":id,"native_event_id":id,"user_id":user,"nickname":user,
        "event_type":"member.candidate","source_kind":"network",
        "payload":{"message_type":10005,"decoder":"wechat.msg_list.arrival.candidate.v1",
            "observed_at_ms":chrono::Utc::now().timestamp_millis(),"arrival_text_match":true}}),
    )
    .unwrap()
}

#[test]
fn thirty_second_window_is_fixed_and_duplicates_do_not_extend_it() {
    let mut engine = Engine::new(RulesConfig::default()).unwrap();
    assert!(welcomed(&engine.ingest(0, arrival("a", "甲", "douyin")).unwrap()).is_empty());
    engine
        .ingest(100, arrival("duplicate", "甲", "douyin"))
        .unwrap();
    engine.ingest(29_999, arrival("b", "乙", "douyin")).unwrap();
    assert!(welcomed(&engine.advance(29_999).unwrap()).is_empty());
    assert_eq!(
        welcomed(&engine.advance(30_000).unwrap()),
        ["欢迎甲、乙来到直播间"]
    );
    assert!(welcomed(&engine.advance(30_000).unwrap()).is_empty());
    engine.ingest(30_001, arrival("c", "丙", "douyin")).unwrap();
    assert!(welcomed(&engine.advance(60_000).unwrap()).is_empty());
    assert_eq!(
        welcomed(&engine.advance(60_001).unwrap()),
        ["欢迎丙来到直播间"]
    );
}

#[test]
fn deadline_arrivals_join_batch_and_scopes_remain_independent() {
    let mut engine = Engine::new(RulesConfig::default()).unwrap();
    engine.ingest(0, arrival("a", "甲", "douyin")).unwrap();
    engine
        .ingest(1000, arrival("b", "乙", "wechat_channels"))
        .unwrap();
    engine.ingest(30_000, arrival("c", "丙", "douyin")).unwrap();
    assert_eq!(
        welcomed(&engine.advance(30_000).unwrap()),
        ["欢迎甲、丙来到直播间"]
    );
    assert_eq!(
        welcomed(&engine.advance(31_000).unwrap()),
        ["欢迎乙来到直播间"]
    );
}

#[test]
fn busy_rooms_emit_one_short_message_and_expiry_starts_after_batching() {
    let mut engine = Engine::new(RulesConfig {
        max_text_chars: 40,
        ..Default::default()
    })
    .unwrap();
    for i in 0..100 {
        engine
            .ingest(i, arrival(&format!("e{i}"), &format!("观众{i}"), "douyin"))
            .unwrap();
    }
    let texts = welcomed(&engine.advance(30_000).unwrap());
    assert_eq!(texts.len(), 1);
    assert!(texts[0].chars().count() <= 40);
    assert!(texts[0].contains("等100位朋友"));
    assert_eq!(engine.pending_actions()[0].expires_at, 40_000);
    assert!(
        engine
            .advance(40_000)
            .unwrap()
            .iter()
            .any(|a| a.kind == "expired")
    );
}

#[test]
fn end_or_gap_clears_waiting_arrivals_and_idle_rooms_do_not_emit() {
    for kind in [EventKind::RoomEnded, EventKind::Gap] {
        let mut engine = Engine::new(RulesConfig::default()).unwrap();
        assert!(engine.advance(30_000).unwrap().is_empty());
        engine.ingest(30_001, arrival("a", "甲", "douyin")).unwrap();
        assert!(engine.has_pending_work());
        let mut end = arrival("end", "甲", "douyin");
        end.kind = kind;
        engine.ingest(30_002, end).unwrap();
        assert!(!engine.has_pending_work());
        assert!(welcomed(&engine.advance(90_000).unwrap()).is_empty());
    }
}

#[test]
fn scheduler_dispatches_one_combined_welcome_after_thirty_seconds() {
    let mut engine = Engine::new(RulesConfig::default()).unwrap();
    let mut scheduler = PreviewScheduler::default();
    engine.ingest(0, arrival("a", "甲", "douyin")).unwrap();
    engine.ingest(1000, arrival("b", "乙", "douyin")).unwrap();
    assert!(
        scheduler
            .advance(29_999, &mut engine)
            .unwrap()
            .iter()
            .all(|a| a.kind != "dispatch_preview")
    );
    let logs = scheduler.advance(30_000, &mut engine).unwrap();
    let dispatched: Vec<_> = logs
        .iter()
        .filter(|a| a.kind == "dispatch_preview")
        .collect();
    assert_eq!(dispatched.len(), 1);
    assert_eq!(
        dispatched[0].action.as_ref().unwrap().text,
        "欢迎甲、乙来到直播间"
    );
}

#[test]
fn wechat_candidates_batch_and_keep_unverified_room_flags() {
    let mut preview = CandidatePreview::new("host".into());
    preview.ingest(0, &sanitize(&candidate("a", "甲")));
    preview.ingest(1, &sanitize(&candidate("reentry", "甲")));
    preview.ingest(29_999, &sanitize(&candidate("b", "乙")));
    assert_eq!(preview.status(29_999)["pending_welcome_users"], 2);
    assert!(preview.advance(29_999).is_empty());
    let logs = preview.advance(30_000);
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0]["kind"], "welcome_candidate_draft");
    assert_eq!(logs[0]["text"], "欢迎甲、乙来到直播间");
    assert_eq!(logs[0]["live_eligible"], false);
    assert_eq!(logs[0]["room_verified"], false);
    assert_eq!(logs[0]["expires_at_ms"], 40_000);
    assert!(preview.advance(60_000).is_empty());
}

#[test]
fn wechat_rejects_wrong_sources_stale_or_disabled_entries_and_cancels_on_pause() {
    let mut preview = CandidatePreview::new("host".into());
    for variant in 0..4 {
        let mut e = candidate("bad", "甲");
        match variant {
            0 => e.source_kind = "dom".into(),
            1 => e.payload["arrival_text_match"] = json!(false),
            2 => e.payload["message_type"] = json!(1),
            _ => e.payload["observed_at_ms"] = json!(1),
        }
        preview.ingest(0, &e);
    }
    assert_eq!(preview.status(0)["pending_welcome_users"], 0);
    preview.ingest(0, &candidate("a", "甲"));
    let mut pause = candidate("pause", "甲");
    pause.event_type = "probe.status".into();
    pause.payload = json!({"stage":"wechat_collection_paused"});
    assert_eq!(
        preview.ingest(1, &pause)[0]["kind"],
        "welcome_candidate_cancelled"
    );
    assert!(preview.advance(30_000).is_empty());
    let mut disabled = CandidatePreview::with_format(
        "host".into(),
        &RulesConfig {
            welcome_enabled: false,
            ..Default::default()
        },
    )
    .unwrap();
    disabled.ingest(0, &candidate("a", "甲"));
    assert!(disabled.advance(30_000).is_empty());
}

#[test]
fn upgrade_removes_only_its_user_from_an_already_queued_group_welcome() {
    let mut engine = Engine::new(RulesConfig::default()).unwrap();
    engine.ingest(0, arrival("a", "甲", "douyin")).unwrap();
    engine.ingest(1, arrival("b", "乙", "douyin")).unwrap();
    engine.advance(30_000).unwrap();
    let mut upgrade = arrival("upgrade", "甲", "douyin");
    upgrade.kind = EventKind::LevelUpgraded(live_archive_core::moderation::events::Level {
        kind: "fansclub".into(),
        name: "粉丝团".into(),
        new_level: 3,
    });
    let logs = engine.ingest(31_000, upgrade).unwrap();
    assert!(logs.iter().any(|a| {
        a.kind == "cancelled"
            && a.action
                .as_ref()
                .is_some_and(|a| a.text == "欢迎甲、乙来到直播间")
    }));
    let welcomes: Vec<_> = engine
        .pending_actions()
        .into_iter()
        .filter(|a| a.kind == ActionKind::Welcome)
        .collect();
    assert_eq!(welcomes.len(), 1);
    assert_eq!(welcomes[0].text, "欢迎乙来到直播间");
    assert_eq!(welcomes[0].expires_at, 40_000);
}

#[test]
fn delayed_wechat_poll_discards_stale_batch_and_gifts_remove_waiting_user() {
    let mut preview = CandidatePreview::new("host".into());
    preview.ingest(0, &candidate("a", "甲"));
    assert_eq!(
        preview.advance(40_000)[0]["kind"],
        "welcome_candidate_skipped"
    );
    let mut preview = CandidatePreview::new("host".into());
    preview.ingest(0, &candidate("a", "甲"));
    preview.ingest(1, &candidate("b", "乙"));
    let mut gift = candidate("gift", "甲");
    gift.event_type = "gift.candidate".into();
    gift.payload = json!({"decoder":"wechat.app20009.reward.candidate.v1","message_type":20009,
        "gift_id":"rose","gift_name":"玫瑰","reward_product_count":1,"recipient_user_id":"host"});
    preview.ingest(2, &gift);
    let logs = preview.advance(30_000);
    assert_eq!(
        logs.iter()
            .find(|a| a["kind"] == "welcome_candidate_draft")
            .unwrap()["text"],
        "欢迎乙来到直播间"
    );
}

fn supporter(id: &str, user: &str, title: &str) -> ProbeEvent {
    let mut e = candidate(id, user);
    e.payload["supporter_title"] = json!(title);
    e
}
fn drafts(logs: &[serde_json::Value]) -> Vec<&serde_json::Value> {
    logs.iter()
        .filter(|a| a["kind"] == "welcome_candidate_draft")
        .collect()
}

#[test]
fn supporter_welcomes_are_immediate_and_throttled_per_user_across_both_titles() {
    let mut preview = CandidatePreview::new("host".into());
    let first = preview.ingest(0, &supporter("a", "甲", "super_fan"));
    assert_eq!(
        drafts(&first)[0]["text"],
        "欢迎超粉甲来到直播间，感谢支持！"
    );
    assert!(drafts(&preview.ingest(1, &supporter("b", "甲", "star_guardian"))).is_empty());
    assert!(drafts(&preview.ingest(59_999, &supporter("c", "甲", "super_fan"))).is_empty());
    // Other users have independent budgets, even inside the same minute.
    assert_eq!(
        drafts(&preview.ingest(59_999, &supporter("d", "乙", "star_guardian")))[0]["text"],
        "欢迎星守护乙来到直播间，感谢支持！"
    );
    assert_eq!(
        drafts(&preview.ingest(60_000, &supporter("e", "甲", "star_guardian"))).len(),
        1
    );
}

#[test]
fn supporter_welcome_removes_normal_batch_and_does_not_repeat_at_thirty_seconds() {
    let mut preview = CandidatePreview::new("host".into());
    preview.ingest(0, &candidate("a", "甲"));
    preview.ingest(1, &candidate("b", "乙"));
    assert_eq!(
        drafts(&preview.ingest(2, &supporter("c", "甲", "super_fan"))).len(),
        1
    );
    preview.ingest(3, &candidate("d", "甲"));
    assert_eq!(
        drafts(&preview.advance(30_000))[0]["text"],
        "欢迎乙来到直播间"
    );
    assert!(preview.advance(60_000).is_empty());
}

#[test]
fn supporter_config_and_unknown_titles_keep_ordinary_welcomes_separate() {
    let mut config = RulesConfig {
        welcome_enabled: false,
        ..Default::default()
    };
    let mut preview = CandidatePreview::with_format("host".into(), &config).unwrap();
    assert_eq!(
        drafts(&preview.ingest(0, &supporter("a", "甲", "super_fan"))).len(),
        1
    );
    assert!(preview.ingest(1, &candidate("b", "乙")).is_empty());
    config.welcome_enabled = true;
    config.supporter_welcome_enabled = false;
    let mut preview = CandidatePreview::with_format("host".into(), &config).unwrap();
    assert!(drafts(&preview.ingest(0, &supporter("a", "甲", "super_fan"))).is_empty());
    assert_eq!(
        drafts(&preview.advance(30_000))[0]["text"],
        "欢迎甲来到直播间"
    );
    let mut preview = CandidatePreview::new("host".into());
    assert!(drafts(&preview.ingest(0, &supporter("a", "甲", "unrecognized"))).is_empty());
    assert_eq!(
        drafts(&preview.advance(30_000))[0]["text"],
        "欢迎甲来到直播间"
    );
}

#[test]
fn core_supporter_cooldown_is_per_scope_per_user_and_prevents_general_duplicates() {
    use live_archive_core::moderation::events::SupporterTitle;
    let mut core = Engine::new(RulesConfig::default()).unwrap();
    let special = |id: &str, user: &str| {
        let mut e = arrival(id, user, "douyin");
        e.kind = EventKind::SupporterEntered(SupporterTitle::SuperFan);
        e
    };
    assert_eq!(
        welcomed(&core.ingest(0, special("a", "甲")).unwrap()),
        ["欢迎超粉甲来到直播间，感谢支持！"]
    );
    assert!(
        core.ingest(1, special("b", "甲"))
            .unwrap()
            .iter()
            .any(|a| a.reason == "supporter_welcome_cooldown")
    );
    assert!(welcomed(&core.ingest(2, arrival("normal", "甲", "douyin")).unwrap()).is_empty());
    assert_eq!(
        welcomed(&core.ingest(2, special("c", "乙")).unwrap()).len(),
        1
    );
    let mut other_room = special("other", "甲");
    other_room.scope.room_id = "another".into();
    assert_eq!(welcomed(&core.ingest(3, other_room).unwrap()).len(), 1);
    assert!(welcomed(&core.advance(30_000).unwrap()).is_empty());
    assert!(welcomed(&core.ingest(59_999, special("d", "甲")).unwrap()).is_empty());
    assert_eq!(
        welcomed(&core.ingest(60_000, special("e", "甲")).unwrap()).len(),
        1
    );
}
