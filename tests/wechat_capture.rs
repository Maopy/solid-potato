use chrono::{Duration, Utc};
use live_archive_core::{
    model::{Platform, RecordingTask, TaskStatus},
    platform::{ResolveError, StreamResolver},
    wechat::{CapturedWeChatStream, WeChatCaptureStore, WeChatChannelsResolver},
};
use std::{path::PathBuf, sync::Arc};
use url::Url;
use uuid::Uuid;

fn task() -> RecordingTask {
    let now = Utc::now();
    RecordingTask {
        id: Uuid::new_v4(),
        platform: Platform::WeChatChannels,
        room_url: Url::parse("https://channels.weixin.qq.com/page/123").unwrap(),
        display_name: "channel".into(),
        status: TaskStatus::Idle,
        output_dir: PathBuf::from("recordings"),
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn resolver_uses_fresh_proxy_capture_for_matching_page() {
    let store = Arc::new(WeChatCaptureStore::default());
    store
        .publish(CapturedWeChatStream {
            page_url: task().room_url,
            stream_url: Url::parse("https://media.example.test/live.m3u8").unwrap(),
            title: "channel live".into(),
            anchor_username: None,
            frontend_task_id: None,
            captured_at: Utc::now(),
        })
        .unwrap();
    let resolved = WeChatChannelsResolver::new(store)
        .resolve(&task())
        .await
        .unwrap();
    assert_eq!(resolved.title, "channel live");
}

#[tokio::test]
async fn resolver_rejects_expired_or_missing_capture() {
    let store = Arc::new(WeChatCaptureStore::default());
    let resolver = WeChatChannelsResolver::with_max_age(Arc::clone(&store), Duration::seconds(1));
    assert!(matches!(
        resolver.resolve(&task()).await,
        Err(ResolveError::AuthenticationRequired)
    ));
    store
        .publish(CapturedWeChatStream {
            page_url: task().room_url,
            stream_url: Url::parse("https://media.example.test/live.m3u8").unwrap(),
            title: "channel live".into(),
            anchor_username: None,
            frontend_task_id: None,
            captured_at: Utc::now() - Duration::seconds(2),
        })
        .unwrap();
    assert!(matches!(
        resolver.resolve(&task()).await,
        Err(ResolveError::Offline)
    ));
}
