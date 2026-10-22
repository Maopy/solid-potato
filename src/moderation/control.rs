//! One controller owns the proxy and independently scoped room runtimes.
use super::{
    account_budget::AccountBudget,
    bridge::ProbeBridge,
    config::{InjectionConfig, ModerationConfig},
    http::ProbeServer,
    inject::{ProbeSidecar, render_addon_with_prefix},
    journal::ProbeJournal,
    protocol::{ProbeScope, identifier},
    scheduler::SchedulerConfig,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

fn default_enabled() -> bool {
    true
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    pub id: String,
    pub platform: String,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    pub id: String,
    #[serde(default)]
    pub label: String,
    pub account_id: String,
    pub config: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlConfig {
    pub version: u32,
    #[serde(default)]
    pub dashboard: bool,
    #[serde(default = "default_enabled")]
    pub start_enabled: bool,
    #[serde(default)]
    pub douyin_userscript: super::userscript::UserscriptConfig,
    pub directory: PathBuf,
    #[serde(default)]
    pub recording: super::recording::RecordingConfig,
    #[serde(default)]
    pub injection: InjectionConfig,
    #[serde(default)]
    pub duration_seconds: u64,
    pub accounts: Vec<AccountConfig>,
    pub tasks: Vec<TaskConfig>,
}
pub struct LoadedTask {
    pub id: String,
    pub label: String,
    pub account_id: String,
    pub config: ModerationConfig,
}
pub struct LoadedControl {
    pub config: ControlConfig,
    pub tasks: Vec<LoadedTask>,
}
impl LoadedControl {
    pub fn load(path: &Path) -> Result<Self> {
        let mut source = Vec::new();
        std::fs::File::open(path)
            .context("open controller YAML")?
            .take(65537)
            .read_to_end(&mut source)?;
        if source.len() > 65536 {
            bail!("controller YAML exceeds 64 KiB");
        }
        let mut c: ControlConfig =
            serde_yaml_ng::from_slice(&source).context("parse controller YAML")?;
        if c.version != 2
            || c.tasks.is_empty()
            || c.tasks.len() > 32
            || c.accounts.is_empty()
            || c.accounts.len() > 32
        {
            bail!("controller requires version=2 and 1..32 accounts/tasks");
        }
        if c.directory.as_os_str().is_empty()
            || c.injection.proxy_port == 0
            || c.injection.mitmdump.as_os_str().is_empty()
            || c.injection.tls_only
            || c.injection.tls_passthrough
            || c.duration_seconds > 3600
        {
            bail!(
                "invalid shared injection/storage settings; TLS diagnostic modes use the legacy run entry"
            );
        }
        let base = path
            .canonicalize()?
            .parent()
            .context("controller config parent")?
            .to_path_buf();
        if c.douyin_userscript.enabled
            && (c.douyin_userscript.port == 0 || c.douyin_userscript.port == c.injection.proxy_port)
        {
            bail!("userscript port must be nonzero and distinct from proxy port");
        }
        if let Some(p) = &mut c.douyin_userscript.key_file {
            if p.as_os_str().is_empty() {
                bail!("userscript key_file cannot be empty");
            }
            if p.is_relative() {
                *p = base.join(&*p);
            }
        }
        c.recording.validate()?;
        c.recording.resolve(&base);
        if c.directory.is_relative() {
            c.directory = base.join(&c.directory);
        }
        if c.injection.mitmdump.is_relative() && c.injection.mitmdump.components().count() > 1 {
            c.injection.mitmdump = base.join(&c.injection.mitmdump);
        }
        let mut accounts = BTreeMap::new();
        for a in &c.accounts {
            if !task_id(&a.id)
                || !identifier(&a.platform, 64)
                || accounts.insert(a.id.clone(), a.platform.clone()).is_some()
            {
                bail!("invalid/duplicate account ID or platform");
            }
            a.scheduler.validate()?;
        }
        let mut ids = BTreeSet::new();
        let mut rooms = BTreeSet::new();
        let mut tasks = vec![];
        for t in &c.tasks {
            if !task_id(&t.id)
                || !ids.insert(t.id.clone())
                || t.label.chars().count() > 80
                || t.label.chars().any(char::is_control)
            {
                bail!("invalid/duplicate task ID or label");
            }
            let p = if t.config.is_absolute() {
                t.config.clone()
            } else {
                base.join(&t.config)
            };
            let config = ModerationConfig::load(&p).with_context(|| format!("task {}", t.id))?;
            if accounts.get(&t.account_id) != Some(&config.room.platform) {
                bail!("task {} account missing or platform mismatch", t.id);
            }
            if config.injection.tls_only
                || config.injection.tls_passthrough
                || config.duration_seconds != 0
            {
                bail!(
                    "task {}: use controller duration and injection settings",
                    t.id
                );
            }
            if !rooms.insert((
                t.account_id.clone(),
                config.room.url.to_string(),
                config.candidate_preview.recipient_user_id.clone(),
            )) {
                bail!("duplicate account/room/recipient task");
            }
            tasks.push(LoadedTask {
                id: t.id.clone(),
                label: if t.label.is_empty() {
                    t.id.clone()
                } else {
                    t.label.clone()
                },
                account_id: t.account_id.clone(),
                config,
            });
        }
        if c.douyin_userscript.enabled
            && tasks
                .iter()
                .filter(|t| t.config.room.platform == "douyin")
                .count()
                != 1
        {
            bail!(
                "userscript mode requires exactly one Douyin task as the template for all opened rooms"
            );
        }
        if c.injection.system_proxy
            && !tasks
                .iter()
                .any(|t| t.config.room.platform == "wechat_channels")
        {
            bail!("system proxy management requires a WeChat task");
        }
        Ok(Self { config: c, tasks })
    }
}
fn task_id(s: &str) -> bool {
    identifier(s, 64)
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub struct TaskRuntime {
    pub id: String,
    pub label: String,
    pub account_id: String,
    pub server: ProbeServer,
    pub scope: ProbeScope,
}
pub struct Controller {
    pub tasks: BTreeMap<String, TaskRuntime>,
    pub userscript: Option<super::userscript::UserscriptHub>,
}
impl Controller {
    pub async fn bind(loaded: &LoadedControl) -> Result<Self> {
        let mut budgets = BTreeMap::new();
        for a in &loaded.config.accounts {
            budgets.insert(
                a.id.clone(),
                Arc::new(Mutex::new(AccountBudget::new(a.scheduler.clone())?)),
            );
        }
        let mut result = Self {
            tasks: BTreeMap::new(),
            userscript: loaded.config.douyin_userscript.enabled.then(|| {
                super::userscript::UserscriptHub::new(
                    loaded.config.directory.clone(),
                    loaded.config.recording.enabled,
                )
            }),
        };
        for task in &loaded.tasks {
            if task.config.room.platform == "douyin" {
                if let Some(hub) = &result.userscript {
                    hub.add_template(
                        &task.id,
                        &task.account_id,
                        task.config.probe_options(),
                        budgets[&task.account_id].clone(),
                    )?;
                    continue;
                }
            }
            let started = async {
                let directory = loaded.config.directory.join("tasks").join(&task.id);
                std::fs::create_dir_all(&directory)?;
                let mut bridge = ProbeBridge::new(
                    ProbeScope::from_room_url(task.config.room.url.clone())?,
                    ProbeJournal::open(&directory.join("moderation-probe.sqlite3"))?,
                );
                let options = task.config.probe_options();
                bridge.configure_task(&task.id, budgets[&task.account_id].clone())?;
                bridge.configure_ui(loaded.config.recording.enabled, loaded.config.dashboard);
                bridge.set_moderation_enabled(loaded.config.start_enabled)?;
                bridge.configure_delivery(options.delivery)?;
                bridge.configure_timed(options.timed_messages)?;
                bridge.configure_likes(options.likes)?;
                bridge.configure_keywords(options.keyword_replies)?;
                bridge.configure_retention(options.retention)?;
                bridge.maintain_journal(true)?;
                bridge.configure_raw_messages(options.raw_messages)?;
                bridge.configure(options.rules_config, options.scheduler_config)?;
                bridge.configure_candidate_preview(options.candidate_preview)?;
                if options.candidate_fill_only {
                    bridge.enable_candidate_fill_preview(options.fill_test_text)?;
                } else if options.fill_preview {
                    bridge.enable_fill_preview(options.fill_test_text)?;
                } else if options.scheduler_preview {
                    bridge.enable_scheduler_preview();
                } else if options.rules_preview {
                    bridge.enable_rules_preview();
                }
                if options.automatic_send {
                    bridge.enable_automatic_send()?;
                }
                let scope = bridge.scope.clone();
                let server = ProbeServer::bind(bridge).await?;
                Ok::<_, anyhow::Error>(TaskRuntime {
                    id: task.id.clone(),
                    label: task.label.clone(),
                    account_id: task.account_id.clone(),
                    server,
                    scope,
                })
            }
            .await;
            match started {
                Ok(runtime) => {
                    result.tasks.insert(task.id.clone(), runtime);
                }
                Err(error) => {
                    let _ = result.stop().await;
                    return Err(error);
                }
            }
        }
        Ok(result)
    }
    pub fn addon(&self) -> Result<String> {
        let mut routes = vec![];
        for task in self.tasks.values() {
            let b = task
                .server
                .bridge
                .lock()
                .map_err(|_| anyhow::anyhow!("task bridge poisoned"))?;
            let prefix = format!("/__live_archive/moderation/v2/tasks/{}/", task.id);
            routes.push(json!({"id":task.id,"label":task.label,"account":task.account_id,"host":task.scope.room_url.host_str(),"path":task.scope.room_url.path(),"prefix":prefix,"source":render_addon_with_prefix(&task.scope,task.server.address,&b.relay_token,true,&prefix)}));
        }
        Ok(
            include_str!("../../assets/moderation/proxy/controller.py").replace(
                "__ROUTES_JSON__",
                &serde_json::to_string(&serde_json::to_string(&routes)?)?,
            ),
        )
    }
    pub fn status(&self) -> Value {
        json!({"tasks":self.tasks.values().map(|t|json!({"id":t.id,"label":t.label,"account_id":t.account_id,"platform":t.scope.platform(),"room":t.scope.room_url,"state":t.server.bridge.lock().ok().map(|mut b|b.summary())})).collect::<Vec<_>>()})
    }
    pub async fn stop_task(&mut self, id: &str) -> Result<()> {
        let Some(mut t) = self.tasks.remove(id) else {
            bail!("unknown task");
        };
        let drained = t.server.drain(Duration::from_secs(8)).await?;
        if !drained {
            eprintln!("live-archive moderation: task {id} drain incomplete");
        }
        t.server.stop().await?;
        t.server
            .bridge
            .lock()
            .map_err(|_| anyhow::anyhow!("task bridge poisoned"))?
            .maintain_journal(true)?;
        Ok(())
    }
    pub async fn stop(&mut self) -> Result<()> {
        // Drain concurrently so one closed/unresponsive page does not delay other rooms.
        let mut errors = vec![];
        if let Some(hub) = &self.userscript {
            if let Err(e) = hub.stop().await {
                errors.push(e.to_string());
            }
        }
        for t in self.tasks.values_mut() {
            t.server
                .bridge
                .lock()
                .map_err(|_| anyhow::anyhow!("task bridge poisoned"))?
                .begin_drain();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline
            && self
                .tasks
                .values()
                .any(|t| !t.server.bridge.lock().unwrap().drain_finished())
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for (_, mut t) in std::mem::take(&mut self.tasks) {
            if let Err(e) = t.server.stop().await {
                errors.push(e.to_string());
            }
            if let Ok(mut b) = t.server.bridge.lock() {
                if let Err(e) = b.maintain_journal(true) {
                    errors.push(e.to_string());
                }
            }
        }
        if !errors.is_empty() {
            bail!("{}", errors.join("; "));
        }
        Ok(())
    }
}

pub async fn serve(path: &Path) -> Result<()> {
    let loaded = LoadedControl::load(path)?;
    let mut interrupt = super::shutdown::ShutdownSignals::new()?;
    let mut controller = Controller::bind(&loaded).await?;
    let scopes: Vec<_> = controller.tasks.values().map(|t| t.scope.clone()).collect();
    let source = controller.addon()?;
    let c = &loaded.config;
    let mut recording_runtime = if c.recording.enabled {
        match super::recording::RecordingRuntime::start(
            c.recording.clone(),
            &c.directory,
            scopes.iter().map(|s| s.room_url.clone()).collect(),
            &source,
        )
        .await
        {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                let _ = controller.stop().await;
                return Err(error);
            }
        }
    } else {
        None
    };
    let source = recording_runtime
        .as_ref()
        .map_or(source.as_str(), |runtime| runtime.source.as_str());
    let userscript_server = if let Some(hub) = &controller.userscript {
        Some(
            super::userscript::UserscriptServer::bind(
                &c.douyin_userscript,
                &c.directory,
                hub.clone(),
                recording_runtime.as_ref().map(|r| r.tasks.clone()),
                recording_runtime.as_ref().map(|r| r.requests.clone()),
            )
            .await?,
        )
    } else {
        None
    };
    if let Some(s) = &userscript_server {
        println!("抖音脚本安装：{}", s.install_url);
    }
    let started = if scopes.is_empty() {
        Ok(None)
    } else {
        ProbeSidecar::start_many_with_recording(
            &c.injection.mitmdump,
            &c.directory,
            c.injection.proxy_port,
            source,
            &scopes,
            false,
            c.injection.http2,
            recording_runtime.is_some(),
        )
        .await
        .map(Some)
    };
    let mut sidecar = match started {
        Ok(s) => s,
        Err(e) => {
            let _ = controller.stop().await;
            return Err(e);
        }
    };
    let proxy = if c.injection.system_proxy {
        match super::system_proxy::start(&c.directory, c.injection.proxy_port, true) {
            Ok(p) => Some(p),
            Err(e) => {
                if let Some(s) = &mut sidecar {
                    let _ = s.stop().await;
                }
                let _ = controller.stop().await;
                return Err(e);
            }
        }
    } else {
        None
    };
    if sidecar.is_some() {
        println!(
            "Shared moderation proxy: 127.0.0.1:{}; CA: {}",
            c.injection.proxy_port,
            c.directory.join("mitmproxy-ca-cert.pem").display()
        );
    }
    println!("{}", serde_json::to_string_pretty(&controller.status())?);
    let mut dashboard = if c.dashboard {
        let dashboard = match super::dashboard::Dashboard::bind_with_userscript(
            &controller,
            recording_runtime.as_ref().map(|r| r.tasks.clone()),
            userscript_server.as_ref().map(|s| s.install_url.clone()),
        )
        .await
        {
            Ok(dashboard) => dashboard,
            Err(error) => {
                let _ = controller.stop().await;
                if let Some(runtime) = &mut recording_runtime {
                    let _ = runtime.stop().await;
                }
                if let Some(proxy) = &proxy {
                    let _ = proxy.stop();
                }
                if let Some(sidecar) = &mut sidecar {
                    let _ = sidecar.stop().await;
                }
                return Err(error);
            }
        };
        println!("统一控制页：{}", dashboard.url);
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open")
            .arg(&dashboard.url)
            .spawn();
        Some(dashboard)
    } else {
        None
    };
    let result = tokio::select! {_ = interrupt.recv()=>Ok(()),r=async {match &mut sidecar {Some(s)=>s.wait_for_exit().await,None=>std::future::pending().await}}=>r,_=tokio::time::sleep(Duration::from_secs(c.duration_seconds)),if c.duration_seconds!=0=>Ok(())};
    // Release network routing before waiting for recording finalization.
    let restored = proxy.as_ref().map_or(Ok(()), |p| p.stop());
    if let Some(dashboard) = &mut dashboard {
        dashboard.stop().await;
    }
    let stopped = controller.stop().await;
    if let Some(s) = userscript_server {
        s.stop().await;
    }
    let recording_stopped = if let Some(runtime) = &mut recording_runtime {
        runtime.stop().await
    } else {
        Ok(())
    };
    let proxy_stopped = if let Some(s) = &mut sidecar {
        s.stop().await
    } else {
        Ok(())
    };
    recording_stopped?;
    stopped?;
    restored?;
    proxy_stopped?;
    result
}
