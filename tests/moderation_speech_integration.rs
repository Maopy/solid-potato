use live_archive_core::moderation::{
    bridge::ProbeBridge,
    journal::ProbeJournal,
    protocol::{Hello, ProbeScope},
};
use serde_json::json;
#[tokio::test]
async fn speech_requires_page_auth_is_separate_from_recording_and_leaves_no_history() {
    for platform in ["douyin", "wechat"] {
        let dir = tempfile::tempdir().unwrap();
        let scope = if platform == "douyin" {
            ProbeScope::douyin("https://live.douyin.com/123".parse().unwrap()).unwrap()
        } else {
            ProbeScope::wechat_desktop(
                "https://channels.weixin.qq.com/web/pages/live"
                    .parse()
                    .unwrap(),
            )
            .unwrap()
        };
        let page = scope.room_url.to_string();
        let adapter = scope.adapter().to_owned();
        let mut b = ProbeBridge::new(
            scope,
            ProbeJournal::open(&dir.path().join("test.sqlite3")).unwrap(),
        );
        b.configure_ui(false, true);
        b.set_moderation_enabled(false).unwrap();
        let boot = b.bootstrap_config(&b.relay_token.clone()).unwrap();
        assert_eq!(boot["speech_recognition"], true);
        let hello=b.hello(serde_json::from_value::<Hello>(json!({"v":1,"ticket":boot["ticket"],"page_url":page,"document_id":"doc","adapter_id":adapter})).unwrap()).unwrap();
        let mut request = json!({"scope":{"v":1,"page_url":page,"connection_id":hello["connection_id"],"binding_id":hello["binding_id"]},"action":"status"});
        assert!(b.speech("wrong", request.clone()).is_err());
        let token = hello["token"].as_str().unwrap();
        assert_eq!(
            b.speech(token, request.clone()).unwrap()["recent"],
            json!([])
        );
        let mut wrong = request.clone();
        wrong["scope"]["page_url"] = json!("https://live.douyin.com/456");
        assert!(b.speech(token, wrong).is_err());
        request["action"] = json!("start");
        request["stream_url"] = json!("file:///private/audio");
        assert!(b.speech(token, request.clone()).is_err());
        request["action"] = json!("stop");
        assert_eq!(b.speech(token, request.clone()).unwrap()["enabled"], false);
        b.begin_drain();
        assert!(b.speech(token, request).is_err());
    }
}
