// Complete SDK -> typed poll command -> DOM executor -> receipt in a VM page.
// Every platform request, click, key event and submission is forbidden.
const test = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const source = ['sdk/probe.js', 'sdk/submit.js', 'adapters/douyin-fill.js'].map(p => fs.readFileSync(path.join(root, 'assets/moderation', p), 'utf8')).join('\n');
const delay = ms => new Promise(r => setTimeout(r, ms));

function environment(options = {}) {
  const requests = [], receipts = [], timers = new Map(), listeners = new Map();
  let writes = 0, clicks = 0, writtenText = null, clock = 0, resolvePoll, caretMarker = '', inputs=0;
  const sent=[],written=[];let restoredSelection=null;
  class Textarea {
    constructor() { this._value = options.text || ''; }
    get value() { return this._value; }
    set value(text) { this._value = text; writes++; }
  }
  const leaf = { _text:options.text || '\u200b', get textContent() {return this._text;}, set textContent(text) {this._text=text; writtenText=text; written.push(text); writes++;}, removeAttribute(name) {assert.equal(name,'data-enter');} };
  const node = options.rich ? { tagName:'DIV', isContentEditable:true, get textContent() {return leaf.textContent + caretMarker;} } : new Textarea();
  if (!options.rich) node.tagName = 'TEXTAREA';
  Object.assign(node, {isConnected:true, disabled:!!options.disabled, readOnly:!!options.readOnly,
    getClientRects:()=>[{}], getAttribute:()=>null, contains:()=>false, matches:s=>!!options.rich&&s.includes('data-slate-editor'), closest:()=>null,
    querySelector:()=>options.richDraft?{}:null, querySelectorAll:()=>options.rich ? [leaf, {textContent:'\u200b',removeAttribute(){}}] : [leaf],
    dispatchEvent(event) {assert.equal(event.type,'input'); assert.equal(event.bubbles,true); if (options.rich) { if(options.platformLimit) leaf._text=leaf._text.slice(0,options.platformLimit); caretMarker='\u200b'; } if(++inputs===1&&options.interrupt){listeners.get('beforeinput')({type:'beforeinput',isTrusted:true,target:node});if(options.rich)leaf._text+='续';else node._value+='续';} return true;},
    click() {throw Error('forbidden click');}, focus() {throw Error('forbidden focus');},
    submit() {throw Error('forbidden submit');}, requestSubmit() {throw Error('forbidden submit');} });
  const document = { hidden:!!options.hidden, activeElement:options.focused ? node : null,
    querySelectorAll:()=>options.absent ? [] : options.ambiguous ? [node,node] : [node],
    addEventListener:(name,fn)=>listeners.set(name,fn), removeEventListener:name=>listeners.delete(name) };
  node.ownerDocument = document;
  const button={isConnected:true,ownerDocument:document,tagName:'BUTTON',textContent:'发送',getClientRects:()=>[{}],closest:()=>null,matches:()=>false,disabled:!!options.sendDisabled,click(){clicks++;sent.push(String(options.rich?leaf.textContent:node.value).replace(/[\u200b\ufeff]/g,''));if(options.clearOnSend){if(options.rich)leaf._text='';else node._value='';}if(options.delayedClear)setTimeout(()=>{if(options.rich)leaf._text='';else node._value='';},options.delayedClear);}};
  const container={querySelectorAll:s=>s.startsWith('svg.')?[]:[button]};
  if(options.automatic)node.closest=s=>s.startsWith('div[class*="webcast-chatroom___input-container"]')?container:null;
  const command = {v:1, command:'fill_input', action_id:'action-1', connection_id:'conn', binding_id:'binding', document_id:'doc',
    page_url:'https://live.douyin.com/139244611014', remaining_ttl_ms:3000, text:'场控填入测试（不会发送）', ...options.command};
  const pollResponse = () => {
    clock += options.elapsed || 0;
    return {mode:options.automatic?'fill_and_send':'fill_only',send_chat:!!options.automatic,commands:[command],rules:{mode:'online_scheduler_preview',paused:!!options.paused},previews:[]};
  };
  const browser = { HTMLTextAreaElement:Textarea, addEventListener() {}, removeEventListener() {},
    getComputedStyle:()=>({visibility:options.invisible ? 'hidden' : 'visible',display:'block',opacity:'1'}),
    fetch:async (url, init) => {
      assert.ok(String(url).startsWith('/__live_archive/moderation/v1/'), 'no platform HTTP');
      const endpoint = String(url).split('/').at(-1), body = JSON.parse(init.body);
      requests.push(endpoint);
      let value;
      if (endpoint === 'hello') value = {connection_id:'conn',binding_id:'binding',token:'token'};
      else if (endpoint === 'events') value = {acked_seq:body.events.at(-1).seq};
      else if (endpoint === 'poll') value = options.holdPoll ? await new Promise(resolve=>{resolvePoll=()=>resolve(pollResponse());}) : pollResponse();
      else if (endpoint === 'results') {receipts.push(body); value={acked_action_id:body.action_id};}
      else if (endpoint === 'goodbye') value = {stopped:true};
      else throw Error('unexpected endpoint '+endpoint);
      return {ok:true,json:async()=>value};
    } };
  if(options.selection){
    const textNode={get textContent(){return leaf.textContent;}};
    node.contains=n=>n===node||n===textNode;
    document.createRange=()=>{let end=0;return {selectNodeContents(){},setEnd(target,offset){end=offset;},toString(){return leaf.textContent.slice(0,end);}};};
    document.createTreeWalker=()=>{let done=false;return {nextNode(){if(done)return null;done=true;return textNode;}};};
    browser.getSelection=()=>({anchorNode:textNode,focusNode:textNode,anchorOffset:2,focusOffset:5,setBaseAndExtent(a,ao,f,fo){restoredSelection=[ao,fo];}});
  }
  browser.self = browser; browser.top = options.iframe ? {} : browser;
  const location = {origin:'https://live.douyin.com',pathname:'/139244611014'};
  const context = vm.createContext({window:browser, document, location, performance:{now:()=>clock},
    crypto:{randomUUID:()=> 'doc'}, AbortController, Event,
    setTimeout:(fn,ms)=> {if (ms === 500 || ms === 2000 && fn.name === 'tick') {const id={}; timers.set(id,fn); return id;} return setTimeout(fn,ms);},
    clearTimeout:id=>{if (!timers.delete(id)) clearTimeout(id);} });
  vm.runInContext(`(()=>{const BOOT={room_url:'https://live.douyin.com/139244611014',prefix:'/__live_archive/moderation/v1/',ticket:'ticket',fill_preview:true,automatic_send:${!!options.automatic}};${source};window.__testExecutor=fillExecutor;})() ;`, context);
  if (options.composing) listeners.get('compositionstart')();
  return {browser, node, leaf, requests, receipts, listeners, location,command,sent,written,selection:()=>restoredSelection,setClock:value=>clock=value,writes:()=>writes, clicks:()=>clicks, writtenText:()=>writtenText,
    release:()=>resolvePoll?.(), async ready(){for(let i=0;i<240&&!receipts.length;i++)await delay(5);}, async cycle() {const entry=timers.entries().next().value; assert.ok(entry); timers.delete(entry[0]); await entry[1]();},
    dispose:()=>browser.__liveArchiveModerationV1.dispose()};
}

test('textarea fills once, duplicate commands return cached receipt without rewriting', async () => {
  const env = environment();
  try {
    await env.ready();
    assert.equal(env.node.value,'场控填入测试（不会发送）');
    assert.equal(env.receipts[0].outcome,'filled');
    assert.equal(env.browser.__liveArchiveModerationV1.getStatus().send_chat,false);
    await env.cycle();
    assert.equal(env.writes(),1);
    assert.deepEqual(env.receipts[1],env.receipts[0]);
  } finally {env.dispose();}
});

test('observed Slate structure receives plain text without replacing the editor', async () => {
  const env = environment({rich:true});
  try {await env.ready(); assert.equal(env.leaf.textContent,'场控填入测试（不会发送）'); assert.equal(env.receipts[0].reason,'input_filled'); assert.equal(env.writes(),1);}
  finally {env.dispose();}
});

test('all rule kinds allow only the exact platform truncation and click once without altering the original draft',async()=>{
 const original='相逢是缘✨相识是份💫点赞是情💥占榜是义🌟祝愿直播间的所有朋友们💫福禄寿喜全都有☀️幸福健康到永久💥✨💫🔥';
 for(const action_id of ['timed-long','keyword-long','gift-long']){
  const env=environment({rich:true,automatic:true,platformLimit:50,command:{command:'fill_and_submit',action_id,text:original}});
  try{
   await env.ready();assert.equal(env.receipts[0]?.reason,'input_submitted');assert.equal(env.clicks(),1);
   assert.equal(env.written[0],original,'write the complete original; only the platform truncates');
   assert.deepEqual(env.sent,[original.slice(0,50)]);assert.equal(env.leaf.textContent,original.slice(0,50));
   await env.cycle();assert.equal(env.clicks(),1,'duplicate command never resubmits');assert.equal(env.writes(),1);
  }finally{env.dispose();}
 }
});

test('busy, composing, disabled, hidden, ambiguous and iframe inputs never receive writes', async () => {
  const cases = [ [{text:'用户正在输入'},'input_busy'], [{focused:true},'input_busy'], [{composing:true},'input_busy'],
    [{disabled:true},'input_disabled'], [{readOnly:true},'input_disabled'], [{invisible:true},'input_hidden'],
    [{hidden:true},'page_hidden'], [{absent:true},'input_not_found'], [{ambiguous:true},'input_not_unique'], [{iframe:true},'not_top_frame'] ];
  for (const [options, reason] of cases) {
    const env = environment(options);
    try {await delay(10); assert.equal(env.writes(),0,reason); assert.equal(env.receipts[0]?.reason,reason);}
    finally {env.dispose();}
  }
});

test('binding, TTL, paused observations and invalid text reject before mutation', async () => {
  const cases = [ [{command:{binding_id:'old-binding'}},'binding_mismatch'], [{command:{document_id:'old-document'}},'binding_mismatch'],
    [{elapsed:3000},'ttl_expired'], [{paused:true},'observation_paused'], [{command:{text:'含有\n换行'}},'invalid_command'],
    [{command:{text:'字'.repeat(81)}},'text_too_long'], [{command:{command:'send_chat'}},'binding_mismatch'] ];
  for (const [options, reason] of cases) {
    const env = environment(options);
    try {await delay(10); assert.equal(env.writes(),0,reason); assert.equal(env.receipts[0]?.reason,reason);}
    finally {env.dispose();}
  }
});

test('stop and room navigation while poll is pending prevent late input writes', async () => {
  for (const stop of [true,false]) {
    const env = environment({holdPoll:true});
    try {
      await delay(10);
      if (stop) env.dispose(); else env.location.pathname='/another-room';
      env.release(); await delay(10);
      assert.equal(env.writes(),0); assert.equal(env.receipts.length,0);
      assert.equal(env.browser.__liveArchiveModerationV1.getStatus().stopped,true);
      assert.equal(env.listeners.size,0);
    } finally {env.dispose();}
  }
});


test('unexpected editor shortening still blocks submission',async()=>{
 const env=environment({rich:true,automatic:true,platformLimit:49,command:{command:'fill_and_submit',text:'字'.repeat(60)}});
 try{await env.ready();assert.equal(env.receipts[0]?.reason,'input_changed');assert.equal(env.clicks(),0);}
 finally{env.dispose();}
});

test('user drafts are never cached, overwritten or restored, whether focused or not',async()=>{
 for(const rich of [false,true])for(const focused of [false,true]){
  const draft='用户写了一半👩‍💻';const env=environment({rich,focused,automatic:true,text:draft,selection:rich,command:{command:'fill_and_submit'}});
  try{await env.ready();assert.equal(env.receipts[0]?.reason,'input_busy');assert.equal(env.writes(),0);assert.equal(env.clicks(),0);assert.equal(rich?env.leaf.textContent:env.node.value,draft);assert.equal(env.selection(),null);}
  finally{env.dispose();}
 }
});
test('pre-click rejection clears only this command from an originally empty input',async()=>{
 const env=environment({rich:true,automatic:true,sendDisabled:true,command:{command:'fill_and_submit'}});
 try{await env.ready();assert.equal(env.receipts[0]?.reason,'send_disabled');assert.equal(env.leaf.textContent.replace(/\u200b/g,''),'');assert.equal(env.clicks(),0);}finally{env.dispose();}
});
test('real typing between fill and click cancels submission and keeps only the new user text',async()=>{
 const env=environment({rich:true,automatic:true,focused:true,interrupt:true,command:{command:'fill_and_submit'}});
 try{await env.ready();assert.equal(env.receipts[0]?.reason,'input_changed');assert.equal(env.leaf.textContent,'续');assert.equal(env.clicks(),0);}finally{env.dispose();}
});
test('composition and embedded objects remain protected',async()=>{
 for(const extra of [{composing:true},{richDraft:true}]){
  const env=environment({rich:true,automatic:true,...extra,command:{command:'fill_and_submit'}});
  try{await env.ready();assert.equal(env.receipts[0]?.reason,'input_busy');assert.equal(env.writes(),0);}finally{env.dispose();}
 }
});
test('focused empty automatic input is usable and queued commands send one at a time',async()=>{
 const env=environment({rich:true,automatic:true,focused:true,clearOnSend:true,command:{command:'fill_and_submit'}});
 try{
  await env.ready();assert.equal(env.receipts[0]?.reason,'input_submitted');
  const executor=env.browser.__testExecutor;
  const results=await Promise.all([executor.execute({...env.command,text:'第二条'},0),executor.execute({...env.command,text:'第三条'},0)]);
  assert.deepEqual(results.map(r=>r.outcome),['submitted','submitted']);assert.deepEqual(env.sent,['场控填入测试（不会发送）','第二条','第三条']);assert.equal(env.leaf.textContent.replace(/\u200b/g,''),'');
 }finally{env.dispose();}
});
test('delayed platform clear never becomes a draft or gets overwritten by the next command',async()=>{
 const env=environment({rich:true,automatic:true,delayedClear:100,command:{command:'fill_and_submit'}});
 try{
  await env.ready();const executor=env.browser.__testExecutor;
  const result=await executor.execute({...env.command,text:'下一条'},0);assert.equal(result.reason,'input_busy');assert.equal(env.clicks(),1);
  await delay(140);assert.equal(env.leaf.textContent.replace(/\u200b/g,''),'');
  const next=await executor.execute({...env.command,text:'下一条'},0);assert.equal(next.outcome,'submitted');assert.equal(env.clicks(),2);
 }finally{await delay(140);env.dispose();}
});
test('queue waiting consumes TTL and a cancelled command does not poison the next transaction',async()=>{
 const env=environment({rich:true,automatic:true,clearOnSend:true,command:{command:'fill_and_submit'}});
 try{
  await env.ready();const executor=env.browser.__testExecutor;
  const first=executor.execute({...env.command,text:'临时文案'},0),second=executor.execute({...env.command,text:'已过期文案'},0);
  await Promise.resolve();env.setClock(3000);const results=await Promise.all([first,second]);
  assert.equal(results[1].reason,'ttl_expired');assert.equal(env.leaf.textContent.replace(/\u200b/g,''),'');
  const next=await executor.execute({...env.command,text:'新文案'},0);assert.equal(next.outcome,'submitted');assert.deepEqual(env.sent,['场控填入测试（不会发送）','新文案']);
 }finally{env.dispose();}
});
