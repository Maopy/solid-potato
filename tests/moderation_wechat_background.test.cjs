// Simulate the page controls locally; no platform calls or real messages.
const test = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const source = ['assets/moderation/sdk/submit.js', 'assets/moderation/adapters/wechat-fill.js'].map(p => fs.readFileSync(p, 'utf8')).join('\n');

function page({identity='host-a', room='host-a', hidden=true, text='', invisible=false, switchDuringInput=false, collapsed=false, manual=false}={}) {
  let current = identity, clicks = 0, writes = 0, open = !collapsed;
  const document = {hidden, body:{}, documentElement:{}, addEventListener(){}, removeEventListener(){}};
  class Textarea { constructor(){this._value=text;} get value(){return this._value;} set value(value){this._value=value;writes++;} }
  const button = {tagName:'BUTTON',textContent:'评论',isConnected:true,ownerDocument:document,disabled:false,getClientRects:()=>[{}],closest:()=>null,matches:()=>false,click(){clicks++;}};
  const editor = new Textarea();
  Object.assign(editor,{tagName:'TEXTAREA',isConnected:true,ownerDocument:document,disabled:false,readOnly:false,getClientRects:()=>[{}],closest:()=>null,matches:()=>false,querySelector:()=>null,parentElement:{querySelectorAll:()=>[button]},dispatchEvent(){if(switchDuringInput)current='other-host';}});
  const opener={tagName:'DIV',textContent:'聊一聊',isConnected:true,ownerDocument:document,getClientRects:()=>[{}],getBoundingClientRect:()=>({width:116,height:32}),closest:()=>null,querySelectorAll:()=>[],click(){open=true;}};
  document.querySelectorAll=selector=>selector.startsWith('textarea,')?(open?[editor]:[]):[opener];
  const window={HTMLTextAreaElement:Textarea,getComputedStyle:()=>({visibility:invisible?'hidden':'visible',display:'block',opacity:'1',borderRadius:'20px',backgroundColor:'rgb(210,210,210)'})};
  window.top=window.self=window;
  const context=vm.createContext({window,document,Event,setTimeout,performance,BOOT:{automatic_send:!manual,manual_send:manual,fill_preview:true,room_identity:room},sdk:{onDispose(){}},stopped:false,draining:false,moderationEnabled:!manual,wxCollecting:!manual,sameRoom:()=>true,wxReadLiveIdentity:()=>current});
  vm.runInContext(`let fillExecutor; ${source}; globalThis.executor=fillExecutor;`,context);
  return {executor:context.executor,editor,clicks:()=>clicks,writes:()=>writes};
}
const command={command:'fill_and_submit',text:'测试文案',remaining_ttl_ms:3000};

test('hidden WeChat room sends once when its identity and controls remain verified',async()=>{
  const p=page();
  assert.equal(p.executor.getStatus().input_ready,true);
  assert.equal((await p.executor.execute(command,0)).reason,'input_submitted');
  assert.equal(p.writes(),1); assert.equal(p.clicks(),1);
});
test('hidden WeChat room can open its collapsed comment editor',async()=>{
  const p=page({collapsed:true});
  assert.equal(p.executor.getStatus().input_ready,false);
  await p.executor.prepareInput(1);
  assert.equal(p.executor.getStatus().input_ready,true);
  assert.equal((await p.executor.execute(command,0)).reason,'input_submitted');
  assert.equal(p.clicks(),1);
});
test('manual sending works while moderation and collection are off',async()=>{
  const p=page({manual:true,collapsed:true});
  await p.executor.prepareInput(1,true);
  assert.equal(p.executor.getStatus(true).input_ready,true);
  assert.equal((await p.executor.execute({...command,action_id:'manual-test'},0)).reason,'input_submitted');
  assert.equal(p.clicks(),1);
});
test('hidden room without matching identity, or an unsafe editor, is not used',async()=>{
  for(const options of [{identity:'host-b'},{identity:null},{room:''},{text:'用户草稿'},{invisible:true}]) {
    const p=page(options);
    assert.equal(p.executor.getStatus().input_ready,false);
    assert.equal((await p.executor.execute(command,0)).outcome,'rejected');
    assert.equal(p.writes(),0); assert.equal(p.clicks(),0);
  }
});
test('a room identity change during filling prevents submission',async()=>{
  const p=page({switchDuringInput:true});
  assert.equal((await p.executor.execute(command,0)).outcome,'rejected');
  assert.equal(p.clicks(),0);
});
