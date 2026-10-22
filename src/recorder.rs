use crate::model::{RecordingTask, ResolvedStream};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::Local;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, ChildStdin, Command},
    sync::watch,
    time::timeout,
};
use url::Url;

#[derive(Debug, Clone)]
pub struct RecorderConfig {
    pub ffmpeg_path: PathBuf,
    pub segment_seconds: u32,
}

impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            ffmpeg_path: PathBuf::from("ffmpeg"),
            segment_seconds: 1800,
        }
    }
}

pub struct FfmpegRecorder {
    config: RecorderConfig,
}

#[async_trait]
pub trait StreamRecorder: Send + Sync {
    async fn record(
        &self,
        task: &RecordingTask,
        stream: &ResolvedStream,
        stop: watch::Receiver<bool>,
    ) -> Result<PathBuf>;
}

#[async_trait]
impl StreamRecorder for FfmpegRecorder {
    async fn record(
        &self,
        task: &RecordingTask,
        stream: &ResolvedStream,
        stop: watch::Receiver<bool>,
    ) -> Result<PathBuf> {
        if self.config.segment_seconds == 0 {
            bail!("segment-seconds must be greater than zero");
        }
        if *stop.borrow() {
            bail!("recording stopped before FFmpeg started");
        }
        self.record_inner(task, stream, stop).await
    }
}

impl FfmpegRecorder {
    pub fn new(config: RecorderConfig) -> Self {
        Self { config }
    }

    async fn record_inner(
        &self,
        task: &RecordingTask,
        stream: &ResolvedStream,
        mut stop: watch::Receiver<bool>,
    ) -> Result<PathBuf> {
        std::fs::create_dir_all(&task.output_dir).context("creating output directory")?;
        // Shared anchor folders must not recover/remux another active writer's TS.
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(task.output_dir.join(".recording.lock"))?;
        guard.try_lock().context("该主播目录已有录制正在运行")?;
        let _guard = RecordingDirectoryGuard(guard);
        self.recover_interrupted_segments(&task.output_dir).await;
        // Keep TS while FFmpeg is writing: it is resilient to an interrupted
        // live edge. Completed segments are remuxed to MP4 below.
        let segment_prefix = Local::now().format("%Y-%m-%d-%H-%M-%S").to_string();
        let index = next_segment_index(&task.output_dir, &segment_prefix)?;
        let output = task.output_dir.join(format!("{segment_prefix}-%03d.ts"));
        let mut child = Command::new(&self.config.ffmpeg_path)
            .args([
                "-stdin",
                "-y",
                "-i",
                stream.stream_url.as_str(),
                "-c",
                "copy",
                "-f",
                "segment",
                "-segment_time",
            ])
            .arg(self.config.segment_seconds.to_string())
            .arg("-segment_start_number")
            .arg(index.to_string())
            .arg(&output)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            // This process is awaited without consuming diagnostic output. A
            // piped stderr can be closed by the runtime and make FFmpeg abort
            // with SIGPIPE before the recording or TS-to-MP4 finalization.
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("starting FFmpeg; install FFmpeg or set its executable path")?;
        // Child::wait closes stdin when first polled. Keep the control pipe
        // separately so the natural-exit branch cannot disable cancellation.
        let mut control = child.stdin.take().context("opening FFmpeg control input")?;

        tokio::select! {
            status = child.wait() => {
                let status = status.context("waiting for FFmpeg")?;
                if status.success() {
                    self.finalize_segments(&task.output_dir, &segment_prefix).await
                } else {
                    // A broken live edge can still leave closed, valid
                    // segments behind. Finalize those before the common
                    // retry layer starts the next FFmpeg process.
                    if let Err(error) = self
                        .finalize_segments(&task.output_dir, &segment_prefix)
                        .await
                    {
                        tracing::warn!(%error, "could not finalize TS segments after FFmpeg failure");
                    }
                    bail!("FFmpeg exited with {status}")
                }
            }
            request = wait_for_stop(&mut stop) => {
                // FFmpeg's `q` command closes the muxer at a packet boundary.
                // Killing it immediately can leave a truncated H.264 packet
                // that remuxes successfully but fails when decoded.
                let stopped = stop_ffmpeg(&mut child, &mut control, Duration::from_secs(10)).await;
                let finalized = self.finalize_segments(&task.output_dir, &segment_prefix).await;
                // Salvage files after a forced stop, but do not report it as
                // a clean session: the last segment may still be incomplete.
                request?;
                stopped?;
                finalized
            }
        }
    }

    /// Remuxes each completed MPEG-TS segment to MP4 without re-encoding.
    /// The source is removed only after FFmpeg has successfully written the
    /// matching MP4, so an interrupted finalization leaves recoverable TS.
    async fn finalize_segments(&self, output_dir: &Path, segment_prefix: &str) -> Result<PathBuf> {
        let segments = completed_ts_segments(output_dir, segment_prefix)?;
        let first = segments
            .first()
            .context("recording ended before FFmpeg wrote a complete segment")?
            .with_extension("mp4");
        for segment in segments {
            let mp4 = segment.with_extension("mp4");
            let temporary_mp4 = temporary_mp4_path(&mp4);
            let status = Command::new(&self.config.ffmpeg_path)
                .args(["-nostdin", "-y", "-i"])
                .arg(&segment)
                .args(["-c", "copy", "-movflags", "+faststart"])
                .arg(&temporary_mp4)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await
                .with_context(|| format!("remuxing {} to MP4", segment.display()))?;
            if !status.success() {
                let _ = std::fs::remove_file(&temporary_mp4);
                bail!(
                    "FFmpeg could not remux {} to MP4: {status}",
                    segment.display()
                );
            }
            if std::fs::metadata(&temporary_mp4)
                .with_context(|| format!("checking remuxed MP4 {}", temporary_mp4.display()))?
                .len()
                == 0
            {
                let _ = std::fs::remove_file(&temporary_mp4);
                bail!("FFmpeg produced an empty MP4 for {}", segment.display());
            }
            std::fs::rename(&temporary_mp4, &mp4).with_context(|| {
                format!(
                    "publishing finalized MP4 {} as {}",
                    temporary_mp4.display(),
                    mp4.display()
                )
            })?;
            std::fs::remove_file(&segment)
                .with_context(|| format!("removing finalized TS segment {}", segment.display()))?;
        }
        tracing::info!(output = %first.display(), "finalized recording segments as MP4");
        Ok(first)
    }

    /// Converts TS files from a previous interrupted recorder process before
    /// opening a new FFmpeg process in the same task directory. Only the
    /// readable timestamp form generated by this recorder is recognized.
    async fn recover_interrupted_segments(&self, output_dir: &Path) {
        for prefix in interrupted_segment_prefixes(output_dir) {
            match self.finalize_segments(output_dir, &prefix).await {
                Ok(output) => {
                    tracing::info!(output = %output.display(), "recovered interrupted recording segment")
                }
                Err(error) => {
                    tracing::warn!(%error, prefix, "could not recover interrupted TS segments")
                }
            }
        }
    }
}

async fn wait_for_stop(stop: &mut watch::Receiver<bool>) -> Result<()> {
    loop {
        if *stop.borrow_and_update() {
            return Ok(());
        }
        stop.changed()
            .await
            .context("recording stop channel closed")?;
    }
}

async fn stop_ffmpeg(child: &mut Child, control: &mut ChildStdin, grace: Duration) -> Result<()> {
    let exit = timeout(grace, async {
        // A concurrent natural exit can close stdin before we write.
        // Its exit status below determines whether it finished cleanly.
        let _ = control.write_all(b"q\n").await;
        child.wait().await.context("waiting for FFmpeg to stop")
    })
    .await;
    match exit {
        Ok(status) => {
            let status = status?;
            if !status.success() {
                bail!("FFmpeg failed while stopping: {status}");
            }
            Ok(())
        }
        Err(_) => {
            child.kill().await.context("forcing FFmpeg to stop")?;
            bail!(
                "FFmpeg did not stop within {grace:?}; forced termination may leave an incomplete final segment"
            );
        }
    }
}

fn temporary_mp4_path(mp4: &Path) -> PathBuf {
    let stem = mp4.file_stem().unwrap_or_default().to_string_lossy();
    mp4.with_file_name(format!("{stem}.partial.mp4"))
}

fn completed_ts_segments(output_dir: &Path, segment_prefix: &str) -> Result<Vec<PathBuf>> {
    let mut segments = std::fs::read_dir(output_dir)
        .with_context(|| format!("reading recording directory {}", output_dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "ts")
                && path.file_stem().is_some_and(|stem| {
                    stem.to_string_lossy().starts_with(&format!(
                        "{segment_prefix}{}",
                        if is_new_timestamp(segment_prefix) {
                            "-"
                        } else {
                            "_"
                        }
                    ))
                })
        })
        .collect::<Vec<_>>();
    segments.sort();
    Ok(segments)
}

// The directory lock makes this allocation exclusive across processes.
fn next_segment_index(directory: &Path, prefix: &str) -> Result<u32> {
    let mut next = 0;
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if let Some(tail) = name.strip_prefix(&format!("{prefix}-")) {
            if let Some(index) = tail.split('.').next().and_then(|n| n.parse::<u32>().ok()) {
                next = next.max(
                    index
                        .checked_add(1)
                        .context("recording segment index exhausted")?,
                );
            }
        }
    }
    Ok(next)
}

struct RecordingDirectoryGuard(std::fs::File);
impl Drop for RecordingDirectoryGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn interrupted_segment_prefixes(output_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(output_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter_map(|path| generated_segment_prefix(&path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn generated_segment_prefix(path: &Path) -> Option<String> {
    if path.extension().is_none_or(|extension| extension != "ts") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let (prefix, index) = stem.rsplit_once('_').or_else(|| stem.rsplit_once('-'))?;
    if index.len() < 3 || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if has_recording_timestamp(prefix) {
        return Some(prefix.to_owned());
    }
    None
}

fn is_new_timestamp(prefix: &str) -> bool {
    prefix.len() == 19 && chrono::NaiveDateTime::parse_from_str(prefix, "%Y-%m-%d-%H-%M-%S").is_ok()
}

fn has_recording_timestamp(prefix: &str) -> bool {
    if is_new_timestamp(prefix) {
        return true;
    }
    let (before_time, time) = prefix.rsplit_once('_').unwrap_or(("", ""));
    if time.len() == 6 && time.bytes().all(|byte| byte.is_ascii_digit()) {
        let (name, date) = before_time.rsplit_once('_').unwrap_or(("", ""));
        if !name.is_empty() && date.len() == 8 && date.bytes().all(|byte| byte.is_ascii_digit()) {
            return true;
        }
    }
    // A same-second collision uses a readable numeric suffix: `_2`, `_3`, …
    let (without_sequence, sequence) = prefix.rsplit_once('_').unwrap_or(("", ""));
    sequence.parse::<u32>().ok().is_some_and(|value| value >= 2)
        && has_recording_timestamp(without_sequence)
}

pub(crate) fn safe_file_name(name: &str) -> String {
    let raw: String = name
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                '_'
            } else {
                ch
            }
        })
        .take(80)
        .collect();
    let end = raw
        .char_indices()
        .find_map(|(i, _)| (i > 200).then_some(i))
        .unwrap_or(raw.len());
    let clean = raw[..end].trim_matches(|c: char| c == '.' || c.is_whitespace());
    if clean.is_empty() {
        "未命名主播".into()
    } else {
        clean.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_wait_ignores_false_updates_and_observes_existing_request() {
        let (sender, mut stop) = watch::channel(false);
        sender.send(false).unwrap();
        assert!(
            timeout(Duration::from_millis(20), wait_for_stop(&mut stop))
                .await
                .is_err()
        );
        sender.send(true).unwrap();
        wait_for_stop(&mut stop).await.unwrap();
    }

    #[tokio::test]
    async fn closed_stop_channel_reports_an_error() {
        let (sender, mut stop) = watch::channel(false);
        drop(sender);
        assert!(wait_for_stop(&mut stop).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn graceful_stop_waits_for_process_flush_and_checks_exit_status() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("closed");
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "read command; test \"$command\" = q || exit 2; echo closed > \"$1\"",
                "test",
            ])
            .arg(&marker)
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut control = child.stdin.take().unwrap();
        stop_ffmpeg(&mut child, &mut control, Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "closed\n");

        let mut child = Command::new("/bin/sh")
            .args(["-c", "read command; exit 7"])
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut control = child.stdin.take().unwrap();
        assert!(
            stop_ffmpeg(&mut child, &mut control, Duration::from_secs(2))
                .await
                .unwrap_err()
                .to_string()
                .contains("failed while stopping")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unresponsive_process_is_reaped_and_not_reported_as_clean_stop() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec sleep 30"])
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut control = child.stdin.take().unwrap();
        let error = stop_ffmpeg(&mut child, &mut control, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("forced termination"));
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn finds_only_completed_ts_segments_for_one_recording() {
        let directory = tempfile::tempdir().unwrap();
        for name in [
            "stream_000.ts",
            "stream_001.ts",
            "other_000.ts",
            "stream_002.mp4",
        ] {
            std::fs::write(directory.path().join(name), b"test").unwrap();
        }
        let paths = completed_ts_segments(directory.path(), "stream").unwrap();
        assert_eq!(
            paths
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec!["stream_000.ts", "stream_001.ts"]
        );
    }

    #[test]
    fn temporary_mp4_keeps_a_recognizable_mp4_extension() {
        assert_eq!(
            temporary_mp4_path(Path::new("recording_000.mp4")),
            PathBuf::from("recording_000.partial.mp4")
        );
    }

    #[test]
    fn identifies_only_readable_recorder_segments() {
        let directory = tempfile::tempdir().unwrap();
        for name in [
            "主播_20260904_123456_000.ts",
            "主播_20260904_123456_2_001.ts",
            "主播_20260904_123456_deadbeef_000.ts",
            "主播_20260904_123456_deadbeef_001.ts",
            "random_000.ts",
            "主播_20260904_123456_not-hex_000.ts",
            "主播_20260904_123456_deadbeef_002.mp4",
        ] {
            std::fs::write(directory.path().join(name), b"test").unwrap();
        }
        assert_eq!(
            interrupted_segment_prefixes(directory.path()),
            vec!["主播_20260904_123456", "主播_20260904_123456_2",]
        );
    }

    #[test]
    fn directory_lease_prevents_recovery_of_another_active_recording() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".recording.lock");
        let first = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .unwrap();
        first.try_lock().unwrap();
        let guard = RecordingDirectoryGuard(first);
        let second = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(second.try_lock().is_err());
        drop(guard);
        second.try_lock().unwrap();
        let _guard = RecordingDirectoryGuard(second);
    }

    #[test]
    fn same_second_recordings_continue_numbering_without_overwriting() {
        let directory = tempfile::tempdir().unwrap();
        for name in [
            "2026-10-21-12-34-56-000.mp4",
            "2026-10-21-12-34-56-001.ts",
            "2026-10-21-12-34-56-002.partial.mp4",
        ] {
            std::fs::write(directory.path().join(name), b"existing").unwrap();
        }
        assert_eq!(
            next_segment_index(directory.path(), "2026-10-21-12-34-56").unwrap(),
            3
        );
        assert_eq!(
            generated_segment_prefix(Path::new("2026-10-21-12-34-56-1000.ts")),
            Some("2026-10-21-12-34-56".into())
        );
        assert_eq!(
            completed_ts_segments(directory.path(), "2026-10-21-12-34-56")
                .unwrap()
                .len(),
            1
        );
    }
}

#[derive(Clone)]
pub struct DirectStreamResolver {
    platform: crate::model::Platform,
    stream_url: Url,
}

impl DirectStreamResolver {
    /// Creates a resolver for a stream URL obtained through a user-authorized
    /// platform workflow.
    pub fn new(platform: crate::model::Platform, stream_url: Url) -> Arc<Self> {
        Arc::new(Self {
            platform,
            stream_url,
        })
    }
}

#[async_trait::async_trait]
impl crate::platform::StreamResolver for DirectStreamResolver {
    fn platform(&self) -> crate::model::Platform {
        self.platform
    }
    async fn resolve(
        &self,
        task: &RecordingTask,
    ) -> Result<ResolvedStream, crate::platform::ResolveError> {
        Ok(ResolvedStream {
            stream_url: self.stream_url.clone(),
            title: task.display_name.clone(),
            started_at: chrono::Utc::now(),
        })
    }
}
