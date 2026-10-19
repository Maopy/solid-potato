//! Replay is an isolated virtual-clock pipeline, never an attachment to a live
//! bridge. Inputs are normalized JSONL fixtures, not raw proxy journals.
use super::{
    engine::{Audit, Engine, RulesConfig},
    events::CoreEvent,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

const MAX_INPUT: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub at_ms: u64,
    #[serde(default)]
    pub event: Option<CoreEvent>,
    #[serde(default)]
    pub simulate_dispatch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReplaySummary {
    pub mode: &'static str,
    pub send_chat: bool,
    pub frames: usize,
    pub planned: usize,
    pub suppressed: usize,
    pub cancelled: usize,
    pub expired: usize,
    pub paused: usize,
    pub ended_at_ms: u64,
}

/// Stable sorting preserves file order between events sharing a timestamp.
/// All events at a deadline are ingested before advancing the virtual timer.
pub fn replay(
    mut frames: Vec<Frame>,
    config: RulesConfig,
    until: Option<u64>,
    mut emit: impl FnMut(&Audit) -> Result<()>,
) -> Result<ReplaySummary> {
    if frames.is_empty() || frames.len() > 10_000 {
        bail!("fixture needs 1..10000 frames");
    }
    for frame in &frames {
        if frame.at_ms > 86_400_000 || frame.event.is_some() && frame.simulate_dispatch.is_some() {
            bail!("fixture timestamps must be within 24h; a frame cannot also simulate dispatch");
        }
        if let Some(event) = &frame.event {
            event.validate()?;
        }
    }
    frames.sort_by_key(|frame| frame.at_ms);
    let last = frames.last().expect("nonempty").at_ms;
    let until = until.unwrap_or(last + 5_000);
    if until < last || until > 86_405_000 {
        bail!("until-ms must follow the last frame and remain within replay bounds");
    }
    let mut engine = Engine::new(config)?;
    let mut summary = ReplaySummary {
        mode: "offline_preview",
        send_chat: false,
        frames: frames.len(),
        planned: 0,
        suppressed: 0,
        cancelled: 0,
        expired: 0,
        paused: 0,
        ended_at_ms: until,
    };
    let mut publish = |logs: Vec<Audit>| -> Result<()> {
        for log in logs {
            match log.kind.as_str() {
                "planned" => summary.planned += 1,
                "suppressed" => summary.suppressed += 1,
                "cancelled" => summary.cancelled += 1,
                "expired" => summary.expired += 1,
                "paused" => summary.paused += 1,
                _ => {}
            }
            emit(&log)?;
        }
        Ok(())
    };
    let mut frames = frames.into_iter().peekable();
    while let Some(frame) = frames.next() {
        let now = frame.at_ms;
        let mut group = vec![frame];
        while frames.peek().is_some_and(|f| f.at_ms == now) {
            group.push(frames.next().expect("peeked"));
        }
        let mut dispatches = Vec::new();
        for frame in group {
            if let Some(event) = frame.event {
                publish(engine.ingest(now, event)?)?;
            }
            if let Some(id) = frame.simulate_dispatch {
                dispatches.push(id);
            }
        }
        publish(engine.advance(now)?)?;
        for id in dispatches {
            publish(engine.simulate_dispatch(now, &id)?)?;
        }
    }
    publish(engine.advance(until)?)?;
    Ok(summary)
}

pub fn run_replay(
    fixture: &Path,
    output: Option<&Path>,
    until: Option<u64>,
    allow_inferred_levels: bool,
) -> Result<()> {
    let mut bytes = Vec::new();
    File::open(fixture)
        .context("opening normalized replay fixture")?
        .take((MAX_INPUT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_INPUT {
        bail!("replay fixture exceeds 4MiB");
    }
    let source = String::from_utf8(bytes).context("fixture must be UTF-8 JSONL")?;
    let mut frames = Vec::new();
    for (index, line) in source.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if frames.len() >= 10_000 {
            bail!("fixture exceeds 10000 frames");
        }
        frames.push(
            serde_json::from_str::<Frame>(line)
                .with_context(|| format!("invalid replay frame at line {}", index + 1))?,
        );
    }
    let config = RulesConfig {
        allow_inferred_levels,
        ..Default::default()
    };
    // Fail malformed fixtures before opening an output. A second deterministic
    // pass writes the audit; no dispatch/network capability exists in either.
    replay(frames.clone(), config.clone(), until, |_| Ok(()))?;
    let mut writer: Box<dyn Write> = match output {
        Some(path) => {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            Box::new(
                options
                    .open(path)
                    .context("creating audit output (existing files are not overwritten)")?,
            )
        }
        None => Box::new(std::io::stdout()),
    };
    let summary = replay(frames, config, until, |audit| {
        serde_json::to_writer(&mut writer, audit)?;
        writer.write_all(b"\n")?;
        Ok(())
    })?;
    writer.flush()?;
    eprintln!("{}", serde_json::to_string(&summary)?);
    Ok(())
}
