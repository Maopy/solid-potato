const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
function bundle(){let source=fs.readFileSync('assets/moderation/userscript/douyin.user.js','utf8').replace('__BASE_JSON__','"http://127.0.0.1:2025"').replace('__KEY_JSON__','"private-pairing-key"');
for(const [key,path] of Object.entries({SDK:'sdk/probe',RAW:'sdk/raw',SUBMIT:'sdk/submit',DECODER:'adapters/douyin-decoder',ADAPTER:'adapters/douyin-live',FILL:'adapters/douyin-fill',RECORDING:'adapters/douyin-recording',PANEL:'sdk/panel'}))source=source.replace('/*__'+key+'__*/',fs.readFileSync('assets/moderation/'+path+'.js','utf8'));return source;}
function setup(pathname='/123',origin='https://live.douyin.com',isolated=false,options={}){
 const requests=[],timers=new Map();let next=0,resolveBoot,platformCalls=0,helloAttempts=0;const listeners=new Map();
 class Node{constructor(tag){this.tagName=tag;this.children=[];this.style={};this.attrs={};this.textContent='';}appendChild(n){this.children.push(n);if(this.tagName==='select'&&!this.value)this.value=n.value;return n;}append(...ns){for(const n of ns)this.appendChild(n);}setAttribute(k,v){this.attrs[k]=v;}remove(){this.removed=true;}replaceChildren(...ns){this.children=ns;}}
 const body=new Node('body'),find=(n,id)=>!n.removed&&(n.id===id?n:n.children.map(n=>find(n,id)).find(Boolean));
 const document={body,readyState:'complete',title:'主播',getElementById:id=>find(body,id),createElement:t=>new Node(t),querySelector:()=>null,querySelectorAll:()=>[],addEventListener(){},removeEventListener(){}};
 class Socket{static OPEN=1;constructor(url){this.url=url;this.listeners=new Map();}addEventListener(k,fn){if(!this.listeners.has(k))this.listeners.set(k,new Set());this.listeners.get(k).add(fn);}removeEventListener(k,fn){this.listeners.get(k)?.delete(fn);}dispatch(k,value){for(const fn of this.listeners.get(k)||[])fn(value);}}
 class Xhr extends Socket{open(){}getResponseHeader(){return 'application/json';}}
 const boot={v:1,room_url:'https://live.douyin.com/'+pathname.split('/').filter(Boolean).at(-1),prefix:'/local/',ticket:'ticket',adapter_id:'douyin.live_page',moderation_enabled:false,control_epoch:0,raw_messages:true,recording:true,unified_ui:true};
 const context={document,location:{origin,pathname,href:origin+pathname},performance:{now:()=>0,getEntriesByType:()=>[{name:'https://media.example.test/current.flv'}]},crypto:require('node:crypto').webcrypto,URL,AbortController,TextDecoder,TextEncoder,Uint8Array,ArrayBuffer,Blob,atob,btoa,console,HTMLElement:Node,MutationObserver:class{observe(){}disconnect(){}},XMLHttpRequest:Xhr,WebSocket:Socket,fetch:async()=>{platformCalls++;return {headers:{get:()=>null},clone:()=>({body:null})};},
 setTimeout:(fn,ms)=>{const id=++next;timers.set(id,{fn,ms});return id;},clearTimeout:id=>timers.delete(id),setInterval:(fn,ms)=>{const id=++next;timers.set(id,{fn,ms,interval:true});return id;},clearInterval:id=>timers.delete(id),addEventListener:(name,fn)=>{if(!listeners.has(name))listeners.set(name,new Set());listeners.get(name).add(fn);},removeEventListener:(name,fn)=>listeners.get(name)?.delete(fn)};
 const storage=options.storage||new Map();context.sessionStorage={getItem:k=>storage.get(k)||null,setItem:(k,v)=>storage.set(k,v)};
 context.window=context;context.top=context;context.self=context;
 context.GM_xmlhttpRequest=o=>{const payload=JSON.parse(o.data);requests.push({o,payload});const reply=v=>o.onload({status:200,responseText:JSON.stringify(v)});
 if(payload.endpoint==='bootstrap'){resolveBoot=()=>reply(boot);return;}
 if(payload.endpoint==='hello'&&options.denyHelloOnce&&helloAttempts++===0){o.onload({status:422,responseText:JSON.stringify({error:'pairing denied'})});return;}
 if(payload.endpoint==='hello')reply({connection_id:'conn',binding_id:'binding',token:'session-key'});
 else if(payload.endpoint==='control')reply({enabled:payload.body.enabled,epoch:1});
 else if(payload.endpoint==='events')reply({acked_seq:payload.body.events.at(-1).seq});
 else if(payload.endpoint==='raw')reply({acked_seq:payload.body.chunks.at(-1).seq});
 else if(payload.endpoint==='recording')reply(payload.action==='list'?{list:[]}:{task_id:'recording'});
 else reply({commands:[],moderation_control:{enabled:false,epoch:0}});
 };
 if(isolated){context.unsafeWindow={...context};context.unsafeWindow.top=context.unsafeWindow;context.unsafeWindow.self=context.unsafeWindow;}
 vm.createContext(context);vm.runInContext(bundle(),context);return {context,requests,timers,listeners,Socket,body,boot,resolveBoot:()=>resolveBoot?.(),get platformCalls(){return platformCalls;}};
}
const settle=()=>new Promise(r=>setImmediate(r));
test('bundled userscript captures sockets during pairing and uses GM transport for controls and recording',async()=>{
 const e=setup();assert.equal(e.requests[0].payload.endpoint,'bootstrap');
 const socket=new e.context.WebSocket('wss://webcast.example.douyin.com/webcast/im/push?room_id=123');
 socket.dispatch('message',{data:new Uint8Array([1,2]).buffer});
 e.resolveBoot();await settle();await settle();
 const sdk=e.context.__liveArchiveModerationV1;assert.ok(sdk);assert.equal(sdk.getStatus().connected,true);assert.equal(sdk.getStatus().moderation_enabled,false);
 assert.equal(sdk.rawStatus().captured,1,'early frame retained');
 socket.dispatch('message',{data:new Uint8Array([3,4]).buffer});assert.equal(sdk.rawStatus().captured,2,'existing socket stays observed');
 const later=new e.context.WebSocket('wss://webcast.example.douyin.com/webcast/im/push?room_id=123');later.dispatch('message',{data:new Uint8Array([5,6]).buffer});assert.equal(sdk.rawStatus().captured,3,'new socket is not observed twice');
 await sdk.setModerationEnabled(true);assert.equal(sdk.getStatus().moderation_enabled,true);
 await sdk.startRecording();const rec=e.requests.find(r=>r.payload.endpoint==='recording'&&r.payload.action==='create');assert.ok(rec);assert.equal(rec.payload.body.objects[0].content.profile.room_url,'https://live.douyin.com/123');assert.equal(rec.payload.session_token,'session-key');
 assert.equal(e.platformCalls,0,'local API traffic never goes through the live website');
 assert.ok(e.requests.every(r=>r.o.url==='http://127.0.0.1:2025/v1/userscript'&&r.o.headers.Authorization==='Bearer private-pairing-key'));
 sdk.dispose();const captured=sdk.rawStatus().captured;socket.dispatch('message',{data:new Uint8Array([7]).buffer});assert.equal(sdk.rawStatus().captured,captured);assert.equal(e.context.WebSocket,e.Socket);
});
test('non-room pages are ignored and navigation during pairing never binds stale events',async()=>{
 const landing=setup('/');assert.equal(landing.requests.length,0);assert.equal(landing.context.__liveArchiveModerationV1,undefined);
 const e=setup();e.context.location.pathname='/456';e.resolveBoot();await settle();assert.equal(e.context.__liveArchiveModerationV1,undefined);assert.equal(e.context.WebSocket,e.Socket);assert.equal(e.requests.length,1);
});
test('same-document duplicate script load keeps exactly one connection',async()=>{
 const e=setup();vm.runInContext(bundle(),e.context);assert.equal(e.requests.length,1);e.resolveBoot();await settle();assert.equal(e.requests.filter(r=>r.payload.endpoint==='hello').length,1);e.context.__liveArchiveModerationV1.dispose();
});

test('isolated userscript world hooks the actual page WebSocket',async()=>{
 const e=setup('/123','https://live.douyin.com',true),page=e.context.unsafeWindow;
 const socket=new page.WebSocket('wss://webcast.example.douyin.com/webcast/im/push?room_id=123');
 socket.dispatch('message',{data:new Uint8Array([1,2]).buffer});e.resolveBoot();await settle();await settle();
 assert.ok(page.__liveArchiveModerationV1);assert.equal(page.__liveArchiveModerationV1.rawStatus().captured,1);
 assert.equal(e.context.__liveArchiveModerationV1,undefined,'page and sandbox globals remain distinct');
 page.__liveArchiveModerationV1.dispose();
});
test('follow/root live routes use canonical room binding including recording',async()=>{
 for(const prefix of ['/follow/live/','/root/live/','/live/']){
  const e=setup(prefix+'123','https://www.douyin.com');assert.equal(e.requests[0].payload.room_url,'https://live.douyin.com/123');
  e.resolveBoot();await settle();await settle();const sdk=e.context.__liveArchiveModerationV1;
  assert.equal(sdk.getStatus().connected,true);await sdk.startRecording();
  assert.equal(e.requests.find(r=>r.payload.action==='create').payload.body.objects[0].content.profile.room_url,'https://live.douyin.com/123');sdk.dispose();
 }
});
test('homepage SPA navigation starts observation and switching rooms disposes old SDK',async()=>{
 const e=setup('/','https://www.douyin.com');assert.equal(e.requests.length,0);
 const route=[...e.timers.values()].find(t=>t.interval&&t.ms===500).fn;
 e.context.location.pathname='/follow/live/123';e.context.location.href=e.context.location.origin+e.context.location.pathname;
 e.boot.room_url='https://live.douyin.com/123';route();e.resolveBoot();await settle();await settle();
 const first=e.context.__liveArchiveModerationV1;assert.equal(first.getStatus().connected,true);
 e.context.location.pathname='/follow/live/456';e.context.location.href=e.context.location.origin+e.context.location.pathname;
 e.boot.room_url='https://live.douyin.com/456';route();assert.equal(first.getStatus().stopped,true);
 e.resolveBoot();await settle();await settle();assert.notEqual(e.context.__liveArchiveModerationV1,first);assert.equal(e.context.__liveArchiveModerationV1.getStatus().connected,true);
 e.context.__liveArchiveModerationV1.dispose();
});
test('XHR room identity loaded during pairing is retained for the decoder',async()=>{
 const e=setup(),xhr=new e.context.XMLHttpRequest();
 e.context.performance.getEntriesByType=()=>[];
 xhr.open('GET','https://live.douyin.com/webcast/room/web/enter/?web_rid=123');
 xhr.responseType='json';xhr.response={data:{data:[{id_str:'789',owner:{id_str:'456'},stream_url:{flv_pull_url:{FULL_HD1:'https://media.example.test/from-early-xhr.flv'}}}]}};xhr.dispatch('load',{});
 e.resolveBoot();await settle();await settle();assert.equal(e.context.__liveArchiveModerationV1.getStatus().decoder.room_bound,true);
 await e.context.__liveArchiveModerationV1.startRecording();
 assert.equal(e.requests.find(r=>r.payload.action==='create').payload.body.objects[0].content.live.stream_url,'https://media.example.test/from-early-xhr.flv','recording retains XHR stream metadata even with a blob player and empty timing buffer');
 e.context.__liveArchiveModerationV1.dispose();
});
test('isolated page socket produces native live comments without any platform writes',async()=>{
 const e=setup('/123','https://live.douyin.com',true);e.boot.rules_preview=true;
 const integer=n=>{n=BigInt(n);const out=[];while(n>127n){out.push(Number(n&127n)|128);n>>=7n;}out.push(Number(n));return out;};
 const field=(k,v)=>typeof v==='number'||typeof v==='bigint'?[...integer(k*8),...integer(v)]:[...integer(k*8+2),...integer(v.length),...v];
 const pb=(...values)=>new Uint8Array(values.flat()),txt=s=>new TextEncoder().encode(s);
 const common=pb(field(1,txt('WebcastChatMessage')),field(2,1001),field(3,789),field(4,Math.floor(Date.now()/1000)));
 const body=pb(field(1,common),field(2,pb(field(1,12345),field(3,txt('模拟观众')))),field(3,txt('胎记')));
 const message=pb(field(1,txt('WebcastChatMessage')),field(2,body),field(3,1001));
 const frame=pb(field(6,txt('pb')),field(7,txt('msg')),field(8,pb(field(1,message))));
 const socket=new e.context.unsafeWindow.WebSocket('wss://webcast.example.douyin.com/webcast/im/push?room_id=789');
 socket.send=()=>{throw Error('real platform writes forbidden');};
 socket.dispatch('message',{data:frame.buffer});e.resolveBoot();await settle();await settle();await settle();
 const status=e.context.unsafeWindow.__liveArchiveModerationV1.getStatus();
 assert.equal(status.decoder.decoded,1);assert.equal(status.delivery.observed['chat.received'],1);assert.equal(status.delivery.generated['chat.received'],undefined,'off mode does not enqueue automation events');assert.equal(status.moderation_enabled,false);
 assert.ok(!e.requests.some(r=>r.payload.endpoint==='results'),'no comment command executed');e.context.unsafeWindow.__liveArchiveModerationV1.dispose();
});


test('pagehide releases the session before early hooks become inactive, and navigation still permits goodbye',async()=>{
 for(const navigation of [false,true]){
  const e=setup();e.resolveBoot();await settle();await settle();
  if(navigation){e.context.location.pathname='/456';e.context.__liveArchiveModerationV1.dispose();}
  else for(const fn of Array.from(e.listeners.get('pagehide')||[]))fn();
  assert.equal(e.requests.filter(r=>r.payload.endpoint==='goodbye').length,1);
 }
});
test('refresh pairing contention retries quietly with the same ticket on an old backend',async()=>{
 const e=setup('/123','https://live.douyin.com',false,{denyHelloOnce:true});e.resolveBoot();await settle();await settle();
 const sdk=e.context.__liveArchiveModerationV1;assert.equal(sdk.getStatus().stopped,false);assert.equal(sdk.getStatus().bridge_error,null);
 const timer=[...e.timers.entries()].find(([,t])=>!t.interval&&t.ms===1000&&t.fn.name==='tick');assert.ok(timer);e.timers.delete(timer[0]);await timer[1].fn();
 assert.equal(sdk.getStatus().connected,true);const hello=e.requests.filter(r=>r.payload.endpoint==='hello');assert.equal(hello.length,2);assert.equal(hello[0].payload.body.ticket,hello[1].payload.body.ticket);sdk.dispose();
});
test('same-tab reload uses a stable client id only when supported by backend',async()=>{
 const storage=new Map(),ids=[];
 for(let i=0;i<2;i++){
  const e=setup('/123','https://live.douyin.com',false,{storage});e.boot.reload_handoff=true;e.resolveBoot();await settle();await settle();
  ids.push(e.requests.find(r=>r.payload.endpoint==='hello').payload.body.client_id);e.context.__liveArchiveModerationV1.dispose();
 }
 assert.match(ids[0],/^[a-f0-9-]{36}$/);assert.equal(ids[0],ids[1]);
});
