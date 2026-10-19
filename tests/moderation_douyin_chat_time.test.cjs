const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const integer=n=>{n=BigInt(n);const out=[];while(n>127n){out.push(Number(n&127n)|128);n>>=7n;}out.push(Number(n));return out;};
const field=(k,v)=>typeof v==='number'||typeof v==='bigint'?[...integer(k*8),...integer(v)]:[...integer(k*8+2),...integer(v.length),...v];
const pb=(...values)=>new Uint8Array(values.flat()),txt=s=>new TextEncoder().encode(s);
function fixture({commonTime=0,eventTime=0,room=789}={}){
 // Minimal native shape: Common fields 1/2/3, ChatMessage.event_time=15.
 const common=pb(field(1,txt('WebcastChatMessage')),field(2,7698237646736856074n),field(3,room),...(commonTime?[field(4,commonTime)]:[]));
 const body=pb(field(1,common),field(2,pb(field(1,12345),field(3,txt('观众')))),field(3,txt('手臂有胎记吗')),...(eventTime?[field(15,eventTime)]:[]));
 const message=pb(field(1,txt('WebcastChatMessage')),field(2,body),field(3,7698237666408353586n));
 return pb(field(6,txt('pb')),field(7,txt('msg')),field(8,pb(field(1,message))));
}
async function decode(options){
 const events=[],now=1792385632605,context={Uint8Array,ArrayBuffer,TextDecoder,URL,Date:class extends Date{static now(){return now;}},BOOT:{rules_preview:true},document:{querySelectorAll:()=>[]},sdk:{setDecoderStatus(){},onDispose(){},emit:(type,source,payload,identity)=>events.push({type,source,payload,identity})}};
 vm.createContext(context);vm.runInContext(fs.readFileSync('assets/moderation/adapters/douyin-decoder.js','utf8')+'\nglobalThis.decoder=douyinDecoder;',context);
 context.decoder.observe(fixture(options).buffer,'wss://webcast.example.douyin.com/webcast/im/push?room_id=789');
 await new Promise(r=>setImmediate(r));return events;
}
test('native chat event_time makes timestamp-less Common comments eligible for keyword and timed rules',async()=>{
 for(const eventTime of [1792385631,1792385631000]){
  const event=(await decode({eventTime})).find(e=>e.type==='chat.received');
  assert.equal(event.payload.live_eligible,true);assert.equal(event.payload.observed_at_ms,1792385632605);
  assert.equal(event.payload.content,'手臂有胎记吗');assert.equal(event.source,'network');assert.equal(event.identity.native_event_id,'7698237646736856074');
 }
});
test('historical and future native chat timestamps are rejected',async()=>{
 for(const eventTime of [1792385531,1792385731]){
  const events=await decode({eventTime});assert.ok(!events.some(e=>e.type==='chat.received'));
  assert.ok(events.some(e=>e.payload.stage==='historical_message_ignored'));
 }
});
test('missing time stays ineligible and valid Common time remains supported',async()=>{
 assert.equal((await decode({})).find(e=>e.type==='chat.received').payload.live_eligible,false);
 assert.equal((await decode({commonTime:1792385631})).find(e=>e.type==='chat.received').payload.live_eligible,true);
 assert.ok(!(await decode({commonTime:1792385531,eventTime:1792385631})).some(e=>e.type==='chat.received'),'fallback must not override an explicitly stale Common');
});
test('event_time does not bypass room identity validation',async()=>{
 const events=await decode({eventTime:1792385631,room:456});assert.ok(!events.some(e=>e.type==='chat.received'));
 assert.ok(events.some(e=>e.type==='probe.gap'&&e.payload.reason==='message_room_mismatch'));
});
