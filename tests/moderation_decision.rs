//! Opt-in smoke test against the actually installed local model; no network provider.
use live_archive_core::moderation::{decision::MODEL, protocol::ProbeEvent, timeline::Timeline};
use serde_json::json;

#[tokio::test]
#[ignore = "requires LIVE_ARCHIVE_DECISION_CONFIG and the local CUDA model"]
async fn local_startlux_scores_comments_without_moderation_and_keeps_rooms_isolated() {
    assert!(std::env::var_os("LIVE_ARCHIVE_DECISION_CONFIG").is_some());
    let first = Timeline::default();
    let second = Timeline::default();
    let now = chrono::Utc::now().timestamp_millis();
    first.speech("欢迎大家，接下来继续跳舞。", now - 8000, now - 1000);
    let event = |id, text| {
        serde_json::from_value::<ProbeEvent>(json!({"seq":1,"event_id":id,"native_event_id":id,"event_type":"chat.observed","source_kind":"network","nickname":"测试观众","payload":{"content":text,"live_eligible":false}})).unwrap()
    };
    let a = event("song", "这首歌叫什么名字？");
    let b = event("hello", "晚上好，来了来了");
    first.observe(&a);
    first.observe(&a);
    second.observe(&b);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
    loop {
        let all = [first.entries(), second.entries()];
        let ratings: Vec<_> = all
            .iter()
            .flatten()
            .filter(|e| e.kind == "chat")
            .map(|e| e.rating.as_ref().expect("rating configured"))
            .collect();
        assert_eq!(ratings.len(), 2, "retries cannot duplicate comments");
        if ratings.iter().all(|r| r.state != "pending") {
            for r in ratings {
                assert_eq!(r.state, "scored");
                assert_eq!(r.model, MODEL);
                assert!(r.score.unwrap() <= 100);
                println!("local StartLux: {} points", r.score.unwrap());
            }
            assert_eq!(first.entries().len(), 2);
            assert_eq!(second.entries().len(), 1);
            first.speech("这首歌叫《夜曲》。", now - 500, now);
            let id = first
                .entries()
                .into_iter()
                .find(|e| e.kind == "chat")
                .unwrap()
                .id;
            let pending =
                live_archive_core::moderation::replies::request(first.clone(), &id).unwrap();
            assert_eq!(pending.state, "pending");
            assert!(live_archive_core::moderation::replies::request(second.clone(), &id).is_err());
            loop {
                let entry = first.entries().into_iter().find(|e| e.id == id).unwrap();
                let advice = entry.reply.unwrap();
                if advice.state != "pending" {
                    assert_eq!(advice.state, "ready", "local StartLux must select replies");
                    assert_eq!(advice.suggestions.len(), 3);
                    let texts: std::collections::HashSet<_> =
                        advice.suggestions.iter().map(|s| s.text.clone()).collect();
                    assert_eq!(texts.len(), advice.suggestions.len());
                    assert!(
                        advice.suggestions.iter().any(|s| s.text.contains("夜曲")),
                        "reply should use the host’s answer"
                    );
                    assert!(
                        advice.suggestions.iter().all(|s| !s.text.contains("舒缓")),
                        "reply must not add an unsupported song description"
                    );
                    assert!(advice.has_speech);
                    for suggestion in advice.suggestions {
                        println!("{}: {}", suggestion.direction, suggestion.text);
                    }
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "suggestions timed out"
                );
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            second.speech(
                "这首歌叫《夜曲》，喜欢跳舞的朋友一起跟着节奏跳。",
                now - 500,
                now,
            );
            let greeting_id = second
                .entries()
                .into_iter()
                .find(|e| e.kind == "chat")
                .unwrap()
                .id;
            live_archive_core::moderation::replies::request(second.clone(), &greeting_id).unwrap();
            loop {
                let advice = second
                    .entries()
                    .into_iter()
                    .find(|e| e.id == greeting_id)
                    .unwrap()
                    .reply
                    .unwrap();
                if advice.state != "pending" {
                    assert_eq!(advice.state, "ready");
                    assert_eq!(advice.suggestions.len(), 3);
                    for suggestion in advice.suggestions {
                        assert!(
                            !["歌", "舞", "主播刚才"]
                                .iter()
                                .any(|topic| suggestion.text.contains(topic)),
                            "greeting replies must stay on the target instead of surrounding music/dance: {}",
                            suggestion.text
                        );
                        println!("greeting / {}: {}", suggestion.direction, suggestion.text);
                    }
                    break;
                }
                assert!(tokio::time::Instant::now() < deadline);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            for (case_id, text) in [
                ("business", "就是少了酒楼饭店这些大客户"),
                ("ambiguous", "有几个"),
                ("hair", "不要染"),
                ("ambiguous_dye", "不要染"),
            ] {
                let room = Timeline::default();
                if case_id == "hair" {
                    room.speech("不是银色，是灰色，还有漂过粉色。", now - 6_000, now - 5_000);
                    room.speech("还有啥色？", now - 4_000, now - 3_000);
                    room.observe(&event("unrelated", "听歌只听贝拉404"));
                }
                room.observe(&event(case_id, text));
                let id = room
                    .entries()
                    .into_iter()
                    .find(|e| e.kind == "chat" && e.text == text)
                    .unwrap()
                    .id;
                live_archive_core::moderation::replies::request(room.clone(), &id).unwrap();
                loop {
                    let advice = room
                        .entries()
                        .into_iter()
                        .find(|e| e.id == id)
                        .unwrap()
                        .reply
                        .unwrap();
                    if advice.state != "pending" {
                        assert_eq!(advice.state, "ready");
                        let texts: Vec<_> =
                            advice.suggestions.iter().map(|s| s.text.as_str()).collect();
                        if case_id == "business" {
                            assert_eq!(texts.len(), 3);
                            assert!(
                                texts.iter().all(|t| !["核实", "反馈", "抱歉"]
                                    .iter()
                                    .any(|w| t.contains(w))),
                                "business discussion should not be treated as a complaint: {texts:?}"
                            );
                            assert!(texts.iter().any(|t| t.contains("酒楼")
                                || t.contains("饭店")
                                || t.contains("这类客户")));
                            assert!(
                                texts.iter().any(|t| ["采购", "进货", "供货", "配送"]
                                    .iter()
                                    .any(|w| t.contains(w))),
                                "reply should include a concrete business angle: {texts:?}"
                            );
                            assert!(
                                texts.iter().all(|t| !t.contains("值得考虑的方向")),
                                "avoid empty acknowledgement: {texts:?}"
                            );
                        } else if case_id == "ambiguous" {
                            assert_eq!(texts, ["你是想问哪样东西有几个呀？"]);
                        } else if case_id == "hair" {
                            assert_eq!(texts.len(), 3);
                            assert!(
                                texts.iter().all(|t| ["染", "发色", "发质"]
                                    .iter()
                                    .any(|word| t.contains(word))),
                                "every reply should address the hair-dye topic: {texts:?}"
                            );
                            assert!(
                                texts.iter().all(|t| !["具体是指", "多讲一点", "继续说说"]
                                    .iter()
                                    .any(|word| t.contains(word))),
                                "do not fall back to generic replies: {texts:?}"
                            );
                        } else {
                            assert_eq!(texts, ["你是指不要染什么呢？"]);
                        }
                        println!("{case_id}: {texts:?}");
                        break;
                    }
                    assert!(tokio::time::Instant::now() < deadline);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "scoring timed out");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}
