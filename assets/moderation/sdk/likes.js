// Page-side opt-in scheduling. Never tied to moderation or recording switches.
const createLikeScheduler=(config,io)=>{
  let enabled=false,nextMark=null,remaining=0,queued=false,batchAttempts=0,total=0,batches=0,busy=false,lastAction=-Infinity,lastNow=0,reason='disabled',epoch=0,initialClaim=false;
  const claimed=new Set();
  const nextAfter=now=>{const d=new Date(now);d.setMinutes((Math.floor(d.getMinutes()/config.every_minutes)+1)*config.every_minutes,0,0);return d.getTime();};
  const begin=()=>{remaining=config.batch_count;batchAttempts=0;batches++;};
  const release=()=>io.release?.();
  const setEnabled=value=>{release();epoch++;enabled=!!value;remaining=0;queued=false;lastAction=-Infinity;lastNow=io.now();initialClaim=enabled;nextMark=enabled&&config.mode==='clock'?(io.claim?lastNow:nextAfter(lastNow)):null;reason=enabled?null:'disabled';};
  const step=async()=>{
    if(!enabled||busy)return;
    if(!io.inRoom()){setEnabled(false);reason='room_changed';return;}
    const now=io.now();
    if(now<lastNow-1000){release();remaining=0;queued=false;nextMark=config.mode==='clock'?(io.claim?now:nextAfter(now)):null;lastAction=-Infinity;}lastNow=now;
    if(!io.visible()){release();reason='page_hidden';return;}
    if(config.mode==='clock'){
      if(now>=nextMark){
        if(io.claim){
          if(!initialClaim&&config.missed==='skip'&&now-nextMark>3000){nextMark=nextAfter(now);reason='waiting_clock';return;}
          const generation=epoch;busy=true;
          try{
            const result=await io.claim();
            if(!enabled||epoch!==generation)return;
            if(typeof result.claimed!=='boolean'||!Number.isFinite(result.period_end_ms))throw Error('invalid period response');
            initialClaim=false;nextMark=Math.max(result.period_end_ms,now+1000);
            if(result.claimed){if(!remaining)begin();else if(batchAttempts>0)queued=true;}
          }catch(_){if(enabled&&epoch===generation){reason='backend_unavailable';nextMark=now+3000;}return;}
          finally{busy=false;}
        }else{
          const slot=nextMark,late=now-slot;nextMark=nextAfter(now);
          if(!claimed.has(slot)&&(config.missed==='catch_up_once'||late<=3000)){claimed.add(slot);if(claimed.size>128)claimed.delete(claimed.values().next().value);if(!remaining)begin();else if(batchAttempts>0)queued=true;}
        }
      }
      if(!remaining){release();reason='waiting_clock';return;}
    }
    if(now-lastAction<config.interval_ms)return;
    busy=true;const generation=epoch;
    try{
      if(await io.action(()=>enabled&&epoch===generation&&io.inRoom()&&io.visible())){
        total++;lastAction=io.now();reason=null;
        if(enabled&&epoch===generation&&config.mode==='clock'){remaining--;batchAttempts++;if(!remaining&&queued){queued=false;begin();}else if(!remaining)release();}
      }else {release();reason=io.reason?.()||'target_unavailable';}
    }catch(_){setEnabled(false);reason='click_error';}
    finally{busy=false;}
  };
  return {setEnabled,step,getStatus:()=>({enabled,mode:config.mode,next_at_ms:nextMark,remaining,queued_batch:queued,total_attempts:total,batches,busy,reason}),dispose:()=>setEnabled(false)};
};
// Match a native like control by semantics and its position relative to the live player.
// Coordinates only constrain candidates; they are never used as a click target.
const findWechatLikeTarget=(doc,win,isVisible)=>{
  const native=Array.from(doc.querySelectorAll('.controls__ft .cheers__wrp .live-opr__btn')).filter(isVisible);
  if(native.length)return native.length===1?native[0]:null;
  const videos=Array.from(doc.querySelectorAll('video')).filter(n=>n.isConnected).map(n=>n.getBoundingClientRect()).filter(r=>r.width>=180&&r.height>=120);
  const main=videos.sort((a,b)=>b.width*b.height-a.width*a.height)[0];
  const nearby=main?videos.filter(r=>r.left<main.right+8&&r.right>main.left-8&&r.top<main.bottom+8&&r.bottom>main.top-8):[];
  const media=nearby.length?{left:Math.min(...nearby.map(r=>r.left)),right:Math.max(...nearby.map(r=>r.right)),top:Math.min(...nearby.map(r=>r.top)),bottom:Math.max(...nearby.map(r=>r.bottom))}:null;
  const matches=new Set();
  const candidates=doc.querySelectorAll('[aria-label*="点赞"],[title*="点赞"],[class*="like" i],[class*="praise" i],[class*="thumb" i],svg use');
  for(const icon of Array.from(candidates).slice(0,200)){
    const label=[icon.getAttribute?.('aria-label'),icon.getAttribute?.('title'),icon.getAttribute?.('class'),icon.getAttribute?.('href'),icon.getAttribute?.('xlink:href')].filter(Boolean).join(' ');
    if(!/(?:点赞|(?:^|[\s_#-])(?:like|praise|thumbs?(?:up)?)(?:[\s_-]|$))/i.test(label)||/dislike|unlike|animation|count|list|comment/i.test(label))continue;
    let node=icon.closest?.('button,[role="button"]');
    if(!node){for(let n=icon,depth=0;n&&n!==doc.body&&depth<4;n=n.parentElement,depth++){if(win.getComputedStyle(n).cursor==='pointer'){node=n;break;}}}
    if(!node||!isVisible(node))continue;
    const r=node.getBoundingClientRect();
    if(r.width<16||r.height<16||r.width>100||r.height>100)continue;
    if(media){const cx=r.left+r.width/2,cy=r.top+r.height/2;if(cx<media.right-100||cx>media.right+8||cy<media.top+(media.bottom-media.top)*.6||cy>win.innerHeight)continue;}
    else if(!/点赞/.test(label))continue;
    matches.add(node);
  }
  const controls=[...matches].filter(n=>![...matches].some(other=>other!==n&&other.contains(n)));
  return controls.length===1?controls[0]:null;
};
const createLikeController=config=>{
  const isDouyin=BOOT.adapter_id==='douyin.live_page';
  // Douyin's native Z handler consumes keyboard auto-repeat events. It does not
  // start its own repeat timer on the first keydown. Keep this cadence separate
  // from the WeChat mouse-click interval, and release only when the hold ends.
  const scheduleConfig=isDouyin?{...config,interval_ms:33}:config;
  const key='live-archive-like-target:'+location.origin+location.pathname;
  let selector=config.target_selector||'',picked=null,picking=false,selectionTimer=null,lastReason=null,roomIdentity=null;
  const currentIdentity=()=>{if(isDouyin)return BOOT.room_url;try{const [error,profile]=window.WXU?.check_live_existing?.({silence:true})||[];if(error)return null;return profile?.username||profile?.contact?.username||profile?.author?.username||null;}catch(_){return null;}};
  if(!selector)try{selector=window.localStorage?.getItem(key)||'';}catch(_){}
  const insidePanel=n=>!!n?.closest?.('#live-archive-assistant,#live-archive-moderation-panel');
  const visible=n=>{
    if(!n||!n.isConnected||n===document.body||n===document.documentElement||insidePanel(n)||n.matches?.('input,textarea,[contenteditable="true"],a[href]')||n.disabled||n.closest?.('[hidden],[inert],[aria-hidden="true"],[aria-disabled="true"]'))return false;
    const r=n.getBoundingClientRect(),style=window.getComputedStyle(n);
    if(!r.width||!r.height||style.display==='none'||style.visibility==='hidden'||style.opacity==='0')return false;
    const top=document.elementFromPoint?.(r.left+r.width/2,r.top+r.height/2);
    return !document.elementFromPoint||!!top&&(n===top||n.contains(top));
  };
  const target=()=>{
    if(!config.target_selector){const native=findWechatLikeTarget(document,window,visible);if(native){lastReason=null;return native;}}
    if(picked&&visible(picked))return picked;
    const query=selector||'button[aria-label="点赞"],[role="button"][aria-label="点赞"],button[title="点赞"],[role="button"][title="点赞"]';
    let nodes;try{nodes=Array.from(document.querySelectorAll(query)).filter(visible);}catch(_){lastReason='invalid_selector';return null;}
    if(nodes.length!==1){lastReason=nodes.length?'target_not_unique':'target_not_found';return null;}
    lastReason=null;return nodes[0];
  };
  let heldNode=null;
  const keyOptions={key:'z',code:'KeyZ',keyCode:90,which:90,bubbles:true,cancelable:true};
  const releaseZ=()=>{const node=heldNode;heldNode=null;if(node)node.dispatchEvent(new window.KeyboardEvent('keyup',{...keyOptions,repeat:false}));};
  const editorFocused=()=>{const n=document.activeElement;return n?.isContentEditable||n?.matches?.('input,textarea,select,iframe,[role="textbox"]')||n?.closest?.('[contenteditable="true"],[role="textbox"]');};
  const gesture=async active=>{
    if(!active())return false;
    const focused=document.activeElement;
    if(editorFocused()){lastReason='input_busy';return false;}
    if(isDouyin){
      const node=document.body;
      if(!node||typeof window.KeyboardEvent!=='function'){lastReason='keyboard_unavailable';return false;}
      if(heldNode&&heldNode!==node)releaseZ();
      const repeat=heldNode===node;heldNode=node;
      node.dispatchEvent(new window.KeyboardEvent('keydown',{...keyOptions,repeat}));
      lastReason=null;return true;
    }
    const node=target();if(!node)return false;
    const click=()=>{const r=node.getBoundingClientRect();node.dispatchEvent(new window.MouseEvent('click',{bubbles:true,cancelable:true,view:window,clientX:r.left+r.width/2,clientY:r.top+r.height/2}));};
    click();
    return true;
  };
  const engine=createLikeScheduler(scheduleConfig,{now:()=>Date.now(),inRoom:()=>sameRoom()&&!sdk.getStatus().stopped&&(!roomIdentity||currentIdentity()===roomIdentity),visible:()=>!document.hidden&&!picking&&(!isDouyin||!document.hasFocus||document.hasFocus()),claim:isDouyin&&BOOT.like_period_ledger?()=>sdk.claimLikePeriod():undefined,action:gesture,release:releaseZ,reason:()=>lastReason});
  const releaseOnFocus=()=>{if(editorFocused())releaseZ();};
  const releaseOnHidden=()=>{if(document.hidden)releaseZ();};
  if(isDouyin){document.addEventListener('focusin',releaseOnFocus,true);document.addEventListener('visibilitychange',releaseOnHidden);window.addEventListener('blur',releaseZ);window.addEventListener('pagehide',releaseZ);}
  const selectionEvents=['pointerdown','pointerup','mousedown','mouseup','click','dblclick'];
  const stopPicking=()=>{picking=false;clearTimeout(selectionTimer);for(const event of selectionEvents)document.removeEventListener(event,select,true);document.removeEventListener('keydown',escape,true);};
  const escape=e=>{if(e.key==='Escape'){e.preventDefault();e.stopImmediatePropagation();stopPicking();}};
  const cssPath=node=>{
    const parts=[];
    for(let n=node;n&&n!==document.body;n=n.parentElement){
      const tag=n.tagName?.toLowerCase();if(!tag)return '';
      const siblings=Array.from(n.parentElement?.children||[]).filter(s=>s.tagName===n.tagName);
      parts.unshift(tag+':nth-of-type('+(siblings.indexOf(n)+1)+')');
      const query='body > '+parts.join(' > ');
      if(query.length>1024)return '';
      if(document.querySelectorAll(query).length===1&&document.querySelector(query)===node)return query;
    }
    return '';
  };
  const select=e=>{
    if(insidePanel(e.target))return;
    e.preventDefault();e.stopImmediatePropagation();
    if(e.type!=='click'||!picking)return;
    const path=e.composedPath?.()||[e.target];
    let node=path.find(n=>n?.matches?.('button,[role="button"]'));
    if(!node)node=path.find(n=>n?.tagName&&n.tagName.toLowerCase()!=='path'&&n.tagName.toLowerCase()!=='svg'&&n.getBoundingClientRect&&window.getComputedStyle(n).cursor==='pointer');
    if(!node||!visible(node)){lastReason='selection_not_control';return;}
    const query=cssPath(node);if(!query){lastReason='selection_not_unique';return;}
    selector=query;picked=node;lastReason=null;picking=false;
    try{window.localStorage?.setItem(key,selector);}catch(_){}
    // Keep the rest of this gesture, including a possible double-click, blocked.
    selectionTimer=setTimeout(stopPicking,300);
  };
  const pick=()=>{
    if(isDouyin)return;
    engine.setEnabled(false);
    if(picking){stopPicking();return;}
    stopPicking();picking=true;lastReason=null;
    for(const event of selectionEvents)document.addEventListener(event,select,true);
    document.addEventListener('keydown',escape,true);
  };
  const timer=setInterval(()=>engine.step(),Math.min(scheduleConfig.interval_ms,1000));
  return {setEnabled:value=>{if(value){roomIdentity=currentIdentity();if(!roomIdentity)throw Error('等待当前直播间资料就绪');if(picking)throw Error('请先完成点赞按钮定位');if(isDouyin&&config.mode==='clock'&&!BOOT.like_period_ledger)throw Error('请升级并连接支持时段记录的新版后台');if(!isDouyin&&!target())throw Error('未识别到原生点赞按钮，请重新打开直播页');}engine.setEnabled(value);if(value)void engine.step();},pick,
    getStatus:()=>({...engine.getStatus(),picking,key_held:!!heldNode,repeat_interval_ms:isDouyin?33:null,target_ready:isDouyin||!!target(),target_reason:lastReason,selector: isDouyin?null:selector}),
    dispose:()=>{stopPicking();clearInterval(timer);engine.dispose();if(isDouyin){document.removeEventListener('focusin',releaseOnFocus,true);document.removeEventListener('visibilitychange',releaseOnHidden);window.removeEventListener('blur',releaseZ);window.removeEventListener('pagehide',releaseZ);}}};
};
if(typeof BOOT!=='undefined'&&BOOT.likes?.enabled){sdk.likes=createLikeController(BOOT.likes);sdk.onDispose(()=>sdk.likes.dispose());}
