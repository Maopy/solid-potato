//! Literal keyword replies with shared per-keyword and per-reply cooldowns.
use super::{
    events::{valid_id, valid_user_id},
    protocol::ProbeEvent,
};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

fn minute() -> u64 {
    60
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    #[default]
    Contains,
    Exact,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeywordRule {
    pub keywords: Vec<String>,
    pub reply: String,
    #[serde(default)]
    pub match_mode: MatchMode,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeywordConfig {
    pub enabled: bool,
    #[serde(default = "minute")]
    pub cooldown_seconds: u64,
    pub rules: Vec<KeywordRule>,
}
impl Default for KeywordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cooldown_seconds: 60,
            rules: vec![],
        }
    }
}
impl KeywordConfig {
    pub fn validate(&self, limit: usize) -> Result<()> {
        if !(1..=86400).contains(&self.cooldown_seconds)
            || self.rules.len() > 32
            || self.enabled && self.rules.is_empty()
        {
            bail!(
                "keyword_replies requires cooldown_seconds=1..86400 and 1..32 rules when enabled"
            );
        }
        let mut seen = HashSet::new();
        for rule in &self.rules {
            super::fill::validate_text(&rule.reply)?;
            if rule.reply.trim() != rule.reply
                || rule.reply.chars().count() > limit
                || rule.keywords.is_empty()
                || rule.keywords.len() > 16
            {
                bail!("invalid keyword_replies rule: reply length or keywords count");
            }
            for keyword in &rule.keywords {
                if keyword.trim() != keyword
                    || keyword.is_empty()
                    || keyword.chars().count() > 64
                    || keyword.chars().any(char::is_control)
                    || !seen.insert(keyword.to_lowercase())
                {
                    bail!(
                        "keyword_replies keywords must be unique, nonempty, at most 64 characters, without surrounding whitespace or controls"
                    );
                }
            }
        }
        Ok(())
    }
}
pub struct KeywordDraft {
    pub text: String,
    pub keyword: String,
    pub event_id: String,
    pub native_event_id: String,
}
pub struct KeywordReplies {
    config: KeywordConfig,
    active: bool,
    seen: HashMap<(String, String), u64>,
    keyword_at: HashMap<String, u64>,
    reply_at: HashMap<String, u64>,
    generated: u64,
    suppressed: u64,
    last_keyword: Option<String>,
}
impl KeywordReplies {
    pub fn new(config: KeywordConfig, active: bool) -> Self {
        Self {
            config,
            active,
            seen: HashMap::new(),
            keyword_at: HashMap::new(),
            reply_at: HashMap::new(),
            generated: 0,
            suppressed: 0,
            last_keyword: None,
        }
    }
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }
    pub fn status(&self) -> Value {
        json!({"enabled":self.config.enabled,"active":self.active&&self.config.enabled,"rules":self.config.rules.len(),
            "cooldown_seconds":self.config.cooldown_seconds,"generated":self.generated,"suppressed":self.suppressed,"last_keyword":self.last_keyword})
    }
    pub fn ingest(
        &mut self,
        now: u64,
        platform: &str,
        event: &ProbeEvent,
        own_text: bool,
        actor: Option<&str>,
    ) -> Option<KeywordDraft> {
        if !self.config.enabled || !self.active {
            return None;
        }
        let p = &event.payload;
        let valid_source = event.source_kind == "network"
            && match platform {
                "wechat_channels" => {
                    event.event_type == "chat.keyword_candidate"
                        && p["decoder"] == "wechat.msg_list.keyword.v1"
                        && p["message_type"] == 1
                        && p["keyword_live"] == true
                }
                "douyin" => {
                    event.event_type == "chat.received"
                        && p["decoder"] == "douyin.pb.v1"
                        && p["live_eligible"] == true
                }
                _ => false,
            };
        let (Some(user), Some(id), Some(content), Some(observed)) = (
            event.user_id.as_deref(),
            event.native_event_id.as_deref(),
            p["content"].as_str(),
            p["observed_at_ms"].as_u64(),
        ) else {
            return None;
        };
        if !valid_source
            || !valid_user_id(user)
            || user.trim().is_empty()
            || !valid_id(id)
            || id.trim().is_empty()
            || content.is_empty()
            || content.chars().count() > 256
            || content.chars().any(char::is_control)
            || p["content_truncated"] == true
            || observed.abs_diff(chrono::Utc::now().timestamp_millis().max(0) as u64) > 30_000
        {
            return None;
        }
        if own_text || actor == Some(user) || self.config.rules.iter().any(|r| r.reply == content) {
            self.suppressed += 1;
            return None;
        }
        self.seen.retain(|_, at| now.saturating_sub(*at) < 600_000);
        let key = (user.into(), id.into());
        if self.seen.contains_key(&key) || self.seen.len() >= 10_000 {
            self.suppressed += 1;
            return None;
        }
        self.seen.insert(key, now);
        let text = content.to_lowercase();
        let cooldown = self.config.cooldown_seconds * 1000;
        self.keyword_at
            .retain(|_, at| now.saturating_sub(*at) < cooldown);
        self.reply_at
            .retain(|_, at| now.saturating_sub(*at) < cooldown);
        for rule in &self.config.rules {
            for keyword in &rule.keywords {
                let normalized = keyword.to_lowercase();
                let matched = match rule.match_mode {
                    MatchMode::Contains => text.contains(&normalized),
                    MatchMode::Exact => text == normalized,
                };
                if !matched {
                    continue;
                }
                // The first configured matching rule wins, even during cooldown.
                if self.keyword_at.contains_key(&normalized)
                    || self.reply_at.contains_key(&rule.reply)
                {
                    self.suppressed += 1;
                    return None;
                }
                self.keyword_at.insert(normalized, now);
                self.reply_at.insert(rule.reply.clone(), now);
                self.generated += 1;
                self.last_keyword = Some(keyword.clone());
                return Some(KeywordDraft {
                    text: rule.reply.clone(),
                    keyword: keyword.clone(),
                    event_id: event.event_id.clone(),
                    native_event_id: id.into(),
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> KeywordConfig {
        serde_json::from_value(json!({"enabled":true,"rules":[
            {"keywords":["胎记","手臂"],"reply":"手臂是胎记"},
            {"keywords":["印记"],"reply":"手臂是胎记"},
            {"keywords":["Hello"],"reply":"你好","match_mode":"exact"}]}))
        .unwrap()
    }
    fn event(id: &str, text: &str, platform: &str) -> ProbeEvent {
        serde_json::from_value(json!({"seq":1,"event_id":id,"event_type":if platform=="douyin"{"chat.received"}else{"chat.keyword_candidate"},"source_kind":"network","user_id":"viewer","native_event_id":id,"payload":{"content":text,"decoder":if platform=="douyin"{"douyin.pb.v1"}else{"wechat.msg_list.keyword.v1"},"message_type":1,"keyword_live":true,"live_eligible":true,"observed_at_ms":chrono::Utc::now().timestamp_millis()}})).unwrap()
    }
    #[test]
    fn cooldown_shared_by_reply_and_keyword_without_replaying_duplicates() {
        for platform in ["douyin", "wechat_channels"] {
            let mut k = KeywordReplies::new(config(), true);
            assert_eq!(
                k.ingest(
                    0,
                    platform,
                    &event("a", "这里是胎记吗", platform),
                    false,
                    None
                )
                .unwrap()
                .text,
                "手臂是胎记"
            );
            assert!(
                k.ingest(59999, platform, &event("b", "印记", platform), false, None)
                    .is_none()
            );
            assert!(
                k.ingest(60000, platform, &event("c", "印记", platform), false, None)
                    .is_some()
            );
            assert!(
                k.ingest(120000, platform, &event("c", "印记", platform), false, None)
                    .is_none()
            );
            assert!(
                k.ingest(120001, platform, &event("d", "胎记", platform), false, None)
                    .is_some()
            );
        }
    }
    #[test]
    fn own_echo_pause_and_invalid_sources_do_not_trigger() {
        let mut k = KeywordReplies::new(config(), true);
        for (id, text, own, actor) in [
            ("a", "手臂是胎记", false, None),
            ("b", "胎记", true, None),
            ("c", "胎记", false, Some("viewer")),
        ] {
            assert!(
                k.ingest(
                    0,
                    "wechat_channels",
                    &event(id, text, "wechat_channels"),
                    own,
                    actor
                )
                .is_none()
            );
        }
        let mut stale = event("d", "胎记", "wechat_channels");
        stale.payload["observed_at_ms"] = json!(0);
        assert!(
            k.ingest(0, "wechat_channels", &stale, false, None)
                .is_none()
        );
        let mut candidate = event("e", "胎记", "wechat_channels");
        candidate.event_type = "chat.candidate".into();
        assert!(
            k.ingest(0, "wechat_channels", &candidate, false, None)
                .is_none()
        );
        k.set_active(false);
        assert!(
            k.ingest(
                0,
                "wechat_channels",
                &event("f", "胎记", "wechat_channels"),
                false,
                None
            )
            .is_none()
        );
        k.set_active(true);
        assert!(
            k.ingest(
                0,
                "wechat_channels",
                &event("g", "胎记", "wechat_channels"),
                false,
                None
            )
            .is_some()
        );
    }
    #[test]
    fn exact_match_case_and_independent_replies_and_validation() {
        let mut k = KeywordReplies::new(config(), true);
        assert!(
            k.ingest(0, "douyin", &event("a", "say hello", "douyin"), false, None)
                .is_none()
        );
        assert_eq!(
            k.ingest(0, "douyin", &event("b", "HELLO", "douyin"), false, None)
                .unwrap()
                .text,
            "你好"
        );
        assert!(
            k.ingest(0, "douyin", &event("c", "胎记", "douyin"), false, None)
                .is_some()
        );
        let mut cfg = config();
        assert!(cfg.validate(80).is_ok());
        cfg.rules[1].keywords.push("胎记".into());
        assert!(cfg.validate(80).is_err());
        cfg = config();
        cfg.rules[0].reply = "x".repeat(81);
        assert!(cfg.validate(80).is_err());
    }
}
