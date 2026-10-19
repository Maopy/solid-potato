use live_archive_core::{
    config::{apply, load},
    credentials::{CredentialStore, MemoryCredentialStore, credential_key},
    model::Platform,
    store::TaskStore,
};

#[test]
fn config_imports_sessions_and_idempotently_syncs_rooms() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("live-archive.toml");
    std::fs::write(
        &config_path,
        r#"
[defaults]
output_root = "captures"

[[auth]]
platform = "douyin"
session = "authorized-session"

[[rooms]]
platform = "douyin"
room_url = "https://live.douyin.com/123"
name = "First name"
"#,
    )
    .unwrap();
    let store = TaskStore::open(directory.path().join("tasks.sqlite3")).unwrap();
    let credentials = MemoryCredentialStore::default();

    let first = apply(load(&config_path).unwrap(), &store, &credentials).unwrap();
    assert_eq!(
        (first.sessions_saved, first.rooms_added, first.rooms_updated),
        (1, 1, 0)
    );
    assert_eq!(
        credentials
            .get(&credential_key(Platform::Douyin, "default"))
            .unwrap()
            .as_deref(),
        Some("authorized-session")
    );
    let task = store.list().unwrap().pop().unwrap();
    assert_eq!(task.display_name, "First name");
    assert_eq!(task.output_dir, std::path::PathBuf::from("captures/douyin"));

    std::fs::write(
        &config_path,
        std::fs::read_to_string(&config_path)
            .unwrap()
            .replace("First name", "Renamed"),
    )
    .unwrap();
    let second = apply(load(&config_path).unwrap(), &store, &credentials).unwrap();
    assert_eq!((second.rooms_added, second.rooms_updated), (0, 1));
    assert_eq!(store.list().unwrap().len(), 1);
    assert_eq!(store.list().unwrap()[0].display_name, "Renamed");
}

#[test]
fn wechat_room_without_public_url_uses_a_stable_capture_identifier() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join("live-archive.toml");
    std::fs::write(
        &config_path,
        r#"
[[rooms]]
platform = "wechat_channels"
key = "creator_1"
name = "Creator"
"#,
    )
    .unwrap();
    let store = TaskStore::open(directory.path().join("tasks.sqlite3")).unwrap();
    let summary = apply(
        load(&config_path).unwrap(),
        &store,
        &MemoryCredentialStore::default(),
    )
    .unwrap();
    assert_eq!((summary.rooms_added, summary.rooms_updated), (1, 0));
    let task = store.list().unwrap().pop().unwrap();
    assert_eq!(task.platform, Platform::WeChatChannels);
    assert_eq!(
        task.room_url.as_str(),
        "https://channels.weixin.qq.com/live-archive/creator_1"
    );
}
