use live_archive_core::moderation::control::serve_in_process;
use std::time::Duration;

#[tokio::test]
async fn embedded_controller_exposes_dashboard_and_stops_on_request() {
    let temp = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    std::fs::write(temp.path().join("room.yml"), "version: 1\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/139244611014\n").unwrap();
    let path = temp.path().join("control.yml");
    std::fs::write(&path, format!("version: 2\ndashboard: true\ndirectory: data\ninjection:\n  system_proxy: false\n  proxy_port: {}\ndouyin_userscript:\n  enabled: true\n  port: {}\naccounts:\n  - {{id: dy, platform: douyin}}\ntasks:\n  - {{id: room, account_id: dy, config: room.yml}}\n", if port == 2024 { 2023 } else { 2024 }, port)).unwrap();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        serve_in_process(
            &path,
            |url| {
                let _ = ready_tx.send(url);
            },
            async {
                let _ = stop_rx.await;
            },
        )
        .await
    });
    let url = tokio::time::timeout(Duration::from_secs(10), ready_rx)
        .await
        .unwrap()
        .unwrap();
    let (base, token) = url.split_once('#').unwrap();
    let response = reqwest::Client::new()
        .get(format!("{base}api/state"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(reqwest::get(base).await.is_err());
}
