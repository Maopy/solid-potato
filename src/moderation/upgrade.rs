//! Explicit native WeChat upgrade notices, independent of arrival welcomes.
use super::{
    engine::RulesConfig,
    events::{clean_text, valid_id, valid_user_id},
    protocol::ProbeEvent,
    templates::Template,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub(crate) struct CandidateUpgrades {
    enabled: bool,
    template: Template,
    nicknames: super::nicknames::Nicknames,
    seen: HashSet<(String, String)>,
    limit: usize,
}
impl CandidateUpgrades {
    pub fn new(config: &RulesConfig) -> Result<Self> {
        Ok(Self {
            enabled: config.wealth_upgrade_enabled,
            template: Template::parse(
                &config.upgrade_template,
                &["nickname", "new_level", "level_name"],
            )?,
            nicknames: config.gift_nicknames.clone(),
            seen: HashSet::new(),
            limit: config.max_text_chars.min(80),
        })
    }
    pub fn ingest(&mut self, now: u64, event: &ProbeEvent, target: &str) -> Vec<Value> {
        let p = &event.payload;
        if !self.enabled
            || event.source_kind != "network"
            || p["decoder"] != "wechat.app20031.upgrade.candidate.v1"
            || p["message_type"] != 20031
            || p["upgrade_type"] != 1
            || p["recipient_user_id"].as_str() != Some(target)
        {
            return vec![];
        }
        let (Some(user), Some(id), Some(name), Some(after)) = (
            event.user_id.as_deref(),
            event.native_event_id.as_deref(),
            event.nickname.as_deref(),
            p["new_level"].as_u64(),
        ) else {
            return vec![];
        };
        if !valid_user_id(user)
            || user.trim().is_empty()
            || !valid_id(id)
            || id.trim().is_empty()
            || name.trim().is_empty()
            || name.chars().count() > 128
            || name.chars().any(char::is_control)
            || self.seen.contains(&(user.into(), id.into()))
            || self.seen.len() >= 10000
        {
            return vec![];
        }
        self.seen.insert((user.into(), id.into()));
        let nickname = clean_text(
            self.nicknames.get(user).map(String::as_str).unwrap_or(name),
            32,
        );
        let Ok(text) = self.template.render(&HashMap::from([
            ("nickname", nickname),
            ("new_level", after.to_string()),
            ("level_name", "观众等级".into()),
        ])) else {
            return vec![];
        };
        // Do not send a broken/truncated congratulation if a custom template is too long.
        if text.trim().is_empty() || text.chars().count() > self.limit {
            return vec![];
        }
        vec![
            json!({"kind":"upgrade_candidate_draft","at_ms":now,"due_at_ms":now,
            "expires_at_ms":now.saturating_add(20000),"user_id":user,"recipient_user_id":target,
            "previous_level":p["previous_level"],"new_level":after,"text":text,
            "source_messages":[{"seq":event.seq,"event_id":event.event_id,"native_event_id":id}],
            "send_chat":false,"fill_input":false,"live_eligible":false,"room_verified":false,"confidence":"candidate"}),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn notice() -> ProbeEvent {
        serde_json::from_value(json!({"seq":1,"event_id":"fixture","event_type":"level.candidate",
            "native_event_id":"upgrade","user_id":"user","nickname":"長**","source_kind":"network",
            "payload":{"decoder":"wechat.app20031.upgrade.candidate.v1","message_type":20031,"upgrade_type":1,
            "previous_level":0,"new_level":1,"recipient_user_id":"host",
            "observed_at_ms":chrono::Utc::now().timestamp_millis()}})).unwrap()
    }
    #[test]
    fn invalid_disabled_and_unsupported_notices_never_congratulate() {
        let config = RulesConfig::default();
        for (field, value) in [
            ("new_level", json!("1")),
            ("new_level", json!(1.5)),
            ("upgrade_type", json!(2)),
            ("message_type", json!(1)),
            ("decoder", json!("chat")),
            ("recipient_user_id", json!("other")),
        ] {
            let mut event = notice();
            event.payload[field] = value;
            assert!(
                CandidateUpgrades::new(&config)
                    .unwrap()
                    .ingest(0, &event, "host")
                    .is_empty(),
                "{field}"
            );
        }
        let mut config = config;
        config.wealth_upgrade_enabled = false;
        assert!(
            CandidateUpgrades::new(&config)
                .unwrap()
                .ingest(0, &notice(), "host")
                .is_empty()
        );
        config.upgrade_template = "{unknown}".into();
        assert!(CandidateUpgrades::new(&config).is_err());
    }
}
