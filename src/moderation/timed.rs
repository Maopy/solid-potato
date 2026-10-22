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
    /// `public_screen` counts display activity, including automatic messages.
    pub count_scope: String,
    pub messages: Vec<String>,
}
impl Default for TimedConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_seconds: 300,
            min_comments: 0,
            count_scope: "comments".into(),
            messages: vec![],
        }
    }
}
impl TimedConfig {
    pub fn validate(&self, max_chars: usize) -> Result<()> {
        if !matches!(
            self.count_scope.as_str(),
            "comments" | "text_comments" | "public_screen"
        ) || !(1..=86400).contains(&self.interval_seconds)
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
    counted_total: u64,
    excluded: std::collections::BTreeMap<&'static str, u64>,
    waiting_ready: bool,
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
            counted_total: 0,
            excluded: Default::default(),
            waiting_ready: false,
            seen_comments: Default::default(),
        }
    }
    pub fn set_active(&mut self, now: u64, active: bool) {
        if self.active != active {
            self.active = active;
            self.comments = 0;
            self.waiting_ready = false;
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
        if !self.config.enabled || !self.active || self.config.min_comments == 0 {
            return;
        }
        if self.config.count_scope == "public_screen" {
            if event.event_type != "public.activity" {
                return;
            }
            let p = &event.payload;
            let valid = match platform {
                "douyin" => event.source_kind == "dom" && p["decoder"] == "douyin.public_screen.v1",
                "wechat_channels" => {
                    event.source_kind == "network" && p["decoder"] == "wechat.msg_list.public.v1"
                }
                _ => false,
            };
            if !valid
                || p["observed_at_ms"].as_u64().is_none_or(|at| {
                    at.abs_diff(chrono::Utc::now().timestamp_millis().max(0) as u64) > 30_000
                })
            {
                self.exclude("source_or_freshness");
                return;
            }
            let Some(id) = event
                .native_event_id
                .as_deref()
                .filter(|id| super::events::valid_id(id))
            else {
                self.exclude("missing_identity_or_content");
                return;
            };
            // No audience identity, content or own-message exclusion is needed
            // for a display counter. Network and DOM observations never mix.
            self.count(now, "public", id);
            return;
        }
        if !matches!(event.event_type.as_str(), "chat.received" | "chat.activity") {
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
        if !valid {
            self.exclude("source_or_freshness");
            return;
        }
        let (Some(user), Some(id), Some(content), Some(observed)) = (
            event.user_id.as_deref(),
            event.native_event_id.as_deref(),
            p["content"].as_str(),
            p["observed_at_ms"].as_u64(),
        ) else {
            self.exclude("missing_identity_or_content");
            return;
        };
        if !super::events::valid_user_id(user)
            || !super::events::valid_id(id)
            || content.trim().is_empty()
            || content.chars().count() > 256
            || content.chars().any(char::is_control)
            || p["content_truncated"] == true
            || observed.abs_diff(chrono::Utc::now().timestamp_millis().max(0) as u64) > 30_000
        {
            self.exclude("invalid_or_stale_comment");
            return;
        }
        if self.config.count_scope == "comments"
            && (own_text
                || actor == Some(user)
                || self.config.messages.iter().any(|s| s == content))
        {
            self.exclude("own_reply");
            return;
        }
        self.count(now, user, id);
    }
    fn count(&mut self, now: u64, user: &str, id: &str) {
        self.seen_comments
            .retain(|_, at| now.saturating_sub(*at) < 60_000);
        let key = (user.to_owned(), id.to_owned());
        if self.seen_comments.len() >= 16_384 || self.seen_comments.contains_key(&key) {
            self.exclude("duplicate_or_capacity");
            return;
        }
        self.seen_comments.insert(key, now);
        self.comments = self.comments.saturating_add(1);
        self.counted_total = self.counted_total.saturating_add(1);
    }
    fn exclude(&mut self, reason: &'static str) {
        *self.excluded.entry(reason).or_default() += 1;
    }
    pub fn tick(&mut self, now: u64, ready: bool) -> Option<String> {
        if !self.config.enabled
            || !self.active
            || now < self.due
            || self.comments < self.config.min_comments
        {
            return None;
        }
        if !ready {
            if !self.waiting_ready {
                self.skipped += 1;
                self.waiting_ready = true;
            }
            return None;
        }
        self.waiting_ready = false;
        self.due = now.saturating_add(self.config.interval_seconds * 1000);
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
        json!({"count_scope":self.config.count_scope,"enabled":self.config.enabled,"active":self.active&&self.config.enabled,"interval_seconds":self.config.interval_seconds,"min_comments":self.config.min_comments,"comments_since_last":self.comments,"counted_total":self.counted_total,"excluded":self.excluded,"waiting_ready":self.waiting_ready,"remaining_comments":self.config.min_comments.saturating_sub(self.comments),"remaining_ms":if self.active&&self.config.enabled{Some(self.due.saturating_sub(now))}else{None},"generated":self.generated,"skipped":self.skipped})
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
            count_scope: "comments".into(),
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
        assert_eq!(t.status(7)["counted_total"], 1);
        assert_eq!(t.status(7)["excluded"]["own_reply"], 2);
        assert_eq!(t.status(7)["excluded"]["duplicate_or_capacity"], 1);
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
        add(&mut t, 30_001, "b");
        assert!(t.tick(30_002, false).is_none());
        assert_eq!(t.status(30_002)["comments_since_last"], 2);
        assert_eq!(t.status(30_002)["skipped"], 1);
        assert!(t.tick(30_003, true).is_some());
        assert!(t.tick(120_000, true).is_none());
        let mut legacy = TimedMessages::new(config(0), 0, true);
        assert!(legacy.tick(30_000, true).is_some());
    }
    #[test]
    fn public_screen_counts_all_authors_and_types_but_not_transport_duplicates() {
        for (platform, source, decoder) in [
            ("douyin", "dom", "douyin.public_screen.v1"),
            ("wechat_channels", "network", "wechat.msg_list.public.v1"),
        ] {
            let mut cfg = config(4);
            cfg.count_scope = "public_screen".into();
            let mut t = TimedMessages::new(cfg, 0, true);
            for (i, content) in ["观众评论", "送了鲜花 ×5", "来了", "定时文案"]
                .iter()
                .enumerate()
            {
                let mut e = comment(&format!("row-{i}"));
                e.event_type = "public.activity".into();
                e.source_kind = source.into();
                e.user_id = None;
                e.payload["decoder"] = json!(decoder);
                e.payload["content"] = json!(content);
                t.observe_comment(i as u64, platform, &e, true, Some("viewer"));
                t.observe_comment(i as u64, platform, &e, true, None);
            }
            // Native comments must not add a second count to DOM/list activity.
            t.observe_comment(5, platform, &comment("transport"), false, None);
            assert_eq!(t.status(5)["comments_since_last"], 4);
            assert!(t.tick(29_999, true).is_none());
            assert!(t.tick(30_000, true).is_some());
            assert_eq!(t.status(30_000)["comments_since_last"], 0);
            let mut stale = comment("stale");
            stale.event_type = "public.activity".into();
            stale.source_kind = source.into();
            stale.payload["decoder"] = json!(decoder);
            stale.payload["observed_at_ms"] = json!(1);
            t.observe_comment(30_001, platform, &stale, false, None);
            assert_eq!(t.status(30_001)["comments_since_last"], 0);
        }
    }
    #[test]
    fn text_comments_include_automatic_text_but_never_arrivals_or_gifts() {
        let mut cfg = config(3);
        cfg.count_scope = "text_comments".into();
        let mut t = TimedMessages::new(cfg, 0, true);
        for i in 0..20 {
            let mut e = comment(&format!("entry-{i}"));
            e.event_type = "public.activity".into();
            e.payload["message_type"] = json!(10005);
            t.observe_comment(i, "wechat_channels", &e, false, None);
        }
        assert_eq!(t.status(30_000)["comments_since_last"], 0);
        assert!(t.tick(30_000, true).is_none());
        for (i, text) in ["观众评论", "定时文案", "场控感谢"].iter().enumerate() {
            let mut e = comment(&format!("text-{i}"));
            e.payload["content"] = json!(text);
            t.observe_comment(30_001 + i as u64, "douyin", &e, true, Some("viewer"));
            t.observe_comment(30_001 + i as u64, "douyin", &e, true, None);
        }
        assert_eq!(t.status(30_003)["comments_since_last"], 3);
        assert!(t.tick(30_003, true).is_some());
        assert!(
            t.tick(60_003, true).is_none(),
            "elapsed time without new text cannot generate again"
        );
        // The ordinary WeChat type-1 activity stream has the same semantics.
        let mut e = comment("wx-own");
        e.event_type = "chat.activity".into();
        e.payload["decoder"] = json!("wechat.msg_list.activity.v1");
        e.payload["keyword_live"] = json!(true);
        e.payload["message_type"] = json!(1);
        t.observe_comment(60_004, "wechat_channels", &e, true, Some("viewer"));
        assert_eq!(t.status(60_004)["comments_since_last"], 1);
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
