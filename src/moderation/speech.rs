//! Local, opt-in live transcription with recent page text and bounded room history.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Semaphore, mpsc},
    task::JoinHandle,
};
use url::Url;

#[path = "speech_segments.rs"]
mod segments;
use segments::{FRAME_BYTES, FRAME_MS, Frame, Segmenter, Utterance};

const LINES: usize = 3;
static INFERENCE: OnceLock<Semaphore> = OnceLock::new();

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    provider: String,
    runner: PathBuf,
    #[serde(rename = "source")]
    _source: PathBuf,
    executable: PathBuf,
    ffmpeg: PathBuf,
    model: PathBuf,
    model_sha256: String,
    language: String,
    threads: usize,
    use_gpu: bool,
    vad_model: PathBuf,
    punc_model: PathBuf,
    hotwords_file: PathBuf,
    sentence_pause_ms: usize,
    max_sentence_ms: usize,
    speaker_model: PathBuf,
    speaker_references: BTreeMap<String, PathBuf>,
    #[serde(skip)]
    active_speaker_reference: PathBuf,
}
impl Default for Config {
    fn default() -> Self {
        let executable = std::env::current_exe().unwrap_or_default();
        let dist = executable
            .parent()
            .and_then(Path::parent)
            .unwrap_or(Path::new("dist"));
        Self {
            provider: "funasr_paraformer".into(),
            runner: PathBuf::new(),
            _source: PathBuf::new(),
            executable: "python".into(),
            ffmpeg: "ffmpeg".into(),
            model: dist.join(".live-archive-models/FunASR-seaco-paraformer"),
            model_sha256: "3d491689244ec5dfbf9170ef3827c358aa10f1f20e42a7c59e15e688647946d1".into(),
            language: "zh".into(),
            threads: 4,
            use_gpu: true,
            vad_model: PathBuf::new(),
            punc_model: PathBuf::new(),
            hotwords_file: PathBuf::from("config/asr-hotwords.txt"),
            sentence_pause_ms: 250,
            max_sentence_ms: 60000,
            speaker_model: PathBuf::new(),
            speaker_references: BTreeMap::new(),
            active_speaker_reference: PathBuf::new(),
        }
    }
}
impl Config {
    fn select_speaker(&mut self, room: Option<&str>) {
        self.active_speaker_reference = room
            .and_then(|key| self.speaker_references.get(key))
            .cloned()
            .unwrap_or_default();
    }
    fn discover() -> Result<Self> {
        let executable = std::env::current_exe()?;
        let path = if let Some(path) = std::env::var_os("LIVE_ARCHIVE_ASR_CONFIG") {
            Some(PathBuf::from(path))
        } else {
            executable
                .ancestors()
                .skip(1)
                .take(4)
                .map(|p| p.join("asr.dev.json"))
                .find(|p| p.is_file())
        };
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let mut config: Self =
            serde_json::from_slice(&std::fs::read(&path).context("无法读取本地语音配置")?)
                .context("本地语音配置格式错误")?;
        let base = path.parent().context("语音配置路径无效")?;
        if config.model.is_relative() {
            config.model = base.join(&config.model);
        }
        if !config.vad_model.as_os_str().is_empty() && config.vad_model.is_relative() {
            config.vad_model = base.join(&config.vad_model);
        }
        if !config.punc_model.as_os_str().is_empty() && config.punc_model.is_relative() {
            config.punc_model = base.join(&config.punc_model);
        }
        if !config.hotwords_file.as_os_str().is_empty() && config.hotwords_file.is_relative() {
            config.hotwords_file = base.join(&config.hotwords_file);
        }
        if !config.speaker_model.as_os_str().is_empty() && config.speaker_model.is_relative() {
            config.speaker_model = base.join(&config.speaker_model);
        }
        for reference in config.speaker_references.values_mut() {
            if reference.is_relative() {
                *reference = base.join(&*reference);
            }
        }
        for p in [
            &mut config.ffmpeg,
            &mut config.executable,
            &mut config.runner,
        ] {
            if p.is_relative() && p.components().count() > 1 {
                *p = base.join(&*p);
            }
        }
        Ok(config)
    }
    fn vad_path(&self) -> PathBuf {
        if !self.vad_model.as_os_str().is_empty() {
            return self.vad_model.clone();
        }
        self.model
            .parent()
            .unwrap_or(Path::new("."))
            .join("FunASR-fsmn-vad")
    }
    fn punc_path(&self) -> PathBuf {
        if !self.punc_model.as_os_str().is_empty() {
            return self.punc_model.clone();
        }
        self.model
            .parent()
            .unwrap_or(Path::new("."))
            .join("FunASR-ct-punc")
    }
    async fn validate(&self) -> Result<()> {
        if self.provider != "funasr_paraformer"
            || !(1..=16).contains(&self.threads)
            || !matches!(self.language.as_str(), "zh" | "auto" | "en")
            || self.model_sha256.len() != 64
            || !(200..=2000).contains(&self.sentence_pause_ms)
            || !(2000..=120000).contains(&self.max_sentence_ms)
            || self.sentence_pause_ms >= self.max_sentence_ms
        {
            bail!("本地语音配置无效：仅支持 FunASR")
        }
        if !self.runner.is_file()
            || !self.model.join("config.yaml").is_file()
            || !self.model.join("tokens.json").is_file()
        {
            bail!("FunASR 本地引擎或模型文件未安装")
        }
        if [self.vad_path(), self.punc_path()]
            .iter()
            .any(|p| !p.join("config.yaml").is_file() || !p.join("model.pt").is_file())
        {
            bail!("FunASR FSMN-VAD 或 CT-Transformer 模型未安装")
        }
        let path = self.model.join("model.pt");
        let expected = self.model_sha256.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            use std::io::Read;
            let mut file = std::fs::File::open(path).context("本地语音模型未安装")?;
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                digest.update(&buffer[..n]);
            }
            if format!("{:x}", digest.finalize()) != expected {
                bail!("本地语音模型校验失败")
            }
            Ok(())
        })
        .await??;
        Ok(())
    }
}
#[derive(Clone, Serialize)]
pub struct View {
    pub enabled: bool,
    pub state: String,
    pub recent: VecDeque<String>,
    pub error: Option<String>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            enabled: false,
            state: "off".into(),
            recent: VecDeque::new(),
            error: None,
        }
    }
}
pub struct SpeechSession {
    view: Arc<Mutex<View>>,
    worker: Option<JoinHandle<()>>,
    timeline: super::timeline::Timeline,
    archive: Option<SpeechArchive>,
}

#[derive(Clone)]
pub struct SpeechArchive {
    path: std::path::PathBuf,
    run_id: String,
    connection_id: String,
}
impl SpeechArchive {
    pub fn new(path: std::path::PathBuf, run_id: String, connection_id: String) -> Self {
        Self { path, run_id, connection_id }
    }
    fn write(&self, text: &str, start: i64, end: i64) -> Result<()> {
        let mut db = rusqlite::Connection::open(&self.path).context("打开语音转写日志失败")?;
        db.busy_timeout(Duration::from_secs(5))?;
        let tx = db.transaction()?;
        for sentence in sentences(text) {
            tx.execute("INSERT INTO speech_transcripts(run_id,connection_id,start_ms,end_ms,text) VALUES(?1,?2,?3,?4,?5)",
                rusqlite::params![self.run_id, self.connection_id, start, end, sentence])?;
        }
        tx.commit().context("保存语音转写失败")?;
        Ok(())
    }
}
impl Default for SpeechSession {
    fn default() -> Self {
        Self {
            view: Arc::new(Mutex::new(View::default())),
            worker: None,
            timeline: Default::default(),
            archive: None,
        }
    }
}
impl SpeechSession {
    pub fn with_timeline(timeline: super::timeline::Timeline) -> Self {
        Self {
            timeline,
            view: Arc::new(Mutex::new(View::default())),
            worker: None,
            archive: None,
        }
    }
    pub fn with_archive(mut self, archive: SpeechArchive) -> Self {
        self.archive = Some(archive);
        self
    }
    pub fn status(&self) -> View {
        self.view.lock().unwrap().clone()
    }
    pub fn stop(&mut self) {
        if self.status().enabled {
            self.timeline.note("语音识别已关闭");
        }
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
        // Replace the shared state so a cancelled inference cannot repopulate it.
        self.view = Arc::new(Mutex::new(View::default()));
    }
    pub fn start(&mut self, source: &str) -> Result<()> {
        self.start_for_room(source, None)
    }
    pub fn start_for_room(&mut self, source: &str, room: Option<&str>) -> Result<()> {
        let url = Url::parse(source).context("直播音频地址无效")?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || source.len() > 8192
        {
            bail!("直播音频地址必须为 HTTP(S)");
        }
        if self.status().enabled {
            return Ok(());
        }
        self.stop();
        {
            let mut state = self.view.lock().unwrap();
            state.enabled = true;
            state.state = "starting".into();
        }
        let state = self.view.clone();
        let timeline = self.timeline.clone();
        let archive = self.archive.clone();
        let room = room.map(str::to_owned);
        timeline.note("语音识别已开启");
        self.worker = Some(tokio::spawn(async move {
            let result = async {
                let mut config = Config::discover()?;
                config.select_speaker(room.as_deref());
                config.validate().await?;
                run(config, url, state.clone(), timeline.clone(), archive).await
            }
            .await;
            if let Err(error) = result {
                let mut view = state.lock().unwrap();
                view.enabled = false;
                view.state = "error".into();
                view.error = Some(error.to_string());
                timeline.note(&format!("语音识别异常：{error}"));
            }
        }));
        Ok(())
    }
}
impl Drop for SpeechSession {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
    }
}
struct Reader(JoinHandle<()>);
impl Drop for Reader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn run(
    config: Config,
    source: Url,
    state: Arc<Mutex<View>>,
    timeline: super::timeline::Timeline,
    archive: Option<SpeechArchive>,
) -> Result<()> {
    let mut last = String::new();
    for attempt in 0..4 {
        if attempt > 0 {
            state.lock().unwrap().state = "starting".into();
            tokio::time::sleep(Duration::from_secs(attempt)).await;
        }
        last = run_stream(&config, &source, &state, &timeline, archive.as_ref()).await?;
    }
    bail!("{last}")
}

/// Decoder EOF is retryable; model and classification errors remain fatal.
async fn run_stream(
    config: &Config,
    source: &Url,
    state: &Arc<Mutex<View>>,
    timeline: &super::timeline::Timeline,
    archive: Option<&SpeechArchive>,
) -> Result<String> {
    let mut decoder = Command::new(&config.ffmpeg);
    #[cfg(windows)]
    decoder.creation_flags(0x08000000);
    decoder
        .args([
            "-nostdin",
            "-v",
            "error",
            "-rw_timeout",
            "15000000",
            "-protocol_whitelist",
            "http,https,tcp,tls,crypto",
            "-i",
        ])
        .arg(source.as_str())
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    let mut child = decoder.spawn().context("无法启动音频解码，请检查 FFmpeg")?;
    let mut stderr = child.stderr.take().context("无法读取音频解码状态")?;
    let diagnostics = tokio::spawn(async move {
        let mut tail = VecDeque::new();
        let mut buf = [0u8; 1024];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
            for byte in &buf[..n] {
                if tail.len() == 8192 {
                    tail.pop_front();
                }
                tail.push_back(*byte);
            }
        }
        tail.into_iter().collect::<Vec<_>>()
    });
    let mut audio = child.stdout.take().context("无法读取直播音频")?;
    let (send, mut chunks) = mpsc::channel::<Result<Utterance>>(16);
    let mut detector = config.clone();
    detector.model = config.vad_path();
    detector.use_gpu = false;
    detector.threads = detector.threads.min(2);
    let mut vad = Some(FunAsrWorker::spawn_mode(&detector, true).await?);
    let pause_ms = config.sentence_pause_ms;
    let max_ms = config.max_sentence_ms.min(8000);
    let _reader = Reader(tokio::spawn(async move {
        if let Err(error) = capture_sentences(&mut audio, &mut vad, &send, pause_ms, max_ms).await {
            let _ = send.send(Err(error)).await;
        }
    }));
    state.lock().unwrap().state = "listening".into();
    while let Some(chunk) = chunks.recv().await {
        let chunk = chunk?;
        let _permit = INFERENCE
            .get_or_init(|| Semaphore::new(1))
            .acquire()
            .await?;
        if is_silent(&chunk.pcm) {
            continue;
        }
        state.lock().unwrap().state = "transcribing".into();
        let lines = recognize(config, &chunk.pcm).await?;
        if let Some(archive) = archive {
            archive.write(&lines.concat(), chunk.start_ms, chunk.end_ms)?;
        }
        let mut view = state.lock().unwrap();
        push_line(&mut view.recent, &lines.concat());
        publish_sentences(&timeline, &lines.concat(), chunk.start_ms, chunk.end_ms);
        view.state = "listening".into();
    }
    let status = child.wait().await.context("等待音频解码结束失败")?;
    let diagnostic = diagnostics.await.unwrap_or_default();
    let diagnostic = String::from_utf8_lossy(&diagnostic).to_lowercase();
    let reason = if diagnostic.contains("403") || diagnostic.contains("forbidden") {
        "音频地址已失效或被拒绝，请刷新直播页后重新开启语音"
    } else if diagnostic.contains("404") || diagnostic.contains("not found") {
        "直播音频地址不可用，请刷新直播页后重新开启语音"
    } else if diagnostic.contains("timed out") || diagnostic.contains("timeout") {
        "直播音频连接超时，请检查网络后重新开启语音"
    } else if !status.success() {
        "直播音频解码失败，请刷新直播页后重新开启语音"
    } else {
        "直播音频已结束或连接中断，请刷新直播页后重新开启语音"
    };
    Ok(reason.into())
}

async fn classify_frames(vad: &mut FunAsrWorker, pcm: &[u8], finish: bool) -> Result<Vec<bool>> {
    use std::fmt::Write;
    let mut hex = String::with_capacity(pcm.len() * 2);
    for byte in pcm {
        write!(&mut hex, "{byte:02x}").unwrap();
    }
    let request = serde_json::to_vec(&serde_json::json!({"pcm_hex":hex,"finish":finish}))?;
    vad.input.write_all(&request).await?;
    vad.input.write_all(b"\n").await?;
    vad.input.flush().await?;
    let response = tokio::time::timeout(Duration::from_secs(30), vad.response())
        .await
        .context("语音活动检测超时")??;
    response["speech"]
        .as_array()
        .context("语音活动检测结果无效")?
        .iter()
        .map(|value| value.as_bool().context("语音活动检测判断格式无效"))
        .collect()
}

async fn capture_sentences(
    audio: &mut tokio::process::ChildStdout,
    vad: &mut Option<FunAsrWorker>,
    send: &mpsc::Sender<Result<Utterance>>,
    pause_ms: usize,
    max_ms: usize,
) -> Result<()> {
    let mut segmenter = Segmenter::new(pause_ms, max_ms);
    let mut pending = VecDeque::new();
    let mut anchor = None;
    let mut frame_count = 0i64;
    loop {
        // Transport packets are short; sentence boundaries never depend on them.
        let mut pcm = vec![0; FRAME_BYTES * 50];
        let mut filled = 0;
        while filled < pcm.len() {
            match audio.read(&mut pcm[filled..]).await? {
                0 => break,
                n => filled += n,
            }
        }
        let finish = filled < pcm.len();
        pcm.truncate(filled.div_ceil(FRAME_BYTES) * FRAME_BYTES);
        if pcm.len() > filled {
            pcm[filled..].fill(0);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let start = *anchor.get_or_insert(now - (pcm.len() / FRAME_BYTES * FRAME_MS) as i64);
        for bytes in pcm.chunks_exact(FRAME_BYTES) {
            frame_count += 1;
            pending.push_back(Frame {
                pcm: bytes.to_vec(),
                end_ms: (start + frame_count * FRAME_MS as i64).min(now),
            });
        }
        let decisions = if let Some(vad) = vad {
            classify_frames(vad, &pcm, finish).await?
        } else {
            pending.iter().map(|frame| !is_silent(&frame.pcm)).collect()
        };
        if decisions.len() > pending.len() {
            bail!("语音活动检测音频帧数不匹配");
        }
        for voice in decisions {
            if let Some(sentence) = segmenter.push(pending.pop_front().unwrap(), voice) {
                send.send(Ok(sentence)).await.context("语音识别已停止")?;
            }
        }
        if pending.len() > 500 {
            bail!("语音活动检测分段积压过多");
        }
        if finish {
            if !pending.is_empty() {
                bail!("语音活动检测未处理完末尾音频");
            }
            if let Some(sentence) = segmenter.finish() {
                send.send(Ok(sentence)).await.context("语音识别已停止")?;
            }
            return Ok(());
        }
    }
}

// Sentence timestamps remain the source audio segment's bounds: the punctuation
// model does not return word-level timing, so do not invent precise boundaries.
fn sentences(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive(['。', '！', '？', '!', '?', '\n']).map(str::trim).filter(|s| !s.is_empty())
}
fn publish_sentences(timeline: &super::timeline::Timeline, text: &str, start: i64, end: i64) {
    for sentence in sentences(text) { timeline.speech(sentence, start, end); }
}

fn is_silent(pcm: &[u8]) -> bool {
    let energy: f64 = pcm
        .chunks_exact(2)
        .map(|s| {
            let x = i16::from_le_bytes([s[0], s[1]]) as f64 / 32768.;
            x * x
        })
        .sum();
    energy / ((pcm.len() / 2).max(1) as f64) < 0.000009
}
fn push_line(recent: &mut VecDeque<String>, text: &str) {
    for part in text.split_inclusive(['。', '！', '？', '!', '?', '\n']) {
        let line = part
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(300)
            .collect::<String>();
        if line.is_empty() || recent.back() == Some(&line) {
            continue;
        }
        recent.push_back(line);
        while recent.len() > LINES {
            recent.pop_front();
        }
    }
}
async fn recognize(config: &Config, pcm: &[u8]) -> Result<Vec<String>> {
    let temp = tempfile::tempdir().context("无法创建临时音频目录")?;
    let input = temp.path().join("audio.wav");
    let mut wav = tokio::fs::File::create(&input).await?;
    let size = pcm.len() as u32;
    let mut header = Vec::new();
    header.extend(b"RIFF");
    header.extend((36 + size).to_le_bytes());
    header.extend(b"WAVEfmt ");
    header.extend(16u32.to_le_bytes());
    header.extend(1u16.to_le_bytes());
    header.extend(1u16.to_le_bytes());
    header.extend(16000u32.to_le_bytes());
    header.extend(32000u32.to_le_bytes());
    header.extend(2u16.to_le_bytes());
    header.extend(16u16.to_le_bytes());
    header.extend(b"data");
    header.extend(size.to_le_bytes());
    wav.write_all(&header).await?;
    wav.write_all(pcm).await?;
    drop(wav);
    funasr_recognize(config, &input).await
}

// One resident model per process; the existing inference semaphore serializes rooms.
static FUNASR: OnceLock<tokio::sync::Mutex<Option<(String, FunAsrWorker)>>> = OnceLock::new();
struct FunAsrWorker {
    child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
}
impl Drop for FunAsrWorker {
    fn drop(&mut self) {
        crate::child_process::drop_proxy_child(&mut self.child);
    }
}
impl FunAsrWorker {
    async fn response(&mut self) -> Result<serde_json::Value> {
        let mut line = Vec::new();
        // Bound stdout even if the interpreter or dependency returns malformed data.
        (&mut self.output)
            .take(1024 * 1024)
            .read_until(b'\n', &mut line)
            .await?;
        if line.last() != Some(&b'\n') {
            bail!("语音引擎返回数据不完整")
        }
        let value: serde_json::Value = serde_json::from_slice(&line)?;
        if value.get("error").is_some() {
            bail!("语音引擎失败: {}", value["error"])
        }
        Ok(value)
    }
    async fn spawn(config: &Config) -> Result<Self> {
        Self::spawn_mode(config, false).await
    }
    async fn spawn_mode(config: &Config, vad: bool) -> Result<Self> {
        let mut command = Command::new(&config.executable);
        command
            .arg("-u")
            .arg(&config.runner)
            .arg("--model")
            .arg(&config.model)
            .arg("--threads")
            .arg(config.threads.to_string())
            .env("PYTHONUTF8", "1")
            .env("HF_HUB_OFFLINE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if vad {
            command.arg("--vad");
        }
        if !vad {
            command.arg("--punc-model").arg(config.punc_path());
            if !config.hotwords_file.as_os_str().is_empty() {
                command.arg("--hotwords-file").arg(&config.hotwords_file);
            }
        }
        if config.use_gpu {
            command.arg("--gpu");
        }
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command.spawn().context("无法启动本地语音引擎")?;
        let input = child.stdin.take().context("FunASR stdin missing")?;
        let output = BufReader::new(child.stdout.take().context("FunASR stdout missing")?);
        let mut worker = Self {
            child,
            input,
            output,
        };
        let ready = tokio::time::timeout(Duration::from_secs(180), worker.response())
            .await
            .context("语音引擎加载模型超时")??;
        if ready["ready"] != true {
            bail!("语音引擎启动失败")
        }
        Ok(worker)
    }
}
async fn funasr_recognize(config: &Config, wav: &Path) -> Result<Vec<String>> {
    let fingerprint = format!(
        "{:?}",
        (
            &config.executable,
            &config.runner,
            &config.model,
            &config.model_sha256,
            &config.punc_model,
            &config.hotwords_file,
            &config.speaker_model,
            &config.active_speaker_reference,
            config.use_gpu,
            config.threads
        )
    );
    let mut cached = FUNASR
        .get_or_init(|| tokio::sync::Mutex::new(None))
        .lock()
        .await;
    // Own the worker across awaits: cancellation kills it rather than caching a stale reply.
    let mut worker = match cached.take() {
        Some((key, worker)) if key == fingerprint => worker,
        _ => FunAsrWorker::spawn(config).await?,
    };
    let request = serde_json::to_vec(&serde_json::json!({"wav": wav}))?;
    worker.input.write_all(&request).await?;
    worker.input.write_all(b"\n").await?;
    worker.input.flush().await?;
    let value = tokio::time::timeout(Duration::from_secs(90), worker.response())
        .await
        .context("本地语音识别超时")??;
    let text = value["text"]
        .as_str()
        .context("语音结果格式无效")?
        .to_owned();
    *cached = Some((fingerprint, worker));
    Ok(vec![text])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speaker_filter_only_applies_to_configured_room() {
        let mut config = Config::default();
        config
            .speaker_references
            .insert("room-a".into(), "reference.json".into());
        config.select_speaker(Some("room-a"));
        assert_eq!(
            config.active_speaker_reference,
            PathBuf::from("reference.json")
        );
        config.select_speaker(Some("room-b"));
        assert!(config.active_speaker_reference.as_os_str().is_empty());
        config.select_speaker(None);
        assert!(config.active_speaker_reference.as_os_str().is_empty());
    }
    #[tokio::test]
    async fn decoder_retries_failures_and_redacts_stream_address() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let Ok(ffmpeg) = std::env::var("LIVE_ARCHIVE_TEST_FFMPEG") else {
            return;
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = [0u8; 4096];
                let _ = socket.read(&mut data).await;
                socket
                    .write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
            }
        });
        let config = Config {
            ffmpeg: ffmpeg.into(),
            provider: "funasr_paraformer".into(),
            ..Default::default()
        };
        let state = Arc::new(Mutex::new(View::default()));
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            run(
                config,
                Url::parse(&format!("http://{address}/audio?private-token=secret")).unwrap(),
                state,
                Default::default(),
                None,
            ),
        )
        .await
        .unwrap();
        let message = result.unwrap_err().to_string();
        assert!(message.contains("音频地址已失效或被拒绝"), "{message}");
        assert!(!message.contains("secret"));
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn funasr_reuses_the_model_and_discards_a_failed_transport() {
        let temp = tempfile::tempdir().unwrap();
        let runner = temp.path().join("worker.py");
        std::fs::write(
            &runner,
            r#"
import json, sys
print('{"ready":true}', flush=True)
count = 0
for line in sys.stdin:
    request = json.loads(line)
    if request['wav'] == 'fail':
        print('{"error":"test failure"}', flush=True)
    else:
        count += 1
        print(json.dumps({'text':str(count)}), flush=True)
"#,
        )
        .unwrap();
        let config = Config {
            provider: "funasr_paraformer".into(),
            executable: "python".into(),
            runner,
            use_gpu: false,
            ..Default::default()
        };
        assert_eq!(
            funasr_recognize(&config, Path::new("one")).await.unwrap(),
            ["1"]
        );
        assert_eq!(
            funasr_recognize(&config, Path::new("two")).await.unwrap(),
            ["2"]
        );
        assert!(funasr_recognize(&config, Path::new("fail")).await.is_err());
        assert_eq!(
            funasr_recognize(&config, Path::new("new")).await.unwrap(),
            ["1"]
        );
        FUNASR.get().unwrap().lock().await.take();
    }
    #[test]
    fn recognized_sentences_are_separate_timeline_entries() {
        let timeline = crate::moderation::timeline::Timeline::default();
        let now = chrono::Utc::now().timestamp_millis();
        publish_sentences(&timeline, "第一句。第二句！", now - 1000, now);
        let entries = timeline.entries();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            vec!["第一句。", "第二句！"]
        );
        assert!(
            entries
                .iter()
                .all(|entry| entry.at_ms == now - 1000 && entry.end_ms == now)
        );
    }

    #[test]
    fn transcripts_survive_reopen_and_keep_room_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.sqlite3");
        let journal = super::super::journal::ProbeJournal::open(&path).unwrap();
        let archive = SpeechArchive::new(path.clone(), "run".into(), "room-a".into());
        archive.write("第一句。第二句！", 100, 200).unwrap();
        drop(journal);
        let db = rusqlite::Connection::open(path).unwrap();
        let mut stmt = db.prepare("SELECT run_id,connection_id,start_ms,end_ms,text FROM speech_transcripts ORDER BY id").unwrap();
        let rows = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,i64>(2)?, r.get::<_,i64>(3)?, r.get::<_,String>(4)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        assert_eq!(rows, vec![("run".into(), "room-a".into(), 100, 200, "第一句。".into()), ("run".into(), "room-a".into(), 100, 200, "第二句！".into())]);
    }

    #[test]
    fn recent_text_is_bounded_and_clear_replaces_the_entire_session() {
        let mut recent = VecDeque::new();
        push_line(
            &mut recent,
            "第一句。第二句！第三句？第四句。第五句。第六句。第七句。",
        );
        assert_eq!(recent.len(), 3);
        assert_eq!(recent.front().unwrap(), "第五句。");
        push_line(&mut recent, "第七句。");
        assert_eq!(recent.len(), 3);
        let mut session = SpeechSession::default();
        let old = session.view.clone();
        old.lock().unwrap().recent = recent;
        session.stop();
        push_line(&mut old.lock().unwrap().recent, "晚到的识别。");
        assert!(session.status().recent.is_empty());
        assert!(is_silent(&vec![0; 32000]));
        assert!(!is_silent(&[255, 127].repeat(16000)));
        for url in [
            "file:///etc/passwd",
            "ftp://example.test/live",
            "https://a:b@example.test/live",
        ] {
            assert!(session.start(url).is_err());
        }
    }
    #[test]
    fn stopping_speech_clears_page_text_but_keeps_room_history() {
        let timeline = super::super::timeline::Timeline::default();
        let mut session = SpeechSession::with_timeline(timeline.clone());
        session.view.lock().unwrap().enabled = true;
        let now = chrono::Utc::now().timestamp_millis();
        timeline.speech("已经识别的文字。", now - 12_000, now);
        session.stop();
        assert!(!session.status().enabled);
        assert!(session.status().recent.is_empty());
        assert_eq!(timeline.entries().len(), 2);
        assert_eq!(timeline.entries()[0].text, "已经识别的文字。");
        assert_eq!(timeline.entries()[1].text, "语音识别已关闭");
    }
    #[tokio::test]
    #[ignore = "requires installed local model and LIVE_ARCHIVE_ASR_SAMPLE video"]
    async fn real_audio_to_local_model_without_saving_history() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let config = Config::discover().unwrap();
        config.validate().await.unwrap();
        let input = std::env::var("LIVE_ARCHIVE_ASR_SAMPLE").unwrap();
        let wav = Command::new(&config.ffmpeg)
            .args([
                "-nostdin", "-v", "error", "-i", &input, "-t", "12", "-vn", "-ac", "1", "-ar",
                "16000", "-f", "wav", "pipe:1",
            ])
            .output()
            .await
            .unwrap();
        assert!(wav.status.success());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                wav.stdout.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(&wav.stdout).await.unwrap();
        });
        let state = Arc::new(Mutex::new(View::default()));
        let source = Url::parse(&format!("http://{address}/audio.wav")).unwrap();
        let timeline = super::super::timeline::Timeline::default();
        let _ = tokio::time::timeout(
            Duration::from_secs(90),
            run_stream(&config, &source, &state, &timeline, None),
        )
        .await
        .unwrap();
        server.await.unwrap();
        if let Some(cache) = FUNASR.get() {
            cache.lock().await.take();
        }
        let recent = state.lock().unwrap().recent.clone();
        assert!(!recent.is_empty(), "expected real transcription");
        assert!(recent.len() <= 3);
        println!(
            "Local live pipeline produced {} recent lines; temporary audio/results removed.",
            recent.len()
        );
    }
}
