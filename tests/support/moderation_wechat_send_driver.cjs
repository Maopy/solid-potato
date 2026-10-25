// Complete injected SDK -> local HTTP bridge. No platform requests or chat writes.
const fs = require('node:fs'), vm = require('node:vm'), assert = require('node:assert/strict');
const [bootstrap, address, editorKind, scenario] = process.argv.slice(2);
let deliveryAttempts=0, savedDelivery, waitedWhileBlocked=false;
const crossingZero=scenario==='delivery_crossing_zero_ack';
const crossing=scenario==='delivery_crossing_poll'||crossingZero;
const recovering=scenario==='delivery_recover'||crossing, keepBlocked=scenario==='delivery_blocked';
const crossingArgs={};let crossingCallback=null;
const neverRecover=scenario==='unknown'||keepBlocked;
let loseAckNext = false, lostAcks = 0, receiptAttempts=0, lastCommand=null, duplicateCommand=false, handoffLost=false;
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
    if(String(url).endsWith('/delivery'))savedDelivery={...options,body:JSON.parse(options.body)};
    if(String(url).endsWith('/delivery')&&++deliveryAttempts===1&&response.ok)return new Response('{}',{status:503});
    if(String(url).endsWith('/results') && ++receiptAttempts===1) return new Response('{}',{status:503});
    if(String(url).endsWith('/poll')) {
      const result=await response.json();
      if(result.fill?.blocked&&result.fill?.pending>0)waitedWhileBlocked=true;
      if(scenario==='handoff'&&result.commands?.length&&!handoffLost){handoffLost=true;throw Error('synthetic lost handoff');}
      if(lastCommand&&!duplicateCommand) {result.commands=[lastCommand];duplicateCommand=true;}
      else if(result.commands?.length)lastCommand=result.commands[0];
      return new Response(JSON.stringify(result),{status:response.status});
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
  nativeCalls++;assert.equal(this,nativeBridge);assert.ok(['synthetic','getLiveMessage','sendLiveCommentAck'].includes(name));
  if(name==='sendLiveCommentAck'){callback.call(nativeThis,{base_resp:{ret:0},jsapi_resp:{ret:0,resp_json:JSON.stringify({baseResponse:{Ret:0},server_msg_id_str:'crossing-server'})}});return nativeResult;}
  if(crossing&&args===crossingArgs){crossingCallback=callback;return nativeResult;}
  assert.equal(args,data);
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

const submittedTexts=[];
let writes=0,submissions=0,editorOpen=true,openClicks=0,headingClicks=0;
const guarded=['disabled','ambiguous','changed'].includes(scenario);
let firstGuard=guarded;
browser.console={info(){},warn(){}};
class Textarea {constructor(){this._text='';}get value(){return this._text;}set value(v){this._text=v;writes++;}}
const editor=editorKind==='textarea'?new Textarea():{_text:'',get textContent(){return this._text;},set textContent(v){this._text=v;writes++;}};
Object.assign(editor,{tagName:editorKind==='textarea'?'TEXTAREA':'DIV',isContentEditable:editorKind!=='textarea',isConnected:true,ownerDocument:document,
  getClientRects:()=>[{}],closest:()=>null,matches:()=>false,contains:()=>false,querySelector:()=>null,
  dispatchEvent:event=>{assert.equal(event.type,'input');if(firstGuard&&scenario==='changed')editor._text='人工修改';return true;},click:()=>{throw Error('editor clicks forbidden');},submit:()=>{throw Error('form submission forbidden');}});
const button={tagName:'BUTTON',disabled:scenario==='disabled',textContent:'评论',contains:()=>false,isConnected:true,ownerDocument:document,getClientRects:()=>[{}],closest:()=>null,matches:()=>false,
  click(){submittedTexts.push(editor._text);submissions++;
    if(recovering&&submissions===1){const text=editor._text;if(crossingZero)nativeBridge.invoke('sendLiveCommentAck',{},()=>{});setTimeout(()=>{
      if(crossing){
        assert.equal(typeof crossingCallback,'function');
        crossingCallback.call(nativeThis,{jsapi_resp:{resp_json:JSON.stringify({baseResponse:{Ret:0},app_msg_list:[],msg_list:[{type:1,seq:crossingZero?'0':'1872',client_msg_id:'crossing-server',local_client_msg_id:'crossing-local',username:'fixture-own',content:text}]})}});
      }else emitEcho(text);
    },9500);setTimeout(()=>{editor._text='';},4000);return;}
    if(scenario.startsWith('delivery_'))emitEcho(editor._text);
    if(neverRecover)throw Error('ambiguous click');editor._text='';if((scenario==='collapsed'||scenario==='open_only'||scenario==='timed'))editorOpen=false;}};
editor.parentElement={querySelectorAll:()=>firstGuard&&scenario==='ambiguous'?[button,{...button}]:[button]};
if(scenario==='nested'||scenario==='nested_custom'){
  const label={...button,tagName:'SPAN',contains:()=>false,click(){button.click();}};
  button.tagName=scenario==='nested'?'BUTTON':'DIV';
  button.contains=n=>n===label;
  const wrapper={...button,tagName:'DIV',contains:n=>n===button||n===label,click(){throw Error('wrapper click must never occur');}};
  editor.parentElement.querySelectorAll=()=>[wrapper,button,label];
}

browser.HTMLTextAreaElement=Textarea;
document.hidden=false;document.activeElement=editor;document.querySelectorAll=()=>[editor];
if((scenario==='collapsed'||scenario==='open_only'||scenario==='timed')){
  editorOpen=false;
  const makeEntry=(heading)=>({tagName:'DIV',textContent:'聊一聊',isConnected:true,ownerDocument:document,children:heading?[]:[{},{}],
    getClientRects:()=>[{}],getBoundingClientRect:()=>({width:116,height:32}),closest:()=>null,querySelectorAll:()=>[],
    click(){if(heading){headingClicks++;throw Error('heading must not be clicked');}openClicks++;editorOpen=true;}});
  const heading=makeEntry(true),opener=makeEntry(false);
  browser.getComputedStyle=n=>({visibility:'visible',display:'block',opacity:'1',borderRadius:n===opener?'20px':'0px',backgroundColor:n===opener?'rgba(210, 210, 210, 0.38)':'transparent'});
  document.querySelectorAll=selector=>selector.startsWith('textarea,')?(editorOpen?[editor]:[]):[heading,opener];
}

const emitEcho=text=>{
  const saved=data.msg_list;
  const id='server-echo-'+submissions;
  const emit=(user,local,content=text,messageId=id,ret=0,base=null)=>{
    data.msg_list=[{type:1,client_msg_id:messageId,local_client_msg_id:local,username:user,content}];
    data.jsapi_resp={resp_json:JSON.stringify({baseResponse:base||(scenario==='delivery_echo'?{ret}:{Ret:ret}),msg_list:data.msg_list,app_msg_list:[]})};
    delete data.msg_list;
    nativeBridge.invoke('synthetic',data,()=>{});
    delete data.jsapi_resp;
  };
  emit('fixture-own','historical-local',text,'historical-echo');
  if(scenario!=='delivery_local_marker')emit('other-actor','local-'+submissions); // Configured actor mismatch never confirms.
  emit('fixture-own',id); // A local-only echo never confirms.
  emit('fixture-own','local-'+submissions,text+'different');
  emit('fixture-own','failed-local-'+submissions,text,'failed-response-'+submissions,1);
  emit('fixture-own','conflict-local',text,'conflict-status-'+submissions,0,{ret:0,Ret:1});
  emit('fixture-own','missing-local',text,'missing-status-'+submissions,0,{});
  emit('fixture-own','string-local',text,'string-status-'+submissions,0,{Ret:'0'});
  if(scenario!=='delivery_timeout'&&!keepBlocked)emit('fixture-own','local-'+submissions);
  if(saved===undefined)delete data.msg_list;else data.msg_list=saved;
};
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const until=async(test,timeout=16000)=>{const deadline=Date.now()+timeout;while(!test()&&Date.now()<deadline)await sleep(100);assert.ok(test(),'condition timeout: '+JSON.stringify(browser.__liveArchiveModerationV1?.getStatus()));};
(async()=>{
  vm.runInContext(fs.readFileSync(bootstrap,'utf8'),context);
  document.readyState='complete';domListeners.get('DOMContentLoaded')();
  // Windows may start several fixture browsers while Cargo links other tests.
  // Keep action/receipt deadlines unchanged; give only initial pairing more time.
  await until(()=>browser.__liveArchiveModerationV1.getStatus().connected,30000);
  document.getElementById('live-archive-moderation-entry').click();
  const toggle=document.getElementById('live-archive-moderation-toggle');toggle.click();
  if(scenario==='open_only'){
    await until(()=>writes===1);
    await until(()=>browser.__liveArchiveModerationV1.getStatus().fill.last_result?.outcome==='filled');
    assert.equal(editor._text,'仅展开填入测试，不发送');assert.equal(submissions,0);assert.equal(headingClicks,0);assert.equal(openClicks,1);
    assert.equal(browser.__liveArchiveModerationV1.getStatus().send_chat,false);
    browser.__liveArchiveModerationV1.dispose();await sleep(300);
    process.stdout.write(JSON.stringify({writes:1,submissions:0,send_chat:false,opener_only:true})+'\n');return;
  }
  if(scenario==='timed'){
    await until(()=>submissions===2);await until(()=>browser.__liveArchiveModerationV1.getStatus().fill.bridge?.submitted===2);await until(()=>receiptAttempts>=3);
    assert.equal(writes,2);assert.notEqual(submittedTexts[0],submittedTexts[1]);assert.ok(submittedTexts.every(t=>['定时文案甲','定时文案乙','定时文案丙'].includes(t)));
    assert.ok(openClicks>=2);assert.equal(headingClicks,0);
    toggle.click();await until(()=>browser.__liveArchiveModerationV1.getStatus().timed_messages?.active===false);await sleep(2500);assert.equal(submissions,2);
    browser.__liveArchiveModerationV1.dispose();await sleep(300);process.stdout.write(JSON.stringify({writes,submissions,send_chat:true})+'\n');return;
  }
  const gift=(id,user,target='fixture-recipient',name=scenario==='emoji_overflow'?'礼'.repeat(31):'小星星')=>({msg_type:20009,client_msg_id:id,
    from_user_contact:{contact:{username:(scenario==='long_users'||scenario==='aliases_long')?'u'.repeat(147)+({ 'first-user':'a','second-user':'b','third-user':'c','other-user':'d'}[user]||'e'):user,nickname:'集成测试观众'}},to_user_contact:{contact:{username:target}},
    payload:JSON.stringify({reward_product_id:'fixture-product',reward_product_count:47,reward_gift:{name,...(scenario==='variation'?{price:1000}:{})}})});
  const deliver=list=>nativeBridge._handleMessageFromWeixin({app_msg_list:list});
  if(scenario.startsWith('delivery_'))emitEcho('感谢集成测试观众送来的小星星');
  if(crossing){
    data.jsapi_resp={resp_json:JSON.stringify({baseResponse:{Ret:0},app_msg_list:[],msg_list:[{type:1,seq:'1829',client_msg_id:'baseline-server',username:'fixture-own',content:'earlier chat'}]})};
    nativeBridge.invoke('synthetic',data,()=>{});delete data.jsapi_resp;
    // The platform owns a long poll already in flight before the button click.
    nativeBridge.invoke('getLiveMessage',crossingArgs,()=>{});
  }
  editor._text='人工草稿';
  deliver([gift('g1','first-user'),gift('g1','first-user'),gift('other','other-user','other-target')]);
  await sleep(6500);assert.equal(writes,0);assert.equal(submissions,0);assert.equal(editor._text,'人工草稿');
  editor._text='';
  if(scenario==='handoff'){
    await until(()=>browser.__liveArchiveModerationV1.getStatus().fill.bridge?.unknown===1);
    assert.equal(writes,0);assert.equal(submissions,0);
    deliver([gift('g2','second-user')]);await until(()=>submissions===1);
    await until(()=>browser.__liveArchiveModerationV1.getStatus().fill.bridge.submitted===1);
    assert.equal(writes,1);assert.equal(browser.__liveArchiveModerationV1.getStatus().fill.bridge.blocked,false);
    browser.__liveArchiveModerationV1.dispose();await sleep(300);
    process.stdout.write(JSON.stringify({writes:1,submissions:1,send_chat:true,handoff_lost:true})+'\n');return;
  }
  if(!guarded)await until(()=>submissions===1);
  const expected=(neverRecover||recovering)?'unknown':guarded?'rejected':'submitted';
  await until(()=>browser.__liveArchiveModerationV1.getStatus().fill.last_result?.outcome===expected);
  if(recovering)deliver([gift('g2','second-user','fixture-recipient','玫瑰')]);
  await until(()=>receiptAttempts>=2&&duplicateCommand);
  assert.equal(submissions,guarded?0:1,'duplicate command and receipt retry never re-click');assert.equal(writes,1);
  firstGuard=false;button.disabled=false;editor._text='';if(!recovering)deliver([gift('g2',scenario==='variation'?'first-user':'second-user','fixture-recipient','玫瑰')]);
  if(neverRecover){
    await until(()=>submissions===2);await until(()=>browser.__liveArchiveModerationV1.getStatus().fill.bridge.unknown===2);assert.equal(writes,2);
    assert.equal(browser.__liveArchiveModerationV1.getStatus().fill.bridge.blocked,false);
  }else{
    await until(()=>submissions===(guarded?1:2));await until(()=>browser.__liveArchiveModerationV1.getStatus().fill.bridge.submitted===(guarded||recovering?1:2));
    assert.equal(editor._text,'');assert.equal(writes,2);
  }
  deliver([gift('g3','third-user')]);toggle.click();await sleep(5500);
  assert.equal(submissions,guarded?1:2);
  if((scenario==='collapsed'||scenario==='open_only'||scenario==='timed')){assert.ok(openClicks>=2);assert.equal(headingClicks,0);}
  const state=browser.__liveArchiveModerationV1.getStatus();assert.equal(state.send_chat,true);assert.equal(state.candidate_preview.paused,true);
  if(scenario.startsWith('delivery_')){
    await until(()=>{const d=browser.__liveArchiveModerationV1.getStatus().delivery_confirmation;return d.pending===0&&d.buffered===0;});
    const d=browser.__liveArchiveModerationV1.getStatus().delivery_confirmation;
    assert.equal(d.echo_observed,scenario!=='delivery_timeout'&&!keepBlocked?2:0);assert.equal(d.unconfirmed,keepBlocked?1:scenario==='delivery_timeout'?2:0);assert.ok(deliveryAttempts>=(keepBlocked?2:3));assert.equal(submissions,keepBlocked?1:2);
    if(recovering){assert.equal(state.fill.bridge.blocked,false);assert.equal(state.fill.bridge.echo_confirmed,1);assert.ok(waitedWhileBlocked,'drafts must remain queued during reconciliation');assert.equal(submittedTexts.filter(t=>t===submittedTexts[0]).length,1,'unknown action never re-sent');}
    if(keepBlocked)assert.equal(state.fill.bridge.blocked,true);
    const receipt=async(patch={},auth=savedDelivery.headers.Authorization)=>fetch('http://'+address+'/v1/delivery',{...savedDelivery,headers:{...savedDelivery.headers,Authorization:auth,Origin:origin},body:JSON.stringify({...savedDelivery.body,...patch})});
    assert.equal((await receipt({},'Bearer wrong-token')).status,422);
    assert.equal((await receipt({action_id:'missing-action'})).status,422);
    assert.equal((await receipt({page_url:'https://channels.weixin.qq.com/web/pages/home'})).status,422);
    assert.equal((await receipt()).status,200);
    if(scenario==='delivery_echo')assert.equal((await receipt({sender_user_id:'other-actor'})).status,422);
    assert.equal((await receipt({state:savedDelivery.body.state==='unconfirmed'?'echo_observed':'unconfirmed',source:'timeout',message_id:null,sender_user_id:null,local_message_id:null})).status,422);

  }
  browser.__liveArchiveModerationV1.dispose();await sleep(300);
  process.stdout.write(JSON.stringify({submissions,writes,submitted_texts:submittedTexts,send_chat:true,unknown:neverRecover,recovered:recovering,duplicate_command:true,receipt_retry:true})+'\n');
})().catch(e=>{console.error(e);process.exitCode=1;browser.__liveArchiveModerationV1?.dispose();});
