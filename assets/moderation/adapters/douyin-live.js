// Read-only adapter. Optional bounded decoder observes existing WS traffic.
// DOM comments are observations, not proof of complete gifts/upgrade coverage.
const trackNetwork = (channel, format, bytes = 0) => sdk.emit("probe.network", "network", { channel, format, bytes });
const relevant = value => {
  try {
    const url = new URL(typeof value === "string" ? value : value.url, location.href);
    return /(^|\.)douyin\.com$/.test(url.hostname) && /webcast|\/im\/|\/live\//.test(url.pathname) && !url.pathname.startsWith(BOOT.prefix);
  } catch (_) { return false; }
};
const originalFetch = window.fetch;
const metadataReaders = new Set();
const fetchHook = function (...args) {
  const result = Reflect.apply(originalFetch, this, args);
  if (relevant(args[0])) result.then(response => { sdk.captureResponse?.("fetch",response); trackNetwork("fetch", (response.headers.get("content-type") || "unknown").split(";")[0]); }).catch(() => {});
  if (BOOT.rules_preview && relevant(args[0]) && String(args[0]?.url || args[0]).includes("/webcast/room/web/enter")) {
    result.then(async response => {
      if (metadataReaders.size >= 4 || sdk.getStatus().stopped) return;
      const copy = response.clone(), reader = copy.body?.getReader();
      if (!reader) return;
      metadataReaders.add(reader);
      const timeout = setTimeout(() => { void reader.cancel().catch(() => {}); }, 2000);
      const chunks = []; let size = 0;
      try {
        while (true) { const r = await reader.read(); if (r.done) break; size += r.value.length; if (size > 2 * 1024 * 1024) return; chunks.push(r.value); }
        const bytes = new Uint8Array(size); let position = 0;
        for (const chunk of chunks) { bytes.set(chunk, position); position += chunk.length; }
        douyinDecoder.bindRoom(JSON.parse(new TextDecoder().decode(bytes)), args[0]?.url || args[0]);
      } finally { clearTimeout(timeout); metadataReaders.delete(reader); void reader.cancel().catch(() => {}); }
    }).catch(() => {});
  }
  return result;
};
window.fetch = fetchHook;
sdk.onDispose(() => {
  if (window.fetch === fetchHook) window.fetch = originalFetch;
  for (const reader of metadataReaders) void reader.cancel().catch(() => {});
  metadataReaders.clear();
});
const originalOpen = XMLHttpRequest.prototype.open;
const xhrListeners = new Map();
const openHook = function (...args) {
  const previous = xhrListeners.get(this);
  if (previous) previous();
  xhrListeners.delete(this);
  const result = Reflect.apply(originalOpen, this, args);
  if (relevant(args[1]) && xhrListeners.size < 100) {
    const xhr = this;
    const listener = () => {
      try { if (xhr.responseType === "json") sdk.captureRaw?.("xhr","response",xhr.response);
        else if (!xhr.responseType || xhr.responseType === "text") sdk.captureRaw?.("xhr","response",xhr.responseText);
        if (BOOT.rules_preview && String(args[1]).includes("/webcast/room/web/enter")) {
          const value=xhr.responseType === "json" ? xhr.response : xhr.responseText;
          if (typeof value !== "string" || value.length <= 2 * 1024 * 1024) douyinDecoder.bindRoom(typeof value === "string" ? JSON.parse(value) : value, args[1]);
        } } catch (_) {sdk.captureRawGap?.("xhr_read_failed");}
      trackNetwork("xhr", (xhr.getResponseHeader("content-type") || "unknown").split(";")[0]);
      release();
    };
    const release = () => {
      xhr.removeEventListener("load", listener);
      xhr.removeEventListener("loadend", release);
      xhrListeners.delete(xhr);
    };
    this.addEventListener("load", listener, { once: true });
    this.addEventListener("loadend", release, { once: true });
    xhrListeners.set(this, release);
  }
  return result;
};
XMLHttpRequest.prototype.open = openHook;
sdk.onDispose(() => {
  if (XMLHttpRequest.prototype.open === openHook) XMLHttpRequest.prototype.open = originalOpen;
  for (const release of [...xhrListeners.values()]) release();
  xhrListeners.clear();
});
const OriginalWebSocket = window.WebSocket;
const socketListeners = new Map();
const SocketHook = new Proxy(OriginalWebSocket, {
  construct(target, args, newTarget) {
    const socket = Reflect.construct(target, args, newTarget);
    if (relevant(args[0]) && socketListeners.size < 100) {
      const listener = event => {
        sdk.captureRaw?.("websocket","message",event.data);
        trackNetwork("websocket", typeof event.data === "string" ? "text" : "binary", typeof event.data === "string" ? event.data.length : event.data?.byteLength || event.data?.size || 0);
        if (BOOT.rules_preview) douyinDecoder.observe(event.data, args[0]);
      };
      socket.addEventListener("message", listener);
      socketListeners.set(socket, listener);
      socket.addEventListener("close", () => socketListeners.delete(socket), { once: true });
    }
    return socket;
  }
});
window.WebSocket = SocketHook;
sdk.onDispose(() => {
  if (window.WebSocket === SocketHook) window.WebSocket = OriginalWebSocket;
  for (const [socket, listener] of socketListeners) socket.removeEventListener("message", listener);
  socketListeners.clear();
});
const seen = new WeakMap();
let chatRoot = null, chatObserver = null, rootTimer = null;
const publicRows = new Set();
const publicScope=()=>BOOT.timed_messages?.count_scope==='public_screen';
const publicRowId=row=>row.getAttribute?.('data-index');
const observePublicRow=row=>{
  const id=publicRowId(row);
  if(!publicScope()||id===null||id===undefined||!String(row.textContent||'').trim()||publicRows.has(id))return;
  publicRows.add(id);
  if(publicRows.size>16384)publicRows.delete(publicRows.values().next().value);
  sdk.emit('public.activity','dom',{content:sdk.clean(row.textContent||'',512),decoder:'douyin.public_screen.v1',observed_at_ms:Date.now()},{native_event_id:'screen:'+id});
};
const comment = row => {
  const nickname = sdk.clean(row.querySelector('[class*="webcast-chatroom___user-name"], .hts-live-text-user')?.textContent || "", 128).trim().replace(/[：:]\s*$/, "").trim();
  const content = sdk.clean(row.querySelector('[class*="webcast-chatroom___content"]')?.textContent || "");
  return { nickname, content, signature: JSON.stringify([row.getAttribute?.("data-index"), nickname, content]) };
};
const inspectNode = node => {
  const element = node instanceof HTMLElement ? node : node?.parentElement;
  if (!element) return;
  const parent = element.closest?.('[data-index]');
  const rows = new Set([...(parent ? [parent] : []), ...(element.matches('[data-index]') ? [element] : [...element.querySelectorAll('[data-index]')])]);
  for (const row of rows) {
    observePublicRow(row);
    const { nickname, content, signature } = comment(row);
    if (seen.get(row) === signature) continue;
    seen.set(row, signature);
    if (nickname && content) sdk.emit("chat.received", "dom", { content }, { nickname });
  }
};
const attachChat = () => {
  const root = document.querySelector('div[class*="webcast-chatroom___list"]');
  if (root === chatRoot) return;
  chatObserver?.disconnect();
  chatRoot = root;
  if (!root) return;
  // Baseline is historical UI content. Do not replay it as fresh interaction.
  for (const row of root.querySelectorAll('[data-index]')) {seen.set(row, comment(row).signature);const id=publicRowId(row);if(id!==null&&id!==undefined)publicRows.add(id);}
  chatObserver = new MutationObserver(records => {
    for (const record of records) {
      inspectNode(record.target);
      for (const node of record.addedNodes) inspectNode(node);
    }
  });
  chatObserver.observe(root, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["data-index"] });
  sdk.emit("probe.status", "diagnostic", { stage: "chat_dom_observer_attached" });
};
rootTimer = setInterval(attachChat, 1000);
attachChat();
sdk.onDispose(() => { clearInterval(rootTimer); chatObserver?.disconnect(); });
sdk.emit("probe.status", "diagnostic", { stage: BOOT.rules_preview ? "douyin_read_only_decoder_installed" : "douyin_network_hooks_installed_no_decoder" });
