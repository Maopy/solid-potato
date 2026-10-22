use live_archive_core::{
    capture_service::{CaptureEvent, CaptureIngestor, CapturePermit, LoopbackCaptureServer},
    download_task_bridge::DownloadTaskBridge,
    model::{Platform, RecordingTask, TaskStatus},
    platform::StreamResolver,
    wechat::{WeChatCaptureStore, WeChatChannelsResolver},
};
use std::{
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    sync::Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use url::Url;

fn event(page: &str, stream: &str) -> CaptureEvent {
    CaptureEvent {
        page_url: Url::parse(page).unwrap(),
        stream_url: Url::parse(stream).unwrap(),
        title: "live".into(),
        anchor_username: None,
        frontend_task_id: None,
    }
}

#[test]
fn capture_ingestor_requires_loopback_permit_and_allowlisted_urls() {
    let permit = CapturePermit::generate();
    let ingestor = CaptureIngestor::new(Arc::new(WeChatCaptureStore::default()), permit.clone());
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    assert!(
        ingestor
            .ingest(
                loopback,
                "wrong",
                event(
                    "https://channels.weixin.qq.com/page/1",
                    "https://media.example.test/live.m3u8"
                )
            )
            .is_err()
    );
    assert!(
        ingestor
            .ingest(
                loopback,
                permit.as_bearer(),
                event(
                    "https://example.test/page/1",
                    "https://media.example.test/live.m3u8"
                )
            )
            .is_err()
    );
    assert!(
        ingestor
            .ingest(
                loopback,
                permit.as_bearer(),
                event(
                    "https://channels.weixin.qq.com/page/1",
                    "http://media.example.test/live.m3u8"
                )
            )
            .is_err()
    );
    assert!(
        ingestor
            .ingest(
                loopback,
                permit.as_bearer(),
                event(
                    "https://channels.weixin.qq.com/page/1",
                    "https://media.example.test/live.m3u8"
                )
            )
            .is_ok()
    );
}

#[tokio::test]
async fn loopback_endpoint_publishes_capture_for_wechat_resolver() {
    let captures = Arc::new(WeChatCaptureStore::default());
    let permit = CapturePermit::generate();
    let server = LoopbackCaptureServer::bind(Arc::new(CaptureIngestor::new(
        Arc::clone(&captures),
        permit.clone(),
    )))
    .await
    .unwrap();
    let body = r#"{"page_url":"https://channels.weixin.qq.com/page/1","stream_url":"https://media.example.test/live.m3u8","title":"captured live"}"#;
    let request = format!(
        "POST /capture?permit={} HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
        permit.as_bearer(),
        body.len(),
        body
    );
    let mut socket = TcpStream::connect(server.address).await.unwrap();
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    assert!(
        String::from_utf8(response)
            .unwrap()
            .starts_with("HTTP/1.1 202")
    );

    let now = chrono::Utc::now();
    let task = RecordingTask {
        id: uuid::Uuid::new_v4(),
        platform: Platform::WeChatChannels,
        room_url: Url::parse("https://channels.weixin.qq.com/page/1").unwrap(),
        display_name: "channel".into(),
        status: TaskStatus::Idle,
        output_dir: PathBuf::from("recordings"),
        created_at: now,
        updated_at: now,
    };
    assert_eq!(
        WeChatChannelsResolver::new(captures)
            .resolve(&task)
            .await
            .unwrap()
            .title,
        "captured live"
    );
    server.stop().unwrap();
}

#[tokio::test]
async fn reference_frontend_create_endpoint_publishes_a_live_capture() {
    let captures = Arc::new(WeChatCaptureStore::default());
    let permit = CapturePermit::generate();
    let server = LoopbackCaptureServer::bind(Arc::new(CaptureIngestor::new(
        Arc::clone(&captures),
        permit.clone(),
    )))
    .await
    .unwrap();
    // The upstream Channels frontend can expose its signed live CDN as HTTP.
    // That format is accepted only by this reference-task route; `/capture`
    // continues to require HTTPS.
    let body = r#"{"objects":[{"platform":"wxchannels","content":{"profile":{"contact":{"username":"anchor","nickname":"主播"},"id":"object","objectNonceId":"nonce"},"live":{"liveSdkInfo":{"liveCdnUrl":"http://media.example.test/live.flv"},"recording_streams":[{"url":"http://media.example.test/origin.flv","tag_name":"原画","rate":4000},{"url":"https://media.example.test/hd.flv","tag_name":"高清","rate":1000}]}}}]}"#;
    let request = format!(
        "POST /api/v1/download_task/create?permit={} HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
        permit.as_bearer(),
        body.len(),
        body
    );
    let mut socket = TcpStream::connect(server.address).await.unwrap();
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("\"tasks\""));
    assert_eq!(captures.observed().len(), 1);
    assert_eq!(
        captures.observed()[0].stream_url.as_str(),
        "http://media.example.test/origin.flv"
    );
    assert_eq!(
        captures.observed()[0].anchor_username.as_deref(),
        Some("anchor")
    );
    server.stop().unwrap();
}

#[tokio::test]
async fn reference_task_api_lists_and_updates_the_created_task() {
    let permit = CapturePermit::generate();
    let tasks = DownloadTaskBridge::default();
    let server = LoopbackCaptureServer::bind(Arc::new(
        CaptureIngestor::new(Arc::new(WeChatCaptureStore::default()), permit.clone())
            .with_task_bridge(tasks),
    ))
    .await
    .unwrap();
    let create_body = r#"{"objects":[{"platform":"wxchannels","content":{"profile":{"contact":{"username":"anchor","nickname":"主播"}},"live":{"liveSdkInfo":{"liveCdnUrl":"https://media.example.test/live.flv"}}}}]}"#;
    let create = format!(
        "POST /api/v1/download_task/create?permit={} HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
        permit.as_bearer(),
        create_body.len(),
        create_body
    );
    let mut socket = TcpStream::connect(server.address).await.unwrap();
    socket.write_all(create.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    let body = response.split_once("\r\n\r\n").unwrap().1;
    let id =
        serde_json::from_str::<serde_json::Value>(body).unwrap()["data"]["tasks"][0]["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

    let list = format!(
        "GET /api/v1/download_task/list?permit={} HTTP/1.1\r\nContent-Length: 0\r\n\r\n",
        permit.as_bearer()
    );
    let mut socket = TcpStream::connect(server.address).await.unwrap();
    socket.write_all(list.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.contains(&format!(r#""id":"{id}""#)));
    assert!(response.contains(r#""status":"waiting""#));

    let pause_body = format!(r#"{{"task_ids":["{id}"]}}"#);
    let pause = format!(
        "POST /api/v1/download_task/pause?permit={} HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
        permit.as_bearer(),
        pause_body.len(),
        pause_body
    );
    let mut socket = TcpStream::connect(server.address).await.unwrap();
    socket.write_all(pause.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    socket.read_to_end(&mut response).await.unwrap();
    assert!(
        String::from_utf8(response)
            .unwrap()
            .contains(r#""status":"paused""#)
    );
    server.stop().unwrap();
}
