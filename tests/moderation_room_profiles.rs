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
