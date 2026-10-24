//! Name-only drafts from unverified candidates. Never produces executable actions.
use super::{
    events::{valid_id, valid_user_id},
    protocol::ProbeEvent,
};
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet, VecDeque};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CandidatePreviewConfig {
    pub enabled: bool,
    pub recipient_user_id: Option<String>,
}
impl CandidatePreviewConfig {
    pub fn validate(&self, platform: &str, injection: bool) -> Result<()> {
        if self.enabled && (platform != "wechat_channels" || !injection) {
            bail!("candidate_preview requires injected WeChat observation");
        }
        if self.enabled
            && !self
                .recipient_user_id
                .as_deref()
                .is_some_and(|id| valid_user_id(id) && !id.trim().is_empty())
        {
            bail!("candidate_preview requires an explicit valid recipient_user_id");
        }
        if self
            .recipient_user_id
            .as_deref()
            .is_some_and(|id| !valid_user_id(id))
        {
            bail!("invalid candidate_preview.recipient_user_id");
        }
        Ok(())
    }
}
struct Pending {
    unit_value: Option<u64>,
    nickname: String,
    names: Vec<String>,
    ids: HashSet<String>,
    due: u64,
    overflow: bool,
    sources: Vec<Value>,
    last_at: u64,
}
pub struct CandidatePreview {
    target: String,
    upgrades: super::upgrade::CandidateUpgrades,
    welcomes: super::welcome::CandidateWelcomes,
    seen: HashSet<(String, String)>,
    pending: BTreeMap<String, Pending>,
    counters: BTreeMap<String, u64>,
    recent: VecDeque<Value>,
    paused: bool,
    gift_text: super::gift_text::GiftText,
    max_text_chars: usize,
}
impl CandidatePreview {
    pub fn new(target: String) -> Self {
        Self::with_format(
            target,
            &super::engine::RulesConfig {
                overflow_template: "感谢礼物飘屏".into(),
                ..Default::default()
            },
        )
        .expect("default format")
    }
    pub fn with_format(target: String, config: &super::engine::RulesConfig) -> Result<Self> {
        let max_text_chars = config.max_text_chars.min(80);
        Ok(Self {
            upgrades: super::upgrade::CandidateUpgrades::new(config)?,
            target,
            welcomes: super::welcome::CandidateWelcomes::new(config)?,
            seen: HashSet::new(),
            pending: BTreeMap::new(),
            counters: BTreeMap::new(),
            recent: VecDeque::new(),
            paused: false,
            gift_text: super::gift_text::GiftText::new(
                &config.gift_template,
                &config.overflow_template,
                &config.gift_emojis,
                max_text_chars,
            )?
            .with_variation(&config.gift_variation)?
            .with_nicknames(&config.gift_nicknames)?,
            max_text_chars,
        })
    }
    fn note(&mut self, reason: &str) {
        *self.counters.entry(reason.into()).or_default() += 1;
    }
    fn remember(&mut self, record: Value) -> Value {
        self.recent.push_back(record.clone());
        if self.recent.len() > 5 {
            self.recent.pop_front();
        }
        record
    }
    pub fn status(&self, now: u64) -> Value {
        json!({"enabled":true,"recipient_user_id":self.target,"paused":self.paused,
            "pending_users":self.pending.len(),"pending_welcome_users":self.welcomes.len(),"welcome_enabled":self.welcomes.enabled(),"welcome_remaining_ms":self.welcomes.remaining_ms(now),"pending_truncated":self.pending.len()>10,
            "pending":self.pending.iter().take(10).map(|(user,p)|json!({"user_id":user,"nickname":p.nickname,
                "remaining_ms":p.due.saturating_sub(now),"candidate_messages":p.ids.len()})).collect::<Vec<_>>(),
            "counters":self.counters,"recent":self.recent.iter().map(|r| {
                let mut record=r.clone(); record.as_object_mut().expect("record").remove("source_messages"); record
            }).collect::<Vec<_>>(),"send_chat":false,"fill_input":false,
            "room_verified":false,"quantity_mode":"unknown"})
    }
    fn cancel_user(&mut self, now: u64, user: &str, reason: &str) -> Option<Value> {
        let p = self.pending.remove(user)?;
        self.note(reason);
        Some(self.remember(json!({"kind":"gift_candidate_cancelled","at_ms":now,"reason":reason,
            "user_id":user,"recipient_user_id":self.target,"source_messages":p.sources,
            "candidate_messages":p.ids.len(),"send_chat":false,"fill_input":false,"live_eligible":false,"room_verified":false,"quantity_mode":"unknown","confidence":"candidate"})))
    }
    fn triggered(&mut self, mut records: Vec<Value>, event: &ProbeEvent) -> Vec<Value> {
        for record in &mut records {
            record["trigger_event"] = json!({"seq":event.seq,"event_id":event.event_id,"native_event_id":event.native_event_id});
            // Replace the matching recent summary with the enriched audit.
            if let Some(recent) = self.recent.iter_mut().rev().find(|r| {
                r["at_ms"] == record["at_ms"]
                    && r["user_id"] == record["user_id"]
                    && r["reason"] == record["reason"]
            }) {
                *recent = record.clone();
            }
        }
        records
    }
    pub fn cancel(&mut self, now: u64, reason: &str) -> Vec<Value> {
        let users: Vec<_> = self.pending.keys().cloned().collect();
        let mut logs: Vec<_> = users
            .into_iter()
            .filter_map(|u| self.cancel_user(now, &u, reason))
            .collect();
        logs.extend(self.welcomes.cancel(now, reason));
        logs
    }
    pub fn ingest(&mut self, now: u64, event: &ProbeEvent) -> Vec<Value> {
        if event.event_type == "probe.status"
            && event.payload["stage"] == "wechat_collection_started"
        {
            self.paused = false;
            return vec![];
        }
        if event.event_type == "probe.gap" {
            let records = self.cancel(now, "observation_gap");
            return self.triggered(records, event);
        }
        if event.event_type == "probe.status"
            && matches!(
                event.payload["stage"].as_str(),
                Some("wechat_collection_paused" | "document_stopped")
            )
        {
            self.paused = true;
            let records = self.cancel(now, "observation_paused");
            return self.triggered(records, event);
        }
        if event.event_type == "room.ended" {
            self.paused = true;
            return self.cancel(now, "room_ended");
        }
        if event.event_type == "level.candidate" {
            if self.paused { return vec![]; }
            let records = self.upgrades.ingest(now, event, &self.target);
            return records.into_iter().map(|record| self.remember(record)).collect();
        }
        if event.event_type == "member.candidate" {
            return if self.paused
                || event
                    .user_id
                    .as_ref()
                    .is_some_and(|u| self.pending.contains_key(u))
            {
                vec![]
            } else {
                let records = self.welcomes.ingest(now, event);
                records
                    .into_iter()
                    .map(|record| self.remember(record))
                    .collect()
            };
        }
        if !matches!(
            event.event_type.as_str(),
            "gift.candidate" | "gift.candidate_revision"
        ) {
            return vec![];
        }
        let (Some(user), Some(id)) = (event.user_id.as_deref(), event.native_event_id.as_deref())
        else {
            self.note("invalid_candidate");
            return vec![];
        };
        if !valid_user_id(user) || !valid_id(id) || user.trim().is_empty() || id.trim().is_empty() {
            self.note("invalid_candidate");
            return vec![];
        }
        let mut growth_revision = false;
        if event.event_type == "gift.candidate_revision" {
            let p = &event.payload;
            if p["recipient_user_id"].as_str() != Some(self.target.as_str()) {
                self.note("revision_only");
                if self.pending.get(user).is_some_and(|p| p.ids.contains(id)) {
                    let records = self
                        .cancel_user(now, user, "target_changed")
                        .into_iter()
                        .collect();
                    return self.triggered(records, event);
                }
                return vec![];
            }
            let growth = p["previous_gift_id"] == p["gift_id"]
                && (p["reward_product_count"]
                    .as_u64()
                    .zip(p["previous_reward_product_count"].as_u64())
                    .is_some_and(|(a, b)| b > 0 && a > b)
                    || p["combo_product_count"]
                        .as_u64()
                        .zip(p["previous_combo_product_count"].as_u64())
                        .is_some_and(|(a, b)| b > 0 && a > b));
            if !growth {
                self.note("revision_only");
                return vec![];
            }
            if !self.seen.contains(&(user.into(), id.into())) {
                self.note("revision_unmatched");
                return vec![];
            }
            growth_revision = true;
            self.note("revision_activity");
        }
        let p = &event.payload;
        let Some(name) = p["gift_name"].as_str() else {
            self.note("invalid_candidate");
            return vec![];
        };
        if !matches!(
            event.event_type.as_str(),
            "gift.candidate" | "gift.candidate_revision"
        ) || event.source_kind != "network"
            || p["decoder"] != "wechat.app20009.reward.candidate.v1"
            || p["message_type"] != 20009
            || !p["gift_id"].as_str().is_some_and(valid_id)
            || !p["reward_product_count"]
                .as_u64()
                .is_some_and(|n| n > 0 && n <= 1_000_000)
            || name.trim().is_empty()
            || name.chars().count() > 64
            || name.chars().any(char::is_control)
        {
            self.note("invalid_candidate");
            return vec![];
        }
        if self.paused {
            self.note("observation_paused_candidate");
            return vec![];
        }
        let Some(target) = p["recipient_user_id"]
            .as_str()
            .filter(|id| valid_user_id(id) && !id.trim().is_empty())
        else {
            self.note("target_missing");
            return vec![];
        };
        if target != self.target {
            self.note("target_mismatch");
            return vec![];
        }
        if !growth_revision && self.seen.contains(&(user.into(), id.into())) {
            self.note("duplicate");
            return vec![];
        }
        if self.seen.len() >= 10_000 {
            self.note("dedupe_limit");
            return vec![];
        }
        if !self.pending.contains_key(user) && self.pending.len() >= 500 {
            self.note("user_limit");
            return vec![];
        }
        if self.pending.get(user).is_some_and(|p| p.ids.len() >= 100) {
            self.seen.insert((user.into(), id.into()));
            let records = self
                .cancel_user(now, user, "window_limit")
                .into_iter()
                .collect();
            return self.triggered(records, event);
        }
        // A delayed poll must not merge two windows separated by >5s.
        let logs = self.advance(now.saturating_sub(1));
        self.seen.insert((user.into(), id.into()));
        self.note("accepted");
        self.welcomes.remove(user);
        let pending = self.pending.entry(user.into()).or_insert_with(|| Pending {
            nickname: event
                .nickname
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| "这位观众".into()),
            names: vec![],
            unit_value: None,
            ids: HashSet::new(),
            due: now,
            overflow: false,
            sources: vec![],
            last_at: now,
        });
        pending.unit_value = pending
            .unit_value
            .max(super::gift_text::valid_unit_value(p["unit_value"].as_u64()));
        pending.due = now.saturating_add(5000);
        pending.ids.insert(id.into());
        pending.last_at = now;
        pending
            .sources
            .push(json!({"seq":event.seq,"event_id":event.event_id,"native_event_id":id}));
        if p["gift_name_truncated"] == true {
            pending.overflow = true;
            pending.names.clear();
        }
        if !pending.names.iter().any(|n| n == name) && !pending.overflow {
            pending.names.push(name.into());
            pending.overflow = pending
                .names
                .iter()
                .map(|n| n.chars().count())
                .sum::<usize>()
                > 30
                || p["gift_name_truncated"] == true;
            if pending.overflow {
                pending.names.clear();
            }
        }
        logs
    }
    pub fn advance(&mut self, now: u64) -> Vec<Value> {
        let ready: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| p.due <= now)
            .map(|(u, _)| u.clone())
            .collect();
        let mut logs: Vec<_> = ready
            .into_iter()
            .map(|user| {
                let p = self.pending.remove(&user).expect("pending");
                let text = self.gift_text.render_for(&user,super::events::clean_text(&p.nickname,12),p.names.join("、"),p.overflow,p.unit_value).expect("validated template values");
                if text.chars().count()>self.max_text_chars {
                    self.note("text_too_long");
                    return self.remember(json!({"kind":"gift_candidate_skipped","at_ms":now,"user_id":user,"recipient_user_id":self.target,"reason":"text_too_long","send_chat":false,"fill_input":false}));
                }
                self.note("draft_generated");
                self.remember(
                    json!({"kind":"gift_candidate_draft","at_ms":now,"user_id":user,
                "recipient_user_id":self.target,"text":text,"max_unit_value":p.unit_value,"wording_basis":"highest_unit_value","candidate_messages":p.ids.len(),
                "quantity_mode":"unknown","confidence":"candidate","live_eligible":false,
                "room_verified":false,"send_chat":false,"fill_input":false,
                "source_messages":p.sources,"last_candidate_at_ms":p.last_at,"due_at_ms":p.due}),
                )
            })
            .collect();
        for record in self.welcomes.advance(now) {
            logs.push(self.remember(record));
        }
        logs
    }
}
