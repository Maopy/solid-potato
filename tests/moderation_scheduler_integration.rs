//! Platform-neutral events -> rules queue -> shared account scheduler -> audit.
//! Virtual time avoids real platform writes and minute-long test sleeps.
use live_archive_core::moderation::{
    engine::{Audit, Engine, RulesConfig},
    events::{CoreEvent, EventKind, Evidence, Gift, Level, QuantityMode, Scope, User},
    scheduler::PreviewScheduler,
};

fn event(id: &str, user: &str, kind: EventKind) -> CoreEvent {
    CoreEvent {
        event_id: id.into(),
        native_event_id: Some(id.into()),
        source: "integration".into(),
        evidence: Evidence::Verified,
        scope: Scope {
            platform: "douyin".into(),
            room_id: "room-1".into(),
            live_session_id: "session-1".into(),
            actor_user_id: "assistant-1".into(),
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
fn texts(logs: &[Audit]) -> Vec<String> {
    logs.iter()
        .filter(|a| a.kind == "dispatch_preview")
        .map(|a| a.action.as_ref().unwrap().text.clone())
        .collect()
}
fn pipeline() -> (Engine, PreviewScheduler) {
    (
        Engine::new(RulesConfig::default()).unwrap(),
        PreviewScheduler::default(),
    )
}

#[test]
fn same_account_shares_budget_across_rooms_but_other_accounts_are_independent() {
    let (mut core, mut scheduler) = pipeline();
    core.ingest(0, event("a", "甲", EventKind::FansclubJoined))
        .unwrap();
    assert_eq!(texts(&scheduler.advance(0, &mut core).unwrap()).len(), 1);
    let mut other_room = event("b", "乙", EventKind::FansclubJoined);
    other_room.scope.room_id = "room-2".into();
    core.ingest(1000, other_room).unwrap();
    let mut other_account = event("c", "丙", EventKind::FansclubJoined);
    other_account.scope.actor_user_id = "assistant-2".into();
    core.ingest(1000, other_account).unwrap();
    assert_eq!(
        texts(&scheduler.advance(1000, &mut core).unwrap()),
        ["欢迎丙加入粉丝团"]
    );
    assert!(texts(&scheduler.advance(7999, &mut core).unwrap()).is_empty());
    assert_eq!(
        texts(&scheduler.advance(8000, &mut core).unwrap()),
        ["欢迎乙加入粉丝团"]
    );
}

#[test]
fn sliding_minute_budget_blocks_seventh_then_releases_at_exact_boundary() {
    let (mut core, mut scheduler) = pipeline();
    for i in 0..6 {
        let at = i * 8000;
        core.ingest(
            at,
            event(
                &format!("e{i}"),
                &format!("u{i}"),
                EventKind::FansclubJoined,
            ),
        )
        .unwrap();
        assert_eq!(texts(&scheduler.advance(at, &mut core).unwrap()).len(), 1);
    }
    core.ingest(
        48000,
        event(
            "seventh",
            "七",
            EventKind::LevelUpgraded(Level {
                kind: "fansclub".into(),
                name: "粉丝团".into(),
                new_level: 3,
            }),
        ),
    )
    .unwrap();
    assert!(texts(&scheduler.advance(48000, &mut core).unwrap()).is_empty());
    assert_eq!(scheduler.status(&core)["next_eligible_at_ms"], 60000);
    assert!(texts(&scheduler.advance(59999, &mut core).unwrap()).is_empty());
    assert_eq!(
        texts(&scheduler.advance(60000, &mut core).unwrap()),
        ["恭喜七的粉丝团升级到3级"]
    );
}

#[test]
fn queued_gifts_reopen_at_dispatch_boundary_then_freeze_as_separate_batches() {
    let (mut core, mut scheduler) = pipeline();
    core.ingest(0, event("welcome", "乙", EventKind::FansclubJoined))
        .unwrap();
    scheduler.advance(0, &mut core).unwrap();
    core.ingest(0, gift("gift-1", "甲", "玫瑰")).unwrap();
    assert!(texts(&scheduler.advance(5000, &mut core).unwrap()).is_empty());
    assert_eq!(scheduler.status(&core)["pending"], 1);
    let logs = core.ingest(8000, gift("gift-2", "甲", "跑车")).unwrap();
    assert!(
        logs.iter()
            .any(|a| a.kind == "cancelled" && a.reason == "new_gift")
    );
    assert!(texts(&scheduler.advance(8000, &mut core).unwrap()).is_empty());
    assert_eq!(
        texts(&scheduler.advance(13000, &mut core).unwrap()),
        ["感谢甲送来的玫瑰×1、跑车×1"]
    );
    core.ingest(13001, gift("gift-3", "甲", "星星")).unwrap();
    scheduler.advance(18001, &mut core).unwrap();
    assert_eq!(
        texts(&scheduler.advance(21000, &mut core).unwrap()),
        ["感谢甲送来的星星×1"]
    );
}

#[test]
fn priorities_and_expiry_do_not_consume_budget_and_gap_cancels_queue() {
    let (mut core, mut scheduler) = pipeline();
    core.ingest(0, event("first", "首", EventKind::FansclubJoined))
        .unwrap();
    scheduler.advance(0, &mut core).unwrap();
    core.ingest(1000, event("welcome", "乙", EventKind::FansclubJoined))
        .unwrap();
    core.ingest(1000, gift("gift", "甲", "玫瑰")).unwrap();
    core.ingest(
        1000,
        event(
            "upgrade",
            "丙",
            EventKind::LevelUpgraded(Level {
                kind: "fansclub".into(),
                name: "粉丝团".into(),
                new_level: 3,
            }),
        ),
    )
    .unwrap();
    assert_eq!(
        texts(&scheduler.advance(8000, &mut core).unwrap()),
        ["恭喜丙的粉丝团升级到3级"]
    );
    let logs = scheduler.advance(11000, &mut core).unwrap();
    assert!(
        logs.iter()
            .any(|a| a.kind == "expired" && a.action.as_ref().unwrap().user_id == "乙")
    );
    assert_eq!(
        texts(&scheduler.advance(16000, &mut core).unwrap()),
        ["感谢甲送来的玫瑰×1"]
    );
    core.ingest(16001, event("pending", "丁", EventKind::FansclubJoined))
        .unwrap();
    core.ingest(16002, event("gap", "丁", EventKind::Gap))
        .unwrap();
    assert!(texts(&scheduler.advance(24000, &mut core).unwrap()).is_empty());
    assert_eq!(scheduler.status(&core)["pending"], 0);
    assert_eq!(scheduler.status(&core)["dispatch_previews"], 3);
}

#[test]
fn exact_ttl_boundary_expires_instead_of_dispatching_and_clock_cannot_rewind() {
    let (mut core, mut scheduler) = pipeline();
    for i in 0..6 {
        let at = i * 8000;
        core.ingest(
            at,
            event(
                &format!("e{i}"),
                &format!("u{i}"),
                EventKind::FansclubJoined,
            ),
        )
        .unwrap();
        scheduler.advance(at, &mut core).unwrap();
    }
    core.ingest(50000, event("ttl", "过期", EventKind::FansclubJoined))
        .unwrap();
    let logs = scheduler.advance(60000, &mut core).unwrap();
    assert!(texts(&logs).is_empty());
    assert!(logs.iter().any(|a| a.kind == "expired"));
    assert!(scheduler.advance(59999, &mut core).is_err());
}
