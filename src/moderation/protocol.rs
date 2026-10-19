use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use url::Url;

pub const VERSION: u32 = 1;
pub const PREFIX: &str = "/__live_archive/moderation/v1/";
pub const MAX_BODY: usize = 128 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMessageChunk {
    pub seq: u64,
    pub capture_id: String,
    pub captured_at_ms: u64,
    pub channel: String,
    pub category: String,
    pub encoding: String,
    pub part: u32,
    pub parts: u32,
    pub data: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMessageBatch {
    pub v: u32,
    pub connection_id: String,
    pub binding_id: String,
    pub chunks: Vec<RawMessageChunk>,
}
impl RawMessageChunk {
    pub fn validate(&self) -> Result<()> {
        if self.seq == 0
            || self.seq > i64::MAX as u64
            || self.captured_at_ms > i64::MAX as u64
            || !identifier(&self.capture_id, 128)
            || !identifier(&self.channel, 64)
            || !identifier(&self.category, 64)
            || !matches!(
                self.encoding.as_str(),
                "json_utf8_base64" | "binary_base64" | "gap"
            )
            || self.parts == 0
            || self.parts > 4096
            || self.part >= self.parts
            || self.data.len() > 24576
            || !self
                .data
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=._:-".contains(&b))
        {
            bail!("invalid raw message chunk");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ProbeScope {
    pub room_url: Url,
    pub room_key: String,
}

impl ProbeScope {
    pub fn from_room_url(room_url: Url) -> Result<Self> {
        match room_url.host_str() {
            Some("live.douyin.com") => Self::douyin(room_url),
            Some("channels.weixin.qq.com") => Self::wechat_desktop(room_url),
            _ => bail!("unsupported moderation page host"),
        }
    }

    pub fn wechat_desktop(mut room_url: Url) -> Result<Self> {
        if room_url.scheme() != "https"
            || room_url.host_str() != Some("channels.weixin.qq.com")
            || room_url.port().is_some()
            || !room_url.username().is_empty()
            || room_url.password().is_some()
            || room_url.path().trim_end_matches('/') != "/web/pages/live"
        {
            bail!("WeChat desktop probe requires https://channels.weixin.qq.com/web/pages/live");
        }
        // This identifies a page route, not a verified live room. No rules may execute here.
        room_url.set_query(None);
        room_url.set_fragment(None);
        room_url.set_path("/web/pages/live");
        Ok(Self {
            room_url,
            room_key: "wechat-desktop-unverified".into(),
        })
    }

    pub fn platform(&self) -> &'static str {
        if self.room_url.host_str() == Some("channels.weixin.qq.com") {
            "wechat_channels"
        } else {
            "douyin"
        }
    }

    pub fn adapter(&self) -> &'static str {
        if self.platform() == "wechat_channels" {
            "wechat_channels.desktop_probe"
        } else {
            "douyin.live_page"
        }
    }

    pub fn douyin(mut room_url: Url) -> Result<Self> {
        let room_key = room_url.path().trim_matches('/').to_owned();
        if room_url.scheme() != "https"
            || room_url.host_str() != Some("live.douyin.com")
            || room_url.port().is_some()
            || !room_url.username().is_empty()
            || room_url.password().is_some()
            || room_key.is_empty()
            || room_key.len() > 32
            || !room_key.bytes().all(|b| b.is_ascii_digit())
        {
            bail!("probe requires an HTTPS live.douyin.com numeric room URL");
        }
        room_url.set_query(None);
        room_url.set_fragment(None);
        room_url.set_path(&format!("/{room_key}"));
        Ok(Self { room_url, room_key })
    }

    pub fn matches(&self, value: &str) -> bool {
        Url::parse(value)
            .ok()
            .and_then(|url| Self::from_room_url(url).ok())
            .is_some_and(|scope| {
                scope.platform() == self.platform() && scope.room_key == self.room_key
            })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub v: u32,
    pub ticket: String,
    pub page_url: String,
    pub document_id: String,
    pub adapter_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProbeEvent {
    pub seq: u64,
    pub event_id: String,
    pub event_type: String,
    pub source_kind: String,
    #[serde(default)]
    pub native_event_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub nickname: Option<String>,
    pub payload: Value,
}

impl ProbeEvent {
    pub fn validate(&self) -> Result<()> {
        if self.seq == 0 || !identifier(&self.event_id, 128) || !identifier(&self.event_type, 96) {
            bail!("invalid event identifier");
        }
        if !matches!(self.source_kind.as_str(), "dom" | "network" | "diagnostic") {
            bail!("invalid source kind");
        }
        for (value, limit) in [
            (&self.native_event_id, 128),
            (&self.user_id, 256),
            (&self.nickname, 128),
        ] {
            if let Some(value) = value {
                if value.chars().count() > limit || value.chars().any(char::is_control) {
                    bail!("invalid user or native identifier");
                }
            }
        }
        if serde_json::to_vec(&self.payload)?.len() > 2048 {
            bail!("event payload too large");
        }
        Ok(())
    }
}

pub fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

/// Extend by registering a sanitizer, never by persisting arbitrary responses.
/// Unknown event types are counted and retain only their type/source identity.
pub fn sanitize(event: &ProbeEvent) -> ProbeEvent {
    let mut result = event.clone();
    let permitted: &[&str] = match event.event_type.as_str() {
        "recording.result" => &["request_id", "state", "message"],
        "chat.received" => &["content", "decoder", "live_eligible", "observed_at_ms"],
        "chat.keyword_candidate" | "chat.activity" => &[
            "content",
            "content_truncated",
            "decoder",
            "message_type",
            "keyword_live",
            "observed_at_ms",
            "confidence",
            "live_eligible",
            "room_verified",
        ],
        "chat.candidate" => &[
            "content",
            "content_truncated",
            "message_type",
            "decoder",
            "confidence",
            "live_eligible",
            "room_verified",
        ],
        "member.candidate" => &[
            "supporter_title",
            "message_type",
            "decoder",
            "arrival_text_match",
            "observed_at_ms",
            "confidence",
            "live_eligible",
            "room_verified",
        ],
        "gift.summary" => &[
            "message_type",
            "combo_id",
            "gift_id",
            "unit_value",
            "gift_name",
            "recipient_user_id",
            "decoder",
        ],
        "gift.candidate" | "gift.candidate_revision" => &[
            "combo_id",
            "previous_gift_id",
            "gift_id",
            "unit_value",
            "gift_name",
            "gift_name_truncated",
            "reward_product_count",
            "combo_product_count",
            "is_floatmsg",
            "float_type",
            "recipient_user_id",
            "previous_reward_product_count",
            "previous_combo_product_count",
            "previous_recipient_user_id",
            "previous_is_floatmsg",
            "previous_float_type",
            "reason",
            "quantity_mode",
            "message_type",
            "decoder",
            "confidence",
            "live_eligible",
            "room_verified",
        ],
        "member.entered" => &[
            "supporter_title",
            "label",
            "internal_room_id",
            "recipient_user_id",
            "decoder",
            "observed_at_ms",
            "live_eligible",
        ],
        "gift.observed" => &[
            "gift_id",
            "unit_value",
            "gift_name",
            "quantity_mode",
            "quantity",
            "combo_id",
            "combo_end",
            "internal_room_id",
            "recipient_user_id",
            "decoder",
            "observed_at_ms",
            "live_eligible",
        ],
        "fansclub.joined" => &[
            "internal_room_id",
            "recipient_user_id",
            "decoder",
            "observed_at_ms",
            "live_eligible",
        ],
        "level.upgraded" | "level.snapshot" | "level.observed" => &[
            "level_kind",
            "level_name",
            "new_level",
            "internal_room_id",
            "recipient_user_id",
            "decoder",
            "observed_at_ms",
            "live_eligible",
        ],
        "room.ended" => &["internal_room_id", "recipient_user_id", "decoder"],
        "probe.network" => &["channel", "format", "bytes"],
        "probe.schema" => &["channel", "category", "api_kind", "fields"],
        "probe.message_batch" => &[
            "channel",
            "category",
            "total",
            "sampled",
            "truncated",
            "overflow",
            "type_counts",
        ],
        "probe.message_match" => &["checked", "window_ms", "scan_truncated", "type_counts"],
        "probe.gift_schema" => &["channel", "context", "message_type", "fields"],
        "probe.message_content" => &[
            "message_type",
            "format",
            "content_empty",
            "contact_sender_id_present",
            "contact_nickname_present",
            "fields",
            "envelope_fields",
            "sender_id_present",
            "message_id_present",
            "nickname_present",
            "arrival_text_hint",
            "confidence",
            "live_eligible",
            "room_verified",
        ],
        "probe.application" => &[
            "message_type",
            "format",
            "fields",
            "envelope_fields",
            "gift_hints",
        ],
        "probe.command" => &[
            "command_id",
            "string_fields",
            "json_strings",
            "base64_json_strings",
            "base64_candidates",
            "protobuf_candidates",
            "plain_strings",
            "oversized_strings",
            "gift_hints",
            "fields",
        ],
        "probe.gift_counts" => &[
            "gift_id",
            "gift_name",
            "group_count",
            "repeat_count",
            "combo_count",
            "combo_end",
            "has_group_id",
        ],
        "probe.status" => &["stage"],
        "probe.gap" => &["reason"],
        "probe.skipped" => &[
            "reason",
            "message_type",
            "decoder",
            "internal_room_id",
            "recipient_user_id",
        ],
        _ => &[],
    };
    let mut payload = serde_json::Map::new();
    for key in permitted {
        if let Some(value) = event.payload.get(*key) {
            match value {
                Value::Array(counts)
                    if matches!(
                        event.event_type.as_str(),
                        "probe.message_batch" | "probe.message_match"
                    ) && *key == "type_counts" =>
                {
                    let counts: Vec<Value> = counts
                        .iter()
                        .take(8)
                        .filter_map(|count| {
                            let kind = match count.get("type")? {
                                Value::Null => Value::Null,
                                v if v.as_u64().is_some_and(|n| n <= 65535) => v.clone(),
                                _ => return None,
                            };
                            let mut safe = serde_json::Map::new();
                            safe.insert("type".into(), kind);
                            let fields: &[&str] = if event.event_type == "probe.message_match" {
                                &["matched", "unmatched"]
                            } else {
                                &["count", "text", "json", "opaque", "absent"]
                            };
                            for field in fields {
                                if let Some(n) = count
                                    .get(*field)
                                    .and_then(Value::as_u64)
                                    .filter(|n| *n <= 50)
                                {
                                    safe.insert((*field).into(), json!(n));
                                }
                            }
                            Some(Value::Object(safe))
                        })
                        .collect();
                    payload.insert((*key).into(), Value::Array(counts));
                }
                Value::String(text) => {
                    payload.insert(
                        (*key).into(),
                        json!(
                            text.chars()
                                .filter(|c| !c.is_control())
                                .take(512)
                                .collect::<String>()
                        ),
                    );
                }
                Value::Number(number) if number.as_u64().is_some() => {
                    payload.insert((*key).into(), value.clone());
                }
                Value::Bool(_) => {
                    payload.insert((*key).into(), value.clone());
                }
                Value::Null if *key == "combo_id" || *key == "quantity" => {
                    payload.insert((*key).into(), Value::Null);
                }
                Value::Array(fields)
                    if matches!(
                        event.event_type.as_str(),
                        "probe.schema"
                            | "probe.gift_schema"
                            | "probe.command"
                            | "probe.application"
                            | "probe.message_content"
                    ) && (*key == "fields" || *key == "envelope_fields") =>
                {
                    let fields: Vec<Value> = fields
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|field| {
                            field.len() <= 96
                                && field
                                    .bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b"$_:.[]<>".contains(&b))
                        })
                        .take(24)
                        .map(|field| Value::String(field.to_owned()))
                        .collect();
                    payload.insert((*key).into(), Value::Array(fields));
                }
                _ => {}
            }
        }
    }
    if matches!(
        event.event_type.as_str(),
        "chat.candidate"
            | "chat.keyword_candidate"
            | "chat.activity"
            | "member.candidate"
            | "gift.candidate"
            | "gift.candidate_revision"
    ) {
        // Candidates never become eligible through page-supplied flags.
        payload.insert("live_eligible".into(), json!(false));
        payload.insert("room_verified".into(), json!(false));
        payload.insert("confidence".into(), json!("candidate"));
        if matches!(
            event.event_type.as_str(),
            "gift.candidate" | "gift.candidate_revision"
        ) {
            payload.insert("quantity_mode".into(), json!("unknown"));
        }
    }
    if event.event_type == "probe.message_content" {
        payload.insert("confidence".into(), json!("unknown"));
        payload.insert("live_eligible".into(), json!(false));
        payload.insert("room_verified".into(), json!(false));
        result.user_id = None;
        result.nickname = None;
        result.native_event_id = None;
    }
    result.payload = Value::Object(payload);
    if permitted.is_empty() {
        result.user_id = None;
        result.nickname = None;
        result.native_event_id = None;
    }
    result
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventBatch {
    pub v: u32,
    pub connection_id: String,
    pub binding_id: String,
    pub first_seq: u64,
    pub events: Vec<ProbeEvent>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Poll {
    pub v: u32,
    pub connection_id: String,
    pub binding_id: String,
    pub page_url: String,
    #[serde(default)]
    pub input_ready: Option<bool>,
}
