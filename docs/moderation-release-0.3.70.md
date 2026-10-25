# 0.3.70 Windows 与 Mac Apple Silicon 同步发布

- 每个版本同时在原生系统中测试、构建和打包 Windows x64（ZIP）与 Mac Apple Silicon（arm64，tar.gz），不构建 Mac Intel 发布包。
- 两个平台全部通过，安装包和 SHA-256 文件齐全并校验成功后，才统一发布到同一个 GitHub Release。
- 本地 Cargo 构建和 Python 打包仍只使用当前系统与 CPU 架构；本地打包继续继承同平台旧版本配置。
- 修复 Python 3.12 与 3.14 清理测试的模拟对象兼容性；礼物集成测试精确核对后端截止时间，避免把云端进程启动耗时当成礼物重复计时错误。
- 核心与抖音脚本版本统一为 0.3.70；包含此前 Windows 启动、FireRed 语音、抖音长按 Z 连赞和视频号代理、缓存修复。

## 升级

Windows 解压后打开“启动场控.cmd”；Mac Apple Silicon 解压后打开“启动场控.command”。先完成当前录制并退出旧后台，再启动新版。抖音从新版控制台重新安装 0.3.70 脚本并刷新直播页。

公开下载包使用默认配置，请保留或迁移已有 config。本机打包自动继承同平台旧版本配置，包括录屏目录。Python、FFmpeg、FireRed 模型与运行依赖单独安装，继续使用本机已有安装。
