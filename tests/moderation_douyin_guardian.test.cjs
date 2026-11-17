const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const source=fs.readFileSync('assets/moderation/adapters/douyin-decoder.js','utf8');
const encode=new TextEncoder();
const vint=value=>{let n=BigInt(value),out=[];do{out.push(Number(n&127n)|(n>127n?128:0));n>>=7n;}while(n);return out;};
const field=(key,value)=>typeof value==='number'||typeof value==='bigint'?[...vint(key*8),...vint(value)]:[...vint(key*8+2),...vint(value.length),...value];
const pb=(...fields)=>new Uint8Array(fields.flat());
const text=value=>encode.encode(value);
const userId='9007199254740993123',anchor='9007199254740993333',room='9007199254740993222';
test('chat carries only the current anchor fan badge',async()=>{
 for(const [badgeAnchor, expected] of [[anchor,14],['123',undefined]]){
  const events=[];const sdk={emit:(type,kind,payload,identity)=>events.push({type,payload,identity}),onDispose(){},setDecoderStatus(){}};
  const context=vm.createContext({sdk,BOOT:{rules_preview:true,room_url:'https://live.douyin.com/123'},location:{href:'https://live.douyin.com/123'},document:{querySelectorAll:()=>[]},URL,TextDecoder,TextEncoder,Uint8Array,ArrayBuffer,Blob,DecompressionStream,setTimeout,clearTimeout});
  vm.runInContext(source+'\nglobalThis.decoderUnderTest=douyinDecoder;',context);
  context.decoderUnderTest.bindRoom({data:{data:[{id_str:room,owner:{id_str:anchor}}]}},'https://live.douyin.com/webcast/room/web/enter?web_rid=123');
  const who=pb(field(1,BigInt(userId)),field(3,text('观众')),field(24,pb(field(1,pb(field(2,14),field(6,BigInt(badgeAnchor)))))));
  const common=pb(field(1,text('WebcastChatMessage')),field(2,1001),field(3,BigInt(room)),field(4,Math.floor(Date.now()/1000)));
  const body=pb(field(1,common),field(2,who),field(3,text('你好'))),message=pb(field(1,text('WebcastChatMessage')),field(2,body),field(3,1001));
  context.decoderUnderTest.observe(pb(field(7,text('pb')),field(8,pb(field(1,message)))).buffer,'wss://webcast.example.douyin.com/webcast/im/push?room_id='+room);
  await new Promise(r=>setTimeout(r,20));assert.equal(events.find(e=>e.type==='chat.received')?.payload.fan_level,expected);
 }
});
const entry=(options={})=>{
 const {effect='star_guard',target=userId,clubAnchor=anchor,nickname='合成观众',action=1,age=0,duplicate=false}=options;
 const log=(key,value)=>field(16,pb(field(1,text(key)),field(2,text(value))));
 const effectConfig=pb(...(effect===null?[]:[log('effect_source',effect),log('to_user_id',target)]),...(duplicate?[log('effect_source','star_guard')]:[]));
 const who=pb(field(1,BigInt(userId)),field(3,text(nickname)),field(24,pb(field(1,pb(field(2,14),field(6,BigInt(clubAnchor)))))));
 const common=pb(field(1,text('WebcastMemberMessage')),field(2,101),field(3,BigInt(room)),field(16,pb(field(4,BigInt(anchor)))));
 const body=pb(field(1,common),field(2,who),field(10,action),field(15,effectConfig));
 const message=pb(field(1,text('WebcastMemberMessage')),field(2,body),field(3,101));
 return pb(field(7,text('pb')),field(8,pb(field(1,message),field(4,Date.now()-age)))).buffer;
};
async function decode(options){
 const events=[],statuses=[];
 const sdk={emit:(type,kind,payload,identity)=>events.push({type,kind,payload,identity}),onDispose(){},setDecoderStatus:s=>statuses.push(s)};
 const context=vm.createContext({sdk,BOOT:{rules_preview:true,room_url:'https://live.douyin.com/123'},location:{href:'https://live.douyin.com/123'},document:{querySelectorAll:()=>[]},URL,TextDecoder,TextEncoder,Uint8Array,ArrayBuffer,Blob,DecompressionStream,setTimeout,clearTimeout});
 vm.runInContext(source+'\nglobalThis.decoderUnderTest=douyinDecoder;',context);
 context.decoderUnderTest.observe(entry(options),'wss://webcast.example.douyin.com/webcast/im/push/?room_id='+room);
 const end=Date.now()+3000;
 while(!statuses.some(s=>s.frames===1&&s.decoded+s.unsupported+s.rejected>0)&&!events.some(e=>e.payload?.stage==='historical_message_ignored')&&Date.now()<end)await new Promise(r=>setTimeout(r,5));
 assert.ok(statuses.some(s=>s.frames===1),'frame processed');
 assert.equal(events.some(e=>e.type==='probe.gap'),false);
 return events;
}
test('native star_guard effect for the entering user and current anchor marks a guardian welcome',async()=>{
 const events=await decode();const e=events.find(e=>e.type==='member.entered');
 assert.equal(e.payload.supporter_title,'star_guardian');assert.equal(e.identity.user_id,userId);assert.equal(e.payload.live_eligible,true);
});
test('ordinary members, honor effects, nickname text and other anchors never become guardian welcomes',async()=>{
 for(const options of [{effect:null,nickname:'星守护回家'}, {effect:'honor_level'}, {effect:'fansclub'}, {target:'123'}, {clubAnchor:'123'}, {duplicate:true}]){
  const events=await decode(options);const e=events.find(e=>e.type==='member.entered');
  assert.ok(e);assert.equal(e.payload.supporter_title,undefined,JSON.stringify(options));
 }
});
test('old guardian frames and non-entry member changes never emit a welcome',async()=>{
 for(const options of [{age:60000},{action:2}]){
  const events=await decode(options);assert.equal(events.some(e=>e.type==='member.entered'),false);
 }
});

const upgradeEntry=(age=0, timestamp=true)=>{
 const common=pb(field(1,text('WebcastFansclubMessage')),field(2,102),field(3,BigInt(room)),field(16,pb(field(4,BigInt(anchor)))));
 const who=pb(field(1,BigInt(userId)),field(3,text('升级观众')),field(24,pb(field(1,pb(field(2,31),field(6,BigInt(anchor)))))));
 const body=pb(field(1,common),field(2,1),field(4,who));
 const message=pb(field(1,text('WebcastFansclubMessage')),field(2,body),field(3,102));
 return pb(field(7,text('pb')),field(8,pb(field(1,message),...(timestamp?[field(4,Date.now()-age)]:[])))).buffer;
};
test('fansclub upgrades without create_time use response time and reject stale frames',async()=>{
 for(const [age,timestamp,expected] of [[0,true,true],[60000,true,false],[0,false,false]]){
  const events=[],statuses=[];
  const sdk={emit:(type,kind,payload,identity)=>events.push({type,kind,payload,identity}),onDispose(){},setDecoderStatus:s=>statuses.push(s)};
  const context=vm.createContext({sdk,BOOT:{rules_preview:true,room_url:'https://live.douyin.com/123'},location:{href:'https://live.douyin.com/123'},document:{querySelectorAll:()=>[]},URL,TextDecoder,TextEncoder,Uint8Array,ArrayBuffer,Blob,DecompressionStream,setTimeout,clearTimeout});
  vm.runInContext(source+'\nglobalThis.decoderUnderTest=douyinDecoder;',context);
  context.decoderUnderTest.observe(upgradeEntry(age,timestamp),'wss://webcast.example.douyin.com/webcast/im/push/?room_id='+room);
  await new Promise(r=>setTimeout(r,20));
  const upgrade=events.find(e=>e.type==='level.upgraded' && e.payload.live_eligible);
  assert.equal(!!upgrade,expected);
  if(upgrade)assert.equal(upgrade.payload.new_level,31);
  assert.equal(events.some(e=>e.type==='probe.gap'),false);
 }
});

const userUpgradeEntry=(options={})=>{
 const {age=0,level='31',tag='level_up_msg',pattern='恭喜{0:user}刚刚升级至Lv.{1:string}',user=userId}=options;
 const who=pb(field(1,BigInt(user)),field(3,text('升级观众')));
 const display=pb(field(2,text(pattern)),field(4,pb(field(1,11),field(21,pb(field(1,who))))),field(4,pb(field(1,1),field(11,text(level)))));
 const common=pb(field(1,text('WebcastRoomMessage')),field(2,103),field(3,BigInt(room)),field(8,display),field(16,pb(field(4,BigInt(anchor)))));
 const body=pb(field(1,common),field(20,text(tag)));
 const message=pb(field(1,text('WebcastRoomMessage')),field(2,body),field(3,103));
 return pb(field(7,text('pb')),field(8,pb(field(1,message),field(4,Date.now()-age)))).buffer;
};
test('captured room level notice decodes user wealth upgrade and rejects unrelated notices',async()=>{
 for(const [options,expected] of [[{},true],[{age:60000},false],[{level:'abc'},false],[{tag:'other'},false],[{pattern:'聊天 {0:user} 刚刚升级至Lv.{1:string}'},false]]){
  const events=[];const sdk={emit:(type,kind,payload,identity)=>events.push({type,kind,payload,identity}),onDispose(){},setDecoderStatus(){}};
  const context=vm.createContext({sdk,BOOT:{rules_preview:true,room_url:'https://live.douyin.com/123'},location:{href:'https://live.douyin.com/123'},document:{querySelectorAll:()=>[]},URL,TextDecoder,TextEncoder,Uint8Array,ArrayBuffer,Blob,DecompressionStream,setTimeout,clearTimeout});
  vm.runInContext(source+'\nglobalThis.decoderUnderTest=douyinDecoder;',context);
  context.decoderUnderTest.observe(userUpgradeEntry(options),'wss://webcast.example.douyin.com/webcast/im/push/?room_id='+room);
  await new Promise(r=>setTimeout(r,20));
  const e=events.find(e=>e.type==='level.upgraded');assert.equal(!!e,expected,JSON.stringify(options));
  if(e){assert.equal(e.payload.level_kind,'wealth');assert.equal(e.payload.new_level,31);assert.equal(e.identity.user_id,userId);assert.equal(e.payload.live_eligible,true);}
  assert.equal(events.some(e=>e.type==='probe.gap'),false);
 }
});
