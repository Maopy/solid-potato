// ==UserScript==
// @name         直播助手 · 抖音统一场控
// @namespace    live-archive.local
// @version      0.3.70
// @description  抖音直播页连接本机场控，支持场控开关、互动与录制。
// @match        https://live.douyin.com/*
// @match        https://www.douyin.com/*
// @match        https://douyin.com/*
// @run-at       document-start
// @noframes
// @sandbox      raw
// @grant        GM_xmlhttpRequest
// @grant        unsafeWindow
// @connect      127.0.0.1
// ==/UserScript==
(() => {
'use strict';
// Tampermonkey may fall back to an isolated world. Hook the page objects explicitly.
const window=typeof unsafeWindow==='undefined'?globalThis.window:unsafeWindow;
const {XMLHttpRequest,HTMLElement,MutationObserver,Event,ArrayBuffer,Uint8Array,Blob}=window;
if(window.top!==window.self||window.__liveArchiveUserscriptRouter)return;
window.__liveArchiveUserscriptRouter=true;
const roomForLocation=()=>{
  const host=new URL(location.href).hostname;
  const match=host==='live.douyin.com'?location.pathname.match(/^\/([0-9]{1,32})\/?$/):
    ['www.douyin.com','douyin.com'].includes(host)?location.pathname.match(/^\/(?:follow\/live|root\/live|live)\/([0-9]{1,32})\/?$/):null;
  return match?'https://live.douyin.com/'+match[1]:null;
};
let activeRoom=null,leaveRoom=null;
const runRoom=async(roomUrl)=>{
let earlyXhrRestore=()=>{};

const base=__BASE_JSON__,key=__KEY_JSON__;
let alive=true,ready=false,frameConsumer=null,metadataConsumer=null;
const socketCleanups=new Set(),frames=[],metadata=[];
let bufferedBytes=0;
const currentRoom=()=>alive&&roomForLocation()===roomUrl;
const userscriptCurrentRoom=currentRoom;
const earlyRelevant=url=>{try{const u=new URL(String(url),location.href);return /(^|\.)douyin\.com$/.test(u.hostname)&&/webcast|\/im\//.test(u.pathname);}catch(_){return false;}};
const OriginalSocket=window.WebSocket;
const earlySocket=new Proxy(OriginalSocket,{construct(target,args,newTarget){
  const socket=Reflect.construct(target,args,newTarget);
  if(earlyRelevant(args[0])&&socketCleanups.size<100){
    const listener=event=>{if(!currentRoom())return;const data=event.data,bytes=typeof data==='string'?data.length*2:data?.byteLength||data?.size||0;
      if(bytes>2097152)return;
      if(frameConsumer)frameConsumer(data,args[0]);
      else if(frames.length<64&&bufferedBytes+bytes<=2097152){frames.push([data,args[0]]);bufferedBytes+=bytes;}
    };
    const cleanup=()=>{socket.removeEventListener('message',listener);socketCleanups.delete(cleanup);};
    socket.addEventListener('message',listener);socket.addEventListener('close',cleanup,{once:true});socketCleanups.add(cleanup);
  }
  return socket;
}});
window.WebSocket=earlySocket;
const earlyOriginalFetch=window.fetch;
let earlyMetadataReaders=0;
const rememberMetadata=async(response,url)=>{
  if(earlyMetadataReaders>=4)return;earlyMetadataReaders++;
  let reader,timer;
  try{
    reader=response.clone().body?.getReader();if(!reader)return;
    timer=setTimeout(()=>void reader.cancel().catch(()=>{}),2000);
    let size=0;const chunks=[];
    while(true){const r=await reader.read();if(r.done)break;size+=r.value.length;if(size>2097152)return;chunks.push(r.value);}
    const bytes=new Uint8Array(size);let offset=0;for(const c of chunks){bytes.set(c,offset);offset+=c.length;}
    const value=JSON.parse(new TextDecoder().decode(bytes));
    if(!currentRoom())return;
    if(metadataConsumer)metadataConsumer(value,url);else if(metadata.length<4)metadata.push([value,url]);
  }catch(_){}finally{clearTimeout(timer);earlyMetadataReaders--;if(reader)void reader.cancel().catch(()=>{});}
};
const earlyFetch=function(...args){const result=Reflect.apply(earlyOriginalFetch,this,args);try{const url=new URL(args[0]?.url||args[0],location.href);if(/(^|\.)douyin\.com$/.test(url.hostname)&&url.pathname.includes('/webcast/room/web/enter')&&url.searchParams.get('web_rid')===new URL(roomUrl).pathname.slice(1))void result.then(r=>rememberMetadata(r,url.href)).catch(()=>{});}catch(_){}return result;};
window.fetch=earlyFetch;
const cleanupEarly=()=>{alive=false;earlyXhrRestore();for(const fn of Array.from(socketCleanups))fn();frames.length=0;metadata.length=0;if(window.WebSocket===earlySocket)window.WebSocket=OriginalSocket;if(window.fetch===earlyFetch)window.fetch=earlyOriginalFetch;document.getElementById('live-archive-connecting')?.remove();};
const earlyOpen=XMLHttpRequest.prototype.open,earlyXhrListeners=new Map();
const earlyXhr=function(...args){
  const result=Reflect.apply(earlyOpen,this,args);
  try{
    const url=new URL(args[1],location.href);
    if(/(^|\.)douyin\.com$/.test(url.hostname)&&url.pathname.includes('/webcast/room/web/enter')&&url.searchParams.get('web_rid')===new URL(roomUrl).pathname.slice(1)&&earlyXhrListeners.size<4){
      const xhr=this;
      const release=()=>{xhr.removeEventListener('load',load);xhr.removeEventListener('loadend',release);earlyXhrListeners.delete(xhr);};
      const load=()=>{try{if(!currentRoom())return;const raw=xhr.responseType==='json'?xhr.response:xhr.responseText;if(typeof raw==='string'&&raw.length>2097152)return;const value=typeof raw==='string'?JSON.parse(raw):raw;if(metadataConsumer)metadataConsumer(value,url.href);else if(metadata.length<4)metadata.push([value,url.href]);}catch(_){}finally{release();}};
      earlyXhrListeners.get(xhr)?.();earlyXhrListeners.set(xhr,release);xhr.addEventListener('load',load,{once:true});xhr.addEventListener('loadend',release,{once:true});
    }
  }catch(_){}return result;
};
XMLHttpRequest.prototype.open=earlyXhr;
earlyXhrRestore=()=>{if(XMLHttpRequest.prototype.open===earlyXhr)XMLHttpRequest.prototype.open=earlyOpen;for(const fn of Array.from(earlyXhrListeners.values()))fn();};
leaveRoom=()=>{window.__liveArchiveModerationV1?.dispose();cleanupEarly();delete window.__liveArchiveModerationV1;for(const id of ['live-archive-assistant','live-archive-recording'])document.getElementById(id)?.remove();};
window.addEventListener('pagehide',()=>{window.__liveArchiveModerationV1?.dispose();cleanupEarly();},{once:true});
const showConnecting=message=>{
  if(!document.body)return;
  let box=document.getElementById('live-archive-connecting');if(!box){box=document.createElement('div');box.id='live-archive-connecting';Object.assign(box.style,{position:'fixed',right:'20px',top:'16px',zIndex:2147483647,background:'#fff',color:'#222',padding:'12px',font:'14px sans-serif',border:'1px solid #ccc',borderRadius:'8px'});document.body.appendChild(box);}box.textContent=message;
};
document.addEventListener('DOMContentLoaded',()=>{if(!ready)showConnecting('直播助手 · 正在连接本机场控…');},{once:true});
const request=payload=>new Promise((resolve,reject)=>{
  if(!currentRoom()&&payload.endpoint!=='goodbye'){reject(Error('直播间已切换，请刷新页面'));return;}
  GM_xmlhttpRequest({method:'POST',url:base+'/v1/userscript',headers:{'Content-Type':'application/json',Authorization:'Bearer '+key},data:JSON.stringify({room_url:roomUrl,...payload}),timeout:2500,anonymous:true,
    onload:r=>{try{const value=JSON.parse(r.responseText);if(r.status!==200){const e=Error(value.error||'本机场控连接失败');e.status=r.status;reject(e);}else resolve(value);}catch(e){reject(e);}},
    onerror:()=>reject(Error('无法连接本机场控，请先双击“启动场控”')),ontimeout:()=>reject(Error('本机场控连接超时'))});
});
let BOOT;
while(currentRoom()){
  try{BOOT=await request({endpoint:'bootstrap'});break;}catch(e){showConnecting('直播助手 · '+e.message);await new Promise(resolve=>setTimeout(resolve,3000));}
}
if(!currentRoom()){cleanupEarly();return;}
if(BOOT.room_url!==roomUrl){showConnecting('直播助手 · 房间绑定不匹配，请刷新页面');cleanupEarly();return;}
if(window.__liveArchiveModerationV1){showConnecting('直播助手 · 检测到已有场控脚本，请停用旧代理注入后刷新');cleanupEarly();return;}
// Release constructors before installing the normal adapters; keep listeners on sockets opened during pairing.
if(window.WebSocket===earlySocket)window.WebSocket=OriginalSocket;
if(window.fetch===earlyFetch)window.fetch=earlyOriginalFetch;
// Existing XHR listeners retain in-flight room metadata during pairing.
if(XMLHttpRequest.prototype.open===earlyXhr)XMLHttpRequest.prototype.open=earlyOpen;
const userscriptTransport=(endpoint,body,session_token)=>request({endpoint,body,session_token});
const userscriptRecordingTransport=(action,body)=>{
  if(!session)throw Error('直播页尚未连接');
  return request({endpoint:'recording',action,body,session_token:session.token,scope:{v:1,connection_id:session.connection_id,binding_id:session.binding_id,page_url:BOOT.room_url}});
};
/*__SDK__*/
/*__RAW__*/
/*__SUBMIT__*/
/*__DECODER__*/
/*__ADAPTER__*/
/*__FILL__*/
if(BOOT.recording||BOOT.speech_recognition){/*__RECORDING__*/}
/*__PANEL__*/
metadataConsumer=(value,url)=>{if(currentRoom())douyinDecoder.bindRoom(value,url);};
frameConsumer=(data,url)=>{if(!currentRoom())return;sdk.captureRaw?.('websocket','message',data);if(BOOT.rules_preview)douyinDecoder.observe(data,url);};
for(const item of metadata.splice(0))metadataConsumer(...item);
for(const item of frames.splice(0))frameConsumer(...item);bufferedBytes=0;
ready=true;document.getElementById('live-archive-connecting')?.remove();
sdk.onDispose(cleanupEarly);
};
const route=()=>{const next=roomForLocation();if(next===activeRoom)return;leaveRoom?.();leaveRoom=null;activeRoom=next;if(next)void runRoom(next).catch(error=>console.error('[直播助手]',error));};
for(const name of ['pushState','replaceState'])if(window.history?.[name]){const original=window.history[name];window.history[name]=function(...args){const result=Reflect.apply(original,this,args);route();return result;};}
window.addEventListener('popstate',route);
const routeTimer=setInterval(route,500);
window.addEventListener('pagehide',()=>{clearInterval(routeTimer);leaveRoom?.();},{once:true});
route();
})();
