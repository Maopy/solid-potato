//! Local, opt-in live transcription. Recent text lives only in the page session.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{Semaphore, watch},
    task::JoinHandle,
};
use url::Url;

const SAMPLES: usize = 16000 * 12;
const LINES: usize = 3;
static INFERENCE: OnceLock<Semaphore> = OnceLock::new();

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    provider: String,
    executable: PathBuf,
    ffmpeg: PathBuf,
    model: PathBuf,
    model_sha256: String,
    language: String,
    threads: usize,
    use_gpu: bool,
}
impl Default for Config {
    fn default() -> Self {
        let executable = std::env::current_exe().unwrap_or_default();
        let dist = executable
            .parent()
            .and_then(Path::parent)
            .unwrap_or(Path::new("dist"));
        Self {
            provider: "whisper_cpp".into(),
            executable: "whisper-cli".into(),
            ffmpeg: "ffmpeg".into(),
            model: dist.join(".live-archive-models/ggml-small.bin"),
            model_sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b".into(),
            language: "zh".into(),
            threads: 4,
            use_gpu: true,
        }
    }
}
impl Config {
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
        for p in [&mut config.ffmpeg, &mut config.executable] {
            if p.is_relative() && p.components().count() > 1 {
                *p = base.join(&*p);
            }
        }
        Ok(config)
    }
    async fn validate(&self) -> Result<()> {
        if self.provider != "whisper_cpp"
            || !(1..=16).contains(&self.threads)
            || !matches!(self.language.as_str(), "zh" | "auto" | "en")
            || self.model_sha256.len() != 64
        {
            bail!("本地语音配置无效")
        }
        let path = self.model.clone();
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
}
impl Default for SpeechSession {
    fn default() -> Self {
        Self {
            view: Arc::new(Mutex::new(View::default())),
            worker: None,
        }
    }
}
impl SpeechSession {
    pub fn status(&self) -> View {
        self.view.lock().unwrap().clone()
    }
    pub fn stop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.abort();
        }
        // Replace the shared state so a cancelled inference cannot repopulate it.
        self.view = Arc::new(Mutex::new(View::default()));
    }
    pub fn start(&mut self, source: &str) -> Result<()> {
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
        self.worker = Some(tokio::spawn(async move {
            let result = async {
                let config = Config::discover()?;
                config.validate().await?;
                run(config, url, state.clone()).await
            }
            .await;
            if let Err(error) = result {
                let mut view = state.lock().unwrap();
                view.enabled = false;
                view.state = "error".into();
                view.error = Some(error.to_string());
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

async fn run(config: Config, source: Url, state: Arc<Mutex<View>>) -> Result<()> {
    let mut decoder = Command::new(&config.ffmpeg);
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
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    let mut child = decoder.spawn().context("无法启动音频解码，请检查 FFmpeg")?;
    let mut audio = child.stdout.take().context("无法读取直播音频")?;
    let (latest, mut chunks) = watch::channel::<Option<Vec<u8>>>(None);
    let _reader = Reader(tokio::spawn(async move {
        loop {
            let mut bytes = vec![0; SAMPLES * 2];
            if audio.read_exact(&mut bytes).await.is_err() {
                break;
            }
            // A slow recognizer only sees the latest chunk; no history queue.
            if latest.send(Some(bytes)).is_err() {
                break;
            }
        }
    }));
    state.lock().unwrap().state = "listening".into();
    loop {
        chunks
            .changed()
            .await
            .context("直播音频已结束或连接中断，请重新开启语音")?;
        let _permit = INFERENCE
            .get_or_init(|| Semaphore::new(1))
            .acquire()
            .await?;
        let Some(pcm) = chunks.borrow_and_update().clone() else {
            continue;
        };
        if is_silent(&pcm) {
            state.lock().unwrap().state = "listening".into();
            continue;
        }
        state.lock().unwrap().state = "transcribing".into();
        let lines = recognize(&config, &pcm).await?;
        let mut view = state.lock().unwrap();
        push_line(&mut view.recent, &lines.concat());
        view.state = "listening".into();
    }
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
    let output = temp.path().join("transcript");
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
    let mut command = Command::new(&config.executable);
    command
        .arg("-m")
        .arg(&config.model)
        .arg("-f")
        .arg(&input)
        .arg("-l")
        .arg(&config.language)
        .arg("-t")
        .arg(config.threads.to_string())
        .args(["-oj", "-of"])
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if !config.use_gpu {
        command.arg("-ng");
    }
    let status = tokio::time::timeout(Duration::from_secs(90), command.status())
        .await
        .context("本地语音识别超时")?
        .context("无法启动本地语音引擎，请检查 whisper-cli")?;
    if !status.success() {
        bail!("本地语音识别失败")
    }
    let bytes = tokio::fs::read(output.with_extension("json"))
        .await
        .context("语音引擎没有返回结果")?;
    if bytes.len() > 1024 * 1024 {
        bail!("语音结果过大")
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(value["transcription"]
        .as_array()
        .context("语音结果格式无效")?
        .iter()
        .filter_map(|v| v["text"].as_str().map(str::to_string))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
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
                "-nostdin", "-v", "error", "-ss", "60", "-i", &input, "-t", "12", "-vn", "-ac",
                "1", "-ar", "16000", "-f", "wav", "pipe:1",
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
        let _ = tokio::time::timeout(
            Duration::from_secs(90),
            run(
                config,
                Url::parse(&format!("http://{address}/audio.wav")).unwrap(),
                state.clone(),
            ),
        )
        .await
        .unwrap();
        server.await.unwrap();
        let recent = state.lock().unwrap().recent.clone();
        assert!(!recent.is_empty(), "expected real transcription");
        assert!(recent.len() <= 3);
        println!(
            "Local live pipeline produced {} recent lines; temporary audio/results removed.",
            recent.len()
        );
    }
}
