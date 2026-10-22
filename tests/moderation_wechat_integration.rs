//! Desktop injection bootstrap -> SDK -> local bridge -> redacted diagnostics journal.
use live_archive_core::moderation::{
    bridge::ProbeBridge,
    config::ModerationConfig,
    http::ProbeServer,
    journal::ProbeJournal,
    protocol::{Hello, ProbeScope, VERSION},
};
use std::{path::Path, process::Command};

#[test]
fn desktop_scope_adapter_and_config_gate_actions_before_startup() {
    let scope = ProbeScope::wechat_desktop(
        "https://channels.weixin.qq.com/web/pages/live?secret=x"
            .parse()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        scope.room_url.as_str(),
        "https://channels.weixin.qq.com/web/pages/live"
    );
    assert!(scope.matches("https://channels.weixin.qq.com/web/pages/live?other=y"));
    for url in [
        "https://channels.weixin.qq.com/",
        "https://channels.weixin.qq.com/web/pages/home",
        "http://channels.weixin.qq.com/web/pages/live",
        "https://user@channels.weixin.qq.com/web/pages/live",
        "https://live.douyin.com/123",
    ] {
        assert!(!scope.matches(url));
    }
    let dir = tempfile::tempdir().unwrap();
    let example = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("moderation.wechat.example.yml"),
    )
    .unwrap();
    let path = dir.path().join("config.yml");
    for mode in ["rules_preview", "scheduler_preview", "fill_only", "execute"] {
        std::fs::write(
            &path,
            example.replace("mode: observe", &format!("mode: {mode}")),
        )
        .unwrap();
        assert!(ModerationConfig::load(&path).is_err());
    }
    std::fs::write(&path, &example).unwrap();
    assert!(ModerationConfig::load(&path).is_ok());
    let mut bridge = ProbeBridge::new(
        scope,
        ProbeJournal::open(&dir.path().join("probe.sqlite3")).unwrap(),
    );
    let script = bridge.bootstrap(&bridge.relay_token.clone()).unwrap();
    let boot: serde_json::Value = serde_json::from_str(
        script
            .split("const BOOT = ")
            .nth(1)
            .unwrap()
            .split(";\n")
            .next()
            .unwrap(),
    )
    .unwrap();
    let hello = |adapter: &str| Hello {
        client_id: None,
        v: VERSION,
        ticket: boot["ticket"].as_str().unwrap().into(),
        page_url: bridge.scope.room_url.to_string(),
        document_id: "desktop-doc".into(),
        adapter_id: adapter.into(),
    };
    let wrong = hello("douyin.live_page");
    let correct = hello("wechat_channels.desktop_probe");
    assert!(bridge.hello(wrong).is_err());
    assert!(bridge.hello(correct).is_ok());
    let mut guarded = ProbeBridge::new(
        bridge.scope.clone(),
        ProbeJournal::open(&dir.path().join("guard.sqlite3")).unwrap(),
    );
    guarded.enable_rules_preview();
    assert!(guarded.bootstrap(&guarded.relay_token.clone()).is_err());
}

#[test]
fn desktop_tls_only_config_disables_injection_and_rejects_ambiguous_transport_modes() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("moderation.wechat.tls-only.example.yml");
    let mut config = ModerationConfig::load(&path).unwrap();
    assert!(config.probe_options().tls_only);
    assert!(config.probe_options().system_proxy);
    assert!(!config.probe_options().tls_passthrough);
    assert_eq!(config.duration_seconds, 60);
    let scope = ProbeScope::from_room_url(config.room.url.clone()).unwrap();
    let source = live_archive_core::moderation::inject::render_addon_with_injection(
        &scope,
        "127.0.0.1:1".parse().unwrap(),
        "synthetic",
        false,
    );
    // Validate the configuration consumed by the actual Python addon.
    let result = Command::new("python3").args(["-c",
        "import ast,json,sys; line=next(l for l in sys.stdin if l.startswith('CONFIG =')); arg=ast.parse(line).body[0].value.args[0]; print(json.loads(ast.literal_eval(arg))['inject_page'])"])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    use std::io::Write;
    let mut result = result;
    result
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let output = result.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "False");
    config.injection.tls_passthrough = true;
    assert!(config.validate().is_err());
}

#[tokio::test]
async fn http1_yaml_reaches_real_mitmdump_runtime_without_system_proxy_changes() {
    let config = ModerationConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("moderation.wechat.http1.example.yml"),
    )
    .unwrap();
    assert_eq!(config.probe_options().http2, Some(false));
    assert!(config.probe_options().tls_only);
    let default_config = ModerationConfig::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("moderation.wechat.tls-only.example.yml"),
    )
    .unwrap();
    assert_eq!(default_config.probe_options().http2, Some(true));
    let directory = tempfile::tempdir().unwrap();
    let runtime = directory.path().join("runtime.json");
    let scope = ProbeScope::from_room_url(config.room.url).unwrap();
    let source = live_archive_core::moderation::inject::render_addon_with_injection(
        &scope,
        "127.0.0.1:1".parse().unwrap(),
        "synthetic",
        false,
    );
    let source = format!(
        "{source}\nfrom mitmproxy import ctx\ndef running():\n    with open({}, 'w') as output:\n        json.dump({{'http2': ctx.options.http2}}, output)\n",
        serde_json::to_string(&runtime).unwrap()
    );
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let mut sidecar = live_archive_core::moderation::inject::ProbeSidecar::start(
        &config.injection.mitmdump,
        directory.path(),
        port,
        &source,
        &scope,
        false,
        config.injection.http2,
    )
    .await
    .unwrap();
    for _ in 0..40 {
        if runtime.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let runtime_result = std::fs::read_to_string(runtime);
    sidecar.stop().await.unwrap();
    let runtime: serde_json::Value = serde_json::from_str(&runtime_result.unwrap()).unwrap();
    assert_eq!(runtime["http2"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_sdk_candidates_and_redacted_diagnostics_never_send() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("probe.sqlite3");
    let scope = ProbeScope::wechat_desktop(
        "https://channels.weixin.qq.com/web/pages/live"
            .parse()
            .unwrap(),
    )
    .unwrap();
    let mut bridge = ProbeBridge::new(scope, ProbeJournal::open(&database).unwrap());
    bridge
        .configure_candidate_preview(
            live_archive_core::moderation::candidate_preview::CandidatePreviewConfig {
                enabled: true,
                recipient_user_id: Some("fixture-recipient".into()),
            },
        )
        .unwrap();
    assert_eq!(
        bridge.summary()["network_decoding"],
        "wechat.msg_list.type1.candidate.v1"
    );
    let bootstrap = directory.path().join("bootstrap.js");
    std::fs::write(
        &bootstrap,
        bridge.bootstrap(&bridge.relay_token.clone()).unwrap(),
    )
    .unwrap();
    let mut server = ProbeServer::bind(bridge).await.unwrap();
    let address = server.address.to_string();
    let denied = reqwest::Client::new()
        .post(format!("http://{address}/v1/hello"))
        .header("Origin", "https://live.douyin.com")
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    let driver =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/moderation_wechat_driver.cjs");
    let drain_ready = directory.path().join("drain-ready");
    let driver_ready = drain_ready.clone();
    let mut driver_task = tokio::task::spawn_blocking(move || {
        Command::new(std::env::var_os("MODERATION_NODE").unwrap_or_else(|| "node".into()))
            .arg(driver)
            .arg(bootstrap)
            .arg(address)
            .arg(driver_ready)
            .output()
            .unwrap()
    });
    let mut drained = false;
    let result = loop {
        tokio::select! {
            result = &mut driver_task => break result.unwrap(),
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)), if !drained => {
                if drain_ready.exists() {
                    drained = server.drain(std::time::Duration::from_secs(8)).await.unwrap();
                    assert!(drained, "SDK did not close its drained session");
                }
            }
        }
    };
    server.stop().await.unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(drained);
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["send_chat"], false);
    let connection = rusqlite::Connection::open(database).unwrap();
    let mut statement = connection
        .prepare("SELECT event_json FROM probe_events")
        .unwrap();
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap();
    let mut schemas = 0;
    let mut native_structure_persisted = false;
    let mut message_batch_persisted = false;
    let mut capped_batch_persisted = false;
    let mut visible_match_persisted = false;
    let mut hidden_nonmatch_persisted = false;
    let mut candidates = Vec::new();
    let mut gift_candidates = Vec::new();
    let mut arrivals = Vec::new();
    let mut gift_revisions = Vec::new();
    let mut command_wire_persisted = false;
    let mut gift_hint_persisted = false;
    let mut application_json_persisted = false;
    let mut application_identity_shape_persisted = false;
    for row in rows {
        let text = row.unwrap();
        assert!(!text.contains("private-") && !text.contains("sensitive=query"));
        assert!(!text.contains("sampleProduct") && !text.contains("Fixture gift"));
        let event: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_ne!(event["event_type"], "gift.observed");
        assert_ne!(event["event_type"], "member.entered");
        if event["event_type"] == "member.candidate" {
            assert_eq!(event["payload"]["confidence"], "candidate");
            assert_eq!(event["payload"]["live_eligible"], false);
            assert_eq!(event["payload"]["room_verified"], false);
            arrivals.push(event.clone());
        }
        if event["event_type"] == "probe.message_content" {
            assert_eq!(event["payload"]["confidence"], "unknown");
            assert_eq!(event["payload"]["live_eligible"], false);
            assert_eq!(event["payload"]["room_verified"], false);
            assert!(
                event["user_id"].is_null()
                    && event["native_event_id"].is_null()
                    && event["nickname"].is_null()
            );
        }
        if event["event_type"] == "gift.candidate"
            || event["event_type"] == "gift.candidate_revision"
        {
            assert_eq!(event["payload"]["live_eligible"], false);
            assert_eq!(event["payload"]["room_verified"], false);
            assert_eq!(event["payload"]["confidence"], "candidate");
            assert_eq!(event["payload"]["quantity_mode"], "unknown");
            assert!(event["payload"].get("quantity").is_none());
            if event["event_type"] == "gift.candidate" {
                gift_candidates.push(event.clone());
            } else {
                gift_revisions.push(event.clone());
            }
        }
        if event["event_type"] == "probe.application" {
            application_json_persisted |= event["payload"]["message_type"] == 20009
                && event["payload"]["format"] == "base64_json";
            application_identity_shape_persisted |= event["payload"]["envelope_fields"]
                .as_array()
                .is_some_and(|fields| {
                    fields
                        .iter()
                        .any(|f| f == "$.from_user_contact.contact.username:string")
                });
        }
        if event["event_type"] == "probe.command" && event["payload"]["command_id"] == 777 {
            command_wire_persisted = event["payload"]["protobuf_candidates"] == 1
                && event["payload"]["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|f| f == "s1.f2:bytes");
        }
        if event["event_type"] == "probe.gift_schema" {
            gift_hint_persisted |= event["payload"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f == "$.gift_name:string");
        }
        if event["event_type"] == "chat.candidate" {
            assert_eq!(event["payload"]["live_eligible"], false);
            assert_eq!(event["payload"]["room_verified"], false);
            assert_eq!(event["payload"]["confidence"], "candidate");
            candidates.push(event.clone());
        }
        if event["event_type"] == "probe.message_match" {
            let counts = event["payload"]["type_counts"].as_array().unwrap();
            assert!(counts.len() <= 8);
            visible_match_persisted |= counts.iter().any(|c| c["type"] == 40 && c["matched"] == 1);
            hidden_nonmatch_persisted |= counts
                .iter()
                .any(|c| c["type"] == 42 && c["unmatched"] == 1);
            assert!(!counts.iter().any(|c| c["type"] == 39));
            assert!(counts.iter().all(|c| {
                c.as_object()
                    .unwrap()
                    .keys()
                    .all(|k| ["type", "matched", "unmatched"].contains(&k.as_str()))
            }));
        }
        if event["event_type"] == "probe.message_batch" {
            let payload = &event["payload"];
            let counts = payload["type_counts"].as_array().unwrap();
            assert!(counts.len() <= 8);
            message_batch_persisted |= counts
                .iter()
                .any(|c| c["type"] == 7 && c["count"] == 3 && c["opaque"] == 1);
            capped_batch_persisted |=
                payload["sampled"] == 55 && payload["total"] == 55 && payload["overflow"] == 47;
            for count in counts {
                assert!(count.as_object().unwrap().keys().all(|k| {
                    ["type", "count", "text", "json", "opaque", "absent"].contains(&k.as_str())
                }));
            }
        }
        if event["event_type"] == "probe.schema" {
            schemas += 1;
            assert!(event["payload"]["fields"].as_array().is_some());
            if event["payload"]["category"] == "dispatch" {
                let fields = event["payload"]["fields"].as_array().unwrap();
                native_structure_persisted |=
                    fields.iter().any(|field| field == "$.args[0]:undefined");
            }
        }
    }
    assert!(schemas >= 7);
    assert_eq!(schemas, result["schemas"].as_u64().unwrap());
    assert!(native_structure_persisted);
    assert!(message_batch_persisted && capped_batch_persisted);
    assert!(visible_match_persisted && hidden_nonmatch_persisted);
    assert!(command_wire_persisted && gift_hint_persisted);
    assert!(application_json_persisted);
    assert!(application_identity_shape_persisted);
    assert_eq!(arrivals.len(), 7);
    assert_eq!(arrivals[3]["payload"]["supporter_title"], "super_fan");
    assert_eq!(arrivals[4]["payload"]["supporter_title"], "star_guardian");
    assert_eq!(arrivals[5]["payload"]["supporter_title"], "star_guardian");
    assert!(arrivals[6]["payload"].get("supporter_title").is_none());
    assert_eq!(arrivals[0]["nickname"], "Fixture arrival");
    assert_eq!(arrivals[1]["user_id"], "fixture-arrival-other");
    assert_eq!(candidates.len(), 31);
    assert_eq!(gift_candidates.len(), 45);
    let anonymous = gift_candidates
        .iter()
        .find(|e| e["native_event_id"] == "gift-budget-0")
        .unwrap();
    assert_eq!(anonymous["nickname"], "神秘观众179");
    assert_eq!(anonymous["user_id"], "fixture-gift-user");

    assert_eq!(gift_revisions.len(), 5);
    assert_eq!(
        gift_revisions
            .iter()
            .filter(|e| e["native_event_id"] == "g1")
            .count(),
        4
    );
    assert!(
        gift_revisions
            .iter()
            .any(|e| e["payload"]["is_floatmsg"] == 1 && e["payload"]["previous_is_floatmsg"] == 0)
    );
    assert!(
        gift_revisions
            .iter()
            .any(|e| e["payload"]["reward_product_count"] == 0
                && e["payload"]["previous_reward_product_count"] == 2)
    );
    assert!(
        gift_revisions
            .iter()
            .any(
                |e| e["payload"]["recipient_user_id"] == "fixture-other-recipient"
                    && e["payload"]["previous_recipient_user_id"] == "fixture-recipient"
            )
    );
    assert_eq!(
        gift_candidates
            .iter()
            .filter(|e| e["native_event_id"] == "g1")
            .count(),
        2
    );
    assert_eq!(
        gift_candidates
            .iter()
            .filter(|e| e["native_event_id"] == "paused-gift")
            .count(),
        1
    );
    assert!(
        !gift_candidates
            .iter()
            .any(|e| e["native_event_id"] == "non-native-gift")
    );
    assert!(
        gift_candidates
            .iter()
            .any(|e| e["payload"]["gift_id"] == "fixture-star"
                && e["payload"]["reward_product_count"] == 1
                && e["payload"]["combo_product_count"] == 1
                && e["payload"]["is_floatmsg"] == 0
                && e["payload"]["float_type"] == 1
                && e["payload"]["recipient_user_id"] == "fixture-recipient")
    );
    assert_eq!(
        candidates
            .iter()
            .filter(|e| e["native_event_id"] == "m1")
            .count(),
        2
    );
    assert_eq!(
        candidates
            .iter()
            .filter(|e| e["native_event_id"] == "m3")
            .count(),
        1
    );
    assert!(
        !candidates
            .iter()
            .any(|e| e["native_event_id"] == "non-native")
    );
    let long = candidates
        .iter()
        .find(|e| e["native_event_id"] == "long")
        .unwrap();
    assert_eq!(
        long["payload"]["content"].as_str().unwrap().chars().count(),
        256
    );
    assert_eq!(long["payload"]["content_truncated"], true);
    assert_eq!(long["nickname"], "Fixture");
    assert!(
        candidates
            .iter()
            .any(|e| e["native_event_id"] == "emoji" && e["payload"]["content"] == "[强]")
    );
    assert!(
        candidates
            .iter()
            .any(|e| e["payload"]["content"] == "fixture forged flags")
    );
    let actions: i64 = connection
        .query_row("SELECT COUNT(*) FROM fill_audits", [], |row| row.get(0))
        .unwrap();
    assert_eq!(actions, 0);
    let previews: i64 = connection
        .query_row("SELECT COUNT(*) FROM preview_audits", [], |row| row.get(0))
        .unwrap();
    assert_eq!(previews, 0);
    let content_schemas:i64=connection.query_row("SELECT count(*) FROM probe_events WHERE json_extract(event_json,'$.event_type')='probe.message_content'",[],|r|r.get(0)).unwrap();
    assert!(content_schemas >= 4);
    let entry_hint:i64=connection.query_row("SELECT count(*) FROM probe_events WHERE json_extract(event_json,'$.event_type')='probe.message_content' AND json_extract(event_json,'$.payload.arrival_text_hint')=1",[],|r|r.get(0)).unwrap();
    assert!(entry_hint >= 2);
    let drained_events: i64 = connection.query_row("SELECT COUNT(*) FROM probe_events WHERE json_extract(event_json,'$.payload.stage')='drain_fixture'", [], |row| row.get(0)).unwrap();
    assert_eq!(drained_events, 125);
}

#[test]
fn desktop_tabs_have_separate_bindings_and_refresh_replaces_only_the_same_tab() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let scope = ProbeScope::wechat_desktop(
        "https://channels.weixin.qq.com/web/pages/live"
            .parse()
            .unwrap(),
    )
    .unwrap();
    let mut b = ProbeBridge::new(
        scope,
        ProbeJournal::open(&dir.path().join("tabs.sqlite3")).unwrap(),
    );
    b.configure_candidate_preview(
        live_archive_core::moderation::candidate_preview::CandidatePreviewConfig {
            enabled: true,
            recipient_user_id: Some("recipient".into()),
        },
    )
    .unwrap();
    b.enable_candidate_fill_preview(None).unwrap();
    b.enable_automatic_send().unwrap();
    b.set_moderation_enabled(false).unwrap();
    let pair = |b: &mut ProbeBridge, client: &str, doc: &str| {
        let boot = b.bootstrap_config(&b.relay_token.clone()).unwrap();
        b.hello(Hello {
            v: VERSION,
            client_id: Some(client.into()),
            ticket: boot["ticket"].as_str().unwrap().into(),
            page_url: b.scope.room_url.to_string(),
            document_id: doc.into(),
            adapter_id: "wechat_channels.desktop_probe".into(),
        })
        .unwrap()
    };
    let a = pair(&mut b, "6647e65f-df75-4f4c-a565-e076490400ee", "tab-a");
    let other = pair(&mut b, "b82a4cd3-8d3b-4112-9d79-7473ec9bc42e", "tab-b");
    assert_eq!(b.summary()["active_connections"], 2);
    let refreshed = pair(
        &mut b,
        "6647e65f-df75-4f4c-a565-e076490400ee",
        "tab-a-reload",
    );
    assert_eq!(b.summary()["active_connections"], 2);
    assert_ne!(a["binding_id"], refreshed["binding_id"]);
    let poll = |v: &serde_json::Value| {
        serde_json::from_value(json!({"v":VERSION,"page_url":"https://channels.weixin.qq.com/web/pages/live","connection_id":v["connection_id"],"binding_id":v["binding_id"]})).unwrap()
    };
    assert!(b.poll(a["token"].as_str().unwrap(), poll(&a)).is_err());
    assert!(
        b.poll(other["token"].as_str().unwrap(), poll(&other))
            .is_ok()
    );
    assert!(
        b.poll(refreshed["token"].as_str().unwrap(), poll(&refreshed))
            .is_ok()
    );
}
