use crate::moderation_client::{Client, Event};
use crate::text_input::{
    Backspace, Copy, Cut, Delete, End, Home, Left, Paste, Right, SelectAll, SelectLeft,
    SelectRight, TextInput,
};
use chrono::{Local, NaiveDateTime, TimeZone};
use gpui::{
    App, Application, Context, Entity, Render, ScrollHandle, Window, WindowOptions, div, prelude::*,
};
use gpui_component::Root;
use serde_json::{Value, json};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

struct UiAssets;
impl gpui::AssetSource for UiAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let data: Option<&'static [u8]> = match path {
            "icons/ai.svg" => Some(include_bytes!("../assets/ai.svg")),
            "icons/broadcast.svg" => Some(include_bytes!("../assets/broadcast.svg")),
            _ => None,
        };
        Ok(data.map(Cow::Borrowed))
    }
    fn list(&self, _: &str) -> anyhow::Result<Vec<gpui::SharedString>> {
        Ok(Vec::new())
    }
}

struct RoomView {
    search: Entity<TextInput>,
    from: Entity<TextInput>,
    to: Entity<TextInput>,
    message: Entity<TextInput>,
    show_chat: bool,
    show_speech: bool,
    show_range: bool,
    following: bool,
    anchor_ids: HashSet<String>,
    selected: Option<String>,
    reply_open: HashSet<String>,
    reply_errors: HashMap<String, String>,
    expanded: HashSet<i64>,
    last_inputs: [String; 3],
    scroll_offset: gpui::Point<gpui::Pixels>,
}

impl RoomView {
    fn new(cx: &mut Context<Control>) -> Self {
        Self {
            search: cx.new(|cx| TextInput::new(cx, "搜索语音、用户或留言")),
            from: cx.new(|cx| TextInput::new(cx, "开始 YYYY-MM-DD HH:MM:SS")),
            to: cx.new(|cx| TextInput::new(cx, "结束 YYYY-MM-DD HH:MM:SS")),
            message: cx.new(|cx| TextInput::new(cx, "输入要发送到此直播间的文字")),
            show_chat: true,
            show_speech: true,
            show_range: false,
            following: true,
            anchor_ids: HashSet::new(),
            selected: None,
            reply_open: HashSet::new(),
            reply_errors: HashMap::new(),
            expanded: HashSet::new(),
            last_inputs: Default::default(),
            scroll_offset: gpui::point(gpui::px(0.), gpui::px(0.)),
        }
    }
}

struct Control {
    client: Client,
    state: Value,
    timelines: HashMap<String, Value>,
    slots: [Option<String>; 2],
    double: bool,
    downloads: bool,
    chooser: Option<usize>,
    message: String,
    action_pending: bool,
    web_url: Option<String>,
    views: HashMap<String, RoomView>,
    scroll: [ScrollHandle; 2],
}

impl Control {
    fn watch(&mut self, cx: &mut Context<Self>) {
        cx.spawn(|this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
            let mut cx = cx.clone();
            async move {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(300))
                        .await;
                    if this
                        .update(&mut cx, |this, cx| {
                            let mut changed = false;
                            let mut edited = Vec::new();
                            for (id, view) in &mut this.views {
                                let inputs = [
                                    view.search.read(cx).value(),
                                    view.from.read(cx).value(),
                                    view.to.read(cx).value(),
                                ];
                                if inputs != view.last_inputs {
                                    view.last_inputs = inputs;
                                    edited.push(id.clone());
                                }
                            }
                            for id in edited {
                                this.pause(&id);
                                changed = true;
                            }
                            while let Ok(event) = this.client.events.try_recv() {
                                changed = true;
                                match event {
                                    Event::Ready(url) => this.web_url = Some(url),
                                    Event::State(state) => {
                                        this.state = state;
                                        let ids: Vec<String> = this.state["rooms"]
                                            .as_array()
                                            .into_iter()
                                            .flatten()
                                            .filter_map(|room| {
                                                room["id"].as_str().map(str::to_owned)
                                            })
                                            .collect();
                                        for id in ids {
                                            if !this.views.contains_key(&id) {
                                                this.views.insert(id, RoomView::new(cx));
                                            }
                                        }
                                        this.normalize_slots();
                                        if this.message == "正在启动中控…" {
                                            this.message.clear();
                                        }
                                    }
                                    Event::Timeline(id, timeline) => {
                                        if let Some(view) = this.views.get_mut(&id) {
                                            if let Some(entries) = timeline["entries"].as_array() {
                                                let ids: HashSet<_> = entries
                                                    .iter()
                                                    .filter_map(|entry| {
                                                        entry["id"].as_str().map(str::to_owned)
                                                    })
                                                    .collect();
                                                view.anchor_ids.retain(|id| ids.contains(id));
                                                view.reply_open.retain(|id| ids.contains(id));
                                                if view
                                                    .selected
                                                    .as_ref()
                                                    .is_some_and(|id| !ids.contains(id))
                                                {
                                                    view.selected = None;
                                                }
                                            }
                                        }
                                        for index in 0..2 {
                                            if this.slots[index].as_ref() == Some(&id) {
                                                let handle = &this.scroll[index];
                                                if this.views.get(&id).is_some_and(|v| v.following)
                                                    && handle.max_offset().height
                                                        + handle.offset().y
                                                        <= gpui::px(35.)
                                                {
                                                    handle.scroll_to_bottom();
                                                } else if this
                                                    .views
                                                    .get(&id)
                                                    .is_some_and(|v| v.following)
                                                {
                                                    this.pause(&id);
                                                }
                                            }
                                        }
                                        this.timelines.insert(id, timeline);
                                    }
                                    Event::Error(error) => this.message = error,
                                    Event::ActionError { path, body, error } => {
                                        this.action_pending = false;
                                        if path == "reply-suggestions" {
                                            if let (Some(room), Some(entry)) =
                                                (body["id"].as_str(), body["entry_id"].as_str())
                                            {
                                                if let Some(view) = this.views.get_mut(room) {
                                                    view.reply_errors
                                                        .insert(entry.to_owned(), error.clone());
                                                }
                                            }
                                        }
                                        this.message = error;
                                    }
                                    Event::ActionFinished { path, body } => {
                                        this.action_pending = false;
                                        if path == "send-message" {
                                            if let Some(view) = body["id"]
                                                .as_str()
                                                .and_then(|id| this.views.get(id))
                                            {
                                                if view.message.read(cx).value()
                                                    == body["text"].as_str().unwrap_or("")
                                                {
                                                    view.message
                                                        .update(cx, |input, cx| input.clear(cx));
                                                }
                                            }
                                            this.message = "已加入发送队列，等待直播页处理".into();
                                        } else {
                                            this.message.clear();
                                        }
                                    }
                                    Event::Stopped => {
                                        this.web_url = None;
                                        if this.message.is_empty()
                                            || this.message == "正在启动中控…"
                                        {
                                            this.message = "后台已停止".into();
                                        }
                                    }
                                }
                            }
                            if changed {
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        })
        .detach();
    }

    fn normalize_slots(&mut self) {
        let old = self.slots.clone();
        let ids: Vec<String> = self.state["rooms"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["id"].as_str().map(str::to_owned))
            .collect();
        for index in 0..2 {
            if !self.slots[index]
                .as_ref()
                .is_some_and(|id| ids.contains(id))
                || self.slots[index] == self.slots[1 - index]
            {
                self.slots[index] = ids
                    .iter()
                    .find(|id| Some(*id) != self.slots[1 - index].as_ref())
                    .cloned();
            }
        }
        // Keep the remaining room visible in single-panel mode when the
        // first of two rooms closes.
        if self.slots[0].is_none() {
            self.slots.swap(0, 1);
        }
        if old != self.slots {
            self.sync_watch();
        }
    }

    fn sync_watch(&self) {
        self.client.watch_rooms(
            self.slots
                .iter()
                .take(if self.double { 2 } else { 1 })
                .flatten()
                .cloned()
                .collect(),
        );
    }

    fn select_room(&mut self, index: usize, next: &str, cx: &mut Context<Self>) {
        for slot in 0..2 {
            if let Some(view) = self.slots[slot]
                .as_ref()
                .and_then(|id| self.views.get_mut(id))
            {
                view.scroll_offset = self.scroll[slot].offset();
            }
        }
        if self.slots[1 - index].as_deref() == Some(next) {
            self.slots.swap(0, 1);
        } else {
            self.slots[index] = Some(next.to_owned());
        }
        self.chooser = None;
        for slot in 0..2 {
            if let Some(view) = self.slots[slot].as_ref().and_then(|id| self.views.get(id)) {
                if view.following {
                    self.scroll[slot].scroll_to_bottom();
                } else {
                    self.scroll[slot].set_offset(view.scroll_offset);
                }
            }
        }
        self.sync_watch();
        cx.notify();
    }

    fn action(&mut self, path: &str, body: Value) {
        if self.action_pending {
            return;
        }
        self.action_pending = true;
        self.client.action(path, body);
    }

    fn pause(&mut self, id: &str) {
        if let Some(view) = self.views.get_mut(id) {
            if view.following {
                view.following = false;
                view.anchor_ids = self
                    .timelines
                    .get(id)
                    .and_then(|v| v["entries"].as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| entry["id"].as_str().map(str::to_owned))
                    .collect();
            }
        }
    }

    fn panel(&self, index: usize, cx: &mut Context<Self>) -> gpui::Div {
        let Some(id) = self.slots[index].as_ref() else {
            return card().flex_1().child("等待直播间连接");
        };
        let Some(room) = self.state["rooms"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|r| r["id"].as_str() == Some(id))
        else {
            return card();
        };
        let Some(view) = self.views.get(id) else {
            return card().flex_1();
        };
        let control = &room["control"];
        let enabled = control["enabled"].as_bool().unwrap_or(false);
        let connected = control["connected_pages"].as_u64().unwrap_or(0) > 0;
        let speech = &control["speech"];
        let speech_enabled = speech["enabled"].as_bool().unwrap_or(false);
        let speech_pending = control["speech_pending"].as_bool().unwrap_or(false);
        let name = room["label"].as_str().unwrap_or("直播间").to_owned();
        let platform = if room["platform"] == "douyin" {
            "抖音"
        } else {
            "视频号"
        };
        let mut actions = div().flex().flex_wrap().gap_1();
        let room_id = id.clone();
        actions = actions.child(
            button(
                ("moderate", index),
                if enabled {
                    "关闭场控"
                } else {
                    "开启场控"
                },
                cx,
                move |this, cx| {
                    this.action("moderation", json!({"id": room_id, "enabled": !enabled}));
                    cx.notify();
                },
            )
            .when(enabled, |button| {
                button
                    .bg(gpui::rgb(0x245b7a))
                    .text_color(gpui::rgb(0xffffff))
            }),
        );
        if let Some(base) = &self.web_url {
            if let Ok(mut url) = url::Url::parse(base) {
                url.query_pairs_mut().append_pair("settings", id);
                let target = url.to_string();
                actions = actions.child(
                    button(
                        ("room-settings", index),
                        "直播间设置 ↗",
                        cx,
                        move |_, _| {
                            if connected {
                                open_url(&target);
                            }
                        },
                    )
                    .when(!connected, |button| button.opacity(0.4)),
                );
            }
        }
        if control["speech_available"].as_bool().unwrap_or(false) {
            let room_id = id.clone();
            actions = actions.child(
                button(
                    ("speech", index),
                    if speech_pending {
                        "取消开启"
                    } else if speech_enabled {
                        "关闭语音"
                    } else {
                        "开启语音"
                    },
                    cx,
                    move |this, _| {
                        if connected {
                            this.action(
                        "speech",
                        json!({"id": room_id, "enabled": !speech_enabled && !speech_pending}),
                    );
                        }
                    },
                )
                .when(!connected, |button| button.opacity(0.4))
                .when(speech_enabled, |button| {
                    button
                        .bg(gpui::rgb(0x245b7a))
                        .text_color(gpui::rgb(0xffffff))
                }),
            );
        }
        if control["recording"].as_bool().unwrap_or(false) {
            let room_id = id.clone();
            actions = actions.child(
                button(("record", index), "录制直播", cx, move |this, _| {
                    if connected {
                        this.action("record", json!({"id": room_id}));
                    }
                })
                .when(!connected, |button| button.opacity(0.4)),
            );
        }
        if room["platform"] == "douyin" {
            if let Some(url) = room["url"].as_str() {
                let url = url.to_owned();
                actions = actions.child(button(
                    ("open-live", index),
                    "打开直播页",
                    cx,
                    move |_, _| open_url(&url),
                ));
            }
        }
        let mut list = div()
            .id(("timeline", index))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll[index])
            .flex()
            .flex_col()
            .gap_1()
            .py_2();
        let query = view.search.read(cx).value().trim().to_lowercase();
        let from = parse_time(&view.from.read(cx).value());
        let to = parse_time(&view.to.read(cx).value());
        if let Some(entries) = self.timelines.get(id).and_then(|v| v["entries"].as_array()) {
            if entries.is_empty() {
                list = list.child("最近 30 分钟暂无内容");
            }
            let eligible = |entry: &Value| {
                let kind = entry["kind"].as_str().unwrap_or("");
                let text = entry["text"].as_str().unwrap_or("");
                let nickname = if kind == "speech" {
                    "主播"
                } else {
                    entry["nickname"].as_str().unwrap_or("观众")
                };
                let timestamp = if kind == "speech" {
                    entry["end_ms"].as_i64()
                } else {
                    entry["at_ms"].as_i64()
                };
                (view.following || view.anchor_ids.contains(entry["id"].as_str().unwrap_or("")))
                    && (kind != "speech" || view.show_speech)
                    && (kind != "chat" || view.show_chat)
                    && (query.is_empty()
                        || format!("{nickname} {text}").to_lowercase().contains(&query))
                    && !timestamp
                        .is_some_and(|t| from.is_some_and(|v| t < v) || to.is_some_and(|v| t > v))
            };
            let mut groups: HashMap<i64, (usize, bool)> = HashMap::new();
            for entry in entries.iter().filter(|entry| eligible(entry)) {
                let kind = entry["kind"].as_str().unwrap_or("");
                let time = if kind == "speech" {
                    entry["end_ms"].as_i64()
                } else {
                    entry["at_ms"].as_i64()
                }
                .unwrap_or(0)
                    / 1000;
                let group = groups.entry(time).or_insert((0, true));
                group.0 += 1;
                group.1 &= kind == "chat";
            }
            let mut counts: HashMap<i64, usize> = HashMap::new();
            for (entry_index, entry) in entries.iter().enumerate() {
                if !eligible(entry) {
                    continue;
                }
                let kind = entry["kind"].as_str().unwrap_or("");
                let text = entry["text"].as_str().unwrap_or("").to_owned();
                let nickname = if kind == "speech" {
                    "主播"
                } else {
                    entry["nickname"].as_str().unwrap_or("观众")
                };
                let timestamp = if kind == "speech" {
                    entry["end_ms"].as_i64()
                } else {
                    entry["at_ms"].as_i64()
                };
                let second = timestamp.unwrap_or(0) / 1000;
                let group = groups.get(&second).copied().unwrap_or((0, false));
                let count = counts.entry(second).or_default();
                *count += 1;
                if group.0 > 3 && group.1 && *count == 4 {
                    let room_id = id.clone();
                    list = list.child(button(
                        ("expand", index * 100000 + entry_index),
                        if view.expanded.contains(&second) {
                            "收起留言"
                        } else {
                            "展开其他留言"
                        },
                        cx,
                        move |this, cx| {
                            this.pause(&room_id);
                            if let Some(view) = this.views.get_mut(&room_id) {
                                if !view.expanded.remove(&second) {
                                    view.expanded.insert(second);
                                }
                            }
                            cx.notify();
                        },
                    ));
                }
                if group.0 > 3 && group.1 && *count > 3 && !view.expanded.contains(&second) {
                    continue;
                }
                let time = timestamp
                    .and_then(|t| Local.timestamp_millis_opt(t).single())
                    .map(|t| t.format("%H:%M:%S").to_string())
                    .unwrap_or_default();
                let selected = view.selected.as_deref() == entry["id"].as_str();
                let related = view
                    .selected
                    .as_ref()
                    .and_then(|id| entries.iter().find(|e| e["id"].as_str() == Some(id)))
                    .is_some_and(|selected| entries_related(entry, selected));
                let mut header = div().flex().gap_2().items_center().child({
                    let room_id = id.clone();
                    let entry_id = entry["id"].as_str().unwrap_or("").to_owned();
                    div()
                        .id(("entry", index * 100000 + entry_index))
                        .flex_1()
                        .min_w_0()
                        .cursor_pointer()
                        .flex()
                        .gap_3()
                        .items_center()
                        .child(
                            div()
                                .w_16()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(gpui::rgb(0x8a9aa9))
                                .child(time),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .flex()
                                .gap_2()
                                .items_center()
                                .children(
                                    entry["fan_level"]
                                        .as_u64()
                                        .filter(|v| (1..=100000).contains(v))
                                        .map(|level| {
                                            div()
                                                .px_2()
                                                .rounded_md()
                                                .bg(gpui::rgb(0xe9a64b))
                                                .text_xs()
                                                .font_weight(gpui::FontWeight::BOLD)
                                                .text_color(gpui::rgb(0x40270f))
                                                .child(level.to_string())
                                        }),
                                )
                                .child(
                                    div()
                                        .text_color(gpui::rgb(0x73d0ff))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(nickname.to_owned()),
                                )
                                .child(div().flex_1().min_w_0().child(text)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pause(&room_id);
                            if let Some(view) = this.views.get_mut(&room_id) {
                                view.selected = if view.selected.as_ref() == Some(&entry_id) {
                                    None
                                } else {
                                    Some(entry_id.clone())
                                };
                            }
                            cx.notify();
                        }))
                });
                let mut details = div().flex().flex_col().gap_1();
                if kind == "chat" {
                    if let Some(score) = entry["rating"]["score"].as_i64() {
                        header = header.child(
                            div()
                                .text_xs()
                                .text_color(gpui::rgb(0xa7dfff))
                                .child(format!("{score}分")),
                        );
                    } else if let Some(status) = entry["rating"]["state"].as_str() {
                        header = header.child(
                            div()
                                .text_xs()
                                .text_color(gpui::rgb(match status {
                                    "error" => 0xf07178,
                                    "busy" => 0xffcc66,
                                    _ => 0x8c9baa,
                                }))
                                .child(match status {
                                    "pending" => "  评分中",
                                    "busy" => "  待重试",
                                    "error" => "  评分不可用",
                                    _ => "",
                                }),
                        );
                    }
                    let entry_id = entry["id"].as_str().unwrap_or("");
                    let reply_open = view.reply_open.contains(entry_id);
                    if reply_open && view.reply_errors.get(entry_id).is_some() {
                        details = details.child(
                            div()
                                .text_color(gpui::rgb(0xf07178))
                                .child(format!("建议暂不可用：{}", view.reply_errors[entry_id])),
                        );
                        let room_id = id.clone();
                        let retry_id = entry_id.to_owned();
                        details = details.child(div().flex().child(button(
                            ("retry-reply", index * 100000 + entry_index),
                            "重试",
                            cx,
                            move |this, cx| {
                                if this.action_pending {
                                    return;
                                }
                                if let Some(view) = this.views.get_mut(&room_id) {
                                    view.reply_errors.remove(&retry_id);
                                }
                                this.action(
                                    "reply-suggestions",
                                    json!({"id":room_id,"entry_id":retry_id}),
                                );
                                cx.notify();
                            },
                        )));
                    } else if reply_open
                        && (entry["reply"].is_null() || entry["reply"]["state"] == "pending")
                    {
                        details = details.child("回复建议生成中");
                    }
                    if reply_open && entry["reply"]["state"] == "error" {
                        details = details.child(
                            div().text_color(gpui::rgb(0xf07178)).child(
                                entry["reply"]["error"]
                                    .as_str()
                                    .unwrap_or("建议暂不可用，请重试")
                                    .to_owned(),
                            ),
                        );
                    }
                    if let Some(suggestions) = entry["reply"]["suggestions"]
                        .as_array()
                        .filter(|_| reply_open)
                    {
                        for (suggestion_index, suggestion) in suggestions.iter().enumerate() {
                            if let Some(text) = suggestion["text"].as_str() {
                                let text = text.to_owned();
                                let copied = text.clone();
                                let direction = suggestion["direction"]
                                    .as_str()
                                    .unwrap_or("回复建议")
                                    .to_owned();
                                details = details.child(
                                    div()
                                        .flex()
                                        .gap_2()
                                        .items_center()
                                        .text_sm()
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .flex()
                                                .flex_col()
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(gpui::rgb(0x73d0ff))
                                                        .child(direction),
                                                )
                                                .child(text),
                                        )
                                        .child(button(
                                            (
                                                "copy-reply",
                                                index * 1000000
                                                    + entry_index * 10
                                                    + suggestion_index,
                                            ),
                                            "复制",
                                            cx,
                                            move |_, cx| {
                                                cx.write_to_clipboard(
                                                    gpui::ClipboardItem::new_string(copied.clone()),
                                                );
                                            },
                                        )),
                                );
                            }
                        }
                    }
                    let room_id = id.clone();
                    let entry_id = entry["id"].as_str().unwrap_or("").to_owned();
                    header = header.child(icon_button(
                        ("reply", index * 100000 + entry_index),
                        reply_open,
                        cx,
                        move |this, cx| {
                            if this.action_pending {
                                return;
                            }
                            this.pause(&room_id);
                            let open = if let Some(view) = this.views.get_mut(&room_id) {
                                if view.reply_open.remove(&entry_id) {
                                    false
                                } else {
                                    view.reply_open.insert(entry_id.clone());
                                    view.reply_errors.remove(&entry_id);
                                    true
                                }
                            } else {
                                false
                            };
                            if open {
                                this.action(
                                    "reply-suggestions",
                                    json!({"id": room_id, "entry_id": entry_id}),
                                );
                            }
                            cx.notify();
                        },
                    ));
                }
                let row = div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(gpui::rgb(if selected {
                        0x73d0ff
                    } else if related {
                        0x35566a
                    } else {
                        0x111720
                    }))
                    .bg(gpui::rgb(if selected {
                        0x203b51
                    } else if related {
                        0x192a36
                    } else if kind == "speech" {
                        0x1c2935
                    } else {
                        0x111720
                    }))
                    .child(header)
                    .child(details.when(reply_open_for(entry, view), |details| {
                        details
                            .mt_1()
                            .ml(gpui::px(76.))
                            .p_2()
                            .rounded_md()
                            .bg(gpui::rgb(0x192735))
                            .border_1()
                            .border_color(gpui::rgb(0x344b5a))
                    }));
                list = list.child(row);
            }
        }
        let speech_status = if speech_pending {
            "等待直播页开启"
        } else {
            match speech["state"].as_str().unwrap_or("off") {
                "starting" => "模型准备中",
                "listening" => "等待说话",
                "transcribing" => "识别中",
                "error" => "识别异常",
                "disconnected" => "连接中断",
                _ => "未开启",
            }
        };
        let script_warning = control["client_outdated"].as_bool().unwrap_or(false);
        let mut selector = div().flex().flex_col().gap_1();
        if self.chooser == Some(index) {
            for (room_index, candidate) in self.state["rooms"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                let Some(candidate_id) = candidate["id"].as_str() else {
                    continue;
                };
                let candidate_id = candidate_id.to_owned();
                let label = format!(
                    "{} · {} · {}{}",
                    candidate["label"].as_str().unwrap_or("直播间"),
                    if candidate["platform"] == "douyin" {
                        "抖音"
                    } else {
                        "视频号"
                    },
                    candidate_id,
                    if candidate["control"]["connected_pages"]
                        .as_u64()
                        .unwrap_or(0)
                        == 0
                    {
                        " · 未连接"
                    } else {
                        ""
                    }
                );
                selector = selector.child(button(
                    ("choose-room", index * 100 + room_index),
                    &label,
                    cx,
                    move |this, cx| this.select_room(index, &candidate_id, cx),
                ));
            }
        }
        card()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(gpui::rgb(0x29313b))
                            .text_xs()
                            .text_color(gpui::rgb(0x73d0ff))
                            .child(format!("{:02}", index + 1)),
                    )
                    .child(button(
                        ("room", index),
                        &format!("{name} ▾"),
                        cx,
                        move |this, cx| {
                            this.chooser = if this.chooser == Some(index) {
                                None
                            } else {
                                Some(index)
                            };
                            cx.notify();
                        },
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(gpui::rgb(0x9baab8))
                            .child(platform),
                    )
                    .child(actions),
            )
            .child(selector)
            .child(
                div()
                    .text_xs()
                    .text_color(gpui::rgb(0x8a9aa9))
                    .child(format!(
                        "{} · 语音：{speech_status}",
                        if connected {
                            "● 已连接"
                        } else {
                            "○ 未连接"
                        }
                    )),
            )
            .children(control["speech_feedback"].as_str().map(|s| {
                div()
                    .text_sm()
                    .text_color(gpui::rgb(0xffcc66))
                    .child(s.to_owned())
            }))
            .children(control["settings_error"].as_str().map(|s| {
                div()
                    .text_sm()
                    .text_color(gpui::rgb(0xffcc66))
                    .child(s.to_owned())
            }))
            .children(speech["error"].as_str().map(|s| {
                div()
                    .text_sm()
                    .text_color(gpui::rgb(0xf07178))
                    .child(s.to_owned())
            }))
            .children(control["blocked"].as_bool().unwrap_or(false).then(|| {
                div()
                    .text_sm()
                    .text_color(gpui::rgb(0xffcc66))
                    .child("发送未确认，后续发送已暂停")
            }))
            .children(
                control["recording_result"]["message"]
                    .as_str()
                    .map(|s| div().text_sm().child(s.to_owned())),
            )
            .children(script_warning.then(|| {
                div()
                    .text_xs()
                    .text_color(gpui::rgb(0xffcc66))
                    .child("抖音脚本版本过旧，请在下载任务中更新后刷新直播页。")
            }))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(view.search.clone()))
                    .child({
                        let room_id = id.clone();
                        button(
                            ("filter-chat", index),
                            if view.show_chat {
                                "✓ 公屏"
                            } else {
                                "公屏"
                            },
                            cx,
                            move |this, cx| {
                                this.pause(&room_id);
                                if let Some(view) = this.views.get_mut(&room_id) {
                                    view.show_chat = !view.show_chat;
                                }
                                cx.notify();
                            },
                        )
                    })
                    .child({
                        let room_id = id.clone();
                        button(
                            ("filter-speech", index),
                            if view.show_speech {
                                "✓ 语音"
                            } else {
                                "语音"
                            },
                            cx,
                            move |this, cx| {
                                this.pause(&room_id);
                                if let Some(view) = this.views.get_mut(&room_id) {
                                    view.show_speech = !view.show_speech;
                                }
                                cx.notify();
                            },
                        )
                    })
                    .child({
                        let room_id = id.clone();
                        button(
                            ("range-toggle", index),
                            "时间范围",
                            cx,
                            move |this, cx| {
                                if let Some(view) = this.views.get_mut(&room_id) {
                                    view.show_range = !view.show_range;
                                }
                                cx.notify();
                            },
                        )
                    })
                    .child({
                        let room_id = id.clone();
                        button(
                            ("follow", index),
                            if view.following {
                                "● 实时"
                            } else {
                                "回到实时"
                            },
                            cx,
                            move |this, cx| {
                                if let Some(view) = this.views.get_mut(&room_id) {
                                    view.following = true;
                                    view.selected = None;
                                    view.anchor_ids.clear();
                                    view.search.update(cx, |v, cx| v.clear(cx));
                                    view.from.update(cx, |v, cx| v.clear(cx));
                                    view.to.update(cx, |v, cx| v.clear(cx));
                                    view.last_inputs = Default::default();
                                }
                                this.scroll[index].scroll_to_bottom();
                                cx.notify();
                            },
                        )
                    }),
            )
            .children(view.show_range.then(|| {
                div()
                    .flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(view.from.clone()))
                    .child(div().flex_1().min_w_0().child(view.to.clone()))
                    .child({
                        let room_id = id.clone();
                        button(
                            ("clear-range", index),
                            "清除时间",
                            cx,
                            move |this, cx| {
                                if let Some(view) = this.views.get(&room_id) {
                                    view.from.update(cx, |v, cx| v.clear(cx));
                                    view.to.update(cx, |v, cx| v.clear(cx));
                                }
                                cx.notify();
                            },
                        )
                    })
            }))
            .child(list)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(view.message.clone()))
                    .child({
                        let room_id = id.clone();
                        button(("send-message", index), "发送", cx, move |this, cx| {
                            if let Some(view) = this.views.get(&room_id) {
                                let text = view.message.read(cx).value();
                                this.action("send-message", json!({"id":room_id,"text":text}));
                                cx.notify();
                            }
                        })
                    }),
            )
            .child(div().text_xs().text_color(gpui::rgb(0x9baab8)).child(
                match control["manual_message"]["state"].as_str() {
                    Some("queued") => "等待直播页输入框就绪",
                    Some("issued") => "正在交给直播页",
                    Some("submitted") => "已提交到直播页，平台显示可能有延迟",
                    Some("unknown") => "发送结果未确认，请先检查直播页",
                    Some("rejected" | "expired" | "cancelled") => {
                        "未能发送，请检查直播页状态后重试"
                    }
                    _ => "发送到本直播间，场控关闭时也可手动发送",
                },
            ))
    }

    fn downloads_view(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let mut list = card()
            .flex_1()
            .id("downloads")
            .overflow_y_scroll()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_lg()
                    .child("下载任务"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(gpui::rgb(0x9baab8))
                    .child("管理直播录制，以及抖音用户脚本的安装与更新。"),
            );
        if let Some(url) = self.state["userscript_install_url"].as_str() {
            let url = url.to_owned();
            list = list.child(button(
                "install",
                "安装 / 更新抖音脚本",
                cx,
                move |_, _| open_url(&url),
            ));
        }
        let tasks = self.state["downloads"].as_array();
        if tasks.is_none_or(|tasks| tasks.is_empty()) {
            return list.child(
                div()
                    .mt_4()
                    .p_4()
                    .rounded_md()
                    .bg(gpui::rgb(0x17202a))
                    .text_color(gpui::rgb(0x9baab8))
                    .child("暂无录制任务。进入并播放直播后，可在直播间点击“录制直播”。"),
            );
        }
        for (index, task) in tasks.unwrap().iter().enumerate() {
            let id = task["id"].as_str().unwrap_or("").to_owned();
            let active = matches!(task["status"].as_str(), Some("waiting" | "downloading"));
            let status = match task["status"].as_str().unwrap_or("") {
                "waiting" => "等待启动",
                "downloading" => "录制中",
                "paused" => "已暂停",
                "finished" => "已完成",
                "failed" => "失败",
                other => other,
            };
            let action = if active { "pause" } else { "start" };
            let action_id = id.clone();
            list = list.child(
                div()
                    .flex()
                    .gap_3()
                    .items_center()
                    .p_3()
                    .rounded_md()
                    .bg(gpui::rgb(0x17202a))
                    .child(format!(
                        "{} · {} {}",
                        task["name"].as_str().unwrap_or("任务"),
                        status,
                        task["error"].as_str().unwrap_or("")
                    ))
                    .child(button(
                        ("download", index),
                        if active { "暂停" } else { "继续" },
                        cx,
                        move |this, _| {
                            this.action("download", json!({"id": action_id, "action": action}))
                        },
                    ))
                    .child(button(("remove", index), "移除", cx, move |this, _| {
                        this.action("download", json!({"id": id, "action": "delete"}))
                    })),
            );
        }
        list
    }
}

impl Render for Control {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1()
            .bg(gpui::rgb(0x0b0e14))
            .text_color(gpui::rgb(0xe6e1cf));
        root = root.child(
            div()
                .flex()
                .gap_2()
                .items_center()
                .px_2()
                .py_1()
                .rounded_md()
                .bg(gpui::rgb(0x111720))
                .border_1()
                .border_color(gpui::rgb(0x293544))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(gpui::px(23.))
                        .rounded_md()
                        .bg(gpui::rgb(0x73d0ff))
                        .text_color(gpui::rgb(0xffffff))
                        .child(
                            gpui::svg()
                                .path("icons/broadcast.svg")
                                .size(gpui::px(17.))
                                .text_color(gpui::rgb(0xffffff)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().font_weight(gpui::FontWeight::BOLD).child("直播助手"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(gpui::rgb(0x8a9aa9))
                                .child("中控台 / 实时内容"),
                        ),
                )
                .child(div().w_2())
                .child(
                    button("rooms", "直播间", cx, |this, cx| {
                        this.downloads = false;
                        cx.notify();
                    })
                    .when(!self.downloads, |button| {
                        button
                            .bg(gpui::rgb(0x203748))
                            .text_color(gpui::rgb(0xa7dfff))
                    }),
                )
                .child(
                    button("downloads-tab", "下载任务", cx, |this, cx| {
                        this.downloads = true;
                        cx.notify();
                    })
                    .when(self.downloads, |button| {
                        button
                            .bg(gpui::rgb(0x203748))
                            .text_color(gpui::rgb(0xa7dfff))
                    }),
                )
                .child(div().flex_1())
                .children(self.web_url.as_ref().map(|url| {
                    let url = url.clone();
                    button("open-web", "打开 Web 中控 ↗", cx, move |_, _| {
                        open_url(&url)
                    })
                })),
        );
        if !self.message.is_empty() {
            root = root.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(gpui::rgb(if self.message == "正在启动中控…" {
                        0x203748
                    } else {
                        0x45252b
                    }))
                    .text_sm()
                    .text_color(gpui::rgb(if self.message == "正在启动中控…" {
                        0x73d0ff
                    } else {
                        0xf07178
                    }))
                    .child(self.message.clone()),
            );
        }
        if self.downloads {
            root.child(self.downloads_view(cx))
        } else {
            let connected = self.state["rooms"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|room| room["control"]["connected_pages"].as_u64().unwrap_or(0) > 0)
                .count();
            root = root.child(
                div()
                    .flex()
                    .items_center()
                    .px_1()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("直播间"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(gpui::rgb(0x8a9aa9))
                            .child(format!("{connected} 个已连接  ·  每个窗口可独立切换")),
                    )
                    .child(div().flex_1())
                    .child(
                        button("single", "单直播间", cx, |this, cx| {
                            this.double = false;
                            this.sync_watch();
                            cx.notify();
                        })
                        .when(!self.double, |button| {
                            button
                                .bg(gpui::rgb(0x20374a))
                                .text_color(gpui::rgb(0xa7dfff))
                        }),
                    )
                    .child(
                        button("double", "双直播间", cx, |this, cx| {
                            this.double = true;
                            this.sync_watch();
                            cx.notify();
                        })
                        .when(self.double, |button| {
                            button
                                .bg(gpui::rgb(0x20374a))
                                .text_color(gpui::rgb(0xa7dfff))
                        }),
                    ),
            );
            let mut panels = div()
                .flex()
                .flex_1()
                .min_h_0()
                .gap_2()
                .child(self.panel(0, cx));
            if self.double && self.slots[1].is_some() {
                panels = panels.child(self.panel(1, cx));
            }
            root.child(panels).child(
                div()
                    .text_xs()
                    .text_color(gpui::rgb(0x8a9aa9))
                    .child("语音按说完时间排列，可能有播放延迟  ·  每个直播间保留最近 30 分钟"),
            )
        }
    }
}

fn entries_related(entry: &Value, selected: &Value) -> bool {
    let kinds = (entry["kind"].as_str(), selected["kind"].as_str());
    if !matches!(
        kinds,
        (Some("chat"), Some("speech")) | (Some("speech"), Some("chat"))
    ) {
        return false;
    }
    entry["at_ms"]
        .as_i64()
        .zip(entry["end_ms"].as_i64())
        .zip(selected["at_ms"].as_i64().zip(selected["end_ms"].as_i64()))
        .is_some_and(|((start, end), (selected_start, selected_end))| {
            start / 1000 <= selected_end / 1000 && end / 1000 >= selected_start / 1000
        })
}

fn reply_open_for(entry: &Value, view: &RoomView) -> bool {
    entry["id"]
        .as_str()
        .is_some_and(|id| view.reply_open.contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_speech_and_chat_can_be_related() {
        let chat = json!({"kind":"chat","at_ms":1234,"end_ms":1234});
        let same_second_chat = json!({"kind":"chat","at_ms":1450,"end_ms":1450});
        let speech = json!({"kind":"speech","at_ms":1000,"end_ms":2000});
        assert!(!entries_related(&chat, &same_second_chat));
        assert!(entries_related(&chat, &speech));
        assert!(entries_related(&speech, &chat));
    }
}

fn card() -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_lg()
        .border_1()
        .border_color(gpui::rgb(0x293544))
        .bg(gpui::rgb(0x111720))
}
fn parse_time(value: &str) -> Option<i64> {
    NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%d %H:%M"))
        .ok()
        .and_then(|time| Local.from_local_datetime(&time).single())
        .map(|time| time.timestamp_millis())
}
fn button(
    id: impl Into<gpui::ElementId>,
    label: &str,
    cx: &mut Context<Control>,
    f: impl Fn(&mut Control, &mut Context<Control>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_2()
        .py(gpui::px(3.))
        .rounded_md()
        .text_xs()
        .text_color(gpui::rgb(0xd2d6dc))
        .border_1()
        .border_color(gpui::rgb(0x344354))
        .bg(gpui::rgb(0x18212c))
        .hover(|style| style.bg(gpui::rgb(0x283442)))
        .cursor_pointer()
        .child(label.to_owned())
        .on_click(cx.listener(move |this, _, _, cx| f(this, cx)))
}
fn icon_button(
    id: impl Into<gpui::ElementId>,
    active: bool,
    cx: &mut Context<Control>,
    f: impl Fn(&mut Control, &mut Context<Control>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(gpui::px(22.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .when(active, |s| s.bg(gpui::rgb(0x20374a)))
        .when(active, |s| s.border_1().border_color(gpui::rgb(0x73d0ff)))
        .hover(|s| s.bg(gpui::rgb(0x20374a)))
        .cursor_pointer()
        .tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(if active {
                "收起回复建议"
            } else {
                "生成回复建议"
            })
            .build(window, cx)
        })
        .child(
            gpui::svg()
                .path("icons/ai.svg")
                .size(gpui::px(17.))
                .text_color(gpui::rgb(0x73d0ff)),
        )
        .on_click(cx.listener(move |this, _, _, cx| f(this, cx)))
}
fn open_url(url: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("rundll32.exe")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
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
pub fn run() {
    #[cfg(windows)]
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            let mut paths = vec![directory.join("tools")];
            if let Some(root) = directory.parent().and_then(|p| p.parent()) {
                paths.push(root.join(".venv/Scripts"));
            }
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            if let Ok(path) = std::env::join_paths(paths) {
                // This runs before GPUI or the controller creates any threads.
                unsafe { std::env::set_var("PATH", path) };
            }
        }
    }
    let config = config_path();
    Application::new()
        .with_assets(UiAssets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            cx.bind_keys([
                gpui::KeyBinding::new("backspace", Backspace, None),
                gpui::KeyBinding::new("delete", Delete, None),
                gpui::KeyBinding::new("left", Left, None),
                gpui::KeyBinding::new("right", Right, None),
                gpui::KeyBinding::new("shift-left", SelectLeft, None),
                gpui::KeyBinding::new("shift-right", SelectRight, None),
                gpui::KeyBinding::new("ctrl-a", SelectAll, None),
                gpui::KeyBinding::new("ctrl-v", Paste, None),
                gpui::KeyBinding::new("ctrl-c", Copy, None),
                gpui::KeyBinding::new("ctrl-x", Cut, None),
                gpui::KeyBinding::new("home", Home, None),
                gpui::KeyBinding::new("end", End, None),
            ]);
            cx.on_window_closed(|cx| cx.quit()).detach();
            cx.open_window(WindowOptions::default(), move |window, cx| {
                window.set_window_title("直播助手 · 中控台");
                let client = Client::start(config);
                let control = cx.new(|cx| {
                    let mut control = Control {
                        client,
                        state: json!({}),
                        timelines: HashMap::new(),
                        slots: [None, None],
                        double: true,
                        downloads: false,
                        chooser: None,
                        message: "正在启动中控…".into(),
                        action_pending: false,
                        web_url: None,
                        views: HashMap::new(),
                        scroll: [ScrollHandle::new(), ScrollHandle::new()],
                    };
                    control.watch(cx);
                    control
                });
                cx.new(|cx| Root::new(control, window, cx))
            })
            .expect("opening control window");
        });
}
