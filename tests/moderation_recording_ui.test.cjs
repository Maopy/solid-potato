const test=require('node:test'),assert=require('node:assert/strict'),vm=require('node:vm'),fs=require('node:fs');
const source=fs.readFileSync('assets/moderation/adapters/douyin-recording.js','utf8');
function setup(options={}){
  const nodes=[],requests=[];let platformCalls=0;
  class Node {
    constructor(tag){this.tagName=tag;this.children=[];this.style={};this.hidden=false;this.textContent='';this.value='';}
    append(...items){this.children.push(...items);if(this.tagName==='select'&&!this.value)this.value=items[0]?.value||'';}
    appendChild(item){this.append(item);return item;}
    replaceChildren(...items){this.children=items;this.value='';this.append(...items);}
    remove(){this.removed=true;}
  }
  const body=new Node('body');
  const document={body,readyState:'complete',title:'测试主播',createElement:tag=>{const n=new Node(tag);nodes.push(n);return n;},querySelectorAll:selector=>selector==='video'?[{currentSrc:'blob:ignored'}]:[]};
  const fetch=async(url,options)=>{
    if(!String(url).startsWith('/api/v1/download_task/')){platformCalls++;return {clone:()=>({json:async()=>({})})};}
    requests.push({url,body:options?.body&&JSON.parse(options.body)});
    return {ok:true,json:async()=>({code:0,data:url.endsWith('/list')?{list:[{id:'task',name:'测试主播',status:'downloading'}]}:{tasks:[]}})};
  };
  let observer,disposer;
  const context={document,fetch,location:{origin:'https://live.douyin.com',pathname:'/123',href:'https://live.douyin.com/123'},performance:{getEntriesByType:()=>options.empty?[]:[{name:'https://media.example.test/current.flv'}]},setInterval(){},clearInterval(){},URL,console,sdk:{onDispose:fn=>disposer=fn},PerformanceObserver:class{constructor(fn){observer=fn;}observe(){}disconnect(){}}};context.window=context;
  vm.createContext(context);vm.runInContext(source,context);return {context,nodes,requests,body,observe:entries=>observer({getEntries:()=>entries}),dispose:()=>disposer?.(),get platformCalls(){return platformCalls;}};
}
test('recording is explicit, preserves native fetch, and pause/resume use download tasks without chat',async()=>{
  const e=setup();assert.equal(e.requests.length,0);
  const record=e.nodes.find(n=>n.textContent==='录制直播');await record.onclick();
  assert.equal(e.requests[0].url,'/api/v1/download_task/create');
  assert.equal(e.requests[0].body.objects[0].platform,'douyin');
  assert.equal(e.requests[0].body.objects[0].content.live.stream_url,'https://media.example.test/current.flv');
  await e.nodes.find(n=>n.textContent==='暂停').onclick();
  assert.equal(e.requests.find(r=>r.url.endsWith('/pause')).body.task_ids[0],'task');
  const count=e.body.children.length;vm.runInContext(source,e.context);assert.equal(e.body.children.length,count);
  assert.equal(e.platformCalls,0);
});
test('room navigation refuses stale stream submission',async()=>{
  const e=setup();e.context.location.pathname='/999';await e.nodes.find(n=>n.textContent==='录制直播').onclick();
  assert.equal(e.requests.length,0);assert.ok(e.nodes.some(n=>n.textContent.includes('直播间已切换')));
});
test('room-scoped metadata supplies blob playback URLs and rejects unrelated rooms',async()=>{
 const e=setup({empty:true});
 const value={data:{data:[{stream_url:{live_core_sdk_data:{pull_data:{stream_data:JSON.stringify({data:{origin:{main:{flv:'https://media.example.test/current-room.flv'}}}})}}}}]}};
 e.context.sdk.observeRoomMetadata(value,'https://live.douyin.com/webcast/room/web/enter?web_rid=999');
 await assert.rejects(e.context.__liveArchiveRecordingController.start(),/请先进入并播放/);
 e.context.sdk.observeRoomMetadata(value,'https://live.douyin.com/webcast/room/web/enter?web_rid=123');
 await e.context.__liveArchiveRecordingController.start();
 assert.equal(e.requests[0].body.objects[0].content.live.stream_url,'https://media.example.test/current-room.flv');
 e.dispose();assert.equal(e.context.sdk.observeRoomMetadata,undefined);
});
test('resource observer retains loaded media after the timing buffer is cleared',async()=>{
 const e=setup({empty:true});e.observe([{name:'https://media.example.test/retained.m3u8'}]);
 await e.context.__liveArchiveRecordingController.start();
 assert.equal(e.requests[0].body.objects[0].content.live.stream_url,'https://media.example.test/retained.m3u8');
 e.dispose();
});


test('recording uses owner nickname from current-room metadata instead of the page title',async()=>{
 const e=setup({empty:true});e.context.document.title='带装饰的直播标题 - 抖音直播';
 e.context.sdk.observeRoomMetadata({data:{data:[{owner:{nickname:'主播甲'},stream_url:{flv_pull_url:{origin:'https://media.example.test/anchor.flv'}}}]}},'https://live.douyin.com/webcast/room/web/enter?web_rid=123');
 await e.context.__liveArchiveRecordingController.start();assert.equal(e.requests[0].body.objects[0].content.profile.nickname,'主播甲');e.dispose();
});
test('title fallback strips only the known Douyin page suffix',async()=>{
 const e=setup();e.context.document.title='心怡076的抖音直播间 - 抖音直播';
 await e.context.__liveArchiveRecordingController.start();assert.equal(e.requests[0].body.objects[0].content.profile.nickname,'心怡076');e.dispose();
});

test('highest advertised quality wins regardless of arrival order, current playback, or stale selection',async()=>{
 const e=setup();
 const url='https://live.douyin.com/webcast/room/web/enter?web_rid=123';
 e.context.sdk.observeRoomMetadata({data:{data:[{stream_url:{flv_pull_url:{SD1:'https://media.example.test/sd.flv',FULL_HD1:'https://media.example.test/fhd.flv',ORIGIN:'https://media.example.test/origin.flv?token=unchanged',HD1:'https://media.example.test/hd.flv'}}}]}},url);
 e.observe([{name:'https://media.example.test/sd.flv'}]);
 const select=e.nodes.find(n=>n.tagName==='select');select.value='https://media.example.test/sd.flv';
 await e.context.__liveArchiveRecordingController.start();
 assert.equal(e.requests[0].body.objects[0].content.live.stream_url,'https://media.example.test/origin.flv?token=unchanged');
 assert.equal(select.disabled,true);e.dispose();
});
test('nested SDK quality labels and bitrate survive resource refreshes',async()=>{
 const e=setup();
 const stream_data=JSON.stringify({data:{ld:{main:{flv:'https://media.example.test/ld.flv'}},origin:{main:{flv:'https://media.example.test/origin-high.flv',sdk_params:JSON.stringify({resolution:'1080x1920',vbitrate:6000000})},backup:{flv:'https://media.example.test/origin-low.flv',sdk_params:JSON.stringify({resolution:'1080x1920',vbitrate:3000000})}}}});
 e.context.sdk.observeRoomMetadata({data:{data:[{stream_url:{live_core_sdk_data:{pull_data:{stream_data}}}}]}},'https://live.douyin.com/webcast/room/web/enter?web_rid=123');
 e.observe([{name:'https://media.example.test/origin-high.flv'}]);
 await e.context.__liveArchiveRecordingController.start();
 assert.equal(e.requests[0].body.objects[0].content.live.stream_url,'https://media.example.test/origin-high.flv');e.dispose();
});
