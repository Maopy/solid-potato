use live_archive_core::moderation::{
    bridge::ProbeBridge,
    candidate_preview::CandidatePreviewConfig,
    journal::ProbeJournal,
    protocol::{Hello, Poll, ProbeScope},
};
use serde_json::Value;

#[test]
fn automatic_page_target_is_explicit_and_rejects_invalid_manual_ids() {
    for target in [None, Some("")] {
        let mut config = CandidatePreviewConfig {
            enabled: true,
            recipient_user_id: target.map(str::to_owned),
            recipient_from_page: false,
        };
        assert!(config.validate("wechat_channels", true).is_err());
        config.recipient_from_page = true;
        assert!(config.validate("wechat_channels", true).is_ok());
        assert!(config.validate("douyin", true).is_err());
        assert!(config.validate("wechat_channels", false).is_err());
        config.recipient_user_id = Some("bad\nidentity".into());
        assert!(config.validate("wechat_channels", true).is_err());
    }
}

#[test]
fn unrecognized_pages_can_pair_without_thanks_and_real_targets_remain_per_page() {
    let dir = tempfile::tempdir().unwrap();
    let page = "https://channels.weixin.qq.com/web/pages/live";
    let mut bridge = ProbeBridge::new(
        ProbeScope::wechat_desktop(page.parse().unwrap()).unwrap(),
        ProbeJournal::open(&dir.path().join("probe.sqlite3")).unwrap(),
    );
    bridge
        .configure_candidate_preview(CandidatePreviewConfig {
            enabled: true,
            recipient_user_id: Some(String::new()),
            recipient_from_page: true,
        })
        .unwrap();
    bridge.enable_candidate_fill_preview(None).unwrap();
    assert!(bridge.waits_for_room_identity());

    for (identity, enabled) in [
        (None, false),
        (Some("bad\nidentity"), false),
        (Some("actual-host-a"), true),
        (Some("actual-host-b"), true),
    ] {
        let script = bridge
            .bootstrap_for_room(&bridge.relay_token.clone(), identity)
            .unwrap();
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
        assert_eq!(boot["recipient_pending"], !enabled);
        let hello = bridge
            .hello(Hello {
                v: 1,
                ticket: boot["ticket"].as_str().unwrap().into(),
                page_url: page.into(),
                document_id: uuid::Uuid::new_v4().to_string(),
                adapter_id: "wechat_channels.desktop_probe".into(),
                client_id: None,
            })
            .unwrap();
        let state = bridge
            .poll(
                hello["token"].as_str().unwrap(),
                Poll {
                    v: 1,
                    connection_id: hello["connection_id"].as_str().unwrap().into(),
                    binding_id: hello["binding_id"].as_str().unwrap().into(),
                    page_url: page.into(),
                    input_ready: Some(true),
                },
            )
            .unwrap();
        assert_eq!(state["candidate_preview"]["enabled"], enabled);
        assert_eq!(state["commands"], serde_json::json!([]));
        if enabled {
            assert_eq!(
                state["candidate_preview"]["recipient_user_id"],
                identity.unwrap()
            );
        }
    }
}
