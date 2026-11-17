use live_archive_core::moderation::{
    account_budget::AccountBudget, config::ModerationConfig, userscript::UserscriptHub,
};
use std::sync::{Arc, Mutex};

fn source() -> &'static str {
    "version: 1\nmode: fill_only\nsending:\n  automatic: true\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/123\nlimits:\n  max_text_chars: 80\nrules:\n  gift:\n    template: '谢谢{nickname}的{gift_list}'\n  upgrade:\n    enabled: true\nnickname_mapping:\n  users:\n    default-user: 默认哥\ntimed_messages:\n  min_comments: 5\nlikes:\n  interval_ms: 100\n"
}
fn load(source: &str) -> anyhow::Result<ModerationConfig> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yml");
    std::fs::write(&path, source).unwrap();
    ModerationConfig::load(&path)
}
#[test]
fn profiles_deep_merge_and_keep_legacy_defaults() {
    let config=load(&(source().to_owned()+"room_profiles:\n  '123':\n    rules:\n      welcome:\n        enabled: false\n      upgrade:\n        template: '{nickname}到{new_level}啦'\n    timed_messages:\n      min_comments: 9\n      messages: ['专属文案']\n    nickname_mapping:\n      users:\n        room-user: 专属哥\n")).unwrap();
    assert!(config.rules.welcome.enabled);
    assert_eq!(config.timed_messages.min_comments, 5);
    let profile = &config.resolved_room_profiles["123"];
    assert!(!profile.rules.welcome_enabled);
    assert_eq!(profile.rules.gift_template, "谢谢{nickname}的{gift_list}");
    assert_eq!(profile.rules.upgrade_template, "{nickname}到{new_level}啦");
    assert_eq!(profile.timed.min_comments, 9);
    assert_eq!(profile.timed.messages, ["专属文案"]);
    assert_eq!(profile.rules.gift_nicknames["default-user"], "默认哥");
    assert_eq!(profile.rules.gift_nicknames["room-user"], "专属哥");
    assert!(load(source()).unwrap().resolved_room_profiles.is_empty());
}
#[test]
fn profiles_validate_every_override_before_startup() {
    for patch in [
        "mode: observe",
        "sending: {automatic: false}",
        "room_profiles: {}",
        "rules: {upgrade: {template: '{unknown}'}}",
        "timed_messages: {min_comments: -1}",
        "rules: {supporter_welcome: {min_fansclub_level: 0}}",
    ] {
        let text = format!("{}room_profiles:\n  '123':\n    {patch}\n", source());
        assert!(load(&text).is_err(), "{patch}");
    }
    assert!(
        load(&(source().to_owned() + "room_profiles:\n  'https://live.douyin.com/123': {}\n"))
            .is_err()
    );
    assert!(load(&(source().to_owned() + "room_profiles:\n  '123': []\n")).is_err());
}
#[tokio::test]
async fn userscript_selects_each_opened_room_and_unmatched_rooms_keep_defaults() {
    let config=load(&(source().to_owned()+"room_profiles:\n  '123':\n    likes: {interval_ms: 135}\n    timed_messages: {min_comments: 9}\n  '456':\n    likes: {interval_ms: 150}\n    timed_messages: {min_comments: 12}\n")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let hub = UserscriptHub::new(dir.path().into(), false);
    hub.add_template(
        "dy",
        "account",
        config.probe_options(),
        Arc::new(Mutex::new(AccountBudget::new(Default::default()).unwrap())),
    )
    .unwrap();
    for (id, comments, interval) in [
        ("123", 9, 135),
        ("456", 12, 150),
        ("789", 5, 100),
        ("123", 9, 135),
    ] {
        let boot = hub
            .bootstrap(&format!("https://live.douyin.com/{id}"))
            .unwrap();
        assert_eq!(boot["timed_messages"]["min_comments"], comments);
        assert_eq!(boot["likes"]["interval_ms"], interval);
    }
    hub.stop().await.unwrap();
}

#[tokio::test]
async fn like_claim_uses_the_connected_rooms_settings() {
    let config = load(&(source().to_owned() + "room_profiles:\n  '123':\n    likes: {enabled: true, mode: clock, every_minutes: 5, batch_count: 25}\n  '456':\n    likes: {enabled: true, mode: clock, every_minutes: 10, batch_count: 40}\n")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let hub = UserscriptHub::new(dir.path().into(), false);
    hub.add_template(
        "dy",
        "account",
        config.probe_options(),
        Arc::new(Mutex::new(AccountBudget::new(Default::default()).unwrap())),
    )
    .unwrap();
    for (room, minutes, count) in [("123", 5, 25), ("456", 10, 40)] {
        let page = format!("https://live.douyin.com/{room}");
        let boot = hub.bootstrap(&page).unwrap();
        let hello = hub.dispatch(&page,"hello","",serde_json::json!({"v":1,"ticket":boot["ticket"],"page_url":page,"document_id":"doc","adapter_id":"douyin.live_page","client_id":"6647e65f-df75-4f4c-a565-e076490400ee"})).unwrap();
        let scope = serde_json::json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":page});
        let result = hub
            .dispatch(
                &page,
                "likes/claim",
                hello["token"].as_str().unwrap(),
                scope,
            )
            .unwrap();
        assert_eq!(result["batch_count"], count);
        assert_eq!(
            result["period_end_ms"].as_i64().unwrap() - result["period_start_ms"].as_i64().unwrap(),
            minutes * 60_000
        );
    }
    hub.stop().await.unwrap();
}

#[test]
fn wechat_clock_like_claim_requires_identity_and_uses_room_config() {
    use live_archive_core::moderation::{
        bridge::ProbeBridge, journal::ProbeJournal, likes::LikeClaim, protocol::ProbeScope,
    };
    use serde_json::{Value, json};
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = ProbeBridge::new(
        ProbeScope::from_room_url(
            "https://channels.weixin.qq.com/web/pages/live"
                .parse()
                .unwrap(),
        )
        .unwrap(),
        ProbeJournal::open(&dir.path().join("journal.sqlite3")).unwrap(),
    );
    let likes = serde_json::from_value(
        json!({"enabled":true,"mode":"clock","every_minutes":5,"batch_count":12}),
    )
    .unwrap();
    bridge.configure_likes(likes).unwrap();
    let boot = bridge
        .bootstrap_for_room(&bridge.relay_token.clone(), Some("host-a"))
        .unwrap();
    let boot: Value = serde_json::from_str(
        boot.split("const BOOT = ")
            .nth(1)
            .unwrap()
            .split(";\n")
            .next()
            .unwrap(),
    )
    .unwrap();
    let hello=bridge.hello(serde_json::from_value(json!({"v":1,"ticket":boot["ticket"],"page_url":"https://channels.weixin.qq.com/web/pages/live","document_id":"doc","adapter_id":"wechat_channels.desktop_probe"})).unwrap()).unwrap();
    let claim = || {
        serde_json::from_value::<LikeClaim>(json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":"https://channels.weixin.qq.com/web/pages/live","initial":true})).unwrap()
    };
    let first = bridge
        .claim_like_period(hello["token"].as_str().unwrap(), claim())
        .unwrap();
    assert_eq!(first["claimed"], true);
    assert_eq!(first["batch_count"], 12);
    assert_eq!(
        bridge
            .claim_like_action(hello["token"].as_str().unwrap(), claim())
            .unwrap()["claimed"],
        true
    );
    assert_eq!(
        bridge
            .claim_like_action(hello["token"].as_str().unwrap(), claim())
            .unwrap()["claimed"],
        false
    );
    let second = bridge
        .claim_like_period(hello["token"].as_str().unwrap(), claim())
        .unwrap();
    if first["period_start_ms"] == second["period_start_ms"] {
        assert_eq!(second["claimed"], false);
    }
    let unbound = bridge
        .bootstrap_for_room(&bridge.relay_token.clone(), None)
        .unwrap();
    let unbound: Value = serde_json::from_str(
        unbound
            .split("const BOOT = ")
            .nth(1)
            .unwrap()
            .split(";\n")
            .next()
            .unwrap(),
    )
    .unwrap();
    let unbound=bridge.hello(serde_json::from_value(json!({"v":1,"ticket":unbound["ticket"],"page_url":"https://channels.weixin.qq.com/web/pages/live","document_id":"unbound","adapter_id":"wechat_channels.desktop_probe"})).unwrap()).unwrap();
    let claim:LikeClaim=serde_json::from_value(json!({"v":1,"connection_id":unbound["connection_id"],"binding_id":unbound["binding_id"],"page_url":"https://channels.weixin.qq.com/web/pages/live"})).unwrap();
    assert!(
        bridge
            .claim_like_period(unbound["token"].as_str().unwrap(), claim)
            .is_err()
    );
}

#[test]
fn wechat_continuous_likes_are_paced_by_backend() {
    use live_archive_core::moderation::{
        bridge::ProbeBridge, journal::ProbeJournal, likes::LikeClaim, protocol::ProbeScope,
    };
    use serde_json::{Value, json};
    let dir = tempfile::tempdir().unwrap();
    let mut bridge = ProbeBridge::new(
        ProbeScope::from_room_url(
            "https://channels.weixin.qq.com/web/pages/live"
                .parse()
                .unwrap(),
        )
        .unwrap(),
        ProbeJournal::open(&dir.path().join("journal.sqlite3")).unwrap(),
    );
    bridge
        .configure_likes(
            serde_json::from_value(json!({"enabled":true,"mode":"continuous","interval_ms":60000}))
                .unwrap(),
        )
        .unwrap();
    let script = bridge
        .bootstrap_for_room(&bridge.relay_token.clone(), Some("host"))
        .unwrap();
    let boot: Value = serde_json::from_str(
        script
            .split("const BOOT = ")
            .nth(1)
            .unwrap()
            .split(";\n")
            .next()
            .unwrap(),
    )
    .unwrap();
    let hello=bridge.hello(serde_json::from_value(json!({"v":1,"ticket":boot["ticket"],"page_url":"https://channels.weixin.qq.com/web/pages/live","document_id":"doc","adapter_id":"wechat_channels.desktop_probe"})).unwrap()).unwrap();
    let claim = |initial| {
        serde_json::from_value::<LikeClaim>(json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":"https://channels.weixin.qq.com/web/pages/live","initial":initial})).unwrap()
    };
    let token = hello["token"].as_str().unwrap();
    assert_eq!(
        bridge.claim_like_period(token, claim(true)).unwrap()["claimed"],
        true
    );
    assert_eq!(
        bridge.claim_like_period(token, claim(false)).unwrap()["claimed"],
        false
    );
    assert_eq!(
        bridge.claim_like_period(token, claim(true)).unwrap()["claimed"],
        true
    );
}

#[tokio::test]
async fn external_yaml_changes_reach_active_and_new_douyin_rooms() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("douyin.yml");
    let original = source().to_owned()
        + "room_profiles:\n  '123':\n    likes: {interval_ms: 135}\n  '456':\n    likes: {interval_ms: 150}\n";
    std::fs::write(&path, &original).unwrap();
    let config = ModerationConfig::load(&path).unwrap();
    let hub = UserscriptHub::new(dir.path().join("data"), false);
    hub.add_template(
        "dy",
        "account",
        config.probe_options(),
        Arc::new(Mutex::new(AccountBudget::new(Default::default()).unwrap())),
    )
    .unwrap();
    hub.set_template_config_path(path.clone()).unwrap();
    let page = "https://live.douyin.com/123";
    let boot = hub.bootstrap(page).unwrap();
    let hello=hub.dispatch(page,"hello","",json!({"v":1,"ticket":boot["ticket"],"page_url":page,"document_id":"doc","adapter_id":"douyin.live_page"})).unwrap();
    std::fs::write(
        &path,
        original
            .replace("interval_ms: 135", "interval_ms: 300")
            .replace("interval_ms: 150", "interval_ms: 350"),
    )
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let poll=hub.dispatch(page,"poll",hello["token"].as_str().unwrap(),json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":page})).unwrap();
    assert_eq!(poll["likes_config"]["interval_ms"], 300);
    let new = hub.bootstrap("https://live.douyin.com/456").unwrap();
    assert_eq!(new["likes"]["interval_ms"], 350);
    hub.stop().await.unwrap();
}
