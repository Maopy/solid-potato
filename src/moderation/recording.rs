//! Shared recording runtime hosted by the moderation process and proxy.
use crate::{
    capture_service::{CaptureIngestor, CapturePermit, LoopbackCaptureServer},
    capture_sidecar::{MitmproxySidecarConfig, render_addon},
    download_task_bridge::{DownloadTaskBridge, TaskSnapshot},
    model::ResolvedStream,
    model::{Platform, RecordingTask, TaskStatus},
    recorder::{FfmpegRecorder, RecorderConfig, StreamRecorder},
    store::TaskStore,
    wechat::WeChatCaptureStore,
    wechat_bridge::ChannelsBridge,
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use url::Url;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecordingConfig {
    pub enabled: bool,
    pub output_dir: Option<PathBuf>,
    pub ffmpeg: PathBuf,
    pub segment_seconds: u32,
}
impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            output_dir: None,
            ffmpeg: "ffmpeg".into(),
            segment_seconds: 1800,
        }
    }
}
impl RecordingConfig {
    pub fn validate(&self) -> Result<()> {
        if self.ffmpeg.as_os_str().is_empty()
            || self
                .output_dir
                .as_ref()
                .is_some_and(|p| p.as_os_str().is_empty())
            || !(1..=86400).contains(&self.segment_seconds)
        {
            bail!("invalid recording output_dir, ffmpeg or segment_seconds (1..86400)");
        }
        Ok(())
    }
    pub fn resolve(&mut self, base: &Path) {
        if let Some(p) = &mut self.output_dir {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        }
        if self.ffmpeg.is_relative() && self.ffmpeg.components().count() > 1 {
            self.ffmpeg = base.join(&self.ffmpeg);
        }
    }
}
#[derive(Clone)]
pub struct RecordingRequest {
    pub id: String,
    pub platform: Platform,
    pub page_url: Url,
    pub stream_url: Url,
    pub title: String,
}
pub struct RecordingRuntime {
    pub source: String,
    pub tasks: DownloadTaskBridge,
    pub requests: mpsc::Sender<RecordingRequest>,
    endpoint: LoopbackCaptureServer,
    stopping: watch::Sender<bool>,
    worker: Option<JoinHandle<Result<()>>>,
}
impl RecordingRuntime {
    pub async fn start(
        config: RecordingConfig,
        directory: &Path,
        rooms: Vec<Url>,
        moderation_source: &str,
    ) -> Result<Self> {
        config.validate()?;
        std::fs::create_dir_all(directory)?;
        let database = directory.join("recordings.sqlite3");
        TaskStore::open(&database)?;
        let permit = CapturePermit::generate();
        let channels = ChannelsBridge::bind().await?;
        let tasks = DownloadTaskBridge::default();
        let task_address = tasks.clone().bind().await?;
        let (sender, receiver) = mpsc::channel(64);
        let endpoint = LoopbackCaptureServer::bind(Arc::new(
            CaptureIngestor::new(Arc::new(WeChatCaptureStore::default()), permit.clone())
                .with_bridge(channels.clone())
                .with_task_bridge(tasks.clone())
                .with_recording_requests(sender.clone(), rooms.clone()),
        ))
        .await?;
        let capture = render_addon(&MitmproxySidecarConfig {
            executable: PathBuf::new(),
            working_directory: directory.into(),
            listen_port: 1,
            capture_endpoint: Url::parse(&format!("http://{}/capture", endpoint.address))?,
            channels_ws_url: format!("ws://{}/ws/channels", channels.address),
            downloader_ws_url: format!("ws://{task_address}/ws/v1/download_task"),
            permit,
        });
        let source = compose(moderation_source, &capture, &rooms)?;
        let (stopping, stop) = watch::channel(false);
        let output = config
            .output_dir
            .clone()
            .unwrap_or_else(|| directory.parent().unwrap_or(directory).join("recordings"));
        let output_display = output.display().to_string();
        let worker = tokio::spawn(run_worker(
            config,
            output,
            database,
            tasks.clone(),
            receiver,
            stop,
        ));
        println!("直播录制已接入场控；点击页面“录制直播”开始，输出目录：{output_display}");
        Ok(Self {
            source,
            tasks,
            requests: sender,
            endpoint,
            stopping,
            worker: Some(worker),
        })
    }
    pub async fn stop(&mut self) -> Result<()> {
        let _ = self.endpoint.stop();
        let _ = self.stopping.send(true);
        if let Some(worker) = self.worker.take() {
            worker.await.context("recording worker panicked")??;
        }
        Ok(())
    }
}
impl Drop for RecordingRuntime {
    fn drop(&mut self) {
        let _ = self.endpoint.stop();
        let _ = self.stopping.send(true);
    }
}
fn compose(moderation: &str, capture: &str, rooms: &[Url]) -> Result<String> {
    Ok(include_str!("../../assets/moderation/proxy/recording.py")
        .replace("__MODERATION_SOURCE__", &serde_json::to_string(moderation)?)
        .replace("__CAPTURE_SOURCE__", &serde_json::to_string(capture)?)
        .replace(
            "__ROOMS__",
            &serde_json::to_string(&serde_json::to_string(rooms)?)?,
        )
        .replace(
            "__DOUYIN_SCRIPT__",
            &serde_json::to_string(include_str!(
                "../../assets/moderation/adapters/douyin-recording.js"
            ))?,
        ))
}
struct Entry {
    request: RecordingRequest,
    task: RecordingTask,
    running: Option<(watch::Sender<bool>, JoinHandle<Result<()>>)>,
    removed: bool,
}
async fn run_worker(
    config: RecordingConfig,
    output: PathBuf,
    database: PathBuf,
    tasks: DownloadTaskBridge,
    mut requests: mpsc::Receiver<RecordingRequest>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let store = TaskStore::open(&database)?;
    // A restart never resumes an old signed URL without another page click.
    for task in store.list()? {
        if matches!(
            task.status,
            TaskStatus::Recording | TaskStatus::Resolving | TaskStatus::Retrying
        ) {
            store.update_status(task.id, TaskStatus::Stopped)?;
        }
    }
    let mut entries: HashMap<String, Entry> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    loop {
        tokio::select! {
            changed=stop.changed()=>{if changed.is_err()||*stop.borrow(){break;}},
            request=requests.recv()=>if let Some(r)=request {
                if entries.len()>=512 {
                    tasks.publish(TaskSnapshot{id:r.id,status:"failed".into(),name:r.title,error:Some("录制任务数量已达上限".into())}).await;continue;
                }
                let now=Utc::now();
                let task=RecordingTask{id:Uuid::new_v4(),platform:r.platform,room_url:r.page_url.clone(),display_name:r.title.clone(),status:TaskStatus::Idle,output_dir:output.join(r.platform.as_str()).join(Uuid::new_v4().to_string()),created_at:now,updated_at:now};
                store.insert(&task)?;
                if tasks.detail(&r.id).await.is_none(){tasks.publish(TaskSnapshot{id:r.id.clone(),status:"waiting".into(),name:r.title.clone(),error:None}).await;}
                entries.insert(r.id.clone(),Entry{request:r,task,running:None,removed:false});
            },
            _=tick.tick()=>{
                let mut occupied:HashSet<_>=entries.values().filter(|e|e.running.is_some()).map(|e|(e.request.platform,e.request.page_url.clone())).collect();
                for (id,e) in &mut entries {
                    let desired=tasks.detail(id).await;
                    if desired.is_none(){e.removed=true;}
                    let wants=desired.as_ref().is_some_and(|s|matches!(s.status.as_str(),"waiting"|"downloading"));
                    if let Some((cancel,handle))=&e.running {
                        if !wants {let _=cancel.send(true);}
                        if !handle.is_finished(){continue;}
                        let (cancel,handle)=e.running.take().unwrap();
                        let cancelled=*cancel.borrow();
                        let result=handle.await.context("recording task panicked").and_then(|r|r);
                        let current=tasks.detail(id).await;
                        if current.is_some() && (result.is_err() || !cancelled && current.as_ref().is_some_and(|s|s.status=="downloading")) {
                            tasks.publish(TaskSnapshot{id:id.clone(),status:if result.is_ok(){"finished"}else{"failed"}.into(),name:e.request.title.clone(),error:result.err().map(|e|format!("{e:#}"))}).await;
                        }
                        continue;
                    }
                    if wants {
                        if !occupied.insert((e.request.platform,e.request.page_url.clone())) {
                            tasks.publish(TaskSnapshot{id:id.clone(),status:"failed".into(),name:e.request.title.clone(),error:Some("该直播已在录制，请使用原任务".into())}).await;
                            continue;
                        }
                        let (cancel,stop)=watch::channel(false);
                        tasks.publish(TaskSnapshot{id:id.clone(),status:"downloading".into(),name:e.request.title.clone(),error:None}).await;
                        let task=e.task.clone();let request=e.request.clone();let config=config.clone();let database=database.clone();
                        let handle=tokio::spawn(record_one(task,request,config,database,stop));
                        e.running=Some((cancel,handle));
                    }
                }
                let removed:Vec<_>=entries.iter().filter(|(_,e)|e.removed&&e.running.is_none()).map(|(id,_)|id.clone()).collect();
                for id in removed {if let Some(e)=entries.remove(&id){store.delete(e.task.id)?;}}
            }
        }
    }
    requests.close();
    for e in entries.values() {
        if let Some((cancel, _)) = &e.running {
            let _ = cancel.send(true);
        }
    }
    let mut errors = vec![];
    for (id, mut e) in entries {
        if let Some((_, handle)) = e.running.take() {
            let result = handle
                .await
                .context("recording task panicked")
                .and_then(|r| r);
            store.update_status(
                e.task.id,
                if result.is_ok() {
                    TaskStatus::Stopped
                } else {
                    TaskStatus::Failed
                },
            )?;
            let error = result.err().map(|e| format!("{e:#}"));
            if let Some(error) = &error {
                errors.push(error.clone());
            }
            if tasks.detail(&id).await.is_some() {
                tasks
                    .publish(TaskSnapshot {
                        id,
                        status: if error.is_some() { "failed" } else { "paused" }.into(),
                        name: e.request.title,
                        error,
                    })
                    .await;
            }
        }
    }
    if !errors.is_empty() {
        bail!("recording shutdown: {}", errors.join("; "));
    }
    Ok(())
}
async fn record_one(
    task: RecordingTask,
    r: RecordingRequest,
    config: RecordingConfig,
    database: PathBuf,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    let store = TaskStore::open(database)?;
    let recorder = FfmpegRecorder::new(RecorderConfig {
        ffmpeg_path: config.ffmpeg,
        segment_seconds: config.segment_seconds,
    });
    for attempt in 0..=3 {
        if *stop.borrow() {
            store.update_status(task.id, TaskStatus::Stopped)?;
            return Ok(());
        }
        store.update_status(task.id, TaskStatus::Recording)?;
        let session = store.begin_session(task.id, &r.title, Utc::now())?;
        let result = recorder
            .record(
                &task,
                &ResolvedStream {
                    stream_url: r.stream_url.clone(),
                    title: r.title.clone(),
                    started_at: Utc::now(),
                },
                stop.clone(),
            )
            .await;
        store.finish_session(
            session,
            result.as_ref().ok().map(|p| p.as_path()),
            result.as_ref().err().map(|e| e.to_string()).as_deref(),
        )?;
        if *stop.borrow() {
            store.update_status(
                task.id,
                if result.is_ok() {
                    TaskStatus::Stopped
                } else {
                    TaskStatus::Failed
                },
            )?;
            return result.map(|_| ());
        }
        match result {
            Ok(_) => {
                store.update_status(task.id, TaskStatus::Completed)?;
                return Ok(());
            }
            Err(error) if attempt == 3 => {
                store.update_status(task.id, TaskStatus::Failed)?;
                return Err(error);
            }
            Err(_) => {
                store.update_status(task.id, TaskStatus::Retrying)?;
            }
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_secs(5<<attempt))=>{},_=stop.changed()=>{}}
    }
    Ok(())
}
