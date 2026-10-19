const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const sdkSource=fs.readFileSync('assets/moderation/sdk/probe.js','utf8'),panel=fs.readFileSync('assets/moderation/sdk/panel.js','utf8');
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn){for(let i=0;i<100&&!fn();i++)await sleep(30);assert.ok(fn(),'condition timeout');}
for(const platform of ['douyin','wechat'])test(platform+' shows one entry, switches both ways, records while off and rejects an old poll',async()=>{
  class Node{constructor(tag){this.tagName=tag;this.children=[];this.style={};this.attrs={};this.textContent='';}appendChild(n){this.children.push(n);return n;}setAttribute(k,v){this.attrs[k]=v;}}
  const body=new Node('body'),find=(node,id)=>node.id===id?node:node.children.map(n=>find(n,id)).find(Boolean);
  const document={body,readyState:'complete',getElementById:id=>find(body,id),createElement:t=>new Node(t),addEventListener(){}};
  let state={enabled:true,epoch:0},writes=0,recordings=0,oldResolve=null,hold=false,remoteRequest=null;const receipts=[],reports=[];
  const room=platform==='douyin'?'https://live.douyin.com/123':'https://channels.weixin.qq.com/web/pages/live';const loc=new URL(room);
  const command={v:1,command:'fill_and_submit',action_id:'stale-action',connection_id:'conn',binding_id:'binding',document_id:'doc',page_url:room,remaining_ttl_ms:10000,text:'旧回复'};
  const response=v=>({ok:true,json:async()=>v});
  const browser={addEventListener(){},removeEventListener(){},console,__liveArchiveRecordingController:{start:async()=>{recordings++;}}};
  browser.fetch=async(path,options)=>{const data=JSON.parse(options.body);if(path.endsWith('/hello'))return response({connection_id:'conn',binding_id:'binding',token:'token'});if(path.endsWith('/control')){state={enabled:data.enabled,epoch:state.epoch+1};return response({...state});}if(path.endsWith('/events'))reports.push(...data.events);if(path.endsWith('/results')){receipts.push(data);return response({acked_action_id:data.action_id});}if(path.endsWith('/poll')){if(hold){hold=false;return new Promise(resolve=>{oldResolve=()=>resolve(response({mode:'fill_and_send',send_chat:true,commands:[command],moderation_control:{enabled:true,epoch:0}}));});}return response({mode:'fill_and_send',send_chat:true,commands:[],moderation_control:{...state},recording_request:remoteRequest});}return response({acked_seq:data.events?.at(-1)?.seq||0});};
  const context={window:browser,document,location:{origin:loc.origin,pathname:loc.pathname},crypto:{randomUUID:()=> 'doc'},URL,AbortController,performance,setTimeout,clearTimeout,setInterval:(fn,ms)=>{const t=setInterval(fn,ms);t.unref();return t;},clearInterval};
  vm.createContext(context);const boot={room_url:room,prefix:'/local/',ticket:'ticket',adapter_id:platform==='douyin'?'douyin.live_page':'wechat_channels.desktop_probe',automatic_send:true,fill_preview:true,recording:true};
  vm.runInContext(`(()=>{const BOOT=${JSON.stringify(boot)};${sdkSource}\nfillExecutor={getStatus:()=>({input_ready:true}),execute:async()=>{window.writes();return {outcome:'submitted',reason:'input_submitted'};}};${panel}})()`,vm.createContext(Object.assign(context,{window:Object.assign(browser,{writes:()=>writes++})})));
  const sdk=browser.__liveArchiveModerationV1;
  await until(()=>sdk.getStatus().connected);const root=document.getElementById('live-archive-assistant');assert.ok(root);const launcher=root.children[0],details=root.children[1];
  assert.equal(details.hidden,true,'starts collapsed');assert.equal(launcher.attrs['aria-expanded'],'false');
  launcher.onclick();assert.equal(details.hidden,false);assert.equal(launcher.attrs['aria-expanded'],'true');assert.equal(writes,0);
  const toggle=details.children.find(n=>n.attrs.role==='switch'),record=details.children.find(n=>n.textContent==='录制直播');
  assert.equal(record.hidden,false);launcher.onclick();assert.equal(details.hidden,true);assert.equal(sdk.getStatus().moderation_enabled,true,'collapsing never switches moderation');launcher.onclick();await toggle.onclick();assert.equal(sdk.getStatus().moderation_enabled,false);await record.onclick();assert.equal(recordings,1);assert.equal(writes,0);
  await toggle.onclick();assert.equal(sdk.getStatus().moderation_enabled,true);
  hold=true;await until(()=>oldResolve);await sdk.setModerationEnabled(false);await sdk.setModerationEnabled(true);oldResolve();await until(()=>receipts.length);
  assert.equal(writes,0);assert.equal(receipts[0].outcome,'rejected');assert.equal(sdk.getStatus().moderation_enabled,true,'late poll does not undo switch');await sdk.setModerationEnabled(false);remoteRequest='remote-request';await until(()=>recordings===2);await until(()=>reports.some(e=>e.event_type==='recording.result'));await sleep(platform==='wechat'?2100:600);assert.equal(recordings,2,'repeated poll never repeats recording');assert.equal(reports.filter(e=>e.event_type==='recording.result').length,1);sdk.dispose();
});
