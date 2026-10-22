//! Independent, opt-in page-side like scheduling configuration.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LikeConfig {
    pub enabled: bool,
    pub mode: String,
    pub interval_ms: u64,
    pub every_minutes: u32,
    pub batch_count: u32,
    pub missed: String,
    pub target_selector: Option<String>,
}
impl Default for LikeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "continuous".into(),
            interval_ms: 800,
            every_minutes: 10,
            batch_count: 200,
            missed: "catch_up_once".into(),
            target_selector: None,
        }
    }
}
impl LikeConfig {
    pub fn validate(&self, _platform: &str) -> Result<()> {
        if !matches!(self.mode.as_str(), "continuous" | "clock")
            || !(100..=60000).contains(&self.interval_ms)
            || self.every_minutes == 0
            || self.every_minutes > 60
            || 60 % self.every_minutes != 0
            || !(1..=10000).contains(&self.batch_count)
            || !matches!(self.missed.as_str(), "skip" | "catch_up_once")
            || self.target_selector.as_ref().is_some_and(|s| {
                s.trim().is_empty() || s.len() > 1024 || s.chars().any(char::is_control)
            })
        {
            bail!(
                "invalid likes config: mode=continuous/clock, interval_ms=100..60000, every_minutes must divide 60, batch_count=1..10000, missed=skip/catch_up_once"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_modes_clock_alignment_and_key_interval() {
        let mut c = LikeConfig::default();
        assert!(!c.enabled);
        c.enabled = true;
        c.mode = "clock".into();
        c.validate("douyin").unwrap();
        c.every_minutes = 7;
        assert!(c.validate("douyin").is_err());
        c.every_minutes = 10;
        c.interval_ms = 300;
        c.validate("douyin").unwrap();
        c.mode = "continuous".into();
        c.validate("wechat_channels").unwrap();
        c.batch_count = 0;
        assert!(c.validate("wechat_channels").is_err());
    }
}
