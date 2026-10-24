//! Per-room behavior overrides; transport, accounts and output paths stay shared.
use super::{config::ModerationConfig, engine::RulesConfig, service::ProbeOptions};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug)]
pub struct RoomProfile {
    pub rules: RulesConfig,
    pub timed: super::timed::TimedConfig,
    pub keywords: super::keywords::KeywordConfig,
    pub likes: super::likes::LikeConfig,
    pub candidate: super::candidate_preview::CandidatePreviewConfig,
}
impl RoomProfile {
    pub fn from_config(c: &ModerationConfig) -> Self {
        Self {
            rules: c.rules_config(),
            timed: c.timed_messages.clone(),
            keywords: c.keyword_replies.clone(),
            likes: c.likes.clone(),
            candidate: c.candidate_preview.clone(),
        }
    }
    pub fn apply(&self, options: &ProbeOptions) -> ProbeOptions {
        ProbeOptions {
            rules_config: self.rules.clone(),
            timed_messages: self.timed.clone(),
            keyword_replies: self.keywords.clone(),
            likes: self.likes.clone(),
            candidate_preview: self.candidate.clone(),
            ..options.clone()
        }
    }
    pub fn boot(&self, value: &mut Value) {
        value["keyword_replies"] = json!(self.keywords);
        value["likes"] = json!(self.likes);
        value["timed_messages"] = json!({"enabled":self.timed.enabled,"min_comments":self.timed.min_comments,"count_scope":self.timed.count_scope});
    }
}
fn merge(base: &mut serde_yaml_ng::Value, patch: serde_yaml_ng::Value) {
    match (base, patch) {
        (serde_yaml_ng::Value::Mapping(base), serde_yaml_ng::Value::Mapping(patch)) => {
            for (key, value) in patch {
                merge(base.entry(key).or_insert(serde_yaml_ng::Value::Null), value);
            }
        }
        (base, patch) => *base = patch,
    }
}
pub fn load(
    source: &str,
    base: &Path,
    platform: &str,
    profiles: BTreeMap<String, serde_yaml_ng::Value>,
) -> Result<BTreeMap<String, RoomProfile>> {
    if profiles.len() > 32 {
        bail!("room_profiles allows at most 32 rooms per platform");
    }
    let mut defaults: serde_yaml_ng::Value = serde_yaml_ng::from_str(source)?;
    defaults
        .as_mapping_mut()
        .context("configuration must be a mapping")?
        .remove("room_profiles");
    profiles.into_iter().map(|(id,patch)| {
        let valid=if platform=="douyin" { !id.is_empty() && id.len()<=20 && !id.starts_with('0') && id.bytes().all(|b|b.is_ascii_digit()) }
            else { super::events::valid_user_id(&id) && !id.trim().is_empty() && id.trim()==id };
        if !valid { bail!("invalid room_profiles room ID: {id}"); }
        let mapping=patch.as_mapping().context("room profile must be a mapping")?;
        for key in mapping.keys() {
            if !matches!(key.as_str(),Some("rules"|"timed_messages"|"keyword_replies"|"nickname_mapping"|"likes"|"limits")) {
                bail!("room_profiles.{id}: only rules, timed_messages, keyword_replies, nickname_mapping, likes and limits can be overridden");
            }
        }
        let mut merged=defaults.clone();merge(&mut merged,patch);
        let mut config:ModerationConfig=serde_yaml_ng::from_value(merged).with_context(||format!("room_profiles.{id}"))?;
        if platform=="wechat_channels" && config.candidate_preview.enabled {
            config.candidate_preview.recipient_user_id=Some(id.clone());
        }
        config.prepare(base).with_context(||format!("room_profiles.{id}"))?;
        Ok((id,RoomProfile::from_config(&config)))
    }).collect()
}
