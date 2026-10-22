use super::{
    bridge::ProbeBridge,
    engine::{Engine, RulesConfig},
    http::ProbeServer,
    inject::{ProbeSidecar, render_addon_with_injection},
    journal::ProbeJournal,
    protocol::ProbeScope,
    scheduler::SchedulerConfig,
};
use anyhow::{Result, bail};
use std::path::Path;
use url::Url;

#[derive(Clone, Default)]
pub struct ProbeOptions {
    pub delivery: super::delivery::DeliveryConfig,
    pub timed_messages: super::timed::TimedConfig,
    pub likes: super::likes::LikeConfig,
    pub keyword_replies: super::keywords::KeywordConfig,
    pub recording: super::recording::RecordingConfig,
    pub retention: super::retention::RetentionPolicy,
    pub raw_messages: bool,
    pub automatic_send: bool,
    pub candidate_fill_only: bool,
    pub candidate_preview: super::candidate_preview::CandidatePreviewConfig,
    pub system_proxy: bool,
    pub tls_passthrough: bool,
    pub tls_only: bool,
    pub http2: Option<bool>,
    pub rules_preview: bool,
    pub scheduler_preview: bool,
    pub fill_preview: bool,
    pub fill_test_text: Option<String>,
    pub rules_config: RulesConfig,
    pub scheduler_config: SchedulerConfig,
}

pub async fn run_probe(
    room: Url,
    directory: &Path,
    port: u16,
    executable: &Path,
    duration_seconds: u64,
) -> Result<()> {
    run_probe_with_preview(room, directory, port, executable, duration_seconds, false).await
}

pub async fn run_probe_with_preview(
    room: Url,
    directory: &Path,
    port: u16,
    executable: &Path,
    duration_seconds: u64,
    rules_preview: bool,
) -> Result<()> {
    run_probe_with_scheduler(
        room,
        directory,
        port,
        executable,
        duration_seconds,
        rules_preview,
        false,
    )
    .await
}

pub async fn run_probe_with_scheduler(
    room: Url,
    directory: &Path,
    port: u16,
    executable: &Path,
    duration_seconds: u64,
    rules_preview: bool,
    scheduler_preview: bool,
) -> Result<()> {
    run_probe_with_options(
        room,
        directory,
        port,
        executable,
        duration_seconds,
        ProbeOptions {
            rules_preview,
            scheduler_preview,
            ..Default::default()
        },
    )
    .await
}

pub async fn run_probe_with_options(
    room: Url,
    directory: &Path,
    port: u16,
    executable: &Path,
    duration_seconds: u64,
    options: ProbeOptions,
) -> Result<()> {
    let ProbeOptions {
        delivery,
        timed_messages,
        likes,
        keyword_replies,
        recording,
        retention,
        raw_messages,
        automatic_send,
        candidate_fill_only,
        candidate_preview,
        system_proxy,
        tls_passthrough,
        tls_only,
        http2,
        rules_preview,
        scheduler_preview,
        fill_preview,
        fill_test_text,
        rules_config,
        scheduler_config,
    } = options;
    retention.validate()?;
    recording.validate()?;
    delivery.validate()?;
    timed_messages.validate(rules_config.max_text_chars)?;
    keyword_replies.validate(rules_config.max_text_chars)?;
    if keyword_replies.enabled
        && (!automatic_send
            || (!fill_preview && !candidate_fill_only)
            || fill_test_text.is_some()
            || tls_only
            || tls_passthrough)
    {
        bail!("keyword_replies requires automatic sending in an injection fill mode");
    }
    if timed_messages.enabled
        && (!automatic_send
            || (!fill_preview && !candidate_fill_only)
            || fill_test_text.is_some()
            || tls_only
            || tls_passthrough)
    {
        bail!("timed_messages requires automatic sending in an injection fill mode");
    }
    Engine::new(rules_config.clone())?;
    scheduler_config.validate()?;
    if fill_test_text.is_some() && !fill_preview && !candidate_fill_only {
        bail!("fill-test-text requires fill-preview");
    }
    if let Some(text) = &fill_test_text {
        super::fill::validate_text(text)?;
    }
    if duration_seconds > 3600 {
        bail!("probe duration must be at most 3600 seconds; 0 waits for Ctrl-C");
    }
    let scope = ProbeScope::from_room_url(room)?;
    if (system_proxy || tls_passthrough || tls_only) && scope.platform() != "wechat_channels" {
        bail!("system proxy management requires the WeChat desktop adapter");
    }
    if tls_passthrough && tls_only {
        bail!("tls_only and tls_passthrough cannot be enabled together");
    }
    if scope.platform() == "wechat_channels" && (rules_preview || scheduler_preview || fill_preview)
    {
        bail!("WeChat desktop injection currently supports observation only");
    }
    candidate_preview.validate(scope.platform(), !tls_only && !tls_passthrough)?;
    if candidate_fill_only
        && (scope.platform() != "wechat_channels"
            || !candidate_preview.enabled
            || rules_preview
            || scheduler_preview
            || fill_preview)
    {
        bail!(
            "candidate_fill_only requires enabled WeChat candidate preview without other rule modes"
        );
    }
    if automatic_send
        && ((!fill_preview && !candidate_fill_only)
            || fill_test_text.is_some()
            || tls_only
            || tls_passthrough)
    {
        bail!("automatic sending requires an injection fill mode without test text");
    }
    // Subscribe before native proxy/certificate commands so Ctrl-C during startup is retained.
    let mut stop_signal = super::shutdown::ShutdownSignals::new()?;
    std::fs::create_dir_all(directory)?;
    let directory = directory.canonicalize()?;
    let database = directory.join("moderation-probe.sqlite3");
    let mut bridge = ProbeBridge::new(scope.clone(), ProbeJournal::open(&database)?);
    bridge.configure_ui(recording.enabled, false);
    bridge.configure_delivery(delivery)?;
    bridge.configure_timed(timed_messages)?;
    bridge.configure_likes(likes)?;
    bridge.configure_keywords(keyword_replies)?;
    bridge.configure_retention(retention)?;
    bridge.maintain_journal(true)?;
    bridge.configure_raw_messages(raw_messages)?;
    bridge.configure(rules_config, scheduler_config)?;
    bridge.configure_candidate_preview(candidate_preview)?;
    if candidate_fill_only {
        bridge.enable_candidate_fill_preview(fill_test_text)?;
    } else if fill_preview {
        bridge.enable_fill_preview(fill_test_text)?;
    } else if scheduler_preview {
        bridge.enable_scheduler_preview();
    } else if rules_preview {
        bridge.enable_rules_preview();
    }
    if automatic_send {
        bridge.enable_automatic_send()?;
    }
    let mut server = ProbeServer::bind(bridge).await?;
    let source = {
        let bridge = server
            .bridge
            .lock()
            .map_err(|_| anyhow::anyhow!("probe lock poisoned"))?;
        render_addon_with_injection(
            &scope,
            server.address,
            &bridge.relay_token,
            !tls_only && !tls_passthrough,
        )
    };
    let mut recording_runtime = if recording.enabled && !tls_only && !tls_passthrough {
        match super::recording::RecordingRuntime::start(
            recording,
            &directory,
            vec![scope.room_url.clone()],
            &source,
        )
        .await
        {
            Ok(runtime) => Some(runtime),
            Err(error) => {
                let _ = server.stop().await;
                return Err(error);
            }
        }
    } else {
        None
    };
    let source = recording_runtime
        .as_ref()
        .map_or(source.as_str(), |runtime| runtime.source.as_str());
    let sidecar = ProbeSidecar::start_many_with_recording(
        executable,
        &directory,
        port,
        source,
        std::slice::from_ref(&scope),
        tls_passthrough,
        http2.unwrap_or(true),
        recording_runtime.is_some(),
    )
    .await;
    let mut sidecar = match sidecar {
        Ok(sidecar) => sidecar,
        Err(error) => {
            server.stop().await?;
            return Err(error);
        }
    };
    let proxy = if system_proxy {
        // mitmdump creates the CA before it begins accepting connections.
        match super::system_proxy::start(&directory, port, !tls_passthrough) {
            Ok(proxy) => Some(proxy),
            Err(error) => {
                let _ = sidecar.stop().await;
                let _ = server.stop().await;
                return Err(error);
            }
        }
    } else {
        None
    };
    println!(
        "Moderation: {} (automatic sending: {automatic_send})",
        scope.room_url
    );
    if tls_passthrough {
        println!(
            "TLS passthrough diagnostic: HTTPS is forwarded unchanged; no button or page injection is expected."
        );
    }
    if tls_only {
        println!(
            "TLS-only diagnostic: selected platform TLS is inspected, but HTML is unchanged and no scripts, buttons or bridge polling are injected."
        );
    }
    if !http2.unwrap_or(true) {
        println!(
            "HTTP/1 compatibility diagnostic: HTTP/2 disabled for inspected connections; TLS passthrough connections remain unchanged."
        );
    }
    if system_proxy {
        println!(
            "System HTTP/HTTPS proxy active: 127.0.0.1:{port}; previous settings will be restored on exit. Reopen the WeChat live page."
        );
    } else {
        println!(
            "Browser proxy: 127.0.0.1:{port}. System proxy and certificate trust were not changed."
        );
    }
    if !system_proxy {
        println!(
            "Use a browser/profile configured for this proxy and its local CA, then reload the room."
        );
    }
    println!(
        "Local CA: {}",
        directory.join("mitmproxy-ca-cert.pem").display()
    );
    println!(
        "Journal: {} (local messages; raw payload capture: {raw_messages})",
        database.display()
    );
    println!(
        "Automatic sending: {automatic_send}; actor identity remains unverified. Rules preview: {}. Scheduler preview: {}. Fill-only: {fill_preview}. Ctrl-C stops the probe.",
        rules_preview || scheduler_preview || fill_preview,
        scheduler_preview || fill_preview
    );
    let waiting = tokio::select! {
        signal = stop_signal.recv() => signal.ok_or_else(||anyhow::anyhow!("interrupt signal stream ended")),
        result = sidecar.wait_for_exit() => result,
        _ = tokio::time::sleep(std::time::Duration::from_secs(duration_seconds)), if duration_seconds != 0 => Ok(()),
    };
    let proxy_stop = proxy.as_ref().map_or(Ok(()), |proxy| proxy.stop());
    if waiting.is_ok() {
        println!(
            "Stopping collection; waiting up to 8 seconds for buffered event acknowledgements. Ctrl-C again ends this wait."
        );
        let drained = tokio::select! {
            result = server.drain(std::time::Duration::from_secs(8)) => result.unwrap_or(false),
            _ = stop_signal.recv() => false,
        };
        println!(
            "Buffered event drain: {}",
            if drained {
                "sessions closed"
            } else {
                "incomplete; unconfirmed page events may remain"
            }
        );
    }
    let recording_stop = if let Some(runtime) = &mut recording_runtime {
        runtime.stop().await
    } else {
        Ok(())
    };
    let server_stop = server.stop().await;
    let sidecar_stop = sidecar.stop().await;
    if let Ok(mut bridge) = server.bridge.lock() {
        if let Err(error) = bridge.maintain_journal(true) {
            eprintln!("live-archive moderation: journal maintenance failed: {error}");
        }
    }
    let summary = server
        .bridge
        .lock()
        .map_err(|_| anyhow::anyhow!("probe lock poisoned"))?
        .summary();
    println!("{}", serde_json::to_string_pretty(&summary)?);
    recording_stop?;
    proxy_stop?;
    waiting?;
    sidecar_stop?;
    server_stop?;
    Ok(())
}
