//! Bounded, in-memory room history shared with the live speech worker.
use super::protocol::ProbeEvent;
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

pub const RETENTION_MS: i64 = 30 * 60 * 1000;

#[derive(Clone, Serialize)]
pub struct Entry {
    pub id: String,
    pub kind: String,
    pub at_ms: i64,
    pub end_ms: i64,
    pub time_basis: String,
    pub text: String,
    pub nickname: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fan_level: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<super::decision::Rating>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<super::replies::Advice>,
}

impl Entry {
    fn time_ms(&self) -> i64 {
        if self.kind == "speech" {
            self.end_ms
        } else {
            self.at_ms
        }
    }
}

#[derive(Clone, Default)]
pub struct Timeline(Arc<Mutex<VecDeque<Entry>>>);

impl Timeline {
    pub fn entries(&self) -> Vec<Entry> {
        self.entries_at(chrono::Utc::now().timestamp_millis())
    }

    fn entries_at(&self, now_ms: i64) -> Vec<Entry> {
        let mut history = self.0.lock().unwrap();
        Self::prune(&mut history, now_ms);
        let mut entries: Vec<_> = history.iter().cloned().collect();
        entries.sort_by_key(Entry::time_ms);
        let retry: Vec<_> = history
            .iter_mut()
            .filter(|e| e.rating.as_ref().is_some_and(|r| r.state == "busy"))
            .map(|e| {
                e.rating = Some(super::decision::Rating::state("pending"));
                (e.id.clone(), e.text.clone(), e.at_ms)
            })
            .collect();
        drop(history);
        for (id, text, at) in retry {
            super::decision::submit(self.clone(), id, text, at);
        }
        entries
    }

    fn prune(entries: &mut VecDeque<Entry>, now_ms: i64) {
        let cutoff = now_ms.saturating_sub(RETENTION_MS);
        // Recognition may arrive late, so expired entries need not form a prefix.
        entries.retain(|entry| entry.time_ms() >= cutoff);
    }

    pub fn push(&self, entry: Entry) {
        let now = chrono::Utc::now().timestamp_millis();
        let mut entries = self.0.lock().unwrap();
        Self::prune(&mut entries, now);
        if entry.time_ms() < now.saturating_sub(RETENTION_MS)
            || entries.iter().any(|e| e.id == entry.id)
        {
            return;
        }
        let job = (entry.kind == "chat"
            && entry.rating.as_ref().is_some_and(|r| r.state == "pending"))
        .then(|| (entry.id.clone(), entry.text.clone(), entry.at_ms));
        entries.push_back(entry);
        drop(entries);
        if let Some((id, text, at)) = job {
            super::decision::submit(self.clone(), id, text, at);
        }
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.0.lock().unwrap().iter().any(|e| {
            e.id == id && e.time_ms() >= chrono::Utc::now().timestamp_millis() - RETENTION_MS
        })
    }

    pub(crate) fn rate(&self, id: &str, rating: super::decision::Rating) {
        if let Some(entry) = self.0.lock().unwrap().iter_mut().find(|e| e.id == id) {
            entry.rating = Some(rating);
        }
    }

    pub(crate) fn reply_advice(&self, id: &str) -> Option<super::replies::Advice> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.reply.clone())
    }
    pub(crate) fn set_reply(&self, id: &str, advice: super::replies::Advice) {
        if let Some(entry) = self.0.lock().unwrap().iter_mut().find(|e| e.id == id) {
            entry.reply = Some(advice);
        }
    }
    pub(crate) fn reply_context(&self, id: &str) -> anyhow::Result<serde_json::Value> {
        let entries = self.entries();
        let target = entries
            .iter()
            .find(|e| e.id == id && e.kind == "chat")
            .ok_or_else(|| anyhow::anyhow!("留言已过期或不属于当前直播间"))?;
        let now = chrono::Utc::now().timestamp_millis();
        // Capture the click-time context: newer host answers can matter for an older comment.
        let recent = |kind: &str, limit| {
            entries.iter().rev().filter(|e|e.kind==kind && e.id!=id && e.time_ms()<=now).take(5)
            .map(|e|serde_json::json!({"at_ms":e.time_ms(),"text":e.text.chars().take(limit).collect::<String>()})).collect::<Vec<_>>()
        };
        Ok(
            serde_json::json!({"TARGET_COMMENT":target.text,"target_at_ms":target.time_ms(),"recent_chat":recent("chat",60),"host_speech":recent("speech",80)}),
        )
    }

    // Reuse only a recent, exact low-priority result from this room. Normal
    // conversation remains context-sensitive and failures are never cached.
    pub(crate) fn recent_repeated_low_score(&self, id: &str, text: &str, at: i64) -> Option<u8> {
        let text = text.trim();
        if text.is_empty() { return None; }
        let entries = self.0.lock().unwrap();
        entries.iter().filter(|e| e.kind == "chat" && e.id != id && e.at_ms <= at
            && e.at_ms >= at.saturating_sub(120_000) && e.text.trim() == text)
            .filter_map(|e| {
                let r = e.rating.as_ref()?;
                let score = r.score?;
                (r.state == "scored" && r.model == super::decision::MODEL)
                    .then_some((e.at_ms, score))
            }).max_by_key(|(time, _)| *time).and_then(|(_, score)| (score <= 55).then_some(score))
    }

    pub(crate) fn scoring_context(&self, id: &str, text: &str, at: i64) -> serde_json::Value {
        let entries = self.entries();
        let host: Vec<_> = entries
            .iter()
            .filter(|e| e.kind == "speech" && e.end_ms <= at && e.end_ms >= at - 90_000)
            .rev()
            .take(3)
            .map(|e| e.text.chars().take(180).collect::<String>())
            .collect();
        let recent: Vec<_> = entries
            .iter()
            .filter(|e| e.kind == "chat" && e.id != id && e.at_ms <= at && e.at_ms >= at - 30_000)
            .rev()
            .take(3)
            .map(|e| e.text.chars().take(100).collect::<String>())
            .collect();
        serde_json::json!({"TARGET_COMMENT":text,"recent_host_speech_newest_first":host,"recent_comments_newest_first":recent})
    }

    pub fn speech(&self, text: &str, start: i64, end: i64) {
        let text: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(2400)
            .collect();
        if text.trim().is_empty() {
            return;
        }
        self.push(Entry {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "speech".into(),
            at_ms: start,
            end_ms: end,
            time_basis: "audio_capture".into(),
            text,
            nickname: "主播".into(),
            fan_level: None,
            rating: None,
            reply: None,
        });
    }

    pub fn note(&self, text: &str) {
        let at = chrono::Utc::now().timestamp_millis();
        self.push(Entry {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "note".into(),
            at_ms: at,
            end_ms: at,
            time_basis: "received".into(),
            text: text.into(),
            nickname: String::new(),
            fan_level: None,
            rating: None,
            reply: None,
        });
    }

    pub fn observe(&self, event: &ProbeEvent) {
        if !matches!(
            event.event_type.as_str(),
            "chat.received"
                | "chat.observed"
                | "chat.activity"
                | "chat.keyword_candidate"
                | "chat.candidate"
        ) {
            return;
        }
        let Some(text) = event.payload["content"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
        else {
            return;
        };
        let now = chrono::Utc::now().timestamp_millis();
        let mut at = event.payload["observed_at_ms"]
            .as_i64()
            .filter(|t| *t > 0 && *t <= now + 60_000)
            .unwrap_or(now);
        // Only Douyin's decoder supplies the native message time. WeChat uses receipt time.
        let message_time = event.payload["message_at_ms"]
            .as_i64()
            .filter(|t| *t > 0 && *t <= now + 60_000);
        let native = matches!(event.event_type.as_str(), "chat.received" | "chat.observed")
            && event.payload["decoder"] == "douyin.pb.v1"
            && message_time.is_some();
        if native {
            at = message_time.unwrap();
        }
        self.push(Entry {
            id: format!(
                "chat:{}:{}",
                event.user_id.as_deref().unwrap_or(""),
                event.native_event_id.as_deref().unwrap_or(&event.event_id)
            ),
            kind: "chat".into(),
            at_ms: at,
            end_ms: at,
            time_basis: if native { "message" } else { "received" }.into(),
            text: text.chars().take(512).collect(),
            nickname: event
                .nickname
                .clone()
                .or_else(|| event.user_id.clone())
                .unwrap_or_else(|| "观众".into()),
            fan_level: event.payload["fan_level"]
                .as_u64()
                .filter(|v| (1..=100000).contains(v))
                .map(|v| v as u32),
            rating: super::decision::initial(),
            reply: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repeated_low_score_is_room_local_exact_recent_and_successful() {
        let t = Timeline::default();
        let other = Timeline::default();
        let now = chrono::Utc::now().timestamp_millis();
        let make = |id: &str, text: &str, at: i64, state: &str, score: Option<u8>| Entry {
            id: id.into(), kind: "chat".into(), at_ms: at, end_ms: at,
            time_basis: "received".into(), text: text.into(), nickname: "viewer".into(),
            fan_level: None, rating: Some(super::super::decision::Rating { state: state.into(), score, model: super::super::decision::MODEL.into() }), reply: None,
        };
        t.push(make("old", "点点礼物墙～", now - 30_000, "scored", Some(26)));
        t.push(make("high", "正常聊天", now - 10_000, "scored", Some(70)));
        t.push(make("error", "失败", now - 10_000, "error", None));
        assert_eq!(t.recent_repeated_low_score("new", "点点礼物墙～", now), Some(26));
        assert_eq!(other.recent_repeated_low_score("new", "点点礼物墙～", now), None);
        assert_eq!(t.recent_repeated_low_score("new", "点点礼物墙!", now), None);
        assert_eq!(t.recent_repeated_low_score("new", "正常聊天", now), None);
        assert_eq!(t.recent_repeated_low_score("new", "失败", now), None);
        assert_eq!(t.recent_repeated_low_score("new", "点点礼物墙～", now + 120_000), None);
        assert_eq!(t.recent_repeated_low_score("new", "点点礼物墙～", now - 60_000), None);
    }

    #[test]
    fn fan_badge_survives_sanitization_and_invalid_levels_are_hidden() {
        for (level, expected) in [(json!(6), Some(6)), (json!(0), None), (json!("6"), None)] {
            let event: ProbeEvent = serde_json::from_value(json!({"seq":1,"event_id":"badge","event_type":"chat.observed","source_kind":"network","payload":{"content":"你好","fan_level":level}})).unwrap();
            let t = Timeline::default();
            t.observe(&super::super::protocol::sanitize(&event));
            assert_eq!(t.entries()[0].fan_level, expected);
        }
    }

    #[test]
    fn reply_context_uses_click_time_last_five_comments_and_speech_only_in_this_room() {
        let t = Timeline::default();
        let other = Timeline::default();
        let now = chrono::Utc::now().timestamp_millis();
        let target: ProbeEvent=serde_json::from_value(json!({"seq":1,"event_id":"target","event_type":"chat.observed","source_kind":"network","payload":{"content":"歌名？","observed_at_ms":now-3000}})).unwrap();
        t.observe(&target);
        for i in 0..7 {
            let e: ProbeEvent=serde_json::from_value(json!({"seq":1,"event_id":format!("c{i}"),"event_type":"chat.observed","source_kind":"network","payload":{"content":format!("留言{i}"),"observed_at_ms":now-2000+i*100}})).unwrap();
            t.observe(&e);
            t.speech(
                &format!("主播{i}"),
                now - 2000 + i * 100,
                now - 1900 + i * 100,
            );
        }
        other.speech("另一位主播不能泄漏", now - 1000, now - 900);
        let c = t.reply_context("chat::target").unwrap();
        assert_eq!(c["recent_chat"].as_array().unwrap().len(), 5);
        assert_eq!(c["host_speech"].as_array().unwrap().len(), 5);
        assert_eq!(c["recent_chat"][0]["text"], "留言6");
        assert_eq!(
            c["host_speech"][0]["text"], "主播6",
            "host answers after the target are available at click time"
        );
        assert!(other.reply_context("chat::target").is_err());
        assert!(t.reply_context("missing").is_err());
        assert!(!c.to_string().contains("另一位主播"));
    }

    #[test]
    fn retries_and_multiple_comment_sources_do_not_duplicate_messages() {
        let t = Timeline::default();
        let mut event: ProbeEvent = serde_json::from_value(json!({"seq":1,"event_id":"one","event_type":"chat.activity","source_kind":"network","native_event_id":"native","user_id":"user","nickname":"小雨","payload":{"content":"好听"}})).unwrap();
        t.observe(&event);
        event.event_type = "chat.keyword_candidate".into();
        event.event_id = "two".into();
        t.observe(&event);
        assert_eq!(t.entries().len(), 1);
        assert_eq!(t.entries()[0].time_basis, "received");
        let now = chrono::Utc::now().timestamp_millis();
        t.speech("晚到的识别", now - 13000, now - 1000);
        assert_eq!(t.entries()[0].at_ms, now - 13000);
    }

    #[test]
    fn thirty_minute_history_has_no_two_thousand_entry_cap_and_isolates_rooms() {
        let t = Timeline::default();
        let worker = t.clone();
        let now = chrono::Utc::now().timestamp_millis();
        for i in 0..2005 {
            worker.speech("一句话", now - 60_000 + i, now - 48_000 + i);
        }
        assert_eq!(t.entries().len(), 2005);
        assert!(Timeline::default().entries().is_empty());
    }

    #[test]
    fn history_expires_without_new_messages_and_late_results_use_speech_end_time() {
        let t = Timeline::default();
        let now = chrono::Utc::now().timestamp_millis();
        t.speech(
            "仍在窗口内",
            now - RETENTION_MS - 12_000,
            now - RETENTION_MS + 1000,
        );
        t.speech(
            "迟到且已过期",
            now - RETENTION_MS - 20_000,
            now - RETENTION_MS - 1000,
        );
        t.speech("最新一句", now - 12_000, now);
        assert_eq!(t.entries_at(now).len(), 2);
        assert_eq!(
            t.entries_at(now + RETENTION_MS).len(),
            1,
            "boundary is inclusive"
        );
        assert!(t.entries_at(now + RETENTION_MS + 1).is_empty());
        assert!(
            t.0.lock().unwrap().is_empty(),
            "reads physically remove expired history"
        );
    }

    #[test]
    fn native_message_time_is_distinct_from_receipt_time() {
        let t = Timeline::default();
        let event: ProbeEvent = serde_json::from_value(json!({"seq":1,"event_id":"native","event_type":"chat.received","source_kind":"network","payload":{"content":"消息","decoder":"douyin.pb.v1","observed_at_ms":chrono::Utc::now().timestamp_millis(),"message_at_ms":chrono::Utc::now().timestamp_millis()-1000}})).unwrap();
        t.observe(&event);
        assert_eq!(
            t.entries()[0].at_ms,
            event.payload["message_at_ms"].as_i64().unwrap()
        );
        assert_eq!(t.entries()[0].time_basis, "message");
    }
}
