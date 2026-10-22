use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand, ValueEnum};
use directories::ProjectDirs;
use live_archive_core::{
    config::{EXAMPLE_CONFIG, apply as apply_config, load as load_config},
    credentials::{CredentialStore, SystemCredentialStore, credential_key},
    doctor::{DoctorConfig, ProcessDoctorProbe, diagnose},
    douyin::{DouyinResolver, DouyinWebLookup},
    model::{Platform, RecordingTask, TaskStatus},
    recorder::{DirectStreamResolver, FfmpegRecorder, RecorderConfig},
    service::{MonitorPolicy, RecordingService},
    store::TaskStore,
};
use std::path::PathBuf;
use std::sync::Arc;
use url::Url;
use uuid::Uuid;

#[derive(Parser)]
#[command(version, about = "Live Archive core command-line interface")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[arg(long)]
    database: Option<PathBuf>,
    /// TOML file used by `config init` and `config apply`.
    #[arg(long)]
    config: Option<PathBuf>,
}
#[derive(Subcommand)]
enum Command {
    /// Live-room probes, draft filling and explicitly configured automatic submission.
    Moderation {
        #[command(subcommand)]
        command: ModerationCommand,
    },
    /// Performs read-only preflight checks. It never changes proxy, certificates, or files.
    Doctor {
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg: PathBuf,
        #[arg(long, default_value = "mitmdump")]
        mitmdump: PathBuf,
        #[arg(long, default_value = "data/recordings")]
        output_directory: PathBuf,
    },
    Add {
        #[arg(value_enum)]
        platform: PlatformArg,
        room_url: Url,
        #[arg(long)]
        name: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    List,
    /// Shows completed and failed recording attempts for a task.
    History {
        id: Uuid,
    },
    /// Manages platform login sessions in the operating-system credential store.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Imports rooms and authorized sessions from a TOML configuration file.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Records a supplied direct media URL for an existing task. The URL must
    /// have been acquired through an account/session you are authorized to use.
    Record {
        id: Uuid,
        stream_url: Url,
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg: PathBuf,
        #[arg(long, default_value_t = 1800)]
        segment_seconds: u32,
    },
    /// Resolves and records an added Douyin room using its saved authorized session.
    RecordDouyin {
        id: Uuid,
        #[arg(long, default_value = "default")]
        account: String,
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg: PathBuf,
        #[arg(long, default_value_t = 1800)]
        segment_seconds: u32,
    },
    /// Keeps monitoring a supplied direct media URL after each recording ends.
    /// Intended for authorized test streams and resolver integration testing.
    Watch {
        id: Uuid,
        stream_url: Url,
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg: PathBuf,
        #[arg(long, default_value_t = 1800)]
        segment_seconds: u32,
        #[arg(long, default_value_t = 60)]
        offline_poll_seconds: u64,
        #[arg(long, default_value_t = 5)]
        retry_initial_seconds: u64,
        #[arg(long, default_value_t = 300)]
        retry_max_seconds: u64,
    },
    /// Starts a consented, metadata-only multi-room WeChat Channels capture and recording session.
    WechatCapture {
        /// Optional legacy task ID. Omit it to create/reuse tasks from observed anchor identities.
        id: Option<Uuid>,
        /// Required acknowledgement before a local CA or system proxy can be changed.
        #[arg(long)]
        consent: bool,
        #[arg(long, default_value = "mitmdump")]
        mitmdump: PathBuf,
        #[arg(long)]
        working_directory: Option<PathBuf>,
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg: PathBuf,
    },
}
#[derive(Subcommand)]
enum AuthCommand {
    /// Reads a cookie/session value from standard input; it is never stored in SQLite.
    Set {
        #[arg(value_enum)]
        platform: PlatformArg,
        #[arg(long, default_value = "default")]
        account: String,
    },
    Status {
        #[arg(value_enum)]
        platform: PlatformArg,
        #[arg(long, default_value = "default")]
        account: String,
    },
    Delete {
        #[arg(value_enum)]
        platform: PlatformArg,
        #[arg(long, default_value = "default")]
        account: String,
    },
}
#[derive(Subcommand)]
enum ConfigCommand {
    /// Creates a commented configuration template without overwriting an existing file.
    Init,
    /// Saves configured sessions into the system credential store and syncs room tasks.
    Apply,
}
#[derive(Subcommand)]
enum ModerationCommand {
    #[command(hide = true)]
    ProxyWatchdog,
    /// Runs multiple room tasks behind one shared injection proxy.
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    /// Validates a multi-room controller config without starting anything.
    CheckController {
        #[arg(long)]
        config: PathBuf,
    },
    /// Read-only source target IDs from the latest run for manual YAML selection.
    InspectGiftTargets {
        #[arg(long)]
        journal: PathBuf,
    },
    /// Read-only, anonymized WeChat candidate count sequences; no rules/actions.
    InspectGifts {
        #[arg(long)]
        journal: PathBuf,
        /// Defaults to the latest run, even when it has no gift candidates.
        #[arg(long)]
        run_id: Option<String>,
        #[arg(long, default_value_t=2000, value_parser=clap::value_parser!(u32).range(1..=10000))]
        limit: u32,
    },
    /// Runs validated YAML; automatic submission is disabled by default.
    Run {
        #[arg(long)]
        config: PathBuf,
    },
    /// Validates YAML without starting the proxy or opening a journal.
    Check {
        #[arg(long)]
        config: PathBuf,
    },
    /// Replays normalized JSONL events using a virtual clock; cannot send chat.
    Replay {
        #[arg(long)]
        fixture: PathBuf,
        /// New audit JSONL file; defaults to stdout, never overwrites a file.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Default advances 5000ms past the final frame.
        #[arg(long)]
        until_ms: Option<u64>,
        #[arg(long)]
        allow_inferred_levels: bool,
    },
    /// Injects a supported page probe; WeChat desktop is observation-only.
    Probe {
        /// Decode candidate Douyin WS events and preview rules; never sends chat.
        #[arg(long)]
        rules_preview: bool,
        /// Simulate rate-limited dispatch timing; no live chat commands.
        #[arg(long, requires = "rules_preview")]
        scheduler_preview: bool,
        /// Fill the chat input only; never clicks send or submits a message.
        #[arg(long, requires = "scheduler_preview")]
        fill_preview: bool,
        /// One fixed fill-only test per document; disables automatic rule filling.
        #[arg(long, requires = "fill_preview")]
        fill_test_text: Option<String>,
        #[arg(long)]
        room: Url,
        #[arg(long, default_value = "data/moderation-probe")]
        directory: PathBuf,
        #[arg(long, default_value_t = 2024)]
        proxy_port: u16,
        #[arg(long, default_value = "mitmdump")]
        mitmdump: PathBuf,
        /// 0 waits for Ctrl-C; maximum is 3600 seconds.
        #[arg(long, default_value_t = 60)]
        duration_seconds: u64,
    },
}
#[derive(Clone, ValueEnum)]
enum PlatformArg {
    Douyin,
    WechatChannels,
}

impl From<PlatformArg> for Platform {
    fn from(value: PlatformArg) -> Self {
        match value {
            PlatformArg::Douyin => Self::Douyin,
            PlatformArg::WechatChannels => Self::WeChatChannels,
        }
    }
}

fn database_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.database {
        return Ok(path.clone());
    }
    let dirs = ProjectDirs::from("io", "livearchive", "LiveArchive")
        .context("finding application data directory")?;
    std::fs::create_dir_all(dirs.data_dir())?;
    Ok(dirs.data_dir().join("tasks.sqlite3"))
}

fn config_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.config {
        return Ok(path.clone());
    }
    // A project-local configuration is convenient for the CLI workflow and
    // avoids requiring `--config` on every invocation. The per-user location
    // remains the default when no local file exists.
    let local = std::env::current_dir()
        .context("finding current directory for configuration")?
        .join("live-archive.toml");
    if local.is_file() {
        return Ok(local);
    }
    let dirs = ProjectDirs::from("io", "livearchive", "LiveArchive")
        .context("finding application configuration directory")?;
    Ok(dirs.config_dir().join("live-archive.toml"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let cli = Cli::parse();
    // Probes use a separate journal and do not open/migrate the recorder DB.
    if let Command::Moderation { command } = &cli.command {
        match command {
            ModerationCommand::ProxyWatchdog => {
                return live_archive_core::moderation::proxy_watchdog::run();
            }
            ModerationCommand::InspectGiftTargets { journal } => {
                let report = live_archive_core::moderation::gift_report::inspect_targets(journal)?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            ModerationCommand::InspectGifts {
                journal,
                run_id,
                limit,
            } => {
                let report = live_archive_core::moderation::gift_report::inspect(
                    journal,
                    run_id.as_deref(),
                    *limit,
                )?;
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            ModerationCommand::Serve { config } => {
                return live_archive_core::moderation::control::serve(config).await;
            }
            ModerationCommand::CheckController { config } => {
                let loaded = live_archive_core::moderation::control::LoadedControl::load(config)?;
                println!(
                    "Controller configuration valid: {} tasks, {} accounts",
                    loaded.tasks.len(),
                    loaded.config.accounts.len()
                );
                return Ok(());
            }
            ModerationCommand::Check { config } => {
                let config = live_archive_core::moderation::config::ModerationConfig::load(config)?;
                println!(
                    "Configuration valid: mode={:?}, room={}, send_chat={}",
                    config.mode, config.room.url, config.sending.automatic
                );
                return Ok(());
            }
            ModerationCommand::Run { config } => {
                let config = live_archive_core::moderation::config::ModerationConfig::load(config)?;
                let options = config.probe_options();
                return live_archive_core::moderation::service::run_probe_with_options(
                    config.room.url,
                    &config.storage.directory,
                    config.injection.proxy_port,
                    &config.injection.mitmdump,
                    config.duration_seconds,
                    options,
                )
                .await;
            }
            ModerationCommand::Replay {
                fixture,
                output,
                until_ms,
                allow_inferred_levels,
            } => {
                return live_archive_core::moderation::replay::run_replay(
                    fixture,
                    output.as_deref(),
                    *until_ms,
                    *allow_inferred_levels,
                );
            }
            ModerationCommand::Probe {
                room,
                directory,
                proxy_port,
                mitmdump,
                duration_seconds,
                rules_preview,
                scheduler_preview,
                fill_preview,
                fill_test_text,
            } => {
                return live_archive_core::moderation::service::run_probe_with_options(
                    room.clone(),
                    directory,
                    *proxy_port,
                    mitmdump,
                    *duration_seconds,
                    live_archive_core::moderation::service::ProbeOptions {
                        rules_preview: *rules_preview,
                        scheduler_preview: *scheduler_preview,
                        fill_preview: *fill_preview,
                        fill_test_text: fill_test_text.clone(),
                        ..Default::default()
                    },
                )
                .await;
            }
        }
    }
    let config = config_path(&cli)?;
    let database = database_path(&cli)?;
    let store = TaskStore::open(database)?;
    match cli.command {
        Command::Moderation { .. } => {
            unreachable!("handled before recorder database initialization")
        }
        Command::Doctor {
            ffmpeg,
            mitmdump,
            output_directory,
        } => {
            #[cfg(target_os = "macos")]
            let network_service = live_archive_core::wechat_proxy::active_macos_network_service()?;
            #[cfg(not(target_os = "macos"))]
            let network_service = String::new();
            let reports = diagnose(
                &ProcessDoctorProbe,
                &DoctorConfig {
                    ffmpeg,
                    mitmdump,
                    output_directory,
                    network_service,
                },
            );
            let has_failure = reports
                .iter()
                .any(|report| report.level == live_archive_core::doctor::CheckLevel::Fail);
            for report in reports {
                println!(
                    "[{}] {}: {}",
                    report.level.as_str(),
                    report.name,
                    report.detail
                );
            }
            if has_failure {
                anyhow::bail!(
                    "doctor found one or more required dependencies or settings that need attention"
                );
            }
        }
        Command::Auth { command } => {
            let credentials = SystemCredentialStore;
            match command {
                AuthCommand::Set { platform, account } => {
                    use std::io::Read;
                    let mut secret = String::new();
                    std::io::stdin().read_to_string(&mut secret)?;
                    let secret = secret.trim();
                    if secret.is_empty() {
                        anyhow::bail!("stdin did not contain a session value");
                    }
                    credentials.put(&credential_key(platform.into(), &account), secret)?;
                    println!("Session saved in the system credential store.");
                }
                AuthCommand::Status { platform, account } => {
                    let present = credentials
                        .get(&credential_key(platform.into(), &account))?
                        .is_some();
                    println!("{}", if present { "saved" } else { "missing" });
                }
                AuthCommand::Delete { platform, account } => {
                    credentials.delete(&credential_key(platform.into(), &account))?;
                    println!("Session removed from the system credential store.");
                }
            }
        }
        Command::Config { command } => match command {
            ConfigCommand::Init => {
                if config.exists() {
                    anyhow::bail!(
                        "configuration already exists at {}; edit it or choose another --config path",
                        config.display()
                    );
                }
                if let Some(parent) = config.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&config, EXAMPLE_CONFIG)?;
                println!("Configuration template created at {}", config.display());
                println!(
                    "Edit it, then run: live-archive-core --config '{}' config apply",
                    config.display()
                );
            }
            ConfigCommand::Apply => {
                let summary = apply_config(load_config(&config)?, &store, &SystemCredentialStore)?;
                println!(
                    "Configuration applied: {} session(s) saved; {} room(s) added; {} room(s) updated.",
                    summary.sessions_saved, summary.rooms_added, summary.rooms_updated
                );
            }
        },
        Command::Add {
            platform,
            room_url,
            name,
            output,
        } => {
            let output_dir = output.unwrap_or_else(|| {
                PathBuf::from("data/recordings").join(Platform::from(platform.clone()).as_str())
            });
            let now = Utc::now();
            let task = RecordingTask {
                id: Uuid::new_v4(),
                platform: platform.into(),
                room_url,
                display_name: name,
                status: TaskStatus::Idle,
                output_dir,
                created_at: now,
                updated_at: now,
            };
            store.insert(&task)?;
            println!("Added task {}", task.id);
        }
        Command::List => {
            for task in store.list()? {
                println!(
                    "{}\t{}\t{}\t{}",
                    task.id,
                    task.platform.as_str(),
                    task.status.as_str(),
                    task.room_url
                );
            }
        }
        Command::History { id } => {
            for session in store.sessions_for_task(id)? {
                println!(
                    "{}\t{}\t{}\t{}",
                    session.started_at,
                    session
                        .finished_at
                        .map(|time| time.to_rfc3339())
                        .unwrap_or_else(|| "running".into()),
                    session
                        .output_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .or(session.error_message)
                        .unwrap_or_else(|| "-".into()),
                    session.title,
                );
            }
        }
        Command::Record {
            id,
            stream_url,
            ffmpeg,
            segment_seconds,
        } => {
            let task = store
                .get(id)?
                .with_context(|| format!("task {id} does not exist"))?;
            let resolver = DirectStreamResolver::new(task.platform, stream_url);
            let service = Arc::new(RecordingService::new(
                store,
                FfmpegRecorder::new(RecorderConfig {
                    ffmpeg_path: ffmpeg,
                    segment_seconds,
                }),
                vec![resolver],
            ));
            println!(
                "Recording {} — press Ctrl-C to stop; transient failures retry automatically.",
                task.display_name
            );
            let running_service = Arc::clone(&service);
            let mut recording = tokio::spawn(async move { running_service.start(task).await });
            tokio::select! {
                result = &mut recording => result.context("recording task panicked")??,
                signal = tokio::signal::ctrl_c() => {
                    signal.context("waiting for Ctrl-C")?;
                    service.stop(id).await?;
                    println!("Stopping FFmpeg...");
                    recording.await.context("recording task panicked")??;
                }
            }
        }
        Command::RecordDouyin {
            id,
            account,
            ffmpeg,
            segment_seconds,
        } => {
            if segment_seconds == 0 {
                anyhow::bail!("segment-seconds must be greater than zero");
            }
            let task = store
                .get(id)?
                .with_context(|| format!("task {id} does not exist"))?;
            if task.platform != Platform::Douyin {
                anyhow::bail!("task {id} is not a Douyin task");
            }
            let lookup = Arc::new(DouyinWebLookup::new().map_err(|error| anyhow::anyhow!(error))?);
            let resolver = Arc::new(DouyinResolver::new(
                account,
                Arc::new(SystemCredentialStore),
                lookup,
            ));
            let service = Arc::new(RecordingService::new(
                store,
                FfmpegRecorder::new(RecorderConfig {
                    ffmpeg_path: ffmpeg,
                    segment_seconds,
                }),
                vec![resolver],
            ));
            println!(
                "Resolving and recording {} — press Ctrl-C to stop; transient failures retry automatically.",
                task.display_name
            );
            let running_service = Arc::clone(&service);
            let mut recording = tokio::spawn(async move { running_service.start(task).await });
            let mut command = tokio::task::spawn_blocking(|| -> Result<String> {
                use std::io::BufRead;
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line)?;
                Ok(line)
            });
            tokio::select! {
                result = &mut recording => result.context("recording task panicked")??,
                signal = tokio::signal::ctrl_c() => {
                    signal.context("waiting for Ctrl-C")?;
                    service.stop(id).await?;
                    println!("Stopping FFmpeg...");
                    recording.await.context("recording task panicked")??;
                }
                command_result = &mut command => {
                    let command = command_result.context("waiting for recording command")??;
                    match command.trim() {
                        "stop" | "quit" | "" => {
                            service.stop(id).await?;
                            println!("Stopping FFmpeg...");
                            recording.await.context("recording task panicked")??;
                        }
                        _ => anyhow::bail!("unknown recording command; use 'stop'"),
                    }
                }
            }
        }
        Command::Watch {
            id,
            stream_url,
            ffmpeg,
            segment_seconds,
            offline_poll_seconds,
            retry_initial_seconds,
            retry_max_seconds,
        } => {
            if segment_seconds == 0 || offline_poll_seconds == 0 || retry_initial_seconds == 0 {
                anyhow::bail!("all interval values must be greater than zero");
            }
            let task = store
                .get(id)?
                .with_context(|| format!("task {id} does not exist"))?;
            let resolver = DirectStreamResolver::new(task.platform, stream_url);
            let service = Arc::new(RecordingService::new(
                store,
                FfmpegRecorder::new(RecorderConfig {
                    ffmpeg_path: ffmpeg,
                    segment_seconds,
                }),
                vec![resolver],
            ));
            let policy = live_archive_core::service::MonitorPolicy {
                offline_poll: std::time::Duration::from_secs(offline_poll_seconds),
                retry_initial: std::time::Duration::from_secs(retry_initial_seconds),
                retry_max: std::time::Duration::from_secs(
                    retry_max_seconds.max(retry_initial_seconds),
                ),
            };
            println!("Monitoring {} — press Ctrl-C to stop.", task.display_name);
            let running_service = Arc::clone(&service);
            let mut monitoring =
                tokio::spawn(async move { running_service.monitor(task, policy).await });
            tokio::select! {
                result = &mut monitoring => result.context("monitoring task panicked")??,
                signal = tokio::signal::ctrl_c() => {
                    signal.context("waiting for Ctrl-C")?;
                    service.stop(id).await?;
                    println!("Stopping monitor...");
                    monitoring.await.context("monitoring task panicked")??;
                }
            }
        }
        Command::WechatCapture {
            id,
            consent,
            mitmdump,
            working_directory,
            ffmpeg,
        } => {
            if !consent {
                anyhow::bail!(
                    "--consent is required before certificate installation or system proxy changes"
                );
            }
            let working_directory = match working_directory {
                Some(path) => path,
                None => ProjectDirs::from("io", "livearchive", "LiveArchive")
                    .context("finding application data directory")?
                    .data_dir()
                    .join("wechat-capture"),
            };
            run_wechat_capture(store, id, mitmdump, working_directory, ffmpeg).await?;
        }
    }
    Ok(())
}

async fn run_wechat_capture_with<C, M>(
    store: TaskStore,
    requested_id: Option<Uuid>,
    mitmdump: PathBuf,
    working_directory: PathBuf,
    ffmpeg: PathBuf,
    proxy: C,
    certificate: M,
) -> Result<()>
where
    C: live_archive_core::wechat_proxy::SystemProxyController,
    M: live_archive_core::wechat_proxy::LocalCertificateController,
{
    use live_archive_core::{
        capture_service::{CaptureIngestor, CapturePermit, LoopbackCaptureServer},
        capture_sidecar::{MitmproxySidecar, MitmproxySidecarConfig},
        download_task_bridge::DownloadTaskBridge,
        wechat::{WeChatCaptureStore, WeChatChannelsResolver},
        wechat_bridge::ChannelsBridge,
        wechat_proxy::ProxyConsent,
        wechat_runtime::WeChatCaptureRuntime,
    };
    if requested_id.is_some() {
        anyhow::bail!("WeChat Channels recordings are created only by an active listener");
    }
    // Video Channels tasks are runtime-only. Clear any rows left behind by an
    // interrupted listener before accepting a new capture session.
    remove_transient_wechat_tasks(&store)?;
    let captures = Arc::new(WeChatCaptureStore::default());
    let permit = CapturePermit::generate();
    let channels_bridge = ChannelsBridge::bind().await?;
    let task_bridge = DownloadTaskBridge::default();
    let task_bridge_address = task_bridge.clone().bind().await?;
    let endpoint = LoopbackCaptureServer::bind(Arc::new(
        CaptureIngestor::new(Arc::clone(&captures), permit.clone())
            .with_bridge(channels_bridge.clone())
            .with_task_bridge(task_bridge.clone()),
    ))
    .await?;
    let capture_url = Url::parse(&format!("http://{}/capture", endpoint.address))?;
    let sidecar = MitmproxySidecar::new(MitmproxySidecarConfig {
        executable: mitmdump,
        working_directory: working_directory.clone(),
        listen_port: 2023,
        capture_endpoint: capture_url,
        channels_ws_url: format!("ws://{}/ws/channels", channels_bridge.address),
        downloader_ws_url: format!("ws://{task_bridge_address}/ws/v1/download_task"),
        permit,
    })?;
    let mut runtime = WeChatCaptureRuntime::new(proxy, certificate, sidecar);
    if let Err(error) = runtime
        .start(ProxyConsent {
            install_local_ca: true,
            intercept_wechat_traffic: true,
        })
        .await
    {
        let _ = endpoint.stop();
        return Err(error);
    }
    println!(
        "WeChat capture is active. Click '录制直播' in WeChat to start. Commands: list | stop <number>|all | quit"
    );
    let mut started = std::collections::HashSet::new();
    let mut discovered = Vec::new();
    let mut active = std::collections::HashSet::new();
    let mut services: std::collections::HashMap<Uuid, Arc<RecordingService>> =
        std::collections::HashMap::new();
    let mut frontend_task_ids: std::collections::HashMap<Uuid, String> =
        std::collections::HashMap::new();
    let mut frontend_task_statuses: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut recordings = tokio::task::JoinSet::new();
    let mut command = tokio::task::spawn_blocking(|| -> Result<String> {
        use std::io::BufRead;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        Ok(line)
    });
    loop {
        for mut capture in captures.observed() {
            let Some(username) = capture.anchor_username.clone() else {
                continue;
            };
            if started.contains(&username) {
                continue;
            }
            let now = Utc::now();
            let task = RecordingTask {
                id: Uuid::new_v4(),
                platform: Platform::WeChatChannels,
                room_url: capture.page_url.clone(),
                display_name: format!("{} [{}]", capture.title, username),
                status: TaskStatus::Idle,
                output_dir: PathBuf::from("data/recordings/wechat_channels"),
                created_at: now,
                updated_at: now,
            };
            // This transient row is the GUI's cross-process handoff. It is
            // removed when the associated recording ends.
            store.insert(&task)?;
            println!("Discovered WeChat anchor: {} ({username})", capture.title);
            let frontend_task_id = capture.frontend_task_id.clone();
            capture.page_url = task.room_url.clone();
            captures.publish(capture)?;
            started.insert(username);
            discovered.push(task);
            println!(
                "Discovered [{}]: {}",
                discovered.len(),
                discovered.last().unwrap().display_name
            );
            // A page click is an explicit recording request, so begin FFmpeg
            // immediately. `start` remains available to resume a task after
            // it has been stopped from the terminal.
            let task = discovered.last().unwrap().clone();
            if active.insert(task.id) {
                if let Some(frontend_task_id) = frontend_task_id {
                    task_bridge
                        .publish(live_archive_core::download_task_bridge::TaskSnapshot {
                            id: frontend_task_id.clone(),
                            status: "downloading".into(),
                            name: task.display_name.clone(),
                            error: None,
                        })
                        .await;
                    frontend_task_statuses.insert(frontend_task_id.clone(), "downloading".into());
                    frontend_task_ids.insert(task.id, frontend_task_id);
                }
                let service = Arc::new(RecordingService::new(
                    store.reopen()?,
                    FfmpegRecorder::new(RecorderConfig {
                        ffmpeg_path: ffmpeg.clone(),
                        ..Default::default()
                    }),
                    vec![Arc::new(WeChatChannelsResolver::new(Arc::clone(&captures)))],
                ));
                let running = Arc::clone(&service);
                let task_id = task.id;
                recordings.spawn(async move {
                    (
                        task_id,
                        running.monitor(task, MonitorPolicy::default()).await,
                    )
                });
                services.insert(task_id, service);
                println!(
                    "Recording started: {}",
                    discovered.last().unwrap().display_name
                );
            }
        }
        tokio::select! {
            result = &mut command => {
                let line = result.context("waiting for CLI command")??;
                // The GUI owns this command's stdin. When it exits, read_line
                // returns EOF (an empty string), not an explicit `quit`.
                // Treat EOF as shutdown so mitmdump cannot outlive the GUI
                // and retain the fixed capture port.
                if line.is_empty() {
                    for (task_id, service) in &services { let _ = service.stop(*task_id).await; }
                    while recordings.join_next().await.is_some() {}
                    remove_transient_wechat_tasks(&store)?;
                    let _ = runtime.stop().await;
                    let _ = endpoint.stop();
                    return Ok(());
                }
                let parts = line.split_whitespace().collect::<Vec<_>>();
                match parts.as_slice() {
                    [] => {}
                    ["help"] => println!("Commands: list | start <number>|all | stop <number>|all | quit"),
                    ["list"] => for (index, task) in discovered.iter().enumerate() {
                        let status = match services.get(&task.id) {
                            Some(service) => service.task(task.id).await?.map(|task| task.status.as_str()).unwrap_or("unknown"),
                            None if active.contains(&task.id) => "starting",
                            None => "ready",
                        };
                        let (segments, bytes) = recording_output_summary(&task.output_dir);
                        println!("[{}] {} — {} · {} MP4 分段 · {}", index + 1, status, task.display_name, segments, format_byte_size(bytes));
                    },
                    ["start", selector] => {
                        let indices = selected_capture_indices(selector, &discovered)?;
                        for index in indices {
                            let task = discovered[index].clone();
                            if !active.insert(task.id) { continue; }
                            let service = Arc::new(RecordingService::new(
                                store.reopen()?,
                                FfmpegRecorder::new(RecorderConfig { ffmpeg_path: ffmpeg.clone(), ..Default::default() }),
                                vec![Arc::new(WeChatChannelsResolver::new(Arc::clone(&captures)))],
                            ));
                            let running = Arc::clone(&service);
                            let task_id = task.id;
                            recordings.spawn(async move {
                                (
                                    task_id,
                                    running.monitor(task, MonitorPolicy::default()).await,
                                )
                            });
                            services.insert(task_id, service);
                            println!("Recording started: {}", discovered[index].display_name);
                        }
                    }
                    ["stop", selector] => {
                        for index in selected_capture_indices(selector, &discovered)? {
                            let task_id = discovered[index].id;
                            if let Some(service) = services.get(&task_id) { service.stop(task_id).await?; }
                        }
                    }
                    ["quit"] | ["exit"] => {
                        for (task_id, service) in &services { let _ = service.stop(*task_id).await; }
                        while recordings.join_next().await.is_some() {}
                        remove_transient_wechat_tasks(&store)?;
                        let _ = runtime.stop().await;
                        let _ = endpoint.stop();
                        return Ok(());
                    }
                    _ => eprintln!("Unknown command. Use: list | start <number>|all | stop <number>|all | quit"),
                }
                command = tokio::task::spawn_blocking(|| -> Result<String> {
                    use std::io::BufRead;
                    let mut line = String::new();
                    std::io::stdin().lock().read_line(&mut line)?;
                    Ok(line)
                });
            }
            result = recordings.join_next(), if !recordings.is_empty() => {
                if let Some(result) = result {
                    let (task_id, outcome) = result.context("WeChat recording task panicked")?;
                    active.remove(&task_id);
                    let final_status = match services.get(&task_id) {
                        Some(service) => service.task(task_id).await?.map(|task| task.status),
                        None => None,
                    };
                    services.remove(&task_id);
                    match outcome {
                        Ok(()) => { if let Some(frontend_id) = frontend_task_ids.remove(&task_id) { let status = if final_status == Some(TaskStatus::Stopped) { "paused" } else { "finished" }; frontend_task_statuses.remove(&frontend_id); task_bridge.publish(live_archive_core::download_task_bridge::TaskSnapshot { id: frontend_id, status: status.into(), name: "微信视频号直播".into(), error: None }).await; } println!("Recording stopped: {task_id}") },
                        Err(error) => { if let Some(frontend_id) = frontend_task_ids.remove(&task_id) { frontend_task_statuses.remove(&frontend_id); task_bridge.publish(live_archive_core::download_task_bridge::TaskSnapshot { id: frontend_id, status: "failed".into(), name: "微信视频号直播".into(), error: Some(error.to_string()) }).await; } eprintln!("Recording failed for {task_id}: {error:#}") },
                    }
                    // The shared task row exists only while this capture is
                    // active, allowing the GUI list to mirror runtime state
                    // without retaining Video Channels history.
                    if let Err(error) = store.delete(task_id) {
                        tracing::warn!(%error, %task_id, "could not remove transient WeChat task");
                    }
                }
            }
            signal = tokio::signal::ctrl_c() => {
                signal?;
                for (task_id, service) in &services { let _ = service.stop(*task_id).await; }
                while recordings.join_next().await.is_some() {}
                remove_transient_wechat_tasks(&store)?;
                let _ = runtime.stop().await;
                let _ = endpoint.stop();
                return Ok(());
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                let mut updates = Vec::new();
                for (task_id, frontend_id) in &frontend_task_ids {
                    let Some(service) = services.get(task_id) else { continue };
                    let Some(task) = service.task(*task_id).await? else { continue };
                    let status = match task.status {
                        TaskStatus::Failed => "failed",
                        TaskStatus::Stopped => "paused",
                        TaskStatus::Completed => "finished",
                        TaskStatus::Idle | TaskStatus::Resolving | TaskStatus::Recording | TaskStatus::Retrying => "downloading",
                    };
                    if frontend_task_statuses.get(frontend_id).is_none_or(|current| current != status) {
                        updates.push((frontend_id.clone(), status.to_owned(), task.display_name));
                    }
                }
                for (frontend_id, status, name) in updates {
                    frontend_task_statuses.insert(frontend_id.clone(), status.clone());
                    task_bridge.publish(live_archive_core::download_task_bridge::TaskSnapshot { id: frontend_id, status, name, error: None }).await;
                }
            }
        }
    }
}

fn selected_capture_indices(selector: &str, tasks: &[RecordingTask]) -> Result<Vec<usize>> {
    if selector == "all" {
        return Ok((0..tasks.len()).collect());
    }
    if let Ok(id) = Uuid::parse_str(selector) {
        return tasks
            .iter()
            .position(|task| task.id == id)
            .map(|index| vec![index])
            .context("capture task ID is not active in this listener");
    }
    let index = selector
        .parse::<usize>()
        .context("capture selection must be a task ID, number, or 'all'")?;
    if index == 0 || index > tasks.len() {
        anyhow::bail!("capture selection {index} is out of range");
    }
    Ok(vec![index - 1])
}

fn remove_transient_wechat_tasks(store: &TaskStore) -> Result<()> {
    for task in store
        .list()?
        .into_iter()
        .filter(|task| task.platform == Platform::WeChatChannels)
    {
        store.delete(task.id)?;
    }
    Ok(())
}

fn recording_output_summary(directory: &std::path::Path) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return (0, 0);
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().is_some_and(|extension| extension == "mp4"))
                .then(|| std::fs::metadata(path).ok().map(|metadata| metadata.len()))
                .flatten()
        })
        .fold((0, 0), |(count, bytes), size| (count + 1, bytes + size))
}

fn format_byte_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(target_os = "macos")]
async fn run_wechat_capture(
    store: TaskStore,
    id: Option<Uuid>,
    mitmdump: PathBuf,
    working_directory: PathBuf,
    ffmpeg: PathBuf,
) -> Result<()> {
    use live_archive_core::wechat_proxy::{
        LocalCaCertificate, MacOsNetworkSetupProxyController, MacOsSecurityCertificateController,
        active_macos_network_service,
    };
    let certificate = LocalCaCertificate {
        common_name: "mitmproxy".into(),
        certificate_path: working_directory.join("mitmproxy-ca-cert.pem"),
    };
    let network_service = active_macos_network_service()?;
    run_wechat_capture_with(
        store,
        id,
        mitmdump,
        working_directory,
        ffmpeg,
        MacOsNetworkSetupProxyController::new(network_service),
        MacOsSecurityCertificateController::new(certificate),
    )
    .await
}

#[cfg(target_os = "windows")]
async fn run_wechat_capture(
    store: TaskStore,
    id: Option<Uuid>,
    mitmdump: PathBuf,
    working_directory: PathBuf,
    ffmpeg: PathBuf,
) -> Result<()> {
    use live_archive_core::wechat_proxy::{
        LocalCaCertificate, WindowsCertUtilCertificateController,
        WindowsInternetSettingsProxyController,
    };
    let certificate = LocalCaCertificate {
        common_name: "mitmproxy".into(),
        certificate_path: working_directory.join("mitmproxy-ca-cert.pem"),
    };
    run_wechat_capture_with(
        store,
        id,
        mitmdump,
        working_directory,
        ffmpeg,
        WindowsInternetSettingsProxyController::new(),
        WindowsCertUtilCertificateController::new(certificate),
    )
    .await
}

#[cfg(target_os = "linux")]
async fn run_wechat_capture(
    store: TaskStore,
    id: Option<Uuid>,
    mitmdump: PathBuf,
    working_directory: PathBuf,
    ffmpeg: PathBuf,
) -> Result<()> {
    use live_archive_core::wechat_proxy::{
        GnomeProxyController, LinuxUpdateCaCertificateController, LocalCaCertificate,
    };
    let certificate = LocalCaCertificate {
        common_name: "mitmproxy".into(),
        certificate_path: working_directory.join("mitmproxy-ca-cert.pem"),
    };
    let installed_path =
        PathBuf::from("/usr/local/share/ca-certificates/live-archive-mitmproxy-ca.crt");
    run_wechat_capture_with(
        store,
        id,
        mitmdump,
        working_directory,
        ffmpeg,
        GnomeProxyController::new(),
        LinuxUpdateCaCertificateController::new(certificate, installed_path),
    )
    .await
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
async fn run_wechat_capture(
    _store: TaskStore,
    _id: Option<Uuid>,
    _mitmdump: PathBuf,
    _working_directory: PathBuf,
    _ffmpeg: PathBuf,
) -> Result<()> {
    anyhow::bail!("wechat-capture is not supported on this operating system")
}
