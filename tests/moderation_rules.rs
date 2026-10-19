use live_archive_core::moderation::{
    engine::{ActionKind, Audit, Engine, PreviewAction, RulesConfig},
    events::{CoreEvent, EventKind, Evidence, Gift, Level, QuantityMode, Scope, User},
    replay::{Frame, replay},
    templates::Template,
};
use std::collections::HashMap;

fn scope() -> Scope {
    Scope {
        platform: "douyin".into(),
        room_id: "test-room".into(),
        live_session_id: "session-1".into(),
        actor_user_id: "assistant".into(),
        recipient_user_id: "anchor".into(),
    }
}
fn event(id: &str, user: &str, kind: EventKind) -> CoreEvent {
    CoreEvent {
        event_id: id.into(),
        scope: scope(),
        native_event_id: Some(format!("native-{id}")),
        source: "verified_network".into(),
        evidence: Evidence::Verified,
        user: Some(User {
            id: Some(user.into()),
            nickname: user.into(),
        }),
        kind,
    }
}
fn gift(id: &str, user: &str, name: &str, quantity: u64) -> CoreEvent {
    event(
        id,
        user,
        EventKind::Gift(Gift {
            unit_value: None,
            gift_id: name.into(),
            name: name.into(),
            recipient_user_id: "anchor".into(),
            quantity_mode: QuantityMode::Incremental,
            quantity: Some(quantity),
            combo_id: None,
            combo_end: false,
        }),
    )
}
fn cumulative(id: &str, quantity: u64, end: bool) -> CoreEvent {
    let mut e = gift(id, "甲", "玫瑰", quantity);
    if let EventKind::Gift(g) = &mut e.kind {
        g.quantity_mode = QuantityMode::Cumulative;
        g.combo_id = Some("combo-1".into());
        g.combo_end = end;
    }
    e
}
fn plans(logs: &[Audit]) -> Vec<PreviewAction> {
    logs.iter()
        .filter(|a| a.kind == "planned")
        .map(|a| a.action.clone().unwrap())
        .collect()
}
fn engine() -> Engine {
    Engine::new(RulesConfig::default()).unwrap()
}
fn level(id: &str, kind: &str, n: u32, snapshot: bool) -> CoreEvent {
    let l = Level {
        kind: kind.into(),
        name: "等级".into(),
        new_level: n,
    };
    event(
        id,
        "甲",
        if snapshot {
            EventKind::LevelSnapshot(l)
        } else {
            EventKind::LevelUpgraded(l)
        },
    )
}

#[test]
fn identical_names_keep_exact_totals_across_large_distinct_gift_ids() {
    let mut core = engine();
    let mut second = gift("large-2", "甲", "玫瑰", u64::MAX);
    if let EventKind::Gift(g) = &mut second.kind {
        g.gift_id = "second-rose".into();
    }
    core.ingest(0, gift("large-1", "甲", "玫瑰", u64::MAX))
        .unwrap();
    core.ingest(1, second).unwrap();
    assert_eq!(
        plans(&core.advance(5_001).unwrap())[0].text,
        "感谢甲送来的玫瑰×36893488147419103230"
    );
}

#[test]
fn one_native_message_can_contain_distinct_gifts_without_losing_items() {
    let mut core = engine();
    let first = gift("envelope-1", "甲", "玫瑰", 1);
    let mut second = gift("envelope-2", "甲", "跑车", 2);
    second.native_event_id = first.native_event_id.clone();
    core.ingest(0, first).unwrap();
    core.ingest(1, second.clone()).unwrap();
    second.event_id = "envelope-3".into();
    assert!(
        core.ingest(2, second)
            .unwrap()
            .iter()
            .any(|a| a.reason == "duplicate_native_gift")
    );
    assert_eq!(
        plans(&core.advance(5_001).unwrap())[0].text,
        "感谢甲送来的玫瑰×1、跑车×2"
    );
}

#[test]
fn users_have_independent_sliding_windows_and_stable_gift_order() {
    let mut core = engine();
    core.ingest(0, gift("a1", "甲", "玫瑰", 1)).unwrap();
    core.ingest(1_000, gift("b1", "乙", "星星", 1)).unwrap();
    core.ingest(3_000, gift("a2", "甲", "跑车", 2)).unwrap();
    assert!(plans(&core.advance(5_000).unwrap()).is_empty());
    let b = plans(&core.advance(6_000).unwrap());
    assert_eq!(b[0].text, "感谢乙送来的星星×1");
    let a = plans(&core.advance(8_000).unwrap());
    assert_eq!(a[0].text, "感谢甲送来的玫瑰×1、跑车×2");
    assert_eq!(a[0].expires_at, 28_000);
    assert_eq!(a[0].gift_batch.as_ref().unwrap().revision, 2);
    assert_eq!(a[0].gift_batch.as_ref().unwrap().event_ids, ["a1", "a2"]);
}

#[test]
fn cumulative_duplicates_reordering_and_end_do_not_add_or_extend() {
    let mut core = engine();
    for (i, n) in [1, 2, 2, 1, 3].into_iter().enumerate() {
        core.ingest(i as u64 * 1_000, cumulative(&format!("c{i}"), n, false))
            .unwrap();
    }
    core.ingest(5_000, cumulative("end", 3, true)).unwrap();
    assert!(plans(&core.advance(8_999).unwrap()).is_empty());
    let a = plans(&core.advance(9_000).unwrap());
    assert_eq!(a[0].text, "感谢甲送来的玫瑰×3");
    assert_eq!(a[0].gift_batch.as_ref().unwrap().revision, 3);
    assert_eq!(a[0].gift_batch.as_ref().unwrap().last_gift_at, 4_000);
    assert!(plans(&core.ingest(9_001, cumulative("late-end", 3, true)).unwrap()).is_empty());
}

#[test]
fn incremental_native_ids_dedupe_even_with_different_envelope_ids() {
    let mut core = engine();
    let first = gift("original", "甲", "玫瑰", 1);
    core.ingest(0, first.clone()).unwrap();
    assert!(
        core.ingest(1_000, first.clone())
            .unwrap()
            .iter()
            .any(|a| a.reason == "duplicate_event")
    );
    let mut retry = first;
    retry.event_id = "new-envelope".into();
    assert!(
        core.ingest(4_000, retry)
            .unwrap()
            .iter()
            .any(|a| a.reason == "duplicate_native_gift")
    );
    assert_eq!(
        plans(&core.advance(5_000).unwrap())[0].text,
        "感谢甲送来的玫瑰×1"
    );
}

#[test]
fn incremental_end_packet_is_not_a_new_gift() {
    let mut core = engine();
    core.ingest(0, gift("first", "甲", "玫瑰", 1)).unwrap();
    let mut end = gift("end", "甲", "玫瑰", 1);
    if let EventKind::Gift(g) = &mut end.kind {
        g.combo_end = true;
    }
    let logs = core.ingest(4_000, end).unwrap();
    assert!(
        logs.iter()
            .any(|a| a.reason == "combo_end_without_increment")
    );
    assert_eq!(
        plans(&core.advance(5_000).unwrap())[0].text,
        "感谢甲送来的玫瑰×1"
    );
}

#[test]
fn continuous_gifts_over_a_minute_do_not_force_a_flush() {
    let mut core = engine();
    for i in 0..=16 {
        assert!(
            plans(
                &core
                    .ingest(i * 4_000, gift(&format!("gift-{i}"), "甲", "星星", 1))
                    .unwrap()
            )
            .is_empty()
        );
        assert!(plans(&core.advance(i * 4_000 + 3_999).unwrap()).is_empty());
    }
    assert!(plans(&core.advance(68_999).unwrap()).is_empty());
    assert_eq!(
        plans(&core.advance(69_000).unwrap())[0].text,
        "感谢甲送来的星星×17"
    );
}

#[test]
fn exact_deadline_gift_precedes_timer_and_invalidates_old_revision() {
    let mut core = engine();
    core.ingest(0, gift("a", "甲", "星星", 1)).unwrap();
    assert!(plans(&core.ingest(5_000, gift("b", "甲", "星星", 1)).unwrap()).is_empty());
    assert!(plans(&core.advance(5_000).unwrap()).is_empty());
    let a = plans(&core.advance(10_000).unwrap());
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].text, "感谢甲送来的星星×2");
}

#[test]
fn queued_thanks_are_cancelled_and_reopened_with_all_gifts() {
    let mut core = engine();
    core.ingest(0, gift("a", "甲", "星星", 1)).unwrap();
    let first = plans(&core.advance(5_000).unwrap()).remove(0);
    let logs = core.ingest(6_000, gift("b", "甲", "跑车", 2)).unwrap();
    assert!(logs.iter().any(|a| a.kind == "cancelled"
        && a.reason == "new_gift"
        && a.action.as_ref().unwrap().action_id == first.action_id));
    let second = plans(&core.advance(11_000).unwrap()).remove(0);
    assert_eq!(second.text, "感谢甲送来的星星×1、跑车×2");
    assert_eq!(
        first.gift_batch.as_ref().unwrap().batch_id,
        second.gift_batch.as_ref().unwrap().batch_id
    );
    assert_eq!(second.gift_batch.as_ref().unwrap().revision, 2);
}

#[test]
fn simulated_dispatch_freezes_old_batch_and_new_gifts_start_another() {
    let mut core = engine();
    core.ingest(0, gift("a", "甲", "星星", 1)).unwrap();
    let old = plans(&core.advance(5_000).unwrap()).remove(0);
    let freeze = core.simulate_dispatch(5_001, &old.action_id).unwrap();
    assert!(freeze.iter().any(|a| a.kind == "dispatch_simulated"));
    core.ingest(6_000, gift("b", "甲", "星星", 1)).unwrap();
    let new = plans(&core.advance(11_000).unwrap()).remove(0);
    assert_eq!(new.text, "感谢甲送来的星星×1");
    assert_ne!(
        old.gift_batch.unwrap().batch_id,
        new.gift_batch.unwrap().batch_id
    );
    assert_eq!(new.mode, "preview");
}

#[test]
fn expiry_discards_old_window_and_new_gift_does_not_resend_it() {
    let mut core = engine();
    core.ingest(0, gift("a", "甲", "星星", 1)).unwrap();
    core.advance(5_000).unwrap();
    assert!(
        core.advance(25_000)
            .unwrap()
            .iter()
            .any(|a| a.kind == "expired")
    );
    core.ingest(26_000, gift("b", "甲", "星星", 1)).unwrap();
    assert_eq!(
        plans(&core.advance(31_000).unwrap())[0].text,
        "感谢甲送来的星星×1"
    );
}

#[test]
fn thirty_scalar_names_list_but_thirty_one_use_overflow() {
    for (n, overflow) in [(30, false), (31, true)] {
        let mut core = engine();
        core.ingest(0, gift("a", "很长的昵称", &"🎁".repeat(n), 123_456))
            .unwrap();
        let a = plans(&core.advance(5_000).unwrap()).remove(0);
        assert_eq!(a.text.contains("感谢礼物飘屏"), overflow);
        if overflow {
            assert_eq!(a.text, "感谢很长的昵称，感谢礼物飘屏");
        } else {
            assert!(a.text.contains("×123456"));
        }
    }
}

#[test]
fn identical_names_across_gift_ids_only_occupy_one_name() {
    let mut core = engine();
    let name = "花".repeat(20);
    core.ingest(0, gift("a", "甲", &name, 1)).unwrap();
    let mut second = gift("b", "甲", &name, 2);
    if let EventKind::Gift(g) = &mut second.kind {
        g.gift_id = "different-id".into();
    }
    core.ingest(1_000, second).unwrap();
    assert_eq!(
        plans(&core.advance(6_000).unwrap())[0].text,
        format!("感谢甲送来的{name}×3")
    );
}

#[test]
fn unknown_quantity_preserves_name_without_inventing_a_count() {
    let mut core = engine();
    let mut unknown = gift("a", "甲", "神秘礼物", 1);
    if let EventKind::Gift(g) = &mut unknown.kind {
        g.quantity_mode = QuantityMode::Unknown;
        g.quantity = None;
    }
    core.ingest(0, unknown).unwrap();
    core.ingest(1_000, gift("b", "甲", "星星", 3)).unwrap();
    assert_eq!(
        plans(&core.advance(6_000).unwrap())[0].text,
        "感谢甲送来的神秘礼物、星星×3"
    );
}

#[test]
fn identity_recipient_and_native_id_must_be_reliable() {
    let mut core = engine();
    let mut missing = gift("missing", "甲", "星星", 1);
    missing.user.as_mut().unwrap().id = None;
    assert!(
        core.ingest(0, missing)
            .unwrap()
            .iter()
            .any(|a| a.reason == "missing_stable_user_id")
    );
    let mut unverified = gift("unverified", "甲", "星星", 1);
    unverified.evidence = Evidence::Unverified;
    assert!(
        core.ingest(1, unverified)
            .unwrap()
            .iter()
            .any(|a| a.reason == "unverified_input")
    );
    let mut wrong = gift("wrong", "甲", "星星", 1);
    if let EventKind::Gift(g) = &mut wrong.kind {
        g.recipient_user_id = "another-anchor".into();
    }
    assert!(
        core.ingest(2, wrong)
            .unwrap()
            .iter()
            .any(|a| a.reason == "wrong_gift_recipient")
    );
    let mut native = gift("native", "甲", "星星", 1);
    native.native_event_id = None;
    assert!(
        core.ingest(3, native)
            .unwrap()
            .iter()
            .any(|a| a.reason == "missing_native_gift_id")
    );
    assert!(plans(&core.advance(10_000).unwrap()).is_empty());
}

#[test]
fn room_session_account_and_future_platform_scopes_do_not_mix() {
    let mut core = engine();
    for i in 0..5 {
        let mut e = gift("same-native-and-event", "甲", "星星", 1);
        match i {
            1 => e.scope.room_id = "other-room".into(),
            2 => e.scope.live_session_id = "other-session".into(),
            3 => e.scope.actor_user_id = "other-assistant".into(),
            4 => e.scope.platform = "future-platform".into(),
            _ => {}
        }
        core.ingest(0, e).unwrap();
    }
    let a = plans(&core.advance(5_000).unwrap());
    assert_eq!(a.len(), 5);
    assert!(a.iter().all(|a| a.text == "感谢甲送来的星星×1"));
}

#[test]
fn gap_and_room_end_cancel_waiting_work_and_only_new_session_recovers() {
    for kind in [EventKind::Gap, EventKind::RoomEnded] {
        let mut core = engine();
        core.ingest(0, gift("a", "甲", "星星", 1)).unwrap();
        core.advance(5_000).unwrap();
        assert!(
            core.ingest(6_000, event("stop", "甲", kind))
                .unwrap()
                .iter()
                .any(|a| a.kind == "cancelled")
        );
        assert!(
            core.ingest(7_000, gift("b", "甲", "星星", 1))
                .unwrap()
                .iter()
                .any(|a| a.reason == "scope_paused")
        );
        assert!(plans(&core.advance(12_000).unwrap()).is_empty());
        let mut fresh = gift("new", "甲", "星星", 1);
        fresh.scope.live_session_id = "session-2".into();
        core.ingest(13_000, fresh).unwrap();
        assert_eq!(plans(&core.advance(18_000).unwrap()).len(), 1);
    }
}

#[test]
fn resource_exhaustion_pauses_instead_of_forcing_an_early_thanks() {
    let config = RulesConfig {
        max_active_users: 1,
        ..Default::default()
    };
    let mut core = Engine::new(config).unwrap();
    core.ingest(0, gift("a", "甲", "星星", 1)).unwrap();
    assert!(
        core.ingest(1, gift("b", "乙", "星星", 1))
            .unwrap()
            .iter()
            .any(|a| a.reason == "active_user_capacity_gap")
    );
    assert!(plans(&core.advance(5_001).unwrap()).is_empty());
}

#[test]
fn item_dedupe_and_text_capacity_fail_explicitly() {
    let mut core = Engine::new(RulesConfig {
        max_gift_items: 1,
        ..Default::default()
    })
    .unwrap();
    core.ingest(0, gift("a", "甲", "星星", 1)).unwrap();
    assert!(
        core.ingest(1, gift("b", "甲", "跑车", 1))
            .unwrap()
            .iter()
            .any(|a| a.reason == "gift_window_capacity_gap")
    );
    let mut core = Engine::new(RulesConfig {
        max_dedupe_entries: 1,
        ..Default::default()
    })
    .unwrap();
    assert!(
        core.ingest(0, gift("a", "甲", "星星", 1))
            .unwrap()
            .iter()
            .any(|a| a.reason == "dedupe_capacity_gap")
    );
    let mut core = Engine::new(RulesConfig {
        max_text_chars: 1,
        ..Default::default()
    })
    .unwrap();
    core.ingest(0, gift("a", "甲", "星星", 1)).unwrap();
    assert!(
        core.advance(5_000)
            .unwrap()
            .iter()
            .any(|a| a.reason == "text_too_long")
    );
}

#[test]
fn event_collision_mixed_counts_and_numeric_overflow_pause() {
    let mut core = engine();
    core.ingest(0, gift("same", "甲", "星星", 1)).unwrap();
    assert!(
        core.ingest(1, gift("same", "甲", "星星", 2))
            .unwrap()
            .iter()
            .any(|a| a.reason == "event_id_conflict")
    );
    let mut core = engine();
    core.ingest(0, cumulative("a", 1, false)).unwrap();
    let mut mixed = cumulative("b", 1, false);
    if let EventKind::Gift(g) = &mut mixed.kind {
        g.quantity_mode = QuantityMode::Incremental;
    }
    assert!(
        core.ingest(1, mixed)
            .unwrap()
            .iter()
            .any(|a| a.reason == "mixed_quantity_semantics")
    );
    let mut core = engine();
    core.ingest(0, gift("a", "甲", "星星", u64::MAX)).unwrap();
    assert!(
        core.ingest(1, gift("b", "甲", "星星", 1))
            .unwrap()
            .iter()
            .any(|a| a.reason == "gift_count_overflow")
    );
}

#[test]
fn welcome_cooldown_is_per_user_and_gifts_cancel_low_priority_welcome() {
    let mut core = engine();
    assert!(
        plans(
            &core
                .ingest(0, event("w1", "甲", EventKind::Welcome))
                .unwrap()
        )
        .is_empty()
    );
    assert!(
        core.ingest(1, event("w2", "甲", EventKind::Welcome))
            .unwrap()
            .iter()
            .any(|a| a.reason == "welcome_cooldown")
    );
    assert!(
        plans(
            &core
                .ingest(1, event("w3", "乙", EventKind::Welcome))
                .unwrap()
        )
        .is_empty()
    );
    assert!(
        core.ingest(2, gift("gift", "甲", "星星", 1))
            .unwrap()
            .iter()
            .any(|a| a.kind == "cancelled" && a.reason == "higher_priority_interaction")
    );
    let logs = core.advance(30_000).unwrap();
    assert_eq!(
        plans(&logs)
            .iter()
            .filter(|a| a.kind == ActionKind::Welcome)
            .map(|a| a.text.as_str())
            .collect::<Vec<_>>(),
        ["欢迎乙来到直播间"]
    );
    core.advance(60_000).unwrap();
    core.ingest(60_001, event("w4", "乙", EventKind::Welcome))
        .unwrap();
    assert_eq!(
        plans(&core.advance(90_001).unwrap())[0].text,
        "欢迎乙来到直播间"
    );
}

#[test]
fn bounded_preview_queue_evicts_welcome_but_does_not_displace_other_thanks() {
    let mut core = Engine::new(RulesConfig {
        max_pending_actions: 1,
        ..Default::default()
    })
    .unwrap();
    core.ingest(0, event("welcome", "甲", EventKind::FansclubJoined))
        .unwrap();
    core.ingest(1, gift("gift", "乙", "星星", 1)).unwrap();
    let logs = core.advance(5_001).unwrap();
    assert!(logs.iter().any(|a| a.reason == "queue_priority"));
    assert_eq!(plans(&logs)[0].kind, ActionKind::GiftThanks);
    core.ingest(6_000, gift("gift2", "丙", "星星", 1)).unwrap();
    assert!(
        core.advance(11_000)
            .unwrap()
            .iter()
            .any(|a| a.reason == "queue_full")
    );
}

#[test]
fn snapshots_only_baseline_by_default_but_native_upgrade_still_counts() {
    let mut core = engine();
    assert!(plans(&core.ingest(0, level("s1", "fansclub", 1, true)).unwrap()).is_empty());
    assert!(
        core.ingest(1, level("s2", "fansclub", 3, true))
            .unwrap()
            .iter()
            .any(|a| a.reason == "inferred_levels_disabled")
    );
    assert_eq!(
        plans(&core.ingest(2, level("n1", "fansclub", 3, false)).unwrap()).len(),
        1
    );
    assert!(
        core.ingest(3, level("n2", "fansclub", 3, false))
            .unwrap()
            .iter()
            .any(|a| a.reason == "duplicate_level_upgrade")
    );
}

#[test]
fn inferred_upgrades_require_opt_in_and_ignore_decrease_source_changes_and_duplicates() {
    let mut core = Engine::new(RulesConfig {
        allow_inferred_levels: true,
        ..Default::default()
    })
    .unwrap();
    core.ingest(0, level("base", "wealth", 3, true)).unwrap();
    assert!(plans(&core.ingest(1, level("down", "wealth", 2, true)).unwrap()).is_empty());
    assert!(plans(&core.ingest(2, level("back", "wealth", 3, true)).unwrap()).is_empty());
    let a = plans(&core.ingest(3, level("up", "wealth", 4, true)).unwrap());
    assert_eq!(a[0].evidence, Evidence::Inferred);
    assert!(plans(&core.ingest(4, level("native", "wealth", 4, false)).unwrap()).is_empty());
    let mut changed = level("source-change", "wealth", 5, true);
    changed.source = "another_source".into();
    assert!(
        core.ingest(5, changed)
            .unwrap()
            .iter()
            .any(|a| a.reason == "level_source_changed")
    );
}

#[test]
fn fanclub_honor_and_wealth_levels_have_separate_deduplication() {
    let mut core = engine();
    for (i, kind) in ["fansclub", "honor", "wealth"].into_iter().enumerate() {
        assert_eq!(
            plans(&core.ingest(i as u64, level(kind, kind, 3, false)).unwrap()).len(),
            1
        );
    }
}

#[test]
fn templates_validate_variables_render_once_and_clean_nickname_controls() {
    assert!(Template::parse("{unknown}", &["nickname"]).is_err());
    assert!(Template::parse("{nickname", &["nickname"]).is_err());
    let template = Template::parse("{{{nickname}}}", &["nickname"]).unwrap();
    assert_eq!(
        template
            .render(&HashMap::from([("nickname", "{gift_list}".into())]))
            .unwrap(),
        "{{gift_list}}"
    );
    let mut core = engine();
    let mut e = gift("a", "甲", "星星", 1);
    e.user.as_mut().unwrap().nickname = "甲\n{gift_list}".into();
    core.ingest(0, e).unwrap();
    assert_eq!(
        plans(&core.advance(5_000).unwrap())[0].text,
        "感谢甲{gift_list}送来的星星×1"
    );
}

#[test]
fn unknown_event_types_are_ignored_without_breaking_the_stream() {
    let mut value = serde_json::to_value(event("future", "甲", EventKind::Welcome)).unwrap();
    value["type"] = "platform.future_event".into();
    value["data"] = serde_json::json!({"future":true});
    let e: CoreEvent = serde_json::from_value(value).unwrap();
    let mut core = engine();
    assert!(
        core.ingest(0, e)
            .unwrap()
            .iter()
            .any(|a| a.reason == "unsupported_event")
    );
    core.ingest(1, gift("known", "甲", "星星", 1)).unwrap();
    assert_eq!(plans(&core.advance(5_001).unwrap()).len(), 1);
}

#[test]
fn replay_is_deterministic_sorted_and_processes_deadline_events_before_ticks() {
    let frames = vec![
        Frame {
            at_ms: 5_000,
            event: None,
            simulate_dispatch: None,
        },
        Frame {
            at_ms: 0,
            event: Some(gift("a", "甲", "星星", 1)),
            simulate_dispatch: None,
        },
        Frame {
            at_ms: 5_000,
            event: Some(gift("b", "甲", "星星", 1)),
            simulate_dispatch: None,
        },
    ];
    let mut runs = Vec::new();
    for _ in 0..2 {
        let mut logs = Vec::new();
        let summary = replay(frames.clone(), RulesConfig::default(), None, |a| {
            logs.push(a.clone());
            Ok(())
        })
        .unwrap();
        assert!(!summary.send_chat);
        assert_eq!(summary.planned, 1);
        assert_eq!(plans(&logs)[0].created_at, 10_000);
        runs.push(serde_json::to_string(&logs).unwrap());
    }
    assert_eq!(runs[0], runs[1]);
}

#[test]
fn invalid_semantics_clock_and_replay_limits_are_rejected() {
    let mut invalid = cumulative("a", 1, false);
    if let EventKind::Gift(g) = &mut invalid.kind {
        g.combo_id = None;
    }
    assert!(engine().ingest(0, invalid).is_err());
    let mut core = engine();
    core.advance(1).unwrap();
    assert!(core.advance(0).is_err());
    assert!(core.advance(u64::MAX).is_err());
    assert!(replay(Vec::new(), RulesConfig::default(), None, |_| Ok(())).is_err());
    assert!(
        replay(
            vec![Frame {
                at_ms: 86_400_001,
                event: None,
                simulate_dispatch: None
            }],
            RulesConfig::default(),
            None,
            |_| Ok(())
        )
        .is_err()
    );
}

#[test]
fn replay_cli_is_offline_ignores_recorder_config_and_never_overwrites_audit() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = directory.path().join("sample.jsonl");
    let output = directory.path().join("audit.jsonl");
    std::fs::write(
        directory.path().join("live-archive.toml"),
        "invalid recorder config",
    )
    .unwrap();
    let frame = Frame {
        at_ms: 0,
        event: Some(gift("test", "甲", "星星", 1)),
        simulate_dispatch: None,
    };
    std::fs::write(&fixture, serde_json::to_string(&frame).unwrap()).unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
            .current_dir(directory.path())
            .args(["moderation", "replay", "--fixture"])
            .arg(&fixture)
            .arg("--output")
            .arg(&output)
            .output()
            .unwrap()
    };
    let first = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let audit = std::fs::read_to_string(&output).unwrap();
    assert!(audit.contains("感谢甲送来的星星×1"));
    assert!(audit.contains("offline_preview"));
    assert!(!directory.path().join("live-archive.sqlite3").exists());
    assert!(!run().status.success());
    assert_eq!(std::fs::read_to_string(&output).unwrap(), audit);
    let bad = directory.path().join("bad-output.jsonl");
    std::fs::write(&fixture, "invalid JSON").unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_live-archive-core"))
        .args(["moderation", "replay", "--fixture"])
        .arg(&fixture)
        .arg("--output")
        .arg(&bad)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!bad.exists());
}
