use live_archive_core::{
    download_task_bridge::{DownloadTaskBridge, TaskSnapshot},
    moderation::{
        control::{Controller, LoadedControl},
        dashboard::Dashboard,
    },
};
use serde_json::{Value, json};
#[tokio::test]
async fn unified_dashboard_controls_rooms_and_downloads_with_per_run_auth() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::write(p.join("room.yml"),"version: 1\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/123\n").unwrap();
    std::fs::write(p.join("unified.yml"),"version: 2\nstart_enabled: false\ndirectory: data\naccounts: [{id: main, platform: douyin}]\ntasks: [{id: main, account_id: main, config: room.yml}]\n").unwrap();
    let loaded = LoadedControl::load(&p.join("unified.yml")).unwrap();
    let mut controller = Controller::bind(&loaded).await.unwrap();
    let downloads = DownloadTaskBridge::default();
    downloads
        .publish(TaskSnapshot {
            id: "video".into(),
            status: "downloading".into(),
            name: "本地任务".into(),
            error: None,
        })
        .await;
    let mut dashboard = Dashboard::bind(&controller, Some(downloads.clone()))
        .await
        .unwrap();
    let mut url = url::Url::parse(&dashboard.url).unwrap();
    let token = url.fragment().unwrap().to_string();
    url.set_fragment(None);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        client
            .get(url.join("api/state").unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let state: Value = serde_json::from_str(
        &client
            .get(url.join("api/state").unwrap())
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(state["rooms"][0]["control"]["enabled"], false);
    let r = client
        .post(url.join("api/moderation").unwrap())
        .bearer_auth(&token)
        .header("Origin", url.origin().ascii_serialization())
        .header("Content-Type", "application/json")
        .body(json!({"id":"main","enabled":true}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        controller.tasks["main"]
            .server
            .bridge
            .lock()
            .unwrap()
            .moderation_enabled
    );
    let bad = client
        .post(url.join("api/moderation").unwrap())
        .bearer_auth(&token)
        .header("Origin", "https://evil.test")
        .header("Content-Type", "application/json")
        .body(json!({"id":"main","enabled":false}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 403);
    let r = client
        .post(url.join("api/download").unwrap())
        .bearer_auth(&token)
        .header("Content-Type", "application/json")
        .body(json!({"id":"video","action":"pause"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(downloads.detail("video").await.unwrap().status, "paused");
    let r = client
        .post(url.join("api/record").unwrap())
        .bearer_auth(&token)
        .header("Content-Type", "application/json")
        .body(json!({"id":"main"}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422, "record button explains disconnected room");
    dashboard.stop().await;
    controller.stop().await.unwrap();
}
