#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};
use tauri::Manager;
use tokio::sync::oneshot;

struct Controller {
    stop: Mutex<Option<oneshot::Sender<()>>>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
    exiting: AtomicBool,
    stopped: AtomicBool,
}

fn stop_controller(handle: &tauri::AppHandle, controller: &Arc<Controller>) {
    if controller.exiting.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(window) = handle.get_webview_window("main") {
        let _ = window.set_title("直播助手 · 正在收尾…");
        let _ = window.eval("document.body.innerHTML='<p style=\"font:16px system-ui;padding:3rem;color:#e6e1cf;background:#0b0e14\">正在结束录制并恢复代理，请稍候…</p>'");
    }
    if let Some(stop) = controller.stop.lock().unwrap().take() {
        let _ = stop.send(());
    }
    let controller = controller.clone();
    let handle = handle.clone();
    thread::spawn(move || {
        if let Some(thread) = controller.thread.lock().unwrap().take() {
            let _ = thread.join();
        }
        controller.stopped.store(true, Ordering::SeqCst);
        // Let Tauri destroy the WebView window before its event loop exits.
        // An explicit app exit while the window is alive makes WebView2 try
        // to unregister a window class that still owns windows on Windows.
        match handle.get_webview_window("main") {
            Some(window) if window.close().is_ok() => {}
            _ => handle.exit(0),
        }
    });
}

fn config_path() -> PathBuf {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("config/unified.yml")))
                .unwrap_or_else(|| PathBuf::from("config/unified.yml"))
        })
}

fn prepare_tools_path() {
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let mut paths = vec![dir.join("tools")];
            if let Some(root) = dir.parent().and_then(|p| p.parent()) {
                paths.push(root.join(".venv/Scripts"));
            }
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            if let Ok(path) = std::env::join_paths(paths) {
                unsafe { std::env::set_var("PATH", path) };
            }
        }
    }
}

fn open_external(url: &str) {
    #[cfg(windows)]
    let command = ("rundll32.exe", vec!["url.dll,FileProtocolHandler", url]);
    #[cfg(target_os = "macos")]
    let command = ("open", vec![url]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let command = ("xdg-open", vec![url]);
    let _ = std::process::Command::new(command.0)
        .args(command.1)
        .spawn();
}

fn main() {
    if std::env::args_os()
        .skip(1)
        .take(2)
        .eq(["moderation", "proxy-watchdog"]
            .into_iter()
            .map(std::ffi::OsString::from))
    {
        if live_archive_core::moderation::proxy_watchdog::run().is_err() {
            std::process::exit(1);
        }
        return;
    }
    prepare_tools_path();
    let config = config_path();
    let (stop, mut stopped) = oneshot::channel();
    let (loaded, page_loaded) = oneshot::channel();
    let loaded = Mutex::new(Some(loaded));
    let controller = Arc::new(Controller {
        stop: Mutex::new(Some(stop)),
        thread: Mutex::new(None),
        exiting: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
    });
    let setup_controller = controller.clone();
    let app = tauri::Builder::default().setup(move |app| {
        let _window = tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("index.html".into()))
            .title("直播助手 · 中控台").inner_size(1280.0, 800.0).min_inner_size(640.0, 480.0)
            .on_page_load(move |_window, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    if let Some(loaded) = loaded.lock().unwrap().take() {
                        let _ = loaded.send(());
                    }
                }
            })
            .on_new_window(|url, _| {
                if matches!(url.scheme(), "http" | "https") { open_external(url.as_str()); }
                tauri::webview::NewWindowResponse::Deny
            }).build()?;
        let handle = app.handle().clone();
        let worker_controller = setup_controller.clone();
        let thread = thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
                runtime.block_on(async {
                    tokio::select! {
                        result = page_loaded => if result.is_err() { return Ok(()); },
                        _ = &mut stopped => return Ok(()),
                    }
                    live_archive_core::moderation::control::serve_in_process(&config, |url| {
                        if worker_controller.exiting.load(Ordering::SeqCst) { return; }
                        if let Some(window) = handle.get_webview_window("main") {
                            let url = url.replacen("/#", "/?desktop=1#", 1);
                            if let Ok(url) = url.parse() { let _ = window.navigate(url); }
                        }
                    }, async { let _ = stopped.await; }).await
                })
            })();
            if let Err(error) = result {
                eprintln!("中控启动或运行失败: {error:#}");
                if worker_controller.exiting.load(Ordering::SeqCst) { return; }
                if let Some(window) = handle.get_webview_window("main") {
                    let message = serde_json::to_string(&format!("{error:#}")).unwrap_or_default();
                    let _ = window.set_title("直播助手 · 中控运行失败");
                    let _ = window.eval(&format!("(()=>{{const p=document.createElement('pre');p.style.cssText='position:fixed;inset:0;z-index:99999;white-space:pre-wrap;padding:3rem;margin:0;background:#0b0e14;color:#f07178;font:16px system-ui';p.textContent='中控启动或运行失败：\\n'+{message};document.body.appendChild(p)}})()"));
                }
            }
        });
        *setup_controller.thread.lock().unwrap() = Some(thread);
        Ok(())
    }).on_window_event({
        let controller = controller.clone();
        move |window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !controller.stopped.load(Ordering::SeqCst) {
                    api.prevent_close();
                    stop_controller(window.app_handle(), &controller);
                }
            }
        }
    }).build(tauri::generate_context!()).expect("无法启动桌面窗口");
    app.run(move |handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            if controller.stopped.load(Ordering::SeqCst) {
                return;
            }
            api.prevent_exit();
            stop_controller(handle, &controller);
        }
    });
}
