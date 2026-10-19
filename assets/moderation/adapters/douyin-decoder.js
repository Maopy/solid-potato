// Independently implemented bounded protobuf reader. Field references:
// docs/moderation-online.md. No platform socket writes or stored raw frames.
const douyinDecoder = (() => {
  const LIMIT = 2 * 1024 * 1024, decoder = new TextDecoder("utf-8", { fatal: true });
  let queued = 0, chain = Promise.resolve(), stopped = false, room = null;
  const stats = { frames: 0, decoded: 0, rejected: 0, unsupported: 0, last_error: null, last_warning: null };
  const status = () => sdk.setDecoderStatus?.({ ...stats, room_bound: !!room });
  const failure = reason => {
    stats.rejected++; stats.last_error = reason; status();
    sdk.emit("probe.gap", "diagnostic", { reason });
  };
  const pb = bytes => {
    if (!(bytes instanceof Uint8Array) || bytes.length > LIMIT) throw Error("protobuf_size");
    const fields = new Map(); let offset = 0, count = 0;
    const integer = () => {
      let n = 0n;
      for (let i = 0; i < 10; i++) {
        if (offset >= bytes.length) throw Error("protobuf_truncated");
        const b = bytes[offset++];
        if (i === 9 && b > 1) throw Error("protobuf_varint_overflow");
        n |= BigInt(b & 127) << BigInt(7 * i);
        if (!(b & 128)) return n;
      }
      throw Error("protobuf_varint");
    };
    while (offset < bytes.length) {
      if (++count > 10000) throw Error("protobuf_field_budget");
      const tag = integer(), wire = Number(tag & 7n), key = Number(tag >> 3n);
      if (!key || key > 536870911) throw Error("protobuf_tag");
      let value;
      if (wire === 0) value = integer();
      else if (wire === 2) {
        const length = integer();
        if (length > BigInt(bytes.length - offset)) throw Error("protobuf_length");
        value = bytes.subarray(offset, offset + Number(length)); offset += Number(length);
      } else if (wire === 1 || wire === 5) {
        offset += wire === 1 ? 8 : 4;
        if (offset > bytes.length) throw Error("protobuf_fixed_truncated");
        continue;
      } else throw Error("protobuf_wire");
      const list = fields.get(key) || []; list.push(value); fields.set(key, list);
    }
    return fields;
  };
  const one = (f, k) => { const values = f.get(k); if (values?.length > 1) throw Error("protobuf_duplicate_field"); return values?.[0]; };
  const number = (f, k) => { const v = one(f, k); if (v !== undefined && typeof v !== "bigint") throw Error("protobuf_integer_type"); return v || 0n; };
  const id = (f, k) => { const n = number(f, k); return n > 0n ? String(n) : null; };
  const text = (f, k, max = 128) => {
    const v = one(f, k); if (v === undefined) return "";
    if (!(v instanceof Uint8Array) || v.length > max * 4) throw Error("protobuf_text_type_size");
    const value = decoder.decode(v); if (Array.from(value).length > max || /[\u0000-\u001f\u007f]/.test(value)) throw Error("protobuf_text_invalid");
    return value;
  };
  const sub = (f, k) => { const v = one(f, k); if (v === undefined) return new Map(); if (!(v instanceof Uint8Array)) throw Error("protobuf_message_type"); return pb(v); };
  const user = f => ({ id: id(f, 1), nickname: text(f, 3), fields: f });
  const safeCount = n => { if (n < 1n || n > BigInt(Number.MAX_SAFE_INTEGER)) throw Error("gift_quantity_unsupported"); return Number(n); };
  const decimal = v => typeof v === "string" && /^[1-9][0-9]{0,19}$/.test(v) ? v : null;
  const bindRoom = (value, url) => {
    try {
      const u = new URL(url, location.href);
      if (!u.pathname.includes("/webcast/room/web/enter") || u.searchParams.get("web_rid") !== new URL(BOOT.room_url).pathname.slice(1)) return;
      const r = value?.data?.data?.[0], candidate = { room_id: decimal(r?.id_str), recipient_user_id: decimal(r?.owner?.id_str) };
      if (!candidate.room_id || !candidate.recipient_user_id) return;
      if (room && JSON.stringify(room) !== JSON.stringify(candidate)) { failure("room_identity_changed"); return; }
      room = candidate; status();
      sdk.emit("probe.status", "diagnostic", { stage: "room_identity_bound" });
    } catch (_) { /* Unknown JSON shape cannot establish identity. */ }
  };
  const emit = (type, common, native, who, payload, fallbackCreated = 0n) => {
    if (!who?.id || !who.nickname || !native) throw Error("message_identity_missing");
    const roomId = id(common, 3), rawCreated = number(common, 4) || fallbackCreated, created = rawCreated >= 1000000000000n ? rawCreated / 1000n : rawCreated;
    if (!roomId) throw Error("message_room_missing");
    if (created && (created < BigInt(Math.floor(Date.now() / 1000) - 30) || created > BigInt(Math.floor(Date.now() / 1000) + 5))) {
      sdk.emit("probe.status", "diagnostic", { stage: "historical_message_ignored" }); return;
    }
    sdk.emit(type, "network", { ...payload, internal_room_id: roomId, decoder: "douyin.pb.v1", observed_at_ms: Date.now(), live_eligible: !!created }, { native_event_id: native, user_id: who.id, nickname: who.nickname });
    stats.decoded++; status();
    if (["chat.received", "gift.observed", "member.entered"].includes(type) && room?.recipient_user_id) {
      const club = sub(sub(who.fields, 24), 1), level = number(club, 2), anchor = id(club, 6);
      const identity = { native_event_id: native, user_id: who.id, nickname: who.nickname };
      const context = { internal_room_id: roomId, recipient_user_id: room.recipient_user_id, decoder: "douyin.pb.v1", observed_at_ms: Date.now(), live_eligible: !!created };
      if (anchor === room.recipient_user_id && level > 0n && level <= 100000n) sdk.emit("level.snapshot", "network", { ...context, level_kind: "fansclub", level_name: "粉丝团", new_level: Number(level) }, identity);
      const pay = number(sub(who.fields, 23), 6);
      // Keep the raw category distinct until current honor/wealth semantics
      // have been verified. This observation never enters an upgrade rule.
      if (pay > 0n && pay <= 100000n) sdk.emit("level.observed", "network", { ...context, level_kind: "pay_grade", level_name: "消费等级候选", new_level: Number(pay) }, identity);
    }
  };
  const message = (bytes, socketRoom) => {
    const m = pb(bytes), method = text(m, 1, 96);
    if (!["WebcastGiftMessage", "WebcastChatMessage", "WebcastMemberMessage", "WebcastFansclubMessage", "WebcastControlMessage"].includes(method)) { stats.unsupported++; return; }
    if (!room) {
      // The room's existing indicator frame exposes decimal room/anchor IDs.
      // Read only these two fields, never its account or token query values.
      for (const frame of Array.from(document.querySelectorAll?.("iframe") || []).slice(0, 50)) {
        try {
          const u = new URL(frame.src);
          if (/(^|\.)bytegecko\.com$/.test(u.hostname) && u.pathname.includes("/live_pc_indicator_container_douyin/") && decimal(u.searchParams.get("room_id")) === socketRoom) {
            const anchor = decimal(u.searchParams.get("anchor_id_str"));
            if (anchor) { room = { room_id: socketRoom, recipient_user_id: anchor }; break; }
          }
        } catch (_) {}
      }
    }
    const body = sub(m, 2), common = sub(body, 1), native = id(common, 2) || id(m, 3), roomId = id(common, 3);
    if (!roomId || socketRoom !== roomId || room && room.room_id !== roomId) throw Error("message_room_mismatch");
    const commonMethod = text(common, 1, 96);
    if (commonMethod && commonMethod !== method) throw Error("message_method_mismatch");
    const commonNative = id(common, 2);
    if (commonNative && id(m, 3) && commonNative !== id(m, 3)) sdk.emit("probe.status", "diagnostic", { stage: "native_common_id_preferred" });
    const owner = id(sub(common, 16), 4);
    if (owner && !room) room = { room_id: roomId, recipient_user_id: owner };
    if (owner && room?.recipient_user_id !== owner) throw Error("room_owner_changed");
    const recipient = room?.recipient_user_id;
    if (method === "WebcastGiftMessage") {
      const who = user(sub(body, 7)), to = user(sub(body, 8)), target = to.id || recipient;
      try {
      const g = sub(body, 15), giftId = id(body, 2), giftName = text(g, 16), group = id(body, 11), repeat = number(body, 5), groupCount = number(body, 4);
      if (!giftId || !giftName || id(g, 5) && id(g, 5) !== giftId) throw Error("gift_identity_missing_or_conflicting");
      if (number(body, 9) > 1n) throw Error("gift_end_flag_unsupported");
      if (!target) throw Error("gift_recipient_missing");
      const combo = number(g, 10) === 1n;
      if (!group && (combo || groupCount > 1n || repeat > 1n)) throw Error("gift_combo_identity_missing");
      const cumulative = !!group;
      // repeat_count is the cumulative quantity, including multi-item groups.
      // group_count is not an additional multiplier (observed 10 / 10 / 1).
      emit("gift.observed", common, native, who, { gift_id: giftId, gift_name: giftName, recipient_user_id: target,
        ...(g.has(12)&&number(g,12)<=1000000000n?{unit_value:Number(number(g,12))}:{}), quantity_mode: cumulative ? "cumulative" : "incremental", quantity: safeCount(repeat || (cumulative ? 0n : 1n)), combo_id: group, combo_end: number(body, 9) === 1n });
      } catch (error) {
        const isolated = new Set(["gift_identity_missing_or_conflicting", "gift_end_flag_unsupported", "gift_recipient_missing", "gift_combo_identity_missing", "gift_quantity_unsupported", "message_identity_missing"]);
        if (!isolated.has(error.message)) throw error;
        stats.unsupported++; stats.last_warning = error.message; status();
        sdk.emit("probe.skipped", "diagnostic", { reason: error.message, message_type: method, decoder: "douyin.pb.v1", internal_room_id: roomId, recipient_user_id: target || "unknown" },
          { ...(who.id ? { user_id: who.id } : {}), ...(native ? { native_event_id: native } : {}), ...(who.nickname ? { nickname: who.nickname } : {}) });
      }
    } else if (method === "WebcastChatMessage") {
      // Current native comments omit Common.create_time; their event_time is
      // ChatMessage field 15 (seconds, verified against captured frame times).
      // Retain the same age check and never infer freshness from receipt alone.
      emit("chat.received", common, native, user(sub(body, 2)), { content: text(body, 3, 512) }, number(body, 15));
    } else if (method === "WebcastMemberMessage") {
      // action=1 is the entry candidate; other member/admin changes aren't welcomes.
      if (!recipient || number(body, 10) !== 1n) { stats.unsupported++; return; }
      emit("member.entered", common, native, user(sub(body, 2)), { recipient_user_id: recipient, label: "native_entry" });
    } else if (method === "WebcastFansclubMessage") {
      const who = user(sub(body, 4)), club = sub(sub(who.fields, 24), 1), anchor = id(club, 6), kind = number(body, 2);
      if (!recipient || anchor !== recipient || ![1n, 2n].includes(kind)) { stats.unsupported++; return; }
      if (kind === 2n) emit("fansclub.joined", common, native, who, { recipient_user_id: recipient });
      else emit("level.upgraded", common, native, who, { recipient_user_id: recipient, level_kind: "fansclub", level_name: "粉丝团", new_level: safeCount(number(club, 2)) });
    } else if (method === "WebcastControlMessage" && number(body, 2) === 3n) {
      sdk.emit("room.ended", "network", { internal_room_id: roomId, decoder: "douyin.pb.v1", recipient_user_id: recipient || "unknown" });
    }
  };
  const inflate = async bytes => {
    if (typeof DecompressionStream === "undefined") throw Error("gzip_unavailable");
    const reader = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip")).getReader();
    let timer; const chunks = []; let length = 0;
    try {
      const deadline = new Promise((_, reject) => { timer = setTimeout(() => { void reader.cancel().catch(() => {}); reject(Error("gzip_timeout")); }, 2000); });
      await Promise.race([(async () => { while (true) { const r = await reader.read(); if (r.done) break; length += r.value.length; if (length > LIMIT) throw Error("gzip_size"); chunks.push(r.value); } })(), deadline]);
      const output = new Uint8Array(length); let position = 0; for (const c of chunks) { output.set(c, position); position += c.length; } return output;
    } finally { clearTimeout(timer); void reader.cancel().catch(() => {}); }
  };
  const decode = async (data, socketRoom) => {
    if (stopped || !BOOT.rules_preview) return;
    const size = data?.byteLength ?? data?.size;
    if (!socketRoom || typeof size !== "number" || size > LIMIT) throw Error("websocket_frame_unsupported");
    const bytes = data instanceof ArrayBuffer ? new Uint8Array(data) : ArrayBuffer.isView(data) ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength) : new Uint8Array(await data.arrayBuffer());
    const frame = pb(bytes), type = text(frame, 7, 16), payload = one(frame, 8), encoding = text(frame, 6, 16);
    stats.frames++; if (["hb", "ack"].includes(type) || !payload) { status(); return; }
    if (!["pb", "msg"].includes(type) || !(payload instanceof Uint8Array)) throw Error("push_format_unsupported:" + type);
    const responseBytes = encoding === "gzip" || payload[0] === 31 && payload[1] === 139 ? await inflate(payload) : ["", "none", "pb"].includes(encoding) ? payload : null;
    if (!responseBytes) throw Error("push_encoding_unsupported");
    const response = pb(responseBytes), messages = response.get(1) || [];
    if (messages.length > 500) throw Error("response_message_budget");
    // Unsupported gift semantics skip one message and continue this frame.
    // Corrupt transport still reports a gap and cancels incomplete windows.
    for (const m of messages) message(m, socketRoom);
    status();
  };
  const observe = (data, url) => {
    if (!BOOT.rules_preview || stopped) return;
    let socketRoom; try { socketRoom = decimal(new URL(url).searchParams.get("room_id")); } catch (_) {}
    if (++queued > 16) { queued--; failure("decoder_queue_full"); return; }
    chain = chain.then(() => decode(data, socketRoom)).catch(e => failure(e.message)).finally(() => { queued--; });
  };
  sdk.onDispose(() => { stopped = true; });
  status(); return { observe, bindRoom };
})();
