const test=require('node:test'),assert=require('node:assert/strict'),vm=require('node:vm'),fs=require('node:fs');
const source=fs.readFileSync('assets/moderation/adapters/wechat-pk-diagnostics.js','utf8');
function fixture({enabled=true,collecting=true}={}) {
  const events=[],listeners=new Map();let live=collecting;
  const window={addEventListener:(n,f)=>listeners.set(n,f),removeEventListener:n=>listeners.delete(n)};
  const sdk={captureRaw:(...args)=>events.push(args)};
  const c=vm.createContext({window,location:{origin:'https://channels.weixin.qq.com'},BOOT:{raw_messages:enabled},sdk,collecting:()=>live});
  vm.runInContext(source+';globalThis.probe=createWechatPkDiagnostics(sdk,collecting)',c);
  return {probe:c.probe,events,listeners,setCollecting:v=>live=v};
}
test('captures bounded PK events without altering emitter calls or other events',()=>{
  const f=fixture(),calls=[];
  const bus={emit(...args){calls.push([this,...args]);return 17;}};
  const original=bus.emit;f.probe.attach(bus);
  assert.equal(bus.emit('chat',{text:'private'}),17);assert.equal(f.events.length,0);
  assert.equal(bus.emit('channels:OnLiveBattleScore',{score:31}),17);
  assert.equal(calls[1][0],bus);assert.equal(calls[1][1],'channels:OnLiveBattleScore');
  assert.equal(f.events[0][0],'pk_diagnostic');assert.equal(f.events[0][1],'event_bus');
  f.setCollecting(false);bus.emit('OnPkUpdate',{score:32});assert.equal(f.events.length,1);
  f.setCollecting(true);for(let i=0;i<400;i++)bus.emit('OnPkUpdate',{score:i});assert.equal(f.events.length,300);
  f.probe.dispose();assert.equal(bus.emit,original);assert.equal(f.listeners.has('message'),false);
});
test('only same-origin PK-shaped window messages are recorded',()=>{
  const f=fixture();const handler=f.listeners.get('message');
  handler({origin:'https://untrusted.example',data:{type:'pkScore',score:2}});
  handler({origin:'https://channels.weixin.qq.com',data:{type:'chat',text:'hello'}});
  assert.equal(f.events.length,0);
  handler({origin:'https://channels.weixin.qq.com',data:{type:'pkScore',score:2}});
  assert.equal(f.events.length,1);assert.equal(f.events[0][1],'window_message');
  f.probe.dispose();
});
test('disabled capture and immutable emitters remain unchanged',()=>{
  const f=fixture({enabled:false}),bus={};
  const emit=()=>42;Object.defineProperty(bus,'emit',{value:emit,configurable:false,writable:false});
  f.probe.attach(bus);assert.equal(bus.emit,emit);assert.equal(bus.emit('pk'),42);
  f.listeners.get('message')({origin:'https://channels.weixin.qq.com',data:{type:'pkScore'}});
  assert.equal(f.events.length,0);f.probe.dispose();
});
