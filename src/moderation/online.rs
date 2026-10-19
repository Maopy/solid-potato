//! Volatile observation only: one isolated engine per authenticated document.
//! No verified actor, persisted recovery, commands or delivery claims.
use super::{
    engine::{Audit, Engine, RulesConfig},
    events::*,
    protocol::ProbeEvent,
    scheduler::{PreviewScheduler, SchedulerConfig},
};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::collections::{BTreeSet, VecDeque};

pub struct OnlinePreview {
    engine: Engine,
    scopes: BTreeSet<Scope>,
    page_room: String,
    session: String,
    recent: VecDeque<Audit>,
    normalized: u64,
    planned: u64,
    skipped: u64,
    last_warning: Option<String>,
    dropped: u64,
    paused: bool,
    last_reason: Option<String>,
    scheduler: Option<PreviewScheduler>,
}
impl OnlinePreview {
    pub fn new(page_room: String, session: String) -> Self {
        Self::with_rules(page_room, session, RulesConfig::default()).expect("valid defaults")
    }
    pub fn with_rules(page_room: String, session: String, rules: RulesConfig) -> Result<Self> {
        Ok(Self {
            engine: Engine::new(rules)?,
            scopes: BTreeSet::new(),
            page_room,
            session,
            recent: VecDeque::new(),
            normalized: 0,
            planned: 0,
            skipped: 0,
            last_warning: None,
            dropped: 0,
            paused: false,
            last_reason: None,
            scheduler: None,
        })
    }
    pub fn enable_scheduler_preview(&mut self) {
        self.scheduler = Some(PreviewScheduler::default());
    }
    pub fn configure_scheduler(&mut self, config: SchedulerConfig) -> Result<()> {
        self.scheduler = Some(PreviewScheduler::new(config)?);
        Ok(())
    }
    fn collect(&mut self, logs: Vec<Audit>) -> Vec<Audit> {
        for log in &logs {
            if log.kind == "planned" {
                self.planned += 1;
            }
            if log.kind == "paused" {
                self.paused = true;
            }
            if log.kind == "skipped" {
                self.skipped += 1;
                self.last_warning = Some(log.reason.clone());
            }
            if log.kind == "paused" || log.kind == "suppressed" && !self.paused {
                self.last_reason = Some(log.reason.clone());
            }
            if self.recent.len() >= 100 {
                self.recent.pop_front();
                self.dropped += 1;
            }
            self.recent.push_back(log.clone());
        }
        logs
    }
    fn scope(&self, e: &ProbeEvent) -> Scope {
        Scope {
            platform: "douyin".into(),
            room_id: e
                .payload
                .get("internal_room_id")
                .and_then(Value::as_str)
                .unwrap_or(&self.page_room)
                .into(),
            live_session_id: self.session.clone(),
            actor_user_id: "observe-only".into(),
            recipient_user_id: e
                .payload
                .get("recipient_user_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .into(),
        }
    }
    fn normalize(&self, e: &ProbeEvent) -> Result<CoreEvent> {
        if e.source_kind != "network"
            || e.payload["decoder"] != "douyin.pb.v1"
            || e.payload["live_eligible"] != true
        {
            bail!("unverified_or_historical_input");
        }
        for id in [
            e.user_id.as_deref(),
            e.native_event_id.as_deref(),
            e.payload["internal_room_id"].as_str(),
            e.payload["recipient_user_id"].as_str(),
        ] {
            let Some(id) = id else {
                bail!("missing_normalized_identity");
            };
            if id.len() > 20
                || id.starts_with('0')
                || !id.bytes().all(|b| b.is_ascii_digit())
                || id.is_empty()
            {
                bail!("invalid_native_identity");
            }
        }
        let timestamp = e.payload["observed_at_ms"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing_observation_time"))?;
        let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
        if timestamp.abs_diff(now) > 30_000 {
            bail!("stale_observation");
        }
        let kind = match e.event_type.as_str() {
            "gift.observed" => EventKind::Gift(Gift {
                unit_value: super::gift_text::valid_unit_value(e.payload["unit_value"].as_u64()),
                gift_id: e.payload["gift_id"].as_str().unwrap_or_default().into(),
                name: e.payload["gift_name"].as_str().unwrap_or_default().into(),
                recipient_user_id: e.payload["recipient_user_id"]
                    .as_str()
                    .unwrap_or_default()
                    .into(),
                quantity_mode: serde_json::from_value(e.payload["quantity_mode"].clone())?,
                quantity: e.payload["quantity"].as_u64(),
                combo_id: e.payload["combo_id"].as_str().map(str::to_owned),
                combo_end: e.payload["combo_end"].as_bool().unwrap_or(false),
            }),
            "member.entered" => {
                if let Some(title) = e.payload.get("supporter_title") {
                    EventKind::SupporterEntered(serde_json::from_value(title.clone())?)
                } else {
                    EventKind::Welcome
                }
            }
            "fansclub.joined" => EventKind::FansclubJoined,
            "level.upgraded" | "level.snapshot" => {
                let level = Level {
                    kind: e.payload["level_kind"].as_str().unwrap_or_default().into(),
                    name: e.payload["level_name"].as_str().unwrap_or_default().into(),
                    new_level: u32::try_from(e.payload["new_level"].as_u64().unwrap_or(0))?,
                };
                if e.event_type == "level.snapshot" {
                    EventKind::LevelSnapshot(level)
                } else {
                    EventKind::LevelUpgraded(level)
                }
            }
            _ => bail!("unsupported_normalized_type"),
        };
        let result = CoreEvent {
            event_id: e.event_id.clone(),
            scope: self.scope(e),
            native_event_id: e.native_event_id.clone(),
            source: "douyin.pb.v1".into(),
            evidence: Evidence::Verified,
            user: Some(User {
                id: e.user_id.clone(),
                nickname: e.nickname.clone().unwrap_or_default(),
            }),
            kind,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn ingest(&mut self, now: u64, e: &ProbeEvent) -> Result<Vec<Audit>> {
        if matches!(e.event_type.as_str(), "probe.gap" | "room.ended") {
            let reason = e.payload["reason"].as_str().unwrap_or("room_ended");
            return self.pause(now, reason);
        }
        if e.event_type == "probe.skipped" {
            let scope = self.scope(e);
            let reason = e.payload["reason"].as_str().unwrap_or("unsupported_gift");
            let mut logs = Vec::new();
            if e.payload["decoder"] == "douyin.pb.v1"
                && e.payload["message_type"] == "WebcastGiftMessage"
            {
                if let Some(user) = e.user_id.as_deref() {
                    logs.extend(self.engine.discard_user_gifts(
                        now,
                        &scope,
                        user,
                        "unsupported_gift_in_user_window",
                    )?);
                }
            }
            logs.push(Audit {
                at_ms: now,
                scope,
                kind: "skipped".into(),
                reason: reason.into(),
                event_id: Some(e.event_id.clone()),
                action: None,
            });
            return Ok(self.collect(logs));
        }
        if !matches!(
            e.event_type.as_str(),
            "gift.observed"
                | "member.entered"
                | "fansclub.joined"
                | "level.upgraded"
                | "level.snapshot"
        ) {
            return Ok(Vec::new());
        }
        if self.paused {
            return Ok(self.collect(vec![Audit {
                at_ms: now,
                scope: self.scope(e),
                kind: "suppressed".into(),
                reason: "observation_paused_reload_required".into(),
                event_id: Some(e.event_id.clone()),
                action: None,
            }]));
        }
        let event = match self.normalize(e) {
            Ok(event) => event,
            Err(error) => {
                return Ok(self.collect(vec![Audit {
                    at_ms: now,
                    scope: self.scope(e),
                    kind: "suppressed".into(),
                    reason: error.to_string(),
                    event_id: Some(e.event_id.clone()),
                    action: None,
                }]));
            }
        };
        if self.scopes.len() >= 16 && !self.scopes.contains(&event.scope) {
            return self.pause(now, "scope_capacity_gap");
        }
        self.scopes.insert(event.scope.clone());
        self.normalized += 1;
        let logs = self.engine.ingest(now, event)?;
        let reason = logs
            .iter()
            .find(|a| a.kind == "paused")
            .map(|a| a.reason.clone());
        let mut result = self.collect(logs);
        if let Some(reason) = reason {
            result.extend(self.pause(now, &reason)?);
        }
        Ok(result)
    }
    pub fn advance(&mut self, now: u64) -> Result<Vec<Audit>> {
        if self.paused {
            return Ok(Vec::new());
        }
        if let Some(scheduler) = &mut self.scheduler {
            let logs = scheduler.advance(now, &mut self.engine)?;
            return Ok(self.collect(logs));
        }
        let mut logs = self.engine.advance(now)?;
        // In observation mode, a generated preview is the end product. There
        // is no sender queue: finalize its window so subsequent gifts start
        // a fresh batch and previews cannot fill a queue forever.
        for id in self.engine.pending_action_ids() {
            for mut record in self.engine.simulate_dispatch(now, &id)? {
                if record.kind == "dispatch_simulated" {
                    record.kind = "previewed".into();
                    record.reason = "online_preview_completed_without_sending".into();
                }
                logs.push(record);
            }
        }
        Ok(self.collect(logs))
    }
    pub fn pause(&mut self, now: u64, reason: &str) -> Result<Vec<Audit>> {
        self.paused = true;
        self.last_reason = Some(reason.into());
        let mut logs = Vec::new();
        for scope in &self.scopes {
            logs.extend(self.engine.ingest(
                now,
                CoreEvent {
                    event_id: format!("gap:{now}"),
                    scope: scope.clone(),
                    native_event_id: None,
                    source: "bridge".into(),
                    evidence: Evidence::Unverified,
                    user: None,
                    kind: EventKind::Gap,
                },
            )?);
        }
        let result = self.collect(logs);
        self.last_reason = Some(reason.into());
        Ok(result)
    }
    pub fn cancel_pending(&mut self, now: u64) -> Result<Vec<Audit>> {
        let logs = self.engine.cancel_pending(now)?;
        Ok(self.collect(logs))
    }
    pub fn has_pending_work(&self) -> bool {
        self.engine.has_pending_work()
    }
    pub fn status(&self) -> Value {
        json!({"mode":if self.scheduler.is_some() {"online_scheduler_preview"} else {"online_preview"},"scheduler":self.scheduler.as_ref().map(|s| s.status(&self.engine)),"send_chat":false,"actor_verified":false,"normalized_events":self.normalized,"planned":self.planned,"paused":self.paused,"last_reason":self.last_reason,"skipped_messages":self.skipped,"last_warning":self.last_warning,"preview_records_dropped":self.dropped,"recovery":"reload_new_document","completion":"preview_only_no_delivery_claim"})
    }
    pub fn drain(&mut self) -> Vec<Audit> {
        self.recent.drain(..self.recent.len().min(20)).collect()
    }
}
