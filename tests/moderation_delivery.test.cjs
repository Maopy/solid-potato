// Sending records a click attempt; it does not wait for or request confirmation.
const test=require('node:test'),assert=require('node:assert/strict'),vm=require('node:vm'),fs=require('node:fs');
const source=fs.readFileSync('assets/moderation/sdk/submit.js','utf8');
function env(throws=false){let clicks=0,waits=0;
 const document={hidden:false,body:{},documentElement:{}};
 const button={isConnected:true,ownerDocument:document,disabled:false,getClientRects:()=>[{}],closest:()=>null,matches:()=>false,click:()=>{clicks++;if(throws)throw Error('handler failed');}};
 const node={isConnected:true};
 const ctx={inputSafe:()=>true,active:()=>true,sameRoom:()=>true,paused:()=>false};
 const c=vm.createContext({BOOT:{automatic_send:true},document,window:{getComputedStyle:()=>({visibility:'visible',display:'block',opacity:'1'})},performance:{now:()=>0},setTimeout:()=>{waits++;throw Error('confirmation wait forbidden');}});
 vm.runInContext(source,c);c.node=node;c.ctx=ctx;c.buttons=()=>[button];
 return {send:()=>vm.runInContext("submitFilledInput(ctx,node,'draft',3000,()=> 'draft',buttons)",c),get clicks(){return clicks;},get waits(){return waits;},c};
}
test('a click succeeds immediately even when the input remains unchanged',async()=>{const e=env();assert.equal((await e.send()).outcome,'submitted');assert.equal(e.clicks,1);assert.equal(e.waits,0);assert.equal(e.c.sdk,undefined);});
test('a throwing click records unknown and does not halt the next command',async()=>{const e=env(true);assert.equal((await e.send()).outcome,'unknown');assert.equal((await e.send()).outcome,'unknown');assert.equal(e.clicks,2);assert.equal(e.waits,0);});
test('pre-click page checks still prevent mutations',async()=>{const e=env();e.c.ctx.active=()=>false;assert.equal((await e.send()).outcome,'rejected');assert.equal(e.clicks,0);});

test('SVG send controls dispatch one bubbling click without HTMLElement.click',async()=>{
 const e=env();let clicks=0;
 e.c.window.MouseEvent=class{constructor(type,options){this.type=type;Object.assign(this,options);}};
 const svg={namespaceURI:'http://www.w3.org/2000/svg',tagName:'svg',isConnected:true,ownerDocument:e.c.document,getClientRects:()=>[{}],closest:()=>null,matches:()=>false,dispatchEvent(event){assert.equal(event.type,'click');assert.equal(event.bubbles,true);assert.equal(event.cancelable,true);clicks++;return true;}};
 e.c.buttons=()=>[svg];assert.equal((await e.send()).outcome,'submitted');assert.equal(clicks,1);assert.equal(e.waits,0);
 svg.dispatchEvent=()=>{clicks++;throw Error('uncertain handler');};assert.equal((await e.send()).outcome,'unknown');assert.equal(clicks,2,'never retries an uncertain SVG click');
});
