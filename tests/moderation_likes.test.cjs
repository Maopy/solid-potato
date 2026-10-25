const test=require('node:test'),assert=require('node:assert/strict'),vm=require('node:vm'),fs=require('node:fs');
const source=fs.readFileSync('assets/moderation/sdk/likes.js','utf8');
function scheduler(extra={}){
 let now=new Date(2026,9,21,12,3).getTime(),visible=true,room=true,available=true,actions=0;
 const config={mode:'clock',interval_ms:800,every_minutes:10,batch_count:200,missed:'catch_up_once',...extra};
 const c={Date,io:{now:()=>now,visible:()=>visible,inRoom:()=>room,action:async active=>{if(!available||!active())return false;actions++;return true;}},config};vm.createContext(c);vm.runInContext(source+';globalThis.engine=createLikeScheduler(config,io);',c);
 return {engine:c.engine,actions:()=>actions,time:(h,m,s=0)=>{now=new Date(2026,9,21,h,m,s).getTime();},advance:ms=>now+=ms,visible:v=>visible=v,room:v=>room=v,available:v=>available=v};
}
test('natural ten-minute batch waits for the wall clock and attempts exactly 200 actions',async()=>{
 const s=scheduler();s.engine.setEnabled(true);await s.engine.step();assert.equal(s.actions(),0);
 s.time(12,9,59);await s.engine.step();assert.equal(s.actions(),0);
 s.time(12,10);await s.engine.step();assert.equal(s.actions(),1);
 for(let i=1;i<200;i++){s.advance(800);await s.engine.step();}
 assert.equal(s.actions(),200);assert.equal(s.engine.getStatus().remaining,0);s.advance(800);await s.engine.step();assert.equal(s.actions(),200);
});
test('hidden tab crossing many slots catches up at most one whole batch',async()=>{
 const s=scheduler();s.engine.setEnabled(true);s.visible(false);s.time(13,1);await s.engine.step();assert.equal(s.actions(),0);
 s.visible(true);await s.engine.step();for(let i=1;i<200;i++){s.advance(800);await s.engine.step();}
 assert.equal(s.actions(),200);s.advance(800);await s.engine.step();assert.equal(s.actions(),200);assert.equal(s.engine.getStatus().queued_batch,false);
});
test('missing target never builds multiple untouched batches and a configurable skip mode skips missed slots',async()=>{
 const s=scheduler();s.engine.setEnabled(true);s.available(false);s.time(12,10);await s.engine.step();s.time(13,1);await s.engine.step();s.available(true);await s.engine.step();
 for(let i=1;i<200;i++){s.advance(800);await s.engine.step();}assert.equal(s.actions(),200);assert.equal(s.engine.getStatus().remaining,0);
 const skip=scheduler({missed:'skip'});skip.engine.setEnabled(true);skip.time(12,15);await skip.engine.step();assert.equal(skip.actions(),0);skip.time(12,20);await skip.engine.step();assert.equal(skip.actions(),1);
});
test('continuous mode never burst-replays a throttled timer and disabling or changing rooms stops it',async()=>{
 const s=scheduler({mode:'continuous',interval_ms:1000});s.engine.setEnabled(true);await s.engine.step();s.time(14,1);await s.engine.step();assert.equal(s.actions(),2);await s.engine.step();assert.equal(s.actions(),2);
 s.engine.setEnabled(false);s.advance(1000);await s.engine.step();assert.equal(s.actions(),2);
 s.engine.setEnabled(true);s.room(false);await s.engine.step();assert.equal(s.engine.getStatus().enabled,false);assert.equal(s.actions(),2);
});
test('batch size and natural period are configurable and clock rollback does not replay',async()=>{
 const s=scheduler({every_minutes:15,batch_count:3});s.engine.setEnabled(true);s.time(12,15);await s.engine.step();s.advance(800);await s.engine.step();s.advance(800);await s.engine.step();assert.equal(s.actions(),3);
 s.time(11,3);await s.engine.step();assert.equal(s.actions(),3);s.time(11,15);await s.engine.step();assert.equal(s.actions(),4);
});


test('a repeated wall-clock boundary after rollback is not counted a second time',async()=>{
 const s=scheduler({batch_count:1,every_minutes:15});s.engine.setEnabled(true);s.time(12,15);await s.engine.step();assert.equal(s.actions(),1);
 s.time(12,1);await s.engine.step();s.time(12,15);await s.engine.step();assert.equal(s.actions(),1);
});

function controller(platform='wechat_channels.desktop_probe',extra={}){
 const listeners=new Map(),timers=[],intervals=[],saved=new Map();const keys=[];let clicks=0,room=true,moderation=false,recording=false,now=1000;
 const body={tagName:'BODY',children:[],dispatchEvent:e=>keys.push({type:e.type,key:e.key,code:e.code,keyCode:e.keyCode,repeat:e.repeat})},node={tagName:'BUTTON',parentElement:body,isConnected:true,disabled:false,children:[],
  matches:s=>s==='button,[role="button"]',closest:()=>null,contains:n=>n===node,getBoundingClientRect:()=>({left:10,top:10,width:20,height:20}),dispatchEvent:()=>{clicks++;}};body.children=[node];
 const document={body,activeElement:body,documentElement:{},hidden:false,querySelectorAll:q=>q.startsWith('body >')||q.startsWith('button[')||q.startsWith('div.')?[node]:[],querySelector:()=>node,elementFromPoint:()=>node,
  addEventListener:(name,fn)=>listeners.set(name,fn),removeEventListener:(name,fn)=>{if(listeners.get(name)===fn)listeners.delete(name);}};
 const window={addEventListener:(name,fn)=>listeners.set(name,fn),removeEventListener:(name,fn)=>{if(listeners.get(name)===fn)listeners.delete(name);},getComputedStyle:()=>({cursor:'pointer'}),localStorage:{getItem:k=>saved.get(k),setItem:(k,v)=>saved.set(k,v)},MouseEvent:class{},KeyboardEvent:class{constructor(type,options){this.type=type;Object.assign(this,options);}},WXU:{check_live_existing:()=>[null,{contact:{username:'anchor'}}]}};
 node.__reactProps$test={onClick:()=> '登录后即可点赞'};
 const context={window,document,Date:{now:()=>now},BOOT:{adapter_id:platform,room_url:'https://live.douyin.com/123',like_period_ledger:extra.mode==='clock'},likeConfig:{mode:'continuous',interval_ms:800,every_minutes:10,batch_count:200,missed:'catch_up_once',...extra},location:{origin:'https://test',pathname:'/live'},sdk:{getStatus:()=>({stopped:false,moderation_enabled:moderation,recording}),claimLikePeriod:async()=>({claimed:true,period_end_ms:600000})},sameRoom:()=>room,
  setInterval:(fn,ms)=>{timers.push(fn);intervals.push(ms);return fn;},clearInterval:fn=>timers.splice(timers.indexOf(fn),1),setTimeout,clearTimeout};
 vm.createContext(context);vm.runInContext(source+';globalThis.controller=createLikeController(likeConfig);',context);
 return {c:context.controller,node,document,timers,intervals,saved,keys,advance:ms=>now+=ms,clicks:()=>clicks,room:v=>room=v,event:type=>{let prevented=false,stopped=false;listeners.get(type)?.({type,target:node,composedPath:()=>[node,body],preventDefault:()=>prevented=true,stopImmediatePropagation:()=>stopped=true});return {prevented,stopped};}};
}
test('target picker consumes the complete selection gesture without liking, saves its selector and cleans up',async()=>{
 const s=controller();assert.equal(s.c.getStatus().enabled,false);assert.equal(s.clicks(),0);
 s.c.pick();assert.throws(()=>s.c.setEnabled(true),/定位/);
 for(const type of ['pointerdown','mousedown','pointerup','mouseup','click','dblclick'])assert.deepEqual(s.event(type),{prevented:true,stopped:true});
 assert.equal(s.clicks(),0);assert.equal(s.c.getStatus().picking,false);assert.equal(s.saved.size,1);assert.match(s.c.getStatus().selector,/body > button/);
 s.c.dispose();assert.deepEqual(s.event('click'),{prevented:false,stopped:false});assert.equal(s.timers.length,0);
});
test('likes operate while moderation and recording are off, and stop on room change or hidden page',async()=>{
 const s=controller();s.c.setEnabled(true);await Promise.resolve();await s.timers[0]();assert.equal(s.clicks(),1);
 s.c.setEnabled(false);s.document.hidden=true;s.c.setEnabled(true);await Promise.resolve();await s.timers[0]();assert.equal(s.clicks(),1);
 s.document.hidden=false;s.room(false);await s.timers[0]();assert.equal(s.c.getStatus().enabled,false);assert.equal(s.clicks(),1);s.c.dispose();
});
test('Douyin holds Z, repeats without releasing, and releases on stop, hidden page, editor focus and room exit',async()=>{
 const s=controller('douyin.live_page');s.document.querySelectorAll=()=>[];
 const down=repeat=>({type:'keydown',key:'z',code:'KeyZ',keyCode:90,repeat});
 const up={type:'keyup',key:'z',code:'KeyZ',keyCode:90,repeat:false};
 s.c.setEnabled(true);await Promise.resolve();await s.timers[0]();
 assert.deepEqual(s.keys,[down(false)]);assert.equal(s.clicks(),0);
 s.advance(800);await s.timers[0]();assert.deepEqual(s.keys,[down(false),down(true)]);
 // Re-enable resets the scheduler's clock, while an existing hold is released first.
 s.c.setEnabled(false);assert.deepEqual(s.keys,[down(false),down(true),up]);
 s.document.activeElement={isContentEditable:true};s.c.setEnabled(true);await s.timers[0]();
 assert.equal(s.keys.length,3);assert.equal(s.c.getStatus().reason,'input_busy');
 s.document.activeElement=s.document.body;await s.timers[0]();assert.deepEqual(s.keys.at(-1),down(false));
 s.document.activeElement={matches:()=>true};s.event('focusin');assert.deepEqual(s.keys.at(-1),up);
 const count=s.keys.length;await s.timers[0]();assert.equal(s.keys.length,count);
 s.c.setEnabled(false);s.document.activeElement=s.document.body;s.c.setEnabled(true);await Promise.resolve();
 s.document.hidden=true;s.event('visibilitychange');assert.deepEqual(s.keys.at(-1),up);
 s.document.hidden=false;s.c.setEnabled(false);s.c.setEnabled(true);await Promise.resolve();
 s.room(false);await s.timers[0]();assert.deepEqual(s.keys.at(-1),up);assert.equal(s.c.getStatus().enabled,false);
 s.c.dispose();assert.equal(s.timers.length,0);
});

test('a held Z produces repeat keydown events and one final keyup when a clock batch finishes',async()=>{
 const s=controller('douyin.live_page');s.c.dispose();
 // Exercise scheduler lifecycle separately from wall-clock scheduling in the controller fixture.
 let now=0,held=false;const events=[];
 const c={Date,config:{mode:'clock',every_minutes:10,batch_count:3,interval_ms:800,missed:'catch_up_once'},io:{now:()=>now,inRoom:()=>true,visible:()=>true,claim:async()=>({claimed:true,period_end_ms:600000}),action:async()=>{events.push(held?'repeat':'down');held=true;return true;},release:()=>{if(held)events.push('up');held=false;}}};
 vm.createContext(c);vm.runInContext(source+';globalThis.engine=createLikeScheduler(config,io);',c);
 c.engine.setEnabled(true);await c.engine.step();now+=800;await c.engine.step();now+=800;await c.engine.step();
 assert.deepEqual(events,['down','repeat','repeat','up']);assert.equal(c.engine.getStatus().remaining,0);
});

test('WeChat skips clicks while any text editor owns focus and resumes without altering the editor',async()=>{
 const s=controller();const editor={isContentEditable:true,textContent:'保留我的草稿',focusCount:0,focus(){this.focusCount++;}};
 s.document.activeElement=editor;s.c.setEnabled(true);await Promise.resolve();await s.timers[0]();assert.equal(s.clicks(),0);assert.equal(s.c.getStatus().reason,'input_busy');assert.equal(s.document.activeElement,editor);assert.equal(editor.textContent,'保留我的草稿');assert.equal(editor.focusCount,0);
 s.document.activeElement=s.document.body;await s.timers[0]();assert.equal(s.clicks(),1);s.c.dispose();
});
test('WeChat automatic detection follows the live player corner and rejects other controls and ambiguous targets',()=>{
 const rect=(left,top,width,height)=>({left,top,width,height,right:left+width,bottom:top+height});
 const make=(label,r)=>({isConnected:true,getAttribute:k=>k==='class'?label:null,getBoundingClientRect:()=>r,closest(){return this;},contains(n){return n===this;}});
 const good=make('live-like-btn',rect(620,790,32,50)),share=make('live-share-btn',rect(570,790,32,50)),recommend=make('live-like-btn',rect(900,790,32,50)),other=make('live-like-btn',rect(580,790,32,50));
 let candidates=[share,recommend,good];
 const doc={querySelectorAll:q=>q.startsWith('.controls__ft')?[]:q==='video'?[{isConnected:true,getBoundingClientRect:()=>rect(0,120,665,600)}]:candidates};
 const c={document:doc,window:{innerHeight:850,getComputedStyle:()=>({cursor:'pointer'})}};vm.createContext(c);vm.runInContext(source+';globalThis.find=()=>findWechatLikeTarget(document,window,()=>true);',c);
 assert.equal(c.find(),good);candidates=[good,other];assert.equal(c.find(),null);candidates=[share,recommend];assert.equal(c.find(),null);
});
test('backend ledger catches up immediately on enable and prevents repeats after toggling or reloading',async()=>{
 let now=new Date(2026,9,22,1,3).getTime(),calls=0,actions=0;const periods=new Set();
 const io={now:()=>now,inRoom:()=>true,visible:()=>true,action:async()=>{actions++;return true;},claim:async()=>{calls++;const start=Math.floor(now/600000)*600000,claimed=!periods.has(start);periods.add(start);return {claimed,period_end_ms:start+600000};}};
 const c={Date,io,config:{mode:'clock',interval_ms:800,every_minutes:10,batch_count:3,missed:'catch_up_once'}};vm.createContext(c);vm.runInContext(source+';globalThis.make=()=>createLikeScheduler(config,io);',c);
 const first=c.make();first.setEnabled(true);await first.step();assert.equal(actions,1);for(let i=0;i<2;i++){now+=800;await first.step();}assert.equal(actions,3);
 first.setEnabled(false);first.setEnabled(true);await first.step();assert.equal(actions,3);assert.equal(first.getStatus().remaining,0);
 const reloaded=c.make();reloaded.setEnabled(true);await reloaded.step();assert.equal(actions,3);assert.equal(calls,3);
 now=new Date(2026,9,22,1,10).getTime();await reloaded.step();assert.equal(actions,4);
});
test('backend failure never falls back to local likes, and disabling during a claim prevents execution',async()=>{
 let now=1000,actions=0,resolve;
 const c={Date,io:{now:()=>now,inRoom:()=>true,visible:()=>true,action:async()=>{actions++;return true;},claim:async()=>{throw Error('offline');}},config:{mode:'clock',interval_ms:800,every_minutes:10,batch_count:200,missed:'catch_up_once'}};
 vm.createContext(c);vm.runInContext(source+';globalThis.engine=createLikeScheduler(config,io);',c);c.engine.setEnabled(true);await c.engine.step();assert.equal(actions,0);assert.equal(c.engine.getStatus().reason,'backend_unavailable');
 now+=3000;c.io.claim=()=>new Promise(r=>resolve=r);const pending=c.engine.step();c.engine.setEnabled(false);resolve({claimed:true,period_end_ms:600000});await pending;assert.equal(actions,0);assert.equal(c.engine.getStatus().enabled,false);
});
test('the confirmed Channels native cheers selector is unique and independent of screen coordinates',()=>{
 const node={};let candidates=[node];const c={doc:{querySelectorAll:q=>q==='.controls__ft .cheers__wrp .live-opr__btn'?candidates:[]},win:{}};
 vm.createContext(c);vm.runInContext(source+';globalThis.find=()=>findWechatLikeTarget(doc,win,()=>true);',c);assert.equal(c.find(),node);candidates=[node,{}];assert.equal(c.find(),null);
});

test('Douyin clock batches hold Z at keyboard repeat speed, independently of mouse interval',async()=>{
 const s=controller('douyin.live_page',{mode:'clock',interval_ms:800});
 assert.equal(s.intervals[0],33);
 s.c.setEnabled(true);await Promise.resolve();await Promise.resolve();await s.timers[0]();
 for(let i=1;i<200;i++){s.advance(33);await s.timers[0]();}
 assert.equal(s.keys.length,201);
 assert.equal(s.keys[0].type,'keydown');assert.equal(s.keys[0].repeat,false);
 assert.ok(s.keys.slice(1,200).every(e=>e.type==='keydown'&&e.repeat===true));
 assert.equal(s.keys[200].type,'keyup');assert.equal(s.clicks(),0);
 assert.equal(s.c.getStatus().total_attempts,200);assert.equal(s.c.getStatus().remaining,0);
 assert.equal(s.c.getStatus().key_held,false);
 s.advance(3300);await s.timers[0]();assert.equal(s.keys.length,201);
 s.c.dispose();
 const wx=controller();assert.equal(wx.intervals[0],800);wx.c.dispose();
});
