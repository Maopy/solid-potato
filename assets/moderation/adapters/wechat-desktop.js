// Desktop WeChat WebView candidates only. Never submits or invents IDs/quantity semantics.
// The transport/nonce injection is shared with the recording approach.
let wxCollecting = false;
const wxStatus = { adapter: 'wechat_channels.desktop_probe', coverage: 'candidate_observation', collecting: false, recent_schemas: [],
  room_verified: false, actor_verified: false, network_messages: 0, schemas: 0,
  message_batches: 0, sampled_entries: 0, recent_message_batch: null,
  page_matches: 0, recent_page_match: null,
  chat_candidates: 0, candidate_duplicates: 0, candidate_conflicts: 0, candidate_skipped: 0, candidate_dropped: 0,
  gift_candidates: 0, gift_revisions: 0, gift_duplicates: 0, gift_conflicts: 0, gift_skipped: 0, gift_dropped: 0,
  message_content_schemas: 0, recent_message_content: null, command_responses: 0, gift_schemas: 0, application_payloads: 0, recent_application: null, recent_command: null, recent_gift_schema: null,
  event_bus: false, dropped: 0, hook_replaced: false, native_bridge: false, native_hooks: 0, native_methods: [], interaction_decoding: 'wechat.type1_candidate' };
const wxPublish = () => sdk.setDecoderStatus({ ...wxStatus });
const wxSetCollecting = value => {
  if (value && !sdk.getStatus().stopped) wxRepairHooks();
  const next = value && !sdk.getStatus().stopped;
  if (next !== wxCollecting) wxKeywordFloor = null;
  wxCollecting = next;
  if (!wxCollecting) wxMatchPending.length = 0;
  wxStatus.collecting = wxCollecting; wxPublish();
  sdk.emit('probe.status', 'diagnostic', { stage: wxCollecting ? 'wechat_collection_started' : 'wechat_collection_paused' });
};
sdk.onModerationChange?.(enabled=>{Promise.resolve().then(()=>wxSetCollecting(enabled));});
const wxRelevant = value => {
  try {
    const u = new URL(typeof value === 'string' ? value : value.url, location.href);
    // Inspect page-owned WeChat/Tencent transports, not just the HTML origin.
    // Never export hostnames, paths or query parameters.
    const host = u.hostname;
    const platform = host === 'weixin.qq.com' || host.endsWith('.weixin.qq.com')
      || host === 'qq.com' || host.endsWith('.qq.com')
      || host === 'weixinbridge.com' || host.endsWith('.weixinbridge.com');
    return platform && ['https:', 'wss:'].includes(u.protocol)
      && !u.pathname.startsWith(BOOT.prefix);
  } catch (_) { return false; }
};
let wxWindow = Date.now(), wxQuota = 0;
const wxEmit = (type, source, payload) => {
  if (!wxCollecting || sdk.getStatus().stopped) return;
  if (Date.now() - wxWindow >= 60000) { wxWindow = Date.now(); wxQuota = 0; }
  if (++wxQuota > 60) { wxStatus.dropped++; wxPublish(); return; }
  sdk.emit(type, source, payload);
};
// Focused protocol hints have their own small budget, so polling diagnostics
// cannot starve the first gift/application/command structure in a minute.
let wxDetailWindow = Date.now(), wxDetailQuota = 0;
const wxDetailEmit = (type, payload) => {
  if (!wxCollecting || sdk.getStatus().stopped) return;
  if (Date.now() - wxDetailWindow >= 60000) { wxDetailWindow = Date.now(); wxDetailQuota = 0; }
  if (wxDetailQuota >= 30) { wxStatus.dropped++; return; }
  wxDetailQuota++; sdk.emit(type, 'diagnostic', payload);
};
const wxNetwork = (channel, format, bytes = 0, source = 'network') => {
  if (!wxCollecting || sdk.getStatus().stopped) return;
  wxStatus.network_messages++;
  wxEmit('probe.network', source, { channel, format, bytes });
  wxPublish();
};
// Structural diagnostics only. Unknown/dynamic keys use position labels so identifiers
// used as object keys cannot leave the page. Values and callback names stay local.
const wxNames = new Set(['data', 'result', 'msgList', 'messageList', 'messages', 'msg', 'commentList',
  'comment', 'giftList', 'gift', 'giftId', 'giftName', 'giftCount', 'count', 'num', 'type', 'cmd',
  'liveId', 'objectId', 'finderUsername', 'username', 'nickname', 'user', 'userId', 'seq', 'timestamp',
  'args', 'payload', 'params', 'body', 'header', 'content', 'items', 'list', 'status', 'code',
  'errMsg', 'errCode', 'event', 'eventName', 'eventType', 'callbackId', 'method', 'name',
  '__json_message', '__params', '__type', '__msg_type', '__eventName', '__callback_id', '__event_id',
  'baseResponse', 'baseRequest', 'BaseResponse', 'BaseRequest', 'ret', 'retCode', 'errmsg', 'err_msg', 'err_code',
  'errMessage', 'errorCode', 'errorMsg', 'response', 'request', 'responseData', 'requestData', 'resp', 'req',
  'apiName', 'cgi', 'cgiName', 'cgiUrl', 'command', 'cmdId', 'buffer', 'json', 'jsonData', 'rawData',
  'liveMsgList', 'liveMsg', 'liveMsgId', 'msgId', 'msgType', 'msgContent', 'msgData', 'msg_type', 'msg_list',
  'msg_id', 'messageType', 'messageId', 'message', 'messageContent', 'text', 'textContent', 'contentType',
  'fromUser', 'fromUserContact', 'finderLiveContact', 'contact', 'toUser', 'toUserContact', 'sender', 'senderUserId', 'displayName', 'headUrl', 'avatar', 'avatarUrl',
  'baseResp', 'baseReq', 'cgiRequest', 'cgiResponse', 'cgiCmd', 'cgiCmdId', 'appMsgList', 'fromUserName', 'clientMsgId', 'msgSource',
  'giftInfo', 'giftDetail', 'giftNum', 'gift_id', 'gift_name', 'gift_count', 'comboCount', 'repeatCount',
  'reward_product_id', 'reward_product_count', 'reward_gift', 'combo_product_count',
  'is_floatmsg', 'float_type',
  'likeCount', 'member', 'memberList', 'fansLevel', 'level', 'levelType', 'roomId', 'room_id', 'live_id']);
// Generated protocol bindings use PascalCase, snake_case and underscore
// prefixes for the same finite vocabulary. Preserve only these known variants.
for (const key of [...wxNames]) {
  const snake = key.replace(/[A-Z]/g, letter => '_' + letter.toLowerCase());
  for (const variant of [key, key[0].toUpperCase() + key.slice(1), snake]) {
    wxNames.add(variant); wxNames.add('_' + variant); wxNames.add('__' + variant);
  }
}
const wxKey = (key, index) => wxNames.has(key) ? key : 'unknown_' + index;
const wxOwnValue = (object, key) => {
  if (!object || typeof object !== 'object') return undefined;
  const descriptor = Object.getOwnPropertyDescriptor(object, key);
  return descriptor && 'value' in descriptor ? descriptor.value : undefined;
};
const wxJsonObjectText = text => {
  if (!/^\s*[\[{]/.test(text)) return false;
  try { const parsed = JSON.parse(text); return parsed !== null && typeof parsed === 'object'; } catch (_) { return false; }
};
const wxGiftSchemas = new Set();
const wxGiftKeys = new Set(['gift','giftInfo','gift_info','giftList','gift_list','giftId','gift_id','giftName','gift_name','giftCount','gift_count','giftNum','gift_num','reward_gift','reward_product_id']);
// Keep root fields first, then inspect bounded child structure without values.
// A gift/app envelope may contain configuration or rewards, not a delivery.
const wxDetailFields = value => {
  const fields = [], queue = [{ value, path: '$', depth: 0 }], seen = new WeakSet();
  let visits = 0, decoded = 0, parsedChars = 0;
  while (queue.length && fields.length < 24 && ++visits <= 100) {
    const item = queue.shift();
    if (item.depth > 3) continue;
    if (typeof item.value === 'string') {
      if (decoded < 3 && item.value.length <= 8192 && parsedChars + item.value.length <= 16384 && wxJsonObjectText(item.value)) {
        decoded++; parsedChars += item.value.length;
        queue.push({ value: JSON.parse(item.value), path: item.path + '<json>', depth: item.depth + 1 });
      }
      continue;
    }
    if (!item.value || typeof item.value !== 'object' || seen.has(item.value)) continue;
    seen.add(item.value);
    const array = Array.isArray(item.value);
    for (const [index, key] of Object.keys(item.value).slice(0, array ? 3 : 20).entries()) {
      const descriptor = Object.getOwnPropertyDescriptor(item.value, key);
      const child = descriptor && 'value' in descriptor ? descriptor.value : undefined;
      const type = descriptor && !('value' in descriptor) ? 'accessor' : child === null ? 'null' : Array.isArray(child) ? 'array' : typeof child;
      const path = item.path + (array ? '[' + index + ']' : '.' + wxKey(key, index + 1));
      if (path.length + type.length + 1 > 96) continue;
      fields.push(path + ':' + type);
      if (fields.length >= 24) break;
      if (queue.length < 100 && descriptor && 'value' in descriptor && child && (typeof child === 'object' || typeof child === 'string')) {
        queue.push({ value: child, path, depth: item.depth + 1 });
      }
    }
  }
  return fields.sort();
};
// Focused structure hints may also describe gift catalogues or configuration.
// They never establish a received gift, quantity semantics, or a stable actor.
const wxGiftSchema = (channel, object, context, messageType = null) => {
  if (!object || typeof object !== 'object' || Array.isArray(object)) return false;
  const keys = Object.keys(object).slice(0, 20);
  if (!keys.some(key => wxGiftKeys.has(key))) return false;
  const fields = wxDetailFields(object);
  const signature = JSON.stringify([channel, context, messageType, fields]);
  if (!wxGiftSchemas.has(signature) && wxGiftSchemas.size < 32) {
    wxGiftSchemas.add(signature); wxStatus.gift_schemas++;
    const payload = { channel, context, message_type: messageType, fields };
    wxStatus.recent_gift_schema = payload;
    wxDetailEmit('probe.gift_schema', payload);
  }
  return true;
};
const wxGiftWalk = (channel, value, context, messageType = null, applications = false) => {
  let visits = 0, hints = 0, decoded = 0, parsedChars = 0;
  const visit = (item, depth) => {
    if (!item || depth > 4 || ++visits > 100) return;
    if (typeof item === 'string') {
      if (decoded < 3 && item.length <= 8192 && parsedChars + item.length <= 16384 && wxJsonObjectText(item)) {
        decoded++; parsedChars += item.length; visit(JSON.parse(item), depth + 1);
      }
      return;
    }
    if (typeof item !== 'object') return;
    if (applications) wxApplication(channel, item);
    if (wxGiftSchema(channel, item, context, messageType)) hints++;
    for (const key of Object.keys(item).slice(0, Array.isArray(item) ? 3 : 20)) {
      const child = wxOwnValue(item, key); if (child && (typeof child === 'object' || typeof child === 'string')) visit(child, depth + 1);
    }
  };
  visit(value, 0); return hints;
};
const wxBase64Json = (text,limit=8192) => {
  if (typeof text !== 'string' || text.length > limit || text.length % 4 !== 0
      || !/^[A-Za-z0-9+/]+={0,2}$/.test(text)) return null;
  try {
    const bytes = Uint8Array.from(atob(text), c => c.charCodeAt(0));
    const decoded = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    return wxJsonObjectText(decoded) ? JSON.parse(decoded) : null;
  } catch (_) { return null; }
};
const wxApplicationSchemas = new Set(), wxSummarySeen=new Set();
sdk.onDispose(()=>wxSummarySeen.clear());
// Development review stays in this document, outside decoder status/transport.
// Only short property labels, public product names and small named counts.
let wxLocalReview = false;
const wxApplicationReview = [];
const wxReviewApplication = (object, value) => {
  if (!wxLocalReview || !value || typeof value !== 'object' || Array.isArray(value)) return;
  const labels = [], counts = [], names = [];
  const label = (key, index) => /^[_a-zA-Z][_a-zA-Z0-9]{0,47}$/.test(key) && !/^wxid_|^[a-fA-F0-9]{16,}$/.test(key)
    ? key : 'unknown_' + index;
  const inspect = (item, prefix, includeNames) => {
    for (const [index,key] of Object.keys(item).slice(0,20).entries()) {
      const v=wxOwnValue(item,key), safe=label(key,index+1);
      const type=v===null?'null':Array.isArray(v)?'array':typeof v;
      labels.push(prefix+safe+':'+type);
      if (safe===key && /count|quantity|(?:^|_)num$|Num$|repeat|combo/i.test(key)
          && Number.isSafeInteger(v) && v>=0 && v<=1000000) counts.push(prefix+safe+'='+v);
      if (includeNames && key==='name' && typeof v==='string') names.push(sdk.clean(v,64));
    }
  };
  inspect(object,'entry.',false); inspect(value,'payload.',false);
  for (const [index,key] of Object.keys(value).slice(0,20).entries()) {
    const item=wxOwnValue(value,key);
    if (item && typeof item==='object' && !Array.isArray(item) && typeof wxOwnValue(item,'name')==='string') {
      inspect(item,'payload.'+label(key,index+1)+'.',true); break;
    }
  }
  // Optional target identity is shown only in the explicit local review.
  // Never add it to decoder status, SDK events or the journal.
  const targetContact=wxOwnValue(wxOwnValue(object,'to_user_contact'),'contact');
  const targetId=wxOwnValue(targetContact,'username'), targetName=wxOwnValue(targetContact,'nickname');
  const target=typeof targetId==='string' && targetId.trim() && Array.from(targetId).length<=256
    && !/[\u0000-\u001f\u007f-\u009f]/.test(targetId)
    ? {id:targetId,name:typeof targetName==='string'?sdk.clean(targetName,64).replace(/[\u0080-\u009f]/g,''):''}:null;
  wxApplicationReview.push({target,labels:labels.slice(0,60),counts:counts.slice(0,12),names:names.slice(0,2)});
  if (wxApplicationReview.length>3) wxApplicationReview.shift();
};
sdk.onDispose(()=>{wxLocalReview=false;wxApplicationReview.length=0;});
// The 20009 Base64/JSON layout is a public assistant-client reference. Desktop
// viewer semantics still require real samples; export only structure hints.
const wxApplication = (channel, object) => {
  if (channel !== 'native_bridge') return;
  const type = wxOwnValue(object, 'msg_type') ?? wxOwnValue(object, 'msgType');
  if (type !== 20009 && type !== 20013) return;
  const text = wxOwnValue(object, 'payload');
  let value = null, format = 'opaque';
  if (typeof text === 'string' && text.length <= 524288) {
    if (wxJsonObjectText(text)) { value = JSON.parse(text); format = 'json'; }
    else { value = wxBase64Json(text,524288); if (value) format = 'base64_json'; }
  }
  const fields = value ? wxDetailFields(value) : [];
  wxReviewApplication(object,value);
  if (value && type===20009) wxGiftCandidate(channel,object,value);
  if (value && type===20013) {
    const contact=wxOwnValue(wxOwnValue(object,'from_user_contact'),'contact'),target=wxOwnValue(wxOwnValue(object,'to_user_contact'),'contact');
    const id=wxOwnValue(object,'client_msg_id'),user=wxOwnValue(contact,'username'),combo=wxOwnValue(value,'combo_id'),gift=wxOwnValue(value,'reward_gift');
    const summaryKey=JSON.stringify([user,id,combo]);
    if(wxIdentifier(id)&&wxIdentifier(user,256)&&!wxSummarySeen.has(summaryKey)&&sdk.emit('gift.summary','network',{message_type:20013,combo_id:wxIdentifier(combo)&&combo!=='0'?combo:null,gift_id:wxOwnValue(value,'reward_product_id'),gift_name:sdk.clean(wxOwnValue(gift,'name'),64),recipient_user_id:wxOwnValue(target,'username'),decoder:'wechat.app20013.display.v1'}, {native_event_id:id,user_id:user})){wxPublicActivity(id,20013);wxSummarySeen.add(summaryKey);if(wxSummarySeen.size>2048)wxSummarySeen.delete(wxSummarySeen.values().next().value);}
  }
  if(!value) sdk.emit('probe.gap','diagnostic',{reason:'gift_payload_unreadable'});
  const payload = { message_type: type, format, fields, envelope_fields: wxDetailFields(object),
    gift_hints: value ? wxGiftWalk(channel, value, 'application_payload', type) : 0 };
  // Correlate only locally; page containment is not delivery semantics.
  if (value) wxQueueMatch(type, wxOwnValue(value, 'content'));
  wxStatus.application_payloads++; wxStatus.recent_application = payload;
  const signature = JSON.stringify(payload);
  if (!wxApplicationSchemas.has(signature) && wxApplicationSchemas.size < 32) {
    wxApplicationSchemas.add(signature); wxDetailEmit('probe.application', payload);
  }
  wxPublish();
};
// This is a bounded wire-format probe, not a platform message decoder. A
// syntactically valid Protobuf payload still has unknown field semantics.
const wxProtoFields = text => {
  if (text.length < 16 || text.length > 8192 || text.length % 4 !== 0
      || !/^[A-Za-z0-9+/]+={0,2}$/.test(text)) return null;
  let bytes; try { bytes = Uint8Array.from(atob(text), c => c.charCodeAt(0)); } catch (_) { return null; }
  let offset = 0, tags = 0; const fields = new Set();
  const varint = () => {
    let value = 0, factor = 1;
    for (let i = 0; i < 10; i++) {
      if (offset >= bytes.length) throw Error('truncated');
      const byte = bytes[offset++]; if (i === 9 && byte > 1) throw Error('overflow');
      value += (byte & 127) * factor; factor *= 128;
      if (!(byte & 128)) return value;
    }
    throw Error('overflow');
  };
  try {
    while (offset < bytes.length) {
      if (++tags > 100) return null;
      const tag = varint();
      if (!Number.isSafeInteger(tag) || tag < 8 || tag > 4294967295) return null;
      const number = Math.floor(tag / 8), wire = tag % 8;
      let kind;
      if (wire === 0) { varint(); kind = 'varint'; }
      else if (wire === 1) { offset += 8; kind = 'fixed64'; }
      else if (wire === 2) { const length = varint(); if (!Number.isSafeInteger(length) || length < 0) return null; offset += length; kind = 'bytes'; }
      else if (wire === 5) { offset += 4; kind = 'fixed32'; }
      else return null;
      if (offset > bytes.length) return null;
      if (fields.size < 12) fields.add('f' + number + ':' + kind);
    }
    return [...fields].sort();
  } catch (_) { return null; }
};
const wxCommandSchemas = new Set();
const wxCommand = (channel, object) => {
  if (channel !== 'native_bridge') return;
  const code = wxOwnValue(object, 'cmd_id');
  if (!Number.isInteger(code) || code < 0 || code > 65535) return;
  const payload = { command_id: code, string_fields: 0, json_strings: 0, base64_json_strings: 0, base64_candidates: 0,
    protobuf_candidates: 0, plain_strings: 0, oversized_strings: 0, gift_hints: 0, fields: [] };
  let chars = 0;
  const jsonFields = value => {
    for (const [i, key] of Object.keys(value).slice(0, 12).entries()) {
      if (payload.fields.length >= 12) break;
      const v = wxOwnValue(value, key);
      payload.fields.push('s' + payload.string_fields + '.$.' + wxKey(key, i + 1) + ':'
        + (v === null ? 'null' : Array.isArray(v) ? 'array' : typeof v));
    }
  };
  for (const key of Object.keys(object).slice(0, 20)) {
    const text = wxOwnValue(object, key);
    if (typeof text !== 'string' || payload.string_fields >= 8) continue;
    payload.string_fields++;
    if (text.length > 8192 || chars + text.length > 32768) { payload.oversized_strings++; continue; }
    chars += text.length;
    if (wxJsonObjectText(text)) {
      const json = JSON.parse(text); jsonFields(json);
      payload.json_strings++; payload.gift_hints += wxGiftWalk(channel, json, 'command_json', null, true);
    } else if (text.length >= 16 && text.length % 4 === 0 && /^[A-Za-z0-9+/]+={0,2}$/.test(text)) {
      payload.base64_candidates++;
      const json = wxBase64Json(text);
      if (json) {
        jsonFields(json);
        payload.base64_json_strings++; payload.gift_hints += wxGiftWalk(channel, json, 'command_base64_json', null, true);
        continue;
      }
      const fields = wxProtoFields(text);
      if (fields?.length) {
        payload.protobuf_candidates++;
        for (const field of fields) if (payload.fields.length < 12) payload.fields.push('s' + payload.string_fields + '.' + field);
      }
    } else payload.plain_strings++;
  }
  wxStatus.command_responses++; wxStatus.recent_command = payload;
  const signature = JSON.stringify(payload);
  if (!wxCommandSchemas.has(signature) && wxCommandSchemas.size < 32) {
    wxCommandSchemas.add(signature); wxDetailEmit('probe.command', payload);
  }
  wxPublish();
};
// Explicit candidate events are separate from verified chat.received events.
// Dedup is document-local and bounded, keyed by observed sender + message ID.
const wxChatSeen = new Map();
let wxChatWindow = Date.now(), wxChatQuota = 0;
const wxIdentifier = (value, limit=128) => typeof value === 'string' && value.trim().length > 0
  && Array.from(value).length <= limit && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
// Confirmed field spellings, unconfirmed delivery/quantity semantics. Keep
// these separate from gift.observed and from the rule engine.
const wxGiftSeen = new Map(), wxGiftRevisions = new Set(), wxGiftProgress=new Map();
const wxGiftNickname = sender => {
  const nickname=wxOwnValue(wxOwnValue(sender,'contact'),'nickname');
  if (typeof nickname!=='string') return null;
  const clean=sdk.clean(nickname,128).replace(/[\u0080-\u009f]/g,'');
  // Native anonymous contacts use a masked nickname; their public number is
  // carried by the type-10 badge, not by the user ID or display_nickname.
  if (clean.trim()==='***') {
    const badges=wxOwnValue(sender,'badge_infos');
    if (Array.isArray(badges)) for (const badge of badges.slice(0,32)) {
      if (wxOwnValue(badge,'badge_type')!==10) continue;
      const name=wxOwnValue(badge,'badge_name');
      const match=typeof name==='string' && name.length<=64 && /^神秘观众[ \t]*([0-9]{1,20})$/.exec(name);
      if (match && match[0]===name) return '神秘观众'+match[1];
    }
  }
  return clean;
};
const wxGiftCandidate = (channel, entry, value) => {
  if (channel !== 'native_bridge' || !wxCollecting || sdk.getStatus().stopped) return;
  const gift = wxOwnValue(value,'reward_gift');
  if (!gift || typeof gift !== 'object' || Array.isArray(gift)) return;
  const id=wxOwnValue(entry,'client_msg_id'), contact=wxOwnValue(wxOwnValue(entry,'from_user_contact'),'contact');
  const user=wxOwnValue(contact,'username'), giftId=wxOwnValue(value,'reward_product_id'), name=wxOwnValue(gift,'name');
  const count=wxOwnValue(value,'reward_product_count'), combo=wxOwnValue(value,'combo_product_count');
  const validCount=v=>Number.isSafeInteger(v)&&v>=0&&v<=1000000;
  if (!wxIdentifier(id)||!wxIdentifier(user,256)||!wxIdentifier(giftId)||typeof name!=='string'||name.length>1024
      ||!validCount(count)||(combo!==undefined&&!validCount(combo))) { wxStatus.gift_skipped++;return; }
  const cleanName=sdk.clean(name,64).replace(/[\u0080-\u009f]/g,'');
  if (!cleanName.trim()) { wxStatus.gift_skipped++;return; }
  const target=wxOwnValue(wxOwnValue(wxOwnValue(entry,'to_user_contact'),'contact'),'username');
  const recipient=wxIdentifier(target,256)?target:null;
  const signals=['is_floatmsg','float_type'].map(key=>{const n=wxOwnValue(entry,key);return Number.isInteger(n)&&n>=0&&n<=65535?n:null;});
  const key=JSON.stringify([user,id]), fingerprint=JSON.stringify([giftId,name,count,combo??null,recipient,...signals]);
  const previous=wxGiftSeen.get(key), progress=wxGiftProgress.get(key);
  if (previous!==undefined) {
    if (previous===fingerprint) {wxStatus.gift_duplicates++;return;}
    wxStatus.gift_conflicts++;
  }
  if (previous===undefined && count===0) {wxStatus.gift_skipped++;return;}
  const revisionKey=JSON.stringify([key,fingerprint]);
  if (previous!==undefined && wxGiftRevisions.has(revisionKey)) return;

  const identity={native_event_id:id,user_id:user}, nickname=wxGiftNickname(wxOwnValue(entry,'from_user_contact'));
  if (nickname!==null) identity.nickname=nickname;
  const payload={gift_id:giftId,gift_name:cleanName,gift_name_truncated:Array.from(name).length>64,
    reward_product_count:count,quantity_mode:'unknown',message_type:20009,
    decoder:'wechat.app20009.reward.candidate.v1',confidence:'candidate',live_eligible:false,room_verified:false};
  const price=wxOwnValue(gift,'price');
  if(Number.isSafeInteger(price)&&price>=0&&price<=1000000000)payload.unit_value=price;
  if (combo!==undefined) payload.combo_product_count=combo;
  const comboId=wxOwnValue(value,'combo_id');
  if(wxIdentifier(comboId)&&comboId!=='0') payload.combo_id=comboId;
  if (recipient!==null) payload.recipient_user_id=recipient;
  // Observed native display hints; do not infer a final gift or count delta.
  for (const [i,key] of ['is_floatmsg','float_type'].entries()) if (signals[i]!==null) payload[key]=signals[i];
  if (previous!==undefined) {
    const original=JSON.parse(previous);
    payload.previous_gift_id=original[0];
    payload.previous_reward_product_count=progress?.count??original[2];
    if ((progress?.combo??original[3])!==null) payload.previous_combo_product_count=progress?.combo??original[3];
    if (original[4]!==null) payload.previous_recipient_user_id=original[4];
    if (original[5]!==null) payload.previous_is_floatmsg=original[5];
    if (original[6]!==null) payload.previous_float_type=original[6];
    payload.reason='same_message_changed';
  }
  if (!sdk.emit(previous===undefined?'gift.candidate':'gift.candidate_revision','network',payload,identity)) {wxStatus.gift_dropped++;return;}
  wxGiftProgress.set(key,{count:Math.max(progress?.count??0,count),combo:Math.max(progress?.combo??0,combo??0)});
  if (previous===undefined) {
    wxStatus.gift_candidates++;wxGiftSeen.set(key,fingerprint);
    if (wxGiftSeen.size>2048) {const first=wxGiftSeen.keys().next().value;wxGiftSeen.delete(first);wxGiftProgress.delete(first);}
  } else {
    wxStatus.gift_revisions++;wxGiftRevisions.add(revisionKey);
    if (wxGiftRevisions.size>2048) wxGiftRevisions.delete(wxGiftRevisions.values().next().value);
  }
};
sdk.onDispose(()=>{wxGiftSeen.clear();wxGiftRevisions.clear();wxGiftProgress.clear();});
const wxChatCandidate = (channel, entry) => {
  if (channel !== 'native_bridge' || wxOwnValue(entry, 'type') !== 1) return;
  const id = wxOwnValue(entry, 'client_msg_id'), user = wxOwnValue(entry, 'username');
  const content = wxOwnValue(entry, 'content');
  if (!wxIdentifier(id) || !wxIdentifier(user,256) || typeof content !== 'string'
      || content.length > 8192 || wxJsonObjectText(content)) { wxStatus.candidate_skipped++; return; }
  const text = sdk.clean(content, 256);
  if (!text.trim()) { wxStatus.candidate_skipped++; return; }
  const key = JSON.stringify([user, id]), previous = wxChatSeen.get(key);
  if (previous !== undefined) {
    if (previous === content) wxStatus.candidate_duplicates++;
    else wxStatus.candidate_conflicts++;
    return;
  }
  if (Date.now() - wxChatWindow >= 60000) { wxChatWindow = Date.now(); wxChatQuota = 0; }
  // Candidate delivery has an independent budget: high-volume polling
  // diagnostics must not consume all slots needed for actual candidates.
  if (wxChatQuota >= 30 || sdk.getStatus().buffered >= 499) { wxStatus.candidate_dropped++; return; }
  const nickname = wxOwnValue(entry, 'nickname');
  const identity = { native_event_id: id, user_id: user };
  if (typeof nickname === 'string') identity.nickname = sdk.clean(nickname, 128).replace(/[\u0080-\u009f]/g, '');
  const accepted = sdk.emit('chat.candidate', 'network', {
    content: text, content_truncated: Array.from(content).length > 256,
    message_type: 1, decoder: 'wechat.msg_list.type1.candidate.v1',
    confidence: 'candidate', live_eligible: false, room_verified: false
  }, identity);
  if (!accepted) { wxStatus.candidate_dropped++; return; }
  wxChatQuota++; wxStatus.chat_candidates++; wxChatSeen.set(key, content);
  if (wxChatSeen.size > 2048) wxChatSeen.delete(wxChatSeen.keys().next().value);
};
sdk.onDispose(() => wxChatSeen.clear());
sdk.onDrain(() => wxSetCollecting(false));
// Arrival candidates require the native notice type, a stable contact and the
// exact arrival wording. Ordinary type=1 chat and suffix-only matches stay chat.
const wxArrivalSeen = new Set();
const wxArrivalCandidate = (channel, entry) => {
  if (channel !== 'native_bridge' || !wxCollecting || sdk.getStatus().stopped
      || wxOwnValue(entry, 'type') !== 10005) return;
  const id = wxOwnValue(entry, 'client_msg_id');
  const contact = wxOwnValue(wxOwnValue(entry, 'finder_live_contact'), 'contact');
  const contactUser = wxOwnValue(contact, 'username'), contactName = wxOwnValue(contact, 'nickname');
  const outerUser = wxOwnValue(entry, 'username'), outerName = wxOwnValue(entry, 'nickname');
  if (wxIdentifier(contactUser,256) && wxIdentifier(outerUser,256) && contactUser !== outerUser) return;
  const user = wxIdentifier(contactUser,256) ? contactUser : outerUser;
  const nickname = wxIdentifier(contactUser,256) ? contactName : outerName;
  if (!wxIdentifier(id) || !wxIdentifier(user,256) || typeof nickname !== 'string'
      || !nickname.trim() || Array.from(nickname).length > 128 || /[\u0000-\u001f\u007f-\u009f]/.test(nickname)) return;
  let content = wxOwnValue(entry, 'content');
  if (typeof content !== 'string' || content.length > 8192) return;
  const parsed = wxJsonObjectText(content) ? JSON.parse(content) : wxBase64Json(content);
  if (parsed) content = wxOwnValue(parsed, 'content');
  if (typeof content !== 'string' || /[\u0000-\u001f\u007f-\u009f]/.test(content)) return;
  const text = content.trim(), name = nickname.trim();
  if (text !== '来了' && text !== name + ' 来了' && text !== name + '来了') return;
  const key = JSON.stringify([user,id]);
  if (wxArrivalSeen.has(key) || sdk.getStatus().buffered >= 499) return;
  // Read the explicit native badge names on the same arriving contact.
  // A nickname containing these words never establishes an identity.
  const badges = wxOwnValue(wxOwnValue(entry,'finder_live_contact'),'badge_infos');
  let supporterTitle = null;
  if (Array.isArray(badges)) for (const badge of badges.slice(0,32)) {
    const title = wxOwnValue(badge,'badge_name');
    if (title === '星守护') { supporterTitle = 'star_guardian'; break; }
    if (title === '超粉') supporterTitle = 'super_fan';
  }
  if (sdk.emit('member.candidate','network',{
    message_type:10005, decoder:'wechat.msg_list.arrival.candidate.v1',
    arrival_text_match:true, ...(supporterTitle?{supporter_title:supporterTitle}:{}),
    observed_at_ms:Date.now(), confidence:'candidate',
    live_eligible:false, room_verified:false
  },{native_event_id:id,user_id:user,nickname:name})) {
    wxArrivalSeen.add(key);
    if (wxArrivalSeen.size > 2048) wxArrivalSeen.delete(wxArrivalSeen.values().next().value);
  }
};
sdk.onDispose(() => wxArrivalSeen.clear());
// Local visible-text correlation is supporting evidence only: a match may be
// another page label, and an unmatched entry may render later or in a canvas.
// Nothing here assigns chat/gift semantics or writes to the platform.
const wxMatchPending = [];
const wxMatchText = text => text.replace(/\s+/g, ' ').trim();
const wxQueueMatch = (type, content) => {
  if (typeof content !== 'string' || content.length > 1024 || wxMatchPending.length >= 32) return;
  const text = wxMatchText(content);
  if (text.length < 2 || wxJsonObjectText(text)) return;
  wxMatchPending.push({ type, text, deadline: Date.now() + 8000 });
};
const wxVisibleTexts = () => {
  const texts = []; let visited = 0, chars = 0, truncated = false;
  if (!document.body || typeof document.createTreeWalker !== 'function') return { texts, truncated: true };
  const walker = document.createTreeWalker(document.body, 4); // SHOW_TEXT
  const start = performance.now(); let node;
  while ((node = walker.nextNode())) {
    if (++visited > 400 || chars >= 32768 || performance.now() - start > 10) { truncated = true; break; }
    const parent = node.parentElement;
    if (!parent || parent.closest('#live-archive-moderation-panel,#live-archive-moderation-entry,input,textarea,[contenteditable],script,style')) continue;
    const rect = parent.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0 || rect.bottom < 0 || rect.right < 0
        || rect.top >= window.innerHeight || rect.left >= window.innerWidth) continue;
    const style = window.getComputedStyle(parent);
    if (style.visibility !== 'visible' || style.display === 'none' || style.opacity === '0') continue;
    const text = wxMatchText((node.nodeValue || '').slice(0, 2048));
    if (text) { texts.push(text); chars += text.length; }
  }
  return { texts, truncated };
};
const wxMatchTimer = setInterval(() => {
  if (!wxCollecting || sdk.getStatus().stopped || !wxMatchPending.length) return;
  try {
    const snapshot = wxVisibleTexts(), now = Date.now(), counts = new Map(); let checked = 0;
    for (let i = wxMatchPending.length - 1; i >= 0; i--) {
      const item = wxMatchPending[i], matched = snapshot.texts.some(text => text.includes(item.text));
      if (!matched && now < item.deadline) continue;
      wxMatchPending.splice(i, 1); checked++;
      if (!counts.has(item.type) && counts.size >= 8) continue;
      if (!counts.has(item.type)) counts.set(item.type, { type: item.type, matched: 0, unmatched: 0 });
      counts.get(item.type)[matched ? 'matched' : 'unmatched']++;
      if (matched) wxStatus.page_matches++;
    }
    if (checked) {
      const payload = { checked, window_ms: 8000, scan_truncated: snapshot.truncated, type_counts: [...counts.values()] };
      wxStatus.recent_page_match = payload;
      wxEmit('probe.message_match', 'diagnostic', payload); wxPublish();
    }
  } catch (_) {
    // Navigation can invalidate DOM access. Never retain expired text on error.
    for (let i = wxMatchPending.length - 1; i >= 0; i--) {
      if (wxMatchPending[i].deadline <= Date.now()) wxMatchPending.splice(i, 1);
    }
  }
}, 1000);
sdk.onDispose(() => { clearInterval(wxMatchTimer); wxMatchPending.length = 0; });
// Batch diagnostics, not interaction events. Polling can repeat entries; these
// counts do not represent unique comments/gifts. Never export content or IDs.
// Unknown message semantics stay diagnostic-only. Parse bounded content and
// export field types/booleans, never raw text, native IDs or nicknames.
const wxMessageContentSchemas = new Set();
let wxMessageContentWindow=Date.now(), wxMessageContentQuota=0;
const wxMessageContent = (channel, entry) => {
  if (channel!=='native_bridge' || !wxCollecting || sdk.getStatus().stopped) return;
  const type=wxOwnValue(entry,'type');
  // Prioritize the unexplained type actually observed in desktop sessions.
  // Other types keep existing batch diagnostics and cannot exhaust this budget.
  if (type!==10005) return;
  const text=wxOwnValue(entry,'content');
  let format='absent', value=null, arrival=false;
  const arrivalHint=text=>typeof text==='string' && Array.from(text).length<=256
    && !/[\u0000-\u001f\u007f-\u009f]/.test(text) && /(?:来了|进入直播间)$/.test(text.trim());
  if (text!==undefined && text!==null) {
    format='opaque';
    if (typeof text==='string' && text.length<=8192) {
      if (wxJsonObjectText(text)) {format='json';value=JSON.parse(text);}
      else {value=wxBase64Json(text);format=value?'base64_json':'plain';}
      const localText=value?wxOwnValue(value,'content'):text;
      arrival=arrivalHint(localText);
      if (value && typeof localText==='string' && localText.length<=8192 && !wxJsonObjectText(localText)) wxQueueMatch(type,localText);
    }
  }
  const contact=wxOwnValue(wxOwnValue(entry,'finder_live_contact'),'contact');
  const payload={message_type:type,format,fields:value?wxDetailFields(value).slice(0,8):[],envelope_fields:wxDetailFields(entry).slice(0,6),
    content_empty:typeof text==='string' && !text.trim(),
    contact_sender_id_present:wxIdentifier(wxOwnValue(contact,'username'),256),
    contact_nickname_present:typeof wxOwnValue(contact,'nickname')==='string' && !!sdk.clean(wxOwnValue(contact,'nickname'),128).trim(),
    sender_id_present:wxIdentifier(wxOwnValue(entry,'username'),256),message_id_present:wxIdentifier(wxOwnValue(entry,'client_msg_id')),
    nickname_present:typeof wxOwnValue(entry,'nickname')==='string' && !!sdk.clean(wxOwnValue(entry,'nickname'),128).trim(),
    arrival_text_hint:arrival,confidence:'unknown',live_eligible:false,room_verified:false};
  wxStatus.recent_message_content=payload;
  const signature=JSON.stringify(payload);
  if (!wxMessageContentSchemas.has(signature)&&wxMessageContentSchemas.size<32) {
    if (Date.now()-wxMessageContentWindow>=60000) {wxMessageContentWindow=Date.now();wxMessageContentQuota=0;}
    if (wxMessageContentQuota<30 && sdk.getStatus().buffered<499 && sdk.emit('probe.message_content','diagnostic',payload)) {
      wxMessageContentSchemas.add(signature);wxStatus.message_content_schemas++;wxMessageContentQuota++;
    }
  }
  wxPublish();
};
sdk.onDispose(()=>wxMessageContentSchemas.clear());
// Only a recognized server polling callback may contribute echo evidence.
// This walk also runs inside the native dispatch wrapper, whose depth guard
// deliberately suppresses a second semantic decode of the same callback.
const wxPollSucceeded=base=>{
  if(!base||typeof base!=='object')return false;
  const lower=wxOwnValue(base,'ret'),upper=wxOwnValue(base,'Ret');
  // Do not accept contradictory status spellings, strings, or missing status.
  return (lower!==undefined||upper!==undefined)&&(lower===undefined||lower===0)&&(upper===undefined||upper===0);
};
// A first successful poll establishes a watermark, never welcomes historical chat.
let wxKeywordFloor=null, wxKeywordWindow=0, wxKeywordQuota=0, wxActivityQuota=0;
const wxMessageSequence=value=>{
  if(typeof value==='number')return Number.isSafeInteger(value)&&value>0?BigInt(value):null;
  return typeof value==='string'&&/^[1-9][0-9]{0,19}$/.test(value)?BigInt(value):null;
};
const wxActivitySeen=new Set();
const wxPublicActivity=(id,type)=>{
  if(!wxCollecting||BOOT.timed_messages?.count_scope!=='public_screen'||!BOOT.timed_messages?.enabled||!BOOT.timed_messages.min_comments)return;
  const key='public:'+id;
  if(wxActivitySeen.has(key)||sdk.getStatus().buffered>=499)return;
  if(sdk.emit('public.activity','network',{decoder:'wechat.msg_list.public.v1',observed_at_ms:Date.now(),message_type:type},{native_event_id:id})){
    wxActivitySeen.add(key);if(wxActivitySeen.size>16384)wxActivitySeen.delete(wxActivitySeen.values().next().value);
  }
};
const wxKeywordSeen=new Set();
const wxObserveKeywords=messages=>{
  const activity=BOOT.timed_messages?.enabled&&BOOT.timed_messages.min_comments>0;
  if((!BOOT.keyword_replies?.enabled&&!activity)||!wxCollecting||sdk.getStatus().stopped)return;
  const floor=wxKeywordFloor;
  let highest=floor;
  for(const entry of messages.slice(0,500)){
    const seq=wxMessageSequence(wxOwnValue(entry,'seq'));
    if(seq!==null&&(highest===null||seq>highest))highest=seq;
  }
  wxKeywordFloor=highest;
  if(floor===null)return;
  if(Date.now()-wxKeywordWindow>=60000){wxKeywordWindow=Date.now();wxKeywordQuota=0;wxActivityQuota=0;}
  for(const entry of messages.slice(0,500)){
    const seq=wxMessageSequence(wxOwnValue(entry,'seq'));
    if(seq===null||seq<=floor)continue;
    if(activity&&BOOT.timed_messages?.count_scope==='public_screen'){
      // msg_list is the native public message list. Count each new list entry
      // regardless of type/content/author; the first poll is a baseline.
      const rawId=wxOwnValue(entry,'client_msg_id');
      const publicId=wxIdentifier(rawId)?rawId:'msg:'+seq.toString();
      wxPublicActivity(publicId,wxOwnValue(entry,'type'));
    }
    if(wxOwnValue(entry,'type')!==1)continue;
    const id=wxOwnValue(entry,'client_msg_id'),user=wxOwnValue(entry,'username'),text=wxOwnValue(entry,'content');
    if(!wxIdentifier(id)||!wxIdentifier(user,256)||typeof text!=='string'||!text.trim()||Array.from(text).length>256||/[\u0000-\u001f\u007f-\u009f]/.test(text)||wxJsonObjectText(text))continue;
    const activityKey=JSON.stringify([user,id]);
    if(activity&&BOOT.timed_messages?.count_scope!=='public_screen'&&!wxActivitySeen.has(activityKey)&&wxActivityQuota<600&&sdk.getStatus().buffered<499){
      if(sdk.emit('chat.activity','network',{content:text,content_truncated:false,message_type:1,
        decoder:'wechat.msg_list.activity.v1',keyword_live:true,observed_at_ms:Date.now(),
        confidence:'candidate',live_eligible:false,room_verified:false},{native_event_id:id,user_id:user})){
        wxActivitySeen.add(activityKey);wxActivityQuota++;
        if(wxActivitySeen.size>2048)wxActivitySeen.delete(wxActivitySeen.values().next().value);
      }
    }
    const lower=text.toLowerCase();
    if(!BOOT.keyword_replies?.enabled||!BOOT.keyword_replies.rules.some(r=>r.keywords.some(k=>r.match_mode==='exact'?lower===k.toLowerCase():lower.includes(k.toLowerCase()))))continue;
    const key=JSON.stringify([user,id]);
    if(wxKeywordSeen.has(key)||wxKeywordQuota>=60||sdk.getStatus().buffered>=499)continue;
    if(sdk.emit('chat.keyword_candidate','network',{content:text,content_truncated:false,message_type:1,
      decoder:'wechat.msg_list.keyword.v1',keyword_live:true,observed_at_ms:Date.now(),
      confidence:'candidate',live_eligible:false,room_verified:false},{native_event_id:id,user_id:user})){
      wxKeywordSeen.add(key);wxKeywordQuota++;
      if(wxKeywordSeen.size>2048)wxKeywordSeen.delete(wxKeywordSeen.values().next().value);
    }
  }
};
sdk.onDispose(()=>{wxKeywordSeen.clear();wxActivitySeen.clear();});
const wxObservePollChats=(values,started,knownPoll)=>{
  if(!BOOT.automatic_send)return;
  let visited=0,decoded=0;
  const walk=(value,depth=0,poll=knownPoll)=>{
    if(depth>9||++visited>1500)return;
    if(typeof value==='string'&&value.length<=1048576&&decoded<4&&/^\s*[\[{]/.test(value)){decoded++;try{walk(JSON.parse(value),depth+1,poll);}catch(_){}return;}
    if(!value||typeof value!=='object')return;
    // Recorded desktop polling responses use this envelope even when the
    // native request name is generic. A bare post-comment callback is insufficient.
    const response=wxOwnValue(wxOwnValue(value,'jsapi_resp'),'resp_json');
    if(typeof response==='string'&&response.length<=1048576){try{const body=JSON.parse(response);if(body&&wxPollSucceeded(wxOwnValue(body,'baseResponse'))&&Array.isArray(wxOwnValue(body,'app_msg_list'))&&Array.isArray(wxOwnValue(body,'msg_list')))walk(body,depth+1,true);}catch(_){}}
    const baseResponse=wxOwnValue(value,'baseResponse');
    if(baseResponse&&!wxPollSucceeded(baseResponse))return;
    const messages=poll?wxOwnValue(value,'msg_list'):null;
    if(Array.isArray(messages)&&wxPollSucceeded(baseResponse)) wxObserveKeywords(messages);

    for(const key of Object.keys(value).slice(0,30))walk(wxOwnValue(value,key),depth+1,poll);
  };try{walk(values);}catch(_){}
};
const wxMessageBatch = (channel, category, entries) => {
  if(entries.length>500) sdk.emit('probe.gap','diagnostic',{reason:'chat_batch_limit'});
  const counts = new Map(); let sampled = 0, overflow = 0, contentJson = 0;
  for (let i = 0; i < Math.min(entries.length, 500); i++) {
    const entry = wxOwnValue(entries, String(i));
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) continue;
    sampled++;
    wxChatCandidate(channel, entry);
    wxMessageContent(channel, entry);
    wxArrivalCandidate(channel, entry);
    const rawType = wxOwnValue(entry, 'type');
    const type = Number.isInteger(rawType) && rawType >= 0 && rawType <= 65535 ? rawType : null;
    if (!counts.has(type) && counts.size >= 8) { overflow++; continue; }
    if (!counts.has(type)) counts.set(type, { type, count: 0, text: 0, json: 0, opaque: 0, absent: 0 });
    const count = counts.get(type); count.count++;
    const content = wxOwnValue(entry, 'content');
    if (content === undefined || content === null) count.absent++;
    else if (typeof content !== 'string' || content.length > 8192) count.opaque++;
    else {
      const json = wxJsonObjectText(content);
      if (json && contentJson++ < 8) wxGiftWalk(channel, JSON.parse(content), 'message_content', type);
      count[json ? 'json' : 'text']++;
      if (!json) wxQueueMatch(type, content);
    }
  }
  const payload = { channel, category, total: Math.min(entries.length, 10000), sampled,
    truncated: entries.length > 500, overflow, type_counts: [...counts.values()] };
  wxStatus.message_batches++; wxStatus.sampled_entries += sampled;
  wxStatus.recent_message_batch = payload;
  wxEmit('probe.message_batch', 'diagnostic', payload); wxPublish();
};
const wxSchemas = new Set();
const wxShape = (channel, value, category = 'response', apiKind = 'other') => {
  if (!wxCollecting || sdk.getStatus().stopped) return;
  const fields = new Set(), messageLists = new WeakSet(), applications = new WeakSet();
  let visits = 0, decoded = 0, parsedBytes = 0, applicationEntries = 0, applicationScanned = 0;
  const application = object => {
    if (!object || typeof object !== 'object' || applications.has(object) || applicationEntries >= 500) return;
    if ((wxOwnValue(object, 'msg_type') ?? wxOwnValue(object, 'msgType')) !== 20009 && (wxOwnValue(object, 'msg_type') ?? wxOwnValue(object, 'msgType')) !== 20013) return;
    applications.add(object); applicationEntries++; wxApplication(channel, object);
  };
  const add = (path, type) => { if (fields.size < 24 && path.length <= 64) fields.add(path + ':' + type); };
  const visit = (v, path, depth) => {
    if (depth > 7 || ++visits > 200) return;
    const type = v === null ? 'null' : Array.isArray(v) ? 'array' : typeof v;
    add(path, type);
    if (type === 'string') {
      // Native envelopes frequently put JSON inside an otherwise opaque string.
      // Parse only bounded object/array JSON, never base64 or executable text.
      if (decoded < 3 && v.length <= 1048576 && parsedBytes + v.length <= 1048576 && /^\s*[\[{]/.test(v)) {
        decoded++; parsedBytes += v.length;
        try { visit(JSON.parse(v), path + '<json>', depth); } catch (_) {}
      }
      return;
    }
    if (!v || type !== 'object' && type !== 'array') return;
    if (type === 'object') { wxGiftSchema(channel, v, 'envelope'); wxCommand(channel, v); application(v); }
    if (Array.isArray(v)) {
      for (let i = 0; i < Math.min(v.length, 3); i++) {
        const descriptor = Object.getOwnPropertyDescriptor(v, String(i));
        const child = path + (path === '$.args' ? '[' + i + ']' : '[]');
        if (descriptor && 'value' in descriptor) visit(descriptor.value, child, path === '$.args' ? 0 : depth + 1);
        else if (descriptor) add(child, 'accessor');
      }
      return;
    }
    let keys = 0;
    for (const key in v) {
      if (!Object.prototype.hasOwnProperty.call(v, key)) continue;
      if (++keys > 50) break;
      const descriptor = Object.getOwnPropertyDescriptor(v, key), child = path + '.' + wxKey(key, keys);
      if (descriptor && 'value' in descriptor) {
        if (key === 'msg_list' && Array.isArray(descriptor.value) && !messageLists.has(descriptor.value)) {
          messageLists.add(descriptor.value); wxMessageBatch(channel, category, descriptor.value);
        }
        // Generic structural arrays sample just three entries. Explicit app
        // batches must also inspect later messages, with a shared 50-item cap.
        if ((key === 'app_msg_list' || key === 'appMsgList') && Array.isArray(descriptor.value)) {
          if(descriptor.value.length>500) sdk.emit('probe.gap','diagnostic',{reason:'application_batch_limit'});
          for (let i = 0; i < Math.min(descriptor.value.length, 500) && applicationScanned < 500; i++) {
            applicationScanned++; application(wxOwnValue(descriptor.value, String(i)));
          }
        }
        visit(descriptor.value, child, depth + 1);
      }
      else if (descriptor) add(child, 'accessor');
    }
  };
  try { visit(value, '$', 0); } catch (_) { add('$', 'unreadable'); }
  const list = [...fields].sort(), signature = channel + ':' + category + ':' + apiKind + ':' + list.join(',');
  if (!list.length || wxSchemas.has(signature)) return;
  if (wxSchemas.size >= 100) { wxStatus.dropped++; wxPublish(); return; }
  wxSchemas.add(signature); wxStatus.schemas++;
  wxStatus.recent_schemas = [...wxStatus.recent_schemas.slice(-9), { channel, category, api_kind: apiKind, fields: list }];
  wxEmit('probe.schema', 'diagnostic', { channel, category, api_kind: apiKind, fields: list }); wxPublish();
};
const wxText = (channel, value) => {

  if (!wxCollecting || sdk.getStatus().stopped || typeof value !== 'string' || value.length > 65536) return;
  wxShape(channel, value);
};

let wxFetch = window.fetch;
const wxReaders = new Set(), wxResponses = new WeakSet();
const wxWrapFetch = base => function (...args) {
  const result = Reflect.apply(base, this, args);
  if (sdk.getStatus().stopped) return result;
  if (wxRelevant(args[0])) result.then(async response => {
    if (!wxCollecting || sdk.getStatus().stopped) return;
    if (wxResponses.has(response)) return;
    sdk.captureResponse?.('fetch',response);
    wxResponses.add(response);
    const json = (response.headers.get('content-type') || '').includes('json');
    wxNetwork('fetch', json ? 'json' : 'unknown');
    if (!json || wxReaders.size >= 4) return;
    const reader = response.clone().body?.getReader();
    if (!reader) return;
    wxReaders.add(reader);
    const timer = setTimeout(() => { void reader.cancel().catch(() => {}); }, 2000);
    const chunks = []; let size = 0;
    try {
      while (true) {
        const { done, value } = await reader.read(); if (done) break;
        size += value.byteLength; if (size > 65536 || !wxCollecting || sdk.getStatus().stopped) return;
        chunks.push(value);
      }
      const bytes = new Uint8Array(size); let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
      wxText('fetch', new TextDecoder().decode(bytes));
    } finally { clearTimeout(timer); wxReaders.delete(reader); void reader.cancel().catch(() => {}); }
  }).catch(() => {});
  return result;
};
let wxFetchHook = wxWrapFetch(wxFetch);
window.fetch = wxFetchHook;
sdk.onDispose(() => {
  if (window.fetch === wxFetchHook) window.fetch = wxFetch;
  for (const reader of wxReaders) void reader.cancel().catch(() => {});
  wxReaders.clear();
});

let wxOpen = XMLHttpRequest.prototype.open;
const wxXhrs = new Map();
const wxWrapOpen = base => function (...args) {
  if (sdk.getStatus().stopped) return Reflect.apply(base, this, args);
  wxXhrs.get(this)?.();
  const result = Reflect.apply(base, this, args);
  if (wxRelevant(args[1]) && !wxXhrs.has(this) && wxXhrs.size < 100) {
    const xhr = this;
    const release = () => { xhr.removeEventListener('load', load); xhr.removeEventListener('loadend', release); wxXhrs.delete(xhr); };
    const load = () => {
      try {
        wxNetwork('xhr', xhr.responseType === 'json' ? 'json' : 'text');
        if (xhr.responseType === 'json') {
          if (wxCollecting) sdk.captureRaw?.('xhr','response',xhr.response);
          wxShape('xhr', xhr.response);
        } else if (!xhr.responseType || xhr.responseType === 'text') {
          if (wxCollecting) sdk.captureRaw?.('xhr','response',xhr.responseText);
          wxText('xhr', xhr.responseText);
        }
      } catch (_) {} finally { release(); }
    };
    xhr.addEventListener('load', load, { once: true }); xhr.addEventListener('loadend', release, { once: true });
    wxXhrs.set(xhr, release);
  }
  return result;
};
let wxOpenHook = wxWrapOpen(wxOpen);
XMLHttpRequest.prototype.open = wxOpenHook;
sdk.onDispose(() => {
  if (XMLHttpRequest.prototype.open === wxOpenHook) XMLHttpRequest.prototype.open = wxOpen;
  for (const release of [...wxXhrs.values()]) release();
});

let wxSocket = window.WebSocket;
const wxSockets = new Map();
const wxWrapSocket = base => new Proxy(base, {
  construct(target, args, newTarget) {
    const socket = Reflect.construct(target, args, newTarget);
    if (sdk.getStatus().stopped) return socket;
    if (wxRelevant(args[0]) && !wxSockets.has(socket) && wxSockets.size < 100) {
      const message = event => {
        wxNetwork('websocket', typeof event.data === 'string' ? 'text' : 'binary',
          typeof event.data === 'string' ? event.data.length : event.data?.byteLength || event.data?.size || 0);
        if (wxCollecting) sdk.captureRaw?.('websocket','message',event.data);
        wxText('websocket', event.data);
      };
      const close = () => { socket.removeEventListener('message', message); socket.removeEventListener('close', close); wxSockets.delete(socket); };
      socket.addEventListener('message', message); socket.addEventListener('close', close, { once: true });
      wxSockets.set(socket, close);
    }
    return socket;
  }
});
let wxSocketHook = wxWrapSocket(wxSocket);
window.WebSocket = wxSocketHook;
sdk.onDispose(() => {
  if (window.WebSocket === wxSocketHook) window.WebSocket = wxSocket;
  for (const close of [...wxSockets.values()]) close();
});

// Reattach once when the user starts/resumes collection, using the page's
// current implementations. Do not continually overwrite the page's hooks.
// Its wrappers may still delegate to older probe hooks: the response/socket
// guards and per-XHR listener map keep those chains from double counting.
const wxRepairHooks = () => {
  if (window.fetch !== wxFetchHook && typeof window.fetch === 'function') {
    wxFetch = window.fetch; wxFetchHook = wxWrapFetch(wxFetch); window.fetch = wxFetchHook;
  }
  if (XMLHttpRequest.prototype.open !== wxOpenHook && typeof XMLHttpRequest.prototype.open === 'function') {
    wxOpen = XMLHttpRequest.prototype.open; wxOpenHook = wxWrapOpen(wxOpen); XMLHttpRequest.prototype.open = wxOpenHook;
  }
  if (window.WebSocket !== wxSocketHook && typeof window.WebSocket === 'function') {
    wxSocket = window.WebSocket; wxSocketHook = wxWrapSocket(wxSocket); window.WebSocket = wxSocketHook;
  }
  wxStatus.hook_replaced = window.fetch !== wxFetchHook || window.WebSocket !== wxSocketHook
    || XMLHttpRequest.prototype.open !== wxOpenHook;
  wxAttachNative(true);
};

// Passive native bridge observation. Never invoke an API or register a native
// subscription ourselves. Only wrap existing writable function properties.
const wxNativeSlots = [], wxNativeKeys = ['invoke', 'call', 'off', 'on', '_handleMessageFromWeixin', '_handleMessageFromWeixinJSON'];
const wxOnWrappers = new WeakMap(); let wxOnWrapperCount = 0;
let wxNativeDepth = 0;
// Labels are diagnostic hints, never decoded interaction types. Request names,
// URL paths and values are inspected locally but never copied into diagnostics.
const wxApiKind = args => {
  const names = typeof args[0] === 'string' ? [args[0]] : [];
  const params = args[1];
  if (params && typeof params === 'object') {
    for (const key of ['apiName', 'method', 'cgiName', 'cgiUrl', 'cgi']) {
      try {
        const descriptor = Object.getOwnPropertyDescriptor(params, key);
        if (descriptor && typeof descriptor.value === 'string') names.push(descriptor.value);
      } catch (_) {}
    }
  }
  const name = names.map(v => v.slice(0, 256).split('?')[0].toLowerCase()).join(' ');
  if (/gift/.test(name)) return 'gift';
  if (/comment|message|msg/.test(name)) return 'message';
  if (/joinlive|enterroom/.test(name)) return 'room_join';
  if (/liveinfo|liveprofile|room/.test(name)) return 'room';
  if (/video|audio|player|stream/.test(name)) return 'media';
  if (/cgi/.test(name)) return 'cgi';
  return 'other';
};
// Auto-binding can happen before the initial JoinLive response arrives.
const createWechatRoomBinding=(readIdentity,onChange,autoBinding=false)=>{
  let identity=readIdentity(),initialJoin=true;
  const profile=()=>{const next=readIdentity();if(identity&&next&&identity!==next){onChange();return;}if(next)identity=next;};
  return {profile,join:()=>{
    const next=readIdentity();
    if(initialJoin&&(identity===null||(autoBinding&&!identity))){initialJoin=false;if(next)identity=next;return;}
    initialJoin=false;
    if(identity&&next&&identity===next)return;
    if(identity===null&&next){identity=next;return;}
    onChange();
  }};
};
const wxReadLiveIdentity=()=>{
  const check=window.WXU?.check_live_existing;if(typeof check!=='function')return undefined;
  try{const [error,p]=check({silence:true});if(error)return null;return p?.contact?.username||p?.username||p?.id||null;}catch(_){return null;}
};
const wxStopTaskForRoomChange = () => {
  if (!BOOT.task_id || sdk.getStatus().stopped) return;
  wxSetCollecting(false);
  sdk.emit('probe.status','diagnostic',{stage:'task_room_changed'});
  sdk.dispose();
  if(document.body){const note=document.createElement('div');note.textContent='直播间已切换，场控已停止。请刷新直播页面以重新连接。';Object.assign(note.style,{position:'fixed',top:'16px',right:'16px',zIndex:'2147483647',background:'#fff',color:'#111',padding:'16px'});document.body.appendChild(note);}
};
const wxNativeObserve = (values, category, apiKind = 'other') => {
  if (wxNativeDepth || !wxCollecting || sdk.getStatus().stopped) return;
  sdk.captureRaw?.('native_bridge',category,values);
  const value = values[0], text = typeof value === 'string';
  wxNetwork('native_bridge', text ? 'text' : 'json', text ? value.length : 0, 'diagnostic');
  wxShape('native_bridge', { args: values.slice(0, 3) }, category, apiKind);
};
const wxAttachNative = (repair = false) => {
  if (sdk.getStatus().stopped) return;
  const bridge = window.WeixinJSBridge;
  wxStatus.native_bridge = !!bridge;
  if (!bridge || (typeof bridge !== 'object' && typeof bridge !== 'function')) return;
  for (const key of wxNativeKeys) {
    try {
      if (key === 'on' && 'off' in bridge) {
        const off = Object.getOwnPropertyDescriptor(bridge, 'off');
        if (!off || typeof off.value !== 'function' || !off.writable) continue;
        const removal = wxNativeSlots.find(slot => slot.bridge === bridge && slot.key === 'off');
        if (!removal || bridge.off !== removal.hook) continue;
      }
      const previous = wxNativeSlots.find(slot => slot.bridge === bridge && slot.key === key);
      if (previous && (!repair || bridge[key] === previous.hook)) continue;
      if (!previous && wxNativeSlots.length >= 16) continue;
      const descriptor = Object.getOwnPropertyDescriptor(bridge, key);
      if (!descriptor || typeof descriptor.value !== 'function' || !descriptor.writable) continue;
      const base = descriptor.value, incoming = key.startsWith('_handleMessage');
      const hook = function (...args) {
        // Translate page-owned unsubscription identities even after disposal.
        // No subscriptions or native calls are made by the probe itself.
        if (key === 'off') {
          const translated = [...args];
          const record = typeof args[1] === 'function' && wxOnWrappers.get(args[1])?.find(r => r.bridge === bridge && r.name === args[0]);
          if (record) translated[1] = record.hook;
          return Reflect.apply(base, this, translated);
        }
        if (sdk.getStatus().stopped && key !== 'on') return Reflect.apply(base, this, args);
        if (incoming) {
          // Native dispatch may synchronously call an invoke callback. Count
          // this delivery once, while preserving callback ordering and this.
          try { wxNativeObserve(args, key === '_handleMessageFromWeixinJSON' ? 'dispatch_json' : 'dispatch'); } catch (_) {}
          wxNativeDepth++;
          try { return Reflect.apply(base, this, args); } finally { wxNativeDepth--; }
        }
        const requestStarted=performance.now();
        const apiKind = wxApiKind(args);
        const requestNames=[typeof args[0]==='string'?args[0]:'',...['cgi','cgiName','cgiUrl','apiName','method'].map(k=>wxOwnValue(args[1],k)).filter(v=>typeof v==='string')].join(' ').toLowerCase();
        const isMessagePoll=/(?:get[^ ]*(?:msg|message)|(?:msg|message)[^ ]*get)/.test(requestNames);
        if (BOOT.task_id && apiKind==='room_join' && key!=='on') {wxStopTaskForRoomChange();return Reflect.apply(base,this,args);}
        const observed = args.map((arg, index) => {
          if (typeof arg !== 'function' || key === 'on' && index !== 1) return arg;
          let records;
          if (key === 'on') {
            records = wxOnWrappers.get(arg) || [];
            const existing = records.find(r => r.bridge === bridge && r.name === args[0]);
            if (existing) return existing.hook;
            if (wxOnWrapperCount >= 1000) return arg;
          }
          const callback = function (...values) {
            if (apiKind==='room_join') wxStopTaskForRoomChange();
            if(key!=='on')wxObservePollChats(values,requestStarted,isMessagePoll);
            try { wxNativeObserve(values, key === 'on' ? 'on_callback' : key === 'invoke' ? 'invoke_callback' : 'call_callback', apiKind); } catch (_) {}
            wxNativeDepth++;
            try { return Reflect.apply(arg, this, values); } finally { wxNativeDepth--; }
          };
          if (records) { records.push({ bridge, name: args[0], hook: callback }); wxOnWrappers.set(arg, records); wxOnWrapperCount++; }
          return callback;
        });
        return Reflect.apply(base, this, observed);
      };
      Object.defineProperty(bridge, key, { ...descriptor, value: hook });
      const slot = { bridge, key, descriptor, hook };
      if (previous) wxNativeSlots[wxNativeSlots.indexOf(previous)] = slot;
      else wxNativeSlots.push(slot);
    } catch (_) { /* Frozen/native properties remain untouched. */ }
  }
  wxStatus.native_methods = wxNativeSlots.filter(slot => slot.bridge === bridge && bridge[slot.key] === slot.hook).map(slot => slot.key).sort();
  wxStatus.native_hooks = wxStatus.native_methods.length;
};
sdk.onDispose(() => {
  for (const slot of wxNativeSlots) {
    // Existing native listeners retain wrapped functions. Keep the bounded
    // on/off identity shims, with observation disabled, so registrations and
    // removals still use a consistent callback identity after disposal.
    if (wxOnWrapperCount && (slot.key === 'on' || slot.key === 'off')) continue;
    try { if (slot.bridge[slot.key] === slot.hook) Object.defineProperty(slot.bridge, slot.key, slot.descriptor); } catch (_) {}
  }
  wxNativeSlots.length = 0;
});

// Recording's existing WXE bus is optional; no recording controls are injected here.
const wxRoomBinding=createWechatRoomBinding(wxReadLiveIdentity,wxStopTaskForRoomChange,BOOT.unified_ui===true);
let wxBus = null; const wxBusListeners = [];
const wxAttachBus = () => {
  wxStatus.hook_replaced = window.fetch !== wxFetchHook || window.WebSocket !== wxSocketHook || XMLHttpRequest.prototype.open !== wxOpenHook;
  wxAttachNative();
  wxPublish();
  if (wxBus || sdk.getStatus().stopped) return;
  const bus = window.WXE;
  if (!bus || typeof bus.on !== 'function' || typeof bus.off !== 'function') return;
  wxBus = bus;
  for (const name of ['channels:OnLiveProfileLoaded', 'channels:JoinLive']) {
    const handler = value => { try { if(name==='channels:JoinLive')wxRoomBinding.join();else wxRoomBinding.profile(); if (wxCollecting) sdk.captureRaw?.('event_bus','live_profile',value); wxShape('event_bus', value); } catch (_) {} };
    bus.on(name, handler); wxBusListeners.push([name, handler]);
  }
  wxStatus.event_bus = true; wxPublish();
};
wxAttachBus();
const wxBusTimer = setInterval(wxAttachBus, 1000);
sdk.onDispose(() => {
  clearInterval(wxBusTimer);
  for (const [name, handler] of wxBusListeners) wxBus.off(name, handler);
});
wxPublish();
sdk.emit('probe.status', 'diagnostic', { stage: 'wechat_desktop_probe_ready' });

// Standalone probe diagnostics only; the unified assistant owns the live-page UI.
sdk.onDispose(() => wxSetCollecting(false));
const wxMountEntry = () => {
  if (BOOT.unified_ui) return;
  if (sdk.getStatus().stopped || !document.body || document.getElementById('live-archive-moderation-entry')) return;
  const entry = document.createElement('button');
  entry.id = 'live-archive-moderation-entry'; entry.type = 'button'; entry.textContent = '场控诊断';
  Object.assign(entry.style, {position:'fixed',right:'20px',top:'200px',zIndex:'2147483647',
    background:'#07c160',color:'#fff',border:'0',borderRadius:'6px',padding:'8px 12px',cursor:'pointer',fontSize:'14px'});
  const panel = document.createElement('section'); panel.id = 'live-archive-moderation-panel'; panel.hidden = true;
  Object.assign(panel.style, {position:'fixed',right:'20px',top:'242px',zIndex:'2147483647',
    width:'300px',maxWidth:'calc(100vw - 40px)',maxHeight:'60vh',overflow:'auto',
    background:'#fff',color:'#1f2329',border:'1px solid #ddd',borderRadius:'8px',padding:'12px',
    fontSize:'13px',lineHeight:'1.6',boxShadow:'0 2px 12px rgba(0,0,0,.2)'});
  panel.setAttribute('aria-label', '视频号场控诊断');
  entry.setAttribute('aria-controls', panel.id); entry.setAttribute('aria-expanded', 'false');
  const title = document.createElement('strong'); title.textContent = '视频号场控诊断';
  const description = document.createElement('p'); description.textContent = BOOT.automatic_send?'自动发送礼物感谢已启用，请核对感谢目标。':BOOT.fill_preview?'自动填入感谢，由你手动发送。':'当前仅采集诊断，不发送消息。';
  if (BOOT.task_id) description.textContent+=' 当前任务：'+BOOT.task_id;
  const status = document.createElement('div'); status.id = 'live-archive-moderation-status';
  status.setAttribute('role', 'status');
  const toggle = document.createElement('button'); toggle.id = 'live-archive-moderation-toggle'; toggle.type = 'button';
  Object.assign(toggle.style, {marginTop:'8px',padding:'6px 10px',cursor:'pointer'});
  const review = document.createElement('button'); review.id = 'live-archive-moderation-review'; review.type = 'button';
  Object.assign(review.style,{marginTop:'8px',padding:'6px 10px',cursor:'pointer'});
  const previewInfo = document.createElement('div'); previewInfo.id = 'live-archive-moderation-preview-status';
  Object.assign(previewInfo.style,{whiteSpace:'pre-line',overflowWrap:'anywhere',marginTop:'8px'});
  const fields = document.createElement('pre'); fields.id = 'live-archive-moderation-fields';
  Object.assign(fields.style, {whiteSpace:'pre-wrap',overflowWrap:'anywhere',fontSize:'12px',marginBottom:'0'});
  for (const node of [title, description, toggle, review, previewInfo, status, fields]) panel.appendChild(node);
  document.body.appendChild(entry); document.body.appendChild(panel);
  let lastView = '';
  const render = () => {
    const state = sdk.getStatus(), info = state.decoder || wxStatus;
    const view = JSON.stringify([state.stopped, state.draining, state.delivery, state.bridge_error, state.connected, wxCollecting,
      info.network_messages, info.schemas, info.dropped, info.hook_replaced, info.native_hooks, info.native_methods, info.recent_schemas,
      info.message_batches, info.sampled_entries, info.recent_message_batch, info.page_matches, info.recent_page_match,
      info.chat_candidates, info.candidate_duplicates, info.candidate_conflicts, info.candidate_skipped, info.candidate_dropped,
      info.gift_candidates,info.gift_revisions,info.gift_duplicates,info.gift_conflicts,info.gift_skipped,info.gift_dropped,
      info.message_content_schemas, info.recent_message_content, info.command_responses, info.gift_schemas, info.application_payloads, info.recent_application, info.recent_command, info.recent_gift_schema,
      wxLocalReview, wxApplicationReview, state.candidate_preview, state.fill, state.keyword_replies]);
    if (view === lastView) return;
    lastView = view;
    const connection = state.stopped ? '连接已停止，请重新打开直播页面' : state.draining ? '正在停止，补交缓存' : state.bridge_error
      ? '连接异常，请检查本地场控服务' : state.connected ? '已连接' : '正在连接';
    status.textContent = connection + ' · ' + (wxCollecting ? '采集中' : '未采集 / 已暂停')
      + '\n通信消息：' + info.network_messages + ' · 结构样本：' + info.schemas + ' · 已忽略：' + info.dropped;
    status.textContent += '\n原生监听入口：' + (info.native_hooks || 0) + '（' + (info.native_methods || []).join('，') + '）';
    const raw=state.raw;
    if (raw?.enabled) status.textContent += '\n原始采集：'+raw.captured+' · 待入库分片：'+raw.buffered_chunks+' · 丢失：'+raw.dropped+(raw.error?' · 错误：'+raw.error:'');
    status.textContent += '\n消息批次：' + (info.message_batches || 0) + ' · 已检查条目：' + (info.sampled_entries || 0) + '（含重复）';
    status.textContent += '\n页面文字命中：' + (info.page_matches || 0) + '（仅对照证据）';
    const generated = state.delivery?.generated?.['chat.candidate'] || 0;
    status.textContent += '\n文字候选（生成）：' + generated + ' · 重复：' + (info.candidate_duplicates || 0)
      + ' · 冲突：' + (info.candidate_conflicts || 0) + '\n候选跳过：' + (info.candidate_skipped || 0)
      + ' · 候选限额：' + (info.candidate_dropped || 0) + '（不触发场控）';
    const confirmed = state.delivery?.acknowledged?.['chat.candidate'] || 0;
    status.textContent += '\n候选确认入库：' + confirmed + ' · 未确认：' + Math.max(0, generated - confirmed);
    const giftsGenerated=state.delivery?.generated?.['gift.candidate']||0, giftsConfirmed=state.delivery?.acknowledged?.['gift.candidate']||0;
    status.textContent += '\n礼物候选：'+giftsGenerated+' · 确认：'+giftsConfirmed+' · 未确认：'+Math.max(0,giftsGenerated-giftsConfirmed)
      +'\n修订观测：'+info.gift_revisions+'（明确续送会延后感谢）'
      +'\n礼物重复：'+info.gift_duplicates+' · 冲突：'+info.gift_conflicts+' · 跳过：'+info.gift_skipped+' · 限额：'+info.gift_dropped+'（数量语义未确认）';
    status.textContent += '\n命令响应：' + (info.command_responses || 0) + ' · 礼物结构：' + (info.gift_schemas || 0) + '（未确认事件）';
    status.textContent += '\n未知消息内容结构：'+(info.message_content_schemas||0)+'（不触发欢迎）';
    status.textContent += '\n20009 应用载荷：' + (info.application_payloads || 0) + '（待真实样本核对）';
    previewInfo.textContent="";
    const draftState=state.candidate_preview;
    if (draftState?.enabled) {
      previewInfo.textContent+='\n'+(BOOT.automatic_send?'礼物感谢（自动填入并发送）：':BOOT.fill_preview?'感谢草稿（自动填入，手动发送）：':'感谢预览（仅日志）：')+(draftState.stale?'状态已过期':draftState.paused?'已暂停':'已启用')
        +'\n配置目标（未验证）：'+sdk.clean(draftState.recipient_user_id,128)
        +'\n待感谢用户：'+draftState.pending_users+' · 已生成：'+(draftState.counters?.draft_generated||0);
      if (draftState.welcome_enabled) previewInfo.textContent+='\n合并欢迎：等待 '+(draftState.pending_welcome_users||0)+' 位'+((draftState.pending_welcome_users||0)>0?' · '+Math.ceil((draftState.welcome_remaining_ms||0)/1000)+'s 后合并':'');
      for (const pending of draftState.pending||[]) previewInfo.textContent+='\n'+sdk.clean(pending.nickname,128)+'：'
        +(Math.ceil(pending.remaining_ms/100)/10).toFixed(1)+'s · '+pending.candidate_messages+' 条';
      if(draftState.pending_truncated) previewInfo.textContent+='\n仅展示前 10 位待处理用户。';
      const reasons={target_missing:'目标缺失',target_mismatch:'其他目标',invalid_candidate:'候选无效',duplicate:'重复',
        revision_only:'仅修订',observation_paused_candidate:'暂停期间',dedupe_limit:'去重上限',user_limit:'用户上限',
        window_limit:'窗口上限',observation_paused:'暂停取消',observation_gap:'缺口取消',target_changed:'目标变更取消'};
      previewInfo.textContent+='\n过滤/取消：'+Object.entries(reasons).filter(([key])=>draftState.counters?.[key])
        .map(([key,label])=>label+' '+draftState.counters[key]).join('，');
      for (const record of draftState.recent||[]) if(record.kind==='gift_candidate_draft'||record.kind==='welcome_candidate_draft')
        previewInfo.textContent+='\n最近草稿：'+sdk.clean(record.text,180);
    } else previewInfo.textContent+='\n感谢预览：未启用（YAML candidate_preview）';
    if(state.keyword_replies?.enabled) previewInfo.textContent+='\n关键词回复：'+(state.keyword_replies.active?'监听中':'暂停')+' · 已生成 '+state.keyword_replies.generated+' 条 · 冷却 '+state.keyword_replies.cooldown_seconds+' 秒';
    if(state.fill) previewInfo.textContent+='\n输入框：'+(state.fill.input_ready?(BOOT.automatic_send?'可自动填入并发送':'可填入（发送需手动）'):state.fill.input_reason||'未就绪')+'\n填入结果：'+(state.fill.last_reason||'尚未填入')+'\n本地发送控件：'+(state.fill.last_send_button_count??state.fill.send_button_count??'未检查')+(state.fill.last_send_target?' · '+state.fill.last_send_target:'')+(state.fill.last_open_reason?'\n展开输入框：'+state.fill.last_open_reason:'');
    status.style.whiteSpace = 'pre-line';
    status.style.overflowWrap = 'anywhere';
    toggle.disabled = state.stopped || state.draining || (!wxCollecting && (!state.connected || !!state.bridge_error));
    toggle.textContent = wxCollecting ? '暂停采集' : '开始采集';
    fields.textContent = info.recent_schemas?.length
      ? info.recent_schemas.map(item => item.channel + (item.category ? ' · ' + item.category : '') + (item.api_kind ? ' · ' + item.api_kind : '') + '\n' + item.fields.join('，')).join('\n\n')
      : info.hook_replaced ? '页面替换了网络采集钩子，当前采集可能不完整。'
      : wxCollecting && info.network_messages === 0
        ? '尚未捕获页面网络消息。互动可能来自客户端原生通道或其他页面上下文，不能据此判断没有礼物。'
        : '暂无结构样本。开始采集后，等待直播页面产生通信。';
    if (info.recent_message_batch) {
      const batch = info.recent_message_batch;
      const types = batch.type_counts.map(c => (c.type === null ? '未知' : c.type) + ' × ' + c.count
        + '（文字 ' + c.text + ' / JSON ' + c.json + ' / 不透明 ' + c.opaque + ' / 缺失 ' + c.absent + '）').join('\n');
      fields.textContent = '最新消息批次：总条目 ' + batch.total + '，检查 ' + batch.sampled
        + (batch.truncated ? '（截断）' : '') + '，超出类型预算 ' + batch.overflow
        + '\n类型尚未映射为弹幕或礼物：\n' + (types || '空批次') + '\n\n' + fields.textContent;
    }
    if (info.recent_message_content) {
      const content=info.recent_message_content;
      fields.textContent='未知消息 type='+content.message_type+' · '+content.format
        +'\n发送者 ID：'+(content.sender_id_present?'存在':'缺失')+' · 消息 ID：'+(content.message_id_present?'存在':'缺失')
        +' · 昵称：'+(content.nickname_present?'存在':'缺失')+'\n到达文字线索：'+(content.arrival_text_hint?'有（未确认入场）':'无')
        +'\n内容结构：'+content.fields.join('，')+'\n原生条目结构：'+content.envelope_fields.join('，')+'\n\n'+fields.textContent;
    }
    if (info.recent_page_match) {
      const match = info.recent_page_match;
      fields.textContent = '页面文字对照（不确定消息语义）：\n'
        + match.type_counts.map(c => (c.type === null ? '未知' : c.type) + '：命中 ' + c.matched + '，未命中 ' + c.unmatched).join('\n')
        + (match.scan_truncated ? '\n页面扫描受限，未命中不能判定未显示。' : '') + '\n\n' + fields.textContent;
    }
    if (info.recent_command) {
      const command = info.recent_command;
      fields.textContent = '最近命令响应：' + command.command_id + ' · JSON ' + command.json_strings
        + ' · Base64 候选 ' + command.base64_candidates + ' · Base64 JSON ' + command.base64_json_strings + ' · Protobuf 候选 ' + command.protobuf_candidates
        + '\n' + command.fields.join('，') + '\n\n' + fields.textContent;
    }
    if (info.recent_application) {
      const app = info.recent_application;
      fields.textContent = '20009 载荷结构：' + app.format + '\n' + app.fields.join('，')
        + '\n原生条目结构：\n' + (app.envelope_fields || []).join('，') + '\n\n' + fields.textContent;
    }
    if (info.recent_gift_schema) {
      const gift = info.recent_gift_schema;
      fields.textContent = '礼物字段线索（不是礼物事件）：' + gift.context
        + '\n' + gift.fields.join('，') + '\n\n' + fields.textContent;
    }
    review.disabled=state.stopped; review.textContent=wxLocalReview?'关闭字段核对':'本地字段核对';
    if (wxLocalReview) {
      status.textContent=connection+' · '+(wxCollecting?'采集中':'已暂停')+'\n字段核对仅在当前页面显示，不入库。';
      const sample=wxApplicationReview.at(-1);
      fields.textContent=sample?(sample.target?'来源目标（未验证）：'+(sample.target.name||'名称缺失')+'\n目标 ID：'+sample.target.id+'\n':'')+'名称：'+sample.names.join('，')+'\n计数字段：'+sample.counts.join('，')
        +'\n结构：\n'+sample.labels.join('，'):'等待新的 20009 应用载荷；尚不确认礼物或数量语义。';
    }
  };
  const open = () => { panel.hidden = !panel.hidden; entry.setAttribute('aria-expanded', String(!panel.hidden)); render(); };
  const collect = () => { if (!toggle.disabled) { wxSetCollecting(!wxCollecting); render(); } };
  const reviewLocal = () => { wxLocalReview=!wxLocalReview;wxApplicationReview.length=0;render(); };
  entry.addEventListener('click', open); toggle.addEventListener('click', collect);
  review.addEventListener('click',reviewLocal);
  const timer = setInterval(() => { if (!panel.hidden) render(); }, 500);
  sdk.onDispose(() => {
    wxSetCollecting(false); clearInterval(timer);
    entry.removeEventListener('click', open); toggle.removeEventListener('click', collect);
    review.removeEventListener('click',reviewLocal);
    entry.remove(); panel.remove();
  });
  render();
};
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', wxMountEntry, {once:true});
  sdk.onDispose(() => document.removeEventListener('DOMContentLoaded', wxMountEntry));
} else wxMountEntry();
