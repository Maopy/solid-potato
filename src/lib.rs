//! Core services for a desktop live-stream recorder.
//!
//! Platform-specific stream discovery is deliberately isolated behind
//! [`platform::StreamResolver`]. Recording only begins after a resolver returns
//! an authorized, directly accessible stream URL.

pub mod capture_service;
pub mod capture_sidecar;
pub mod config;
pub mod credentials;
pub mod doctor;
pub mod douyin;
pub mod download_task_bridge;
pub mod model;
pub mod moderation;
pub mod platform;
pub mod recorder;
pub mod service;
pub mod store;
pub mod wechat;
pub mod wechat_bridge;
pub mod wechat_inject;
pub mod wechat_proxy;
pub mod wechat_runtime;
