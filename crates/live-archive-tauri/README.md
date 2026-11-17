# Tauri 桌面客户端

此客户端直接展示现有 `assets/moderation/dashboard.html`，由进程内中控提供页面和 API。GPUI 客户端仍保留。本目录是独立 Cargo workspace，Windows 发布版从 0.5.0 起使用此客户端。

Windows 开发环境（在仓库根目录执行）：

```powershell
$env:CARGO_HOME="$env:LOCALAPPDATA\Temp\solid-potato-rust\cargo"
$env:RUSTUP_HOME="$env:LOCALAPPDATA\Temp\solid-potato-rust\rustup"
$env:PATH="$env:CARGO_HOME\bin;$env:PATH"
$env:CARGO_TARGET_DIR="$PWD\target"
cargo run --manifest-path crates/live-archive-tauri/Cargo.toml -- '配置目录的绝对路径\unified.yml'
```

不传参数时与 GPUI 一样，读取可执行文件旁 `config/unified.yml`。请使用已有的有效配置和配套工具；不要同时启动占用同一配置、端口或系统代理的旧客户端。Windows 需要 WebView2 Runtime 和 Rust/MSVC 构建工具。

关闭窗口后程序会等待中控停止，执行录制收尾和代理恢复；异常退出时保留现有代理看护进程机制。页面中的新窗口链接交给系统默认浏览器打开，便于安装抖音用户脚本。

首次运行或变更配置前请检查其代理和端口设置。
