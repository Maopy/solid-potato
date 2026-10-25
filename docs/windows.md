# Windows 开发与运行

支持 Windows 10/11 的原生 Rust CLI 和浏览器场控入口。GPUI 桌面壳仍是可选实验项目，以下命令只构建 `live-archive-core`。

## 环境与验证

安装 Rust stable MSVC 工具链，以及 Visual Studio Build Tools 的“使用 C++ 的桌面开发”工作负载和 Windows SDK，重新打开 PowerShell。录制需要 FFmpeg，视频号代理需要 mitmproxy 的 `mitmdump.exe`，将它们加入 PATH。Python 测试使用 `python`（建议 3.12 或更新），脚本测试需要 Node.js。

```powershell
cargo build -p live-archive-core --release --locked
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

解压整个 ZIP，编辑 `config` 中的直播间配置（视频号默认 `candidate_preview.recipient_from_page: true`，从直播页识别主播；也可填写 `recipient_user_id` 手动指定），再双击 `启动场控.cmd` 或 `run.cmd`。支持中文和空格路径，自动切换到发布包目录并在默认浏览器打开控制页。可将便携版 FFmpeg 及其所需 DLL 放进包内 `tools` 目录，或使用 PATH 中安装的版本。mitmdump 同样必须可被找到。

系统代理和证书行为沿用 YAML 中明确启用的 `injection.system_proxy`。首次运行可能出现 Windows 证书信任提示，证书安装到当前用户的受信任根证书存储。正常退出在终端按 **Ctrl+C**。Windows 场控还会启动独立恢复进程，在主进程异常终止后恢复仍由本程序占用的 HTTP/HTTPS 代理，保留后来手工修改的代理。

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

这台机器的共享语音配置位于 `dist/asr.dev.json`，优先于项目根目录的旧 Mac 开发配置。使用 FireRedASR2S 项目的 **FireRedASR2-AED** 识别组件、RTX 3090 CUDA 推理和常驻 Python 进程。现有直播音频分段及静音过滤继续使用，不加载该项目的额外 VAD、语种识别和标点模型。

- 权重：`dist/.live-archive-models/FireRedASR2-AED/`，主权重 4731558506 字节，SHA-256 `4677cbd30988d63ed3e777f6a42a1e5260a3865317f6e15e488bef40954f7054`。
- 独立环境：`dist/.live-archive-tools/firered-venv/`，Python 3.12、PyTorch / torchaudio 2.10.0 CUDA 12.8。解释器副本位于 `dist/.live-archive-tools/python312/`，监护进程直接启动解释器并加载独立环境的 site-packages，绕过 Windows 虚拟环境重定向启动器。
- 官方源码：`dist/.live-archive-tools/FireRedASR2S/`；传输适配：`tools/firered_asr_worker.py`，部署副本在共享工具目录内。本机配置通过 `firered_asr_launcher.py` 监护进程转发结果，捕获模型子进程的非零退出码及最多 3 KB 的错误尾部；诊断只保存在内存，不落盘保存音频或识别文字。
- 配置新增 `provider: firered_asr2_aed`、`runner` 和 `source`，`model` 指向模型目录。`model_sha256` 校验其中的主权重，`use_gpu: true` 要求 CUDA 可用，不会静默退回 CPU。

首次启用时校验权重并加载模型。后续音频复用同一进程，各直播间串行识别；推理出错或被取消时销毁进程，下一次重新加载。页面最多显示最近三行文字，关闭语音清空页面会话；临时音频随请求结束删除。

这些本机依赖位于各版本发布目录之外，多版本共用，没有放入发布 ZIP。升级到包含 FireRed 适配的后台需要正常退出旧后台，再双击 `启动场控.cmd`。后续只调整模型配置时，重新打开页面的语音开关即可。

旧 Whisper 模型和运行时保留，切换前的配置备份为 `dist/asr.whisper.before-firered.json`；恢复该配置可继续使用旧引擎。`tools/transcribe_local.py` 仍是 Whisper 离线开发工具，FireRed 实际管线可用 Rust 的 `real_audio_to_local_model_without_saving_history` 测试验证。
