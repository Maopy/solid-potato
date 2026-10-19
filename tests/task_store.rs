use chrono::Utc;
use live_archive_core::{
    model::{Platform, RecordingTask, TaskStatus},
    store::TaskStore,
};
use std::path::PathBuf;
use url::Url;
use uuid::Uuid;

#[test]
fn persists_and_updates_a_task() {
    let temporary = tempfile::tempdir().unwrap();
    let store = TaskStore::open(temporary.path().join("tasks.sqlite3")).unwrap();
    let now = Utc::now();
    let task = RecordingTask {
        id: Uuid::new_v4(),
        platform: Platform::Douyin,
        room_url: Url::parse("https://live.douyin.com/123").unwrap(),
        display_name: "creator".into(),
        status: TaskStatus::Idle,
        output_dir: PathBuf::from("recordings/douyin"),
        created_at: now,
        updated_at: now,
    };
    store.insert(&task).unwrap();
    store.update_status(task.id, TaskStatus::Recording).unwrap();

    let saved = store.get(task.id).unwrap().unwrap();
    assert_eq!(saved.platform, Platform::Douyin);
    assert_eq!(saved.status, TaskStatus::Recording);
    assert_eq!(store.list().unwrap().len(), 1);

    let session_id = store.begin_session(task.id, "test live", now).unwrap();
    store
        .finish_session(
            session_id,
            Some(std::path::Path::new("recordings/test.ts")),
            None,
        )
        .unwrap();
    let sessions = store.sessions_for_task(task.id).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].output_path.as_deref(),
        Some(std::path::Path::new("recordings/test.ts"))
    );
}

#[test]
fn persists_wechat_anchor_binding_by_stable_username() {
    let temporary = tempfile::tempdir().unwrap();
    let store = TaskStore::open(temporary.path().join("tasks.sqlite3")).unwrap();
    let now = Utc::now();
    let task = RecordingTask {
        id: Uuid::new_v4(),
        platform: Platform::WeChatChannels,
        room_url: Url::parse("https://channels.weixin.qq.com/live-archive/creator").unwrap(),
        display_name: "主播".into(),
        status: TaskStatus::Idle,
        output_dir: PathBuf::from("recordings/wechat"),
        created_at: now,
        updated_at: now,
    };
    store.insert(&task).unwrap();
    store
        .bind_wechat_anchor(task.id, "stable-anchor-id", "主播")
        .unwrap();

    assert_eq!(
        store.wechat_task_for_anchor("stable-anchor-id").unwrap(),
        Some(task.id)
    );
}
