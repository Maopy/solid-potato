const test=require('node:test'),assert=require('node:assert/strict'),vm=require('node:vm'),fs=require('node:fs');
const source=fs.readFileSync('assets/moderation/adapters/douyin-recording.js','utf8');
function setup(){
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
  const context={document,fetch,location:{origin:'https://live.douyin.com',pathname:'/123',href:'https://live.douyin.com/123'},performance:{getEntriesByType:()=>[{name:'https://media.example.test/current.flv'}]},setInterval(){},clearInterval(){},URL,console};context.window=context;
  vm.createContext(context);vm.runInContext(source,context);return {context,nodes,requests,body,get platformCalls(){return platformCalls;}};
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
