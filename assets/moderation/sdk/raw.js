// Independent development journal. Captures before semantic decoding, quotas
// and content deduplication. Transport retries keep the same chunk sequence.
const rawQueue = [];
let rawSeq = 0, rawBytes = 0, rawBusy = false, rawTimer = null;
let rawAcked = 0, rawCaptured = 0, rawDropped = 0, rawError = null, rawOverflow = false, rawReads = 0;
const RAW_BUFFER = 8 * 1024 * 1024, RAW_CHUNK = 24576;
const rawBase64 = bytes => {
  const pieces = [];
  for (let i=0;i<bytes.length;i+=8192) pieces.push(String.fromCharCode(...bytes.subarray(i,i+8192)));
  return btoa(pieces.join(''));
};
const rawSanitize = value => {
  const active = new WeakSet(); let visits = 0;
  const walk = (v, depth=0) => {
    if (++visits > 200000 || depth > 128) throw Error('serialization_budget');
    if (typeof v === 'string') {
      // Native callbacks often wrap JSON in a string. Redact inside it too.
      if (/^\s*[\[{]/.test(v)) { let parsed; try { parsed=JSON.parse(v); } catch (_) {} if (parsed) return JSON.stringify(walk(parsed,depth+1)); }
      if (v.length && v.length<=4*1024*1024 && v.length%4===0 && /^[A-Za-z0-9+/]+={0,2}$/.test(v)) {
        let parsed;
        try {const text=new TextDecoder('utf-8',{fatal:true}).decode(Uint8Array.from(atob(v),c=>c.charCodeAt(0)));if (/^\s*[\[{]/.test(text)) parsed=JSON.parse(text);} catch (_) {}
        if (parsed) return rawBase64(new TextEncoder().encode(JSON.stringify(walk(parsed,depth+1))));
      }
      return v.replace(/([?&](?:access_token|refresh_token|token|ticket|pass_ticket|wxtoken|authkey|cookie|skey|session_key)=)[^&#\s]*/gi,'$1[REDACTED]');
    }
    if (typeof v === 'bigint') return v.toString();
    if (v == null || typeof v !== 'object') return typeof v === 'function' || typeof v === 'undefined' ? '[UNSERIALIZABLE]' : v;
    if (active.has(v)) throw Error('cyclic_message');
    active.add(v);
    const out=Array.isArray(v)?[]:{};
    for (const [key,d] of Object.entries(Object.getOwnPropertyDescriptors(v))) {
      if (!d.enumerable) continue;
      Object.defineProperty(out,key,{value:/^(?:cookie|set[-_]?cookie|authorization|proxy[-_]?authorization|password|passwd|access[-_]?token|refresh[-_]?token|token|ticket|pass[-_]?ticket|wxtoken|auth[-_]?key|skey|session[-_]?key|secret|credential)$/i.test(key)
        ? '[REDACTED]' : 'value' in d ? walk(d.value,depth+1) : '[ACCESSOR]', enumerable:true, configurable:true, writable:true});
    }
    active.delete(v); return out;
  };
  return walk(value);
};
const rawPush = (channel,category,encoding,data,capturedAt=Date.now()) => {
  const id=crypto.randomUUID(), parts=Math.max(1,Math.ceil(data.length/RAW_CHUNK));
  for (let part=0;part<parts;part++) {
    const chunk={seq:++rawSeq,capture_id:id,captured_at_ms:capturedAt,channel,category,encoding,part,parts,data:data.slice(part*RAW_CHUNK,(part+1)*RAW_CHUNK)};
    rawQueue.push(chunk); rawBytes+=chunk.data.length+256;
  }
};
const rawGap = reason => {
  if (stopped || !BOOT.raw_messages) return;
  rawDropped++;
  if (!rawOverflow) { rawPush('diagnostic','capture','gap',reason); rawOverflow=true; }
};
const rawEnqueue = (channel,category,encoding,data,at) => {
  if (stopped || !sameRoom()) return;
  if (data.length > 4*1024*1024 || rawBytes+data.length+Math.ceil(data.length/RAW_CHUNK)*256>RAW_BUFFER) {rawGap(data.length>4*1024*1024?'message_too_large':'buffer_full');return;}
  if (rawOverflow) {rawPush('diagnostic','capture','gap','capture_resumed_dropped_'+rawDropped);rawOverflow=false;}
  rawPush(channel,category,encoding,data,at); rawCaptured++;
};
const rawCapture = (channel,category,value,accepted=false,at=Date.now()) => {
  if (!BOOT.raw_messages || stopped || (!accepted && draining) || !sameRoom()) return;
  try {
    if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
      const bytes=value instanceof ArrayBuffer?new Uint8Array(value):new Uint8Array(value.buffer,value.byteOffset,value.byteLength);
      if (bytes.byteLength>3*1024*1024) {rawGap('message_too_large');return;}
      rawEnqueue(channel,category,'binary_base64',rawBase64(bytes),at);
    } else if (typeof Blob!=='undefined' && value instanceof Blob) {
      if (value.size>3*1024*1024) {rawGap('message_too_large');return;}
      rawReads++;
      void value.arrayBuffer().then(bytes=>rawEnqueue(channel,category,'binary_base64',rawBase64(new Uint8Array(bytes)),at)).catch(()=>rawGap('blob_read_failed')).finally(()=>rawReads--);
    } else {
      const json=JSON.stringify(rawSanitize(value));
      rawEnqueue(channel,category,'json_utf8_base64',rawBase64(new TextEncoder().encode(json)),at);
    }
  } catch (_) {rawGap('serialization_failed');}
};
sdk.captureRaw=(channel,category,value)=>rawCapture(channel,category,value);
sdk.captureRawGap=rawGap;
sdk.captureResponse = async (channel,response) => {
  if (!BOOT.raw_messages || stopped || draining || !sameRoom()) return;
  if (!/json|text/.test(response.headers.get('content-type')||'')) return;
  if (rawReads>=4) {rawGap('response_reader_limit');return;}
  let reader, timeout, timedOut=false; const at=Date.now();rawReads++;
  try {
    reader=response.clone().body?.getReader();
    if (!reader) return;
    timeout=setTimeout(()=>{timedOut=true;rawGap('response_read_timeout');void reader.cancel().catch(()=>{});},5000);
    const chunks=[];let size=0;
    while (!stopped) {
      const next=await reader.read();if (next.done) break;
      size+=next.value.length;
      if (size>3*1024*1024) {rawGap('response_too_large');return;}
      chunks.push(next.value);
    }
    if (timedOut || stopped) return;
    const bytes=new Uint8Array(size);let offset=0;
    for (const chunk of chunks) {bytes.set(chunk,offset);offset+=chunk.length;}
    rawCapture(channel,'response',new TextDecoder('utf-8',{fatal:true}).decode(bytes),true,at);
  } catch (_) {rawGap('response_read_failed');} finally {clearTimeout(timeout);void reader?.cancel().catch(()=>{});rawReads--;}
};
sdk.rawStatus=()=>({enabled:BOOT.raw_messages===true,captured:rawCaptured,acked_chunks:rawAcked,buffered_chunks:rawQueue.length+rawReads,dropped:rawDropped,error:rawError});
const rawFlush = async () => {
  if (stopped || rawBusy) return;
  rawBusy=true;
  try {
    if (session && rawQueue.length) {
      const chunks=rawQueue.slice(0,3), result=await post('raw',{v:1,connection_id:session.connection_id,binding_id:session.binding_id,chunks},session.token);
      if (!Number.isSafeInteger(result.acked_seq) || result.acked_seq!==chunks[chunks.length-1].seq) throw Error('invalid_raw_ack');
      while (rawQueue.length && rawQueue[0].seq<=result.acked_seq) {const chunk=rawQueue.shift();rawBytes-=chunk.data.length+256;}
      rawAcked=result.acked_seq; rawError=null;
    }
  } catch(error) {rawError=error.message;} finally {
    rawBusy=false;
    if (!stopped) rawTimer=setTimeout(rawFlush,rawError?2000:rawQueue.length?20:100);
  }
};
if (BOOT.raw_messages) rawTimer=setTimeout(rawFlush,100);
sdk.onDispose(()=>{clearTimeout(rawTimer);rawQueue.length=0;});
