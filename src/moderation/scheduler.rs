//! Dispatch timing simulation. No page commands, leases or delivery claims.
//! One scheduler shares budgets across the scopes of its engine. Online probes
//! intentionally isolate documents until a verified execution account exists.
use super::engine::{Audit, Engine, PreviewAction};
use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

const WINDOW_MS: u64 = 60_000;
type Account = (String, String);

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SchedulerConfig {
    pub min_interval_ms: u64,
    pub account_per_minute: usize,
}
impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            min_interval_ms: 8_000,
            account_per_minute: 6,
        }
    }
}
impl SchedulerConfig {
    pub fn validate(&self) -> Result<()> {
        if !(8_000..=60_000).contains(&self.min_interval_ms)
            || !(1..=6).contains(&self.account_per_minute)
        {
            bail!("scheduler requires min_interval_ms=8000..60000 and account_per_minute=1..6");
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct PreviewScheduler {
    config: SchedulerConfig,
    clock: u64,
    budgets: BTreeMap<Account, VecDeque<u64>>,
    dispatched: u64,
}

impl PreviewScheduler {
    pub fn new(config: SchedulerConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            ..Self::default()
        })
    }
    fn account(action: &PreviewAction) -> Account {
        (
            action.scope.platform.clone(),
            action.scope.actor_user_id.clone(),
        )
    }

    fn ready_at(&self, action: &PreviewAction) -> u64 {
        let Some(history) = self.budgets.get(&Self::account(action)) else {
            return action.created_at;
        };
        let interval = history
            .back()
            .map_or(0, |at| at.saturating_add(self.config.min_interval_ms));
        let window = if history.len() >= self.config.account_per_minute {
            history.front().map_or(0, |at| at.saturating_add(WINDOW_MS))
        } else {
            0
        };
        action.created_at.max(interval).max(window)
    }

    /// Caller ingests all events for `now` first, so new gifts can retract a
    /// queued thanks even at the exact budget boundary. No duplicate queue.
    pub fn advance(&mut self, now: u64, engine: &mut Engine) -> Result<Vec<Audit>> {
        if now < self.clock {
            bail!("scheduler clock must be monotonic");
        }
        let mut logs = engine.advance(now)?;
        self.clock = now;
        for history in self.budgets.values_mut() {
            while history
                .front()
                .is_some_and(|at| now.saturating_sub(*at) >= WINDOW_MS)
            {
                history.pop_front();
            }
        }
        self.budgets.retain(|_, history| !history.is_empty());
        loop {
            let candidate = engine
                .pending_actions()
                .into_iter()
                .filter(|a| a.expires_at > now && self.ready_at(a) <= now)
                .min_by(|a, b| {
                    b.priority
                        .cmp(&a.priority)
                        .then(a.created_at.cmp(&b.created_at))
                        .then(a.scope.cmp(&b.scope))
                        .then(a.action_id.cmp(&b.action_id))
                })
                .map(|a| (a.action_id.clone(), Self::account(a)));
            let Some((id, account)) = candidate else {
                break;
            };
            // The engine is limited to 16 scopes, hence at most 16 accounts.
            if !self.budgets.contains_key(&account) && self.budgets.len() >= 16 {
                bail!("scheduler account capacity exceeded");
            }
            let records = engine.simulate_dispatch(now, &id)?;
            self.budgets.entry(account).or_default().push_back(now);
            self.dispatched += 1;
            for mut record in records {
                if record.kind == "dispatch_simulated" {
                    record.kind = "dispatch_preview".into();
                    record.reason = "scheduler_preview_without_sending".into();
                }
                logs.push(record);
            }
        }
        Ok(logs)
    }

    pub fn status(&self, engine: &Engine) -> Value {
        let pending = engine.pending_actions();
        let next = pending
            .iter()
            .filter_map(|a| {
                let at = self.ready_at(a).max(self.clock);
                (a.expires_at > at).then_some(at)
            })
            .min();
        json!({"mode":"dispatch_preview","send_chat":false,"min_interval_ms":self.config.min_interval_ms,
            "account_per_minute":self.config.account_per_minute,"pending":pending.len(),"dispatch_previews":self.dispatched,
            "next_eligible_at_ms":next,"budget_scope":"document_simulation_only"})
    }
}
