//! Guarded draft commands; automatic submission is an explicit opt-in.
use super::engine::Audit;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Debug, Serialize)]
pub struct FillCommand {
    pub v: u32,
    pub command: &'static str,
    pub action_id: String,
    pub connection_id: String,
    pub binding_id: String,
    pub document_id: String,
    pub page_url: String,
    pub remaining_ttl_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FillOutcome {
    Filled,
    Submitted,
    Unknown,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FillReason {
    InputFilled,
    InputSubmitted,
    SubmissionUnconfirmed,
    SendNotFound,
    SendNotUnique,
    SendDisabled,
    InputNotFound,
    InputNotUnique,
    InputBusy,
    InputDisabled,
    InputHidden,
    InputChanged,
    InputError,
    BindingMismatch,
    Stopped,
    RoomChanged,
    NotTopFrame,
    PageHidden,
    TtlExpired,
    InvalidCommand,
    TextTooLong,
    ExecutorUnavailable,
    CacheFull,
    ObservationPaused,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillReport {
    pub v: u32,
    pub connection_id: String,
    pub binding_id: String,
    pub page_url: String,
    pub action_id: String,
    pub outcome: FillOutcome,
    pub reason: FillReason,
}

#[derive(Clone, Debug, Serialize)]
pub struct FillAudit {
    pub at_ms: u64,
    pub action_id: String,
    pub state: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<FillCommand>,
}
struct Entry {
    command: FillCommand,
    expires_at: u64,
    issued_at: Option<u64>,
    state: String,
    report: Option<(FillOutcome, FillReason)>,
    delivery: Option<super::delivery::DeliveryReport>,
}

pub struct FillSession {
    connection: String,
    binding: String,
    document: String,
    page: String,
    entries: BTreeMap<String, Entry>,
    queue: VecDeque<String>,
    in_flight: Option<String>,
    manual_only: bool,
    automatic_send: bool,
    send_config: super::scheduler::SchedulerConfig,
    attempts: VecDeque<u64>,
}

pub fn validate_text(text: &str) -> Result<()> {
    if text.trim().is_empty() || text.chars().any(char::is_control) || text.chars().count() > 80 {
        bail!("fill-only text requires 1..80 characters without control characters");
    }
    Ok(())
}

impl FillSession {
    pub fn new(
        connection: String,
        binding: String,
        document: String,
        page: String,
        manual_only: bool,
    ) -> Self {
        Self {
            connection,
            binding,
            document,
            page,
            entries: BTreeMap::new(),
            queue: VecDeque::new(),
            in_flight: None,
            manual_only,
            automatic_send: false,
            send_config: Default::default(),
            attempts: VecDeque::new(),
        }
    }
    pub fn enable_automatic_send(
        &mut self,
        config: super::scheduler::SchedulerConfig,
    ) -> Result<()> {
        config.validate()?;
        if self.manual_only || !self.entries.is_empty() {
            bail!("automatic sending must be set before enqueue without fixed test text");
        }
        self.automatic_send = true;
        self.send_config = config;
        Ok(())
    }
    fn audit(now: u64, id: &str, state: &str, reason: &str) -> FillAudit {
        FillAudit {
            at_ms: now,
            action_id: id.into(),
            state: state.into(),
            reason: reason.into(),
            command: None,
        }
    }
    pub fn enqueue(
        &mut self,
        now: u64,
        id: String,
        text: String,
        expires_at: u64,
    ) -> Vec<FillAudit> {
        if self.entries.contains_key(&id) {
            return vec![];
        }
        let reason = if validate_text(&text).is_err() {
            Some("invalid_fill_text")
        } else if expires_at <= now {
            Some("ttl_expired")
        } else if self.entries.len() >= 1000 {
            Some("document_command_budget_exhausted")
        } else if self.queue.len() >= 20 {
            Some("fill_queue_full")
        } else {
            None
        };
        if let Some(reason) = reason {
            return vec![Self::audit(now, &id, "rejected", reason)];
        }
        let command = FillCommand {
            v: 1,
            command: if self.automatic_send {
                "fill_and_submit"
            } else {
                "fill_input"
            },
            action_id: id.clone(),
            connection_id: self.connection.clone(),
            binding_id: self.binding.clone(),
            document_id: self.document.clone(),
            page_url: self.page.clone(),
            remaining_ttl_ms: 0,
            text,
        };
        let mut audit = Self::audit(
            now,
            &id,
            "queued",
            if self.automatic_send {
                "automatic_submission_enabled"
            } else {
                "fill_only_no_submission"
            },
        );
        audit.command = Some(command.clone());
        self.entries.insert(
            id.clone(),
            Entry {
                command,
                expires_at,
                issued_at: None,
                state: "queued".into(),
                report: None,
                delivery: None,
            },
        );
        self.queue.push_back(id);
        vec![audit]
    }
    pub fn collect(&mut self, now: u64, logs: &[Audit]) -> Vec<FillAudit> {
        if self.manual_only {
            return vec![];
        }
        let mut records = vec![];
        for log in logs.iter().filter(|log| log.kind == "dispatch_preview") {
            if let Some(action) = &log.action {
                records.extend(self.enqueue(
                    now,
                    action.action_id.clone(),
                    action.text.clone(),
                    action.expires_at,
                ));
            }
        }
        records
    }
    /// Commands are handed off once. Lost responses become unknown, never retry.
    pub fn issue(&mut self, now: u64, paused: bool) -> (Vec<FillCommand>, Vec<FillAudit>) {
        self.issue_when_ready(now, paused, true)
    }
    pub fn issue_when_ready(
        &mut self,
        now: u64,
        paused: bool,
        ready: bool,
    ) -> (Vec<FillCommand>, Vec<FillAudit>) {
        if paused {
            return (vec![], self.cancel(now, "observation_paused"));
        }
        let mut logs = vec![];
        if let Some(id) = &self.in_flight {
            let entry = self.entries.get_mut(id).expect("in-flight entry");
            if now.saturating_sub(entry.issued_at.expect("issued")) >= 5000 {
                entry.state = "unknown".into();

                logs.push(Self::audit(
                    now,
                    id,
                    "unknown",
                    "fill_result_timeout_no_retry",
                ));
                self.in_flight = None;
            }
        }
        // Expired queued commands are retired even while another is in flight.
        self.queue.retain(|id| {
            let entry = self.entries.get_mut(id).expect("queued entry");
            if entry.expires_at <= now {
                entry.state = "expired".into();
                logs.push(Self::audit(now, id, "expired", "ttl_expired"));
                false
            } else {
                true
            }
        });
        if self.in_flight.is_some() {
            return (vec![], logs);
        }
        while self
            .attempts
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= 60_000)
        {
            self.attempts.pop_front();
        }
        if self.automatic_send
            && (self.attempts.len() >= self.send_config.account_per_minute
                || self
                    .attempts
                    .back()
                    .is_some_and(|t| now.saturating_sub(*t) < self.send_config.min_interval_ms))
        {
            return (vec![], logs);
        }
        if !ready {
            return (vec![], logs);
        }
        let Some(id) = self.queue.pop_front() else {
            return (vec![], logs);
        };
        let entry = self.entries.get_mut(&id).expect("queued entry");
        entry.command.remaining_ttl_ms = (entry.expires_at - now).min(3000);
        entry.state = "issued".into();
        if self.automatic_send {
            self.attempts.push_back(now);
        }
        entry.issued_at = Some(now);
        self.in_flight = Some(id.clone());
        logs.push(Self::audit(
            now,
            &id,
            "issued",
            if self.automatic_send {
                "automatic_submission_enabled"
            } else {
                "fill_only_no_submission"
            },
        ));
        (vec![entry.command.clone()], logs)
    }
    pub fn report(&mut self, now: u64, report: &FillReport) -> Result<Vec<FillAudit>> {
        let consistent = match report.outcome {
            FillOutcome::Filled => report.reason == FillReason::InputFilled && !self.automatic_send,
            FillOutcome::Submitted => {
                report.reason == FillReason::InputSubmitted && self.automatic_send
            }
            FillOutcome::Unknown => {
                report.reason == FillReason::SubmissionUnconfirmed && self.automatic_send
            }
            FillOutcome::Rejected => !matches!(
                report.reason,
                FillReason::InputFilled
                    | FillReason::InputSubmitted
                    | FillReason::SubmissionUnconfirmed
            ),
        };
        if !consistent {
            bail!("inconsistent fill result");
        }
        let entry = self
            .entries
            .get_mut(&report.action_id)
            .ok_or_else(|| anyhow::anyhow!("unknown fill command"))?;
        if entry.issued_at.is_none() {
            bail!("fill command was never issued");
        }
        let outcome = (report.outcome.clone(), report.reason.clone());
        if let Some(previous) = &entry.report {
            if previous != &outcome {
                bail!("conflicting fill result");
            }
            return Ok(vec![]);
        }
        entry.report = Some(outcome);
        entry.state = match report.outcome {
            FillOutcome::Filled => "filled",
            FillOutcome::Submitted => "submitted",
            FillOutcome::Unknown => "unknown",
            FillOutcome::Rejected => "rejected",
        }
        .into();

        if self.in_flight.as_deref() == Some(&report.action_id) {
            self.in_flight = None;
        }
        let reason = serde_json::to_value(&report.reason)?
            .as_str()
            .expect("reason string")
            .to_owned();
        Ok(vec![Self::audit(
            now,
            &report.action_id,
            &entry.state,
            &reason,
        )])
    }
    pub fn record_delivery(&mut self, report: &super::delivery::DeliveryReport) -> Result<()> {
        if !self.automatic_send {
            bail!("delivery requires automatic submission");
        }
        let entry = self
            .entries
            .get_mut(&report.action_id)
            .ok_or_else(|| anyhow::anyhow!("unknown delivery action"))?;
        if !matches!(
            entry.report,
            Some((FillOutcome::Submitted | FillOutcome::Unknown, _))
        ) {
            bail!("local submission result required before delivery evidence");
        }
        if let Some(previous) = &entry.delivery {
            if previous != report {
                bail!("conflicting delivery evidence");
            }
            return Ok(());
        }
        entry.delivery = Some(report.clone());
        Ok(())
    }
    /// Called only after the matching delivery record has been persisted.
    pub fn reconcile_delivery(
        &mut self,
        now: u64,
        report: &super::delivery::DeliveryReport,
    ) -> (bool, Vec<FillAudit>) {
        if report.state != "echo_observed" {
            return (false, vec![]);
        }
        let Some(entry) = self.entries.get_mut(&report.action_id) else {
            return (false, vec![]);
        };
        if entry.delivery.as_ref() != Some(report) {
            return (false, vec![]);
        }
        let mut logs = vec![];
        if entry.state == "unknown" && matches!(entry.report, Some((FillOutcome::Unknown, _))) {
            entry.state = "echo_confirmed".into();
            logs.push(Self::audit(
                now,
                &report.action_id,
                "echo_confirmed",
                "network_echo_confirmed_no_resend",
            ));
        }
        (true, logs)
    }

    pub fn cancel_queued(&mut self, now: u64) -> Vec<FillAudit> {
        self.queue
            .drain(..)
            .collect::<Vec<_>>()
            .into_iter()
            .map(|id| {
                self.entries.get_mut(&id).expect("entry").state = "cancelled".into();
                Self::audit(now, &id, "cancelled", "moderation_disabled")
            })
            .collect()
    }
    pub fn cancel(&mut self, now: u64, reason: &str) -> Vec<FillAudit> {
        let mut ids: Vec<_> = self.queue.drain(..).collect();
        ids.extend(self.in_flight.take());
        ids.into_iter()
            .map(|id| {
                self.entries.get_mut(&id).expect("entry").state = "cancelled".into();
                Self::audit(now, &id, "cancelled", reason)
            })
            .collect()
    }
    pub fn is_own_text(&self, text: &str) -> bool {
        self.entries
            .values()
            .any(|entry| entry.command.text == text)
    }
    pub fn ready_for_timed(&self, now: u64) -> bool {
        self.automatic_send
            && !self.manual_only
            && self.queue.is_empty()
            && self.in_flight.is_none()
            && self.entries.len() < 1000
            && self
                .attempts
                .iter()
                .filter(|t| now.saturating_sub(**t) < 60000)
                .count()
                < self.send_config.account_per_minute
            && self
                .attempts
                .back()
                .is_none_or(|t| now.saturating_sub(*t) >= self.send_config.min_interval_ms)
    }
    pub fn status(&self) -> Value {
        let count = |state: &str| self.entries.values().filter(|e| e.state == state).count();
        json!({"mode":if self.automatic_send {"fill_and_send"} else {"fill_only"},"send_chat":self.automatic_send,"blocked":false,"submitted":count("submitted"),"echo_observed":self.entries.values().filter(|e|e.delivery.as_ref().is_some_and(|d|d.state=="echo_observed")).count(),"delivery_unconfirmed":self.entries.values().filter(|e|e.delivery.as_ref().is_some_and(|d|d.state=="unconfirmed")).count(),"actor_verified":false,"pending":self.queue.len(),
            "in_flight":self.in_flight,"filled":count("filled"),"rejected":count("rejected"),"unknown":count("unknown"),"echo_confirmed":count("echo_confirmed"),
            "manual_only":self.manual_only,"completion":if self.automatic_send {"click_attempt_not_delivery"} else {"input_filled_no_submission"}})
    }
}
