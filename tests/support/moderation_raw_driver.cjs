const fs=require('node:fs'), vm=require('node:vm'), assert=require('node:assert/strict');
const [bootstrap,address,platform,enabledArg]=process.argv.slice(2), enabled=enabledArg==='true';
const wechat=platform==='wechat_channels', origin=wechat?'https://channels.weixin.qq.com':'https://live.douyin.com';
const pathname=wechat?'/web/pages/live':'/123';
let lost=0, rawRequests=0, pairing=null;
class Element {
  constructor(tag){this.tagName=tag.toUpperCase();this.children=[];this.style={};this.listeners=new Map();}
  setAttribute(){} appendChild(child){child.parent=this;this.children.push(child);return child;}
  addEventListener(n,f){this.listeners.set(n,f);} removeEventListener(n){this.listeners.delete(n);}
  click(){this.listeners.get('click')?.();} remove(){}
}
const document={readyState:'complete',body:new Element('body'),hidden:false,activeElement:null,
  createElement:t=>new Element(t),querySelector:()=>null,querySelectorAll:()=>[],
  getElementById(id){const find=e=>e.id===id?e:e.children.map(find).find(Boolean);return find(this.body);},
  addEventListener(){},removeEventListener(){}};
class Socket {
  constructor(){this.listeners=new Map();} addEventListener(n,f){this.listeners.set(n,f);}
  removeEventListener(n){this.listeners.delete(n);} dispatch(data){this.listeners.get('message')?.({data});}
  send(){throw Error('platform send forbidden');}
}
class Xhr extends Socket {open(){} getResponseHeader(){return 'application/json';}}
const bridge={_handleMessageFromWeixin(value){return value;},invoke(){},on(){}};
const browser={WeixinJSBridge:bridge,WebSocket:Socket,console:{info(){},warn(){}},
  addEventListener(){},removeEventListener(){},innerWidth:1200,innerHeight:900,getComputedStyle:()=>({visibility:'visible',display:'block',opacity:'1'}),
  fetch:async(url,options)=>{
    assert.ok(String(url).startsWith('/__live_archive/moderation/'));
    const path=String(url).replace(/^\/__live_archive\/moderation\/v[12]\/(?:tasks\/[^/]+\/)?/,'/v1/');
    const response=await fetch('http://'+address+path,{...options,headers:{...options.headers,Origin:origin}});
    if(path==='/v1/hello'&&response.ok) pairing=await response.clone().json();
    if(path==='/v1/raw') {rawRequests++;if(!lost&&response.ok){lost++;return new Response('{}',{status:503});}}
    return response;
  }};
browser.self=browser;browser.top=browser;
const context=vm.createContext({window:browser,document,location:{origin,pathname,href:origin+pathname},XMLHttpRequest:Xhr,
  crypto:require('node:crypto').webcrypto,URL,performance,TextDecoder,TextEncoder,AbortController,atob,btoa,Blob,
  ArrayBuffer,Uint8Array,Event,setTimeout,clearTimeout,setInterval,clearInterval,HTMLElement:Element,MutationObserver:class{observe(){}disconnect(){}}});
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const until=async f=>{let deadline=Date.now()+10000;while(!f()&&Date.now()<deadline)await sleep(50);assert.ok(f(),'condition timeout');};
(async()=>{
 vm.runInContext(fs.readFileSync(bootstrap,'utf8'),context);
 const sdk=browser.__liveArchiveModerationV1;await until(()=>sdk.getStatus().connected);
 if(wechat){document.getElementById('live-archive-moderation-entry').click();document.getElementById('live-archive-moderation-toggle').click();}
 const chunk={seq:1,capture_id:'fixture',captured_at_ms:Date.now(),channel:'native_bridge',category:'dispatch',encoding:'json_utf8_base64',part:0,parts:1,data:'e30='};
 const postRaw=(token,chunks=[chunk],requestOrigin=origin)=>fetch('http://'+address+'/v1/raw',{method:'POST',headers:{'Content-Type':'application/json',Origin:requestOrigin,Authorization:'Bearer '+token},body:JSON.stringify({v:1,connection_id:pairing.connection_id,binding_id:pairing.binding_id,chunks})});
 assert.equal((await postRaw('wrong-token')).status,422);
 assert.equal((await postRaw(pairing.token,[chunk],'https://unrelated.example')).status,403);
 assert.equal((await postRaw(pairing.token,[{...chunk,data:'x'.repeat(24577)}])).status,422);
 if(!enabled) assert.equal((await postRaw(pairing.token)).status,422);
 const entries=Array.from({length:80},(_,i)=>({msg_type:20009,client_msg_id:'gift-'+i,
  from_user_contact:{contact:{username:'user-'+i,nickname:i===79?'陌**':'孤**'}},
  to_user_contact:{contact:{username:'other-recipient'}},payload:JSON.stringify({reward_product_id:'rose',reward_product_count:1,reward_gift:{name:'玫瑰'}})}));
 const sample={app_msg_list:entries,unknown_message:{msg_type:55555,content:'未知消息',large:'x'.repeat(70000)},cookie:'do-not-store-cookie',nested:JSON.stringify({access_token:'do-not-store-token',content:'retain-me'})};
 let expected=2;
 if(wechat){assert.equal(bridge._handleMessageFromWeixin(sample),sample);bridge._handleMessageFromWeixin(sample);}
 else{const socket=new browser.WebSocket('wss://webcast.example.douyin.com/webcast/im/push');socket.dispatch(JSON.stringify(sample));socket.dispatch(JSON.stringify(sample));socket.dispatch(new Uint8Array([0,255,17,18]).buffer);expected++;}
 if(enabled){await until(()=>sdk.getStatus().raw.captured===expected&&sdk.getStatus().raw.buffered_chunks===0);assert.equal(lost,1);
  // An unserializable callback is an explicit gap, never a silent omission.
  const cycle={};cycle.self=cycle;sdk.captureRaw('native_bridge','fixture',cycle);
  sdk.captureRaw('native_bridge','fixture',{recovered:true});
  await until(()=>sdk.getStatus().raw.buffered_chunks===0);assert.equal(sdk.getStatus().raw.dropped,1);
 }else{await sleep(500);assert.equal(rawRequests,0);assert.equal(sdk.getStatus().raw.captured,0);}
 // Semantic events have a separate 50-event transport batch and acknowledgement.
 if(wechat&&enabled)await until(()=>sdk.getStatus().delivery.acknowledged['gift.candidate']===80);
 if(sdk.getStatus().task_id){bridge.invoke('joinLive',{},()=>{});assert.equal(sdk.getStatus().stopped,true);assert.equal(sdk.getStatus().decoder.collecting,false);}
 assert.equal(sdk.getStatus().send_chat,false);
 const status=sdk.getStatus();sdk.dispose();await sleep(100);
 process.stdout.write(JSON.stringify({lost,raw:status.raw,send_chat:false})+'\n');
})().catch(e=>{console.error(e);browser.__liveArchiveModerationV1?.dispose();process.exitCode=1;});
