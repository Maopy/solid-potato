mod text_input;

use chrono::Utc;
use directories::ProjectDirs;
use gpui::{App, Application, Context, Entity, Render, Window, WindowOptions, div, prelude::*};
use gpui_component::{Root, WindowExt, button::ButtonVariant, dialog::DialogButtonProps};
use live_archive_core::{
    model::{Platform, RecordingTask, TaskStatus},
    store::TaskStore,
};
use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use text_input::{
    Backspace, Copy, Cut, Delete, End, Home, Left, Paste, Right, SelectAll, SelectLeft,
    SelectRight, TextInput,
};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordingInfo {
    quality: String,
    duration: String,
}

struct Dashboard {
    database: PathBuf,
    tasks: Vec<RecordingTask>,
    recording_info: HashMap<Uuid, RecordingInfo>,
    room_url: Entity<TextInput>,
    display_name: Entity<TextInput>,
    message: String,
    add_platform: Option<Platform>,
    capture: Option<Child>,
    capture_recording_active: bool,
    recordings: HashMap<Uuid, Child>,
}

impl Drop for Dashboard {
    fn drop(&mut self) {
        // Ask the capture command to stop its recording services and restore
        // the proxy before the GUI releases its child-process handle.
        if self.capture.is_some() {
            let _ = self.send_capture_command("quit");
            self.stop_active_wechat_tasks();
        }
        for child in self.recordings.values_mut() {
            if let Some(stdin) = child.stdin.as_mut() {
                let _ = writeln!(stdin, "stop");
                let _ = stdin.flush();
            }
        }
    }
}

impl Dashboard {
    fn stop_active_wechat_tasks(&self) {
        let Ok(store) = TaskStore::open(&self.database) else {
            return;
        };
        let Ok(tasks) = store.list() else {
            return;
        };
        for task in tasks {
            if task.platform == Platform::WeChatChannels
                && matches!(
                    task.status,
                    TaskStatus::Resolving | TaskStatus::Recording | TaskStatus::Retrying
                )
            {
                let _ = store.update_status(task.id, TaskStatus::Stopped);
            }
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.sync_tasks(cx, true);
    }

    /// Refresh the shared task database. Video Channels tasks are created by the
    /// capture process, so the GUI needs to observe that process' writes.
    fn sync_tasks(&mut self, cx: &mut Context<Self>, update_message: bool) {
        match TaskStore::open(&self.database).and_then(|store| store.list()) {
            Ok(tasks) => {
                let info = TaskStore::open(&self.database)
                    .ok()
                    .map(|store| {
                        tasks
                            .iter()
                            .map(|task| (task.id, recording_info(&store, task)))
                            .collect()
                    })
                    .unwrap_or_default();
                let changed = tasks != self.tasks || info != self.recording_info;
                self.tasks = tasks;
                self.recording_info = info;
                if changed || update_message {
                    cx.notify();
                }
            }
            Err(_) if update_message => cx.notify(),
            Err(_) => {}
        }
    }

    fn add_task(&mut self, platform: Platform, _window: &mut Window, cx: &mut Context<Self>) {
        let raw_url = self.room_url.read(cx).value();
        let name = self.display_name.read(cx).value();
        let room_url = match Url::parse(raw_url.trim()) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => url,
            _ => {
                self.message = "请输入有效的 http(s) 直播间链接。".into();
                cx.notify();
                return;
            }
        };
        let display_name = if name.trim().is_empty() {
            match platform {
                Platform::Douyin => "抖音直播间",
                Platform::WeChatChannels => "视频号直播间",
            }
            .to_owned()
        } else {
            name.trim().to_owned()
        };
        let now = Utc::now();
        let task = RecordingTask {
            id: Uuid::new_v4(),
            platform,
            room_url,
            display_name,
            status: TaskStatus::Idle,
            output_dir: PathBuf::from("data/recordings").join(platform.as_str()),
            created_at: now,
            updated_at: now,
        };
        match TaskStore::open(&self.database).and_then(|store| store.insert(&task)) {
            Ok(()) => {
                self.room_url.update(cx, |input, cx| input.clear(cx));
                self.display_name.update(cx, |input, cx| input.clear(cx));
                self.add_platform = None;
                self.reload(cx);
                self.message = "链接已保存到任务库。关闭或重启 GUI 后仍会保留。".into();
            }
            Err(error) => {
                self.message = format!("保存链接失败：{error}");
                cx.notify();
            }
        }
    }

    fn show_add(&mut self, platform: Platform, cx: &mut Context<Self>) {
        self.add_platform = Some(platform);
        cx.notify();
    }

    fn cancel_add(&mut self, cx: &mut Context<Self>) {
        self.add_platform = None;
        self.room_url.update(cx, |input, cx| input.clear(cx));
        self.display_name.update(cx, |input, cx| input.clear(cx));
        cx.notify();
    }

    fn start_douyin(&mut self, id: Uuid, _window: &mut Window, cx: &mut Context<Self>) {
        if self.recordings.contains_key(&id) {
            return;
        }
        let mut command = core_command();
        command.args([
            "--database",
            self.database.to_string_lossy().as_ref(),
            "record-douyin",
            &id.to_string(),
        ]);
        command.stdin(Stdio::piped());
        match command.spawn() {
            Ok(child) => {
                self.recordings.insert(id, child);
                let _ = TaskStore::open(&self.database)
                    .and_then(|store| store.update_status(id, TaskStatus::Resolving));
                self.message = "已启动抖音录制进程；状态会在刷新任务列表后同步。".into();
            }
            Err(error) => self.message = format!("无法启动抖音录制：{error}"),
        }
        self.reload(cx);
    }

    fn stop_douyin(&mut self, id: Uuid, _window: &mut Window, cx: &mut Context<Self>) {
        if self.recordings.contains_key(&id) {
            match self.send_douyin_command(id, "stop") {
                Ok(()) => {
                    self.recordings.remove(&id);
                    let _ = TaskStore::open(&self.database)
                        .and_then(|store| store.update_status(id, TaskStatus::Stopped));
                    self.message = "已停止抖音录制。".into();
                }
                Err(error) => self.message = format!("无法向抖音录制发送停止指令：{error}"),
            }
        }
        self.reload(cx);
    }

    fn send_douyin_command(&mut self, id: Uuid, command: &str) -> std::io::Result<()> {
        let child = self.recordings.get_mut(&id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotConnected, "抖音录制未启动")
        })?;
        let stdin = child.stdin.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "录制进程没有控制通道")
        })?;
        writeln!(stdin, "{command}")?;
        stdin.flush()
    }

    fn request_capture_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.capture.is_some() {
            self.capture_recording_active = true;
            self.message = "视频号监听已恢复。".into();
            cx.notify();
        } else {
            let dashboard = cx.entity().downgrade();
            window.open_dialog(cx, move |dialog, _, _| {
                let confirm = dashboard.clone();
                let cancel = dashboard.clone();
                dialog
                    .title("开启视频号监听")
                    .confirm()
                    .button_props(
                        DialogButtonProps::default()
                            .ok_text("确认开启")
                            .cancel_text("取消"),
                    )
                    .child("开启监听会启动本机 HTTPS 代理。首次运行可能需要系统授权安装并信任本地证书。")
                    .on_ok(move |_, window, cx| {
                        let _ = confirm.update(cx, |this, cx| this.start_capture(window, cx));
                        true
                    })
                    .on_cancel(move |_, _, cx| {
                        let _ = cancel.update(cx, |_, _| {});
                        true
                    })
            });
        }
    }
    fn start_capture(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let mut command = core_command();
        command.args([
            "--database",
            self.database.to_string_lossy().as_ref(),
            "wechat-capture",
            "--consent",
        ]);
        command.stdin(Stdio::piped());
        match command.spawn() {
            Ok(child) => {
                self.capture = Some(child);
                self.capture_recording_active = true;
                self.message = "视频号监听进程已启动。创建录制任务后会自动显示在下方列表。".into();
            }
            Err(error) => self.message = format!("无法启动监听：{error}"),
        }
        cx.notify();
    }

    fn watch_task_store(&mut self, cx: &mut Context<Self>) {
        cx.spawn(
            |this: gpui::WeakEntity<Dashboard>, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    loop {
                        cx.background_executor().timer(Duration::from_secs(1)).await;
                        if this
                            .update(&mut cx, |this, cx| this.sync_tasks(cx, false))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            },
        )
        .detach();
    }
    fn stop_capture(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.capture.is_some() {
            match self.send_capture_command("stop all") {
                Ok(()) => {
                    self.stop_active_wechat_tasks();
                    self.capture_recording_active = false;
                    self.message = "已停止全部视频号录制任务；后台监听进程保持就绪。".into();
                }
                Err(error) => {
                    self.message = format!("无法向视频号监听发送停止全部录制指令：{error}")
                }
            }
        }
        self.sync_tasks(cx, false);
        cx.notify();
    }

    fn send_capture_command(&mut self, command: &str) -> std::io::Result<()> {
        let child = self.capture.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotConnected, "视频号监听未启动")
        })?;
        let stdin = child.stdin.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "监听进程没有控制通道")
        })?;
        writeln!(stdin, "{command}")?;
        stdin.flush()
    }
    fn request_delete(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .tasks
            .iter()
            .find(|task| task.id == id)
            .map(presenter_name)
            .unwrap_or_else(|| "该链接".into());
        let dashboard = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let confirm = dashboard.clone();
            dialog
                .title("移除任务")
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("确认移除")
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text("取消"),
                )
                .child(format!("确认移除「{name}」？已经录制的文件会保留。"))
                .on_ok(move |_, window, cx| {
                    let _ = confirm.update(cx, |this, cx| this.delete_task(id, window, cx));
                    true
                })
        });
    }

    fn delete_task(&mut self, id: Uuid, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|task| task.id == id) else {
            self.message = "任务不存在，无法删除。".into();
            cx.notify();
            return;
        };
        let active_douyin_task = task.platform == Platform::Douyin
            && (self.recordings.contains_key(&id)
                || matches!(
                    task.status,
                    TaskStatus::Resolving | TaskStatus::Recording | TaskStatus::Retrying
                ));
        if active_douyin_task {
            if !self.recordings.contains_key(&id) {
                self.message = "该抖音录制不是由当前窗口启动，无法安全停止；任务未删除。".into();
                cx.notify();
                return;
            }
            if let Err(error) = self.send_douyin_command(id, "stop") {
                self.message = format!("无法停止抖音录制，任务未删除：{error}");
                cx.notify();
                return;
            }
            // The recorder finalizes TS to MP4 asynchronously after accepting
            // `stop`; dropping this handle does not kill that child process.
            self.recordings.remove(&id);
        }
        let active_wechat_task = self
            .tasks
            .iter()
            .find(|candidate| candidate.id == id)
            .is_some_and(|candidate| {
                candidate.platform == Platform::WeChatChannels
                    && matches!(
                        candidate.status,
                        TaskStatus::Resolving | TaskStatus::Recording | TaskStatus::Retrying
                    )
            });
        if active_wechat_task && self.capture.is_some() {
            if let Err(error) = self.send_capture_command(&format!("stop {id}")) {
                self.message = format!("无法停止视频号录制，未删除任务：{error}");
                cx.notify();
                return;
            }
        }
        match TaskStore::open(&self.database).and_then(|store| store.delete(id)) {
            Ok(()) => {
                self.reload(cx);
                self.message = "链接已移除；已经录制的文件不会删除。".into();
            }
            Err(error) => self.message = format!("删除链接失败：{error}"),
        }
    }

    fn add_form(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let platform = self.add_platform?;
        Some(
            card("添加直播间")
                .child(muted(
                    "填写链接后保存。任务会持久化到本地数据库，后续可在此列表管理。",
                ))
                .child(self.room_url.clone())
                .child(self.display_name.clone())
                .child(
                    div()
                        .flex()
                        .gap_3()
                        .child(button(
                            "save-task",
                            "保存任务",
                            gpui::rgb(0x187a55),
                            cx,
                            move |this, window, cx| this.add_task(platform, window, cx),
                        ))
                        .child(button(
                            "cancel-add",
                            "取消",
                            gpui::rgb(0x4c5563),
                            cx,
                            |this, _, cx| this.cancel_add(cx),
                        )),
                ),
        )
    }

    fn task_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .flex()
            .items_center()
            .gap_4()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(gpui::rgb(0x303845))
            .bg(gpui::rgb(0x202631))
            .text_xs()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(gpui::rgb(0x9aa0a6))
            .child(div().w_64().child("主播"))
            .child(div().w_24().child("平台"))
            .child(div().w_32().child("清晰度"))
            .child(div().w_24().child("录制时长"))
            .child(div().w_24().child("状态"))
            .child(div().flex_1())
            .child(div().w_48().child("操作"));
        let mut content = div()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_md()
            .bg(gpui::rgb(0x1b2028))
            .border_1()
            .border_color(gpui::rgb(0x303845))
            .child(header);
        if self.tasks.is_empty() {
            return content.child(
                div()
                    .p_6()
                    .text_sm()
                    .text_color(gpui::rgb(0x9aa0a6))
                    .child("暂无任务。添加抖音链接，或开启视频号监听后在微信内创建录制任务。"),
            );
        }
        for (index, task) in self.tasks.iter().enumerate() {
            let id = task.id;
            let mut actions = div().flex().gap_2().w_48();
            if task.platform == Platform::Douyin {
                actions = if self.recordings.contains_key(&id) {
                    actions.child(button(
                        ("stop", index),
                        "停止",
                        gpui::rgb(0x7f3038),
                        cx,
                        move |this, window, cx| this.stop_douyin(id, window, cx),
                    ))
                } else {
                    actions.child(button(
                        ("start", index),
                        "录制",
                        gpui::rgb(0x315d9a),
                        cx,
                        move |this, window, cx| this.start_douyin(id, window, cx),
                    ))
                };
            } else {
                actions = actions.child(muted(if self.capture_recording_active {
                    "监听中"
                } else if self.capture.is_some() {
                    "已停止"
                } else {
                    "由全局监听"
                }));
            }
            actions = actions.child(button(
                ("delete", index),
                "删除",
                gpui::rgb(0x7f3038),
                cx,
                move |this, window, cx| this.request_delete(id, window, cx),
            ));
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(gpui::rgb(0x303845))
                    .bg(if index.is_multiple_of(2) {
                        gpui::rgb(0x1b2028)
                    } else {
                        gpui::rgb(0x1e252f)
                    })
                    .hover(|style| style.bg(gpui::rgb(0x293340)))
                    .child(div().w_64().child(presenter_name(task)))
                    .child(div().w_24().child(platform_name(task.platform)))
                    .child(
                        div()
                            .w_32()
                            .text_sm()
                            .text_color(gpui::rgb(0xb8c0cc))
                            .child(
                                self.recording_info
                                    .get(&id)
                                    .map(|info| info.quality.clone())
                                    .unwrap_or_else(|| "—".into()),
                            ),
                    )
                    .child(
                        div()
                            .w_24()
                            .text_sm()
                            .text_color(gpui::rgb(0xb8c0cc))
                            .child(
                                self.recording_info
                                    .get(&id)
                                    .map(|info| info.duration.clone())
                                    .unwrap_or_else(|| "—".into()),
                            ),
                    )
                    .child(div().w_24().child(status_badge(task.status)))
                    .child(div().flex_1())
                    .child(actions),
            );
        }
        content
    }
}

impl Render for Dashboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .id("dashboard")
            .flex()
            .size_full()
            .flex_col()
            .overflow_y_scroll()
            .bg(gpui::rgb(0x111722))
            .text_color(gpui::rgb(0xe8eaed))
            .p_4()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(gpui::rgb(0x1b2028))
                    .border_1()
                    .border_color(gpui::rgb(0x303845))
                    .shadow_sm()
                    .child(button(
                        "toolbar-add-douyin",
                        "添加抖音",
                        gpui::rgb(0x315d9a),
                        cx,
                        |this, _, cx| this.show_add(Platform::Douyin, cx),
                    ))
                    .child(if self.capture_recording_active {
                        button(
                            "toolbar-capture-stop",
                            "停止视频号监听",
                            gpui::rgb(0x7f3038),
                            cx,
                            |this, window, cx| this.stop_capture(window, cx),
                        )
                    } else {
                        button(
                            "toolbar-capture-start",
                            "开启视频号监听",
                            gpui::rgb(0x187a55),
                            cx,
                            |this, window, cx| this.request_capture_start(window, cx),
                        )
                    })
                    .child(button(
                        "refresh",
                        "刷新",
                        gpui::rgb(0x4c5563),
                        cx,
                        |this, _, cx| this.reload(cx),
                    ))
                    .child(div().flex_1())
                    .child(muted(&format!("{} 个任务", self.tasks.len()))),
            );
        if let Some(form) = self.add_form(cx) {
            root = root.child(form);
        }
        root.child(self.task_list(cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

fn card(title: &'static str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded_md()
        .bg(gpui::rgb(0x1b2028))
        .border_1()
        .border_color(gpui::rgb(0x303845))
        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(title))
}
fn muted(text: &str) -> gpui::Div {
    div()
        .text_sm()
        .text_color(gpui::rgb(0xb8c0cc))
        .child(text.to_owned())
}
fn button(
    id: impl Into<gpui::ElementId>,
    text: &str,
    color: gpui::Rgba,
    cx: &mut Context<Dashboard>,
    on_click: impl Fn(&mut Dashboard, &mut Window, &mut Context<Dashboard>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(color)
        .text_sm()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.opacity(0.88))
        .child(text.to_owned())
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
}
fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Douyin => "抖音",
        Platform::WeChatChannels => "微信视频号",
    }
}
fn presenter_name(task: &RecordingTask) -> String {
    if task.platform == Platform::WeChatChannels {
        // The stable WeChat username is retained in storage for matching, but
        // the list should remain focused on the human-readable anchor name.
        task.display_name
            .rsplit_once(" [")
            .filter(|(_, suffix)| suffix.ends_with(']'))
            .map(|(name, _)| name)
            .unwrap_or(&task.display_name)
            .to_owned()
    } else {
        task.display_name.clone()
    }
}

fn recording_info(store: &TaskStore, task: &RecordingTask) -> RecordingInfo {
    if task.status != TaskStatus::Recording {
        return RecordingInfo {
            quality: if task.status == TaskStatus::Resolving {
                "解析中".into()
            } else {
                "—".into()
            },
            duration: "—".into(),
        };
    }
    let session = store
        .sessions_for_task(task.id)
        .ok()
        .and_then(|sessions| sessions.into_iter().next());
    let Some(session) = session else {
        return RecordingInfo {
            quality: "原始流".into(),
            duration: "—".into(),
        };
    };
    let started_at = if session.finished_at.is_none() {
        // The status transition is a compatible fallback for active sessions
        // written by earlier versions, regardless of platform. New sessions
        // persist this same real recorder-start moment in `started_at`.
        task.updated_at
    } else {
        session.started_at
    };
    let end = session.finished_at.unwrap_or_else(Utc::now);
    let seconds = end.signed_duration_since(started_at).num_seconds().max(0) as u64;
    RecordingInfo {
        // FFmpeg uses stream copy for both recording and TS-to-MP4 remuxing,
        // so the saved file retains the source stream's quality.
        quality: "原始流".into(),
        duration: format_recording_duration(seconds),
    }
}

fn format_recording_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = seconds % 3600 / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}
fn database_path() -> PathBuf {
    ProjectDirs::from("io", "livearchive", "LiveArchive")
        .map(|dirs| dirs.data_local_dir().join("live-archive.sqlite3"))
        .unwrap_or_else(|| PathBuf::from("data/live-archive.sqlite3"))
}
fn core_command() -> Command {
    // `cargo run -p live-archive-gui` only rebuilds the core library dependency,
    // not the separate core CLI binary. In development, always use Cargo so the
    // recorder process includes the current control and status-sync code.
    if cfg!(debug_assertions) {
        let mut command = Command::new("cargo");
        command.args(["run", "--quiet", "-p", "live-archive-core", "--"]);
        return command;
    }
    let mut path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("live-archive-core"));
    path.set_file_name(if cfg!(windows) {
        "live-archive-core.exe"
    } else {
        "live-archive-core"
    });
    if path.is_file() {
        return Command::new(path);
    }
    Command::new("live-archive-core")
}

fn status_badge(status: TaskStatus) -> gpui::Div {
    let (text, background) = match status {
        TaskStatus::Recording => (gpui::rgb(0x75d9a5), gpui::rgb(0x1d4737)),
        TaskStatus::Resolving | TaskStatus::Retrying => (gpui::rgb(0xf1c66d), gpui::rgb(0x4a3b1c)),
        TaskStatus::Failed => (gpui::rgb(0xf08b95), gpui::rgb(0x4a252c)),
        TaskStatus::Stopped => (gpui::rgb(0xb8c0cc), gpui::rgb(0x313946)),
        _ => (gpui::rgb(0xb8c0cc), gpui::rgb(0x28303c)),
    };
    div()
        .px_2()
        .py_0p5()
        .rounded_full()
        .text_xs()
        .text_color(text)
        .bg(background)
        .child(status.as_str())
}

fn main() {
    Application::new().run(|cx: &mut App| {
        gpui_component::init(cx);
        cx.bind_keys([
            gpui::KeyBinding::new("backspace", Backspace, None),
            gpui::KeyBinding::new("delete", Delete, None),
            gpui::KeyBinding::new("left", Left, None),
            gpui::KeyBinding::new("right", Right, None),
            gpui::KeyBinding::new("shift-left", SelectLeft, None),
            gpui::KeyBinding::new("shift-right", SelectRight, None),
            gpui::KeyBinding::new("cmd-a", SelectAll, None),
            gpui::KeyBinding::new("cmd-v", Paste, None),
            gpui::KeyBinding::new("cmd-c", Copy, None),
            gpui::KeyBinding::new("cmd-x", Cut, None),
            gpui::KeyBinding::new("ctrl-a", SelectAll, None),
            gpui::KeyBinding::new("ctrl-v", Paste, None),
            gpui::KeyBinding::new("ctrl-c", Copy, None),
            gpui::KeyBinding::new("ctrl-x", Cut, None),
            gpui::KeyBinding::new("home", Home, None),
            gpui::KeyBinding::new("end", End, None),
        ]);
        cx.open_window(WindowOptions::default(), |window, cx| {
            let room_url = cx.new(|cx| TextInput::new(cx, "直播间链接（https://…）"));
            let display_name = cx.new(|cx| TextInput::new(cx, "显示名称（可选）"));
            let database = database_path();
            let (tasks, message) = match TaskStore::open(&database).and_then(|store| store.list()) {
                Ok(tasks) if tasks.is_empty() => {
                    (tasks, "暂无链接。添加后会与 CLI 共用同一任务库。".into())
                }
                Ok(tasks) => (tasks, "任务列表已加载。".into()),
                Err(error) => (Vec::new(), format!("无法读取任务库：{error}")),
            };
            let dashboard = cx.new(|cx| {
                let mut dashboard = Dashboard {
                    database,
                    tasks,
                    recording_info: HashMap::new(),
                    room_url,
                    display_name,
                    message,
                    add_platform: None,
                    capture: None,
                    capture_recording_active: false,
                    recordings: HashMap::new(),
                };
                dashboard.watch_task_store(cx);
                dashboard
            });
            cx.new(|cx| Root::new(dashboard, window, cx))
        })
        .expect("opening Live Archive window");
    });
}
