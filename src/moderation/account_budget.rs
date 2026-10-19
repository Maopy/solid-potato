//! Shared conservative command budget for tasks using the same configured account.
use super::scheduler::SchedulerConfig;
use std::{collections::VecDeque, time::Instant};
pub struct AccountBudget {
    config: SchedulerConfig,
    history: VecDeque<Instant>,
}
impl AccountBudget {
    pub fn new(config: SchedulerConfig) -> anyhow::Result<Self> {
        config.validate()?;
        Ok(Self {
            config,
            history: VecDeque::new(),
        })
    }
    pub fn ready(&mut self) -> bool {
        let now = Instant::now();
        while self
            .history
            .front()
            .is_some_and(|t| now.duration_since(*t).as_millis() >= 60_000)
        {
            self.history.pop_front();
        }
        self.history.len() < self.config.account_per_minute
            && self.history.back().is_none_or(|t| {
                now.duration_since(*t).as_millis() >= u128::from(self.config.min_interval_ms)
            })
    }
    pub fn reserve(&mut self) {
        self.history.push_back(Instant::now());
    }
}
