//! Observation, planning, draft filling and opt-in guarded submission.
pub mod account_budget;
pub mod bridge;
pub mod candidate_preview;
pub mod config;
pub mod control;
pub mod delivery;
pub mod engine;
pub mod events;
pub mod fill;
pub mod gift_report;
pub mod gift_text;
pub mod http;
pub mod inject;
pub mod journal;
pub mod keywords;
pub mod nicknames;
pub mod online;
pub mod protocol;
pub mod recording;
pub mod replay;
pub mod retention;
pub mod scheduler;
pub mod service;
pub mod system_proxy;
pub mod templates;
pub mod timed;
mod welcome;

pub const RAW_CAPTURE: &str = include_str!("../../assets/moderation/sdk/raw.js");

pub const SDK: &str = include_str!("../../assets/moderation/sdk/probe.js");
pub const DOUYIN_ADAPTER: &str = include_str!("../../assets/moderation/adapters/douyin-live.js");
pub const DOUYIN_DECODER: &str = include_str!("../../assets/moderation/adapters/douyin-decoder.js");
pub const DOUYIN_FILL: &str = include_str!("../../assets/moderation/adapters/douyin-fill.js");
pub const WECHAT_DESKTOP: &str = include_str!("../../assets/moderation/adapters/wechat-desktop.js");

pub const WECHAT_FILL: &str = include_str!("../../assets/moderation/adapters/wechat-fill.js");

pub const AUTO_SEND: &str = include_str!("../../assets/moderation/sdk/submit.js");

pub const PANEL: &str = include_str!("../../assets/moderation/sdk/panel.js");
pub const DOUYIN_RECORDING: &str =
    include_str!("../../assets/moderation/adapters/douyin-recording.js");
pub mod dashboard;

pub mod userscript;
