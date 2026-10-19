//! Run explicitly with an installed FFmpeg: cargo test --test ffmpeg_stop -- --ignored
#![cfg(unix)]

use chrono::Utc;
use live_archive_core::{
    model::{Platform, RecordingTask, ResolvedStream, TaskStatus},
    recorder::{FfmpegRecorder, RecorderConfig, StreamRecorder},
};
use std::{os::unix::fs::PermissionsExt, time::Duration};
use tokio::{
    process::Command,
    sync::watch,
    time::{sleep, timeout},
};
use url::Url;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires FFmpeg with libx264 and AAC encoders"]
async fn stopping_real_ffmpeg_preserves_decodable_tail() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.ts");
    let generated = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=25",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:sample_rate=48000",
            "-t",
            "20",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "25",
            "-c:a",
            "aac",
        ])
        .arg(&source)
        .output()
        .await
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    // Pace the file like a live input so cancellation occurs mid-segment.
    // Remux subprocesses run at their normal speed.
    let wrapper = directory.path().join("ffmpeg-paced");
    std::fs::write(&wrapper, "#!/bin/sh\ncase \" $* \" in\n  *' segment '*) exec ffmpeg -re \"$@\" ;;\n  *) exec ffmpeg \"$@\" ;;\nesac\n").unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output_dir = directory.path().join("recordings");
    let now = Utc::now();
    let task = RecordingTask {
        id: Uuid::new_v4(),
        platform: Platform::Douyin,
        room_url: Url::parse("https://live.douyin.com/test").unwrap(),
        display_name: "stop-test".into(),
        status: TaskStatus::Idle,
        output_dir: output_dir.clone(),
        created_at: now,
        updated_at: now,
    };
    let stream = ResolvedStream {
        stream_url: Url::from_file_path(&source).unwrap(),
        title: "test".into(),
        started_at: now,
    };
    let recorder = FfmpegRecorder::new(RecorderConfig {
        ffmpeg_path: wrapper,
        segment_seconds: 2,
    });
    let (sender, stop) = watch::channel(false);
    let mut recording = tokio::spawn(async move { recorder.record(&task, &stream, stop).await });
    timeout(Duration::from_secs(15), async {
        loop {
            assert!(
                !recording.is_finished(),
                "recorder exited before cancellation"
            );
            if let Ok(entries) = std::fs::read_dir(&output_dir) {
                let written = entries
                    .flatten()
                    .filter(|entry| {
                        entry.path().extension().is_some_and(|ext| ext == "ts")
                            && entry
                                .metadata()
                                .is_ok_and(|metadata| metadata.len() > 32_000)
                    })
                    .count();
                if written >= 2 {
                    break;
                }
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    sender.send(true).unwrap();
    let result = timeout(Duration::from_secs(15), &mut recording)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(result.exists());
    let mut files = std::fs::read_dir(&output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    files.sort();
    assert!(files.len() >= 2);
    for file in files {
        assert_eq!(
            file.extension().unwrap(),
            "mp4",
            "unfinalized segment: {file:?}"
        );
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-xerror", "-i"])
            .arg(&file)
            .args(["-f", "null", "-"])
            .output()
            .await
            .unwrap();
        assert!(
            decoded.status.success() && decoded.stderr.is_empty(),
            "{file:?}: {}",
            String::from_utf8_lossy(&decoded.stderr)
        );
    }
}
