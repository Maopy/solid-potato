// Unique visible live-page editor; explicit opt-in submits with its local comment button.
const createWechatFillExecutor = context => {
  let composing=false,lastReason=null,lastSendButtonCount=null,lastSendTarget=null,lastOpenAt=-Infinity,lastOpenReason=null;
  const start=()=>{composing=true;},end=()=>{composing=false;};
  document.addEventListener('compositionstart',start,true);document.addEventListener('compositionend',end,true);
  const contents=n=>n.tagName==='TEXTAREA'?n.value:n.textContent;
  const findSendButtons=node=>{
    let parent=node.parentElement;
    for(let depth=0;parent&&depth<3;depth++,parent=parent.parentElement){
      if(parent===document.body||parent===document.documentElement)break;
      const buttons=chatSubmitCandidates(parent,'评论');
      if(buttons.length)return buttons;
    }
    return [];
  };
  const inspect=()=>{
    if(window.top!==window.self)return {reason:'not_top_frame'};
    if(document.hidden)return {reason:'page_hidden'};
    const nodes=Array.from(document.querySelectorAll('textarea,[contenteditable="true"]')).filter(n=>
      n.isConnected&&n.ownerDocument===document&&n.getClientRects().length&&!n.closest('#live-archive-moderation-panel'));
    if(!nodes.length)return {reason:'input_not_found'};
    if(nodes.length!==1)return {reason:'input_not_unique'};
    const node=nodes[0],style=window.getComputedStyle(node);
    if(style.visibility!=='visible'||style.display==='none'||style.opacity==='0'||node.closest('[hidden],[inert],[aria-hidden="true"]'))return {reason:'input_hidden'};
    if(node.disabled||node.readOnly||node.matches(':disabled')||node.closest('[aria-disabled="true"],[aria-readonly="true"]')||node.tagName!=='TEXTAREA'&&!node.isContentEditable)return {reason:'input_disabled'};
    // Channels collapses its editor on blur. Focus alone is safe when empty;
    // preserve existing text and composition instead of forcing blur.
    if(composing||String(contents(node)||'').replace(/[\u200b\ufeff]/g,'')!==''||node.querySelector?.('img,video,audio,[data-mention]'))return {node,reason:'input_busy'};
    return {node};
  };
  const prepareInput=async(pending)=>{
    if((!BOOT.automatic_send&&!BOOT.input_open_test)||pending<1||composing||!context.active()||!context.sameRoom()||context.paused()||document.hidden)return;
    if(inspect().reason!=='input_not_found'||performance.now()-lastOpenAt<3000)return;
    // The observed desktop entry is a rounded, visible "聊一聊" control.
    // The sidebar heading has the same words; never click the heading.
    const nodes=Array.from(document.querySelectorAll('button,[role="button"],div,span,input,textarea'));
    if(nodes.length>3000){lastOpenReason='opener_scan_limit';return;}
    const label=n=>String(n.textContent||n.getAttribute?.('placeholder')||n.getAttribute?.('aria-label')||'').trim();
    const entries=nodes.filter(n=>{
      if(!n.isConnected||n.ownerDocument!==document||n.closest('#live-archive-moderation-panel'))return false;
      if(label(n)!=='聊一聊'||!n.getClientRects().length||n.closest('[hidden],[inert],[aria-hidden="true"],[aria-disabled="true"],[disabled]'))return false;
      const style=window.getComputedStyle(n),rect=n.getBoundingClientRect();
      return style.visibility==='visible'&&style.display!=='none'&&style.opacity!=='0'&&
        parseFloat(style.borderRadius)>=8&&style.backgroundColor!=='transparent'&&style.backgroundColor!=='rgba(0, 0, 0, 0)'&&
        rect.width>=100&&rect.height>=20&&rect.height<=100;
    });
    if(entries.length!==1){lastOpenReason=entries.length?'opener_not_unique':'opener_not_found';return;}
    if(!context.active()||!context.sameRoom()||context.paused()||composing||inspect().reason!=='input_not_found')return;
    lastOpenAt=performance.now();
    const labels=chatSubmitCandidates(entries[0],'聊一聊');
    if(labels.length>1){lastOpenReason='opener_not_unique';return;}
    const entry=labels[0]||entries[0];
    try{entry.click();await new Promise(resolve=>setTimeout(resolve,0));lastOpenReason=inspect().node?'input_opened':'opener_no_editor';}
    catch(_){lastOpenReason='opener_error';}
  };
  const execute=async(command,elapsed)=>{
    const reject=reason=>{lastReason=reason;return {outcome:'rejected',reason};};
    if(!context.active())return reject('stopped');
    if(!context.sameRoom())return reject('room_changed');
    if(context.paused())return reject('observation_paused');
    if(command.command!==(BOOT.automatic_send?'fill_and_submit':'fill_input')||typeof command.text!=='string'||!command.text.trim()||/[\u0000-\u001f\u007f-\u009f]/.test(command.text))return reject('invalid_command');
    if(Array.from(command.text).length>80)return reject('text_too_long');
    if(!Number.isFinite(elapsed)||elapsed<0||!Number.isFinite(command.remaining_ttl_ms)||command.remaining_ttl_ms<=elapsed||command.remaining_ttl_ms>3000)return reject('ttl_expired');
    const deadline=performance.now()+command.remaining_ttl_ms-elapsed,candidate=inspect();
    if(candidate.reason)return reject(candidate.reason);
    const node=candidate.node;
    if(!context.active()||!context.sameRoom())return reject('stopped');
    if(context.paused())return reject('observation_paused');
    if(inspect().node!==node)return reject('input_changed');
    if(performance.now()>=deadline)return reject('ttl_expired');
    try{
      if(node.tagName==='TEXTAREA'){
        const setter=Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype,'value')?.set;
        if(!setter)return reject('input_error');setter.call(node,command.text);
      }else node.textContent=command.text;
      node.dispatchEvent(typeof InputEvent==='function'?new InputEvent('input',{bubbles:true,inputType:'insertText',data:command.text}):new Event('input',{bubbles:true}));
      await new Promise(resolve=>setTimeout(resolve,0));
      if(!node.isConnected||String(contents(node)||'').replace(/[\u200b\ufeff]/g,'')!==command.text.replace(/[\u200b\ufeff]/g,''))return reject('input_changed');
      const targets=findSendButtons(node);
      lastSendButtonCount=targets.length;lastSendTarget=targets.length===1?String(targets[0].tagName||'').toLowerCase():null;
      if(BOOT.automatic_send){
        const result=await submitFilledInput({...context,inputSafe:()=>!composing},node,command.text,deadline,contents,()=>findSendButtons(node));
        lastReason=result.reason;return result;
      }
      lastReason='input_filled';return {outcome:'filled',reason:'input_filled'};
    }catch(_){return reject('input_error');}
  };
  return {execute,prepareInput,getStatus:()=>{const state=context.paused()?{reason:'observation_paused'}:inspect();if(state.node){const targets=findSendButtons(state.node);lastSendButtonCount=targets.length;lastSendTarget=targets.length===1?String(targets[0].tagName||'').toLowerCase():null;}return {mode:BOOT.automatic_send?'fill_and_send':'fill_only',send_chat:BOOT.automatic_send===true,input_ready:!state.reason,input_reason:state.reason||null,last_reason:lastReason,last_send_button_count:lastSendButtonCount,last_send_target:lastSendTarget,last_open_reason:lastOpenReason,send_button_count:state.node?findSendButtons(state.node).length:0};},
    dispose:()=>{document.removeEventListener('compositionstart',start,true);document.removeEventListener('compositionend',end,true);}};
};
if(BOOT.fill_preview){
  fillExecutor=createWechatFillExecutor({active:()=>!stopped&&!draining&&(typeof moderationEnabled==="undefined"||moderationEnabled),sameRoom,paused:()=>!wxCollecting});
  sdk.onDispose(()=>fillExecutor.dispose());
}
