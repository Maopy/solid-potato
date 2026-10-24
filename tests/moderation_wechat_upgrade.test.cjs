const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// Run the production decoder with a minimal native SDK boundary.
const adapter = fs.readFileSync('assets/moderation/adapters/wechat-desktop.js', 'utf8');
const code = adapter.slice(adapter.indexOf('const wxUpgradeSeen ='), adapter.indexOf('const wxChatCandidate ='));
const emitted = [];
const BOOT = {};
const context = vm.createContext({
  BOOT, wxCollecting: true, Date,
  wxOwnValue: (o, k) => o && Object.hasOwn(o, k) ? o[k] : undefined,
  wxIdentifier: (v, n = 128) => typeof v === 'string' && v.trim().length > 0
    && v.length <= n && !/[\u0000-\u001f\u007f-\u009f]/.test(v),
  wxGiftNickname: o => o?.contact?.nickname ?? null,
  sdk: {getStatus: () => ({stopped: false, buffered: 0}), onDispose() {},
    emit: (...event) => { emitted.push(event); return true; }},
});
vm.runInContext(code + '\nglobalThis.decode = wxUpgradeCandidate;', context);
const entry = id => ({client_msg_id: id,
  from_user_contact: {contact: {username: 'viewer', nickname: '测试哥'}},
  to_user_contact: {contact: {bind_info: [], menu: [], reference_info: []}},
});
const level = {type: 1, from_level: 1, to_level: 8};
context.decode('native_bridge', entry('no-page-identity'), level);
assert.equal(emitted.length, 0, 'no default recipient inference without native page identity');
BOOT.room_identity = 'host';
context.decode('native_bridge', entry('native-no-recipient'), level);
context.decode('native_bridge', entry('native-no-recipient'), level);
assert.equal(emitted.length, 1, 'duplicate native notice is emitted once');
assert.equal(emitted[0][0], 'level.candidate');
assert.equal(emitted[0][2].recipient_user_id, 'host');
assert.equal(emitted[0][2].recipient_source, 'page_room_identity');
const explicit = entry('explicit-other');
explicit.to_user_contact.contact.username = 'other-host';
context.decode('native_bridge', explicit, level);
assert.equal(emitted[1][2].recipient_user_id, 'other-host', 'explicit recipient is never replaced');
const malformed = entry('malformed-target');
malformed.to_user_contact.contact.username = {username: 'host'};
context.decode('native_bridge', malformed, level);
context.decode('fetch', entry('not-native'), level);
context.decode('native_bridge', entry('not-upgrade'), {...level, type: 2});
assert.equal(emitted.length, 2, 'malformed targets and unsupported notices cannot use fallback');
for (const [id, values] of [
  ['same-level', {from_level: 8, to_level: 8}],
  ['lower-level', {from_level: 16, to_level: 10}],
  ['repeated-level', {from_level: 0, to_level: 8}],
  ['no-before', {to_level: 10}],
  ['zero-level', {from_level: 0, to_level: 0}],
]) {
  context.decode('native_bridge', entry(id), {type: 1, ...values});
  context.decode('native_bridge', entry(id), {type: 1, ...values});
}
assert.equal(emitted.length, 7, 'every distinct notice triggers once, regardless of level history or comparison');
console.log('WeChat upgrade notification and transport deduplication tests passed');
