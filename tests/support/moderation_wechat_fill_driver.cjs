// Complete injected SDK -> local HTTP bridge. No platform requests or chat writes.
const fs = require('node:fs'), vm = require('node:vm'), assert = require('node:assert/strict');
const [bootstrap, address, editorKind] = process.argv.slice(2);
let loseAckNext = false, lostAcks = 0;
const origin = 'https://channels.weixin.qq.com', room = origin + '/web/pages/live';
const data = {data:{msgList:[{gift:{giftId:'private-gift-id',giftName:'private-gift-name'},
  username:'private-user-id',nickname:'private-nickname',count:2}],cookie:'private-cookie'}};
const requests = [], listeners = new Map();
const bus = {on(name, fn) {listeners.set(name, fn);},off(name, fn) {if(listeners.get(name) === fn) listeners.delete(name);}};
class Socket {
  constructor(url) {this.url=url;this.listeners=new Map();}
  addEventListener(name,fn) {this.listeners.set(name,fn);}
  removeEventListener(name,fn) {if(this.listeners.get(name) === fn) this.listeners.delete(name);}
  dispatch(name,event) {this.listeners.get(name)?.(event);}
  send() {throw Error('platform send forbidden');}
}
class Xhr extends Socket {open() {} getResponseHeader() {return 'application/json';}}
const originalOpen=Xhr.prototype.open;
const originalFetch = async (url, options) => {
  if (String(url).startsWith('/__live_archive/moderation/v1/')) {
    requests.push(JSON.parse(options.body));
    const response = await fetch('http://'+address+String(url).replace('/__live_archive/moderation/v1/', '/v1/'),
      {...options, headers:{...options.headers,Origin:origin}});
    if (String(url).endsWith('/events') && loseAckNext && response.ok) {
      loseAckNext=false;lostAcks++;return new Response('{}',{status:503});
    }
    return response;
  }
  assert.ok([origin+'/synthetic-local-fixture', 'https://finder.video.qq.com/synthetic-local-fixture'].includes(url));
  return new Response(JSON.stringify(data), {headers:{'content-type':'application/json'}});
};
const browser = {fetch:originalFetch,WebSocket:Socket,WXE:bus,addEventListener(){},removeEventListener(){},console,
  innerWidth:1200,innerHeight:900,getComputedStyle:()=>({visibility:'visible',display:'block',opacity:'1'})};
let nativeCalls=0;
const nativeThis={native:true}, nativeResult={preserved:true};
const nativeInvoke=function(name,args,callback) {
  nativeCalls++;assert.equal(this,nativeBridge);assert.ok(['synthetic','getLiveMessage'].includes(name));assert.equal(args,data);
  callback.call(nativeThis,data,'unchanged');return nativeResult;
};
const nativeDispatch=function(value) {assert.equal(this,nativeBridge);return value;};
const nativeSubscriptions=new Map();let nativeRegistrations=0;
const nativeOn=function(name,callback) {assert.equal(this,nativeBridge);nativeRegistrations++;nativeSubscriptions.set(name,callback);return nativeResult;};
const nativeOff=function(name,callback) {assert.equal(this,nativeBridge);if(nativeSubscriptions.get(name)===callback)nativeSubscriptions.delete(name);return nativeResult;};
const nativeBridge={invoke:nativeInvoke,on:nativeOn,off:nativeOff,_handleMessageFromWeixin:nativeDispatch};
Object.defineProperty(nativeBridge,'call',{value:nativeInvoke,writable:false});
browser.WeixinJSBridge=nativeBridge;
browser.self=browser; browser.top=browser;
const location={origin,pathname:'/web/pages/live',href:room+'?sensitive=query'};
class Element {
  constructor(tag) {this.tagName=tag.toUpperCase();this.children=[];this.style={};this.attributes={};this.listeners=new Map();this.hidden=false;this.disabled=false;this.textContent='';}
  setAttribute(key,value) {this.attributes[key]=value;}
  appendChild(node) {node.parent=this;this.children.push(node);return node;}
  addEventListener(name,fn) {this.listeners.set(name,fn);}
  removeEventListener(name,fn) {if(this.listeners.get(name)===fn)this.listeners.delete(name);}
  click() {if(!this.disabled)this.listeners.get('click')?.();}
  remove() {if(this.parent)this.parent.children=this.parent.children.filter(node=>node!==this);}
}
const domListeners=new Map();
const pageTextNodes=[];
const pageText=(text,excluded=false,hidden=false)=>({nodeValue:text,parentElement:{
  closest:()=>excluded?{}:null,
  getBoundingClientRect:()=>({width:hidden?0:200,height:20,top:50,left:50,bottom:70,right:250})
}});
const document={body:new Element('body'),readyState:'loading',createElement:tag=>new Element(tag),
  createTreeWalker(){let i=0;const nodes=[...pageTextNodes];return {nextNode:()=>nodes[i++]||null};},
  addEventListener(name,fn) {domListeners.set(name,fn);},removeEventListener(name,fn) {if(domListeners.get(name)===fn)domListeners.delete(name);},
  getElementById(id) {const find=node=>node.id===id?node:node.children.map(find).find(Boolean);return find(this.body)||null;}};
const context=vm.createContext({window:browser,location,XMLHttpRequest:Xhr,performance,
  document, Event,
  crypto:require('node:crypto').webcrypto,URL,TextDecoder,TextEncoder,AbortController,atob:globalThis.atob,btoa:globalThis.btoa,ArrayBuffer,Blob,
  Uint8Array,setTimeout,clearTimeout,setInterval,clearInterval});

let writes=0,receipts=0;
browser.console={info(){},warn(){}};
class Textarea {constructor(){this._text='';}get value(){return this._text;}set value(v){this._text=v;writes++;}}
const editor=editorKind==='textarea'?new Textarea():{_text:'',get textContent(){return this._text;},set textContent(v){this._text=v;writes++;}};
Object.assign(editor,{tagName:editorKind==='textarea'?'TEXTAREA':'DIV',isContentEditable:editorKind!=='textarea',isConnected:true,ownerDocument:document,
  getClientRects:()=>[{}],closest:()=>null,matches:()=>false,contains:()=>false,querySelector:()=>null,
  dispatchEvent:event=>{assert.equal(event.type,'input','no keyboard/change/submit events');return true;},
  click:()=>{throw Error('editor clicks forbidden');},submit:()=>{throw Error('submission forbidden');}});
browser.HTMLTextAreaElement=Textarea;
document.hidden=false;document.activeElement=editor;document.querySelectorAll=()=>[editor];
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const until=async test=>{const deadline=Date.now()+11000;while(!test()&&Date.now()<deadline)await sleep(100);assert.ok(test(),'condition timeout');};
(async()=>{
  vm.runInContext(fs.readFileSync(bootstrap,'utf8'),context);
  document.readyState='complete';domListeners.get('DOMContentLoaded')();
  await until(()=>browser.__liveArchiveModerationV1.getStatus().connected);
  document.getElementById('live-archive-moderation-entry').click();
  const toggle=document.getElementById('live-archive-moderation-toggle');assert.equal(toggle.disabled,false);toggle.click();
  const gift=(id,user,target='fixture-recipient',name='小星星')=>({msg_type:20009,client_msg_id:id,
    from_user_contact:{contact:{username:user,nickname:'集成测试观众'}},to_user_contact:{contact:{username:target}},
    payload:JSON.stringify({reward_product_id:'fixture-product',reward_product_count:47,reward_gift:{name}})});
  const deliver=list=>nativeBridge._handleMessageFromWeixin({app_msg_list:list});
  deliver([gift('g1','first-user'),gift('g1','first-user'),gift('other','other-user','other-target')]);
  await until(()=>writes===1);
  assert.equal(editor._text,'感谢集成测试观众送来的小星星');assert.ok(!editor._text.includes('×47'));
  await until(()=>browser.__liveArchiveModerationV1.getStatus().fill.last_result?.outcome==='filled');
  const preserved=editor._text;
  const anonymousGift=gift('g2','second-user','fixture-recipient','玫瑰');
  anonymousGift.from_user_contact.contact.nickname='***';
  anonymousGift.from_user_contact.badge_infos=[{badge_type:10,badge_name:'神秘观众 179'}];
  deliver([anonymousGift]);
  await sleep(6500);assert.equal(writes,1);assert.equal(editor._text,preserved,'existing text cannot be overwritten');
  // Simulate the human clearing/submitting outside this software; no send call.
  editor._text='';
  await until(()=>writes===2);assert.equal(editor._text,'感谢神秘观众179送来的玫瑰');
  await until(()=>browser.__liveArchiveModerationV1.getStatus().fill.last_result?.outcome==='filled');
  deliver([gift('g3','third-user')]);toggle.click();
  await sleep(5500);assert.equal(writes,2);assert.equal(editor._text,'感谢神秘观众179送来的玫瑰');
  const state=browser.__liveArchiveModerationV1.getStatus();assert.equal(state.send_chat,false);assert.equal(state.fill.send_chat,false);
  assert.equal(state.candidate_preview.paused,true);
  browser.__liveArchiveModerationV1.dispose();await sleep(300);
  process.stdout.write(JSON.stringify({writes,editor:editorKind,send_chat:false,preserved_input:true,manual_clear_continues:true,paused:true})+'\n');
})().catch(e=>{console.error(e);process.exitCode=1;browser.__liveArchiveModerationV1?.dispose();});
