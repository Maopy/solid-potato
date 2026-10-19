// Attempt each command once. Do not wait for input clearing or a server echo.
const submitFilledInput = async (context, node, text, deadline, contents, findButtons) => {
  const reject = reason => ({outcome:'rejected',reason});
  const guard = () => context.inputSafe() && context.active() && context.sameRoom() && !context.paused() && !document.hidden &&
    node.isConnected && performance.now() < deadline && String(contents(node)||'').replace(/[\u200b\ufeff]/g,'')===text.replace(/[\u200b\ufeff]/g,'');
  if (BOOT.automatic_send !== true) return reject('invalid_command');
  if (!guard()) return reject('input_changed');
  const visible = n => n.isConnected && n.ownerDocument===document && n.getClientRects().length &&
    !n.closest('[hidden],[inert],[aria-hidden="true"]') && window.getComputedStyle(n).visibility==='visible' &&
    window.getComputedStyle(n).display!=='none' && window.getComputedStyle(n).opacity!=='0';
  const buttons = findButtons().filter(visible);
  if (!buttons.length) return reject('send_not_found');
  if (buttons.length!==1) return reject('send_not_unique');
  const button=buttons[0];
  if (button.disabled || button.matches(':disabled') || button.closest('button:disabled,[disabled],[aria-disabled="true"]')) return reject('send_disabled');
  const finalButtons=findButtons().filter(visible);
  if (!guard() || finalButtons.length!==1 || finalButtons[0]!==button || button.disabled || button.matches(':disabled') || button.closest('button:disabled,[disabled],[aria-disabled="true"]')) return reject('input_changed');
  try {
    // SVG controls do not expose HTMLElement.click(). Dispatch one bubbling
    // mouse click on the exact control selected by the platform adapter.
    if(typeof button.click==='function')button.click();
    else if(button.namespaceURI==='http://www.w3.org/2000/svg'&&String(button.tagName).toLowerCase()==='svg')button.dispatchEvent(new window.MouseEvent('click',{bubbles:true,cancelable:true,view:window}));
    else throw Error('unsupported_send_control');
    return {outcome:'submitted',reason:'input_submitted'};
  } catch (_) {
    // A throwing handler may have sent. Record the attempt; never retry it.
    return {outcome:'unknown',reason:'submission_unconfirmed'};
  }
};

const chatSubmitCandidates = (container,label) => {
  if(!container||container===document.body||container===document.documentElement)return [];
  const nodes=Array.from(container.querySelectorAll('button,[role="button"],div,span'));
  if(nodes.length>300)return [];
  const matching=nodes.filter(n=>String(n.textContent||'').trim()===label);
  // Never click a wrapper whose text happens to be identical to its button.
  // Prefer the actual control; for custom controls click the innermost label
  // so its click bubbles through the real handler rather than skipping it.
  const native=matching.filter(n=>n.tagName==='BUTTON');
  if(native.length)return native;
  const roles=matching.filter(n=>n.getAttribute?.('role')==='button');
  const targets=roles.length?roles:matching;
  return targets.filter(n=>!targets.some(child=>child!==n&&n.contains(child)));
};
