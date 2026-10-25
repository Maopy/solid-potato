# Live Archive Core

Rust recording core for a GPUI-based desktop application. It targets Windows, macOS and Linux; the UI can be supplied by a Windows-capable GPUI distribution later without coupling it to the recorder.

## Current scope

- SQLite-backed recording tasks and status transitions.
- A platform resolver boundary for Douyin and WeChat Channels.
- FFmpeg process recording, `.ts` segment output, cancellation and safe file names.
- Unattended monitoring with offline polling, bounded exponential retry, and
  cooperative cancellation.
- User-editable TOML configuration for rooms and authorized sessions.

The shipped Douyin/WeChat resolvers intentionally report `NotConfigured`: each integration must obtain media URLs through an account/session the user is authorized to use, and must not circumvent DRM or access controls.

## Run

Windows 开发、ZIP 打包和双击启动步骤见 [Windows 使用说明](docs/windows.md)。

```sh
# Create the user configuration template, edit it, then import it.
cargo run -- config init
# The command prints the platform-appropriate default path.
cargo run -- config apply
# Or keep a project-local file:
cargo run -- --config ./live-archive.toml config init
cargo run -- --config ./live-archive.toml config apply

# The older one-off commands remain available.
cargo run -- add douyin https://live.douyin.com/example --name creator
cargo run -- list
cargo run -- doctor
cargo run -- history <task-id>
# Save an authorized session without putting it in shell history or SQLite.
printf '%s' 'your-cookie-or-session' | cargo run -- auth set douyin
cargo run -- auth status douyin
# Resolve the saved-session Douyin room and record it.
cargo run -- record-douyin <task-id>
# Record a direct stream URL that you obtained through an authorized workflow.
cargo run -- record <task-id> 'https://example.com/authorized-live.m3u8'
# Keep monitoring after a recording ends (for an authorized test stream).
cargo run -- watch <task-id> 'https://example.com/authorized-live.m3u8' --offline-poll-seconds 30
# Capture every WeChat Channels room that you open. The command creates local
# recording tasks automatically from the page-observed anchor identity.
cargo run -- wechat-capture --consent
```

When no `--config` is supplied, a `live-archive.toml` in the current directory
is selected first; otherwise the platform-specific user configuration location
is used.

`config apply` creates or updates rooms matched by platform + room URL and
imports each configured `auth.session` into the operating-system credential
store. It never stores sessions in SQLite and does not delete omitted rooms.
The configuration itself contains secrets, so do not commit it or share it.

For a WeChat Channels room no configuration is needed during testing. Start
`wechat-capture --consent` and open every desired live room. It keeps a
discovery list in the terminal. Click the page's "录制直播" button to start
recording immediately; use `list` to show tasks, `stop <number>` or `stop all`
to stop them, and `quit` to restore the original system proxy. The proxy only
intercepts page/script traffic; live CDN media is transparently forwarded.

## 统一场控入口

Windows 发布包双击 `启动场控.cmd`；macOS 双击 `启动场控.command` 或执行 `./run.sh`，同时启动抖音和视频号并打开统一控制页，支持场控开关、录制与共享下载任务。实际房间配置沿用原自动配置；详见 [统一入口说明](docs/moderation-unified.md)。

## 场控内录制

场控已集成抖音和视频号录制，继续使用原场控启动脚本，在直播页点击“录制直播”，通过下载任务列表暂停、继续或移除；无需另启录制代理。旧配置可继续使用，打包时继承上一版本 config。详见 [录制说明](docs/moderation-recording.md)。

## Architecture and license

The first smart moderation increment provides a read-only Douyin injection
probe. It pairs a page with a local Rust bridge, observes DOM comments and
network metadata, and records bounded samples in a separate SQLite journal.
By default it does not decode gift/level events. It cannot send chat. UI development remains
paused. See [M0 setup and verification](docs/moderation-m0.md) for the actual
command, proxy setup, limitations, and tests.

M1 adds a platform-neutral offline rules engine and deterministic JSONL replay
for gift batching, welcomes, and level upgrades. It produces preview action
logs without attaching to a live page or executing chat. See
[M1 replay and verification](docs/moderation-m1.md) for runnable fixtures.

The optional `moderation probe --rules-preview` now connects a bounded Douyin
WebSocket decoder to live preview rules. See [online integration testing](docs/moderation-online.md)
for the browser steps, current coverage, and pause reasons. Gift coverage is
partial; real chat execution remains pending.

M2's first increment adds optional `--rules-preview --scheduler-preview` for
rate-limited dispatch simulation, queue priority, gift reopening and expiry.
It does not issue chat commands. See [M2 browser integration testing](docs/moderation-m2.md).

Optional `--fill-preview` adds a typed fill-only executor and audited receipts.
It writes a draft into the chat input without clicking send, keyboard submission
or platform send requests. A fixed `--fill-test-text` can test the input without
waiting for gifts. See [fill-only integration testing](docs/moderation-fill.md).

Moderation also supports a versioned YAML file with rule switches, templates,
resource limits and preview scheduling budgets. Copy `moderation.example.yml`
to `moderation.yml`, validate with `moderation check --config moderation.yml`,
then start with `moderation run --config moderation.yml`. All modes keep chat
submission disabled. See [YAML configuration and integration testing](docs/moderation-config.md).

The 0.3.0 release adds a shared controller for concurrent platforms and room tasks,
with per-room journals and cross-room account budgets. See [controller setup](docs/moderation-release-0.3.0.md).

The 0.2.7 patch accepts longer WeChat user identities and fixes the raw-log proxy route.
See [gift recognition fix](docs/moderation-release-0.2.7.md).

The 0.2.6 release automatically expires local logs and caps raw captures, with preserved runs
and idle disk reclamation. See [retention settings](docs/moderation-release-0.2.6.md).

The 0.2.5 release records injected incoming live messages before decoding and filtering
in a separate local raw journal. See [development logging](docs/moderation-release-0.2.5.md).

The 0.2.4 release shares configurable gift wording and a random emoji suffix
between both platforms. See [wording setup](docs/moderation-release-0.2.4.md).
The 0.2.3 patch opens the collapsed WeChat chat editor for pending thanks,
with real-page continuous submission verified. See [0.2.3 setup](docs/moderation-release-0.2.3.md).
The 0.2.2 patch fixes nested comment-control selection; see
[update instructions](docs/moderation-release-0.2.2.md).
The 0.2.1 release adds explicitly configured automatic submission; see
[automatic submission setup](docs/moderation-release-0.2.1.md).
The 0.2.0 gift-thanks release supports Douyin and WeChat desktop input filling
with manual sending and no quantities. See [release setup](docs/moderation-release-0.2.0.md).
WeChat desktop live-page injection also has an observation-only probe using the same
proxy approach. `moderation.wechat.example.yml` enables bounded network-shape
diagnostics without storing raw network bodies or issuing chat actions.
Text and reward gift candidates now support document-local deduplication and
acknowledged storage; gift quantity semantics and room identity remain unverified.
Use `moderation inspect-gifts --journal data/moderation-wechat/moderation-probe.sqlite3`
to inspect anonymized candidate count sequences without starting a proxy or rules.
See [WeChat desktop injection testing](docs/moderation-wechat.md).

The WeChat example enables `injection.system_proxy: true` to reuse recording's
native proxy and CA setup. HTTP/HTTPS proxy settings are restored on shutdown;
the local CA remains trusted for later sessions.

Live Archive is self-contained: recording tasks, the local capture endpoint,
the page observer, and the proxy lifecycle live in this Rust workspace. The
small set of browser-compatibility resources required by the existing WeChat
Channels flow is maintained under `assets/wechat_capture/`; no upstream source
tree, Go service, build tooling, or unrelated features are vendored. License
and provenance information are in [LICENSE](LICENSE) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Next increments

1. Add an authenticated, policy-compliant Douyin resolver with fixture-based parser tests.
2. Add the user-approved local-proxy capture flow for WeChat Channels, including explicit certificate/proxy consent and restoration.
3. Add scheduler/retry policy and a GPUI-compatible desktop shell.

视频号可选候选礼物感谢日志预览：在 `mode: observe` 下设置 YAML `candidate_preview.enabled: true` 与明确选定的 `candidate_preview.recipient_user_id`。先用 `moderation inspect-gift-targets --journal data/moderation-wechat/moderation-probe.sqlite3` 列出最新采集的来源目标。按用户等待 5 秒并合并名称，只写终端和独立日志表，不填入或发送；目标/数量/尾条限制见 [视频号说明](docs/moderation-wechat.md)。

The 0.3.1 release improves gift ingestion and records network chat echoes separately from local submissions. See [delivery evidence and upgrade notes](docs/moderation-release-0.3.1.md).

The 0.3.2 release adds configurable value-tier gift wording pools with random selection and repeat avoidance. See [wording presets](docs/moderation-release-0.3.2.md).

The 0.3.3 release refreshes all 38 gift wording presets with a more energetic livestream style.

The 0.3.4 release fixes WeChat Ret/ret echo parsing and resumes queued thanks after a durable matching echo, without retrying the uncertain submission. See [recovery behavior](docs/moderation-release-0.3.4.md).

The 0.3.5 release supports shared platform-scoped nickname tables and per-room overrides for gift thanks. See [nickname mapping](docs/moderation-release-0.3.5.md).

The 0.3.7 release fixes WeChat anonymous donor names using their public mystery-viewer badge number. See [nickname fix](docs/moderation-release-0.3.7.md).

The 0.3.6 release adds room-scoped periodic random messages with gift priority and shared account budgets. See [timed messages](docs/moderation-release-0.3.6.md).

进场欢迎支持抖音和桌面视频号，每个直播间收集 30 秒后合并一条；重复进场去重，长名单自动缩短。配置、识别范围和验证说明见 [进场用户合并欢迎](docs/moderation-welcome.md)。

本地发布时运行 `python3 release/package.py`，打包流程会自动继承 `dist` 中上一版本的整个 `config` 目录，保留旧配置值和自定义文件，新版本新增的配置文件使用模板默认值。校验通过后再替换配置，压缩包也包含继承后的配置。修复已生成版本可使用 `--inherit-config-only`，原目标配置会备份到该版本目录下的 `config.before-inherit`（重名时添加编号）。

本地开发使用 `cargo build -p live-archive-core`；本地优化构建使用 `cargo build -p live-archive-core --release --locked`。两者默认只构建当前系统与 CPU 架构，`python -X utf8 release/package.py` 也只打包当前平台，并仅从相同平台的旧版本继承配置。

推送 `v<版本号>` 标签后，[发布流程](.github/workflows/build.yml) 在两台原生构建机上同时测试和打包 Windows x64（ZIP）与 Mac Apple Silicon（arm64，tar.gz）。两个目标全部成功、版本号与标签一致、安装包及 SHA-256 文件齐全且校验通过后，才统一发布到同一个 GitHub Release。云端使用仓库中的默认配置，不继承本机配置；各平台的 Python、FFmpeg 和语音模型仍需单独准备。

评论关键词自动回复支持抖音、视频号，多个关键词可共用回复并共享冷却。见 [配置说明](docs/moderation-keywords.md)。发布打包继承上一版本 config。

Windows 0.3.68 更新及升级步骤见 [发布说明](docs/moderation-release-0.3.68.md)。

0.3.70 起 Windows 与 Mac Apple Silicon 同步发布，见 [升级说明](docs/moderation-release-0.3.70.md)。
