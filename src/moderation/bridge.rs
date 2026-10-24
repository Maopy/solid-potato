use super::{
    AUTO_SEND, DOUYIN_ADAPTER, DOUYIN_DECODER, DOUYIN_FILL, DOUYIN_RECORDING, PANEL, RAW_CAPTURE,
    SDK, WECHAT_DESKTOP, WECHAT_FILL,
    engine::{Engine, RulesConfig},
    fill::{FillReport, FillSession, validate_text},
    journal::ProbeJournal,
    online::OnlinePreview,
    protocol::*,
    scheduler::SchedulerConfig,
};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use uuid::Uuid;

struct Ticket {
    profile: Option<super::room_profiles::RoomProfile>,
    deadline: Instant,
}
struct Session {
    speech: super::speech::SpeechSession,
    client_id: Option<String>,
    collecting: bool,
    timed: super::timed::TimedMessages,
    keywords: super::keywords::KeywordReplies,
    token: String,
    binding: String,
    last_seen: Instant,
    acked_seq: u64,
    raw_acked_seq: u64,
    preview: Option<OnlinePreview>,
    candidate_preview: Option<super::candidate_preview::CandidatePreview>,
    fill: Option<FillSession>,
}

pub struct ProbeBridge {
    pub scope: ProbeScope,
    pub relay_token: String,
    pub run_id: String,
    tickets: HashMap<String, Ticket>,
    sessions: HashMap<String, Session>,
    journal: ProbeJournal,
    pub counts: HashMap<String, u64>,
    pub total_events: usize,
    room_profiles: std::collections::BTreeMap<String, super::room_profiles::RoomProfile>,
    candidate_config: super::candidate_preview::CandidatePreviewConfig,
    candidate_fill_only: bool,
    rules_preview: bool,
    scheduler_preview: bool,
    fill_preview: bool,
    automatic_send: bool,
    delivery: super::delivery::DeliveryConfig,
    timed_config: super::timed::TimedConfig,
    likes_config: super::likes::LikeConfig,
    keyword_config: super::keywords::KeywordConfig,
    task_id: Option<String>,
    prefix: String,
    account_budget: Option<std::sync::Arc<std::sync::Mutex<super::account_budget::AccountBudget>>>,
    raw_messages: bool,
    retention: super::retention::RetentionPolicy,
    last_cleanup: Instant,
    fill_test_text: Option<String>,
    rules_config: RulesConfig,
    scheduler_config: SchedulerConfig,
    started: Instant,
    storage_failed: bool,
    draining: bool,
    pub moderation_enabled: bool,
    control_epoch: u64,
    recording_ui: bool,
    unified_ui: bool,
    recording_request: Option<(String, Instant)>,
    recording_result: Option<Value>,
}

fn token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

impl ProbeBridge {
    pub fn new(scope: ProbeScope, journal: ProbeJournal) -> Self {
        let rules_config = super::engine::RulesConfig {
            overflow_template: if scope.platform() == "wechat_channels" {
                "感谢礼物飘屏".into()
            } else {
                RulesConfig::default().overflow_template
            },
            ..Default::default()
        };
        Self {
            scope,
            relay_token: token(),
            run_id: Uuid::new_v4().to_string(),
            tickets: HashMap::new(),
            sessions: HashMap::new(),
            journal,
            counts: HashMap::new(),
            total_events: 0,
            room_profiles: Default::default(),
            candidate_config: Default::default(),
            candidate_fill_only: false,
            rules_preview: false,
            scheduler_preview: false,
            fill_preview: false,
            automatic_send: false,
            delivery: Default::default(),
            timed_config: Default::default(),
            likes_config: Default::default(),
            keyword_config: Default::default(),
            task_id: None,
            prefix: PREFIX.into(),
            account_budget: None,
            raw_messages: false,
            retention: super::retention::RetentionPolicy {
                enabled: false,
                ..Default::default()
            },
            last_cleanup: Instant::now(),
            fill_test_text: None,
            rules_config,
            scheduler_config: SchedulerConfig::default(),
            started: Instant::now(),
            storage_failed: false,
            draining: false,
            moderation_enabled: true,
            control_epoch: 0,
            recording_ui: false,
            unified_ui: false,
            recording_request: None,
            recording_result: None,
        }
    }

    pub fn configure_room_profiles(&mut self, profiles: std::collections::BTreeMap<String, super::room_profiles::RoomProfile>) {
        self.room_profiles = profiles;
    }
    pub fn has_room_profiles(&self) -> bool { !self.room_profiles.is_empty() }
    fn default_room_profile(&self) -> super::room_profiles::RoomProfile {
        super::room_profiles::RoomProfile { rules:self.rules_config.clone(), timed:self.timed_config.clone(),
            keywords:self.keyword_config.clone(), likes:self.likes_config.clone(), candidate:self.candidate_config.clone() }
    }
    pub fn configure_ui(&mut self, recording: bool, unified: bool) {
        self.recording_ui = recording;
        self.unified_ui = unified;
    }
    pub fn control_status(&self) -> Value {
        json!({"enabled":self.moderation_enabled,"epoch":self.control_epoch,"recording":self.recording_ui,
            "connected_pages":self.sessions.values().filter(|s|s.last_seen.elapsed()<Duration::from_secs(15)).count(),"automatic_send":self.automatic_send,"blocked":self.sessions.values().any(|s|s.fill.as_ref().is_some_and(|f|f.status()["blocked"]==true)),"recording_result":if self.recording_request.as_ref().is_some_and(|(_,at)|at.elapsed()>=Duration::from_secs(15)){Some(json!({"state":"failed","message":"直播页未响应，请重新打开并播放直播后重试"}))}else{self.recording_result.clone()}})
    }
    pub fn set_moderation_enabled(&mut self, enabled: bool) -> Result<Value> {
        if self.draining || self.storage_failed {
            bail!("service stopping or storage unavailable");
        }
        if enabled != self.moderation_enabled {
            self.moderation_enabled = enabled;
            self.control_epoch += 1;
            let now = self.now();
            for (connection, session) in &mut self.sessions {
                session.timed.set_active(now, enabled && session.collecting);
                session.keywords.set_active(enabled && session.collecting);
                if !enabled {
                    let fill = session
                        .fill
                        .as_mut()
                        .map(|f| f.cancel_queued(now))
                        .unwrap_or_default();
                    self.journal
                        .append_fill_audits(&self.run_id, connection, &fill)?;
                    let logs = session
                        .preview
                        .as_mut()
                        .map(|p| p.cancel_pending(now))
                        .transpose()?
                        .unwrap_or_default();
                    self.journal
                        .append_audits(&self.run_id, connection, &logs)?;
                    let candidates = session
                        .candidate_preview
                        .as_mut()
                        .map(|p| p.cancel(now, "moderation_disabled"))
                        .unwrap_or_default();
                    self.journal
                        .append_candidate_drafts(&self.run_id, connection, &candidates)?;
                }
            }
        }
        Ok(self.control_status())
    }
    pub fn control(&mut self, bearer: &str, body: Value) -> Result<Value> {
        let c: Poll = serde_json::from_value(body["scope"].clone())?;
        self.authorize(bearer, &c.connection_id, &c.binding_id)?;
        if c.v != VERSION || !self.scope.matches(&c.page_url) {
            bail!("invalid control scope");
        }
        self.set_moderation_enabled(
            body["enabled"]
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("enabled boolean required"))?,
        )
    }
    pub fn claim_like_period(&mut self, bearer: &str, scope: Poll) -> Result<Value> {
        use chrono::Timelike;
        if scope.v != VERSION || !self.scope.matches(&scope.page_url) {
            bail!("invalid like scope");
        }
        self.authorize(bearer, &scope.connection_id, &scope.binding_id)?;
        if self.draining
            || self.scope.platform() != "douyin"
            || !self.likes_config.enabled
            || self.likes_config.mode != "clock"
        {
            bail!("clock likes unavailable");
        }
        let now = chrono::Local::now();
        let start = now
            .with_minute(
                now.minute() / self.likes_config.every_minutes * self.likes_config.every_minutes,
            )
            .and_then(|n| n.with_second(0))
            .and_then(|n| n.with_nanosecond(0))
            .ok_or_else(|| anyhow::anyhow!("invalid local clock period"))?
            .timestamp_millis();
        let client = self.sessions[&scope.connection_id]
            .client_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("stable tab client required"))?;
        let key = format!("{}#{}", self.scope.room_url, client);
        let claimed =
            self.journal
                .claim_like_period(&key, self.likes_config.every_minutes, start)?;
        Ok(
            json!({"claimed":claimed,"period_start_ms":start,"period_end_ms":start+self.likes_config.every_minutes as i64*60000,"batch_count":self.likes_config.batch_count}),
        )
    }
    pub fn request_recording(&mut self) -> Result<Value> {
        if !self.recording_ui || self.draining {
            bail!("recording unavailable");
        }
        if self
            .recording_request
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() < Duration::from_secs(15))
        {
            bail!("录制请求处理中，请等待直播页响应");
        }
        if self
            .sessions
            .values()
            .all(|s| s.last_seen.elapsed() > Duration::from_secs(15))
        {
            bail!("直播页未连接，请先打开直播页");
        }
        let id = Uuid::new_v4().to_string();
        self.recording_request = Some((id.clone(), Instant::now()));
        self.recording_result = Some(json!({"state":"pending","message":"等待直播页响应"}));
        Ok(json!({"request_id":id}))
    }
    pub fn configure_keywords(&mut self, config: super::keywords::KeywordConfig) -> Result<()> {
        if !self.sessions.is_empty() || !self.tickets.is_empty() {
            bail!("configuration is immutable after bootstrap");
        }
        config.validate(80)?;
        self.keyword_config = config;
        Ok(())
    }

    pub fn configure_likes(&mut self, config: super::likes::LikeConfig) -> Result<()> {
        config.validate(self.scope.platform())?;
        self.likes_config = config;
        Ok(())
    }
    pub fn configure_timed(&mut self, config: super::timed::TimedConfig) -> Result<()> {
        if !self.sessions.is_empty() || !self.tickets.is_empty() {
            bail!("configuration is immutable after bootstrap");
        }
        config.validate(80)?;
        self.timed_config = config;
        Ok(())
    }
    pub fn configure_delivery(&mut self, config: super::delivery::DeliveryConfig) -> Result<()> {
        if !self.sessions.is_empty() || !self.tickets.is_empty() {
            bail!("configuration is immutable after bootstrap");
        }
        config.validate()?;
        self.delivery = config;
        Ok(())
    }
    pub fn delivery_result(
        &mut self,
        bearer: &str,
        report: super::delivery::DeliveryReport,
    ) -> Result<Value> {
        self.authorize(bearer, &report.connection_id, &report.binding_id)?;
        if !self.scope.matches(&report.page_url) || self.storage_failed {
            bail!("invalid delivery scope");
        }
        report.validate(
            self.scope.platform(),
            self.delivery.actor_user_id.as_deref(),
        )?;
        self.sessions
            .get_mut(&report.connection_id)
            .and_then(|s| s.fill.as_mut())
            .ok_or_else(|| anyhow::anyhow!("no fill session"))?
            .record_delivery(&report)?;
        if let Err(error) =
            self.journal
                .append_delivery(&self.run_id, &report.connection_id, &report)
        {
            self.storage_failed = true;
            return Err(error);
        }
        let now = self.now();
        let (resumed, logs) = self
            .sessions
            .get_mut(&report.connection_id)
            .and_then(|s| s.fill.as_mut())
            .expect("validated fill session")
            .reconcile_delivery(now, &report);
        if let Err(error) =
            self.journal
                .append_fill_audits(&self.run_id, &report.connection_id, &logs)
        {
            self.storage_failed = true;
            return Err(error);
        }
        Ok(json!({"v":VERSION,"acked_action_id":report.action_id,"submission_resumed":resumed}))
    }
    pub fn configure_task(
        &mut self,
        id: &str,
        budget: std::sync::Arc<std::sync::Mutex<super::account_budget::AccountBudget>>,
    ) -> Result<()> {
        if !self.sessions.is_empty()
            || !self.tickets.is_empty()
            || !identifier(id, 64)
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("invalid task configuration");
        }
        self.task_id = Some(id.into());
        self.prefix = format!("/__live_archive/moderation/v2/tasks/{id}/");
        self.account_budget = Some(budget);
        Ok(())
    }

    pub fn configure_retention(&mut self, policy: super::retention::RetentionPolicy) -> Result<()> {
        if !self.sessions.is_empty() || !self.tickets.is_empty() {
            bail!("configuration is immutable after bootstrap");
        }
        policy.validate()?;
        self.retention = policy;
        Ok(())
    }
    pub fn maintain_journal(&mut self, reclaim: bool) -> Result<()> {
        let now = chrono::Utc::now().timestamp_millis();
        let report = self.journal.cleanup(&self.retention, now, reclaim)?;
        self.last_cleanup = Instant::now();
        if report.raw_captures_deleted > 0 || report.audit_rows_deleted > 0 || report.over_limit {
            eprintln!(
                "live-archive moderation: journal cleanup: {}",
                serde_json::to_string(&report)?
            );
        }
        Ok(())
    }

    pub fn configure_raw_messages(&mut self, enabled: bool) -> Result<()> {
        if !self.sessions.is_empty() || !self.tickets.is_empty() {
            bail!("configuration is immutable after bootstrap");
        }
        self.raw_messages = enabled;
        Ok(())
    }

    pub fn raw_messages(&mut self, bearer: &str, batch: RawMessageBatch) -> Result<Value> {
        self.authorize(bearer, &batch.connection_id, &batch.binding_id)?;
        if !self.raw_messages
            || self.storage_failed
            || batch.v != VERSION
            || batch.chunks.is_empty()
            || batch.chunks.len() > 3
        {
            bail!("raw capture unavailable or invalid batch");
        }
        let acked = self.sessions[&batch.connection_id].raw_acked_seq;
        if batch.chunks[0].seq > acked.saturating_add(1) {
            bail!("raw sequence gap");
        }
        for (index, chunk) in batch.chunks.iter().enumerate() {
            chunk.validate()?;
            if batch.chunks[0].seq.checked_add(index as u64) != Some(chunk.seq) {
                bail!("raw batch sequence mismatch");
            }
        }
        if let Err(error) = self.journal.append_raw(
            &self.run_id,
            &batch.connection_id,
            &batch
                .chunks
                .iter()
                .filter(|c| c.seq > acked)
                .cloned()
                .collect::<Vec<_>>(),
        ) {
            self.storage_failed = true;
            return Err(error);
        }
        let acked = acked.max(batch.chunks.last().unwrap().seq);
        self.sessions
            .get_mut(&batch.connection_id)
            .unwrap()
            .raw_acked_seq = acked;
        Ok(json!({"v":VERSION,"acked_seq":acked}))
    }

    pub fn configure_candidate_preview(
        &mut self,
        config: super::candidate_preview::CandidatePreviewConfig,
    ) -> Result<()> {
        if !self.sessions.is_empty() || !self.tickets.is_empty() {
            bail!("configuration is immutable after bootstrap");
        }
        config.validate(self.scope.platform(), true)?;
        self.candidate_config = config;
        Ok(())
    }

    fn cancel_candidate(&mut self, connection: &str, reason: &str) -> Result<()> {
        let now = self.now();
        let logs = self
            .sessions
            .get_mut(connection)
            .and_then(|s| s.candidate_preview.as_mut())
            .map(|p| p.cancel(now, reason))
            .unwrap_or_default();
        if let Err(error) = self
            .journal
            .append_candidate_drafts(&self.run_id, connection, &logs)
        {
            self.storage_failed = true;
            return Err(error);
        }
        Ok(())
    }
    fn candidate_status(&self, connection: &str) -> Value {
        self.sessions
            .get(connection)
            .and_then(|s| s.candidate_preview.as_ref())
            .map(|p| p.status(self.now()))
            .unwrap_or_else(|| json!({"enabled":false,"send_chat":false,"fill_input":false}))
    }

    fn candidate_drafts(
        &mut self,
        connection: &str,
        now: u64,
        events: &[ProbeEvent],
    ) -> Result<()> {
        if self.draining || !self.moderation_enabled {
            return Ok(());
        }
        let logs = if let Some(preview) = self
            .sessions
            .get_mut(connection)
            .and_then(|s| s.candidate_preview.as_mut())
        {
            let mut logs = Vec::new();
            for event in events {
                logs.extend(preview.ingest(now, event));
            }
            logs.extend(preview.advance(now));
            logs
        } else {
            vec![]
        };
        if let Err(error) = self
            .journal
            .append_candidate_drafts(&self.run_id, connection, &logs)
        {
            self.storage_failed = true;
            return Err(error);
        }
        if self.candidate_fill_only && self.fill_test_text.is_none() {
            let mut fill_logs = Vec::new();
            let session = self.sessions.get_mut(connection).expect("authorized");
            if let Some(fill) = session.fill.as_mut() {
                for log in &logs {
                    if log["kind"] == "gift_candidate_draft"
                        || log["kind"] == "welcome_candidate_draft"
                        || log["kind"] == "upgrade_candidate_draft"
                    {
                        fill_logs.extend(fill.enqueue(
                            now,
                            format!("candidate-{}", Uuid::new_v4()),
                            log["text"].as_str().expect("draft text").into(),
                            log["expires_at_ms"].as_u64().unwrap_or(now + 20_000),
                        ));
                    } else if log["kind"] == "gift_candidate_cancelled"
                        || log["kind"] == "welcome_candidate_cancelled"
                    {
                        fill_logs.extend(fill.cancel(now, "observation_paused"));
                    }
                }
            }
            if let Err(error) =
                self.journal
                    .append_fill_audits(&self.run_id, connection, &fill_logs)
            {
                self.storage_failed = true;
                return Err(error);
            }
        }
        for log in logs {
            println!(
                "live-archive moderation: {}{} {}",
                self.task_id
                    .as_ref()
                    .map(|id| format!("task {id} "))
                    .unwrap_or_default(),
                match log["kind"].as_str() {
                    Some("gift_candidate_draft" | "welcome_candidate_draft" | "upgrade_candidate_draft") => "candidate draft",
                    Some("gift_candidate_cancelled" | "welcome_candidate_cancelled") =>
                        "candidate cancellation",
                    _ => "candidate event",
                },
                serde_json::to_string(&log)?
            );
        }
        Ok(())
    }

    pub fn enable_rules_preview(&mut self) {
        self.rules_preview = true;
    }

    pub fn begin_drain(&mut self) {
        for session in self.sessions.values_mut() {
            session.speech.stop();
        }
        self.draining = true;
        self.tickets.clear();
        let connections: Vec<_> = self.sessions.keys().cloned().collect();
        for connection in connections {
            let _ = self.cancel_candidate(&connection, "service_stopped");
        }
    }

    pub fn drain_finished(&mut self) -> bool {
        self.prune();
        self.sessions.is_empty()
    }

    pub fn configure(&mut self, rules: RulesConfig, scheduler: SchedulerConfig) -> Result<()> {
        if !self.sessions.is_empty() || !self.tickets.is_empty() {
            bail!("configuration is immutable after bootstrap; restart to apply changes");
        }
        Engine::new(rules.clone())?;
        scheduler.validate()?;
        self.rules_config = rules;
        self.scheduler_config = scheduler;
        Ok(())
    }

    pub fn enable_scheduler_preview(&mut self) {
        self.enable_rules_preview();
        self.scheduler_preview = true;
    }

    pub fn enable_automatic_send(&mut self) -> Result<()> {
        if !self.fill_preview
            || self.fill_test_text.is_some()
            || !self.tickets.is_empty()
            || !self.sessions.is_empty()
        {
            bail!("automatic sending requires fill mode without test text before bootstrap");
        }
        self.automatic_send = true;
        Ok(())
    }

    pub fn enable_candidate_fill_preview(&mut self, test_text: Option<String>) -> Result<()> {
        if self.scope.platform() != "wechat_channels"
            || !self.candidate_config.enabled
            || self.rules_preview
        {
            bail!("candidate fill requires an enabled WeChat target without other rules");
        }
        if let Some(text) = &test_text {
            validate_text(text)?;
        }
        self.candidate_fill_only = true;
        self.fill_preview = true;
        self.fill_test_text = test_text;
        Ok(())
    }
    pub fn enable_fill_preview(&mut self, test_text: Option<String>) -> Result<()> {
        if self.scope.platform() == "wechat_channels" {
            bail!("WeChat desktop adapter currently supports observation only");
        }
        if let Some(text) = &test_text {
            validate_text(text)?;
        }
        self.enable_scheduler_preview();
        self.fill_preview = true;
        self.fill_test_text = test_text;
        Ok(())
    }

    fn now(&self) -> u64 {
        self.started
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX - 1_800_000)) as u64
    }

    fn prune(&mut self) {
        let now = Instant::now();
        self.tickets.retain(|_, ticket| ticket.deadline > now);
        let expired: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| now.duration_since(s.last_seen) >= Duration::from_secs(30))
            .map(|(id, _)| id.clone())
            .collect();
        for connection in expired {
            let _ = self.cancel_candidate(&connection, "connection_expired");
            self.sessions.remove(&connection);
        }
    }

    /// Only authenticated local transports mint scoped single-use tickets.
    pub fn bootstrap_config(&mut self, relay: &str) -> Result<Value> {
        if self.draining {
            bail!("probe is draining");
        }
        if self.scope.platform() == "wechat_channels"
            && (self.rules_preview || (self.fill_preview && !self.candidate_fill_only))
        {
            bail!("WeChat desktop adapter currently supports observation only");
        }
        self.prune();
        if relay != self.relay_token || self.tickets.len() >= 32 {
            bail!("bootstrap denied");
        }
        let ticket = token();
        self.tickets.insert(
            ticket.clone(),
            Ticket {
                profile: None,
                deadline: Instant::now() + Duration::from_secs(60),
            },
        );
        let config = json!({"v":VERSION,"reload_handoff":true,"task_id":self.task_id,"ticket":ticket,"room_url":self.scope.room_url.as_str(),"adapter_id":self.scope.adapter(),"prefix":self.prefix,"rules_preview":self.rules_preview,"fill_preview":self.fill_preview,"automatic_send":self.automatic_send,"raw_messages":self.raw_messages,"delivery_actor_user_id":self.delivery.actor_user_id,"keyword_replies":self.keyword_config,"speech_recognition":self.unified_ui,"like_period_ledger":true,"likes":self.likes_config,"timed_messages":{"enabled":self.timed_config.enabled,"min_comments":self.timed_config.min_comments,"count_scope":self.timed_config.count_scope},"moderation_enabled":self.moderation_enabled,"control_epoch":self.control_epoch,"recording":self.recording_ui,"unified_ui":self.unified_ui,"input_open_test":self.scope.platform()=="wechat_channels" && self.fill_test_text.is_some(),"wait_for_input":self.automatic_send || self.candidate_fill_only || (self.fill_preview && !self.rules_config.gift_show_quantity)});
        Ok(config)
    }

    pub fn bootstrap(&mut self, relay: &str) -> Result<String> {
        self.bootstrap_for_room(relay, None)
    }
    pub fn bootstrap_for_room(&mut self, relay: &str, identity: Option<&str>) -> Result<String> {
        let mut config = self.bootstrap_config(relay)?;
        let identity = identity.filter(|id| self.scope.platform()=="wechat_channels" && super::events::valid_user_id(id) && !id.trim().is_empty() && id.trim()==*id);
        let mut profile = identity.and_then(|id|self.room_profiles.get(id)).cloned().unwrap_or_else(||self.default_room_profile());
        config["room_profile"] = json!(identity.filter(|id|self.room_profiles.contains_key(*id)).unwrap_or("default"));
        if let Some(id) = identity {
            config["room_identity"] = json!(id);
            if profile.candidate.enabled { profile.candidate.recipient_user_id=Some(id.into()); }
        }
        profile.boot(&mut config);
        self.tickets.get_mut(config["ticket"].as_str().expect("minted ticket")).expect("minted ticket").profile=Some(profile);

        if self.scope.platform() == "wechat_channels" {
            return Ok(format!(
                "(() => {{ const BOOT = {config};\n{SDK}\n{RAW_CAPTURE}\n{AUTO_SEND}\n{WECHAT_DESKTOP}\n{WECHAT_FILL}\n{PANEL}\n}})();"
            ));
        }
        Ok(format!(
            "(() => {{ const BOOT = {config};\n{SDK}\n{RAW_CAPTURE}\n{AUTO_SEND}\n{DOUYIN_DECODER}\n{DOUYIN_ADAPTER}\n{DOUYIN_FILL}\nif(BOOT.recording||BOOT.speech_recognition){{{DOUYIN_RECORDING}}}\n{PANEL}\n}})();"
        ))
    }

    pub fn hello(&mut self, hello: Hello) -> Result<Value> {
        if self.draining {
            bail!("probe is draining");
        }
        if self.scope.platform() == "wechat_channels"
            && (self.rules_preview || (self.fill_preview && !self.candidate_fill_only))
        {
            bail!("WeChat desktop adapter currently supports observation only");
        }
        self.prune();
        if self.storage_failed {
            bail!("journal failed; restart required");
        }
        if hello.v != VERSION
            || hello.adapter_id != self.scope.adapter()
            || !identifier(&hello.document_id, 128)
            || !self.scope.matches(&hello.page_url)
            || hello
                .client_id
                .as_ref()
                .is_some_and(|id| Uuid::parse_str(id).is_err())
        {
            bail!("hello scope or version mismatch");
        }
        if !self.tickets.contains_key(&hello.ticket) {
            bail!("pairing denied");
        }
        let profile = self.tickets.get(&hello.ticket).and_then(|ticket|ticket.profile.clone()).unwrap_or_else(||self.default_room_profile());
        if profile.keywords.enabled
            && (!self.automatic_send || !self.fill_preview || self.fill_test_text.is_some())
        {
            bail!("keyword_replies requires automatic sending without fixed test text");
        }
        if profile.timed.enabled
            && (!self.automatic_send || !self.fill_preview || self.fill_test_text.is_some())
        {
            bail!("timed_messages requires automatic sending without fixed test text");
        }
        // A scoped, single-use ticket may replace only this tab's previous
        // document. Other tabs still cannot take over an automatic sender.
        if self.automatic_send {
            if let Some(client) = &hello.client_id {
                let previous: Vec<_> = self
                    .sessions
                    .iter()
                    .filter(|(_, s)| s.client_id.as_ref() == Some(client))
                    .map(|(id, _)| id.clone())
                    .collect();
                for connection in previous {
                    self.cancel_candidate(&connection, "document_stopped")?;
                    let now = self.now();
                    let records = self
                        .sessions
                        .get_mut(&connection)
                        .and_then(|s| s.fill.as_mut())
                        .map(|fill| fill.cancel(now, "document_stopped"))
                        .unwrap_or_default();
                    if let Err(error) =
                        self.journal
                            .append_fill_audits(&self.run_id, &connection, &records)
                    {
                        self.storage_failed = true;
                        return Err(error);
                    }
                    self.sessions.remove(&connection);
                }
            }
        }
        if self.sessions.len() >= 16
            || (self.automatic_send
                && self.scope.platform() == "douyin"
                && !self.sessions.is_empty())
        {
            bail!("pairing denied");
        }
        self.tickets.remove(&hello.ticket);
        let connection = Uuid::new_v4().to_string();
        let binding = Uuid::new_v4().to_string();
        let session_token = token();
        let mut fill = self.fill_preview.then(|| {
            FillSession::new(
                connection.clone(),
                binding.clone(),
                hello.document_id.clone(),
                self.scope.room_url.to_string(),
                self.fill_test_text.is_some(),
            )
        });
        if self.automatic_send {
            fill.as_mut()
                .expect("fill enabled")
                .enable_automatic_send(self.scheduler_config.clone())?;
        }
        let logs = if let (Some(fill), Some(text)) = (&mut fill, &self.fill_test_text) {
            fill.enqueue(
                self.now(),
                "fill-test-1".into(),
                text.clone(),
                // Fixed UI acceptance may need time to load and start collection.
                // Natural gift drafts keep their separate 20-second expiry.
                self.now() + 120_000,
            )
        } else {
            vec![]
        };
        if let Err(error) = self
            .journal
            .append_fill_audits(&self.run_id, &connection, &logs)
        {
            self.storage_failed = true;
            return Err(error);
        }
        let preview = if self.rules_preview {
            let mut preview = OnlinePreview::with_rules(
                self.scope.room_key.clone(),
                format!("{}:{binding}", self.run_id),
                profile.rules.clone(),
            )?;
            if self.scheduler_preview {
                preview.configure_scheduler(self.scheduler_config.clone())?;
            }
            Some(preview)
        } else {
            None
        };
        self.sessions.insert(
            connection.clone(),
            Session {
                speech: Default::default(),
                client_id: hello.client_id.clone(),
                collecting: self.scope.platform() != "wechat_channels",
                keywords: super::keywords::KeywordReplies::new(
                    profile.keywords.clone(),
                    self.moderation_enabled && self.scope.platform() != "wechat_channels",
                ),
                timed: super::timed::TimedMessages::new(
                    profile.timed.clone(),
                    self.now(),
                    self.moderation_enabled && self.scope.platform() != "wechat_channels",
                ),
                token: session_token.clone(),
                binding: binding.clone(),
                last_seen: Instant::now(),
                acked_seq: 0,
                raw_acked_seq: 0,
                fill,
                preview,
                candidate_preview: profile.candidate.enabled.then(|| {
                    super::candidate_preview::CandidatePreview::with_format(
                        profile.candidate
                            .recipient_user_id
                            .clone()
                            .expect("validated target"),
                        &profile.rules,
                    )
                    .expect("validated rules format")
                }),
            },
        );
        Ok(
            json!({"v":VERSION,"mode":if self.automatic_send {"fill_and_send"} else if self.fill_preview {"fill_only"} else {"observe"},"connection_id":connection,"binding_id":binding,"token":session_token,"room_key":self.scope.room_key,"actor_verified":false,"send_chat":self.automatic_send,"acked_seq":0}),
        )
    }

    pub fn speech(&mut self, bearer: &str, body: Value) -> Result<Value> {
        let scope: Poll = serde_json::from_value(body["scope"].clone())?;
        if self.draining
            || !self.unified_ui
            || scope.v != VERSION
            || !self.scope.matches(&scope.page_url)
        {
            bail!("speech scope unavailable");
        }
        self.authorize(bearer, &scope.connection_id, &scope.binding_id)?;
        let session = self
            .sessions
            .get_mut(&scope.connection_id)
            .expect("authorized");
        match body["action"].as_str().unwrap_or("status") {
            "start" => session.speech.start(
                body["stream_url"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("直播音频尚未就绪"))?,
            )?,
            "stop" => session.speech.stop(),
            "status" => {}
            _ => bail!("invalid speech action"),
        }
        Ok(serde_json::to_value(session.speech.status())?)
    }

    pub fn authorize_scope(&mut self, bearer: &str, scope: Poll) -> Result<()> {
        if self.draining || !self.recording_ui {
            bail!("recording unavailable");
        }
        if scope.v != VERSION || !self.scope.matches(&scope.page_url) {
            bail!("invalid scope");
        }
        self.authorize(bearer, &scope.connection_id, &scope.binding_id)
    }

    fn authorize(&mut self, bearer: &str, connection: &str, binding: &str) -> Result<()> {
        self.prune();
        let Some(session) = self.sessions.get_mut(connection) else {
            bail!("unknown connection");
        };
        if bearer != session.token || binding != session.binding {
            bail!("invalid binding");
        }
        session.last_seen = Instant::now();
        Ok(())
    }

    pub fn events(&mut self, bearer: &str, batch: EventBatch) -> Result<Value> {
        self.authorize(bearer, &batch.connection_id, &batch.binding_id)?;
        if self.storage_failed {
            bail!("journal failed; restart required");
        }
        if batch.v != VERSION
            || batch.events.is_empty()
            || batch.events.len() > 50
            || batch.first_seq == 0
        {
            bail!("invalid batch");
        }
        let acked = self.sessions[&batch.connection_id].acked_seq;
        if batch.first_seq > acked.saturating_add(1) {
            bail!("sequence gap");
        }
        let mut sanitized = Vec::new();
        for (index, event) in batch.events.iter().enumerate() {
            if batch.first_seq.checked_add(index as u64) != Some(event.seq) {
                bail!("batch sequence mismatch");
            }
            event.validate()?;
            sanitized.push(sanitize(event));
        }
        let new_count = sanitized.iter().filter(|e| e.seq > acked).count();
        if !self.raw_messages && self.total_events + new_count > 50_000 {
            bail!("probe event budget exceeded; restart with a fresh journal");
        }
        if let Err(error) = self
            .journal
            .append(&self.run_id, &batch.connection_id, &sanitized)
        {
            self.storage_failed = true;
            return Err(error);
        }
        for event in sanitized
            .iter()
            .filter(|e| e.seq > acked && e.event_type == "recording.result")
        {
            if self
                .recording_request
                .as_ref()
                .is_some_and(|(id, _)| event.payload["request_id"].as_str() == Some(id.as_str()))
            {
                self.recording_result = Some(event.payload.clone());
                self.recording_request = None;
            }
        }
        let now = self.now();
        let mut logs = Vec::new();
        let draining = self.draining || !self.moderation_enabled;
        if let Some(preview) = self
            .sessions
            .get_mut(&batch.connection_id)
            .and_then(|s| if draining { None } else { s.preview.as_mut() })
        {
            for event in sanitized.iter().filter(|e| e.seq > acked) {
                logs.extend(preview.ingest(now, event)?);
            }
            // All deadline events in a batch precede advancing that deadline.
            logs.extend(preview.advance(now)?);
        }
        if let Err(error) = self
            .journal
            .append_audits(&self.run_id, &batch.connection_id, &logs)
        {
            self.storage_failed = true;
            return Err(error);
        }
        for event in sanitized.iter().filter(|e| e.seq > acked) {
            let active = if event.event_type == "probe.status"
                && event.payload["stage"] == "wechat_collection_started"
            {
                Some(true)
            } else if event.event_type == "room.ended"
                || event.event_type == "probe.gap"
                || (event.event_type == "probe.status"
                    && matches!(
                        event.payload["stage"].as_str(),
                        Some("wechat_collection_paused" | "document_stopped")
                    ))
            {
                Some(false)
            } else {
                None
            };
            if let Some(active) = active {
                let session = self
                    .sessions
                    .get_mut(&batch.connection_id)
                    .expect("authorized");
                session.collecting = active;
                session
                    .timed
                    .set_active(now, active && self.moderation_enabled);
            }
            if !draining {
                let session = self
                    .sessions
                    .get_mut(&batch.connection_id)
                    .expect("authorized");
                let own_text = event.payload["content"].as_str().is_some_and(|text| {
                    session
                        .fill
                        .as_ref()
                        .is_some_and(|fill| fill.is_own_text(text))
                });
                session.timed.observe_comment(
                    now,
                    self.scope.platform(),
                    event,
                    own_text,
                    self.delivery.actor_user_id.as_deref(),
                );
            }
        }
        for event in sanitized.iter().filter(|e| e.seq > acked) {
            *self.counts.entry(event.event_type.clone()).or_default() += 1;
        }
        self.candidate_drafts(
            &batch.connection_id,
            now,
            &sanitized
                .iter()
                .filter(|e| e.seq > acked)
                .cloned()
                .collect::<Vec<_>>(),
        )?;
        self.collect_fill(&batch.connection_id, now, &logs)?;
        self.keyword_drafts(
            &batch.connection_id,
            now,
            &sanitized
                .iter()
                .filter(|e| e.seq > acked)
                .cloned()
                .collect::<Vec<_>>(),
        )?;
        self.total_events += new_count;
        let last = sanitized.last().expect("nonempty validated").seq.max(acked);
        self.sessions
            .get_mut(&batch.connection_id)
            .expect("authorized session")
            .acked_seq = last;
        Ok(json!({"v":VERSION,"acked_seq":last}))
    }

    pub fn poll(&mut self, bearer: &str, poll: Poll) -> Result<Value> {
        if poll.v != VERSION || !self.scope.matches(&poll.page_url) {
            // Validate the bearer before revoking, so unauthenticated callers
            // cannot evict an otherwise healthy page.
            self.authorize(bearer, &poll.connection_id, &poll.binding_id)?;
            self.cancel_candidate(&poll.connection_id, "page_changed")?;
            self.sessions.remove(&poll.connection_id);
            bail!("page changed; binding revoked");
        }
        self.authorize(bearer, &poll.connection_id, &poll.binding_id)?;
        if self.last_cleanup.elapsed() >= Duration::from_secs(60) {
            self.maintain_journal(false)?;
        }
        if self.storage_failed {
            bail!("journal failed; restart required");
        }
        if self.draining {
            return Ok(
                json!({"v":VERSION,"mode":"observe","send_chat":false,"draining":true,"commands":[],"previews":[],"candidate_preview":self.candidate_status(&poll.connection_id)}),
            );
        }
        let now = self.now();
        let logs = if let Some(preview) = self.sessions.get_mut(&poll.connection_id).and_then(|s| {
            if self.moderation_enabled {
                s.preview.as_mut()
            } else {
                None
            }
        }) {
            preview.advance(now)?
        } else {
            Vec::new()
        };
        if let Err(error) = self
            .journal
            .append_audits(&self.run_id, &poll.connection_id, &logs)
        {
            self.storage_failed = true;
            return Err(error);
        }
        self.candidate_drafts(&poll.connection_id, now, &[])?;
        self.collect_fill(&poll.connection_id, now, &logs)?;
        let wait_for_input = self.automatic_send
            || self.candidate_fill_only
            || (self.fill_preview && !self.rules_config.gift_show_quantity);
        let session = self
            .sessions
            .get_mut(&poll.connection_id)
            .expect("authorized");
        let paused = !self.moderation_enabled
            || session
                .preview
                .as_ref()
                .is_some_and(|p| p.status()["paused"] == true)
            || session
                .candidate_preview
                .as_ref()
                .is_some_and(|p| p.status(now)["paused"] == true);
        let shared = self.account_budget.clone();
        let mut account_guard = shared
            .as_ref()
            .map(|budget| {
                budget
                    .lock()
                    .map_err(|_| anyhow::anyhow!("account budget poisoned"))
            })
            .transpose()?;
        let account_ready =
            !self.automatic_send || account_guard.as_mut().is_none_or(|g| g.ready());
        let pending_interactions = session
            .preview
            .as_ref()
            .is_some_and(|p| p.has_pending_dispatch())
            || session
                .candidate_preview
                .as_ref()
                .is_some_and(|p| p.status(now)["pending_users"].as_u64().unwrap_or(0) > 0);
        let timed_ready = !paused
            && account_ready
            && poll.input_ready == Some(true)
            && !pending_interactions
            && session
                .fill
                .as_ref()
                .is_some_and(|f| f.ready_for_timed(now));
        let mut timed_logs = vec![];
        if let Some(text) = session.timed.tick(now, timed_ready) {
            let id = format!("timed-{}", Uuid::new_v4());
            timed_logs.extend(session.fill.as_mut().expect("timed fill enabled").enqueue(
                now,
                id,
                text,
                now + 20000,
            ));
        }
        let (commands, mut fill_logs) = session
            .fill
            .as_mut()
            .map(|fill| {
                fill.issue_when_ready(
                    now,
                    paused,
                    account_ready && (!wait_for_input || poll.input_ready == Some(true)),
                )
            })
            .unwrap_or_default();
        timed_logs.append(&mut fill_logs);
        let fill_logs = timed_logs;
        if self.automatic_send && !commands.is_empty() {
            if let Some(budget) = account_guard.as_mut() {
                budget.reserve();
            }
        }
        drop(account_guard);
        if let Err(error) =
            self.journal
                .append_fill_audits(&self.run_id, &poll.connection_id, &fill_logs)
        {
            self.storage_failed = true;
            return Err(error);
        }
        let fill = self.sessions[&poll.connection_id]
            .fill
            .as_ref()
            .map(FillSession::status);
        let (rules, previews) = if let Some(preview) = self
            .sessions
            .get_mut(&poll.connection_id)
            .and_then(|s| s.preview.as_mut())
        {
            (preview.status(), preview.drain())
        } else {
            (Value::Null, Vec::new())
        };
        Ok(
            json!({"v":VERSION,"mode":if self.automatic_send {"fill_and_send"} else if self.fill_preview {"fill_only"} else {"observe"},"commands":commands,"fill":fill,"send_chat":self.automatic_send,"rules":rules,"previews":previews,"candidate_preview":self.candidate_status(&poll.connection_id),"timed_messages":self.sessions[&poll.connection_id].timed.status(now),"keyword_replies":self.sessions[&poll.connection_id].keywords.status(),"moderation_control":self.control_status(),"recording_request":self.recording_request.as_ref().filter(|(_,at)|at.elapsed()<Duration::from_secs(15)).map(|(id,_)|id)}),
        )
    }

    fn keyword_drafts(&mut self, connection: &str, now: u64, events: &[ProbeEvent]) -> Result<()> {
        // Each session owns the selected room's keyword configuration. The
        // default may be disabled while this room explicitly enables replies.
        if self.draining || !self.moderation_enabled {
            return Ok(());
        }
        let platform = self.scope.platform();
        let actor = self.delivery.actor_user_id.as_deref();
        let session = self.sessions.get_mut(connection).expect("authorized");
        let mut logs = vec![];
        let mut fill_logs = vec![];
        for event in events {
            let active = if event.event_type == "probe.status"
                && event.payload["stage"] == "wechat_collection_started"
            {
                Some(true)
            } else if matches!(event.event_type.as_str(), "probe.gap" | "room.ended")
                || event.event_type == "probe.status"
                    && matches!(
                        event.payload["stage"].as_str(),
                        Some("wechat_collection_paused" | "document_stopped")
                    )
            {
                Some(false)
            } else {
                None
            };
            if let Some(active) = active {
                session.keywords.set_active(active);
                if !active {
                    if let Some(fill) = session.fill.as_mut() {
                        fill_logs.extend(fill.cancel(now, "observation_paused"));
                    }
                }
            }
            let own_text = event.payload["content"].as_str().is_some_and(|text| {
                session
                    .fill
                    .as_ref()
                    .is_some_and(|fill| fill.is_own_text(text))
            });
            if let Some(draft) = session
                .keywords
                .ingest(now, platform, event, own_text, actor)
            {
                let id = format!("keyword-{}", Uuid::new_v4());
                let records = session
                    .fill
                    .as_mut()
                    .expect("keyword fill enabled")
                    .enqueue(now, id.clone(), draft.text.clone(), now + 20_000);
                let queued = records.iter().any(|r| r.state == "queued");
                logs.push(json!({"kind":if queued {"keyword_reply_draft"} else {"keyword_reply_skipped"},
                    "at_ms":now,"action_id":id,"text":draft.text,"keyword":draft.keyword,
                    "event_id":draft.event_id,"native_event_id":draft.native_event_id,"seq":event.seq,
                    "reason":if queued {"keyword_matched"} else {"fill_queue_rejected"},"send_chat":false,"fill_input":false}));
                fill_logs.extend(records);
            }
        }
        if let Err(error) = self
            .journal
            .append_candidate_drafts(&self.run_id, connection, &logs)
            .and_then(|_| {
                self.journal
                    .append_fill_audits(&self.run_id, connection, &fill_logs)
            })
        {
            self.storage_failed = true;
            return Err(error);
        }
        for log in logs {
            println!(
                "live-archive moderation: keyword reply {}",
                serde_json::to_string(&log)?
            );
        }
        Ok(())
    }

    fn collect_fill(
        &mut self,
        connection: &str,
        now: u64,
        logs: &[super::engine::Audit],
    ) -> Result<()> {
        let session = self.sessions.get_mut(connection).expect("authorized");
        let paused = session
            .preview
            .as_ref()
            .is_some_and(|p| p.status()["paused"] == true)
            || session
                .candidate_preview
                .as_ref()
                .is_some_and(|p| p.status(now)["paused"] == true);
        let records = if let Some(fill) = &mut session.fill {
            if paused {
                fill.cancel(now, "observation_paused")
            } else {
                fill.collect(now, logs)
            }
        } else {
            vec![]
        };
        if let Err(error) = self
            .journal
            .append_fill_audits(&self.run_id, connection, &records)
        {
            self.storage_failed = true;
            return Err(error);
        }
        Ok(())
    }

    pub fn fill_result(&mut self, bearer: &str, report: FillReport) -> Result<Value> {
        self.authorize(bearer, &report.connection_id, &report.binding_id)?;
        if self.storage_failed || report.v != VERSION || !self.scope.matches(&report.page_url) {
            bail!("fill result scope or journal unavailable");
        }
        let now = self.now();
        let records = self
            .sessions
            .get_mut(&report.connection_id)
            .expect("authorized")
            .fill
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("fill-only mode disabled"))?
            .report(now, &report)?;
        if let Err(error) =
            self.journal
                .append_fill_audits(&self.run_id, &report.connection_id, &records)
        {
            self.storage_failed = true;
            return Err(error);
        }
        Ok(json!({"v":VERSION,"acked_action_id":report.action_id,"send_chat":self.automatic_send}))
    }

    pub fn goodbye(&mut self, bearer: &str, poll: Poll) -> Result<Value> {
        self.authorize(bearer, &poll.connection_id, &poll.binding_id)?;
        self.cancel_candidate(&poll.connection_id, "document_stopped")?;
        let now = self.now();
        let records = self
            .sessions
            .get_mut(&poll.connection_id)
            .expect("authorized")
            .fill
            .as_mut()
            .map(|fill| fill.cancel(now, "document_stopped"))
            .unwrap_or_default();
        if let Err(error) =
            self.journal
                .append_fill_audits(&self.run_id, &poll.connection_id, &records)
        {
            self.storage_failed = true;
            self.sessions.remove(&poll.connection_id);
            return Err(error);
        }
        self.sessions.remove(&poll.connection_id);
        Ok(json!({"v":VERSION,"stopped":true}))
    }

    pub fn summary(&mut self) -> Value {
        self.prune();
        let decoding = if self.scope.platform() == "wechat_channels" {
            "wechat.msg_list.type1.candidate.v1"
        } else if self.rules_preview {
            "douyin.pb.v1_candidate"
        } else {
            "not_implemented"
        };
        json!({"mode":if self.automatic_send {"fill_and_send"} else if self.fill_preview {"fill_only"} else {"observe"},"send_chat":self.automatic_send,"run_id":self.run_id,"room_key":self.scope.room_key,"active_connections":self.sessions.len(),"event_count":self.total_events,"event_types":self.counts,"actor_verified":false,"network_decoding":decoding,"rules_preview":self.rules_preview,"scheduler_preview":self.scheduler_preview,"fill_preview":self.fill_preview,"storage_failed":self.storage_failed,"raw_messages":self.raw_messages,"coverage":"observed_only","candidate_preview":self.candidate_config.enabled,"candidate_fill_only":self.candidate_fill_only})
    }
}
