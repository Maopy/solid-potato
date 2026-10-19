use crate::{
    model::{RecordingTask, TaskStatus},
    platform::{ResolveError, StreamResolver},
    recorder::StreamRecorder,
    store::TaskStore,
};
use anyhow::{Result, bail};
use chrono::Utc;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::{Mutex, watch},
    time::sleep,
};
use uuid::Uuid;

/// Timing policy for unattended monitoring. A resolver reporting `Offline`
/// waits for `offline_poll`; other failures back off exponentially.
#[derive(Debug, Clone)]
pub struct MonitorPolicy {
    pub offline_poll: Duration,
    pub retry_initial: Duration,
    pub retry_max: Duration,
}

/// Bounded recovery policy for a single recording command. It is deliberately
/// platform-agnostic: resolver and recorder failures are treated uniformly
/// whether the stream came from Douyin, WeChat Channels, or a direct URL.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u8,
    pub retry_initial: Duration,
    pub retry_max: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_initial: Duration::from_secs(5),
            retry_max: Duration::from_secs(60),
        }
    }
}

impl Default for MonitorPolicy {
    fn default() -> Self {
        Self {
            offline_poll: Duration::from_secs(60),
            retry_initial: Duration::from_secs(5),
            retry_max: Duration::from_secs(300),
        }
    }
}

pub struct RecordingService {
    store: Arc<Mutex<TaskStore>>,
    recorder: Arc<dyn StreamRecorder>,
    resolvers: HashMap<crate::model::Platform, Arc<dyn StreamResolver>>,
    stops: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
}

impl RecordingService {
    pub fn new<R>(store: TaskStore, recorder: R, resolvers: Vec<Arc<dyn StreamResolver>>) -> Self
    where
        R: StreamRecorder + 'static,
    {
        Self {
            store: Arc::new(Mutex::new(store)),
            recorder: Arc::new(recorder),
            resolvers: resolvers
                .into_iter()
                .map(|resolver| (resolver.platform(), resolver))
                .collect(),
            stops: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Resolves and records one live stream. It is suitable for the manual
    /// `record` action; transient resolver and recorder failures are retried
    /// with the default bounded policy. Use [`Self::monitor`] for unattended
    /// recording that should continue until explicitly stopped.
    pub async fn start(&self, task: RecordingTask) -> Result<()> {
        self.start_with_retry(task, RetryPolicy::default()).await
    }

    pub async fn start_with_retry(&self, task: RecordingTask, policy: RetryPolicy) -> Result<()> {
        let (sender, receiver) = watch::channel(false);
        self.stops.lock().await.insert(task.id, sender);
        let mut stop = receiver;
        let mut retry_delay = policy.retry_initial;
        let mut attempts = 0;
        let result = loop {
            if *stop.borrow() {
                self.set_status(task.id, TaskStatus::Stopped).await?;
                break Ok(());
            }
            match self.run_once(&task, stop.clone()).await {
                Ok(()) => break Ok(()),
                Err(error) if *stop.borrow() => break Err(error),
                Err(error) if attempts >= policy.max_retries => break Err(error),
                Err(error) => {
                    attempts += 1;
                    tracing::warn!(task = %task.id, retry = attempts, "recording attempt failed; retrying: {error:#}");
                    self.set_status(task.id, TaskStatus::Retrying).await?;
                    policy_wait(&mut stop, retry_delay).await?;
                    retry_delay = retry_delay.saturating_mul(2).min(policy.retry_max);
                }
            }
        };
        self.stops.lock().await.remove(&task.id);
        result
    }

    /// Monitors a room until [`Self::stop`] is called. This method does not
    /// require a UI or platform network integration and is therefore suitable
    /// for deterministic tests with resolver/recorder test doubles.
    pub async fn monitor(&self, task: RecordingTask, policy: MonitorPolicy) -> Result<()> {
        let (sender, mut stop) = watch::channel(false);
        self.stops.lock().await.insert(task.id, sender);
        let resolver = match self.resolvers.get(&task.platform).cloned() {
            Some(resolver) => resolver,
            None => {
                self.stops.lock().await.remove(&task.id);
                bail!("no resolver registered for {}", task.platform.as_str());
            }
        };
        let mut retry_delay = policy.retry_initial;
        let outcome = loop {
            if *stop.borrow() {
                break Ok(());
            }
            self.set_status(task.id, TaskStatus::Resolving).await?;
            match resolver.resolve(&task).await {
                Ok(stream) => {
                    retry_delay = policy.retry_initial;
                    match self.record_resolved(&task, &stream, stop.clone()).await {
                        Ok(_) if *stop.borrow() => break Ok(()),
                        Ok(_) => {
                            // A monitor is long-lived: an FFmpeg exit means the
                            // live edge ended or rotated, not that the user's
                            // recording request is complete. Re-resolve after
                            // the short retry delay so a transient CDN break
                            // does not require another page click.
                            self.set_status(task.id, TaskStatus::Retrying).await?;
                            policy_wait(&mut stop, retry_delay).await?;
                            retry_delay = retry_delay.saturating_mul(2).min(policy.retry_max);
                        }
                        Err(error) if *stop.borrow() => break Err(error),
                        Err(error) => {
                            tracing::warn!(task = %task.id, "recording ended with error: {error:#}");
                            self.set_status(task.id, TaskStatus::Retrying).await?;
                            policy_wait(&mut stop, retry_delay).await?;
                            retry_delay = retry_delay.saturating_mul(2).min(policy.retry_max);
                        }
                    }
                }
                Err(ResolveError::Offline) => {
                    self.set_status(task.id, TaskStatus::Idle).await?;
                    policy_wait(&mut stop, policy.offline_poll).await?;
                }
                Err(error) => {
                    tracing::warn!(task = %task.id, "stream resolution failed: {error}");
                    self.set_status(task.id, TaskStatus::Retrying).await?;
                    policy_wait(&mut stop, retry_delay).await?;
                    retry_delay = retry_delay.saturating_mul(2).min(policy.retry_max);
                }
            }
        };
        self.stops.lock().await.remove(&task.id);
        self.set_status(task.id, TaskStatus::Stopped).await?;
        outcome
    }

    async fn run_once(&self, task: &RecordingTask, stop: watch::Receiver<bool>) -> Result<()> {
        let resolver = self.resolvers.get(&task.platform).cloned().ok_or_else(|| {
            anyhow::anyhow!("no resolver registered for {}", task.platform.as_str())
        })?;
        self.set_status(task.id, TaskStatus::Resolving).await?;
        let stream = match resolver.resolve(task).await {
            Ok(stream) => stream,
            Err(error) => {
                self.set_status(task.id, TaskStatus::Failed).await?;
                bail!(error);
            }
        };
        let mut task = task.clone();
        if task.platform == crate::model::Platform::Douyin
            && is_usable_douyin_display_name(&stream.title)
            && task.display_name != stream.title
        {
            self.store
                .lock()
                .await
                .update_definition(task.id, &stream.title, &task.output_dir)?;
            task.display_name = stream.title.clone();
        }
        let result = self.record_resolved(&task, &stream, stop.clone()).await;
        self.set_status(
            task.id,
            if *stop.borrow() {
                TaskStatus::Stopped
            } else if result.is_ok() {
                TaskStatus::Completed
            } else {
                TaskStatus::Failed
            },
        )
        .await?;
        result.map(|_| ())
    }

    async fn record_resolved(
        &self,
        task: &RecordingTask,
        stream: &crate::model::ResolvedStream,
        stop: watch::Receiver<bool>,
    ) -> Result<std::path::PathBuf> {
        self.set_status(task.id, TaskStatus::Recording).await?;
        let session_id = self
            .store
            .lock()
            .await
            // `ResolvedStream::started_at` can be the time a platform
            // observed a media URL (notably WeChat Channels), which may
            // precede the actual recorder start. Persist the instant this
            // recording session is opened instead.
            .begin_session(task.id, &stream.title, Utc::now())?;
        let result = self.recorder.record(task, stream, stop).await;
        let completion = match &result {
            Ok(output_path) => {
                self.store
                    .lock()
                    .await
                    .finish_session(session_id, Some(output_path), None)
            }
            Err(error) => self.store.lock().await.finish_session(
                session_id,
                None,
                Some(&format!("{error:#}")),
            ),
        };
        completion?;
        result
    }

    async fn set_status(&self, id: Uuid, status: TaskStatus) -> Result<()> {
        self.store.lock().await.update_status(id, status)
    }

    pub async fn stop(&self, id: Uuid) -> Result<()> {
        let sender = self
            .stops
            .lock()
            .await
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("task is not being monitored or recorded"))?;
        sender.send(true)?;
        Ok(())
    }

    pub async fn task(&self, id: Uuid) -> Result<Option<RecordingTask>> {
        self.store.lock().await.get(id)
    }
}

/// Do not persist page-template placeholders as a task's broadcaster name.
/// The resolver applies the same rule, but this is a final safeguard for any
/// alternate Douyin resolver implementation.
fn is_usable_douyin_display_name(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "douyin live"
                | "undefined"
                | "$undefined"
                | "null"
                | "$null"
                | "抖音直播"
                | "抖音直播间"
        )
}

async fn policy_wait(stop: &mut watch::Receiver<bool>, delay: Duration) -> Result<()> {
    tokio::select! {
        _ = sleep(delay) => Ok(()),
        changed = stop.changed() => { changed?; Ok(()) }
    }
}
