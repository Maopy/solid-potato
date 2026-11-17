//! Verify that recording preserves the source video stream without re-encoding.
use chrono::Utc;
use live_archive_core::{
    model::{Platform, RecordingTask, ResolvedStream, TaskStatus},
    recorder::{FfmpegRecorder, RecorderConfig, StreamRecorder},
};
use std::process::Command;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use url::Url;
use uuid::Uuid;

fn video_stream(path: &std::path::Path) -> Vec<u8> {
    let result = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-bsf:v",
            "h264_mp4toannexb",
            "-f",
            "h264",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

// TS remuxing may add access-unit delimiters and repeat codec headers. Compare
// the coded picture slices, whose bytes must not change during stream copy.
fn picture_slices(stream: &[u8]) -> Vec<Vec<u8>> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 < stream.len() {
        let length = if stream[i..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if stream[i..].starts_with(&[0, 0, 1]) {
            3
        } else {
            i += 1;
            continue;
        };
        starts.push((i, i + length));
        i += length;
    }
    let mut slices = Vec::new();
    for (index, &(_, start)) in starts.iter().enumerate() {
        let end = starts.get(index + 1).map_or(stream.len(), |p| p.0);
        if start < end && matches!(stream[start] & 31, 1 | 5) {
            slices.push(stream[start..end].to_vec());
        }
    }
    slices
}

#[tokio::test]
async fn douyin_recording_preserves_video_packets_and_audio() {
    if Command::new("ffmpeg").arg("-version").output().is_err() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.mp4");
    let generated = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=48000",
            "-t",
            "2",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "aac",
            "-movflags",
            "+faststart",
            "-y",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let now = Utc::now();
    let task = RecordingTask {
        id: Uuid::new_v4(),
        platform: Platform::Douyin,
        room_url: Url::parse("https://live.douyin.com/123").unwrap(),
        display_name: "fixture".into(),
        status: TaskStatus::Idle,
        output_dir: dir.path().join("recordings"),
        created_at: now,
        updated_at: now,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let data = std::fs::read(&source).unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request).await.unwrap();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", data.len()).as_bytes()).await.unwrap();
        socket.write_all(&data).await.unwrap();
    });
    let stream = ResolvedStream {
        stream_url: Url::parse(&format!("http://{address}/source.mp4")).unwrap(),
        title: "fixture".into(),
        started_at: now,
    };
    let recorder = FfmpegRecorder::new(RecorderConfig {
        ffmpeg_path: "ffmpeg".into(),
        segment_seconds: 60,
    });
    let (_sender, stop) = watch::channel(false);
    let output = recorder.record(&task, &stream, stop).await.unwrap();
    server.await.unwrap();
    let original = picture_slices(&video_stream(&source));
    let recorded = picture_slices(&video_stream(&output));
    assert!(!original.is_empty());
    assert!(
        original == recorded,
        "encoded picture slices must survive recording unchanged"
    );
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(&output)
        .output()
        .unwrap();
    assert!(probe.status.success());
    assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), "aac");
}
