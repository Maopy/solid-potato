//! Same-origin resources injected into WeChat Channels live pages.
//!
//! Keeping these resources outside the proxy process controller makes the
//! browser-side contract independently testable and lets the sidecar expose
//! stable paths, matching the reference implementation's architecture.

/// This is the same asset root used by the reference interceptor. The proxy
/// serves it locally only because its Go HTTP server is replaced by Rust.
pub const UPSTREAM_PREFIX: &str = "/__assets";

pub fn upstream_assets() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "/inject/components.css",
            include_str!("../assets/wechat_capture/frontend/inject/components.css"),
        ),
        (
            "/public/timeless/0.32.0/timeless.weui.css",
            include_str!(
                "../assets/wechat_capture/frontend/public/timeless/0.32.0/timeless.weui.css"
            ),
        ),
        (
            "/inject/eventbus.js",
            include_str!("../assets/wechat_capture/frontend/inject/eventbus.js"),
        ),
        (
            "/public/dl.utils.js",
            include_str!("../assets/wechat_capture/frontend/public/dl.utils.js"),
        ),
        (
            "/public/dl.sdk.js",
            include_str!("../assets/wechat_capture/frontend/public/dl.sdk.js"),
        ),
        (
            "/inject/env.js",
            include_str!("../assets/wechat_capture/frontend/inject/env.js"),
        ),
        (
            "/inject/utils.js",
            include_str!("../assets/wechat_capture/frontend/inject/utils.js"),
        ),
        (
            "/inject/components.js",
            include_str!("../assets/wechat_capture/frontend/inject/components.js"),
        ),
        (
            "/public/virtual-list-view.js",
            include_str!("../assets/wechat_capture/frontend/public/virtual-list-view.js"),
        ),
        (
            "/inject/download/model.js",
            include_str!("../assets/wechat_capture/frontend/inject/download/model.js"),
        ),
        (
            "/inject/download/view.js",
            include_str!("../assets/wechat_capture/frontend/inject/download/view.js"),
        ),
        (
            "/inject/download/panel.js",
            include_str!("../assets/wechat_capture/frontend/inject/download/panel.js"),
        ),
        (
            "/inject/channels.events.js",
            include_str!(
                "../assets/wechat_capture/pkg/scraper/wxchannels/inject/channels.events.js"
            ),
        ),
        (
            "/inject/channels.env.js",
            include_str!("../assets/wechat_capture/pkg/scraper/wxchannels/inject/channels.env.js"),
        ),
        (
            "/inject/channels.utils.js",
            include_str!(
                "../assets/wechat_capture/pkg/scraper/wxchannels/inject/channels.utils.js"
            ),
        ),
        (
            "/inject/channels.ws.js",
            include_str!("../assets/wechat_capture/pkg/scraper/wxchannels/inject/channels.ws.js"),
        ),
        (
            "/inject/channels.live.js",
            include_str!("../assets/wechat_capture/pkg/scraper/wxchannels/inject/channels.live.js"),
        ),
        (
            "/public/timeless/0.32.0/timeless.umd.min.js",
            include_str!(
                "../assets/wechat_capture/frontend/public/timeless/0.32.0/timeless.umd.min.js"
            ),
        ),
        (
            "/public/timeless/0.32.0/timeless.weui.umd.min.js",
            include_str!(
                "../assets/wechat_capture/frontend/public/timeless/0.32.0/timeless.weui.umd.min.js"
            ),
        ),
        (
            "/public/timeless/0.32.0/timeless.dom.umd.min.js",
            include_str!(
                "../assets/wechat_capture/frontend/public/timeless/0.32.0/timeless.dom.umd.min.js"
            ),
        ),
        (
            "/public/timeless/0.32.0/timeless.web.umd.min.js",
            include_str!(
                "../assets/wechat_capture/frontend/public/timeless/0.32.0/timeless.web.umd.min.js"
            ),
        ),
    ]
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LivePageState {
    pub profile_ready: bool,
    pub live_ready: bool,
    pub join_api_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivePageEvent {
    ApiLoaded,
    FeedProfileLoaded,
    JoinLiveLoaded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingCandidate {
    pub page_url: String,
    pub stream_url: String,
    pub title: String,
    pub anchor_username: Option<String>,
}

impl RecordingCandidate {
    pub fn is_complete(&self) -> bool {
        self.page_url.starts_with("https://channels.weixin.qq.com/")
            && self.stream_url.starts_with("https://")
            && !self.title.trim().is_empty()
    }
}

impl LivePageState {
    pub fn apply(&mut self, event: LivePageEvent) {
        match event {
            LivePageEvent::ApiLoaded => self.join_api_ready = true,
            LivePageEvent::FeedProfileLoaded => self.profile_ready = true,
            LivePageEvent::JoinLiveLoaded => self.live_ready = true,
        }
    }
}

impl LivePageState {
    /// Equivalent to the reference page's `handleLoaded(profile, live)` gate.
    pub fn can_create_recording_candidate(&self) -> bool {
        self.profile_ready && self.live_ready
    }

    pub fn can_resolve_on_click(&self) -> bool {
        self.can_create_recording_candidate() || self.join_api_ready
    }
}

pub fn html_tags(channels_ws_url: &str, downloader_ws_url: &str) -> String {
    let asset = |path: &str| format!("{UPSTREAM_PREFIX}{path}");
    let scripts = [
        "/public/timeless/0.32.0/timeless.umd.min.js",
        "/public/timeless/0.32.0/timeless.weui.umd.min.js",
        "/public/timeless/0.32.0/timeless.dom.umd.min.js",
        "/public/timeless/0.32.0/timeless.web.umd.min.js",
        "/inject/eventbus.js",
        "/public/dl.utils.js",
        "/public/dl.sdk.js",
        "/inject/env.js",
        "/inject/utils.js",
        "/inject/components.js",
        "/public/virtual-list-view.js",
        "/inject/download/model.js",
        "/inject/download/view.js",
        "/inject/download/panel.js",
        "/inject/channels.events.js",
        "/inject/channels.env.js",
        "/inject/channels.utils.js",
        "/inject/channels.ws.js",
    ];
    // Keep the reference interceptor's order: Timeless is loaded first, then
    // its complete runtime configuration, then the dependent inject scripts.
    // A config assignment before Timeless is usually harmless but is not the
    // behavior of the upstream page and has caused loader-order differences
    // in the desktop Channels renderer.
    let mut tags = format!(
        "<link rel=\"stylesheet\" href=\"{}\"><link rel=\"stylesheet\" href=\"{}\">",
        asset("/inject/components.css"),
        asset("/public/timeless/0.32.0/timeless.weui.css")
    );
    for path in &scripts[..4] {
        tags.push_str(&format!(
            "<script crossorigin=\"anonymous\" src=\"{}\"></script>",
            asset(path)
        ));
    }
    tags.push_str(&format!(
        "<script>window.__d_config={{apiOrigin:location.origin,apiProtocol:location.protocol.replace(':',''),apiHost:location.host,assets_base_url:'/__assets',channelsWSURL:{channels_ws_url:?},downloaderWSURL:{downloader_ws_url:?}}};</script>"
    ));
    for path in &scripts[4..] {
        tags.push_str(&format!("<script src=\"{}\"></script>", asset(path)));
    }
    tags
}

/// The upstream interceptor adds the live-page behaviour only for
/// `/web/pages/live`; injecting it into generic Channels HTML can race with
/// unrelated views and leave the button unmounted.
pub fn live_page_tag() -> String {
    format!(
        "<script crossorigin=\"anonymous\" src=\"{UPSTREAM_PREFIX}/inject/channels.live.js\"></script>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_stable_same_origin_resource_paths() {
        let tags = html_tags(
            "ws://127.0.0.1:2022/ws/channels",
            "ws://127.0.0.1:2023/ws/v1/download_task",
        );
        assert!(!tags.contains("/__live_archive/live.js"));
        assert!(tags.contains(UPSTREAM_PREFIX));
        assert!(tags.contains("window.__d_config={apiOrigin:location.origin"));
        assert!(tags.contains("crossorigin=\"anonymous\""));
        assert!(tags.contains("channelsWSURL"));
        assert!(tags.contains("/inject/components.css"));
        assert!(!tags.contains("/inject/channels.live.js"));
        assert!(live_page_tag().contains("/inject/channels.live.js"));
        assert!(tags.contains("/public/timeless/0.32.0/timeless.weui.css"));
        assert!(
            upstream_assets()
                .iter()
                .any(|(path, _)| *path == "/inject/channels.live.js")
        );
        assert!(
            upstream_assets()
                .iter()
                .any(|(path, _)| *path == "/public/timeless/0.32.0/timeless.web.umd.min.js")
        );
        assert!(
            upstream_assets()
                .iter()
                .any(|(path, _)| *path == "/inject/components.css")
        );
    }

    #[test]
    fn live_page_requires_state_or_join_api_before_a_click_can_resolve() {
        let mut state = LivePageState::default();
        assert!(!state.can_resolve_on_click());
        state.join_api_ready = true;
        assert!(state.can_resolve_on_click());
        state.join_api_ready = false;
        state.profile_ready = true;
        state.live_ready = true;
        assert!(state.can_create_recording_candidate());
    }

    #[test]
    fn reference_event_order_enables_a_recording_candidate() {
        let mut state = LivePageState::default();
        state.apply(LivePageEvent::ApiLoaded);
        assert!(state.can_resolve_on_click());
        state.apply(LivePageEvent::FeedProfileLoaded);
        assert!(!state.can_create_recording_candidate());
        state.apply(LivePageEvent::JoinLiveLoaded);
        assert!(state.can_create_recording_candidate());
    }

    #[test]
    fn candidate_requires_allowed_page_https_stream_and_title() {
        let candidate = RecordingCandidate {
            page_url: "https://channels.weixin.qq.com/live-archive/creator".into(),
            stream_url: "https://pull.example.com/live.flv".into(),
            title: "Creator".into(),
            anchor_username: Some("creator".into()),
        };
        assert!(candidate.is_complete());
    }
}
