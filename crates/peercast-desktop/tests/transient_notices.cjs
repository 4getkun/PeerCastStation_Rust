const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const app = fs.readFileSync(path.join(__dirname, '../frontend/app.js'), 'utf8');
function appFunction(name, bindings = {}) {
  const match = app.match(new RegExp(`  (?:async )?function ${name}\\([^\\n]*\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, `The ${name} helper must exist`);
  return new Function(...Object.keys(bindings), `return (${match[0].trim()})`)(...Object.values(bindings));
}

const timers = new Map();
let nextTimer = 0;
const cleared = [];
const fakeWindow = {
  setTimeout(callback, delay) { const id = ++nextTimer; timers.set(id, { callback, delay }); return id; },
  clearTimeout(id) { cleared.push(id); },
};
const state = { noticeTimer: null, noticeToken: 0, noticeMessage: '', noticeIsError: false };
const element = { textContent: '', hidden: true, classes: new Set(), classList: {
  toggle(name, enabled) { if (enabled) element.classes.add(name); else element.classes.delete(name); },
} };
const setNotice = appFunction('setNotice', {
  state,
  byId: () => element,
  window: fakeWindow,
});

setNotice('保存しました');
const successTimer = state.noticeTimer;
assert.equal(timers.get(successTimer).delay, 5000, 'Success notices expire after five seconds');
setNotice('保存しました');
assert.equal(state.noticeTimer, successTimer, 'Repeating the same notice does not extend its lifetime');
assert.equal(timers.size, 1);
timers.get(successTimer).callback();
assert.equal(element.hidden, true, 'The success notice is hidden after expiry');
assert.equal(element.textContent, '', 'Expiry clears the old message');

setNotice('失敗しました', true);
const errorTimer = state.noticeTimer;
const staleErrorCallback = timers.get(errorTimer).callback;
assert.equal(timers.get(errorTimer).delay, 12000, 'Error notices expire after twelve seconds');
setNotice('処理中…', false, { persistent: true });
const persistentTimer = state.noticeTimer;
assert.equal(state.noticeTimer, null, 'Persistent progress has no expiry timer');
assert(cleared.includes(errorTimer), 'Replacing a notice cancels its prior timer');
staleErrorCallback();
assert.equal(element.textContent, '処理中…', 'A stale timer cannot clear a replacement notice');
assert.equal(element.hidden, false);

setNotice('一時メッセージ');
const pendingTimer = state.noticeTimer;
setNotice('');
assert.equal(element.hidden, true, 'Clearing a notice hides it');
if (timers.has(pendingTimer)) timers.get(pendingTimer).callback();
assert.equal(element.textContent, '', 'A late timer has no effect after explicit clear');
console.log('Frontend transient notice regression: success/error expiry, persistence, replacement race, duplicate, and clear passed');
