// Narrow chat editor; optional explicit submission uses its local send button.
const createDouyinFillExecutor = context => {
  const selector = 'div[class*="webcast-chatroom___input-container"] [data-slate-editor="true"][contenteditable="true"],div[class*="webcast-chatroom___input-container"] textarea[placeholder="与大家互动一下..."]';
  const sendCandidates=container=>{
    if(!container||container===document.body||container===document.documentElement)return [];
    const icons=Array.from(container.querySelectorAll('svg.webcast-chatroom___send-btn[type="button"]')).filter(n=>n.namespaceURI==='http://www.w3.org/2000/svg'&&String(n.tagName).toLowerCase()==='svg');
    return [...new Set([...icons,...chatSubmitCandidates(container,'发送')])];
  };
  let composing = false, lastReason = null;
  const compositionStart = () => { composing = true; };
  const compositionEnd = () => { composing = false; };
  document.addEventListener("compositionstart", compositionStart, true);
  document.addEventListener("compositionend", compositionEnd, true);
  const contents = node => node.tagName === "TEXTAREA" ? node.value : node.textContent;
  const blank = node => String(contents(node) || "").replace(/[\u200b\ufeff]/g, "") === "" &&
    !node.querySelector?.('img,video,audio,[data-mention]');
  const inspect = () => {
    if (window.top !== window.self) return { reason: "not_top_frame" };
    if (document.hidden) return { reason: "page_hidden" };
    const nodes = Array.from(document.querySelectorAll(selector)).filter(n => n.isConnected && n.ownerDocument === document && n.getClientRects().length);
    if (!nodes.length) return { reason: "input_not_found" };
    if (nodes.length !== 1) return { reason: "input_not_unique" };
    const node = nodes[0], style = window.getComputedStyle(node);
    if (style.visibility !== "visible" || style.display === "none" || style.opacity === "0" || node.closest('[hidden],[inert],[aria-hidden="true"]')) return { reason: "input_hidden" };
    if (node.disabled || node.readOnly || node.matches(':disabled') || node.closest('[aria-disabled="true"],[aria-readonly="true"]') || node.tagName !== "TEXTAREA" && !node.isContentEditable) return { reason: "input_disabled" };
    if (composing || document.activeElement === node || node.contains(document.activeElement) || !blank(node)) return { reason: "input_busy" };
    return { node, reason: null };
  };
  const execute = async (command, pollElapsed) => {
    const reject = reason => { lastReason = reason; return { outcome: "rejected", reason }; };
    if (!context.active()) return reject("stopped");
    if (!context.sameRoom()) return reject("room_changed");
    if (context.paused()) return reject("observation_paused");
    if (command.command !== (BOOT.automatic_send ? "fill_and_submit" : "fill_input") || typeof command.text !== "string" || !command.text.trim() || /[\u0000-\u001f\u007f-\u009f]/.test(command.text)) return reject("invalid_command");
    if (Array.from(command.text).length > 80) return reject("text_too_long");
    const text = command.text;
    if (!Number.isFinite(pollElapsed) || pollElapsed < 0 || !Number.isFinite(command.remaining_ttl_ms) || command.remaining_ttl_ms <= pollElapsed || command.remaining_ttl_ms > 3000) return reject("ttl_expired");
    const started = performance.now(), deadline = started + command.remaining_ttl_ms - pollElapsed;
    const candidate = inspect();
    if (candidate.reason) return reject(candidate.reason);
    const node = candidate.node;
    // Re-check after locating; writing itself has no asynchronous gap.
    if (!context.active() || !context.sameRoom()) return reject("stopped");
    if (context.paused()) return reject("observation_paused");
    if (performance.now() >= deadline) return reject("ttl_expired");
    if (inspect().node !== node) return reject("input_changed");
    try {
      if (node.tagName === "TEXTAREA") {
        const setter = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, "value")?.set;
        if (!setter) return reject("input_error");
        if (performance.now() >= deadline) return reject("ttl_expired");
        setter.call(node, text);
      } else {
        // Preserve the observed Slate editor structure. Unknown rich editors
        // are refused rather than destroying their contents with innerHTML.
        const leaves = node.querySelectorAll('[data-string="true"][data-leaf="true"]');
        if (leaves.length < 1 || leaves.length > 10) return reject("input_changed");
        if (performance.now() >= deadline) return reject("ttl_expired");
        leaves[0].textContent = text;
        leaves[0].removeAttribute("data-enter");
      }
      // Only input notification: no beforeinput, change, key or submit event.
      node.dispatchEvent(new Event("input", { bubbles: true }));
      await new Promise(resolve => setTimeout(resolve, 0));
      // Slate appends zero-width caret markers outside the text leaf. These
      // are editor placeholders, not a change to the visible draft.
      const rawActual = String(contents(node) || "");
      const actual = node.tagName === "TEXTAREA" ? rawActual : rawActual.replace(/[\u200b\ufeff]/g, "");
      const expected = node.tagName === "TEXTAREA" ? text : text.replace(/[\u200b\ufeff]/g, "");
      // Douyin itself truncates at 50 UTF-16 units on input. Accept only that
      // exact platform prefix; never shorten/rewrite text ourselves or accept
      // arbitrary edits. Submission rechecks the resulting editor contents.
      const matches = actual === expected || expected.length > 50 && actual === expected.slice(0, 50);
      if (!node.isConnected || !matches) return reject("input_changed");
      if (BOOT.automatic_send) {
        const result=await submitFilledInput({...context,inputSafe:()=>!composing},node,actual,deadline,contents,()=>{
          const container=node.closest('div[class*="webcast-chatroom___input-container"]');
          return sendCandidates(container);
        });
        lastReason=result.reason;return result;
      }
      lastReason = "input_filled";
      return { outcome: "filled", reason: "input_filled" };
    } catch (_) { return reject("input_error"); }
  };
  return { execute,
    getStatus: () => { const state = inspect(); return { mode: BOOT.automatic_send?"fill_and_send":"fill_only", send_chat: BOOT.automatic_send===true, input_ready: !state.reason, input_reason: state.reason, last_reason: lastReason, send_button_count: state.node?sendCandidates(state.node.closest('div[class*="webcast-chatroom___input-container"]')).length:0 }; },
    dispose: () => { document.removeEventListener("compositionstart", compositionStart, true); document.removeEventListener("compositionend", compositionEnd, true); }
  };
};
if (BOOT.fill_preview) {
  fillExecutor = createDouyinFillExecutor({ active: () => !stopped && !draining && (typeof moderationEnabled==="undefined"||moderationEnabled), sameRoom,
    paused: () => truncated || !!decoderStatus?.last_error || !!previewStatus?.paused });
  sdk.onDispose(() => fillExecutor.dispose());
}
