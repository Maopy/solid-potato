use live_archive_core::{
    download_task_bridge::{DownloadTaskBridge, TaskSnapshot},
    moderation::{
        control::{Controller, LoadedControl},
        dashboard::Dashboard,
    },
};
use serde_json::{Value, json};

#[tokio::test]
async fn manual_message_routes_to_selected_wechat_room_only() {
    use live_archive_core::moderation::protocol::{EventBatch, Hello, Poll};
    let dir = tempfile::tempdir().unwrap();
    let page = "https://channels.weixin.qq.com/web/pages/live";
    std::fs::write(dir.path().join("room.yml"),format!("version: 1\nmode: observe\nroom:\n  platform: wechat_channels\n  adapter: wechat_channels.desktop_probe\n  url: {page}\n")).unwrap();
    std::fs::write(dir.path().join("unified.yml"),"version: 2\nstart_enabled: false\ndirectory: data\naccounts: [{id: main, platform: wechat_channels}]\ntasks: [{id: main, account_id: main, config: room.yml}]\n").unwrap();
    let loaded = LoadedControl::load(&dir.path().join("unified.yml")).unwrap();
    let mut controller = Controller::bind(&loaded).await.unwrap();
    let mut dashboard = Dashboard::bind(&controller, None).await.unwrap();
    let mut url = url::Url::parse(&dashboard.url).unwrap();
    let token = url.fragment().unwrap().to_owned();
    url.set_fragment(None);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut connections = Vec::new();
    let mut sent_action = String::new();
    {
        let mut bridge = controller.tasks["main"].server.bridge.lock().unwrap();
        bridge.configure_ui(false, true);
        let relay = bridge.relay_token.clone();
        for (host, name) in [("host-a", "主播甲"), ("host-b", "主播乙")] {
            let script = bridge.bootstrap_for_room(&relay, Some(host)).unwrap();
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
            let hello=bridge.hello(serde_json::from_value::<Hello>(json!({"v":1,"ticket":boot["ticket"],"page_url":page,"document_id":host,"adapter_id":"wechat_channels.desktop_probe"})).unwrap()).unwrap();
            let events = json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"first_seq":1,"events":[{"seq":1,"event_id":"profile","event_type":"room.profile","source_kind":"diagnostic","payload":{"nickname":name}}]});
            bridge
                .events(
                    hello["token"].as_str().unwrap(),
                    serde_json::from_value::<EventBatch>(events).unwrap(),
                )
                .unwrap();
            connections.push(hello);
        }
    }
    let state: Value = client
        .get(url.join("api/state").unwrap())
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rooms = state["rooms"].as_array().unwrap();
    let a = rooms.iter().find(|r| r["label"] == "主播甲").unwrap()["id"]
        .as_str()
        .unwrap();
    assert!(rooms.iter().any(|r| r["label"] == "主播乙"));
    let request = |path: &str, body: Value| {
        client
            .post(url.join(path).unwrap())
            .bearer_auth(&token)
            .json(&body)
    };
    assert_eq!(
        request("api/send-message", json!({"id":a,"text":"\n"}))
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    assert_eq!(
        request("api/send-message", json!({"id":a,"text":"只给甲的直播间"}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    {
        let mut bridge = controller.tasks["main"].server.bridge.lock().unwrap();
        for (index, hello) in connections.iter().enumerate() {
            let poll:Poll=serde_json::from_value(json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":page,"input_ready":true})).unwrap();
            let result = bridge.poll(hello["token"].as_str().unwrap(), poll).unwrap();
            if index == 0 {
                assert_eq!(result["commands"][0]["text"], "只给甲的直播间");
                assert_eq!(result["commands"][0]["command"], "fill_and_submit");
                sent_action = result["commands"][0]["action_id"]
                    .as_str()
                    .unwrap()
                    .to_owned();
            } else {
                assert_eq!(result["commands"], json!([]));
            }
        }
        let hello = &connections[0];
        bridge.fill_result(hello["token"].as_str().unwrap(),serde_json::from_value(json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":page,"action_id":sent_action,"outcome":"submitted","reason":"input_submitted"})).unwrap()).unwrap();
    }
    let state: Value = client
        .get(url.join("api/state").unwrap())
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        state["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == a)
            .unwrap()["control"]["manual_message"]["state"],
        "submitted"
    );
    dashboard.stop().await;
    controller.stop().await.unwrap();
}

#[tokio::test]
async fn wechat_rooms_have_independent_dashboard_and_page_controls() {
    use live_archive_core::moderation::protocol::{EventBatch, Hello, Poll};
    let dir = tempfile::tempdir().unwrap();
    let page = "https://channels.weixin.qq.com/web/pages/live";
    std::fs::write(dir.path().join("room.yml"), format!("version: 1\nroom:\n  platform: wechat_channels\n  adapter: wechat_channels.desktop_probe\n  url: {page}\n")).unwrap();
    std::fs::write(dir.path().join("unified.yml"), "version: 2\nstart_enabled: false\ndirectory: data\naccounts: [{id: main, platform: wechat_channels}]\ntasks: [{id: main, account_id: main, config: room.yml}]\n").unwrap();
    let loaded = LoadedControl::load(&dir.path().join("unified.yml")).unwrap();
    let mut controller = Controller::bind(&loaded).await.unwrap();
    let mut dashboard = Dashboard::bind(&controller, None).await.unwrap();
    let mut url = url::Url::parse(&dashboard.url).unwrap();
    let token = url.fragment().unwrap().to_owned();
    url.set_fragment(None);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut connections = Vec::new();
    {
        let mut bridge = controller.tasks["main"].server.bridge.lock().unwrap();
        bridge.configure_ui(true, true);
        let relay = bridge.relay_token.clone();
        for (host, name) in [("host-a", "主播甲"), ("host-b", "主播乙")] {
            let script = bridge.bootstrap_for_room(&relay, Some(host)).unwrap();
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
            let hello = bridge.hello(serde_json::from_value::<Hello>(json!({"v":1,"ticket":boot["ticket"],"page_url":page,"document_id":host,"adapter_id":"wechat_channels.desktop_probe"})).unwrap()).unwrap();
            let batch = json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"first_seq":1,"events":[{"seq":1,"event_id":"profile","event_type":"room.profile","source_kind":"diagnostic","payload":{"nickname":name}},{"seq":2,"event_id":"chat","native_event_id":host,"event_type":"chat.received","source_kind":"network","nickname":"观众","payload":{"content":name}}]});
            bridge
                .events(
                    hello["token"].as_str().unwrap(),
                    serde_json::from_value::<EventBatch>(batch).unwrap(),
                )
                .unwrap();
            connections.push(hello);
        }
    }
    let state: Value = client
        .get(url.join("api/state").unwrap())
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rooms = state["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 2);
    let a = rooms.iter().find(|r| r["label"] == "主播甲").unwrap()["id"]
        .as_str()
        .unwrap();
    let b = rooms.iter().find(|r| r["label"] == "主播乙").unwrap()["id"]
        .as_str()
        .unwrap();
    assert_ne!(a, b);
    let request = |path: &str, body: Value| {
        client
            .post(url.join(path).unwrap())
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .body(body.to_string())
    };
    assert_eq!(
        request("api/moderation", json!({"id":a,"enabled":true}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    for (index, expected) in [true, false].into_iter().enumerate() {
        let hello = &connections[index];
        let mut bridge = controller.tasks["main"].server.bridge.lock().unwrap();
        let poll: Poll = serde_json::from_value(json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":page})).unwrap();
        let result = bridge.poll(hello["token"].as_str().unwrap(), poll).unwrap();
        assert_eq!(result["moderation_control"]["enabled"], expected);
    }
    {
        let mut bridge = controller.tasks["main"].server.bridge.lock().unwrap();
        let hello = &connections[1];
        let scope = json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":page});
        bridge
            .control(
                hello["token"].as_str().unwrap(),
                json!({"scope":scope,"enabled":true}),
            )
            .unwrap();
        let hello = &connections[0];
        let scope = json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":page});
        bridge
            .control(
                hello["token"].as_str().unwrap(),
                json!({"scope":scope,"enabled":false}),
            )
            .unwrap();
    }
    let state: Value = client
        .get(url.join("api/state").unwrap())
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rooms = state["rooms"].as_array().unwrap();
    assert_eq!(
        rooms.iter().find(|r| r["id"] == a).unwrap()["control"]["enabled"],
        false
    );
    assert_eq!(
        rooms.iter().find(|r| r["id"] == b).unwrap()["control"]["enabled"],
        true
    );
    let history_a: Value = request("api/timeline", json!({"id":a}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let history_b: Value = request("api/timeline", json!({"id":b}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history_a["entries"][0]["text"], "主播甲");
    assert_eq!(history_b["entries"][0]["text"], "主播乙");
    assert_eq!(
        request(
            "api/room-rules",
            json!({"id":a,"room_id":"host-b","rules":{"welcome_enabled":false}})
        )
        .send()
        .await
        .unwrap()
        .status(),
        422
    );
    assert_eq!(
        request("api/record", json!({"id":b}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    {
        let mut bridge = controller.tasks["main"].server.bridge.lock().unwrap();
        for (index, hello) in connections.iter().enumerate() {
            let poll: Poll = serde_json::from_value(json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":page})).unwrap();
            let result = bridge.poll(hello["token"].as_str().unwrap(), poll).unwrap();
            assert_eq!(result["recording_request"].is_string(), index == 1);
        }
        let relay = bridge.relay_token.clone();
        let script = bridge.bootstrap_for_room(&relay, Some("host-a")).unwrap();
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
        assert_eq!(
            boot["moderation_enabled"], false,
            "refresh retains this room's control"
        );
        let hello = &connections[1];
        let scope: Poll = serde_json::from_value(json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"page_url":page})).unwrap();
        bridge
            .goodbye(hello["token"].as_str().unwrap(), scope)
            .unwrap();
    }
    let state: Value = client
        .get(url.join("api/state").unwrap())
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let rooms = state["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0]["id"], a);
    assert_eq!(rooms[0]["control"]["enabled"], false);
    assert_eq!(
        request("api/timeline", json!({"id":a}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        request("api/timeline", json!({"id":b}))
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    dashboard.stop().await;
    controller.stop().await.unwrap();
}

#[tokio::test]
async fn timeline_is_authenticated_isolated_and_deduplicated_and_speech_can_be_cancelled() {
    use live_archive_core::moderation::protocol::{EventBatch, Hello, Poll};
    let dir = tempfile::tempdir().unwrap();
    for (id, number) in [("one", "123"), ("two", "456")] {
        std::fs::write(dir.path().join(format!("{id}.yml")), format!("version: 1\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/{number}\n")).unwrap();
    }
    std::fs::write(dir.path().join("unified.yml"), "version: 2\nstart_enabled: false\ndirectory: data\naccounts: [{id: main, platform: douyin}]\ntasks: [{id: one, account_id: main, config: one.yml}, {id: two, account_id: main, config: two.yml}]\n").unwrap();
    let loaded = LoadedControl::load(&dir.path().join("unified.yml")).unwrap();
    let mut controller = Controller::bind(&loaded).await.unwrap();
    let mut dashboard = Dashboard::bind(&controller, None).await.unwrap();
    let mut url = url::Url::parse(&dashboard.url).unwrap();
    let token = url.fragment().unwrap().to_owned();
    url.set_fragment(None);
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        client
            .get(url.join("?settings=one").unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let hello = {
        let mut b = controller.tasks["one"].server.bridge.lock().unwrap();
        b.configure_ui(false, true);
        let relay = b.relay_token.clone();
        let boot = b.bootstrap_config(&relay).unwrap();
        let hello = b.hello(serde_json::from_value::<Hello>(json!({"v":1,"ticket":boot["ticket"],"page_url":"https://live.douyin.com/123","document_id":"doc","adapter_id":"douyin.live_page"})).unwrap()).unwrap();
        let batch = json!({"v":1,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"],"first_seq":1,"events":[{"seq":1,"event_id":"first","native_event_id":"message","event_type":"chat.received","source_kind":"network","nickname":"小雨","payload":{"content":"好听"}},{"seq":2,"event_id":"profile","event_type":"room.profile","source_kind":"diagnostic","payload":{"nickname":"主播小怡","ignored_private_field":"never persisted"}}]});
        b.events(
            hello["token"].as_str().unwrap(),
            serde_json::from_value::<EventBatch>(batch.clone()).unwrap(),
        )
        .unwrap();
        b.events(
            hello["token"].as_str().unwrap(),
            serde_json::from_value::<EventBatch>(batch).unwrap(),
        )
        .unwrap();
        hello
    };
    let request = |path: &str, body: Value| {
        client
            .post(url.join(path).unwrap())
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .body(body.to_string())
    };
    assert_eq!(
        client
            .post(url.join("api/timeline").unwrap())
            .header("Content-Type", "application/json")
            .body("{\"id\":\"one\"}")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let response = request("api/timeline", json!({"id":"one"}))
        .send()
        .await
        .unwrap();
    let history: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(history["entries"].as_array().unwrap().len(), 1);
    assert_eq!(history["retention_ms"], 30 * 60 * 1000);
    let settings: Value = request("api/room-settings", json!({"id":"one"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings["rooms"][0]["room_id"], "123");
    assert!(settings["rooms"][0]["rules"]["gift"]["enabled"].is_boolean());
    let changed: Value = request(
        "api/room-rules",
        json!({"id":"one","room_id":"123","rules":{"welcome_enabled":false}}),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(changed["rooms"][0]["rules"]["welcome"]["enabled"], false);
    assert_eq!(request("api/room-rules", json!({"id":"one","room_id":"123","revision":settings["revision"],"rules":{"gift_enabled":false}})).send().await.unwrap().status(), 422);
    let persisted =
        live_archive_core::moderation::config::ModerationConfig::load(&dir.path().join("one.yml"))
            .unwrap();
    assert!(
        !persisted.resolved_room_profiles["123"]
            .rules
            .welcome_enabled
    );
    assert!(
        std::fs::read_to_string(dir.path().join("one.yml"))
            .unwrap()
            .contains("live.douyin.com/123")
    );
    assert_eq!(
        request(
            "api/room-rules",
            json!({"id":"one","room_id":"123","rules":{"gift_enabled":false}})
        )
        .send()
        .await
        .unwrap()
        .status(),
        200
    );
    let config_before = std::fs::read(dir.path().join("one.yml")).unwrap();
    let persisted =
        live_archive_core::moderation::config::ModerationConfig::load(&dir.path().join("one.yml"))
            .unwrap();
    assert!(!persisted.resolved_room_profiles["123"].rules.gift_enabled);
    assert!(
        !persisted.resolved_room_profiles["123"]
            .rules
            .welcome_enabled
    );
    assert_eq!(
        request(
            "api/room-rules",
            json!({"id":"one","room_id":"456","rules":{"welcome_enabled":true}})
        )
        .send()
        .await
        .unwrap()
        .status(),
        422
    );
    assert_eq!(
        request(
            "api/room-rules",
            json!({"id":"one","room_id":"123","rules":{"welcome_template":"{invalid}"}})
        )
        .send()
        .await
        .unwrap()
        .status(),
        422
    );
    assert_eq!(
        std::fs::read(dir.path().join("one.yml")).unwrap(),
        config_before
    );
    assert!(!dir.path().join("one.room-overrides.json").exists());
    let unchanged: Value = request("api/room-settings", json!({"id":"one"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unchanged["rooms"][0]["rules"]["welcome"]["enabled"], false);
    let other_settings: Value = request("api/room-settings", json!({"id":"two"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(other_settings["rooms"], json!([]));
    assert!(history.get("limit").is_none());
    assert_eq!(
        request(
            "api/reply-suggestions",
            json!({"id":"two","entry_id":"chat::message"})
        )
        .send()
        .await
        .unwrap()
        .status(),
        422,
        "a different room cannot request this message's context"
    );
    let response = client
        .get(url.join("api/state").unwrap())
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    let names: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    let first = names["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|room| room["id"] == "one")
        .unwrap();
    assert_eq!(first["anchor_name"], "主播小怡");
    assert_eq!(first["label"], "主播小怡");

    assert_eq!(history["entries"][0]["text"], "好听");
    let response = request("api/timeline", json!({"id":"two"}))
        .send()
        .await
        .unwrap();
    let other: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert!(other["entries"].as_array().unwrap().is_empty());
    assert_eq!(
        request("api/timeline", json!({"id":"missing"}))
            .send()
            .await
            .unwrap()
            .status(),
        422
    );
    let response = request("api/speech", json!({"id":"one","enabled":true}))
        .send()
        .await
        .unwrap();
    let control: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(control["speech_pending"], true);
    let scope = json!({"v":1,"page_url":"https://live.douyin.com/123","connection_id":hello["connection_id"],"binding_id":hello["binding_id"]});
    let pending = {
        let mut b = controller.tasks["one"].server.bridge.lock().unwrap();
        b.poll(
            hello["token"].as_str().unwrap(),
            serde_json::from_value::<Poll>(scope.clone()).unwrap(),
        )
        .unwrap()["speech_request"]
            .clone()
    };
    assert!(pending.is_string());
    assert_eq!(
        request("api/speech", json!({"id":"one","enabled":false}))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    {
        let mut b = controller.tasks["one"].server.bridge.lock().unwrap();
        assert!(b.speech(hello["token"].as_str().unwrap(), json!({"scope":scope,"action":"start","request_id":pending,"stream_url":"https://media.test/audio"})).is_err());
        assert_eq!(
            b.timeline()["entries"].as_array().unwrap().len(),
            1,
            "stopping keeps history"
        );
    }
    dashboard.stop().await;
    controller.stop().await.unwrap();
}
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
