use async_trait::async_trait;
use chrono::Utc;
use live_archive_core::{
    model::{Platform, RecordingTask, ResolvedStream, TaskStatus},
    platform::{ResolveError, StreamResolver},
    recorder::StreamRecorder,
    service::{MonitorPolicy, RecordingService, RetryPolicy},
    store::TaskStore,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;
use url::Url;
use uuid::Uuid;

struct OfflineResolver(AtomicUsize);

#[async_trait]
impl StreamResolver for OfflineResolver {
    fn platform(&self) -> Platform {
        Platform::Douyin
    }

    async fn resolve(&self, _task: &RecordingTask) -> Result<ResolvedStream, ResolveError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(ResolveError::Offline)
    }
}

struct NeverRecorder;

#[async_trait]
impl StreamRecorder for NeverRecorder {
    async fn record(
        &self,
        _task: &RecordingTask,
        _stream: &ResolvedStream,
        _stop: watch::Receiver<bool>,
    ) -> anyhow::Result<PathBuf> {
        panic!("offline rooms must not start the recorder")
    }
}

struct LiveResolver;

#[async_trait]
impl StreamResolver for LiveResolver {
    fn platform(&self) -> Platform {
        Platform::Douyin
    }

    async fn resolve(&self, task: &RecordingTask) -> Result<ResolvedStream, ResolveError> {
        Ok(ResolvedStream {
            stream_url: Url::parse("https://media.example.test/live.m3u8").unwrap(),
            title: task.display_name.clone(),
            started_at: Utc::now(),
        })
    }
}

struct ImmediateRecorder(Arc<AtomicUsize>);

#[async_trait]
impl StreamRecorder for ImmediateRecorder {
    async fn record(
        &self,
        task: &RecordingTask,
        _stream: &ResolvedStream,
        _stop: watch::Receiver<bool>,
    ) -> anyhow::Result<PathBuf> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(task.output_dir.join("recording.ts"))
    }
}

/// Simulates a dropped live edge followed by a healthy reconnect. The second
/// attempt remains active until the test explicitly stops the monitor.
struct FailsOnceThenRecords(Arc<AtomicUsize>);

#[async_trait]
impl StreamRecorder for FailsOnceThenRecords {
    async fn record(
        &self,
        task: &RecordingTask,
        _stream: &ResolvedStream,
        mut stop: watch::Receiver<bool>,
    ) -> anyhow::Result<PathBuf> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            anyhow::bail!("simulated CDN disconnect");
        }
        stop.changed().await?;
        Ok(task.output_dir.join("reconnected.ts"))
    }
}

struct FailsOnceThenCompletes(Arc<AtomicUsize>);

struct FailsOnStop;

#[async_trait]
impl StreamRecorder for FailsOnStop {
    async fn record(
        &self,
        _task: &RecordingTask,
        _stream: &ResolvedStream,
        mut stop: watch::Receiver<bool>,
    ) -> anyhow::Result<PathBuf> {
        while !*stop.borrow_and_update() {
            stop.changed().await?;
        }
        anyhow::bail!("forced termination may leave an incomplete final segment")
    }
}

#[tokio::test]
async fn stop_failures_are_returned_and_saved_without_retrying() {
    for monitoring in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("tasks.sqlite3");
        let store = TaskStore::open(&database).unwrap();
        let recording_task = task(temporary.path());
        store.insert(&recording_task).unwrap();
        let id = recording_task.id;
        let service = Arc::new(RecordingService::new(
            store,
            FailsOnStop,
            vec![Arc::new(LiveResolver)],
        ));
        let running = Arc::clone(&service);
        let job = tokio::spawn(async move {
            if monitoring {
                running
                    .monitor(recording_task, MonitorPolicy::default())
                    .await
            } else {
                running.start(recording_task).await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while service.task(id).await.unwrap().unwrap().status != TaskStatus::Recording {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        service.stop(id).await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), job)
            .await
            .unwrap()
            .unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("forced termination")
        );
        assert_eq!(
            service.task(id).await.unwrap().unwrap().status,
            TaskStatus::Stopped
        );
        let sessions = TaskStore::open(database)
            .unwrap()
            .sessions_for_task(id)
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].finished_at.is_some());
        assert!(
            sessions[0]
                .error_message
                .as_ref()
                .unwrap()
                .contains("forced termination")
        );
        assert!(sessions[0].output_path.is_none());
        assert!(
            service.stop(id).await.is_err(),
            "stop handle must be released"
        );
    }
}

#[async_trait]
impl StreamRecorder for FailsOnceThenCompletes {
    async fn record(
        &self,
        task: &RecordingTask,
        _stream: &ResolvedStream,
        _stop: watch::Receiver<bool>,
    ) -> anyhow::Result<PathBuf> {
        if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
            anyhow::bail!("simulated transient transport error");
        }
        Ok(task.output_dir.join("recovered.ts"))
    }
}

fn task(output_dir: &Path) -> RecordingTask {
    let now = Utc::now();
    RecordingTask {
        id: Uuid::new_v4(),
        platform: Platform::Douyin,
        room_url: Url::parse("https://live.douyin.com/123").unwrap(),
        display_name: "creator".into(),
        status: TaskStatus::Idle,
        output_dir: output_dir.into(),
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn monitor_polls_offline_room_and_stops_cleanly() {
    let temporary = tempfile::tempdir().unwrap();
    let store = TaskStore::open(temporary.path().join("tasks.sqlite3")).unwrap();
    let recording_task = task(temporary.path());
    store.insert(&recording_task).unwrap();
    let resolver = Arc::new(OfflineResolver(AtomicUsize::new(0)));
    let service = Arc::new(RecordingService::new(
        store,
        NeverRecorder,
        vec![resolver.clone()],
    ));
    let running = {
        let service = Arc::clone(&service);
        let task = recording_task.clone();
        tokio::spawn(async move {
            service
                .monitor(
                    task,
                    MonitorPolicy {
                        offline_poll: Duration::from_millis(1),
                        retry_initial: Duration::from_millis(1),
                        retry_max: Duration::from_millis(2),
                    },
                )
                .await
        })
    };

    tokio::time::timeout(Duration::from_secs(2), async {
        while resolver.0.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    service.stop(recording_task.id).await.unwrap();
    running.await.unwrap().unwrap();

    assert!(resolver.0.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        service
            .task(recording_task.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Stopped
    );
}

#[tokio::test]
async fn manual_recording_reaches_completed_status() {
    let temporary = tempfile::tempdir().unwrap();
    let store = TaskStore::open(temporary.path().join("tasks.sqlite3")).unwrap();
    let recording_task = task(temporary.path());
    store.insert(&recording_task).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let service = RecordingService::new(
        store,
        ImmediateRecorder(Arc::clone(&calls)),
        vec![Arc::new(LiveResolver)],
    );

    service.start(recording_task.clone()).await.unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        service
            .task(recording_task.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Completed
    );
}

#[tokio::test]
async fn single_recording_retries_transient_failures_for_any_platform() {
    let temporary = tempfile::tempdir().unwrap();
    let store = TaskStore::open(temporary.path().join("tasks.sqlite3")).unwrap();
    let recording_task = task(temporary.path());
    store.insert(&recording_task).unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let service = RecordingService::new(
        store,
        FailsOnceThenCompletes(Arc::clone(&attempts)),
        vec![Arc::new(LiveResolver)],
    );

    service
        .start_with_retry(
            recording_task.clone(),
            RetryPolicy {
                max_retries: 1,
                retry_initial: Duration::from_millis(1),
                retry_max: Duration::from_millis(1),
            },
        )
        .await
        .unwrap();

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(
        service
            .task(recording_task.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Completed
    );
}

#[tokio::test]
async fn monitor_reconnects_after_a_recording_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let store = TaskStore::open(temporary.path().join("tasks.sqlite3")).unwrap();
    let recording_task = task(temporary.path());
    store.insert(&recording_task).unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let service = Arc::new(RecordingService::new(
        store,
        FailsOnceThenRecords(Arc::clone(&attempts)),
        vec![Arc::new(LiveResolver)],
    ));
    let running = {
        let service = Arc::clone(&service);
        let task = recording_task.clone();
        tokio::spawn(async move {
            service
                .monitor(
                    task,
                    MonitorPolicy {
                        offline_poll: Duration::from_millis(1),
                        retry_initial: Duration::from_millis(1),
                        retry_max: Duration::from_millis(2),
                    },
                )
                .await
        })
    };

    tokio::time::timeout(Duration::from_secs(2), async {
        while attempts.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(attempts.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        service
            .task(recording_task.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Recording
    );
    service.stop(recording_task.id).await.unwrap();
    running.await.unwrap().unwrap();
}
