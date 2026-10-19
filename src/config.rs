//! User-editable configuration for rooms and authorized platform sessions.
//!
//! Applying a configuration writes sessions to the operating-system credential
//! store. They are never copied into SQLite.

use crate::{
    credentials::{CredentialStore, credential_key},
    model::{Platform, RecordingTask, TaskStatus},
    store::TaskStore,
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Deserialize;
use std::{collections::HashSet, path::Path};
use url::Url;
use uuid::Uuid;

pub const EXAMPLE_CONFIG: &str = r#"# Do not commit this file: `auth.session` is an authorized login secret.
# Applying it stores sessions in your system credential store, never in SQLite.

[defaults]
output_root = "data/recordings"

[[auth]]
platform = "douyin"
account = "default"
session = "" # paste your authorized cookie or session here

[[rooms]]
platform = "douyin"
room_url = "https://live.douyin.com/example"
name = "Creator"
account = "default"
# output = "data/recordings/douyin/creator" # optional

# WeChat Channels rooms are added here too. Capturing still requires explicit
# consent when running `wechat-capture` because it changes the local proxy/CA.
[[rooms]]
platform = "wechat_channels"
# No public room URL is needed. `key` is a stable, local task identifier.
key = "wechat-creator"
name = "WeChat Creator"
"#;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct FileConfig {
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub auth: Vec<AuthConfig>,
    #[serde(default)]
    pub rooms: Vec<RoomConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default = "default_output_root")]
    pub output_root: std::path::PathBuf,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            output_root: default_output_root(),
        }
    }
}

fn default_output_root() -> std::path::PathBuf {
    "data/recordings".into()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    pub platform: FilePlatform,
    #[serde(default = "default_account")]
    pub account: String,
    pub session: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomConfig {
    pub platform: FilePlatform,
    /// Required for Douyin; optional for WeChat Channels, where a local `key`
    /// can identify a room that is opened manually after capture starts.
    pub room_url: Option<Url>,
    pub key: Option<String>,
    pub name: String,
    #[serde(default = "default_account")]
    pub account: String,
    pub output: Option<std::path::PathBuf>,
}

fn default_account() -> String {
    "default".into()
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilePlatform {
    Douyin,
    WechatChannels,
}

impl From<FilePlatform> for Platform {
    fn from(value: FilePlatform) -> Self {
        match value {
            FilePlatform::Douyin => Self::Douyin,
            FilePlatform::WechatChannels => Self::WeChatChannels,
        }
    }
}

pub struct ApplySummary {
    pub sessions_saved: usize,
    pub rooms_added: usize,
    pub rooms_updated: usize,
}

pub fn load(path: &Path) -> Result<FileConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading configuration {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing configuration {}", path.display()))
}

/// Imports sessions and creates or updates room tasks. Existing rooms absent
/// from the file are deliberately left alone, preserving their history.
pub fn apply<C: CredentialStore>(
    config: FileConfig,
    store: &TaskStore,
    credentials: &C,
) -> Result<ApplySummary> {
    let mut auth_keys = HashSet::new();
    for auth in &config.auth {
        validate_account(&auth.account)?;
        if auth.session.trim().is_empty() {
            bail!(
                "auth session for {}:{} is empty",
                Platform::from(auth.platform),
                auth.account
            );
        }
        let platform: Platform = auth.platform.into();
        if !auth_keys.insert((platform, auth.account.as_str())) {
            bail!("duplicate auth entry for {}:{}", platform, auth.account);
        }
    }
    let mut room_keys = HashSet::new();
    for room in &config.rooms {
        validate_account(&room.account)?;
        if room.name.trim().is_empty() {
            bail!("room name cannot be empty");
        }
        let platform: Platform = room.platform.into();
        let room_url = configured_room_url(room)?;
        if !room_keys.insert((platform, room_url.to_string())) {
            bail!("duplicate room entry for {room_url}");
        }
    }

    for auth in &config.auth {
        credentials.put(
            &credential_key(auth.platform.into(), &auth.account),
            auth.session.trim(),
        )?;
    }

    let existing = store.list()?;
    let mut summary = ApplySummary {
        sessions_saved: config.auth.len(),
        rooms_added: 0,
        rooms_updated: 0,
    };
    for room in config.rooms {
        let platform: Platform = room.platform.into();
        let room_url = configured_room_url(&room)?;
        let output_dir = room
            .output
            .unwrap_or_else(|| config.defaults.output_root.join(platform.as_str()));
        if let Some(task) = existing
            .iter()
            .find(|task| task.platform == platform && task.room_url == room_url)
        {
            store.update_definition(task.id, &room.name, &output_dir)?;
            summary.rooms_updated += 1;
        } else {
            let now = Utc::now();
            store.insert(&RecordingTask {
                id: Uuid::new_v4(),
                platform,
                room_url,
                display_name: room.name,
                status: TaskStatus::Idle,
                output_dir,
                created_at: now,
                updated_at: now,
            })?;
            summary.rooms_added += 1;
        }
    }
    Ok(summary)
}

fn validate_account(account: &str) -> Result<()> {
    if account.trim().is_empty() {
        bail!("account cannot be empty");
    }
    Ok(())
}

fn configured_room_url(room: &RoomConfig) -> Result<Url> {
    if let Some(url) = &room.room_url {
        return Ok(url.clone());
    }
    if !matches!(room.platform, FilePlatform::WechatChannels) {
        bail!("room_url is required for Douyin rooms");
    }
    let key = room
        .key
        .as_deref()
        .context("a WeChat Channels room without room_url requires a `key`")?;
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("WeChat Channels room key must use only letters, numbers, '-' or '_'");
    }
    Url::parse(&format!(
        "https://channels.weixin.qq.com/live-archive/{key}"
    ))
    .context("creating internal WeChat Channels capture identifier")
}
