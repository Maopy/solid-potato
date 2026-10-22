# 本地语音识别开发环境

开发阶段使用 whisper.cpp 的 Whisper small 多语种模型，语言设为中文（zh），4 个 CPU 线程，默认允许 Apple Metal 加速。当前环境引擎为 Homebrew whisper.cpp 1.9.5。

模型保存在 `dist/.live-archive-models/ggml-small.bin`，大小 487601967 字节。该目录在发布包之外，多版本可共用，不需要随每个新版重复复制。

官方模型：https://huggingface.co/ggerganov/whisper.cpp/blob/main/ggml-small.bin

SHA-256：`1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b`

官方站直连失败时，本次从 Telestream 的 Whisper 模型镜像下载，并按上述官方大小及 SHA-256 验证相同内容。转写运行前也会重新检查模型哈希。

## 使用

在项目根目录执行：

```sh
python3 tools/transcribe_local.py /绝对路径/录像.mp4 --start 60 --duration 30
```

默认只读取前 30 秒，可指定 `--start` 和 `--duration`。可通过 `--output` 指定一个尚不存在的输出目录；默认输出到 `data/asr-dev/时间戳/`。输入文件只读，解码用的临时 WAV 在识别后自动清理。没有远程识别请求。

使用 `--cpu` 可禁用 GPU，供 CPU 方案实验。模型路径、线程、语言及可执行文件通过根目录 `asr.dev.json` 配置；相对模型路径相对于该配置文件。

## 输出

- `transcript.txt`：转写文字。
- `transcript.srt`：字幕，时间相对于所选音频片段。
- `transcript.json`：引擎原始结果。
- `segments.jsonl`：开发使用的标准片段数据，时间相对于原始输入文件，已加上 `--start` 偏移。
- `run.json`：识别引擎、模型、耗时与输入片段信息。
- `engine.log`：本地引擎日志，诊断加载或推理问题。

JSONL 每行包含 `id`、`start_ms`、`end_ms`、`text`、`is_final`、`boundary`、`provider`。`is_final` 表示该次音频片段的最终识别结果；`boundary=asr_segment` 表示模型划分的片段，未保证每个片段恰好是一句完整的话。后续语义处理需进一步合句、断句。

本次仅准备本地模型及录音片段开发入口，尚未自动接入场控的直播实时音频。上线前的云端 API 适配可以复用 `segments.jsonl` 的输出字段，无需把后续语义逻辑绑定到本地模型。

## 安装验收（2026-10-19）

在当前 M2 Pro / 32 GB 上，截取用户提供录像的 60～90 秒，使用中文模型完成 30 秒音频转写，输出 22 个带时间戳片段。该次进程识别耗时约 12.4 秒，总处理约 12.5 秒。此数据只代表该机器和该片段，不代表低端机或长时间直播的表现。模型可能输出繁体字和错认人名，后续需做格式规范及识别质量评估。

验收产物：`data/asr-dev/local-small-check/`。

## 场控实时接入（0.3.45）

双平台统一面板的“语音”按钮手动启停，默认关闭，不依赖场控开关或录像任务。约 12 秒音频作为一个识别窗口，面板只显示最近 3 句。转写不写入数据库或历史文件，短时 WAV 和引擎 JSON 在临时目录中处理后删除。停止或销毁页面会话时取消解码和推理、清空文字。

多个直播会话共享单个推理并发；每个会话最多保留最新一个待识别音频窗口，慢机器会跳过旧窗口而不是持续累积延迟。识别存在窗口和推理耗时，跨窗口句子仍可能不完整。与上面的离线开发工具不同，面板路径不会产出 transcript.txt、字幕或 segments.jsonl。

后台优先读取环境变量 LIVE_ARCHIVE_ASR_CONFIG 指向的文件，否则查找程序上层目录的 asr.dev.json；都没有时使用发布目录旁的 .live-archive-models/ggml-small.bin。配置中的相对模型路径以配置文件所在目录为基准。沿用模型 SHA-256 校验，音频仅在本机识别。
