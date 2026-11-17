const fs=require('node:fs'),vm=require('node:vm'),assert=require('node:assert/strict');
const source=fs.readFileSync('assets/moderation/adapters/wechat-desktop.js','utf8');
const helper=source.slice(source.indexOf('let wxKeywordFloor='),source.indexOf('const wxMessageBatch ='));
const emitted=[];
const ctx={BOOT:{automatic_send:true,keyword_replies:{enabled:true,rules:[{keywords:['胎记'],reply:'手臂是胎记'}]}},wxCollecting:true,
 sdk:{getStatus:()=>({buffered:0}),onDispose(){},emit:(...e)=>{emitted.push(e);return true;},observeServerChat(){}},
 wxOwnValue:(o,k)=>o?.[k],wxIdentifier:v=>typeof v==='string'&&!!v,wxJsonObjectText:()=>false,
 deliverySequence:v=>{try{return v===undefined?null:BigInt(v);}catch{return null;}},wxPollSucceeded:b=>b?.ret===0,Date};
vm.createContext(ctx);vm.runInContext(helper,ctx);
const poll=(seq,text='胎记',ret=0)=>{ctx.input={baseResponse:{ret},msg_list:[{type:1,seq,client_msg_id:'id'+seq,username:'viewer',content:text}]};vm.runInContext('wxObservePollChats(input,Date.now(),true)',ctx);};
const kinds=kind=>emitted.filter(e=>e[0]===kind);
poll(100);assert.equal(emitted.length,0,'first poll is history');
poll(101,'胎记',1);assert.equal(emitted.length,0,'failed poll');
poll(99);assert.equal(emitted.length,0,'old sequence');
poll(101);assert.equal(kinds('chat.activity').length,1);
poll(101);assert.equal(kinds('chat.activity').length,1,'duplicate');
poll(102,'你好');assert.equal(kinds('chat.activity').length,2,'unmatched text is sent for backend evaluation');
vm.runInContext('wxCollecting=false',ctx);poll(103);assert.equal(kinds('chat.activity').length,2,'paused');
vm.runInContext('wxCollecting=true;wxKeywordFloor=null',ctx);poll(104);assert.equal(kinds('chat.activity').length,2,'resume baseline');
poll(105);assert.equal(kinds('chat.activity').length,3);
console.log('keyword native poll tests passed');

// All observation types remain available regardless of boot-time rules.
ctx.BOOT.keyword_replies.enabled=false;ctx.BOOT.timed_messages={enabled:false,min_comments:0};
vm.runInContext('wxKeywordFloor=null',ctx);emitted.length=0;
poll(200,'历史评论');assert.equal(emitted.length,0);
poll(201,'普通评论');assert.equal(kinds('chat.activity').length,1);assert.equal(kinds('public.activity').length,1);
poll(201,'重复');assert.equal(emitted.length,2);
poll(202,'另一条普通评论');assert.equal(kinds('chat.activity').length,2);
ctx.BOOT.keyword_replies.enabled=true;poll(203,'胎记');
assert.equal(kinds('chat.activity').length,3);
console.log('timed activity native poll tests passed');

