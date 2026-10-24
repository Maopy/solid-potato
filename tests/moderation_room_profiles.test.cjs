const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const source=fs.readFileSync('assets/moderation/proxy/controller.py','utf8');
const helper=source.slice(source.indexOf('const load=choice=>'),source.indexOf('if(choices.length===1)'));
function fixture(){
 const scripts=[],timers=[],window={};
 const context={window,nonce:'fixture',setTimeout:fn=>timers.push(fn),encodeURIComponent,
   document:{createElement:()=>({}),head:{appendChild:s=>scripts.push(s)}}};
 vm.createContext(context);vm.runInContext(helper+';globalThis.loadRoom=load;',context);
 const choice={prefix:'/local/',host:'channels.weixin.qq.com',room_profiles:true};
 return {scripts,timers,window,load:()=>context.loadRoom(choice)};
}
test('WeChat profile bootstrap waits for native room identity and passes its exact encoded ID',()=>{
 const f=fixture();f.load();assert.equal(f.scripts.length,0);
 f.window.WXU={check_live_existing:()=>[null,{contact:{username:'v2_anchor@finder'}}]};
 f.timers.shift()();assert.equal(f.scripts.length,1);
 assert.equal(f.scripts[0].src,'/local/bootstrap.js?room_identity=v2_anchor%40finder');
 assert.equal(f.scripts[0].nonce,'fixture');
});
test('unavailable native room identity eventually falls back without a task picker',()=>{
 const f=fixture();f.load();for(let i=0;i<51&&f.timers.length;i++)f.timers.shift()();
 assert.equal(f.scripts.length,1);assert.equal(f.scripts[0].src,'/local/bootstrap.js');
});
test('room titles and invalid native identity cannot select a room profile',()=>{
 const f=fixture();f.window.WXU={check_live_existing:()=>[null,{nickname:'主播',id:'guessed-id',contact:{username:'bad\nidentity'}}]};
 f.load();for(let i=0;i<51&&f.timers.length;i++)f.timers.shift()();
 assert.equal(f.scripts[0].src,'/local/bootstrap.js');
});
