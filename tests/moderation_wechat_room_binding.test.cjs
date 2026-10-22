const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const source=fs.readFileSync('assets/moderation/adapters/wechat-desktop.js','utf8');
const helper=source.slice(source.indexOf('const createWechatRoomBinding='),source.indexOf('const wxReadLiveIdentity='));
function fixture(initial){let identity=initial,stops=0;const c={read:()=>identity,stop:()=>stops++};vm.createContext(c);vm.runInContext(helper+';globalThis.binding=createWechatRoomBinding(read,stop);',c);return {binding:c.binding,set:v=>identity=v,stops:()=>stops};}
test('automatic binding tolerates initial join before profile and keeps same-room reconnects',()=>{
 const f=fixture(null);f.binding.join();assert.equal(f.stops(),0);f.set('anchor-a');f.binding.profile();f.binding.join();assert.equal(f.stops(),0);f.binding.join();assert.equal(f.stops(),0);f.set('anchor-b');f.binding.profile();assert.equal(f.stops(),1);
});
test('profile arriving before first join is initialization, while a real switch stops the binding',()=>{
 const f=fixture(null);f.set('anchor-a');f.binding.profile();f.binding.join();assert.equal(f.stops(),0);f.set('anchor-b');f.binding.join();assert.equal(f.stops(),1);
});
test('a page already bound to a known room rejects another room, and uninspectable legacy pages stop on join',()=>{
 const f=fixture('anchor-a');f.set('anchor-b');f.binding.join();assert.equal(f.stops(),1);const legacy=fixture(undefined);legacy.binding.join();assert.equal(legacy.stops(),1);
});

test('automatic single-task binding also tolerates the native helper loading after bootstrap',()=>{
 let identity,stops=0;const c={read:()=>identity,stop:()=>stops++};vm.createContext(c);vm.runInContext(helper+';globalThis.b=createWechatRoomBinding(read,stop,true);',c);c.b.join();assert.equal(stops,0);identity='anchor-a';c.b.profile();c.b.join();assert.equal(stops,0);identity='anchor-b';c.b.join();assert.equal(stops,1);
});
