//! Room-scoped periodic text selection. Dispatch still uses the ordinary fill queue.
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimedConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub min_comments: u64,
    pub messages: Vec<String>,
}
impl Default for TimedConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 300,
            min_comments: 0,
            messages: vec![],
        }
    }
}
impl TimedConfig {
    pub fn validate(&self, max_chars: usize) -> Result<()> {
        if !(1..=86400).contains(&self.interval_seconds)
            || self.min_comments > 10_000
            || self.messages.len() > 64
            || (self.enabled && self.messages.is_empty())
        {
            bail!(
                "timed_messages requires interval_seconds=1..86400, min_comments=0..10000 and 1..64 messages when enabled"
            );
        }
        let mut seen = std::collections::HashSet::new();
        for text in &self.messages {
            super::fill::validate_text(text)?;
            if text.chars().count() > max_chars || text.trim() != text || !seen.insert(text) {
                bail!(
                    "timed_messages texts must fit limits.max_text_chars, have no surrounding whitespace, and be unique"
                );
            }
        }
        Ok(())
    }
}
pub struct TimedMessages {
    config: TimedConfig,
    active: bool,
    due: u64,
    previous: Option<usize>,
    generated: u64,
    skipped: u64,
    comments: u64,
    seen_comments: std::collections::HashMap<(String, String), u64>,
}
impl TimedMessages {
    pub fn new(config: TimedConfig, now: u64, active: bool) -> Self {
        let due = now.saturating_add(config.interval_seconds * 1000);
        Self {
            config,
            active,
            due,
            previous: None,
            generated: 0,
            skipped: 0,
            comments: 0,
            seen_comments: Default::default(),
        }
    }
    pub fn set_active(&mut self, now: u64, active: bool) {
        if self.active != active {
            self.active = active;
            self.comments = 0;
            self.due = now.saturating_add(self.config.interval_seconds * 1000);
        }
    }
    pub fn observe_comment(
        &mut self,
        now: u64,
        platform: &str,
        event: &super::protocol::ProbeEvent,
        own_text: bool,
        actor: Option<&str>,
    ) {
        if !self.config.enabled || !self.active || self.config.min_comments == 0 || own_text {
            return;
        }
        let p = &event.payload;
        let valid = event.source_kind == "network"
            && match platform {
                "douyin" => {
                    event.event_type == "chat.received"
                        && p["decoder"] == "douyin.pb.v1"
                        && p["live_eligible"] == true
                }
                "wechat_channels" => {
                    event.event_type == "chat.activity"
                        && p["decoder"] == "wechat.msg_list.activity.v1"
                        && p["keyword_live"] == true
                        && p["message_type"] == 1
                }
                _ => false,
            };
        let (Some(user), Some(id), Some(content), Some(observed)) = (
            event.user_id.as_deref(),
            event.native_event_id.as_deref(),
            p["content"].as_str(),
            p["observed_at_ms"].as_u64(),
        ) else {
            return;
        };
        if !valid
            || !super::events::valid_user_id(user)
            || !super::events::valid_id(id)
            || content.trim().is_empty()
            || content.chars().count() > 256
            || content.chars().any(char::is_control)
            || p["content_truncated"] == true
            || actor == Some(user)
            || self.config.messages.iter().any(|s| s == content)
            || observed.abs_diff(chrono::Utc::now().timestamp_millis().max(0) as u64) > 30_000
        {
            return;
        }
        self.seen_comments
            .retain(|_, at| now.saturating_sub(*at) < 60_000);
        let key = (user.to_owned(), id.to_owned());
        if self.seen_comments.len() >= 16_384 || self.seen_comments.contains_key(&key) {
            return;
        }
        self.seen_comments.insert(key, now);
        self.comments = self
            .comments
            .saturating_add(1)
            .min(self.config.min_comments);
    }
    pub fn tick(&mut self, now: u64, ready: bool) -> Option<String> {
        if !self.config.enabled
            || !self.active
            || now < self.due
            || self.comments < self.config.min_comments
        {
            return None;
        }
        self.due = now.saturating_add(self.config.interval_seconds * 1000);
        if !ready {
            self.skipped += 1;
            return None;
        }
        let choices: Vec<_> = (0..self.config.messages.len())
            .filter(|i| self.config.messages.len() == 1 || Some(*i) != self.previous)
            .collect();
        let i = choices[(uuid::Uuid::new_v4().as_u128() % choices.len() as u128) as usize];
        self.comments = 0;
        self.previous = Some(i);
        self.generated += 1;
        Some(self.config.messages[i].clone())
    }
    pub fn status(&self, now: u64) -> Value {
        json!({"enabled":self.config.enabled,"active":self.active&&self.config.enabled,"interval_seconds":self.config.interval_seconds,"min_comments":self.config.min_comments,"comments_since_last":self.comments,"remaining_comments":self.config.min_comments.saturating_sub(self.comments),"remaining_ms":if self.active&&self.config.enabled{Some(self.due.saturating_sub(now))}else{None},"generated":self.generated,"skipped":self.skipped})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moderation::protocol::ProbeEvent;
    fn config(min_comments: u64) -> TimedConfig {
        TimedConfig {
            enabled: true,
            interval_seconds: 30,
            min_comments,
            messages: vec!["定时文案".into()],
        }
    }
    fn comment(id: &str) -> ProbeEvent {
        serde_json::from_value(json!({"seq":1,"event_id":id,"event_type":"chat.received","source_kind":"network","user_id":"viewer","native_event_id":id,"payload":{"content":"你好","decoder":"douyin.pb.v1","live_eligible":true,"observed_at_ms":chrono::Utc::now().timestamp_millis()}})).unwrap()
    }
    fn add(t: &mut TimedMessages, now: u64, id: &str) {
        t.observe_comment(now, "douyin", &comment(id), false, None);
    }
    #[test]
    fn both_conditions_are_required_in_either_order_and_reset_after_generation() {
        let mut t = TimedMessages::new(config(2), 0, true);
        add(&mut t, 1, "a");
        add(&mut t, 2, "b");
        assert!(t.tick(29_999, true).is_none());
        assert!(t.tick(30_000, true).is_some());
        assert_eq!(t.status(30_000)["comments_since_last"], 0);
        assert!(t.tick(60_000, true).is_none());
        assert!(t.tick(90_000, true).is_none());
        add(&mut t, 90_001, "c");
        assert!(t.tick(90_001, true).is_none());
        add(&mut t, 90_002, "d");
        assert!(t.tick(90_002, true).is_some());
    }
    #[test]
    fn only_fresh_unique_audience_comments_count_and_pause_discards_progress() {
        let mut t = TimedMessages::new(config(3), 0, true);
        add(&mut t, 1, "a");
        add(&mut t, 2, "a");
        let mut old = comment("old");
        old.payload["observed_at_ms"] = json!(1);
        t.observe_comment(3, "douyin", &old, false, None);
        let mut dom = comment("dom");
        dom.source_kind = "dom".into();
        t.observe_comment(4, "douyin", &dom, false, None);
        t.observe_comment(5, "douyin", &comment("own"), true, None);
        t.observe_comment(6, "douyin", &comment("actor"), false, Some("viewer"));
        assert_eq!(t.status(7)["comments_since_last"], 1);
        t.set_active(8, false);
        add(&mut t, 9, "off");
        t.set_active(10, true);
        assert_eq!(t.status(11)["comments_since_last"], 0);
        assert_eq!(t.status(11)["remaining_ms"], 29999);
        add(&mut t, 12, "a");
        assert_eq!(
            t.status(12)["comments_since_last"],
            0,
            "pause cannot replay an old native message"
        );
    }
    #[test]
    fn busy_dispatch_retains_comments_but_never_accumulates_messages() {
        let mut t = TimedMessages::new(config(1), 0, true);
        add(&mut t, 1, "a");
        assert!(t.tick(30_000, false).is_none());
        assert!(t.tick(30_001, true).is_none());
        assert!(t.tick(60_000, true).is_some());
        assert!(t.tick(120_000, true).is_none());
        let mut legacy = TimedMessages::new(config(0), 0, true);
        assert!(legacy.tick(30_000, true).is_some());
    }
    #[test]
    fn configuration_defaults_and_limits_are_validated() {
        let config: TimedConfig =
            serde_yaml_ng::from_str("enabled: true\ninterval_seconds: 30\nmessages: ['hello']")
                .unwrap();
        assert_eq!(config.min_comments, 0);
        config.validate(80).unwrap();
        let mut bad = config;
        bad.min_comments = 10_001;
        assert!(bad.validate(80).is_err());
    }
}
