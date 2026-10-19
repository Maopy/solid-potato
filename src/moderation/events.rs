//! Platform-neutral inputs to the offline core. Adapters must establish these
//! identities and quantity semantics before enabling a rule.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub platform: String,
    pub room_id: String,
    pub live_session_id: String,
    pub actor_user_id: String,
    pub recipient_user_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct User {
    pub id: Option<String>,
    pub nickname: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    Verified,
    Inferred,
    #[default]
    Unverified,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuantityMode {
    Incremental,
    Cumulative,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Gift {
    #[serde(default)]
    pub unit_value: Option<u64>,
    pub gift_id: String,
    pub name: String,
    pub recipient_user_id: String,
    pub quantity_mode: QuantityMode,
    pub quantity: Option<u64>,
    pub combo_id: Option<String>,
    #[serde(default)]
    pub combo_end: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Level {
    pub kind: String,
    pub name: String,
    pub new_level: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SupporterTitle {
    SuperFan,
    StarGuardian,
}
impl SupporterTitle {
    pub fn label(self) -> &'static str {
        match self {
            Self::SuperFan => "超粉",
            Self::StarGuardian => "星守护",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum EventKind {
    #[serde(rename = "gift.observed")]
    Gift(Gift),
    #[serde(rename = "member.entered")]
    Welcome,
    #[serde(rename = "supporter.entered")]
    SupporterEntered(SupporterTitle),
    #[serde(rename = "fansclub.joined")]
    FansclubJoined,
    #[serde(rename = "level.snapshot")]
    LevelSnapshot(Level),
    #[serde(rename = "level.upgraded")]
    LevelUpgraded(Level),
    #[serde(rename = "probe.gap")]
    Gap,
    #[serde(rename = "room.ended")]
    RoomEnded,
    Unsupported,
}

impl<'de> Deserialize<'de> for EventKind {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let kind = value
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| serde::de::Error::custom("event type must be a string"))?;
        let data = value
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        match kind {
            "gift.observed" => serde_json::from_value(data)
                .map(Self::Gift)
                .map_err(serde::de::Error::custom),
            "member.entered" => Ok(Self::Welcome),
            "supporter.entered" => serde_json::from_value(data)
                .map(Self::SupporterEntered)
                .map_err(serde::de::Error::custom),
            "fansclub.joined" => Ok(Self::FansclubJoined),
            "level.snapshot" => serde_json::from_value(data)
                .map(Self::LevelSnapshot)
                .map_err(serde::de::Error::custom),
            "level.upgraded" => serde_json::from_value(data)
                .map(Self::LevelUpgraded)
                .map_err(serde::de::Error::custom),
            "probe.gap" => Ok(Self::Gap),
            "room.ended" => Ok(Self::RoomEnded),
            _ => Ok(Self::Unsupported),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CoreEvent {
    pub event_id: String,
    pub scope: Scope,
    #[serde(default)]
    pub native_event_id: Option<String>,
    pub source: String,
    #[serde(default)]
    pub evidence: Evidence,
    pub user: Option<User>,
    #[serde(flatten)]
    pub kind: EventKind,
}

pub fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 128 && !value.chars().any(char::is_control)
}

/// Platform user identities may exceed message/gift identifier lengths.
pub fn valid_user_id(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 256 && !value.chars().any(char::is_control)
}

pub fn clean_text(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(limit)
        .collect::<String>()
        .trim()
        .to_owned()
}

impl CoreEvent {
    pub fn validate(&self) -> Result<()> {
        let s = &self.scope;
        for id in [
            &self.event_id,
            &self.source,
            &s.platform,
            &s.room_id,
            &s.live_session_id,
            &s.actor_user_id,
            &s.recipient_user_id,
        ] {
            if !valid_id(id) {
                bail!("invalid event/scope identifier");
            }
        }
        if self.native_event_id.as_ref().is_some_and(|s| !valid_id(s)) {
            bail!("invalid native identifier");
        }
        if let Some(user) = &self.user {
            if user.id.as_ref().is_some_and(|s| !valid_id(s)) || user.nickname.chars().count() > 256
            {
                bail!("invalid user");
            }
        }
        if serde_json::to_vec(self)?.len() > 4096 {
            bail!("normalized event too large");
        }
        if let EventKind::Gift(g) = &self.kind {
            if !valid_id(&g.gift_id)
                || !valid_id(&g.recipient_user_id)
                || clean_text(&g.name, 128).is_empty()
                || g.name.chars().count() > 128
                || g.combo_id.as_ref().is_some_and(|s| !valid_id(s))
            {
                bail!("invalid gift");
            }
            match g.quantity_mode {
                QuantityMode::Cumulative if g.quantity.is_none() || g.combo_id.is_none() => {
                    bail!("cumulative gift needs combo ID and count")
                }
                QuantityMode::Incremental if g.quantity.is_none_or(|n| n == 0) && !g.combo_end => {
                    bail!("incremental gift needs positive count")
                }
                QuantityMode::Unknown if g.quantity.is_some() => {
                    bail!("unknown quantity must not have a count")
                }
                _ => {}
            }
        }
        if let EventKind::LevelSnapshot(l) | EventKind::LevelUpgraded(l) = &self.kind {
            if !valid_id(&l.kind)
                || l.name.chars().count() > 64
                || l.new_level == 0
                || l.new_level > 100_000
            {
                bail!("invalid level");
            }
        }
        Ok(())
    }
}
