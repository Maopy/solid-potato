//! Per-room behavior overrides; transport, accounts and output paths stay shared.
use super::{config::ModerationConfig, engine::RulesConfig, service::ProbeOptions};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Mutex, OnceLock},
};

static OVERLAY_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn overlay_path(config: &Path) -> PathBuf {
    config.with_extension("room-overrides.json")
}
pub(crate) fn config_revision(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn load_synchronized(config: &Path) -> Result<ModerationConfig> {
    let _guard = OVERLAY_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("configuration lock unavailable"))?;
    ModerationConfig::load(config)
}

pub(crate) fn read_rule_overlays(
    config: &Path,
) -> Result<BTreeMap<String, super::bridge::RoomRulesPatch>> {
    let path = overlay_path(config);
    let source = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.into()),
    };
    if source.len() > 65_536 {
        bail!("room overrides exceed 64 KiB");
    }
    let overlays: BTreeMap<String, super::bridge::RoomRulesPatch> =
        serde_json::from_slice(&source)?;
    if overlays.len() > 32 {
        bail!("room overrides allow at most 32 rooms");
    }
    Ok(overlays)
}

fn patch_values(patch: &super::bridge::RoomRulesPatch) -> Result<Value> {
    let mut out = json!({});
    let fields = [
        ("gift_enabled", "gift", "enabled"),
        ("gift_show_quantity", "gift", "show_quantity"),
        ("gift_template", "gift", "template"),
        ("overflow_template", "gift", "overflow_template"),
        ("welcome_enabled", "welcome", "enabled"),
        ("welcome_template", "welcome", "template"),
        ("supporter_welcome_enabled", "supporter_welcome", "enabled"),
        (
            "supporter_welcome_template",
            "supporter_welcome",
            "template",
        ),
        ("fansclub_joined_enabled", "fansclub_joined", "enabled"),
        ("fansclub_joined_template", "fansclub_joined", "template"),
        ("upgrade_template", "upgrade", "template"),
    ];
    let input = serde_json::to_value(patch)?;
    for (source, group, target) in fields {
        if !input[source].is_null() {
            out["rules"][group][target] = input[source].clone();
        }
    }
    if !input["gift_variation_enabled"].is_null() {
        out["rules"]["gift"]["variation"]["enabled"] = input["gift_variation_enabled"].clone();
    }
    for key in ["timed_messages", "keyword_replies", "likes"] {
        if !input[key].is_null() {
            out[key] = input[key].clone();
        }
    }
    Ok(out)
}

fn edit_mapping(mapping: &yaml_edit::Mapping, patch: &Value) -> Result<()> {
    for (key, value) in patch
        .as_object()
        .context("configuration patch must be an object")?
    {
        if value.is_object() {
            if !mapping.contains_key(key.as_str()) {
                mapping.set(key.as_str(), yaml_edit::Mapping::new_pending_block());
            }
            let child = mapping
                .get_mapping(key.as_str())
                .with_context(|| format!("{key} is not a mapping"))?;
            edit_mapping(&child, value)?;
        } else {
            // JSON is valid YAML and preserves scalar types, escaping and Unicode.
            let literal = serde_json::to_string(value)?;
            let node_doc = yaml_edit::Document::from_str(&format!("value: {literal}\n"))
                .map_err(|e| anyhow::anyhow!("cannot encode configuration value: {e}"))?;
            let node = node_doc
                .as_mapping()
                .and_then(|m| m.get("value"))
                .context("cannot encode configuration value")?;
            mapping.set(key.as_str(), node);
        }
    }
    Ok(())
}

pub(crate) fn save_rule_overlay(
    config: &Path,
    room: &str,
    patch: &super::bridge::RoomRulesPatch,
    revision: Option<&str>,
) -> Result<()> {
    let _guard = OVERLAY_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("configuration lock unavailable"))?;
    let original = std::fs::read_to_string(config)?;
    if let Some(revision) = revision {
        use sha2::{Digest, Sha256};
        if format!("{:x}", Sha256::digest(original.as_bytes())) != revision {
            bail!("配置文件已更改，请重新打开设置后再保存");
        }
    }
    let legacy_path = overlay_path(config);
    let legacy = std::fs::read(&legacy_path).ok();
    let mut overlays = read_rule_overlays(config)?;
    let mut combined = serde_json::to_value(overlays.remove(room).unwrap_or_default())?;
    for (key, value) in serde_json::to_value(patch)?
        .as_object()
        .context("invalid configuration patch")?
    {
        if !value.is_null() {
            combined[key] = value.clone();
        }
    }
    overlays.insert(room.to_owned(), serde_json::from_value(combined)?);
    if overlays.len() > 32 {
        bail!("room overrides allow at most 32 rooms");
    }
    let doc = yaml_edit::Document::from_str(&original)
        .map_err(|e| anyhow::anyhow!("cannot edit configuration: {e}"))?;
    let root = doc
        .as_mapping()
        .context("configuration must be a mapping")?;
    if !root.contains_key("room_profiles") {
        root.set("room_profiles", yaml_edit::Mapping::new_pending_block());
    }
    let profiles = root
        .get_mapping("room_profiles")
        .context("room_profiles must be a mapping")?;
    let mut expected: serde_yaml_ng::Value = serde_yaml_ng::from_str(&original)?;
    let expected_root = expected
        .as_mapping_mut()
        .context("configuration must be a mapping")?;
    let expected_profiles = expected_root
        .entry(serde_yaml_ng::Value::String("room_profiles".into()))
        .or_insert_with(|| serde_yaml_ng::Value::Mapping(Default::default()));
    let expected_profiles = expected_profiles
        .as_mapping_mut()
        .context("room_profiles must be a mapping")?;
    for (id, value) in overlays {
        if !profiles.contains_key(id.as_str()) {
            profiles.set(id.as_str(), yaml_edit::Mapping::new_pending_block());
        }
        let target = profiles
            .get_mapping(id.as_str())
            .context("room profile must be a mapping")?;
        let values = patch_values(&value)?;
        edit_mapping(&target, &values)?;
        let expected_room = expected_profiles
            .entry(serde_yaml_ng::Value::String(id))
            .or_insert_with(|| serde_yaml_ng::Value::Mapping(Default::default()));
        merge(expected_room, serde_yaml_ng::to_value(values)?);
    }
    let updated = doc.to_string();
    if updated.len() > 65_536 {
        bail!("configuration exceeds 64 KiB");
    }
    let actual: serde_yaml_ng::Value = serde_yaml_ng::from_str(&updated)?;
    if actual != expected {
        bail!("configuration edit did not preserve existing values");
    }
    let parent = config.parent().context("configuration has no parent")?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(updated.as_bytes())?;
    tmp.as_file().sync_all()?;
    super::config::ModerationConfig::load(tmp.path())
        .context("updated configuration is invalid")?;
    if std::fs::read_to_string(config)? != original || std::fs::read(&legacy_path).ok() != legacy {
        bail!("configuration changed during save; please retry");
    }
    tmp.persist(config).map_err(|e| e.error)?;
    if legacy.is_some() {
        std::fs::remove_file(legacy_path)?;
    }
    Ok(())
}

pub fn migrate_legacy_overlays(config: &Path) -> Result<()> {
    if let Some(room) = read_rule_overlays(config)?.keys().next().cloned() {
        save_rule_overlay(config, &room, &Default::default(), None)?;
    } else if overlay_path(config).is_file() {
        std::fs::remove_file(overlay_path(config))?;
    }
    Ok(())
}

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

#[cfg(test)]
mod persistence_tests {
    use super::*;
    #[test]
    fn save_updates_yaml_preserves_comments_and_migrates_legacy_rooms() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("douyin-auto.yml");
        let original = "version: 1\nmode: fill_only\nsending: {automatic: true}\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/123\n# 用户备注\nroom_profiles:\n  '123':\n    # 主播备注\n    rules:\n      gift:\n        enabled: true # 保留此注释\n      welcome:\n        enabled: true\n";
        std::fs::write(&config, original).unwrap();
        std::fs::write(
            overlay_path(&config),
            r#"{"123":{"gift_enabled":false},"456":{"welcome_enabled":false}}"#,
        )
        .unwrap();
        let patch = super::super::bridge::RoomRulesPatch {
            welcome_enabled: Some(false),
            ..Default::default()
        };
        save_rule_overlay(&config, "123", &patch, None).unwrap();
        let written = std::fs::read_to_string(&config).unwrap();
        assert!(written.contains("# 用户备注"));
        assert!(written.contains("# 主播备注"));
        assert!(written.contains("# 保留此注释"));
        assert!(!overlay_path(&config).exists());
        let loaded = super::super::config::ModerationConfig::load(&config).unwrap();
        assert!(!loaded.resolved_room_profiles["123"].rules.gift_enabled);
        assert!(!loaded.resolved_room_profiles["123"].rules.welcome_enabled);
        assert!(!loaded.resolved_room_profiles["456"].rules.welcome_enabled);
    }
}
