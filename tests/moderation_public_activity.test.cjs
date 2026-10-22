const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const vm=require('node:vm');
const source=fs.readFileSync('assets/moderation/adapters/wechat-desktop.js','utf8');
const activity=source.slice(source.indexOf('let wxKeywordFloor='),source.indexOf('sdk.onDispose(()=>{wxKeywordSeen.clear()'));
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
 assert.deepEqual(events.map(e=>e.type),['chat.activity','chat.activity']);
 assert.deepEqual(events.map(e=>e.payload.content),['场控自动文案','普通评论']);
});
