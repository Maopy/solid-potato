use live_archive_core::moderation::{
    bridge::ProbeBridge,
    http::ProbeServer,
    journal::ProbeJournal,
    protocol::{EventBatch, Hello, Poll, ProbeEvent, ProbeScope, VERSION, sanitize},
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn make_bridge() -> (TempDir, ProbeBridge) {
    let dir = tempfile::tempdir().unwrap();
    let journal = ProbeJournal::open(&dir.path().join("probe.sqlite3")).unwrap();
    let scope =
        ProbeScope::douyin("https://live.douyin.com/992927935592".parse().unwrap()).unwrap();
    (dir, ProbeBridge::new(scope, journal))
}

fn pair(bridge: &mut ProbeBridge) -> Value {
    let script = bridge.bootstrap(&bridge.relay_token.clone()).unwrap();
    let config = script
        .split("const BOOT = ")
        .nth(1)
        .unwrap()
        .split(";\n")
        .next()
        .unwrap();
    let config: Value = serde_json::from_str(config).unwrap();
    bridge
        .hello(Hello {
            v: VERSION,
            ticket: config["ticket"].as_str().unwrap().into(),
            page_url: bridge.scope.room_url.to_string(),
            document_id: "document-1".into(),
            adapter_id: "douyin.live_page".into(),
        })
        .unwrap()
}

fn event(seq: u64) -> ProbeEvent {
    ProbeEvent {
        seq,
        event_id: format!("document-1:{seq}"),
        event_type: "chat.received".into(),
        source_kind: "dom".into(),
        native_event_id: None,
        user_id: None,
        nickname: Some("观众".into()),
        payload: json!({"content":"你好","cookie":"must_not_persist"}),
    }
}

fn batch(session: &Value, first_seq: u64, events: Vec<ProbeEvent>) -> EventBatch {
    EventBatch {
        v: VERSION,
        connection_id: session["connection_id"].as_str().unwrap().into(),
        binding_id: session["binding_id"].as_str().unwrap().into(),
        first_seq,
        events,
    }
}

fn poll(session: &Value, url: &str) -> Poll {
    Poll {
        v: VERSION,
        connection_id: session["connection_id"].as_str().unwrap().into(),
        binding_id: session["binding_id"].as_str().unwrap().into(),
        page_url: url.into(),
        input_ready: None,
    }
}

#[test]
fn scope_strips_tracking_but_rejects_other_rooms_hosts_and_credentials() {
    let scope = ProbeScope::douyin(
        "https://live.douyin.com/123/?tracking=secret#fragment"
            .parse()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(scope.room_url.as_str(), "https://live.douyin.com/123");
    assert!(scope.matches("https://live.douyin.com/123?anything=x"));
    for url in [
        "https://live.douyin.com/124",
        "https://other.example/123",
        "https://user@live.douyin.com/123",
        "https://live.douyin.com/123/456",
        "http://live.douyin.com/123",
    ] {
        assert!(!scope.matches(url), "{url}");
    }
}

#[test]
fn pairing_ticket_is_single_use_and_probe_has_no_send_commands() {
    let (_dir, mut bridge) = make_bridge();
    assert!(bridge.bootstrap("wrong").is_err());
    let script = bridge.bootstrap(&bridge.relay_token.clone()).unwrap();
    let config: Value = serde_json::from_str(
        script
            .split("const BOOT = ")
            .nth(1)
            .unwrap()
            .split(";\n")
            .next()
            .unwrap(),
    )
    .unwrap();
    let hello = || Hello {
        v: 1,
        ticket: config["ticket"].as_str().unwrap().into(),
        page_url: "https://live.douyin.com/992927935592".into(),
        document_id: "document-1".into(),
        adapter_id: "douyin.live_page".into(),
    };
    let session = bridge.hello(hello()).unwrap();
    assert!(bridge.hello(hello()).is_err());
    let result = bridge
        .poll(
            session["token"].as_str().unwrap(),
            poll(&session, "https://live.douyin.com/992927935592"),
        )
        .unwrap();
    assert_eq!(result["send_chat"], false);
    assert_eq!(result["commands"], json!([]));
}

#[test]
fn retries_are_idempotent_gaps_rejected_and_ack_follows_durable_write() {
    let (dir, mut bridge) = make_bridge();
    let session = pair(&mut bridge);
    let token = session["token"].as_str().unwrap();
    assert!(
        bridge
            .events(token, batch(&session, 2, vec![event(2)]))
            .is_err()
    );
    assert_eq!(
        bridge
            .events(token, batch(&session, 1, vec![event(1)]))
            .unwrap()["acked_seq"],
        1
    );
    assert_eq!(
        bridge
            .events(token, batch(&session, 1, vec![event(1)]))
            .unwrap()["acked_seq"],
        1
    );
    assert_eq!(bridge.total_events, 1);
    let mut changed = event(1);
    changed.payload = json!({"content":"changed"});
    assert!(
        bridge
            .events(token, batch(&session, 1, vec![changed]))
            .is_err()
    );
    let connection = rusqlite::Connection::open(dir.path().join("probe.sqlite3")).unwrap();
    let text: String = connection
        .query_row("SELECT event_json FROM probe_events", [], |r| r.get(0))
        .unwrap();
    assert!(!text.contains("must_not_persist"));
    assert!(!text.contains("cookie"));
    assert!(text.contains("你好"));
}

#[test]
fn multiple_pages_are_independent_and_identity_change_revokes_only_its_binding() {
    let (_dir, mut bridge) = make_bridge();
    let a = pair(&mut bridge);
    let b = pair(&mut bridge);
    let room = "https://live.douyin.com/992927935592";
    assert!(
        bridge
            .poll("wrong", poll(&a, "https://live.douyin.com/123"))
            .is_err()
    );
    assert!(
        bridge
            .poll(a["token"].as_str().unwrap(), poll(&a, room))
            .is_ok()
    );
    assert!(
        bridge
            .poll(
                a["token"].as_str().unwrap(),
                poll(&a, "https://live.douyin.com/123")
            )
            .is_err()
    );
    assert!(
        bridge
            .poll(a["token"].as_str().unwrap(), poll(&a, room))
            .is_err()
    );
    assert!(
        bridge
            .poll(b["token"].as_str().unwrap(), poll(&b, room))
            .is_ok()
    );
}

#[test]
fn extensible_unknown_types_do_not_store_arbitrary_payloads() {
    let (_dir, mut bridge) = make_bridge();
    let session = pair(&mut bridge);
    let mut unknown = event(1);
    unknown.event_type = "future.new_message".into();
    unknown.payload = json!({"authorization":"secret","deep":{"cookie":"secret"}});
    assert_eq!(sanitize(&unknown).payload, json!({}));
    assert!(
        bridge
            .events(
                session["token"].as_str().unwrap(),
                batch(&session, 1, vec![unknown])
            )
            .is_ok()
    );
    assert_eq!(bridge.counts["future.new_message"], 1);
    let mut large = event(2);
    large.payload = json!({"content":"x".repeat(2049)});
    assert!(
        bridge
            .events(
                session["token"].as_str().unwrap(),
                batch(&session, 2, vec![large])
            )
            .is_err()
    );
    assert_eq!(bridge.total_events, 1);
}

async fn http(address: std::net::SocketAddr, raw: String) -> String {
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(raw.as_bytes()).await.unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn http_requires_host_origin_and_strict_framing_and_stops_listener() {
    let (_dir, bridge) = make_bridge();
    let mut server = ProbeServer::bind(bridge).await.unwrap();
    let address = server.address;
    let body = "{}";
    let request = |headers: &str| {
        format!(
            "POST /v1/hello HTTP/1.1\r\nHost: {address}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{headers}\r\n{body}",
            body.len()
        )
    };
    assert!(http(address, request("")).await.starts_with("HTTP/1.1 403"));
    assert!(
        http(address, request("Origin: https://live.douyin.com\r\n"))
            .await
            .starts_with("HTTP/1.1 422")
    );
    assert!(
        http(address, request("Transfer-Encoding: chunked\r\n"))
            .await
            .starts_with("HTTP/1.1 400")
    );
    assert!(
        http(address, request("Content-Length: 2\r\n"))
            .await
            .starts_with("HTTP/1.1 400")
    );
    let wrong_host = request("").replace(&format!("Host: {address}"), "Host: live.douyin.com");
    assert!(http(address, wrong_host).await.starts_with("HTTP/1.1 400"));
    server.stop().await.unwrap();
    assert!(tokio::net::TcpStream::connect(address).await.is_err());
}
