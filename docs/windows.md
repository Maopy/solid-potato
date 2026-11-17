# Windows 开发与运行

支持 Windows 10/11 的 Tauri 桌面中控和 Rust 后台。0.5.0 起 Windows 桌面端复用 Web 控制页，旧版 GPUI 客户端仍保留在旧发布包中。

## 环境与验证

安装 Rust stable MSVC 工具链、Microsoft Edge WebView2 Runtime，以及 Visual Studio Build Tools 的“使用 C++ 的桌面开发”工作负载和 Windows SDK，重新打开 PowerShell。录制需要 FFmpeg，视频号代理需要 mitmproxy 的 `mitmdump.exe`，将它们加入 PATH。Python 测试使用 `python`（建议 3.12 或更新），脚本测试需要 Node.js。

```powershell
cargo build -p live-archive-core --release --locked
cargo build --manifest-path crates/live-archive-tauri/Cargo.toml --release --locked
cargo test -p live-archive-core --locked -- --test-threads=4
python -X utf8 tests/release_package_test.py
node --test tests/*.test.cjs
cargo run -p live-archive-core -- doctor
```

集成测试会启动多个 Node/mitmdump 子进程，Windows 建议将测试并发限制为 4。

`doctor` 报告 FFmpeg、mitmdump 和录制目录状态；`--ffmpeg` 也支持完整的 `.exe` 路径。无需 WSL 或 Git Bash。

## 打包和启动

在项目根目录执行：

```powershell
python -X utf8 release/package.py
```

自动识别本机系统和架构，常见 x64 Windows 生成 `dist/live-archive-moderation-<版本>-windows-x64.zip` 及 SHA256 文件。自定义 Cargo 输出目录可传 `--binary <exe路径>`。`--target windows-x64` 仅选择包名和资源，不执行交叉编译，须提供对应架构的可运行程序。

解压整个 ZIP，编辑 `config` 中的直播间配置（视频号默认 `candidate_preview.recipient_from_page: true`，从直播页识别主播；也可填写 `recipient_user_id` 手动指定），再双击 `启动场控.exe`。支持中文和空格路径。可将便携版 FFmpeg、mitmdump 及所需 DLL 放进包内 `tools` 目录，或使用 PATH 中安装的版本。

系统代理和证书行为沿用 YAML 中明确启用的 `injection.system_proxy`。首次运行可能出现 Windows 证书信任提示，证书安装到当前用户的受信任根证书存储。正常退出时关闭中控窗口，等待后台完成录制收尾和代理恢复。Windows 场控还会启动独立恢复进程，在主进程异常终止后恢复仍由本程序占用的 HTTP/HTTPS 代理，保留后来手工修改的代理。

Windows 代理在套接字清理的 `shutdown()` 阶段遇到对端重置（WinError 10054）时继续关闭连接并注销，避免 Python Proactor 清理回调抛错。兼容处理仅存在于本程序生成的代理插件中，不修改全局 Python；其他连接错误和协议异常仍会报告。

## 从 Mac 迁移

默认只继承同系统、同架构的上一版本配置。迁移 Mac 配置时，先修改 `/Users/...`、`/Volumes/...`、`/opt/homebrew/...` 等绝对路径，再显式指定旧目录：

```powershell
python -X utf8 release/package.py --config-from 'E:/备份/旧版本/config'
```

继承保留自定义文件及旧配置值，校验通过后替换。已有目标发布目录时使用 `--inherit-config-only --config-from <目录>`，旧目标配置备份到 `config.before-inherit`。

YAML 路径建议使用正斜杠或单引号，避免双引号中反斜杠被当作转义：

```yaml
recording:
  output_dir: 'E:/直播录屏'
  ffmpeg: 'C:/Program Files/ffmpeg/bin/ffmpeg.exe'
```

`output_dir: null` 使用 Windows 用户下载文件夹下的“直播录屏”。会话存入 Windows Credential Manager；Mac 钥匙串不会随配置迁移，需要在 Windows 重新设置或导入会话。


视频号未识别主播身份时，场控和录屏仍能启动，页面会提示礼物感谢暂停。直播加载完成后刷新直播页可重新绑定；不会使用虚构 ID 或从收到的任意礼物猜测感谢目标。


## 本机语音模型

新版使用 FunASR Paraformer、FSMN-VAD 和 CT-Transformer，模型与 Python 环境位于发布目录旁的共享目录，不随发布包分发。`asr.dev.json` 中 `provider` 为 `funasr_paraformer`，模型与热词可按需配置。首次启用会加载模型，识别在本机运行。

FSMN-VAD 用于检测语音停顿。可使用 CAM++ 声纹对指定直播间做保守筛选，但这不能分离重叠人声，仍可能误滤主播或保留歌词。
