#![cfg(unix)]
use live_archive_core::moderation::{
    control::{Controller, LoadedControl},
    recording::{RecordingConfig, RecordingRuntime},
};
use std::{os::unix::fs::PermissionsExt, time::Duration};
#[tokio::test]
async fn shared_proxy_records_both_platforms_preserving_chat_and_task_controls() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    std::fs::write(p.join("dy.yml"),"version: 1\nroom:\n  platform: douyin\n  adapter: douyin.live_page\n  url: https://live.douyin.com/123\n").unwrap();
    std::fs::write(p.join("wx.yml"),"version: 1\nroom:\n  platform: wechat_channels\n  adapter: wechat_channels.desktop_probe\n  url: https://channels.weixin.qq.com/web/pages/live\n").unwrap();
    std::fs::write(p.join("control.yml"),"version: 2\ndirectory: runtime\naccounts:\n  - {id: dy, platform: douyin}\n  - {id: wx, platform: wechat_channels}\ntasks:\n  - {id: dy, account_id: dy, config: dy.yml}\n  - {id: wx, account_id: wx, config: wx.yml}\n").unwrap();
    let config = LoadedControl::load(&p.join("control.yml")).unwrap();
    let mut controller = Controller::bind(&config).await.unwrap();
    let fake = p.join("fake-ffmpeg");
    std::fs::write(
        &fake,
        r#"#!/usr/bin/env python3
import sys,pathlib
args=sys.argv[1:]
output=pathlib.Path(args[-1].replace('%03d','000'))
output.parent.mkdir(parents=True,exist_ok=True)
if '-segment_time' in args:
    source=args[args.index('-i')+1]
    output.write_bytes(b'failed-fixture' if source.endswith('/fail.flv') else b'local-fixture-media')
    sys.stdin.readline()
else:
    source=pathlib.Path(args[args.index('-i')+1])
    if source.read_bytes()==b'failed-fixture':sys.exit(1)
    output.write_bytes(b'local-fixture-media')
"#,
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = p.join("videos");
    let mut recording = RecordingRuntime::start(
        RecordingConfig {
            ffmpeg: fake,
            output_dir: Some(output.clone()),
            ..Default::default()
        },
        &p.join("runtime"),
        controller
            .tasks
            .values()
            .map(|t| t.scope.room_url.clone())
            .collect(),
        &controller.addon().unwrap(),
    )
    .await
    .unwrap();
    let addon = p.join("combined.py");
    std::fs::write(&addon, &recording.source).unwrap();
    let result = tokio::process::Command::new("python3")
        .arg(format!(
            "{}/tests/support/moderation_recording_proxy_driver.py",
            env!("CARGO_MANIFEST_DIR")
        ))
        .arg(&addon)
        .output()
        .await
        .unwrap();
    // Always shut down the worker before reporting assertion failures.
    tokio::time::timeout(Duration::from_secs(15), recording.stop())
        .await
        .unwrap()
        .unwrap();
    controller.stop().await.unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    for platform in ["douyin", "wechat_channels"] {
        let files: Vec<_> = std::fs::read_dir(output.join(platform))
            .unwrap()
            .flat_map(|d| std::fs::read_dir(d.unwrap().path()).unwrap())
            .map(|f| f.unwrap().path())
            .collect();
        assert!(
            files
                .iter()
                .any(|f| f.extension().is_some_and(|e| e == "mp4")),
            "{platform}: {files:?}"
        );
        for file in files
            .iter()
            .filter(|f| f.extension().is_some_and(|e| e == "ts"))
        {
            assert_eq!(
                std::fs::read(file).unwrap(),
                b"failed-fixture",
                "only failed remux input is retained"
            );
        }
    }
}
