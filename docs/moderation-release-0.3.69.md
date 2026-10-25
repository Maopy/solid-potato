# 0.3.69 Windows 与 Mac 同步发布

- 每个版本同时构建 Windows x64（ZIP）和 Mac Apple Silicon（arm64，tar.gz），不提供 Mac Intel 发布包。
- 两个平台分别在原生系统中测试、构建和校验配置；安装包及 SHA-256 文件齐全且校验通过后，统一发布到同一个 GitHub Release。
- 本地开发和打包仍只处理当前系统与 CPU 架构，继承相同平台上一版本的配置。
- 核心与抖音脚本版本统一为 0.3.69；包含 0.3.68 的 Windows、FireRed 语音、抖音长按 Z 连赞和视频号修复。

## 升级

Windows 解压后打开“启动场控.cmd”；Mac Apple Silicon 解压后打开“启动场控.command”。退出旧后台后再启动新版，避免中断正在进行的录制。抖音从新版控制台重新安装 0.3.69 脚本并刷新直播页。

公开下载包使用默认配置。自行保留或迁移已有 config；本机打包自动继承同平台旧版本配置。Python、FFmpeg、FireRed 模型与运行依赖不包含在安装包中，继续使用本机已有安装。
