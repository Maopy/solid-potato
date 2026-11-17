# 0.4.17 FunASR 语音优化

- FunASR 使用 FSMN-VAD 检测语音、Paraformer 识别文字，并用 CT-Transformer 补充标点。
- 新版继承原有场控配置；语音模型和运行环境保存在共享目录，旧版本保留。
