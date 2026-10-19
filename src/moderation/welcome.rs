//! Bounded welcome text and batching for page-bound WeChat arrival candidates.
use super::{
    engine::RulesConfig,
    events::{SupporterTitle, clean_text},
    protocol::ProbeEvent,
    templates::Template,
};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashMap;

pub(crate) fn render(
    template: &Template,
    users: &[(String, String)],
    limit: usize,
) -> Result<String> {
    for shown in (0..=users.len()).rev() {
        let names = users
            .iter()
            .take(shown)
            .map(|(_, name)| clean_text(name, 12))
            .collect::<Vec<_>>()
            .join("、");
        let nickname = if shown == users.len() {
            names
        } else if shown == 0 {
            format!("新来的{}位朋友", users.len())
        } else {
            format!("{names}等{}位朋友", users.len())
        };
        let text = template.render(&HashMap::from([("nickname", nickname)]))?;
        if text.chars().count() <= limit || shown == 0 {
            return Ok(text);
        }
    }
    unreachable!("nonempty welcome batch")
}

pub(crate) struct CandidateWelcomes {
    enabled: bool,
    supporter_enabled: bool,
    supporter_template: Template,
    supporter_cooldown: HashMap<String, u64>,
    template: Template,
    limit: usize,
    due: Option<u64>,
    users: Vec<(String, String)>,
    cooldown: HashMap<String, u64>,
    sources: Vec<Value>,
}
impl CandidateWelcomes {
    pub fn new(config: &RulesConfig) -> Result<Self> {
        Ok(Self {
            enabled: config.welcome_enabled,
            supporter_enabled: config.supporter_welcome_enabled,
            supporter_template: Template::parse(
                &config.supporter_welcome_template,
                &["nickname", "title"],
            )?,
            supporter_cooldown: HashMap::new(),
            template: Template::parse(&config.welcome_template, &["nickname"])?,
            limit: config.max_text_chars.min(80),
            due: None,
            users: vec![],
            cooldown: HashMap::new(),
            sources: vec![],
        })
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn remaining_ms(&self, now: u64) -> Option<u64> {
        self.due.map(|due| due.saturating_sub(now))
    }
    pub fn len(&self) -> usize {
        self.users.len()
    }
    pub fn remove(&mut self, user: &str) {
        self.users.retain(|(id, _)| id != user);
    }
    pub fn cancel(&mut self, now: u64, reason: &str) -> Vec<Value> {
        self.due = None;
        self.sources.clear();
        if self.users.is_empty() {
            return vec![];
        }
        self.users.clear();
        vec![
            json!({"kind":"welcome_candidate_cancelled","at_ms":now,"reason":reason,
            "send_chat":false,"fill_input":false,"live_eligible":false,"room_verified":false}),
        ]
    }
    pub fn ingest(&mut self, now: u64, event: &ProbeEvent) -> Vec<Value> {
        let mut logs = self.advance(now.saturating_sub(1));
        if (!self.enabled && !self.supporter_enabled)
            || event.source_kind != "network"
            || event.payload["decoder"] != "wechat.msg_list.arrival.candidate.v1"
            || event.payload["message_type"] != 10005
            || event.payload["arrival_text_match"] != true
        {
            return logs;
        }
        let (Some(user), Some(name), Some(id), Some(observed)) = (
            event.user_id.as_deref(),
            event.nickname.as_deref(),
            event.native_event_id.as_deref(),
            event.payload["observed_at_ms"].as_u64(),
        ) else {
            return logs;
        };
        let wall = chrono::Utc::now().timestamp_millis().max(0) as u64;
        if !super::events::valid_user_id(user)
            || user.trim().is_empty()
            || !super::events::valid_id(id)
            || id.trim().is_empty()
            || name.trim().is_empty()
            || name.chars().any(char::is_control)
            || name.chars().count() > 128
            || observed.abs_diff(wall) > 30_000
        {
            return logs;
        }
        if self.supporter_enabled {
            if let Some(title) = event
                .payload
                .get("supporter_title")
                .and_then(|value| serde_json::from_value::<SupporterTitle>(value.clone()).ok())
            {
                self.remove(user);
                self.supporter_cooldown
                    .retain(|_, at| now.saturating_sub(*at) < 60_000);
                if self.supporter_cooldown.contains_key(user)
                    || self.supporter_cooldown.len() >= 2000
                {
                    return logs;
                }
                let text = self
                    .supporter_template
                    .render(&HashMap::from([
                        ("nickname", clean_text(name, 12)),
                        ("title", title.label().into()),
                    ]))
                    .expect("validated supporter template");
                if text.chars().count() > self.limit {
                    logs.push(json!({"kind":"welcome_candidate_skipped","at_ms":now,"reason":"text_too_long",
                        "supporter_title":title,"user_id":user,"send_chat":false,"fill_input":false}));
                    return logs;
                }
                self.supporter_cooldown.insert(user.into(), now);
                self.cooldown
                    .retain(|_, at| now.saturating_sub(*at) < 60_000);
                if self.cooldown.len() < 2000 {
                    self.cooldown.insert(user.into(), now);
                }
                logs.push(json!({"kind":"welcome_candidate_draft","at_ms":now,"due_at_ms":now,
                    "expires_at_ms":now.saturating_add(10_000),"text":text,"user_id":user,"user_count":1,
                    "supporter_title":title,"reason":"supporter_arrival","confidence":"candidate",
                    "live_eligible":false,"room_verified":false,"send_chat":false,"fill_input":false,
                    "source_messages":[{"seq":event.seq,"event_id":event.event_id,"native_event_id":id}]}));
                return logs;
            }
        }
        if !self.enabled {
            return logs;
        }
        self.cooldown
            .retain(|_, at| now.saturating_sub(*at) < 60_000);
        if self
            .supporter_cooldown
            .get(user)
            .is_some_and(|at| now.saturating_sub(*at) < 60_000)
            || self.cooldown.contains_key(user)
            || self.cooldown.len() >= 2000
            || self.users.len() >= 500
        {
            return logs;
        }
        self.due.get_or_insert(now.saturating_add(30_000));
        self.users.push((user.into(), name.into()));
        self.cooldown.insert(user.into(), now);
        self.sources
            .push(json!({"seq":event.seq,"event_id":event.event_id,"native_event_id":id}));
        logs.push(json!({"kind":"welcome_candidate_accepted","at_ms":now,"user_id":user,
            "due_at_ms":self.due,"confidence":"candidate","live_eligible":false,"room_verified":false}));
        logs
    }
    pub fn advance(&mut self, now: u64) -> Vec<Value> {
        let Some(due) = self.due.filter(|due| *due <= now) else {
            return vec![];
        };
        self.due = None;
        let users = std::mem::take(&mut self.users);
        let sources = std::mem::take(&mut self.sources);
        if users.is_empty() {
            return vec![];
        }
        let text = render(&self.template, &users, self.limit).expect("validated welcome template");
        let reason = if now >= due.saturating_add(10_000) {
            Some("welcome_expired")
        } else if text.chars().count() > self.limit {
            Some("text_too_long")
        } else {
            None
        };
        vec![
            json!({"kind":if reason.is_some(){"welcome_candidate_skipped"}else{"welcome_candidate_draft"},
            "at_ms":now,"due_at_ms":due,"expires_at_ms":due.saturating_add(10_000),
            "text":text,"user_count":users.len(),"source_messages":sources,"reason":reason,
            "confidence":"candidate","live_eligible":false,"room_verified":false,"send_chat":false,"fill_input":false}),
        ]
    }
}
