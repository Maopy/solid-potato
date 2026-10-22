const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const path = require("node:path");
const root = path.resolve(__dirname, "..");
const sdk = fs.readFileSync(path.join(root, "assets/moderation/sdk/probe.js"), "utf8");
const adapter = fs.readFileSync(path.join(root, "assets/moderation/adapters/douyin-live.js"), "utf8");

function environment() {
  const requests = [], timers = new Map();
  let counter = 0;
  class Socket {
    static OPEN = 1;
    constructor(url) { this.url = url; this.listeners = new Map(); }
    addEventListener(name, callback) { this.listeners.set(name, callback); }
    removeEventListener(name, callback) { if (this.listeners.get(name) === callback) this.listeners.delete(name); }
    dispatch(name, value) { this.listeners.get(name)?.(value); }
    send() { throw new Error("probe must never send on platform socket"); }
  }
  class Xhr extends Socket {
    open(method, url) { this.method = method; this.url = url; }
    getResponseHeader() { return "application/json"; }
  }
  const platformPromise = Promise.resolve({ headers: { get: () => "application/json" }, json: async () => ({ untouched: true }) });
  const nativeFetch = function (url, options) {
    if (!String(url).startsWith("/__live_archive/")) return platformPromise;
    const body = JSON.parse(options.body);
    requests.push({ url, body, headers: options.headers });
    return Promise.resolve({ ok: true, json: async () => String(url).endsWith("hello")
      ? { connection_id: "conn", binding_id: "binding", token: "token" }
      : { acked_seq: body.events?.at(-1)?.seq || 0, commands: [], send_chat: false } });
  };
  const window = { fetch: nativeFetch, WebSocket: Socket, addEventListener() {}, removeEventListener() {} };
  const context = vm.createContext({ window, XMLHttpRequest: Xhr, location: { origin: "https://live.douyin.com", pathname: "/123" },
    performance, crypto: { randomUUID: () => "document-id" }, AbortController, URL,
    document: { querySelector: () => null }, HTMLElement: class {}, MutationObserver: class {},
    setTimeout: (fn, delay) => { const id = ++counter; timers.set(id, { fn, delay }); return id; },
    clearTimeout: id => timers.delete(id), setInterval: (fn, delay) => { const id = ++counter; timers.set(id, { fn, delay, interval: true }); return id; },
    clearInterval: id => timers.delete(id) });
  const source = `(() => { const BOOT = {room_url:"https://live.douyin.com/123", prefix:"/__live_archive/moderation/v1/", ticket:"ticket"};\n${sdk}\n${adapter}\n})();`;
  return { context, window, requests, timers, nativeFetch, originalOpen: Xhr.prototype.open, Socket, Xhr, platformPromise, source };
}

const settle = () => new Promise(resolve => setImmediate(resolve));

test("network observation preserves fetch promise, XHR, WS behavior and excludes bridge traffic", async () => {
  const env = environment(); vm.runInContext(env.source, env.context); await settle();
  const sdk = env.window.__liveArchiveModerationV1;
  assert.equal(sdk.getStatus().send_chat, false);
  const promise = env.window.fetch("https://live.douyin.com/webcast/im/fetch");
  assert.equal(promise, env.platformPromise);
  assert.deepEqual(await (await promise).json(), { untouched: true });
  assert.equal(env.window.WebSocket.OPEN, 1);
  const socket = new env.window.WebSocket("wss://webcast100-ws-web-lq.douyin.com/webcast/im/push/v2/");
  assert.ok(socket instanceof env.Socket);
  socket.dispatch("message", { data: new Uint8Array([1, 2]).buffer });
  const xhr = new env.Xhr(); xhr.open("GET", "https://live.douyin.com/webcast/im/fetch");
  xhr.dispatch("load", {});
  xhr.open("GET", "https://live.douyin.com/not-a-live-api");
  assert.equal(xhr.listeners.size, 0);
  await settle();
  const next = [...env.timers.values()].find(t => t.delay === 500);
  await next.fn(); await settle();
  const events = env.requests.filter(r => r.url.endsWith("events")).flatMap(r => r.body.events);
  assert.deepEqual(events.filter(e => e.event_type === "probe.network").map(e => e.payload.channel).sort(), ["fetch", "websocket", "xhr"]);
  assert.equal(events.some(e => JSON.stringify(e).includes("wss://")), false);
});

test("duplicate bootstrap is idempotent; room navigation disposes hooks and poll timers", async () => {
  const env = environment(); vm.runInContext(env.source, env.context); await settle();
  const firstFetch = env.window.fetch, instance = env.window.__liveArchiveModerationV1;
  vm.runInContext(env.source, env.context); await settle();
  assert.equal(env.window.fetch, firstFetch);
  assert.equal(env.window.__liveArchiveModerationV1, instance);
  assert.equal(env.requests.filter(r => r.url.endsWith("hello")).length, 1);
  env.context.location.pathname = "/456";
  await [...env.timers.values()].find(t => t.delay === 500).fn(); await settle();
  assert.equal(instance.getStatus().stopped, true);
  assert.equal(env.window.fetch, env.nativeFetch);
  assert.equal(env.window.WebSocket, env.Socket);
  assert.equal(env.Xhr.prototype.open, env.originalOpen);
  assert.equal([...env.timers.values()].filter(t => t.interval).length, 0);
  assert.equal(env.requests.some(r => r.url.endsWith("goodbye")), true);
});

test("buffer overload is bounded and records an explicit gap instead of unlimited events", async () => {
  const env = environment(); vm.runInContext(env.source, env.context); await settle();
  const instance = env.window.__liveArchiveModerationV1;
  for (let i = 0; i < 2000; i++) instance.emit("future.event", "diagnostic", { stage: "test" });
  assert.equal(instance.getStatus().buffered, 500);
  assert.equal(instance.getStatus().gap, true);
  assert.equal(instance.getStatus().send_chat, false);
  instance.dispose(); await settle();
  assert.equal(instance.getStatus().buffered, 0);
});

test("leaving during pairing releases the eventual session without restarting polling", async () => {
  const env = environment();
  let resolveHello;
  env.window.fetch = (url, options) => String(url).endsWith("hello")
    ? new Promise(resolve => { resolveHello = resolve; }) : env.nativeFetch(url, options);
  vm.runInContext(env.source, env.context);
  env.window.__liveArchiveModerationV1.dispose();
  resolveHello({ ok: true, json: async () => ({ connection_id: "conn", binding_id: "binding", token: "token" }) });
  await settle();
  assert.equal(env.window.__liveArchiveModerationV1.getStatus().stopped, true);
  assert.equal(env.requests.filter(r => r.url.endsWith("goodbye")).length, 1);
  assert.equal(env.requests.some(r => r.url.endsWith("events") || r.url.endsWith("poll")), false);
  assert.equal(env.timers.size, 0);
});

test("current Douyin nickname markup produces a DOM observation without inventing user identity", async () => {
  const env = environment();
  let observer;
  const root = { querySelectorAll: () => [] };
  env.context.document.querySelector = () => root;
  env.context.MutationObserver = class {
    constructor(callback) { observer = callback; }
    observe() {}
    disconnect() {}
  };
  const row = new env.context.HTMLElement();
  row.matches = () => true;
  row.querySelector = selector => selector.includes(".hts-live-text-user")
    ? { textContent: " 观众甲 ： " }
    : selector.includes("webcast-chatroom___content") ? { textContent: "测试评论" } : null;
  vm.runInContext(env.source, env.context);
  observer([{ addedNodes: [row] }]);
  await settle();
  const comments = env.requests.filter(r => r.url.endsWith("events")).flatMap(r => r.body.events).filter(e => e.event_type === "chat.received");
  assert.equal(comments.length, 1);
  assert.equal(comments[0].nickname, "观众甲");
  assert.equal(comments[0].payload.content, "测试评论");
  assert.equal(comments[0].source_kind, "dom");
  assert.equal(comments[0].user_id, undefined);
  // Virtualized chat updates an existing row instead of adding a new row.
  row.querySelector = selector => selector.includes(".hts-live-text-user")
    ? { textContent: "观众乙：" }
    : selector.includes("webcast-chatroom___content") ? { textContent: "下一条评论" } : null;
  observer([{ target: row, addedNodes: [] }]);
  observer([{ target: row, addedNodes: [] }]);
  await [...env.timers.values()].find(t => t.delay === 500).fn();
  await settle();
  const updated = env.requests.filter(r => r.url.endsWith("events")).flatMap(r => r.body.events).filter(e => e.event_type === "chat.received");
  assert.equal(updated.length, 2);
  assert.equal(updated[1].nickname, "观众乙");
  assert.equal(updated[1].payload.content, "下一条评论");
  env.window.__liveArchiveModerationV1.dispose();
});


test('public screen counts new rows of every type once, baselines history, and ignores combo redraws',async()=>{
 const env=environment();env.source=env.source.replace('ticket:"ticket"','ticket:"ticket",timed_messages:{enabled:true,min_comments:2,count_scope:"public_screen"}');
 let observer;
 const row=(id,text)=>{const r=new env.context.HTMLElement();r.textContent=text;r.getAttribute=()=>id;r.matches=()=>true;r.querySelector=()=>null;return r;};
 const history=row('1','历史评论');
 const root={querySelectorAll:()=>[history]};env.context.document.querySelector=()=>root;
 env.context.MutationObserver=class{constructor(fn){observer=fn;}observe(){}disconnect(){}};
 vm.runInContext(env.source,env.context);
 const gift=row('2','观众送出鲜花 ×5'),own=row('3','场控自动文案'),system=row('4','系统提示');
 observer([{addedNodes:[gift,own,system]}]);gift.textContent='观众送出鲜花 ×6';observer([{target:gift,addedNodes:[]}]);
 observer([{addedNodes:[row('2','观众送出鲜花 ×6'),history]}]);await settle();
 const events=env.requests.filter(r=>r.url.endsWith('events')).flatMap(r=>r.body.events).filter(e=>e.event_type==='public.activity');
 assert.equal(events.length,3);assert.deepEqual(events.map(e=>e.native_event_id),['screen:2','screen:3','screen:4']);
 env.window.__liveArchiveModerationV1.dispose();
});
