const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const source=fs.readFileSync('assets/moderation/adapters/wechat-desktop.js','utf8');
const fan=source.slice(source.indexOf('const wxFanLevel='),source.indexOf('// Arrival candidates require'));
const activity=fan+source.slice(source.indexOf('let wxKeywordFloor='),source.indexOf('sdk.onDispose(()=>{wxActivitySeen.clear()'));
test('native arrival carries the actual fan badge rather than the viewer level',()=>{
 const events=[];
 const context=vm.createContext({Date,Set,Array,Number,JSON,wxCollecting:true,
  wxOwnValue:(v,k)=>v?.[k],wxIdentifier:v=>typeof v==='string'&&v.length>0,
  wxJsonObjectText:()=>false,wxBase64Json:()=>null,
  sdk:{getStatus:()=>({stopped:false,buffered:0}),emit:(type,channel,payload,identity)=>{events.push({type,payload,identity});return true;},onDispose(){}}});
 const arrival=source.slice(source.indexOf('const wxArrivalSeen ='),source.indexOf('sdk.onDispose(() => wxArrivalSeen.clear()'));
 vm.runInContext(fan+arrival+';globalThis.arrive=wxArrivalCandidate;',context);
 const row={type:10005,client_msg_id:'entry',content:'来了',finder_live_contact:{contact:{username:'user',nickname:'观众'},badge_infos:[{badge_type:2,badge_level:36},{badge_type:4,badge_level:7,badge_name:'心上怡'}]}};
 context.arrive('native_bridge',row);assert.equal(events.length,1);assert.equal(events[0].payload.fan_level,7);
});
test('unified timeline observes fresh public text without enabling keyword replies or timers',()=>{
 const events=[];
 const context=vm.createContext({Date,Set,BigInt,Array,Number,JSON,
  BOOT:{unified_ui:true},wxCollecting:true,
  wxOwnValue:(v,k)=>v?.[k],wxIdentifier:v=>typeof v==='string'&&v.length>0,
  wxGiftNickname:()=>null,wxJsonObjectText:v=>v.startsWith('{'),
  sdk:{clean:(s,n)=>s.slice(0,n),getStatus:()=>({buffered:0}),emit:(type,source,payload,identity)=>{events.push({type,payload,identity});return true;}}
 });
 vm.runInContext(activity+';globalThis.observe=wxObserveKeywords;',context);
 const row=(seq,content)=>({seq,type:1,client_msg_id:'text-'+seq,username:'viewer',nickname:'小雨',content});
 context.observe([row(1,'历史')]);assert.equal(events.length,0);
 const fresh=[{...row(2,'新留言'),finder_live_contact:{badge_infos:[{badge_type:2,badge_level:36},{badge_type:4,badge_level:6,badge_name:'心上怡'}]}},row(3,'另一条留言')];
 context.observe(fresh);context.observe(fresh);
 const observed=()=>events.filter(e=>e.type==='chat.observed');
 assert.deepEqual(observed().map(e=>e.payload.content),['新留言','另一条留言']);
 assert.equal(observed()[0].identity.nickname,'小雨');
 assert.equal(observed()[0].payload.fan_level,6);assert.equal(observed()[1].payload.fan_level,undefined);
 const ambiguous={...row(4,'多灯牌'),finder_live_contact:{badge_infos:[{badge_type:4,badge_level:6,badge_name:'心上怡'},{badge_type:4,badge_level:8,badge_name:'其他'}]}};
 context.observe([ambiguous]);assert.equal(observed()[2].payload.fan_level,undefined);
 context.wxCollecting=false;context.observe([row(5,'暂停期间')]);assert.equal(observed().length,3);
});
test('WeChat public list counts all fresh message types and authors, baselines history and deduplicates display gifts',()=>{
 const events=[];
 const context=vm.createContext({Date,Set,BigInt,Array,Number,JSON,
  BOOT:{timed_messages:{enabled:true,min_comments:3,count_scope:'public_screen'},keyword_replies:{enabled:false}},
  wxCollecting:true,wxOwnValue:(v,k)=>v?.[k],wxIdentifier:v=>typeof v==='string'&&v.length>0,
  wxJsonObjectText:v=>v.startsWith('{'),sdk:{getStatus:()=>({buffered:0}),emit:(type,source,payload,identity)=>{events.push({type,source,payload,identity});return true;}}
 });
 vm.runInContext(activity+';globalThis.observe=wxObserveKeywords;globalThis.gift=wxPublicActivity;',context);
 const entry=(seq,type,id,content='')=>({seq,type,client_msg_id:id,content});
 context.observe([entry(1,1,'historical','历史')]);assert.equal(events.length,0);
 const messages=[entry(2,1,'own','定时文案'),entry(3,10005,'arrival','来了'),entry(4,20013,'gift','{"gift":"flower"}'),entry(5,99,'system')];
 context.observe(messages);context.observe(messages);context.gift('gift',20013);
 assert.equal(events.length,4);assert.ok(events.every(e=>e.type==='public.activity'));
 assert.deepEqual(events.map(e=>e.identity.native_event_id),['own','arrival','gift','system']);
 context.gift('gift-display',20013);context.gift('gift-display',20013);assert.equal(events.length,5);
});

test('WeChat text_comments emits only fresh type-1 text including automatic text, never join or gift notifications',()=>{
 const events=[];
 const context=vm.createContext({Date,Set,BigInt,Array,Number,JSON,
  BOOT:{timed_messages:{enabled:true,min_comments:10,count_scope:'text_comments'},keyword_replies:{enabled:false}},
  wxCollecting:true,wxOwnValue:(v,k)=>v?.[k],wxIdentifier:v=>typeof v==='string'&&v.length>0,
  wxJsonObjectText:v=>v.startsWith('{'),sdk:{getStatus:()=>({buffered:0}),emit:(type,source,payload,identity)=>{events.push({type,payload,identity});return true;}}
 });
 vm.runInContext(activity+';globalThis.observe=wxObserveKeywords;globalThis.gift=wxPublicActivity;',context);
 const row=(seq,type,content)=>({seq,type,client_msg_id:'text-'+seq,username:'actor',content});
 context.observe([row(1,1,'历史评论')]);
 context.observe([row(2,10005,'来了'),row(3,20013,'礼物提示'),row(4,1,'场控自动文案'),row(5,1,'普通评论')]);
 context.gift('other-gift',20013);
 const counts=events.filter(e=>e.type==='chat.activity');
 assert.deepEqual(counts.map(e=>e.payload.content),['场控自动文案','普通评论']);
});


for(const unified of [true,false])test(`WeChat ${unified?'unified':'standalone'} collection respects its own lifecycle`,async()=>{
 let listener,latest;
 const state={stopped:false,draining:false};
 const context=vm.createContext({BOOT:{unified_ui:unified},wxKeywordFloor:100,wxMatchPending:[],wxRepairHooks(){},
  sdk:{getStatus:()=>state,setDecoderStatus:value=>latest=value,emit(){},onModerationChange:fn=>{listener=fn;if(unified)fn(false);}}});
 vm.runInContext(source.slice(0,source.indexOf('const wxRelevant =')),context);
 await new Promise(resolve=>setImmediate(resolve));
 if(unified)assert.equal(latest.collecting,true,'collection starts while moderation is initially off');
 listener(true);await new Promise(resolve=>setImmediate(resolve));
 assert.equal(latest.collecting,true);
 context.wxKeywordFloor=500;
 listener(false);await new Promise(resolve=>setImmediate(resolve));
 assert.equal(latest.collecting,unified,'unified collection continues after moderation closes');
 if(unified)assert.equal(context.wxKeywordFloor,500,'toggling moderation does not rebaseline public text');
 state.draining=true;listener(false);await new Promise(resolve=>setImmediate(resolve));
 assert.equal(latest.collecting,false,'service drain still stops observation');
});
