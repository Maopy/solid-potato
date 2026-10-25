//! Restricted `mitmdump` sidecar for WeChat Channels capture.
//!
//! The sidecar is intentionally narrow: loopback listener and metadata-only
//! handoff for HTTPS HLS/FLV requests observed during one user-started capture
//! session. It is not a general-purpose traffic recorder.
use crate::{capture_service::CapturePermit, wechat_inject, wechat_proxy::ProxyAddress};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use std::{
    fs,
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    process::Stdio,
};
use tokio::process::{Child, Command};
use tokio::time::{Duration, sleep};
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
pub struct MitmproxySidecarConfig {
    pub executable: PathBuf,
    pub working_directory: PathBuf,
    pub listen_port: u16,
    pub capture_endpoint: Url,
    pub channels_ws_url: String,
    pub downloader_ws_url: String,
    pub permit: CapturePermit,
}

impl MitmproxySidecarConfig {
    pub fn validate(&self) -> Result<()> {
        if self.listen_port == 0 {
            bail!("sidecar port must be greater than zero");
        }
        if self.capture_endpoint.scheme() != "http"
            || self.capture_endpoint.host_str() != Some("127.0.0.1")
        {
            bail!("capture endpoint must be an http://127.0.0.1 URL");
        }
        Ok(())
    }
}

pub struct MitmproxySidecar {
    config: MitmproxySidecarConfig,
    addon_path: PathBuf,
    child: Option<Child>,
}

#[async_trait]
pub trait CaptureSidecar: Send {
    fn proxy_address(&self) -> ProxyAddress;
    async fn start(&mut self) -> Result<()>;
    async fn stop(&mut self) -> Result<()>;
}

impl MitmproxySidecar {
    pub fn new(config: MitmproxySidecarConfig) -> Result<Self> {
        config.validate()?;
        let addon_path = config.working_directory.join("live_archive_capture.py");
        Ok(Self {
            config,
            addon_path,
            child: None,
        })
    }

    /// Generates the sidecar addon with owner-only filesystem permissions where
    /// supported. The permit expires with this capture session.
    pub fn prepare(&self) -> Result<()> {
        fs::create_dir_all(&self.config.working_directory)
            .context("creating mitmproxy work directory")?;
        fs::write(&self.addon_path, render_addon(&self.config))
            .context("writing mitmproxy capture addon")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.addon_path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub async fn start(&mut self) -> Result<()> {
        if self.child.is_some() {
            return Ok(());
        }
        // Do this before generating the addon or changing the system proxy.
        // Otherwise an old sidecar can make the readiness connection below
        // succeed while the newly spawned mitmdump exits with EADDRINUSE.
        let address = SocketAddr::from(([127, 0, 0, 1], self.config.listen_port));
        let port_probe = TcpListener::bind(address).with_context(|| {
            format!(
                "WeChat capture proxy port {} is already in use; stop the existing video-channel listener before starting another one",
                self.config.listen_port
            )
        })?;
        drop(port_probe);
        self.prepare()?;
        let child = Command::new(&self.config.executable)
            .args(["-q", "--listen-host", "127.0.0.1", "--listen-port"])
            .arg(self.config.listen_port.to_string())
            .args([
                "--set",
                "block_global=false",
                "--set",
                "connection_strategy=lazy",
                // Keep known live CDN TLS end-to-end between WeChat and the
                // CDN. Current desktop clients may pin these connections;
                // attempting to inspect their media traffic prevents playback.
                "--set",
                "ignore_hosts=.*(wxlivecdn\\.com|wxqcloud\\.qq\\.com|bytefcdnrd\\.com).*",
                // A cached JavaScript bundle would bypass the joinLive hook.
                "--set",
                "anticache=true",
                // Hide per-request proxy access logs. The addon emits a small
                // set of capture-specific status lines instead.
                "--set",
                "termlog_verbosity=warn",
                "--set",
            ])
            .arg(format!(
                "confdir={}",
                self.config.working_directory.display()
            ))
            .args(["-s"])
            .arg(&self.addon_path)
            .current_dir(&self.config.working_directory)
            .stdin(Stdio::null())
            // `termlog_verbosity=warn` suppresses per-request access logs;
            // retain stdout because mitmproxy emits addon diagnostics there.
            .stdout(Stdio::inherit())
            // Keep warnings/errors visible as well.
            .stderr(Stdio::inherit())
            .spawn()
            .context("starting mitmdump sidecar; install mitmproxy first")?;
        self.child = Some(child);
        self.wait_until_ready().await?;
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<()> {
        if let Some(child) = self.child.as_mut() {
            crate::child_process::stop_proxy_child(child).await?;
        }
        self.child.take();
        if self.addon_path.exists() {
            fs::remove_file(&self.addon_path).context("removing temporary capture addon")?;
        }
        Ok(())
    }

    async fn wait_until_ready(&mut self) -> Result<()> {
        let certificate = self.config.working_directory.join("mitmproxy-ca-cert.pem");
        let address = SocketAddr::from(([127, 0, 0, 1], self.config.listen_port));
        for _ in 0..50 {
            // A certificate from an earlier session can already be on disk,
            // so it alone is not evidence that this newly spawned proxy
            // successfully bound its port.  Confirm both prerequisites.
            if certificate.exists()
                && TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok()
            {
                return Ok(());
            }
            if self
                .child
                .as_mut()
                .is_some_and(|child| child.try_wait().ok().flatten().is_some())
            {
                bail!("mitmdump exited before the WeChat capture proxy became ready");
            }
            sleep(Duration::from_millis(100)).await;
        }
        bail!("mitmdump did not start the WeChat capture proxy within 5 seconds")
    }
}

#[async_trait]
impl CaptureSidecar for MitmproxySidecar {
    fn proxy_address(&self) -> ProxyAddress {
        ProxyAddress {
            host: "127.0.0.1".into(),
            port: self.config.listen_port,
        }
    }
    async fn start(&mut self) -> Result<()> {
        MitmproxySidecar::start(self).await
    }
    async fn stop(&mut self) -> Result<()> {
        MitmproxySidecar::stop(self).await
    }
}

impl Drop for MitmproxySidecar {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            crate::child_process::drop_proxy_child(child);
        }
        let _ = fs::remove_file(&self.addon_path);
    }
}

/// Encodes generated, embedded frontend resources without introducing escape
/// sequences that the Python parser could interpret before `json.loads` sees
/// them. Keeping this local avoids adding a runtime dependency just for addon
/// generation.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);

    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 0b11) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            ALPHABET[(((second & 0b1111) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            ALPHABET[(third & 0b11_1111) as usize] as char
        } else {
            '='
        });
    }
    output
}

pub fn render_addon(config: &MitmproxySidecarConfig) -> String {
    let endpoint = config.capture_endpoint.as_str();
    let token = config.permit.as_bearer();
    // Keep this separate from the loopback permit: it is a harmless, unique
    // static-asset cache key for this one proxy session.
    let cache_bust = Uuid::new_v4().to_string();
    // Do not interpolate JSON directly into the generated Python source.
    // JavaScript bundles legitimately contain sequences such as `\\u`, which
    // Python would interpret while parsing its string literal (and reject if
    // they are not a complete Unicode escape). Base64 is ASCII-only, so the
    // generated addon reconstructs the exact reference assets safely.
    let upstream_assets = base64_encode(
        serde_json::to_string(&wechat_inject::upstream_assets())
            .expect("serializing embedded upstream frontend assets")
            .as_bytes(),
    );
    let injected_tags =
        wechat_inject::html_tags(&config.channels_ws_url, &config.downloader_ws_url);
    let live_page_tag = wechat_inject::live_page_tag();
    let recorder_adapter = r#"(() => {
  const endpoint = "/__live_archive/capture";
  const report = stage => { try { navigator.sendBeacon("/__live_archive/capture-status?stage=" + stage, ""); } catch (_) {} };
  const findLive = (value, seen = new Set()) => {
    if (!value || typeof value !== "object" || seen.has(value)) return null;
    seen.add(value);
    if (value.liveSdkInfo && value.liveSdkInfo.liveCdnUrl) return value;
    for (const child of Object.values(value)) { const found = findLive(child, seen); if (found) return found; }
    return null;
  };
  // Desktop Channels sometimes resolves `joinLive` in an isolated renderer:
  // the reference event reaches the page, but its CDN field is omitted. Once
  // the user has actually played the live room, Resource Timing still holds
  // the page-visible HLS/FLV URL. This is deliberately consulted only from a
  // user click, never as background traffic collection.
  const playedStream = () => {
    try {
      const entries = performance.getEntriesByType("resource");
      for (let index = entries.length - 1; index >= 0; index -= 1) {
        const name = String(entries[index] && entries[index].name || "");
        if (/^https:\/\//i.test(name) && (/(?:\.m3u8|\.flv)(?:[?#]|$)/i.test(name) || /(?:wxlivecdn|liveplay|livecdn)/i.test(name))) return name;
      }
    } catch (_) {}
    return "";
  };
  const resolveLive = async (profile, live) => {
    const resolved = findLive(live);
    if (resolved && resolved.liveSdkInfo && resolved.liveSdkInfo.liveCdnUrl) return resolved;
    const findField = (value, names, seen = new Set()) => {
      if (!value || typeof value !== "object" || seen.has(value)) return "";
      seen.add(value);
      for (const name of names) if (typeof value[name] === "string" && value[name]) return value[name];
      for (const child of Object.values(value)) { const found = findField(child, names, seen); if (found) return found; }
      return "";
    };
    const state = [profile, live, window.__liveArchiveLatestLiveProfile];
    const payload = {
      clientStatus: { videoDecoderSupportMask: 1 },
      finderUsername: state.map(value => findField(value, ["username", "finderUsername"])).find(Boolean) || "",
      liveId: state.map(value => findField(value, ["liveId"])).find(Boolean) || "",
      objectId: findField(profile, ["objectId", "id"]),
      objectNonceId: findField(profile, ["objectNonceId", "nid"]),
      scene: 2,
    };
    if (!payload.finderUsername || !payload.liveId || !payload.objectId || !payload.objectNonceId) { report("missing-live-identifiers"); return resolved; }
    try {
      report("bridge-request");
      const response = await fetch("/__live_archive/live-info", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ username: payload.finderUsername, oid: payload.objectId, nid: payload.objectNonceId, id: payload.liveId }) });
      if (!response.ok) { report("bridge-unavailable"); return resolved; }
      const result = await response.json();
      const joined = findLive(result && (result.data || result)) || resolved;
      if (!joined || !joined.liveSdkInfo || !joined.liveSdkInfo.liveCdnUrl) report("join-returned-no-cdn");
      return joined;
    } catch (_) {
      return resolved;
    }
  };
  const submit = async (profile, live) => {
    const resolved = await resolveLive(profile, live);
    const stream = resolved && resolved.liveSdkInfo && resolved.liveSdkInfo.liveCdnUrl;
    if (!/^https:\/\//i.test(stream || "")) { report("awaiting-playback"); return false; }
    if (!window.dl$ || typeof window.dl$.create !== "function") return false;
    const result = await window.dl$.create({ profile, live: resolved }, { platform: "wxchannels" });
    if (result && result.error) return false;
    return true;
  };
  const install = () => {
    // `channels.live.js` is preserved verbatim. On newer desktop bundles its
    // initial event can occasionally precede that script's listener, which
    // leaves the reference icon visible but without its `onclick` setup. Add
    // a narrow fallback only for that unbound, injected first-child icon.
    const attachFallback = () => {
      const button = document.querySelector('.host__info .extra > [ml-key="live-menu-share"]:first-child');
      if (!button || button.__liveArchiveFallback) return;
      button.__liveArchiveFallback = true;
      button.addEventListener("click", async () => {
        if (button.__setup) return;
        await submit(window.__liveArchiveLatestProfile || {}, window.__liveArchiveLatestLive);
      }, true);
    };
    attachFallback();
    new MutationObserver(attachFallback).observe(document.documentElement, { childList: true, subtree: true });
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", install); else install();
  setTimeout(install, 0);
})();"#;
    format!(
        r##"# Generated by Live Archive; metadata-only WeChat capture addon.
{windows_asyncio}
from mitmproxy import http
import http.client as http_client
import base64
import json
import os
import re
import urllib.parse

VERBOSE = os.environ.get("LIVE_ARCHIVE_VERBOSE", "").lower() in ("1", "true", "yes", "on")

def verbose_log(message):
    if VERBOSE:
        print(message)

def bridge_log(message, status):
    if status >= 400:
        print(message + str(status))
    else:
        verbose_log(message + str(status))

ENDPOINT = {endpoint:?}
TOKEN = {token:?}
CACHE_BUST = {cache_bust:?}
UPSTREAM_ASSETS = dict(json.loads(base64.b64decode({upstream_assets:?}).decode("utf-8")))
LIVE_PAGE_TAG = {live_page_tag:?}
RECORDER_ADAPTER = {recorder_adapter:?}

# The desktop client frequently uses several chunks from the same host. Keep a
# small, path-only inventory so a changed bundle can be compared with the
# reference interceptor without recording query strings, source text, or an
# unbounded amount of diagnostic data.

# This is served as a same-origin script instead of an inline <script>.  The
# desktop Channels client applies a CSP which can silently discard inline
# scripts, while it allows the same-origin script injection used by the
# reference application.
PAGE_CAPTURE_SCRIPT = r'''/* live-archive-response-observer */
(() => {{
  // Use a same-origin bridge: the desktop client's CSP blocks direct HTTPS
  // page requests to a local HTTP listener.
  const endpoint = "/__live_archive/capture";
  const diagnosticEndpoint = "/__live_archive/capture-status?";
  // Minimal, self-contained equivalent of the reference project's WXE/WXU
  // runtime. It deliberately exposes only the live-page contracts.
  const listeners = new Map();
  const on = (name, handler) => {{ const items = listeners.get(name) || []; items.push(handler); listeners.set(name, items); return () => listeners.set(name, items.filter(item => item !== handler)); }};
  const emit = (name, value) => (listeners.get(name) || []).forEach(handler => {{ try {{ handler(value); }} catch (_) {{}} }});
  window.WXE = window.WXE || {{on, off: () => {{}}, emit, Events: {{APILoaded:"channels:APILoaded", FeedProfileLoaded:"channels:OnFeedProfileLoaded", LiveProfileLoaded:"channels:OnLiveProfileLoaded", JoinLive:"channels:JoinLive"}}}};
  window.WXU = window.WXU || {{emit, onAPILoaded: handler => on("channels:APILoaded", handler), onFetchFeedProfile: handler => on("channels:OnFeedProfileLoaded", handler), onFetchLiveProfile: handler => on("channels:OnLiveProfileLoaded", handler), onJoinLive: handler => on("channels:JoinLive", handler)}};
  window.WXU.onFetchFeedProfile(profile => {{ window.__liveArchiveProfileResult = profile; }});
  window.WXU.onFetchLiveProfile(profile => {{ window.__liveArchiveProfileResult = profile; }});
  window.WXU.onJoinLive(live => {{ window.__liveArchiveJoinLiveResult = live; window.__liveArchiveRemember(live); }});
  const findLiveData = (value, seen = new Set(), depth = 0) => {{
    if (!value || typeof value !== "object" || seen.has(value) || depth > 6) return null;
    seen.add(value);
    if (value.liveSdkInfo && value.liveSdkInfo.liveCdnUrl) return value;
    for (const child of Object.values(value)) {{
      const found = findLiveData(child, seen, depth + 1);
      if (found) return found;
    }}
    return null;
  }};
  const normalize = value => {{
    try {{
      const data = findLiveData(value);
      const sdk = data && data.liveSdkInfo;
      const anchor = (data && (data.anchorContact || data.contact)) || {{}};
      const username = anchor.username || (data && data.username) || "";
      const title = anchor.nickname || (data && data.nickname) || username;
      // Some clients expose more than one directly playable CDN rendition.
      // Keep this deliberately conservative: only HTTPS URLs already exposed
      // to the signed-in page are offered, with no key derivation or media
      // decryption.  Unknown field names remain usable instead of binding the
      // capture flow to one Channels bundle version.
      const qualities = sdk ? Object.entries(sdk)
        .filter(([key, url]) => /url$/i.test(key) && typeof url === "string" && /^https:\/\//i.test(url))
        .map(([key, url]) => ({{key, url}}))
        .filter((item, index, all) => all.findIndex(other => other.url === item.url) === index) : [];
      if (sdk && sdk.liveCdnUrl && !qualities.some(item => item.url === sdk.liveCdnUrl)) qualities.unshift({{key:"liveCdnUrl", url:sdk.liveCdnUrl}});
      return qualities.length ? {{data, sdk, username, title, qualities}} : null;
    }} catch (_) {{}}
    return null;
  }};
  window.__liveArchiveRemember = value => {{
    window.__liveArchiveJoinLiveResult = value;
    const candidate = normalize(value);
    if (candidate) window.__liveArchiveLive = candidate;
    return candidate;
  }};
  window.__liveArchiveSend = streamUrl => {{
    const candidate = window.__liveArchiveLive;
    if (!candidate) return false;
    const {{sdk, username, title}} = candidate;
    const selectedUrl = streamUrl || sdk.liveCdnUrl || candidate.qualities[0].url;
    if (!/^https:\/\//i.test(selectedUrl)) return false;
    const pageUrl = username
      ? "https://channels.weixin.qq.com/live-archive/" + encodeURIComponent(username)
      : location.href;
    navigator.sendBeacon(endpoint, JSON.stringify({{page_url: pageUrl, stream_url: selectedUrl, title: title || document.title || "微信视频号直播", anchor_username: username || null}}));
    return true;
  }};
  // Bundle exports vary between WeChat desktop releases. When the internal
  // `joinLive` API is not exposed, a stream that the user has already played
  // is still visible through the standard Resource Timing API. This fallback
  // uses only an HTTPS HLS/FLV URL that the page has actually loaded.
  const observedLiveUrl = () => {{
    try {{
      const entries = performance.getEntriesByType("resource");
      for (let index = entries.length - 1; index >= 0; index -= 1) {{
        const value = entries[index] && entries[index].name;
        if (/^https:\/\//i.test(value) && /\.(?:m3u8|flv)(?:[?#]|$)/i.test(value)) return value;
      }}
    }} catch (_) {{}}
    return "";
  }};
  const pageCaptureId = () => {{
    let hash = 2166136261;
    for (const char of location.href) {{ hash = Math.imul(hash ^ char.charCodeAt(0), 16777619); }}
    return "page-" + (hash >>> 0).toString(36);
  }};
  window.__liveArchiveSendObserved = () => {{
    const streamUrl = observedLiveUrl();
    if (!streamUrl) return false;
    navigator.sendBeacon(endpoint, JSON.stringify({{page_url: location.href, stream_url: streamUrl, title: document.title || "微信视频号直播", anchor_username: pageCaptureId()}}));
    return true;
  }};
  const inspect = response => {{ try {{ response.clone().json().then(window.__liveArchiveRemember).catch(() => {{}}); }} catch (_) {{}} return response; }};
  const fetch0 = window.fetch;
  if (fetch0) window.fetch = (...args) => fetch0(...args).then(inspect);
  const open0 = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function(...args) {{ this.addEventListener("load", () => {{ try {{ window.__liveArchiveRemember(JSON.parse(this.responseText)); }} catch (_) {{}} }}); return open0.apply(this, args); }};
  const WebSocket0 = window.WebSocket;
  if (WebSocket0) {{
    const ObservedWebSocket = function(...args) {{
      const socket = new WebSocket0(...args);
      socket.addEventListener("message", event => {{
        if (typeof event.data === "string") {{ try {{ window.__liveArchiveRemember(JSON.parse(event.data)); }} catch (_) {{}} }}
        else if (event.data instanceof Blob) event.data.text().then(text => {{ try {{ window.__liveArchiveRemember(JSON.parse(text)); }} catch (_) {{}} }}).catch(() => {{}});
      }});
      return socket;
    }};
    ObservedWebSocket.prototype = WebSocket0.prototype;
    window.WebSocket = ObservedWebSocket;
  }}
  const mountButton = () => {{
    if (window.__liveArchiveModerationV1) return;
    if (document.getElementById("live-archive-download-button")) return;
    const button = document.createElement("button");
    button.id = "live-archive-download-button";
    button.type = "button";
    button.textContent = "录制直播";
    button.title = "将当前直播加入 Live Archive 录制列表";
    Object.assign(button.style, {{position:"fixed", right:"20px", top:"76px", zIndex:"2147483647", border:"0", borderRadius:"6px", padding:"8px 12px", color:"#fff", background:"#07c160", fontSize:"14px", cursor:"pointer", boxShadow:"0 2px 8px rgba(0,0,0,.25)"}});
    const status = document.createElement("div");
    status.id = "live-archive-status";
    status.textContent = "状态：等待直播页面数据";
    Object.assign(status.style, {{position:"fixed", right:"20px", top:"154px", zIndex:"2147483647", maxWidth:"220px", color:"#57606a", background:"rgba(255,255,255,.92)", borderRadius:"4px", padding:"4px 6px", fontSize:"12px", textAlign:"right"}});
    const setStatus = (text, stage) => {{
      status.textContent = "状态：" + text;
      if (stage) {{ try {{ navigator.sendBeacon(diagnosticEndpoint + "stage=" + stage, ""); }} catch (_) {{}} }}
    }};
    const quality = document.createElement("select");
    quality.id = "live-archive-quality";
    quality.title = "选择页面已提供的直播清晰度";
    Object.assign(quality.style, {{position:"fixed", right:"20px", top:"118px", zIndex:"2147483647", maxWidth:"180px", border:"1px solid #07c160", borderRadius:"6px", padding:"5px", background:"#fff", color:"#1f2329", fontSize:"12px"}});
    const updateQualities = () => {{
      const current = quality.value;
      quality.replaceChildren();
      const options = (window.__liveArchiveLive && window.__liveArchiveLive.qualities) || [];
      for (const option of options) {{
        const item = document.createElement("option");
        item.value = option.url;
        item.textContent = option.key === "liveCdnUrl" ? "默认清晰度" : option.key;
        quality.appendChild(item);
      }}
      if (current && [...quality.options].some(option => option.value === current)) quality.value = current;
      quality.style.display = options.length > 1 ? "block" : "none";
    }};
    button.addEventListener("click", async () => {{
      updateQualities();
      if (window.__liveArchiveSend(quality.value) || window.__liveArchiveSendObserved()) {{ setStatus("已提交录制请求", "submitted"); button.textContent = "正在启动录制"; setTimeout(() => button.textContent = "录制直播", 1800); }}
      else {{
        // Re-evaluate the raw joinLive result at click time. Some versions
        // populate nested fields after resolving the initial promise.
        window.__liveArchiveRemember(window.__liveArchiveJoinLiveResult);
        if (window.__liveArchiveSend(quality.value) || window.__liveArchiveSendObserved()) {{ setStatus("已提交录制请求", "submitted"); button.textContent = "正在启动录制"; setTimeout(() => button.textContent = "录制直播", 1800); }}
        else {{
          // Match the reference live page: recording is available only after
          // the page itself has joined the room and emitted its live data.
          // Do not manufacture a joinLive payload or invoke internal APIs.
          setStatus("请先进入并播放直播", "awaiting-playback");
          button.textContent = "请先播放直播后再录制";
          setTimeout(() => button.textContent = "录制直播", 2200);
        }}
      }}
    }});
    document.body.appendChild(button);
    document.body.appendChild(quality);
    document.body.appendChild(status);
  }};
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", mountButton, {{once:true}}); else mountButton();
}})();'''

def load(loader):
    print("live-archive: page capture addon loaded")

def forward_to_loopback(path, body, method="POST"):
    # The page talks only to its own Channels origin. The addon forwards the
    # compact metadata payload directly to the bound loopback endpoint without
    # using a system proxy, and never logs the payload or response body.
    target = urllib.parse.urlsplit(ENDPOINT)
    query = "permit=" + urllib.parse.quote(TOKEN)
    if "?" in path:
        query += "&" + path.split("?", 1)[1]
    target_path = target.path + ("-status" if path.startswith("/__live_archive/capture-status") else "")
    if path.startswith("/api/v1/download_task/"):
        target_path = path.split("?", 1)[0]
    if path.startswith("/__live_archive/live-info"):
        target_path = "/live-info"
    connection = http_client.HTTPConnection(target.hostname, target.port, timeout=3)
    try:
        connection.request(method, target_path + "?" + query, body=body.encode("utf-8"), headers={{"Content-Type": "application/json", "Content-Length": str(len(body.encode("utf-8"))), "Authorization": "Bearer " + TOKEN}})
        response = connection.getresponse()
        return response.status, response.read()
    finally:
        connection.close()

def forward_live_profile(path):
    target = urllib.parse.urlsplit(ENDPOINT)
    query = "permit=" + urllib.parse.quote(TOKEN)
    if "?" in path:
        query += "&" + path.split("?", 1)[1]
    connection = http_client.HTTPConnection(target.hostname, target.port, timeout=12)
    try:
        connection.request("GET", "/api/channels/live/profile?" + query)
        response = connection.getresponse()
        return response.status, response.read()
    finally:
        connection.close()

def request(flow: http.HTTPFlow):
    path = flow.request.path.split("?", 1)[0].lower()
    if flow.request.host == "channels.weixin.qq.com" and path.startswith("/__assets/"):
        asset_path = path[len("/__assets"):]
        source = UPSTREAM_ASSETS.get(asset_path)
        if source is None:
            flow.response = http.Response.make(404, b"not found")
            return
        content_type = "text/css; charset=utf-8" if asset_path.endswith(".css") else "application/javascript; charset=utf-8"
        flow.response = http.Response.make(200, source.encode("utf-8"), {{"Content-Type": content_type, "Cache-Control": "no-store"}})
        if asset_path == "/inject/channels.live.js":
            verbose_log("live-archive: served reference channels.live.js")
        return
    if flow.request.host == "channels.weixin.qq.com" and path in ("/__live_archive/capture", "/__live_archive/capture-status"):
        if flow.request.method != "POST":
            flow.response = http.Response.make(405, b"method not allowed")
            return
        try:
            body = flow.request.get_text(strict=False)
            status, _ = forward_to_loopback(flow.request.path, body)
        except Exception:
            status = 502
        kind = "button status" if path.endswith("capture-status") else "recording metadata"
        bridge_log("live-archive: same-origin " + kind + " bridge returned ", status)
        flow.response = http.Response.make(status, b"")
        return
    if flow.request.host == "channels.weixin.qq.com" and path.startswith("/api/v1/download_task/"):
        if flow.request.method not in ("GET", "POST"):
            flow.response = http.Response.make(405, b"method not allowed")
            return
        try:
            body = flow.request.get_text(strict=False)
            status, response_body = forward_to_loopback(flow.request.path, body, flow.request.method)
        except Exception:
            status, response_body = 502, b'{{"code":502,"msg":"bridge unavailable"}}'
        bridge_log("live-archive: reference download task API bridge returned ", status)
        flow.response = http.Response.make(status, response_body, {{"Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store"}})
        return
    if flow.request.host == "channels.weixin.qq.com" and path == "/api/channels/live/profile":
        try:
            status, response_body = forward_live_profile(flow.request.path)
        except Exception:
            status, response_body = 502, b'{{"code":502,"msg":"bridge unavailable"}}'
        flow.response = http.Response.make(status, response_body, {{"Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store"}})
        return
    if flow.request.host == "channels.weixin.qq.com" and path == "/__live_archive/live-info":
        try:
            status, response_body = forward_to_loopback(flow.request.path, flow.request.get_text(strict=False))
        except Exception:
            status, response_body = 502, b'{{"code":502}}'
        flow.response = http.Response.make(status, response_body, {{"Content-Type": "application/json; charset=utf-8", "Cache-Control": "no-store"}})
        return
def set_uncached_text(flow, text):
    flow.response.set_text(text)
    # Modified pages/scripts depend on the temporary same-origin bridge. Do not
    # let the embedded browser replay them after the proxy has stopped.
    flow.response.headers["cache-control"] = "no-store"
    flow.response.headers.pop("etag", None)
    flow.response.headers.pop("last-modified", None)

def response(flow: http.HTTPFlow):
    # The Channels web client resolves a live room through its internal
    # joinLive method. Hook that method in the delivered JavaScript so the
    # page reports its own stable anchor username together with the resolved
    # CDN URL. This avoids guessing a room from CDN host names or open order.
    content_type = flow.response.headers.get("content-type", "").lower()
    page_path = flow.request.path.split("?", 1)[0]
    # Service probes may return the site's HTML fallback; they are not pages.
    if page_path.lower().startswith("/api/"):
        return
    if flow.request.host == "channels.weixin.qq.com" and "text/html" in content_type:
        try:
            html = flow.response.get_text(strict=False)
        except Exception:
            return
        # Match the reference implementation's cache-busting behavior.  The
        # client otherwise commonly reuses a previously cached application
        # bundle, which means a response-time hook is never evaluated.
        def cache_bust(match):
            prefix, url = match.group(1), match.group(2)
            if "live_archive_capture=" in url:
                return match.group(0)
            separator = "&" if "?" in url else "?"
            return prefix + url + separator + "live_archive_capture=" + CACHE_BUST + "\""
        html, html_script_changes = re.subn(r'((?:src|href)=")([^"]*\.js(?:\?[^"]*)?)"', cache_bust, html)
        page_path = flow.request.path.split("?", 1)[0]
        if "/__assets/inject/channels.live.js" not in html:
            # Match the reference interceptor exactly: its support scripts go
            # immediately after `<head>`, before Channels' own application
            # bundle.  Placing them at `</head>` can subscribe after the live
            # page has already emitted its initial profile event, leaving a
            # visible but inert download button.
            observer = {injected_tags:?}
            if page_path == "/web/pages/live":
                observer += LIVE_PAGE_TAG
            if "<head>" in html:
                set_uncached_text(flow, html.replace("<head>", "<head>\n" + observer, 1))
                verbose_log("live-archive: response observer injected into Channels page " + page_path)
            else:
                verbose_log("live-archive: skipped non-document Channels HTML response " + page_path)
        return
    request_path = flow.request.path.split("?", 1)[0].lower()
    # Resources served by this addon contain the bridge's own marker names;
    # do not feed them back into the WeChat bundle diagnostics.
    if flow.request.host == "channels.weixin.qq.com" and (request_path.startswith("/__live_archive/") or request_path.startswith("/__assets/")):
        return
    # Follow the reference interceptor's scope: never inspect scripts from
    # unrelated applications that happen to use the temporary system proxy.
    if flow.request.host not in ("channels.weixin.qq.com", "finder.video.qq.com", "res.wx.qq.com"):
        return
    looks_like_script = ".js" in request_path or ".mjs" in request_path
    script_content = "javascript" in content_type or "ecmascript" in content_type
    # Match only explicit JavaScript resources. The reference interceptor
    # rewrites Channels application scripts, not arbitrary finder.video CDN
    # responses; treating generic text as script risks logging media requests.
    if not looks_like_script and not script_content:
        return
    try:
        source = flow.response.get_text(strict=False)
    except Exception:
        return
    # This follows the reference interceptor's resource scope. Every res.wx
    # module has its imports cache-busted; only the named modules receive
    # method/export wrappers.
    if flow.request.host != "res.wx.qq.com":
        return
    def cache_bust_module(match):
        prefix, url, suffix = match.group(1), match.group(2), match.group(3)
        if "live_archive_capture=" in url:
            return match.group(0)
        separator = "&" if "?" in url else "?"
        return prefix + url + separator + "live_archive_capture=" + CACHE_BUST + suffix
    patched, import_changes = re.subn(r'((?:from\s*|import\()\s*["\'])([^"\']+\.js(?:\?[^"\']*)?)(["\'])', cache_bust_module, source)
    # Bundlers also publish lazy chunks as plain "js/...js" manifest strings.
    patched, manifest_changes = re.subn(r'(["\'])(js/[^"\']+\.js(?:\?[^"\']*)?)(["\'])', cache_bust_module, patched)
    changes = import_changes + manifest_changes
    if "virtual_svg-icons-register.publish" not in request_path:
        if "connect.publish" in request_path or "applyMic.publish" in request_path:
            utils_prefix = ';typeof WXU!=="undefined"&&WXU.emit("channels:UtilsLoaded",{{decodeBase64ToUint64String:decodeBase64ToUint64String,createAdapterFromGlobalMapper:createAdapterFromGlobalMapper,finderJoinLiveMapper:finderJoinLiveMapper}});export{{'
            patched, count = re.subn(r'export\s*\{{', utils_prefix, patched, count=1)
            changes += count
        if changes:
            set_uncached_text(flow, patched)
        return
    live_info_pattern = re.compile(r"async\s+finderGetLiveInfo\s*\(([^)]*)\)\s*\{{(.*?)\}}\s*async", re.DOTALL)
    def hook_live_info(match):
        args, body = match.group(1), match.group(2)
        return '''async finderGetLiveInfo(''' + args + '''){{var result=await(async()=>{{''' + body + '''}})();var live=result.data;typeof WXU!=="undefined"&&WXU.emit("channels:OnLiveProfileLoaded",live);return result;}}async'''
    patched, count = live_info_pattern.subn(hook_live_info, patched, count=1)
    if count:
        changes += count

    # The reference live-page button receives its room identity from the feed
    # profile event. On current desktop builds that arrives through
    # finderGetCommentDetail rather than finderGetLiveInfo.
    feed_profile_pattern = re.compile(r"async\s+finderGetCommentDetail\s*\(([^)]*)\)\s*\{{(.*?)\}}\s*async", re.DOTALL)
    def hook_feed_profile(match):
        args, body = match.group(1), match.group(2)
        # This endpoint wraps the actual profile in `result.data.object`.
        # Keep the reference interceptor's shape exactly; forwarding the
        # wrapper makes the button appear but leaves it without anchor data.
        return '''async finderGetCommentDetail(''' + args + '''){{var result=await(async()=>{{''' + body + '''}})();var feed=result.data.object;typeof WXU!=="undefined"&&WXU.emit("channels:OnFeedProfileLoaded",feed);return result;}}async'''
    patched, count = feed_profile_pattern.subn(hook_feed_profile, patched, count=1)
    if count:
        changes += count

    join_live_pattern = re.compile(r"async\s+joinLive\s*\(([^)]*)\)\s*\{{(.*?)\}}\s*async", re.DOTALL)
    def hook_join_live(match):
        args, body = match.group(1), match.group(2)
        return '''async joinLive(''' + args + '''){{var result=await(async()=>{{''' + body + '''}})();var data=result.data;typeof WXU!=="undefined"&&WXU.emit("channels:JoinLive",data);return result;}}async'''
    patched, count = join_live_pattern.subn(hook_join_live, patched, count=1)
    if count:
        changes += count

    # This is the reference interceptor's API-export bridge. `channels.utils`
    # discovers the object containing `joinLive` from this event and exposes
    # it as `WXU.LiveAPI`; the click handler can then ask the page itself for
    # a fresh CDN URL when the initially rendered live state omits one.
    export_block_pattern = re.compile(r'export\s*\{{([^}}]*)\}}')
    export_block = export_block_pattern.search(patched)
    if export_block:
        locals = []
        for item in export_block.group(1).split(','):
            local = item.strip().split(' as ', 1)[0].strip()
            if re.match(r'^[A-Za-z_$][A-Za-z0-9_$]*$', local):
                locals.append(local)
        if locals:
            api_methods = "{{" + ",".join(locals) + "}}"
            export_prefix = ';try{{window.WXU&&window.WXU.emit("channels:APILoaded",' + api_methods + ');}}catch(_){{}};export{{'
            patched, export_count = re.subn(r'export\s*\{{', export_prefix, patched, count=1)
            if export_count:
                changes += export_count
    if changes:
        set_uncached_text(flow, patched)
"##,
        windows_asyncio = include_str!("../assets/moderation/proxy/windows_asyncio.py")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn addon_captures_only_explicit_page_actions_without_sensitive_data() {
        let config = MitmproxySidecarConfig {
            executable: "mitmdump".into(),
            working_directory: PathBuf::from("/tmp/live-archive-test"),
            listen_port: 2023,
            capture_endpoint: Url::parse("http://127.0.0.1:1234/capture").unwrap(),
            channels_ws_url: "ws://127.0.0.1:2022/ws/channels".into(),
            downloader_ws_url: "ws://127.0.0.1:2023/ws/v1/download_task".into(),
            permit: CapturePermit::generate(),
        };
        let addon = render_addon(&config);
        assert!(addon.contains("async joinLive("));
        assert!(addon.contains("navigator.sendBeacon"));
        assert!(addon.contains("page capture addon loaded"));
        assert!(addon.contains("res.wx.qq.com"));
        assert!(addon.contains("response observer injected into Channels page"));
        assert!(addon.contains("if page_path == \"/web/pages/live\":"));
        assert!(addon.contains("LIVE_PAGE_TAG ="));
        assert!(addon.contains("html.replace(\"<head>\", \"<head>\\n\" + observer, 1)"));
        assert!(addon.contains("CACHE_BUST ="));
        assert!(!addon.contains("refreshed \" + str(html_script_changes) + \" page script URLs"));
        assert!(addon.contains("/__live_archive/capture-status"));
        assert!(addon.contains("forward_to_loopback"));
        assert!(addon.contains("forward_live_profile"));
        assert!(addon.contains("/api/channels/live/profile"));
        assert!(addon.contains("flow.request.get_text(strict=False)"));
        assert!(addon.contains("same-origin \" + kind + \" bridge returned"));
        assert!(!addon.contains("reference API export bridge installed"));
        assert!(addon.contains("WXU.LiveAPI"));
        assert!(addon.contains("UPSTREAM_ASSETS = dict(json.loads(base64.b64decode"));
        assert!(addon.contains(r#"async\s+joinLive\s*\(([^)]*)\)\s*\{(.*?)\}\s*async"#));
        assert!(!addon.contains("live API candidates from"));
        assert!(!addon.contains("live markers from"));
        assert!(addon.contains("virtual_svg-icons-register.publish"));
        assert!(addon.contains("Every res.wx"));
        assert!(addon.contains("channels:UtilsLoaded"));
        assert!(addon.contains("request_path.startswith(\"/__assets/\")"));
        assert!(addon.contains("flow.request.path.split(\"?\", 1)[0]"));
        assert!(!addon.contains("flow.request.content"));
        assert!(!addon.contains("flow.response.content"));
        assert!(!addon.contains("or \"text/plain\" in content_type"));
        let mut child =
            std::process::Command::new(if cfg!(windows) { "python" } else { "python3" })
                .args(["-X", "utf8"])
                .args([
                    "-c",
                    include_str!("../tests/support/capture_logging_driver.py"),
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), addon.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn encodes_python_unsafe_asset_source_as_base64() {
        assert_eq!(base64_encode(br#"\u12"#), "XHUxMg==");
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }
}
