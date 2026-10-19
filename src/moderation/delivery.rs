//! Evidence of a network chat echo, separate from a local UI submission.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeliveryConfig {
    pub actor_user_id: Option<String>,
}
impl DeliveryConfig {
    pub fn validate(&self) -> Result<()> {
        if self
            .actor_user_id
            .as_ref()
            .is_some_and(|id| !super::events::valid_user_id(id) || id.trim().is_empty())
        {
            bail!("invalid delivery.actor_user_id");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryReport {
    pub v: u32,
    pub connection_id: String,
    pub binding_id: String,
    pub page_url: String,
    pub action_id: String,
    pub state: String,
    pub source: String,
    pub message_id: Option<String>,
    pub sender_user_id: Option<String>,
    pub local_message_id: Option<String>,
}
impl DeliveryReport {
    pub fn validate(&self, platform: &str, actor: Option<&str>) -> Result<()> {
        if self.v != 1
            || !super::protocol::identifier(&self.action_id, 128)
            || !matches!(self.state.as_str(), "echo_observed" | "unconfirmed")
        {
            bail!("invalid delivery report");
        }
        if self.state == "unconfirmed" {
            if self.source != "timeout"
                || self.message_id.is_some()
                || self.sender_user_id.is_some()
                || self.local_message_id.is_some()
            {
                bail!("invalid unconfirmed evidence");
            }
            return Ok(());
        }
        let id = self
            .message_id
            .as_deref()
            .filter(|id| super::events::valid_id(id) && !id.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing message identity"))?;
        let sender = self
            .sender_user_id
            .as_deref()
            .filter(|id| super::events::valid_user_id(id) && !id.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing sender identity"))?;
        if actor.is_some_and(|expected| expected != sender) {
            bail!("delivery actor mismatch");
        }
        match platform {
            "wechat_channels" if self.source == "wechat_native_poll" => {
                let local = self
                    .local_message_id
                    .as_deref()
                    .filter(|s| super::events::valid_id(s) && !s.trim().is_empty())
                    .ok_or_else(|| anyhow::anyhow!("missing local correlation"))?;
                if local == id {
                    bail!("local-only message is not a network echo");
                }
            }
            "douyin"
                if self.source == "douyin_ws"
                    && actor.is_some()
                    && self.local_message_id.is_none() => {}
            _ => bail!("unsupported delivery evidence"),
        }
        Ok(())
    }
}
