// Complete injected SDK -> local HTTP bridge. No platform requests or chat writes.
const fs = require('node:fs'), vm = require('node:vm'), assert = require('node:assert/strict');
const [bootstrap, address, drainReady] = process.argv.slice(2);
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
  document,
  crypto:require('node:crypto').webcrypto,URL,TextDecoder,TextEncoder,AbortController,atob:globalThis.atob,
  Uint8Array,setTimeout,clearTimeout,setInterval,clearInterval});
(async () => {
  vm.runInContext(fs.readFileSync(bootstrap,'utf8'),context);
  assert.equal(document.getElementById('live-archive-moderation-entry'),null);
  document.readyState='complete';domListeners.get('DOMContentLoaded')();
  const entry=document.getElementById('live-archive-moderation-entry');
  const panel=document.getElementById('live-archive-moderation-panel');
  const toggle=panel.children.find(node=>node.id==='live-archive-moderation-toggle');
  assert.equal(panel.hidden,true);assert.equal(toggle.disabled,true);
  await browser.fetch(origin+'/synthetic-local-fixture');
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.network_messages,0);
  entry.click();assert.equal(panel.hidden,false);
  const connectionDeadline=Date.now()+5000;
  while(toggle.disabled&&Date.now()<connectionDeadline)await new Promise(resolve=>setTimeout(resolve,10));
  assert.equal(toggle.disabled,false,JSON.stringify(browser.__liveArchiveModerationV1.getStatus()));
  assert.equal(entry.attributes['aria-expanded'],'true');
  // The real page replaces these during initialization. Preserve its wrappers
  // (including wrappers delegating to our old hooks) when Start reattaches.
  const earlyFetch=browser.fetch, earlySocket=browser.WebSocket, earlyOpen=Xhr.prototype.open;
  let pageFetchCalls=0, pageOpenCalls=0;
  const pageFetch=function(...args) {pageFetchCalls++; return Reflect.apply(earlyFetch,this,args);};
  class PageSocket extends earlySocket {}
  const pageOpen=function(...args) {pageOpenCalls++; return Reflect.apply(earlyOpen,this,args);};
  browser.fetch=pageFetch;browser.WebSocket=PageSocket;Xhr.prototype.open=pageOpen;
  await new Promise(resolve=>setTimeout(resolve,1100));
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.hook_replaced,true);
  toggle.click();assert.equal(toggle.textContent,'暂停采集');
  const injectedFetch=browser.fetch, injectedSocket=browser.WebSocket, injectedOpen=Xhr.prototype.open;
  await browser.fetch('https://finder.video.qq.com/synthetic-local-fixture');
  const socket=new browser.WebSocket('wss://long.weixin.qq.com/synthetic-local-fixture');
  socket.dispatch('message',{data:JSON.stringify(data)});
  socket.dispatch('message',{data:new Uint8Array([1,2,3])});
  const xhr=new Xhr(); xhr.open('GET',origin+'/synthetic-local-fixture');
  xhr.responseType='';xhr.responseText=JSON.stringify(data);xhr.dispatch('load',{});
  const nativeJson=new Xhr();nativeJson.open('GET',origin+'/synthetic-local-fixture');
  nativeJson.responseType='json';nativeJson.response=data;nativeJson.dispatch('load',{});
  listeners.get('channels:JoinLive')?.({liveId:'private-live-id',objectId:'private-object-id'});
  await new Promise(resolve=>setTimeout(resolve,2100));
  const status=browser.__liveArchiveModerationV1.getStatus();
  assert.equal(status.connected,true);assert.equal(status.bridge_error,null);
  assert.equal(status.send_chat,false);assert.equal(status.rules,null);assert.equal(status.fill,null);
  assert.equal(status.decoder.adapter,'wechat_channels.desktop_probe');
  assert.equal(status.decoder.room_verified,false);assert.equal(status.decoder.network_messages,5);
  assert.equal(status.decoder.schemas,5);assert.equal(status.decoder.event_bus,true);
  assert.equal(status.decoder.hook_replaced,false);
  assert.equal(status.decoder.native_hooks,4);assert.equal(nativeCalls,0,'probe must never call native APIs');
  assert.equal(nativeRegistrations,0,'probe must never register native subscriptions');
  assert.equal(pageFetchCalls,1);assert.equal(pageOpenCalls,2);
  assert.ok(socket instanceof PageSocket);
  assert.ok(requests.length < 9, 'desktop probe should not poll every 500 ms');
  assert.equal(requests[0].adapter_id,'wechat_channels.desktop_probe');
  const fields=document.getElementById('live-archive-moderation-fields').textContent;
  assert.ok(fields.includes('giftId:string'));assert.ok(!fields.includes('private-'));
  assert.ok(document.getElementById('live-archive-moderation-status').textContent.includes('已连接'));
  let callbackCalls=0;
  const nativeReturn=nativeBridge.invoke('synthetic',data,function(value,extra) {
    callbackCalls++;assert.equal(this,nativeThis);assert.equal(value,data);assert.equal(extra,'unchanged');
  });
  assert.equal(nativeReturn,nativeResult);assert.equal(callbackCalls,1);assert.equal(nativeCalls,1);
  const nativeText=JSON.stringify(data);
  assert.equal(nativeBridge._handleMessageFromWeixin(nativeText),nativeText);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.network_messages,7);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.schemas,7);
  toggle.click();assert.equal(toggle.textContent,'开始采集');
  const latestPageFetch=function(...args) {return Reflect.apply(originalFetch,this,args);};
  browser.fetch=latestPageFetch;
  const previousNativeInvoke=nativeBridge.invoke;
  const pageNativeInvoke=function(...args) {return Reflect.apply(previousNativeInvoke,this,args);};
  nativeBridge.invoke=pageNativeInvoke;
  socket.dispatch('message',{data:JSON.stringify(data)});
  nativeBridge._handleMessageFromWeixin(nativeText);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.network_messages,7);
  toggle.click();socket.dispatch('message',{data:JSON.stringify(data)});
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.network_messages,8);
  await browser.fetch(origin+'/synthetic-local-fixture');
  await new Promise(resolve=>setTimeout(resolve,50));
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.network_messages,9);
  assert.equal(nativeBridge.invoke('getLiveMessage',data,function() {callbackCalls++;}),nativeResult);
  assert.equal(callbackCalls,2);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.network_messages,10,'nested native wrappers count delivery once');
  assert.ok(browser.__liveArchiveModerationV1.getStatus().decoder.recent_schemas.some(item=>item.api_kind==='message'));
  toggle.click();
  const frozenOffBridge={on:nativeOn};
  Object.defineProperty(frozenOffBridge,'off',{value:nativeOff,writable:false});
  browser.WeixinJSBridge=frozenOffBridge;toggle.click();
  assert.equal(frozenOffBridge.on,nativeOn,'do not wrap subscription callback if its removal identity cannot be translated');
  toggle.click();
  const rejectedOffBridge=new Proxy({on:nativeOn,off:nativeOff},{defineProperty(target,key,descriptor) {
    if(key==='off') return false;
    return Reflect.defineProperty(target,key,descriptor);
  }});
  browser.WeixinJSBridge=rejectedOffBridge;toggle.click();
  assert.equal(rejectedOffBridge.on,nativeOn,'failed off installation must leave page subscriptions untouched');
  toggle.click();browser.WeixinJSBridge=nativeBridge;toggle.click();
  const envelope={payload:JSON.stringify({messages:[{content:'private-chat-content'}]}), 'private-dynamic-key':{unexpected:'private-value'}, baseResponse:{ret:0,errMsg:'private-error'}, liveMsgList:[{msgType:1,msgContent:'private-chat'}]};
  Object.defineProperty(envelope,'nickname',{enumerable:true,get(){throw Error('diagnostic must not execute getters');}});
  nativeBridge._handleMessageFromWeixin(undefined,envelope);
  nativeBridge._handleMessageFromWeixin('private-non-json-text');
  const shapes=browser.__liveArchiveModerationV1.getStatus().decoder.recent_schemas;
  assert.ok(shapes.some(item=>item.category==='dispatch' && item.fields.includes('$.args[0]:undefined')));
  assert.ok(shapes.some(item=>item.fields.some(field=>field.includes('payload<json>.messages'))));
  assert.ok(shapes.some(item=>item.fields.includes('$.args[1].nickname:accessor')));
  assert.ok(shapes.some(item=>item.fields.some(field=>field.includes('baseResponse.ret:number'))));
  assert.ok(shapes.some(item=>item.fields.some(field=>field.includes('liveMsgList[].msgContent:string'))));
  assert.ok(shapes.every(item=>item.fields.length<=24));
  assert.ok(!JSON.stringify(shapes).includes('private-'));
  assert.ok(!JSON.stringify(shapes).includes('getLiveMessage'));
  const messageFixture=JSON.stringify({msg_list:[
    {type:7,content:'private-message',nickname:'private-nick',client_msg_id:'private-id'},
    {type:8,content:JSON.stringify({giftName:'private-gift'})},
    {type:'private-type',content:'private-content'},
    {type:7,content:'private-oversized'.repeat(1000)},
    {type:7}
  ]});
  nativeBridge._handleMessageFromWeixin(messageFixture);
  let batch=browser.__liveArchiveModerationV1.getStatus().decoder.recent_message_batch;
  assert.equal(batch.sampled,5);assert.equal(batch.total,5);assert.equal(batch.truncated,false);
  assert.deepEqual(JSON.parse(JSON.stringify(batch.type_counts)),[
    {type:7,count:3,text:1,json:0,opaque:1,absent:1},
    {type:8,count:1,text:0,json:1,opaque:0,absent:0},
    {type:null,count:1,text:1,json:0,opaque:0,absent:0}
  ]);
  const batchCount=browser.__liveArchiveModerationV1.getStatus().decoder.message_batches;
  const beforeRepeat=browser.__liveArchiveModerationV1.getStatus().decoder.schemas;
  nativeBridge._handleMessageFromWeixin(messageFixture);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.message_batches,batchCount+1,'repeated polling remains batch diagnostics');
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.schemas,beforeRepeat);
  toggle.click();nativeBridge._handleMessageFromWeixin(messageFixture);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.message_batches,batchCount+1,'paused batches must not be inspected');
  toggle.click();
  const accessorEntry={};
  Object.defineProperty(accessorEntry,'type',{enumerable:true,get(){throw Error('no type getter');}});
  Object.defineProperty(accessorEntry,'content',{enumerable:true,get(){throw Error('no content getter');}});
  nativeBridge._handleMessageFromWeixin({msg_list:[accessorEntry]});
  batch=browser.__liveArchiveModerationV1.getStatus().decoder.recent_message_batch;
  assert.equal(batch.type_counts[0].type,null);assert.equal(batch.type_counts[0].absent,1);
  const wideEnvelope=Object.fromEntries(Array.from({length:30},(_,i)=>['private-key-'+i,'private-value']));
  wideEnvelope.msg_list=[{type:65536,content:'private-body'}];
  nativeBridge._handleMessageFromWeixin(wideEnvelope);
  batch=browser.__liveArchiveModerationV1.getStatus().decoder.recent_message_batch;
  assert.equal(batch.sampled,1,'field display cap must not prevent bounded batch discovery');
  assert.equal(batch.type_counts[0].type,null,'out-of-range type values stay anonymous');
  nativeBridge._handleMessageFromWeixin(JSON.stringify({msg_list:Array.from({length:55},(_,i)=>({type:i,content:'private-capped'}))}));
  batch=browser.__liveArchiveModerationV1.getStatus().decoder.recent_message_batch;
  assert.equal(batch.sampled,55);assert.equal(batch.total,55);assert.equal(batch.truncated,false);
  assert.equal(batch.type_counts.length,8);assert.equal(batch.overflow,47);
  nativeBridge._handleMessageFromWeixin(JSON.stringify({msg_list:[{type:39,content:'private-pause-candidate'}]}));
  toggle.click();pageTextNodes.push(pageText('private-pause-candidate'));
  const matchesBefore=browser.__liveArchiveModerationV1.getStatus().decoder.page_matches;
  await new Promise(resolve=>setTimeout(resolve,1200));
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.page_matches,matchesBefore);
  toggle.click();
  nativeBridge._handleMessageFromWeixin(JSON.stringify({msg_list:[
    {type:40,content:'private-visible-candidate'},
    {type:41,content:'private-panel-candidate'},
    {type:42,content:'private-hidden-candidate'}
  ]}));
  pageTextNodes.push(pageText('private-panel-candidate',true),pageText('private-hidden-candidate',false,true));
  await new Promise(resolve=>setTimeout(resolve,400));
  pageTextNodes.push(pageText('someone: private-visible-candidate'));
  await new Promise(resolve=>setTimeout(resolve,1800));
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.page_matches,matchesBefore+1,'delayed visible text must match; excluded/hidden text must not');
  let match;
  const matchDeadline=Date.now()+10000;
  do {
    await new Promise(resolve=>setTimeout(resolve,100));
    match=browser.__liveArchiveModerationV1.getStatus().decoder.recent_page_match;
  } while (!match.type_counts.some(c=>c.type===41 && c.unmatched===1) && Date.now()<matchDeadline);
  assert.ok(match.type_counts.some(c=>c.type===41 && c.unmatched===1));
  assert.ok(match.type_counts.some(c=>c.type===42 && c.unmatched===1));
  assert.ok(!match.type_counts.some(c=>c.type===39),'pause clears pending candidates instead of replaying them');
  const applicationPayload=Buffer.from(JSON.stringify({gift_info:{gift_id:'private-app-id',gift_name:'private-app-name',gift_count:3}}),'utf8').toString('base64');
  let eventCallbackCount=0;
  const ownEventCallback=function(value,extra) {assert.equal(this,nativeThis);assert.equal(extra,'original-extra');eventCallbackCount++;return nativeResult;};
  assert.equal(nativeBridge.on('private-gift-event',ownEventCallback),nativeResult);
  assert.equal(nativeSubscriptions.get('private-gift-event').call(nativeThis,{msg_type:20009,payload:applicationPayload},'original-extra'),nativeResult);
  assert.equal(eventCallbackCount,1);
  assert.ok(browser.__liveArchiveModerationV1.getStatus().decoder.recent_schemas.some(s=>s.category==='on_callback'&&s.api_kind==='gift'));
  assert.equal(nativeBridge.off('private-gift-event',ownEventCallback),nativeResult);
  assert.equal(nativeSubscriptions.size,0,'original callback identity must unsubscribe its wrapper');
  nativeBridge.on('private-gift-event',ownEventCallback);
  nativeBridge._handleMessageFromWeixin({app_msg_list:[{msg_type:20009,payload:applicationPayload}]});
  let application=browser.__liveArchiveModerationV1.getStatus().decoder.recent_application;
  assert.equal(application.format,'base64_json');assert.ok(application.gift_hints>=1);
  assert.ok(application.fields.includes('$.gift_info:object'));
  assert.ok(application.fields.includes('$.gift_info.gift_count:number'),'decoded application children retain types without values');
  nativeBridge._handleMessageFromWeixin({msg_type:20009,payload:JSON.stringify({data:JSON.stringify({gift_info:{gift_count:7,'private-child-key':'private-child-value'}})})});
  application=browser.__liveArchiveModerationV1.getStatus().decoder.recent_application;
  assert.ok(application.fields.includes('$.data<json>.gift_info.gift_count:number'));
  assert.ok(application.fields.length<=24);
  assert.ok(!JSON.stringify(application).includes('private-'));
  const applicationBefore=browser.__liveArchiveModerationV1.getStatus().decoder.application_payloads;
  const lateApplication={msg_type:20009,client_msg_id:'private-app-message',
    from_user_contact:{contact:{username:'private-app-user',nickname:'private-app-nickname'}},payload:applicationPayload};
  nativeBridge._handleMessageFromWeixin({app_msg_list:[{msg_type:20008},{msg_type:20008},{msg_type:20008},lateApplication,lateApplication]});
  application=browser.__liveArchiveModerationV1.getStatus().decoder.recent_application;
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.application_payloads,applicationBefore+1,'later entries are inspected once per received envelope');
  assert.ok(application.envelope_fields.includes('$.from_user_contact.contact.username:string'));
  assert.ok(application.envelope_fields.includes('$.client_msg_id:string'));
  assert.ok(!JSON.stringify(application).includes('private-'));
  const cappedApplicationBefore=browser.__liveArchiveModerationV1.getStatus().decoder.application_payloads;
  nativeBridge._handleMessageFromWeixin({appMsgList:Array.from({length:55},()=>({msgType:20009,payload:applicationPayload}))});
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.application_payloads,cappedApplicationBefore+55,'all entries within the expanded application batch are inspected');
  pageTextNodes.push(pageText('fixture application display'));
  nativeBridge._handleMessageFromWeixin({msgType:20009,payload:JSON.stringify({content:'fixture application display'})});
  const applicationMatchDeadline=Date.now()+2500;
  do {
    await new Promise(resolve=>setTimeout(resolve,100));
    match=browser.__liveArchiveModerationV1.getStatus().decoder.recent_page_match;
  } while (!match.type_counts.some(c=>c.type===20009&&c.matched===1)&&Date.now()<applicationMatchDeadline);
  assert.ok(match.type_counts.some(c=>c.type===20009&&c.matched===1),'decoded application content is compared locally only');
  const reviewButton=document.getElementById('live-archive-moderation-review');
  assert.equal(reviewButton.textContent,'本地字段核对');reviewButton.click();
  nativeBridge._handleMessageFromWeixin({msgType:20009,client_msg_id:'private-review-id',payload:JSON.stringify({
    sampleProduct:{name:'Fixture gift',reward_count:3,secret:'private-review-value'},
    sample_count:2,user_id:'private-review-user','private-dynamic-key':'private-dynamic-value'
  })});
  entry.click();entry.click();
  const reviewFields=document.getElementById('live-archive-moderation-fields').textContent;
  assert.ok(reviewFields.includes('Fixture gift')&&reviewFields.includes('sample_count=2'));
  assert.ok(reviewFields.includes('payload.sampleProduct:object'));
  assert.ok(!reviewFields.includes('private-'));
  assert.ok(!JSON.stringify(browser.__liveArchiveModerationV1.getStatus()).includes('sampleProduct'),'review labels remain outside SDK status and event transport');
  reviewButton.click();
  nativeBridge._handleMessageFromWeixin({msgType:20009,payload:Buffer.from([255,255,255]).toString('base64')});
  application=browser.__liveArchiveModerationV1.getStatus().decoder.recent_application;
  assert.equal(application.format,'opaque');assert.equal(application.gift_hints,0,'invalid UTF-8 cannot become JSON');
  const protoWire=Buffer.from([8,1,18,8,97,98,99,100,101,102,103,104]).toString('base64');
  nativeBridge._handleMessageFromWeixin({cmd_id:777,body:protoWire,
    data:JSON.stringify({gift_info:{gift_id:'private-gift-id',gift_name:'private-gift-name',gift_count:2}}),
    'private-dynamic-command-key':'private-token'});
  let command=browser.__liveArchiveModerationV1.getStatus().decoder.recent_command;
  assert.equal(command.command_id,777);assert.equal(command.protobuf_candidates,1);assert.equal(command.json_strings,1);
  assert.ok(command.fields.includes('s1.f1:varint'));assert.ok(command.fields.includes('s1.f2:bytes'));assert.ok(command.gift_hints>=1);
  const giftHint=browser.__liveArchiveModerationV1.getStatus().decoder.recent_gift_schema;
  assert.ok(giftHint.fields.includes('$.gift_id:string'));assert.ok(!JSON.stringify(giftHint).includes('private-'));
  nativeBridge._handleMessageFromWeixin({cmd_id:778,body:Buffer.from([18,99,97,98,99,100,101,102,103,104,105,106]).toString('base64')});
  command=browser.__liveArchiveModerationV1.getStatus().decoder.recent_command;
  assert.equal(command.base64_candidates,1);assert.equal(command.protobuf_candidates,0,'malformed wire payload must not be labelled Protobuf');
  nativeBridge._handleMessageFromWeixin({cmd_id:779,body:Buffer.from(JSON.stringify({data:JSON.stringify({appMsgList:[{msgType:20009,payload:applicationPayload}]})}),'utf8').toString('base64')});
  command=browser.__liveArchiveModerationV1.getStatus().decoder.recent_command;
  assert.equal(command.base64_json_strings,1);assert.ok(command.fields.includes('s1.$.data:string'));
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.recent_application.format,'base64_json','application payload must be found within decoded command JSON');
  const commandsBefore=browser.__liveArchiveModerationV1.getStatus().decoder.command_responses;
  toggle.click();nativeBridge._handleMessageFromWeixin({cmd_id:777,body:protoWire});
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.command_responses,commandsBefore);
  toggle.click();
  const chat=(id,user='fixture-user',content='fixture chat text',type=1)=>({type,client_msg_id:id,username:user,content,nickname:'Fixture\u0085'});
  const deliver=entries=>nativeBridge._handleMessageFromWeixin(JSON.stringify({msg_list:entries}));
  loseAckNext=true;
  deliver([chat('m1')]);deliver([chat('m1')]);
  deliver([chat('m1','fixture-user','changed content')]);
  deliver([chat('m2'),chat('m1','fixture-other-user')]);
  let candidate=browser.__liveArchiveModerationV1.getStatus().decoder;
  assert.equal(candidate.chat_candidates,3);assert.equal(candidate.candidate_duplicates,1);assert.equal(candidate.candidate_conflicts,1);
  deliver([chat(undefined),chat('bad',''),chat('bad\u0085'),chat('json','fixture-user','{"a":1}'),chat('unknown','fixture-user','private-unknown-type',10005)]);
  candidate=browser.__liveArchiveModerationV1.getStatus().decoder;
  assert.equal(candidate.chat_candidates,3);
  const messageContentBefore=browser.__liveArchiveModerationV1.getStatus().decoder.message_content_schemas;
  deliver([chat('private-entry-id','private-entry-user','private-entrant来了',10005)]);
  let unknown=browser.__liveArchiveModerationV1.getStatus().decoder.recent_message_content;
  assert.equal(unknown.format,'plain');assert.equal(unknown.arrival_text_hint,true);
  assert.equal(unknown.sender_id_present,true);assert.equal(unknown.message_id_present,true);
  const hintedSchemas=browser.__liveArchiveModerationV1.getStatus().decoder.message_content_schemas;
  deliver([chat('different-private-entry-id','different-private-entry-user','different-private-entrant来了',10005)]);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.message_content_schemas,hintedSchemas,'same metadata shape does not disclose or retain entrant identities');
  const entryJson={content:'private-entry-text进入直播间',nickname:'private-nickname',username:'private-user','private-dynamic-key':'private-token'};
  deliver([chat('private-json-entry','private-entry-user',JSON.stringify(entryJson),10005)]);
  unknown=browser.__liveArchiveModerationV1.getStatus().decoder.recent_message_content;
  assert.equal(unknown.format,'json');assert.equal(unknown.arrival_text_hint,true);assert.ok(unknown.fields.includes('$.content:string'));
  deliver([{type:10005,content:Buffer.from(JSON.stringify(entryJson)).toString('base64')}]);
  unknown=browser.__liveArchiveModerationV1.getStatus().decoder.recent_message_content;
  assert.equal(unknown.format,'base64_json');assert.equal(unknown.sender_id_present,false);assert.equal(unknown.message_id_present,false);
  deliver([{type:10005,content:'',client_msg_id:'private-empty-id',finder_live_contact:{contact:{username:'private-contact-user',nickname:'private-contact-name'}}}]);
  unknown=browser.__liveArchiveModerationV1.getStatus().decoder.recent_message_content;
  assert.equal(unknown.content_empty,true);assert.equal(unknown.arrival_text_hint,false);assert.equal(unknown.sender_id_present,false);
  assert.equal(unknown.contact_sender_id_present,true);assert.equal(unknown.contact_nickname_present,true);
  assert.ok(Buffer.byteLength(JSON.stringify(unknown))<=2048);
  deliver([{type:10005,content:'x'.repeat(8193)}]);assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.recent_message_content.format,'opaque');
  assert.ok(!JSON.stringify(browser.__liveArchiveModerationV1.getStatus().decoder.recent_message_content).includes('private-'));
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.chat_candidates,3,'arrival hint remains diagnostic, never ordinary chat or welcome');
  const arrivalEntry=(id,user='fixture-arrival-user',name='Fixture arrival',content=name+' 来了')=>({
    type:10005,client_msg_id:id,content,finder_live_contact:{contact:{username:user,nickname:name}}
  });
  deliver([arrivalEntry('arrival-1'),arrivalEntry('arrival-1')]);
  deliver([arrivalEntry('arrival-2','fixture-arrival-other','Another arrival','来了')]);
  deliver([arrivalEntry('arrival-3','fixture-arrival-json','JSON arrival',JSON.stringify({content:'JSON arrival 来了'}))]);
  const supporterEntry=(id,user,name,badges)=>{
    const entry=arrivalEntry(id,user,name);
    entry.finder_live_contact.badge_infos=badges.map(badge_name=>({badge_name}));
    return entry;
  };
  deliver([supporterEntry('supporter-a','fixture-super-fan','Fixture super',['超粉']),
    supporterEntry('supporter-b','fixture-guardian','Fixture guardian',['星守护']),
    supporterEntry('supporter-both','fixture-both','Fixture both',['超粉','星守护']),
    supporterEntry('ordinary-title-name','fixture-title-name','超粉 Fixture name',['星守护者'])]);
  const upgrade=(id,body={type:1,from_level:0,to_level:1})=>({msg_type:20031,client_msg_id:id,
    from_user_contact:{contact:{username:'fixture-upgrade-user',nickname:'Fixture upgraded'}},
    to_user_contact:{contact:{username:'fixture-host'}},payload:Buffer.from(JSON.stringify(body)).toString('base64')});
  nativeBridge._handleMessageFromWeixin({app_msg_list:[upgrade('upgrade-a'),upgrade('upgrade-a'),
    {...upgrade('upgrade-no-recipient',{type:1,from_level:1,to_level:8}),to_user_contact:{contact:{bind_info:[],menu:[],reference_info:[]},badge_infos:[]}},
    {...upgrade('upgrade-no-recipient',{type:1,from_level:1,to_level:8}),to_user_contact:{}},
    upgrade('upgrade-same-level',{type:1,from_level:1,to_level:1}),
    upgrade('upgrade-unknown',{type:2,from_level:0,to_level:1}),
    {...upgrade('upgrade-missing'),from_user_contact:{}},
    {...upgrade('upgrade-chat'),msg_type:1}]});
  const conflictArrival=arrivalEntry('arrival-conflict');conflictArrival.username='different-contact';
  deliver([conflictArrival,arrivalEntry('arrival-notice','fixture-arrival-other','Another arrival','欢迎某某来了'),
    {...arrivalEntry('arrival-wrong-type'),type:12345},arrivalEntry('arrival-empty','fixture-arrival-empty','Empty arrival','')]);
  browser.__liveArchiveModerationV1.emit('probe.message_content','diagnostic',{message_type:65500,format:'plain',confidence:'verified',live_eligible:true,room_verified:true},
    {native_event_id:'fixture-probe-id',user_id:'fixture-probe-user',nickname:'Fixture probe user'});
  toggle.click();deliver([chat('m3')]);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.chat_candidates,3);
  toggle.click();deliver([chat('m1'),chat('m3')]);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.chat_candidates,4,'resume preserves dedup cache and accepts previously paused unseen message');
  deliver([chat('long','fixture-user','🐈'.repeat(300)+'A')]);
  deliver([chat('long','fixture-user','🐈'.repeat(300)+'B')]);
  candidate=browser.__liveArchiveModerationV1.getStatus().decoder;
  assert.equal(candidate.chat_candidates,5);assert.equal(candidate.candidate_conflicts,2,'conflicts after display truncation must remain detectable');
  socket.dispatch('message',{data:JSON.stringify({msg_list:[chat('non-native')]})});
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.chat_candidates,5);
  deliver([chat('emoji','fixture-user','[强]')]);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.chat_candidates,6,'bracketed plain text must not be mistaken for JSON');
  // Candidate delivery must survive exhaustion of the diagnostic quota.
  deliver(Array.from({length:40},(_,i)=>chat('budget-'+i)));
  candidate=browser.__liveArchiveModerationV1.getStatus().decoder;
  assert.equal(candidate.chat_candidates,30);assert.equal(candidate.candidate_dropped,16);
  // Verify server-enforced ineligibility independently of the decoder flags.
  browser.__liveArchiveModerationV1.emit('chat.candidate','network',{content:'fixture forged flags',live_eligible:true,room_verified:true,confidence:'verified'});
  const rewardEntry=(id,user='fixture-gift-user',count=1)=>({msg_type:20009,client_msg_id:id,is_floatmsg:0,float_type:1,
    from_user_contact:{contact:{username:user,nickname:'Fixture donor'}},
    to_user_contact:{contact:{username:'fixture-recipient',nickname:'private-target-name'}},
    payload:Buffer.from(JSON.stringify({reward_product_id:'fixture-star',reward_gift:{name:'Fixture star',extra:'private-gift-extra'},
      reward_product_count:count,combo_product_count:count,token:'private-gift-token'})).toString('base64')});
  const rewards=entries=>nativeBridge._handleMessageFromWeixin({app_msg_list:entries});
  reviewButton.click();
  rewards([rewardEntry('g1'),rewardEntry('g1'),rewardEntry('g1','fixture-gift-user',2),rewardEntry('g2'),rewardEntry('g1','fixture-other-donor')]);
  entry.click();entry.click();
  const targetReview=document.getElementById('live-archive-moderation-fields').textContent;
  assert.ok(targetReview.includes('private-target-name')&&targetReview.includes('fixture-recipient'),'target mapping is local review only');
  assert.ok(!JSON.stringify(browser.__liveArchiveModerationV1.getStatus()).includes('private-target-name'));
  reviewButton.click();entry.click();entry.click();
  assert.ok(!document.getElementById('live-archive-moderation-fields').textContent.includes('private-target-name'),'disabling local review clears the identity');
  let gifts=browser.__liveArchiveModerationV1.getStatus().decoder;
  assert.equal(gifts.gift_candidates,3);assert.equal(gifts.gift_duplicates,1);assert.equal(gifts.gift_conflicts,1);
  const targetConflict=rewardEntry('g1');targetConflict.to_user_contact.contact.username='fixture-other-recipient';
  rewards([targetConflict]);assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.gift_conflicts,2,'changed target under one source ID is a conflict');
  rewards([targetConflict]);assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.gift_revisions,2,'repeat revision is observed once');
  rewards([rewardEntry('g1','fixture-gift-user',0),rewardEntry('zero-without-context','fixture-gift-user',0)]);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.gift_revisions,3,'zero count updates keep evidence without becoming new gifts');
  const displayUpdate=rewardEntry('g1');displayUpdate.is_floatmsg=1;
  rewards([displayUpdate]);assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.gift_revisions,4,'display-only updates are retained without creating a gift');
  toggle.click();rewards([rewardEntry('paused-gift')]);assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.gift_candidates,3);
  toggle.click();rewards([rewardEntry('g1'),rewardEntry('paused-gift')]);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.gift_candidates,4);
  rewards([rewardEntry(''),rewardEntry('invalid-count','fixture-gift-user',-1)]);
  socket.dispatch('message',{data:JSON.stringify({app_msg_list:[rewardEntry('non-native-gift')]})});
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.gift_candidates,4,'unrelated transports cannot produce gifts');
  const summary={...rewardEntry('display-summary'),msg_type:20013};
  rewards([summary,summary]);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().delivery.generated['gift.summary'],1,'display summary is recorded once, never a new gift');
  const nicknameCases=[
    ['***',[{badge_type:10,badge_name:'神秘观众 179'}],'神秘观众179'],
    ['***',[{badge_type:10,badge_name:'神秘观众311'}],'神秘观众311'],
    ['Fixture donor',[{badge_type:10,badge_name:'神秘观众 333'}],'Fixture donor'],
    ['***',[{badge_type:9,badge_name:'神秘观众 577'}],'***'],
    ['***',[{badge_type:10,badge_name:'神秘观众 179\\n伪造'}],'***'],
    ['***',[{badge_type:10,badge_name:'神秘观众 179\n'}],'***'],
    ['***',[],'***'],
  ];
  rewards(Array.from({length:40},(_,i)=>{
    const gift=rewardEntry('gift-budget-'+i);
    if(i<nicknameCases.length){gift.from_user_contact.contact.nickname=nicknameCases[i][0];gift.from_user_contact.badge_infos=nicknameCases[i][1];}
    return gift;
  }));
  gifts=browser.__liveArchiveModerationV1.getStatus().decoder;
  assert.equal(gifts.gift_candidates,44);assert.equal(gifts.gift_revisions,4);assert.equal(gifts.gift_dropped,0);
  browser.__liveArchiveModerationV1.emit('gift.candidate','network',{gift_id:'fixture-forged',gift_name:'Fixture forged',
    reward_product_count:2,quantity_mode:'incremental',live_eligible:true,room_verified:true,confidence:'verified',secret:'private-forged-token'});
  browser.__liveArchiveModerationV1.emit('gift.candidate_revision','network',{reward_product_count:0,previous_reward_product_count:1,
    quantity_mode:'incremental',live_eligible:true,room_verified:true,confidence:'verified',reason:'same_message_changed',secret:'private-revision-token'});
  const schemaCount=browser.__liveArchiveModerationV1.getStatus().decoder.schemas;
  nativeBridge._handleMessageFromWeixin('different-private-text');
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.schemas,schemaCount,'same structure must be deduplicated');
  const ackDeadline=Date.now()+8000;
  while ((browser.__liveArchiveModerationV1.getStatus().delivery.acknowledged['chat.candidate']||0)<31 && Date.now()<ackDeadline) await new Promise(resolve=>setTimeout(resolve,100));
  const arrivalDeadline=Date.now()+5000;
  while ((browser.__liveArchiveModerationV1.getStatus().delivery.acknowledged['member.candidate']||0)<7 && Date.now()<arrivalDeadline) await new Promise(resolve=>setTimeout(resolve,100));
  assert.equal(browser.__liveArchiveModerationV1.getStatus().delivery.acknowledged['member.candidate'],7,'only exact native arrival notices are candidates and repeated messages are deduplicated');
  const statusDeadline=Date.now()+5000;
  while(!(browser.__liveArchiveModerationV1.getStatus().candidate_preview?.counters?.accepted>=44) && Date.now()<statusDeadline) await new Promise(resolve=>setTimeout(resolve,100));
  const candidateStatus=browser.__liveArchiveModerationV1.getStatus().candidate_preview;
  assert.equal(candidateStatus.enabled,true);assert.equal(candidateStatus.recipient_user_id,'fixture-recipient');
  assert.equal(candidateStatus.fill_input,false);assert.equal(candidateStatus.send_chat,false);
  assert.ok(candidateStatus.pending_users>0 || candidateStatus.counters.draft_generated>0);assert.ok(candidateStatus.counters.revision_only>=3);
  assert.ok(candidateStatus.recent.every(r=>!r.source_messages),'full source linkage remains in SQLite, not every poll');
  entry.click();entry.click();
  assert.ok(document.getElementById('live-archive-moderation-preview-status').textContent.includes('感谢预览（仅日志）'));
  assert.equal(lostAcks,1);
  assert.equal(browser.__liveArchiveModerationV1.getStatus().delivery.acknowledged['chat.candidate'],31,'lost ACK retry must not double count confirmations');
  entry.click();assert.equal(panel.hidden,true);
  // Values cannot cross the authenticated bridge even when present in observed frames.
  // Direct SDK forgery fixtures deliberately submit two secret keys so the
  // server sanitizer can be checked. Exclude only these exact fixtures from
  // the independent native-observer transport privacy assertion.
  const observerRequests=requests.map(r=>({...r,events:r.events?.filter(e=>
    e.payload?.secret!=='private-forged-token' && e.payload?.secret!=='private-revision-token')}));
  assert.ok(!JSON.stringify(observerRequests).includes('private-'));
  for(let i=0;i<nicknameCases.length;i++) {
    const event=requests.flatMap(r=>r.events||[]).find(e=>e.event_type==='gift.candidate'&&e.native_event_id==='gift-budget-'+i);
    assert.ok(event);assert.equal(event.nickname,nicknameCases[i][2]);assert.equal(event.user_id,'fixture-gift-user');
  }
  assert.ok(!JSON.stringify(requests).includes('sensitive=query'));
  assert.ok(!JSON.stringify(requests).includes('finder.video.qq.com'));
  assert.ok(!JSON.stringify(requests).includes('long.weixin.qq.com'));
  assert.notEqual(injectedFetch,originalFetch);assert.notEqual(injectedSocket,Socket);
  assert.notEqual(injectedOpen,originalOpen);
  for(let i=0;i<125;i++) browser.__liveArchiveModerationV1.emit('probe.status','diagnostic',{stage:'drain_fixture'});
  fs.writeFileSync(drainReady,'ready');
  const stopDeadline=Date.now()+10000;
  while(!browser.__liveArchiveModerationV1.getStatus().stopped && Date.now()<stopDeadline) await new Promise(resolve=>setTimeout(resolve,100));
  const finalDelivery=browser.__liveArchiveModerationV1.getStatus();
  assert.equal(finalDelivery.stopped,true,'drain must acknowledge all batches before disposing');
  assert.equal(finalDelivery.buffered,0);
  assert.deepEqual(JSON.parse(JSON.stringify(finalDelivery.delivery.acknowledged)),JSON.parse(JSON.stringify(finalDelivery.delivery.generated)));
  assert.equal(browser.__liveArchiveModerationV1.emit('probe.status','diagnostic',{}),false);
  const countAfterStop=browser.__liveArchiveModerationV1.getStatus().decoder.network_messages;
  nativeSubscriptions.get('private-gift-event').call(nativeThis,{msg_type:20009,payload:applicationPayload},'original-extra');
  assert.equal(browser.__liveArchiveModerationV1.getStatus().decoder.network_messages,countAfterStop,'retained callbacks stop observation');
  nativeBridge.off('private-gift-event',ownEventCallback);assert.equal(nativeSubscriptions.size,0);
  nativeBridge.on('private-gift-event',ownEventCallback);nativeBridge.off('private-gift-event',ownEventCallback);
  assert.equal(nativeSubscriptions.size,0,'register/unregister after disposal must keep callback identity');
  assert.equal(browser.fetch,latestPageFetch);assert.equal(browser.WebSocket,PageSocket);
  assert.equal(Xhr.prototype.open,pageOpen);
  assert.equal(nativeBridge.invoke,pageNativeInvoke);assert.equal(nativeBridge._handleMessageFromWeixin,nativeDispatch);
  assert.equal(nativeBridge.call,nativeInvoke);
  const disposedXhr=new Xhr();disposedXhr.open('GET',origin+'/synthetic-local-fixture');
  assert.equal(disposedXhr.listeners.size,0);
  const disposedSocket=new browser.WebSocket('wss://long.weixin.qq.com/synthetic-local-fixture');
  assert.equal(disposedSocket.listeners.size,0);
  assert.equal(listeners.size,0);assert.equal(socket.listeners.size,0);
  assert.equal(document.getElementById('live-archive-moderation-entry'),null);
  assert.equal(document.getElementById('live-archive-moderation-panel'),null);
  // The shared status panel deliberately remains visible after disconnect.
  // Its actions must be disabled; the old collection UI and hooks are gone.
  assert.ok(document.body.children.every(n=>n.id==='live-archive-assistant'),JSON.stringify(document.body.children.map(n=>n.id)));
  const shared=document.getElementById('live-archive-assistant');
  if(shared){const descendants=n=>[n,...n.children.flatMap(descendants)];const toggle=descendants(shared).find(n=>n.attributes.role==='switch');assert.equal(toggle.disabled,true);}
  const submittedSchemas=new Set(requests.flatMap(r=>r.events||[]).filter(e=>e.event_type==='probe.schema').map(e=>e.event_id)).size;
  assert.ok(submittedSchemas<=schemaCount);
  process.stdout.write(JSON.stringify({send_chat:false,schemas:submittedSchemas,observed_schemas:schemaCount,stopped:true})+'\n');
})().catch(error=>{console.error(error);process.exitCode=1;browser.__liveArchiveModerationV1?.dispose();});
