// Full SDK + decoder + adapter -> real loopback HTTP bridge -> Rust rules.
// Entirely synthetic room data; the socket's send() deliberately throws.
const vm = require('node:vm');
const fs = require('node:fs');
const zlib = require('node:zlib');
const assert = require('node:assert/strict');
const [bootstrapPath, address, mode] = process.argv.slice(2);
const emoji=mode==='auto_emoji', aliases=mode==='auto_alias';
const timed=mode==='auto_timed', timedSent=[];
const automatic=mode==='auto'||emoji||aliases||timed;
const namesOnly = mode === 'fill_names'||automatic;
const filling = mode === 'fill' || namesOnly;
const firstThanks=aliases?'配置：感谢霸气哥送来的玫瑰':emoji?'谢谢集成测试甲的玫瑰👩‍💻':namesOnly?'配置：感谢集成测试甲送来的玫瑰':'配置：感谢集成测试甲送来的玫瑰×3';
const secondThanks=aliases?'配置：感谢霸气哥送来的玫瑰':emoji?'谢谢集成测试甲的玫瑰👩‍💻':namesOnly?'配置：感谢集成测试甲送来的玫瑰':'配置：感谢集成测试甲送来的玫瑰×20';
const scheduled = mode === 'scheduled' || filling;
const sleep = ms => new Promise(r => setTimeout(r, ms));
const enc = new TextEncoder();
const vint = value => { let n = BigInt(value), out = []; while (n > 127n) { out.push(Number(n & 127n) | 128); n >>= 7n; } out.push(Number(n)); return out; };
const field = (key, value) => typeof value === 'bigint' || typeof value === 'number'
  ? [...vint(key * 8), ...vint(value)]
  : [...vint(key * 8 + 2), ...vint(value.length), ...value];
const pb = (...fields) => new Uint8Array(fields.flat());
const text = value => enc.encode(value);
const USER = '9007199254740993123', ROOM = '9007199254740993222', ANCHOR = '9007199254740993333';
const makeGift = (msg, qty, end = false, options = {}) => {
  const created = msg % 2n ? BigInt(Date.now()) : BigInt(Math.floor(Date.now()/1000));
  const common = pb(field(1, text('WebcastGiftMessage')), field(2, msg), field(3, BigInt(ROOM)), field(4, created));
  const user = pb(field(1, BigInt(options.user || USER)), field(3, text(options.nickname || '集成测试甲')));
  const target = pb(field(1, BigInt(ANCHOR)), field(3, text('测试主播')));
  const gift = pb(field(5, 101), field(10, 1), field(12, 1000), field(16, text('玫瑰')));
  const body = pb(field(1, common), field(2, 101), field(4, options.groupCount ?? 1), field(5, qty), field(6, options.comboCount ?? 0), field(7, user), field(8, target), field(9, end ? 1 : 0),
    ...(options.group === null ? [] : [field(11, options.group ?? 777)]), field(15, gift));
  const message = pb(field(1, text('WebcastGiftMessage')), field(2, body), field(3, msg + 99n));
  const response = pb(field(1, message));
  const bytes = pb(field(6, text('gzip')), field(7, text('msg')), field(8, zlib.gzipSync(response)));
  return options.messageOnly ? message : bytes.buffer;
};
const memberOrUpgrade = (msg, upgrade) => {
  const method = upgrade ? 'WebcastFansclubMessage' : 'WebcastMemberMessage';
  const metadata = pb(field(4, BigInt(ANCHOR)));
  const common = pb(field(1, text(method)), field(2, msg), field(3, BigInt(ROOM)), field(4, BigInt(Math.floor(Date.now()/1000))), field(16, metadata));
  const club = pb(field(1, pb(field(2, 3), field(6, BigInt(ANCHOR)))));
  const who = pb(field(1, 9007199254740993555n), field(3, text('集成测试乙')), field(24, club));
  const body = upgrade ? pb(field(1, common), field(2, 1), field(4, who)) : pb(field(1, common), field(2, who), field(10, 1));
  const message = pb(field(1, text(method)), field(2, body), field(3, msg));
  return pb(field(7, text('pb')), field(8, pb(field(1, message)))).buffer;
};
const requests = [], consolePreviews = [], consoleWarnings = [], dispatchTimes = [], fillLogs = [];
let deliverySocket;
let inputWrites = 0, receiptAttempts = 0, submissions=0;
const leaf = { _text:'\u200b', get textContent() { return this._text; }, set textContent(text) {this._text=text; inputWrites++;}, removeAttribute() {} };
const editor = { tagName:'DIV', isConnected:true, isContentEditable:true, get textContent() {return leaf.textContent;},
  querySelector:()=>null, querySelectorAll:()=>[leaf], getClientRects:()=>[{}], getAttribute:()=>null,
  closest:()=>null, contains:()=>false, matches:()=>false, dispatchEvent(event) {assert.equal(event.type,'input'); return true;},
  click() {throw Error('click forbidden');}, submit() {throw Error('submit forbidden');} };
const document = { querySelector:()=>null, querySelectorAll:()=>filling ? [editor] : [], activeElement:null, hidden:false,
  addEventListener() {}, removeEventListener() {} };
editor.ownerDocument = document;
const sendButton={tagName:'svg',namespaceURI:'http://www.w3.org/2000/svg',isConnected:true,ownerDocument:document,textContent:'',contains:()=>false,getClientRects:()=>[{}],closest:()=>null,matches:()=>false,
  dispatchEvent(event){assert.equal(event.type,'click');assert.equal(event.bubbles,true);submissions++;const content=leaf._text;timedSent.push(content);leaf._text='';if(!deliverySocket)return;
    const chat=(sender,stamp)=>{
      const msg=9007199254740993800n+BigInt(submissions*10)+sender%10n;
      const common=pb(field(1,text('WebcastChatMessage')),field(2,stamp===1n?msg+1000n:stamp>BigInt(Math.floor(Date.now()/1000)+5)?msg+2000n:msg),field(3,BigInt(ROOM)),field(4,stamp));
      const body=pb(field(1,common),field(2,pb(field(1,sender),field(3,text('发送账号')))),field(3,text(content)));
      return pb(field(7,text('pb')),field(8,pb(field(1,pb(field(1,text('WebcastChatMessage')),field(2,body),field(3,msg)))))).buffer;
    };
    deliverySocket.dispatch(chat(123n,BigInt(Math.floor(Date.now()/1000))));
    deliverySocket.dispatch(chat(9007199254740993999n,1n));
    deliverySocket.dispatch(chat(9007199254740993999n,BigInt(Math.floor(Date.now()/1000)+3600)));
    setTimeout(()=>deliverySocket.dispatch(chat(9007199254740993999n,BigInt(Math.floor(Date.now()/1000)))),20);
  }};
if(automatic)editor.closest=selector=>selector.includes('webcast-chatroom')?{querySelectorAll:()=>[sendButton]}:null;
class Socket {
  constructor(url) { this.url = url; this.listeners = new Map(); }
  addEventListener(name, fn) { this.listeners.set(name, fn); }
  removeEventListener(name) { this.listeners.delete(name); }
  dispatch(data) { this.listeners.get('message')?.({data}); }
  send() { throw Error('platform send is forbidden in observation mode'); }
}
class Xhr extends Socket { open() {} getResponseHeader() { return ''; } }
const browser = { fetch: async (url, options) => {
  assert.ok(String(url).startsWith('/__live_archive/moderation/v1/'), 'no platform HTTP request');
  const endpoint = String(url).replace('/__live_archive/moderation/v1/', '/v1/');
  const body = JSON.parse(options.body); requests.push({ endpoint, body });
  if (filling && endpoint === '/v1/results' && ++receiptAttempts === 1) throw Error('synthetic lost receipt');
  return fetch('http://' + address + endpoint, {...options, headers: {...options.headers, Origin:'https://live.douyin.com'}});
}, WebSocket: Socket, addEventListener() {}, removeEventListener() {}, console: { info(...args) {
  if (args[0] === '[Live Archive preview]' || args[0] === '[Live Archive dispatch preview]') {
    consolePreviews.push(args); dispatchTimes.push(Date.now());
  }
  if (args[0] === '[Live Archive fill]') fillLogs.push(args);
}, warn(...args) { consoleWarnings.push(args); } } };
browser.MouseEvent=class extends Event{};
browser.self = browser; browser.top = browser;
browser.getComputedStyle = () => ({visibility:'visible',display:'block',opacity:'1'});
const context = vm.createContext({ window: browser, XMLHttpRequest:Xhr, location:{origin:'https://live.douyin.com',pathname:'/123',href:'https://live.douyin.com/123'},
  document, performance, Event, crypto:require('node:crypto').webcrypto, AbortController, URL, TextDecoder, TextEncoder,
  atob,btoa,Blob, DecompressionStream, ArrayBuffer, Uint8Array, setTimeout, clearTimeout, setInterval, clearInterval, HTMLElement:class {}, MutationObserver:class {} });
(async () => {
  vm.runInContext(fs.readFileSync(bootstrapPath,'utf8'), context);
  if(timed){
    const until=async f=>{const end=Date.now()+18000;while(!f()&&Date.now()<end)await sleep(100);assert.ok(f(),'timed condition timeout');};
    await until(()=>submissions===2);await until(()=>browser.__liveArchiveModerationV1.getStatus().fill.bridge?.submitted===2);
    assert.equal(inputWrites,2);assert.notEqual(timedSent[0],timedSent[1]);assert.ok(timedSent.every(t=>['定时文案甲','定时文案乙'].includes(t)));
    browser.__liveArchiveModerationV1.dispose();await sleep(200);process.stdout.write(JSON.stringify({submissions,input_writes:inputWrites,send_chat:true})+'\n');return;
  }
  const socket = new browser.WebSocket('wss://webcast.example.douyin.com/webcast/im/push/v2/?room_id='+ROOM);
  deliverySocket=socket;
  socket.dispatch(makeGift(9007199254740993444n, 1));
  await sleep(700);
  socket.dispatch(makeGift(9007199254740993445n, 3));
  await sleep(700);
  // Duplicate cumulative and end packets must not push the timer forward.
  socket.dispatch(makeGift(9007199254740993446n, 3));
  socket.dispatch(makeGift(9007199254740993447n, 2));
  socket.dispatch(makeGift(9007199254740993448n, 3, true));
  await sleep(filling ? 8500 : 5700);
  const status = browser.__liveArchiveModerationV1.getStatus();
  assert.equal(status.send_chat, automatic);
  assert.equal(status.bridge_error, null);
  assert.equal(status.rules.paused, false);
  assert.equal(status.rules.normalized_events, 5);
  if (scheduled) {
    assert.equal(status.rules.mode, 'online_scheduler_preview');
    assert.equal(status.rules.scheduler.dispatch_previews, 1);
    assert.equal(status.rules.scheduler.min_interval_ms, 8000);
    assert.equal(consolePreviews[0][0], '[Live Archive dispatch preview]');
  }
  const planned = status.previews.filter(x => x.kind === 'planned');
  assert.equal(planned.length, 1);
  assert.equal(planned[0].action.text, firstThanks);
  assert.equal(planned[0].action.user_id, USER);
  assert.equal(planned[0].action.scope.room_id, ROOM);
  assert.equal(planned[0].action.scope.recipient_user_id, ANCHOR);
  assert.ok(planned[0].action.created_at < 6500, 'no postponement by duplicate or end packets');
  assert.equal(requests.some(r => r.endpoint.includes('send')), false);
  if (filling) {
    assert.equal(leaf.textContent, automatic?'':firstThanks);
    assert.equal(status.fill.last_result.outcome, automatic?'submitted':'filled');
    assert.equal(status.fill.pending_result, false);
    assert.ok(receiptAttempts >= 2, 'receipt retried after network failure');
    assert.equal(inputWrites, 1, 'receipt retry never repeats input write');
  }
  // An unsupported gift affects only its user's incomplete window. A valid
  // multi-item gift later in the SAME frame must still reach the bridge.
  const other = {user:'9007199254740993555', nickname:'集成测试乙'};
  socket.dispatch(makeGift(9007199254740993500n, 1, false, {...other, group:800}));
  await sleep(700);
  const unsupported = makeGift(9007199254740993501n, 10, false, {...other, group:null, groupCount:10, messageOnly:true});
  const grouped = makeGift(9007199254740993502n, 10, false, {group:778, groupCount:10, comboCount:1, messageOnly:true});
  socket.dispatch(pb(field(7, text('msg')), field(8, pb(field(1, unsupported), field(1, grouped)))).buffer);
  await sleep(700);
  socket.dispatch(makeGift(9007199254740993503n, 20, false, {group:778, groupCount:10, comboCount:2}));
  await sleep(700);
  socket.dispatch(makeGift(9007199254740993504n, 20, true, {group:778, groupCount:10, comboCount:2}));
  await sleep(scheduled ? 7500 : 5200);
  const continued = browser.__liveArchiveModerationV1.getStatus();
  assert.equal(continued.rules.paused, false);
  assert.equal(continued.rules.skipped_messages, 1);
  assert.ok(consoleWarnings.some(x => x[0] === '[Live Archive skipped gift]'));
  assert.deepEqual(consolePreviews.map(x => x[1]), [firstThanks, secondThanks]);
  if (scheduled) {
    assert.ok(dispatchTimes[1] - dispatchTimes[0] >= 7900, 'rate limited dispatch reaches SDK console');
    assert.equal(continued.rules.scheduler.dispatch_previews, 2);
    assert.equal(continued.rules.scheduler.pending, 0);
  }
  assert.ok(continued.previews.some(x => x.kind === 'cancelled' && x.reason === 'unsupported_gift_in_user_window'));
  if (filling) {
    assert.equal(leaf.textContent, automatic?'':firstThanks, 'later action cannot overwrite existing text');
    assert.equal(inputWrites, automatic?2:1);
    assert.deepEqual(fillLogs.map(x => x.slice(1)), automatic?[['submitted','input_submitted'],['submitted','input_submitted']]:namesOnly?[['filled','input_filled']]:[['filled','input_filled'], ['rejected','input_busy']]);
  }
  if(automatic)assert.equal(submissions,2);
  socket.dispatch(memberOrUpgrade(9007199254740993666n, false));
  socket.dispatch(memberOrUpgrade(9007199254740993667n, true));
  await sleep(1200);
  const actions = browser.__liveArchiveModerationV1.getStatus().previews;
  assert.ok(actions.some(x => x.kind === 'accepted' && x.reason === 'welcome_batched'));
  assert.ok(!actions.some(x => x.kind === 'planned' && x.action.kind === 'welcome'), 'arrival waits for the 30-second batch');
  assert.ok(actions.some(x => x.kind === 'planned' && x.action.text === '恭喜集成测试乙的粉丝团升级到3级'));
  assert.ok(actions.some(x => x.kind === 'cancelled' && x.reason === 'higher_priority_interaction'));
  // Bad transport input must cancel pending work and latch the pause.
  socket.dispatch(new Uint8Array([255]).buffer);
  await sleep(1200);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().rules.paused, true);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().rules.last_reason, 'protobuf_truncated');
  if(automatic){const status=browser.__liveArchiveModerationV1.getStatus();assert.equal(status.delivery_confirmation,undefined,'submission does not wait for a platform echo');assert.equal(status.fill.bridge.submitted,2);}
  browser.__liveArchiveModerationV1.dispose();
  await sleep(200);
  process.stdout.write(JSON.stringify({integration:'sdk_decoder_http_rules',fill_only:filling,scheduler_preview:scheduled,text:planned[0].action.text,exact_user_id:planned[0].action.user_id,send_chat:automatic,welcome_and_upgrade:true,continuous_grouped_previews:true,isolated_unsupported_gift:true,paused_on_gap:true})+'\n');
})().catch(e => { browser.__liveArchiveModerationV1?.dispose(); console.error(e); process.exitCode=1; });
