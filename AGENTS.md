# 发布约定

新增功能发布时更新版本号，构建并打包独立的新版本，保留原版本；不得用同一版本号覆盖已发布的程序。同步更新相关版本标识和发布说明。外部发布按用户授权执行。

# 本机 Rust 环境

Windows 上项目使用 `%LOCALAPPDATA%\Temp\solid-potato-rust` 下的 Rust 环境，可能不在 PATH 中。PowerShell 中先设置 `$env:CARGO_HOME="$env:LOCALAPPDATA\Temp\solid-potato-rust\cargo"`、`$env:RUSTUP_HOME="$env:LOCALAPPDATA\Temp\solid-potato-rust\rustup"`，再将 `$env:CARGO_HOME\bin` 加入 PATH 后运行 cargo。若临时目录被清理，先查找现有环境再判断是否需要重新安装。
