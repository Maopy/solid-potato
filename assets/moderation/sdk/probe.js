// This bundle is served in an outer closure with a per-document BOOT ticket.
// Observation by default; submission requires explicit bootstrap opt-in.
if (window.__liveArchiveModerationV1) return;
const nativeFetch = window.fetch.bind(window);
const documentId = crypto.randomUUID();
const pairingStarted=performance.now();
let tabClientId;
if(BOOT.reload_handoff)try{const key='live-archive-tab-client';tabClientId=window.sessionStorage.getItem(key);if(!/^[a-f0-9-]{36}$/.test(tabClientId||'')){tabClientId=crypto.randomUUID();window.sessionStorage.setItem(key,tabClientId);}}catch(_){}
const pending = [];
let seq = 0, session = null, stopped = false, busy = false, timer = null;
let truncated = false, bridgeError = null;
let decoderStatus = null, previewStatus = null, preview = [];
let draining = false, ackedSeq = 0;
let candidateStatus = null, candidateUpdatedAt = 0, timedStatus=null, keywordStatus=null;
const observedCounts = Object.create(null), generatedCounts = Object.create(null), acknowledgedCounts = Object.create(null), drainers = [];
let moderationEnabled=BOOT.moderation_enabled!==false, moderationEpoch=BOOT.control_epoch||0;
const moderationListeners=[],recordingRequests=new Set();
const applyModeration=state=>{
  if(!state||state.epoch<moderationEpoch)return false;
  const changed=moderationEnabled!==state.enabled;
  moderationEpoch=state.epoch;moderationEnabled=state.enabled;
  if(changed)for(const fn of moderationListeners){try{fn(moderationEnabled);}catch(_){}}
  return true;
};
let executingFillCommand=null;
let fillExecutor = null, fillStatus = null, fillReport = null, lastFillResult = null, lastInputWait = undefined;
const fillCache = new Map();
let recordingState={state:"idle",task_id:null,error:null},recordingBusy=false,recordingPoll=null,recordingRequestedAt=0;
const disposers = [];
const sameRoom = () => typeof userscriptCurrentRoom === "function" ? userscriptCurrentRoom() : location.origin + location.pathname.replace(/\/$/, "") === BOOT.room_url;
const post = async (path, body, token) => {
  if (typeof userscriptTransport === "function") return userscriptTransport(path,body,token);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 2000);
  try {
    const response = await nativeFetch(BOOT.prefix + path, {
      method: "POST", credentials: "omit", cache: "no-store",
      headers: { "Content-Type": "application/json", ...(token ? { Authorization: "Bearer " + token } : {}) },
      body: JSON.stringify(body), signal: controller.signal
    });
    if (!response.ok) {
      const error = new Error("bridge_http_" + response.status);
      error.status = response.status;
      throw error;
    }
    return await response.json();
  } finally { clearTimeout(timeout); }
};
const clean = (text, limit = 512) => Array.from(String(text || "").replace(/[\u0000-\u001f\u007f]/g, "")).slice(0, limit).join("");
const emit = (type, source, payload = {}, identity = {}) => {
  if (stopped || draining || !sameRoom()) return false;
  observedCounts[type]=(observedCounts[type]||0)+1;
  if(!moderationEnabled&&/^(?:chat\.|gift\.|member\.|level\.|fansclub\.)/.test(type))return false;
  if (pending.length >= 499) {
    if (!truncated) {
      truncated = true;
      pending.push({ seq: ++seq, event_id: documentId + ":" + seq, event_type: "probe.gap", source_kind: "diagnostic", payload: { reason: "page_buffer_full" } });
      generatedCounts['probe.gap'] = (generatedCounts['probe.gap'] || 0) + 1;
    }
    return false;
  }
  pending.push({ seq: ++seq, event_id: documentId + ":" + seq, event_type: type, source_kind: source, payload,
    ...identity });
  generatedCounts[type] = (generatedCounts[type] || 0) + 1;
  return true;
};
const dispose = () => {
  if (stopped) return;
  stopped = true;
  recordingState={...recordingState,state:'unknown'};
  clearTimeout(timer);
  for (const fn of disposers.splice(0).reverse()) { try { fn(); } catch (_) {} }
  pending.length = 0;
  if (session) void post("goodbye", { v: 1, connection_id: session.connection_id, binding_id: session.binding_id, page_url: BOOT.room_url }, session.token).catch(() => {});
};
const sdk = { emit, clean, onDispose: fn => disposers.push(fn), onDrain: fn => drainers.push(fn), dispose,
  setDecoderStatus: value => { decoderStatus = value; },
  getStatus: () => ({ moderation_enabled:moderationEnabled,control_epoch:moderationEpoch,task_id:BOOT.task_id||null, mode: BOOT.automatic_send ? "fill_and_send" : BOOT.fill_preview ? "fill_only" : "observe", send_chat: BOOT.automatic_send===true, connected: !!session && !stopped, stopped, draining, buffered: pending.length, gap: truncated, bridge_error: bridgeError,
    delivery: { observed: { ...observedCounts }, generated: { ...generatedCounts }, acknowledged: { ...acknowledgedCounts }, acked_seq: ackedSeq },
    fill: BOOT.fill_preview ? { ...fillExecutor?.getStatus(), bridge: fillStatus, pending_result: !!fillReport, last_result: lastFillResult } : null,
    candidate_preview: candidateStatus ? {...candidateStatus,
      stale: stopped || !!bridgeError, pending: (candidateStatus.pending||[]).map(p=>({...p,
        remaining_ms: Math.max(0,p.remaining_ms-(performance.now()-candidateUpdatedAt))}))} : null,
    likes:sdk.likes?.getStatus()||null,timed_messages:timedStatus, keyword_replies:keywordStatus, recording:{...window.__liveArchiveRecordingController?.getStatus?.(),...recordingState,busy:recordingBusy}, raw: sdk.rawStatus?.() || null, decoder: decoderStatus, rules: previewStatus, previews: preview.map(item => ({ ...item })) }) };
sdk.onModerationChange=fn=>{moderationListeners.push(fn);if(BOOT.unified_ui)fn(moderationEnabled);};
sdk.setModerationEnabled=async enabled=>{
  if(!session||stopped||draining)throw Error('直播页尚未连接');
  // An off click stops local execution immediately, even if its HTTP ACK is delayed.
  if(!enabled){moderationEnabled=false;for(const fn of moderationListeners)fn(false);}
  const state=await post('control',{scope:{v:1,connection_id:session.connection_id,binding_id:session.binding_id,page_url:BOOT.room_url},enabled},session.token);
  applyModeration(state);return state;
};
sdk.claimLikePeriod=async()=>{
  if(!session||stopped||!BOOT.like_period_ledger)throw Error('请连接支持点赞时段记录的新版后台');
  return post('likes/claim',{v:1,connection_id:session.connection_id,binding_id:session.binding_id,page_url:BOOT.room_url},session.token);
};
const recordingApi=async(action,body)=>{
  if(typeof userscriptRecordingTransport==='function')return userscriptRecordingTransport(action,body);
  const response=await nativeFetch('/api/v1/download_task/'+action,action==='list'?{}:{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
  const result=await response.json();if(!response.ok||result.code!==0)throw Error(result.msg||'录制服务暂不可用');return result.data;
};
sdk.getRecordingStatus=()=>({...recordingState,busy:recordingBusy});
sdk.refreshRecording=()=>{
  if(recordingPoll)return recordingPoll;
  if(!BOOT.recording||!session||stopped)return Promise.resolve(sdk.getRecordingStatus());
  recordingPoll=(async()=>{
    try{
      const data=await recordingApi('list');if(stopped)return sdk.getRecordingStatus();
      const platform=BOOT.adapter_id==='wechat_channels.desktop_probe'?'wechat_channels':'douyin';
      let nickname;
      if(platform==='wechat_channels')try{const [error,profile]=window.WXU?.check_live_existing?.({silence:true})||[];if(!error)nickname=profile?.nickname||profile?.contact?.nickname||profile?.author?.nickname;}catch(_){}
      const matches=(data.list||[]).filter(t=>t.id===recordingState.task_id||t.platform===platform&&(platform==='douyin'?t.page_url===BOOT.room_url:!!nickname&&t.name===nickname));
      const rank={downloading:0,waiting:1,paused:2,failed:3,finished:4};matches.sort((a,b)=>(rank[a.status]??5)-(rank[b.status]??5)||Number(b.id===recordingState.task_id)-Number(a.id===recordingState.task_id)||(b.created_at_ms||0)-(a.created_at_ms||0));
      const task=matches[0],state={downloading:'recording',waiting:'starting',paused:'paused',failed:'failed',finished:'idle'};
      if(task)recordingState={state:state[task.status]||'idle',task_id:task.id,error:task.error||null};
      else if((data.list||[]).some(t=>!t.platform&&['waiting','downloading','paused'].includes(t.status)))recordingState={state:'unknown',task_id:null,error:'请更新后台以识别当前直播的录制状态'};
      else if(recordingState.state!=='starting'||Date.now()-recordingRequestedAt>5000)recordingState={state:'idle',task_id:null,error:null};
    }catch(e){recordingState={...recordingState,state:'unknown',error:e.message};}
    return sdk.getRecordingStatus();
  })().finally(()=>{recordingPoll=null;});return recordingPoll;
};
const recordingAction=async fn=>{
  if(!BOOT.recording||!session||stopped||draining||!sameRoom())throw Error('录制连接不可用');
  if(recordingBusy)throw Error('录制操作处理中');recordingBusy=true;
  try{return await fn();}finally{recordingBusy=false;}
};
sdk.startRecording=()=>recordingAction(async()=>{
  await sdk.refreshRecording();
  if(recordingState.state==='unknown')throw Error(recordingState.error||'录制状态未知');
  if(recordingState.state==='recording'||recordingState.state==='starting')return;
  let result;
  const controller=window.__liveArchiveRecordingController;
  if(controller)result=await controller.start();
  else{
    const check=window.WXU?.check_live_existing;if(typeof check!=='function')throw Error('录制入口尚未就绪，请重新打开并播放直播');
    const [error,profile,live]=check({silence:true});if(error)throw Error('请先进入并播放直播');
    result=await recordingApi('create',{objects:[{platform:'wxchannels',content:{profile,live:window.WXU.recording_live_data?.(live)||live}}]});
  }
  recordingRequestedAt=Date.now();
  recordingState={state:'starting',task_id:result?.task_id||result?.tasks?.[0]?.data?.id||null,error:null};
  await sdk.refreshRecording();
});
sdk.pauseRecording=()=>recordingAction(async()=>{
  await sdk.refreshRecording();if(!recordingState.task_id||!['recording','starting'].includes(recordingState.state))return;
  await recordingApi('pause',{task_ids:[recordingState.task_id]});await sdk.refreshRecording();
});
sdk.resumeRecording=()=>recordingAction(async()=>{
  await sdk.refreshRecording();if(!recordingState.task_id||recordingState.state!=='paused')return;
  await recordingApi('start',{task_ids:[recordingState.task_id]});await sdk.refreshRecording();
});
Object.defineProperty(window, "__liveArchiveModerationV1", { value: sdk, configurable: true });
const flushFillReport = async () => {
  if (!fillReport) return;
  const ack = await post("results", fillReport, session.token);
  if (ack.acked_action_id !== fillReport.action_id) throw new Error("fill_result_ack_mismatch");
  fillReport = null;
};
const applyFillCommand = async (command, pollElapsed, blocked=false) => {
  const valid = command?.v === 1 && command.command === (BOOT.automatic_send ? "fill_and_submit" : "fill_input") &&
    command.connection_id === session.connection_id && command.binding_id === session.binding_id &&
    command.document_id === documentId && command.page_url === BOOT.room_url &&
    typeof command.action_id === "string" && /^[a-zA-Z0-9._:-]{1,128}$/.test(command.action_id);
  let result = valid ? fillCache.get(command?.action_id) : null;
  if (!result) {
    if (!valid) result = { outcome: "rejected", reason: "binding_mismatch" };
    else if (fillCache.size >= 1000) result = { outcome: "rejected", reason: "cache_full" };
    else {
      // Reserve before awaiting the DOM verification. Replays never refill.
      fillCache.set(command.action_id, { outcome: "rejected", reason: "input_error" });
      try { executingFillCommand=command; result = blocked ? {outcome:"rejected",reason:"observation_paused"} : fillExecutor ? await fillExecutor.execute(command, pollElapsed) : { outcome: "rejected", reason: "executor_unavailable" }; }
      catch (_) { result = { outcome: "rejected", reason: "input_error" }; } finally {executingFillCommand=null;}
    }
    if (valid && fillCache.size <= 1000 && fillCache.has(command.action_id)) fillCache.set(command.action_id, result);
    window.console?.info?.("[Live Archive fill]", result.outcome, result.reason);
  }
  lastFillResult = { action_id: command.action_id, ...result, send_chat: BOOT.automatic_send===true };
  fillReport = { v: 1, connection_id: session.connection_id, binding_id: session.binding_id,
    page_url: BOOT.room_url, action_id: command.action_id, ...result };
  // Retry only this receipt after an HTTP failure, never the input write.
  if (!stopped && sameRoom()) await flushFillReport();
};
const tick = async () => {
  if (stopped || busy) return;
  if (!sameRoom()) { dispose(); return; }
  busy = true;
  try {
    if (!session) {
      session = await post("hello", { v: 1, ticket: BOOT.ticket, page_url: BOOT.room_url, document_id: documentId, adapter_id: BOOT.adapter_id || "douyin.live_page", ...(tabClientId?{client_id:tabClientId}:{}) });
      if (stopped) {
        void post("goodbye", { v: 1, connection_id: session.connection_id, binding_id: session.binding_id, page_url: BOOT.room_url }, session.token).catch(() => {});
        return;
      }
    }
    const scope = { v: 1, connection_id: session.connection_id, binding_id: session.binding_id };
    if (BOOT.fill_preview) await flushFillReport();
    if (stopped || !sameRoom()) { dispose(); return; }
    const batch = pending.slice(0, 50);
    if (batch.length) {
      const result = await post("events", { ...scope, first_seq: batch[0].seq, events: batch }, session.token);
      if (!Number.isSafeInteger(result.acked_seq) || result.acked_seq < batch[0].seq - 1 || result.acked_seq > batch[batch.length - 1].seq) throw new Error('invalid_events_ack');
      for (const event of batch) {
        if (event.seq > ackedSeq && event.seq <= result.acked_seq) acknowledgedCounts[event.event_type] = (acknowledgedCounts[event.event_type] || 0) + 1;
      }
      ackedSeq = Math.max(ackedSeq, result.acked_seq);
      while (pending.length && pending[0].seq <= result.acked_seq) pending.shift();
      if (stopped) return;
    }
    if (BOOT.automatic_send || BOOT.input_open_test) await fillExecutor?.prepareInput?.(fillStatus?.pending || (timedStatus?.active&&timedStatus.remaining_ms<=3000?1:0));
    if (stopped || !sameRoom()) { dispose(); return; }
    const pollStarted = BOOT.fill_preview ? performance.now() : 0;
    const inputState=fillExecutor?.getStatus();
    if(BOOT.wait_for_input && inputState?.input_reason!==lastInputWait){lastInputWait=inputState?.input_reason;sdk.emit('probe.status','diagnostic',{stage:'input_wait_changed',reason:lastInputWait||'input_ready'});}
    const result = await post("poll", { ...scope, page_url: BOOT.room_url, ...(BOOT.wait_for_input ? {input_ready:inputState?.input_ready===true}: {}) }, session.token);
    if (stopped || !sameRoom()) { dispose(); return; }
    if (result.draining === true || draining) {
      if (!draining) {
        for (const fn of drainers.splice(0)) { try { fn(); } catch (_) {} }
        draining = true;
      }
      bridgeError = null;
      if (pending.length || sdk.rawStatus?.().buffered_chunks) { timer = setTimeout(tick, 100); return; }
      await post('goodbye', { ...scope, page_url: BOOT.room_url }, session.token);
      session = null; dispose(); return;
    }
    applyModeration(result.moderation_control);
    const staleControl=result.moderation_control&&result.moderation_control.epoch<moderationEpoch;
    if(result.recording_request&&!recordingRequests.has(result.recording_request)){
      recordingRequests.add(result.recording_request);
      try{await sdk.startRecording();sdk.emit('recording.result','diagnostic',{request_id:result.recording_request,state:'submitted',message:'已提交录制，请在下载任务中查看'});}
      catch(error){sdk.emit('recording.result','diagnostic',{request_id:result.recording_request,state:'failed',message:sdk.clean(error.message,160)});}
    }
    timedStatus=result.timed_messages||null;
    keywordStatus=result.keyword_replies||null;
    candidateStatus = result.candidate_preview || null;
    candidateUpdatedAt = performance.now();
    const wasPaused = previewStatus?.paused;
    previewStatus = result.rules || null;
    fillStatus = result.fill || null;
    if (previewStatus?.paused && !wasPaused) window.console?.warn?.("[Live Archive paused]", previewStatus.last_reason);
    for (const item of result.previews || []) {
      preview.push(item); if (preview.length > 10) preview.shift();
      if (item.kind === "planned" && previewStatus?.mode !== "online_scheduler_preview") window.console?.info?.("[Live Archive preview]", item.action?.text);
      if (item.kind === "dispatch_preview") window.console?.info?.("[Live Archive dispatch preview]", item.action?.text);
      if (previewStatus?.mode === "online_scheduler_preview" && (item.kind === "cancelled" || item.kind === "expired")) window.console?.info?.("[Live Archive queue]", item.kind, item.reason);
      if (item.kind === "skipped") window.console?.warn?.("[Live Archive skipped gift]", item.reason);
    }
    if (BOOT.fill_preview && result.mode === (BOOT.automatic_send ? "fill_and_send" : "fill_only") && result.send_chat === (BOOT.automatic_send===true)) {
      if (!Array.isArray(result.commands) || result.commands.length > 1) throw new Error("invalid_fill_command_batch");
      if (result.commands.length) await applyFillCommand(result.commands[0], performance.now() - pollStarted, !moderationEnabled||staleControl);
    }
    bridgeError = null;
    if (stopped) return;
    timer = setTimeout(tick, BOOT.adapter_id === "wechat_channels.desktop_probe" ? 2000 : 500);
  } catch (error) {
    // A refresh can race the old document's goodbye. Its session expires in
    // 30s on older backends; keep pairing with the same scoped ticket, without
    // issuing commands or treating this temporary handover as a stopped page.
    if(!session && typeof userscriptTransport==='function' && error.status===422 && error.message==='pairing denied' && performance.now()-pairingStarted<45000){bridgeError=null;timer=setTimeout(tick,1000);return;}
    // Keep unacknowledged IDs stable. Expired sessions require a page reload
    // and a new ticket; never silently rebind buffered events to another room.
    bridgeError = error.message;
    if (stopped) return;
    if (error.status === 403 || error.status === 422) {
      dispose();
    } else {
      timer = setTimeout(tick, 2000);
    }
  } finally { busy = false; }
};
window.addEventListener("pagehide", dispose, { once: true });
sdk.onDispose(() => window.removeEventListener("pagehide", dispose));
sdk.emit("probe.status", "diagnostic", { stage: "bootstrap_installed" });
void tick();
