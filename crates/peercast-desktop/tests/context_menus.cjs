const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const app = fs.readFileSync(path.join(__dirname, '../frontend/app.js'), 'utf8');
const html = fs.readFileSync(path.join(__dirname, '../frontend/index.html'), 'utf8');
function appFunction(name, bindings = {}) {
  const match = app.match(new RegExp(`  (?:async )?function ${name}\\([^\\n]*\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, `The ${name} helper must exist`);
  return new Function(...Object.keys(bindings), `return (${match[0].trim()})`)(...Object.values(bindings));
}

const recordingIsActive = appFunction('recordingIsActive');
const displayRecordingPath = appFunction('displayRecordingPath');
assert.equal(displayRecordingPath('\\\\?\\C:\\録画\\長い ファイル名.wmv'), 'C:\\録画\\長い ファイル名.wmv', 'Extended drive paths should display cleanly without losing spaces');
assert.equal(displayRecordingPath('\\\\?\\UNC\\server\\share name\\録画.wmv'), '\\\\server\\share name\\録画.wmv', 'Extended UNC paths should display as ordinary UNC paths');
assert.equal(displayRecordingPath('D:\\録画\\file.wmv'), 'D:\\録画\\file.wmv', 'Ordinary paths must remain unchanged');
assert.equal(displayRecordingPath(''), '');
assert.equal(displayRecordingPath(null), '');
const canUseRecordingFile = appFunction('canUseRecordingFile', { recordingIsActive });
const allRuntimeChannels = appFunction('allRuntimeChannels');
const connectionChannelKey = appFunction('connectionChannelKey');
const isEditableContextTarget = appFunction('isEditableContextTarget');
const suppressNativeContextMenu = appFunction('suppressNativeContextMenu', { isEditableContextTarget });
const editable = { closest: (selector) => selector.includes('input') ? {} : null };
const background = { closest: () => null };
let prevented = 0;
suppressNativeContextMenu({ target: background, preventDefault: () => prevented++ });
assert.equal(prevented, 1, 'Every non-editable region suppresses the browser menu');
suppressNativeContextMenu({ target: editable, preventDefault: () => prevented++ });
assert.equal(prevented, 1, 'Editable controls retain the native menu');

const file = { id: 'r1', currentFile: 'C:/recordings/a.ts', status: 'stopped' };
assert.equal(canUseRecordingFile(file, 'play', false), true);
assert.equal(canUseRecordingFile({ ...file, currentFile: null }, 'reveal', false), false);
assert.equal(canUseRecordingFile(file, 'play', true), false, 'Busy state blocks repeated actions');
for (const status of ['queued', 'connecting', 'recording', 'retrying']) assert.equal(canUseRecordingFile({ ...file, status }, 'trash', false), false);
for (const status of ['stopped', 'error', 'failed']) assert.equal(canUseRecordingFile({ ...file, status }, 'trash', false), true);

const nodes = new Map(['yp-context-menu', 'runtime-context-menu', 'recording-context-menu'].map((id) => [id, { hidden: false }]));
const state = { runtimeContextKey: 'a:s1', recordingContextId: 'A', ypContextChannel: {} };
const byId = (id) => nodes.get(id) || { disabled: false, focus() {} };
const closeAllContextMenus = appFunction('closeAllContextMenus', { byId, state });
closeAllContextMenus();
assert([...nodes.values()].every((node) => node.hidden));
assert.equal(state.runtimeContextKey, null);
assert.equal(state.recordingContextId, null);
assert.equal(state.ypContextChannel, null);

function runtimeHarness(channels, key = 'a:s1', overrides = {}) {
  const liveState = { runtimeContextKey: key, connectionSelectionId: 'b:s2', recordingBusy: false, busy: false, snapshot: { channels } };
  const calls = [];
  const close = () => { liveState.runtimeContextKey = null; };
  const action = appFunction('controlRuntimeChannel', {
    state: liveState, allRuntimeChannels, connectionChannelKey, closeAllContextMenus: close,
    updateRecordingControls() {}, invoke: async (...args) => { calls.push(args); return { channels: [] }; },
    renderSnapshot() {}, setNotice() {}, errorText: String, ...overrides,
  });
  return { liveState, calls, action };
}
const a = { id: 'a', sessionId: 's1', canReconnect: true, canDisconnect: true };
const b = { id: 'b', sessionId: 's2', canReconnect: true, canDisconnect: true };
(async () => {
  let h = runtimeHarness([a, b]);
  assert.equal(await h.action('reconnect'), true);
  assert.deepEqual(h.calls[0], ['control_relay_channel', { channelId: 'a', sessionId: 's1', action: 'reconnect' }], 'Selection changes must not redirect action from A to B');
  h = runtimeHarness([b]);
  assert.equal(await h.action('reconnect'), false, 'A vanished from the latest snapshot must be a no-op');
  assert.equal(h.calls.length, 0);
  h = runtimeHarness([a], 'a:s1', { }); h.liveState.recordingBusy = true;
  assert.equal(await h.action('disconnect'), false);
  assert.equal(h.calls.length, 0, 'Busy state prevents duplicate actions');

  const menu = { hidden: true };
  const controls = new Map(['runtime-context-reconnect', 'runtime-context-disconnect'].map((id) => [id, { disabled: false, focus() {} }]));
  const menuState = { connectionSelectionId: 'b:s2', runtimeContextKey: null, recordingContextId: null, ypContextChannel: null, snapshot: { channels: [a, b] }, recordingBusy: false, busy: false };
  const open = appFunction('openRuntimeContextMenu', {
    isEditableContextTarget, state: menuState, connectionMenu: menu, allRuntimeChannels, connectionChannelKey,
    closeAllContextMenus: () => { menuState.runtimeContextKey = null; }, renderConnections() {},
    byId: (id) => controls.get(id), placeContextMenu: () => { menu.hidden = false; },
  });
  const rowTarget = { closest: (selector) => selector.includes('input') ? null : ({ dataset: { connectionChannelId: 'a:s1' } }) };
  const event = { target: rowTarget, preventDefault() { this.prevented = true; } };
  open(event);
  assert.equal(menuState.runtimeContextKey, 'a:s1');
  assert.equal(menuState.connectionSelectionId, 'a:s1');
  const denied = { ...a, canReconnect: false, canDisconnect: false };
  menuState.snapshot = { channels: [denied] }; menu.hidden = true;
  open({ target: rowTarget, preventDefault() { this.prevented = true; } });
  assert.equal(menu.hidden, true, 'A channel with neither capability does not open a menu');
  const beforeEditable = menuState.runtimeContextKey;
  open({ target: editable, preventDefault() { throw new Error('editable context must not be intercepted'); } });
  assert.equal(menuState.runtimeContextKey, beforeEditable);

  const recordingHarness = (jobs, contextId, busy = false) => {
    const live = { recordingContextId: contextId, recordingSelectionId: 'B', snapshot: { recordings: jobs }, recordingBusy: busy, busy: false };
    const calls = []; let refreshed = 0;
    const close = () => { live.recordingContextId = null; };
    const action = appFunction('recordingFileAction', {
      state: live, closeAllContextMenus: close, canUseRecordingFile,
      updateRecordingControls() {}, invoke: async (...args) => calls.push(args), refresh: async () => refreshed++,
      setNotice() {}, errorText: String,
    });
    return { live, calls, action, get refreshed() { return refreshed; } };
  };
  const jobA = { id: 'A', currentFile: 'A.ts', status: 'stopped' }, jobB = { id: 'B', currentFile: 'B.ts', status: 'stopped' };
  let rec = recordingHarness([jobA, jobB], 'A');
  assert.equal(await rec.action('play'), true);
  assert.deepEqual(rec.calls[0], ['recording_file_action', { jobId: 'A', action: 'play' }], 'Selection changes must not redirect action from A to B');
  assert.equal(rec.refreshed, 1);
  rec = recordingHarness([jobB], 'A');
  assert.equal(await rec.action('play'), false, 'A vanished from latest snapshot is a no-op');
  assert.equal(rec.calls.length, 0);
  rec = recordingHarness([jobA], 'A', true);
  assert.equal(await rec.action('trash'), false, 'Busy state prevents duplicate file operations');
  assert.equal(rec.calls.length, 0);

  for (const id of ['runtime-context-menu', 'recording-context-menu']) assert(html.includes(`id="${id}"`));
  for (const label of ['既定のアプリで再生', 'ファイルの場所を開く', 'ファイルをごみ箱へ移動', '一覧から削除']) assert(html.includes(label), `Expected distinct menu/control label ${label}`);
  assert.match(app, /document\.addEventListener\("contextmenu", suppressNativeContextMenu\)/);
  console.log('Context menu regression: runtime/recording target capture, vanished targets, capabilities, busy guards, editable native menus, global suppression, and distinct labels passed');
})().catch((error) => { console.error(error); process.exitCode = 1; });
