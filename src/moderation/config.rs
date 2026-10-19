//! Versioned YAML entry point. Validate before opening storage or starting a proxy.
use super::{
    engine::{Engine, RulesConfig},
    protocol::ProbeScope,
    scheduler::SchedulerConfig,
    service::ProbeOptions,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Observe,
    RulesPreview,
    SchedulerPreview,
    FillOnly,
    CandidateFillOnly,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModerationConfig {
    pub version: u32,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub sending: SendingConfig,
    #[serde(default)]
    pub delivery: super::delivery::DeliveryConfig,
    #[serde(default)]
    pub nickname_mapping: super::nicknames::NicknameMapping,
    #[serde(default)]
    pub timed_messages: super::timed::TimedConfig,
    #[serde(default)]
    pub keyword_replies: super::keywords::KeywordConfig,
    #[serde(default)]
    pub recording: super::recording::RecordingConfig,
    pub room: RoomConfig,
    #[serde(default)]
    pub candidate_preview: super::candidate_preview::CandidatePreviewConfig,
    #[serde(default)]
    pub injection: InjectionConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub rules: RuleSettings,
    #[serde(default)]
    pub limits: LimitsConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub duration_seconds: u64,
    /// Optional one-off draft disables automatic rule filling for this run.
    #[serde(default)]
    pub fill_test_text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SendingConfig {
    pub automatic: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomConfig {
    pub platform: String,
    pub adapter: String,
    pub url: Url,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InjectionConfig {
    pub system_proxy: bool,
    pub tls_passthrough: bool,
    pub tls_only: bool,
    pub http2: bool,
    pub proxy_port: u16,
    pub mitmdump: PathBuf,
}
impl Default for InjectionConfig {
    fn default() -> Self {
        Self {
            system_proxy: false,
            tls_passthrough: false,
            tls_only: false,
            http2: true,
            proxy_port: 2024,
            mitmdump: "mitmdump".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    pub retention: super::retention::RetentionPolicy,
    pub raw_messages: bool,
    pub directory: PathBuf,
}
impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            retention: Default::default(),
            raw_messages: true,
            directory: "data/moderation-probe".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Rule {
    pub enabled: bool,
    pub template: Option<String>,
}
impl Default for Rule {
    fn default() -> Self {
        Self {
            enabled: true,
            template: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GiftRule {
    pub enabled: bool,
    pub show_quantity: bool,
    pub template: Option<String>,
    pub overflow_template: Option<String>,
    pub emojis: Vec<String>,
    pub variation: super::gift_text::GiftVariation,
}
impl Default for GiftRule {
    fn default() -> Self {
        Self {
            enabled: true,
            show_quantity: true,
            template: None,
            overflow_template: None,
            emojis: super::gift_text::default_emojis(),
            variation: Default::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpgradeRule {
    pub enabled: bool,
    pub fansclub: bool,
    pub honor: bool,
    pub wealth: bool,
    pub allow_inferred_levels: bool,
    pub template: Option<String>,
}
impl Default for UpgradeRule {
    fn default() -> Self {
        Self {
            enabled: true,
            fansclub: true,
            honor: true,
            wealth: true,
            allow_inferred_levels: false,
            template: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuleSettings {
    pub gift: GiftRule,
    pub welcome: Rule,
    pub supporter_welcome: Rule,
    pub fansclub_joined: Rule,
    pub upgrade: UpgradeRule,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    pub max_text_chars: usize,
    pub max_active_users: usize,
    pub max_gift_items: usize,
    pub max_pending_actions: usize,
    pub max_dedupe_entries: usize,
}
impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_text_chars: 80,
            max_active_users: 500,
            max_gift_items: 100,
            max_pending_actions: 20,
            max_dedupe_entries: 10_000,
        }
    }
}

impl ModerationConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let mut source = String::new();
        File::open(path)
            .context("open moderation YAML")?
            .take(65_537)
            .read_to_string(&mut source)
            .context("read moderation YAML")?;
        if source.len() > 65_536 {
            bail!("moderation YAML exceeds 64 KiB");
        }
        let mut config: Self = serde_yaml_ng::from_str(&source).context("parse moderation YAML")?;
        let base = path
            .canonicalize()?
            .parent()
            .context("configuration has no parent")?
            .to_path_buf();
        config
            .nickname_mapping
            .resolve(&base, &config.room.platform)?;
        config.recording.resolve(&base);
        config.validate()?;
        if config.storage.directory.is_relative() {
            config.storage.directory = base.join(&config.storage.directory);
        }
        // Bare executable names use PATH. Explicit relative executable paths use the YAML directory.
        if config.injection.mitmdump.is_relative()
            && config.injection.mitmdump.components().count() > 1
        {
            config.injection.mitmdump = base.join(&config.injection.mitmdump);
        }
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported moderation configuration version (expected 1)");
        }
        self.storage.retention.validate()?;
        self.delivery.validate()?;
        self.recording.validate()?;
        self.timed_messages.validate(self.limits.max_text_chars)?;
        self.keyword_replies.validate(self.limits.max_text_chars)?;
        if self.keyword_replies.enabled
            && (!self.sending.automatic
                || !matches!(self.mode, Mode::FillOnly | Mode::CandidateFillOnly)
                || self.fill_test_text.is_some()
                || self.injection.tls_only
                || self.injection.tls_passthrough)
        {
            bail!(
                "keyword_replies requires automatic sending in an injection fill mode without test text or TLS diagnostics"
            );
        }
        if self.timed_messages.enabled
            && (!self.sending.automatic
                || !matches!(self.mode, Mode::FillOnly | Mode::CandidateFillOnly)
                || self.fill_test_text.is_some()
                || self.injection.tls_only
                || self.injection.tls_passthrough)
        {
            bail!(
                "timed_messages requires automatic sending in an injection fill mode without test text or TLS diagnostics"
            );
        }
        super::nicknames::validate(&self.nickname_mapping.users)?;
        let scope = ProbeScope::from_room_url(self.room.url.clone())?;
        if self.room.platform != scope.platform() || self.room.adapter != scope.adapter() {
            bail!("platform/adapter does not match the supported page environment");
        }
        if scope.platform() == "wechat_channels"
            && !matches!(self.mode, Mode::Observe | Mode::CandidateFillOnly)
        {
            bail!("wechat_channels.desktop_probe currently supports mode=observe only");
        }
        if self.mode == Mode::CandidateFillOnly
            && (scope.platform() != "wechat_channels" || !self.candidate_preview.enabled)
        {
            bail!(
                "candidate_fill_only requires enabled WeChat candidate_preview and an explicit target"
            );
        }
        if self.injection.system_proxy && scope.platform() != "wechat_channels" {
            bail!("system_proxy is currently supported only for the WeChat desktop adapter");
        }
        if self.injection.tls_passthrough && scope.platform() != "wechat_channels" {
            bail!("tls_passthrough is a WeChat desktop diagnostic option only");
        }
        if self.injection.tls_only && scope.platform() != "wechat_channels" {
            bail!("tls_only is a WeChat desktop diagnostic option only");
        }
        if self.injection.tls_only && self.injection.tls_passthrough {
            bail!("tls_only and tls_passthrough cannot be enabled together");
        }
        self.candidate_preview.validate(
            scope.platform(),
            !self.injection.tls_only && !self.injection.tls_passthrough,
        )?;
        if self.injection.proxy_port == 0 {
            bail!("proxy_port must be 1..65535");
        }
        if self.injection.mitmdump.as_os_str().is_empty()
            || self.storage.directory.as_os_str().is_empty()
        {
            bail!("mitmdump and storage.directory must not be empty");
        }
        if self.duration_seconds > 3600 {
            bail!("duration_seconds must be 0..3600 (0 waits for Ctrl-C)");
        }
        if matches!(self.mode, Mode::FillOnly | Mode::CandidateFillOnly)
            && self.limits.max_text_chars > 80
        {
            bail!("fill_only requires limits.max_text_chars <= 80");
        }
        if let Some(text) = &self.fill_test_text {
            if !matches!(self.mode, Mode::FillOnly | Mode::CandidateFillOnly) {
                bail!("fill_test_text requires mode=fill_only");
            }
            super::fill::validate_text(text)?;
        }
        if self.sending.automatic
            && (!matches!(self.mode, Mode::FillOnly | Mode::CandidateFillOnly)
                || self.fill_test_text.is_some()
                || self.injection.tls_only
                || self.injection.tls_passthrough)
        {
            bail!(
                "automatic sending requires an injection fill mode without fill_test_text or TLS diagnostics"
            );
        }
        Engine::new(self.rules_config()).context("invalid moderation rules")?;
        self.scheduler.validate()?;
        Ok(())
    }

    pub fn rules_config(&self) -> RulesConfig {
        let defaults = RulesConfig::default();
        RulesConfig {
            gift_enabled: self.rules.gift.enabled,
            gift_show_quantity: self.rules.gift.show_quantity,
            gift_emojis: self.rules.gift.emojis.clone(),
            gift_variation: self.rules.gift.variation.clone(),
            gift_nicknames: self.nickname_mapping.users.clone(),
            welcome_enabled: self.rules.welcome.enabled,
            supporter_welcome_enabled: self.rules.supporter_welcome.enabled,
            supporter_welcome_template: self
                .rules
                .supporter_welcome
                .template
                .clone()
                .unwrap_or(defaults.supporter_welcome_template),
            fansclub_joined_enabled: self.rules.fansclub_joined.enabled,
            fansclub_upgrade_enabled: self.rules.upgrade.enabled && self.rules.upgrade.fansclub,
            honor_upgrade_enabled: self.rules.upgrade.enabled && self.rules.upgrade.honor,
            wealth_upgrade_enabled: self.rules.upgrade.enabled && self.rules.upgrade.wealth,
            gift_template: self
                .rules
                .gift
                .template
                .clone()
                .unwrap_or(defaults.gift_template),
            overflow_template: self
                .rules
                .gift
                .overflow_template
                .clone()
                .unwrap_or_else(|| {
                    if self.room.platform == "wechat_channels" {
                        "感谢礼物飘屏".into()
                    } else {
                        defaults.overflow_template
                    }
                }),
            welcome_template: self
                .rules
                .welcome
                .template
                .clone()
                .unwrap_or(defaults.welcome_template),
            fansclub_joined_template: self
                .rules
                .fansclub_joined
                .template
                .clone()
                .unwrap_or(defaults.fansclub_joined_template),
            upgrade_template: self
                .rules
                .upgrade
                .template
                .clone()
                .unwrap_or(defaults.upgrade_template),
            allow_inferred_levels: self.rules.upgrade.allow_inferred_levels,
            max_text_chars: self.limits.max_text_chars,
            max_active_users: self.limits.max_active_users,
            max_gift_items: self.limits.max_gift_items,
            max_pending_actions: self.limits.max_pending_actions,
            max_dedupe_entries: self.limits.max_dedupe_entries,
        }
    }

    pub fn probe_options(&self) -> ProbeOptions {
        ProbeOptions {
            retention: self.storage.retention.clone(),
            delivery: self.delivery.clone(),
            timed_messages: self.timed_messages.clone(),
            keyword_replies: self.keyword_replies.clone(),
            recording: self.recording.clone(),
            raw_messages: self.storage.raw_messages,
            automatic_send: self.sending.automatic,
            candidate_preview: self.candidate_preview.clone(),
            system_proxy: self.injection.system_proxy,
            tls_passthrough: self.injection.tls_passthrough,
            tls_only: self.injection.tls_only,
            http2: Some(self.injection.http2),
            candidate_fill_only: self.mode == Mode::CandidateFillOnly,
            rules_preview: matches!(
                self.mode,
                Mode::RulesPreview | Mode::SchedulerPreview | Mode::FillOnly
            ),
            scheduler_preview: matches!(self.mode, Mode::SchedulerPreview | Mode::FillOnly),
            fill_preview: self.mode == Mode::FillOnly,
            fill_test_text: self.fill_test_text.clone(),
            rules_config: self.rules_config(),
            scheduler_config: self.scheduler.clone(),
        }
    }
}
