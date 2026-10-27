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
const hasValidConnectionClearKey = appFunction('hasValidConnectionClearKey');
const isClearableDisconnectedConnection = appFunction('isClearableDisconnectedConnection', { hasValidConnectionClearKey });
const filterClearedConnectionSnapshot = appFunction('filterClearedConnectionSnapshot', { connectionChannelKey, hasValidConnectionClearKey, isClearableDisconnectedConnection });
const clearDisconnectedConnections = appFunction('clearDisconnectedConnections', { connectionChannelKey, isClearableDisconnectedConnection });
const isEditableContextTarget = appFunction('isEditableContextTarget');
const suppressNativeContextMenu = appFunction('suppressNativeContextMenu', { isEditableContextTarget });
const ypContextChannelFor = appFunction('ypContextChannelFor');
const editable = { closest: (selector) => selector.includes('input') ? {} : null };
const background = { closest: () => null };
let prevented = 0;
suppressNativeContextMenu({ target: background, preventDefault: () => prevented++ });
assert.equal(prevented, 1, 'Every non-editable region suppresses the browser menu');
suppressNativeContextMenu({ target: editable, preventDefault: () => prevented++ });
assert.equal(prevented, 1, 'Editable controls retain the native menu');

const ypA = { channelId: 'same-id', yellowPage: 'Page A', name: 'A' };
const ypB = { channelId: 'same-id', yellowPage: 'Page B', name: 'B' };
assert.equal(ypContextChannelFor(ypA, [ypB, ypA]), ypA, 'A context action must resolve the row target even if another row is selected');
assert.equal(ypContextChannelFor(ypA, [ypB]), null, 'A vanished context target must not redirect to a different Yellow Page row');

const contextState = { ypChannels: [ypB, ypA], snapshot: { running: true, playerAvailable: true, desktop: { channelRules: [] } }, busy: false, ypBusy: false, recordingBusy: false, ypContextChannel: null };
const contextMenu = { hidden: true };
const menuButtons = new Map(['yp-context-play', 'yp-context-record', 'yp-context-favorite', 'yp-context-ng'].map((id) => [id, { disabled: false, focus() {} }]));
let selectedTarget = null;
let preventedContext = false;
const openYpContextMenu = appFunction('handleYpResultsContextMenu', {
  isEditableContextTarget: () => false,
  state: contextState,
  byId: (id) => id === 'yp-context-menu' ? contextMenu : menuButtons.get(id) || { disabled: false },
  selectYpChannel: (channel) => { selectedTarget = channel; },
  closeAllContextMenus: () => { contextState.ypContextChannel = null; },
  isPlayableYpChannel: () => true,
  directFavoriteRuleIndex: () => -1,
  text() {}, placeContextMenu: () => { contextMenu.hidden = false; },
});
const clickedRow = { dataset: { ypIndex: '1' } };
openYpContextMenu({ target: { closest: (selector) => selector === '.yp-row' ? clickedRow : null }, preventDefault() { preventedContext = true; } });
assert.equal(preventedContext, true);
assert.equal(selectedTarget, ypA, 'Right-click selection must use the exact list row');
assert.equal(contextState.ypContextChannel, ypA);
assert.equal(contextMenu.hidden, false);
contextMenu.hidden = true;
contextState.ypContextChannel = null;
selectedTarget = null;
preventedContext = false;
openYpContextMenu({ target: { closest: () => null }, preventDefault() { preventedContext = true; } });
assert.equal(contextMenu.hidden, true, 'YouTube cards must not open the Yellow Page custom menu');
assert.equal(contextState.ypContextChannel, null);
assert.equal(preventedContext, false);

let contextClosed = false;
let playedContextChannel = null;
const playContext = appFunction('playYpContextChannel', {
  currentYpContextChannel: () => ypA,
  closeYpContextMenu: () => { contextClosed = true; },
  playYpChannel: (channel) => { playedContextChannel = channel; },
});
playContext();
assert.equal(playedContextChannel, ypA, 'Play must receive the captured right-clicked target');
assert.equal(contextClosed, true);
let recordedContextChannel = null;
const recordContext = appFunction('recordYpContextChannel', {
  currentYpContextChannel: () => ypA,
  closeYpContextMenu() {},
  enqueueSelectedRecording: (channel) => { recordedContextChannel = channel; },
});
recordContext();
assert.equal(recordedContextChannel, ypA, 'Recording must receive the captured right-clicked target');

async function testContextRecordingTarget() {
  const calls = [];
  const state = { snapshot: { running: true }, recordingBusy: false, busy: false };
  const enqueue = appFunction('enqueueSelectedRecording', {
    state, currentSelectedDirectoryChannel: () => ypB, isPlayableYpChannel: () => true,
    updateRecordingControls() {},
    invoke: async (command, args) => { calls.push([command, args]); return command === 'enqueue_recording' ? 'job-A' : { recordings: [] }; },
    renderSnapshot() {}, setNotice() {}, displayValue: String, requestView: async () => {}, errorText: String,
  });
  await enqueue(ypA);
  assert.deepEqual(calls[0], ['enqueue_recording', { channelId: 'same-id', yellowPage: 'Page A', keep: false }], 'Context recording must not follow a later selected row');
}

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
  await testContextRecordingTarget();
  let h = runtimeHarness([a, b]);
  assert.equal(await h.action('reconnect'), true);
  assert.deepEqual(h.calls[0], ['control_relay_channel', { channelId: 'a', sessionId: 's1', action: 'reconnect' }], 'Selection changes must not redirect action from A to B');
  h = runtimeHarness([b]);
  assert.equal(await h.action('reconnect'), false, 'A vanished from the latest snapshot must be a no-op');
  assert.equal(h.calls.length, 0);
  h = runtimeHarness([a], 'a:s1', { }); h.liveState.recordingBusy = true;
  assert.equal(await h.action('disconnect'), false);
  assert.equal(h.calls.length, 0, 'Busy state prevents duplicate actions');
  h = runtimeHarness([a], 'a:s1', { }); h.liveState.clearingLogs = true;
  assert.equal(await h.action('disconnect'), false, 'Runtime control is blocked while clearing connection rows');
  assert.equal(h.calls.length, 0);

  const menu = { hidden: true };
  const controls = new Map(['runtime-context-reconnect', 'runtime-context-disconnect', 'runtime-context-clear'].map((id) => [id, { disabled: false, focus() {} }]));
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
  assert.equal(menu.hidden, false, 'Clear remains available when a channel cannot be controlled');
  assert.equal(controls.get('runtime-context-reconnect').disabled, true);
  assert.equal(controls.get('runtime-context-disconnect').disabled, true);
  assert.equal(controls.get('runtime-context-clear').disabled, false);
  menuState.busy = true;
  menu.hidden = true;
  open({ target: rowTarget, preventDefault() {} });
  assert.equal(controls.get('runtime-context-clear').disabled, true, 'Clear is disabled while a runtime operation is busy');
  menuState.busy = false;
  menuState.clearingLogs = true;
  menu.hidden = true;
  open({ target: rowTarget, preventDefault() {} });
  assert.equal(controls.get('runtime-context-reconnect').disabled, true, 'Reconnect is blocked while clearing');
  assert.equal(controls.get('runtime-context-disconnect').disabled, true, 'Disconnect is blocked while clearing');
  assert.equal(controls.get('runtime-context-clear').disabled, true, 'Repeated clear is blocked while clearing');
  menuState.clearingLogs = false;
  menuState.snapshot = { channels: [] }; menu.hidden = true;
  open({ target: background, preventDefault() {} });
  assert.equal(menu.hidden, false, 'An empty Connections view must still offer Clear');
  assert.equal(menuState.runtimeContextKey, null);
  const beforeEditable = menuState.runtimeContextKey;
  open({ target: editable, preventDefault() { throw new Error('editable context must not be intercepted'); } });
  assert.equal(menuState.runtimeContextKey, beforeEditable);

  const stoppedRelay = { id: 'stopped', sessionId: 'relay-session', canReconnect: true, canDisconnect: false, isBroadcasting: false };
  assert.equal(isClearableDisconnectedConnection(stoppedRelay), true);
  assert.equal(isClearableDisconnectedConnection({ ...stoppedRelay, sessionId: '' }), false, 'A missing session ID cannot be hidden safely');
  assert.equal(isClearableDisconnectedConnection({ ...stoppedRelay, canDisconnect: true }), false, 'An active relay cannot be hidden');
  assert.equal(isClearableDisconnectedConnection({ ...stoppedRelay, isBroadcasting: true }), false, 'A broadcasting channel cannot be hidden');
  const renderSnapshotHarness = (snapshotState, rendered) => appFunction('renderSnapshot', {
    state: snapshotState, filterClearedConnectionSnapshot, restoreLastBroadcastOnce() {},
    updateYpSources: () => false, renderChannels() {},
    renderConnections: (snapshot) => rendered.push(snapshot.channels),
    currentView: () => 'connections', allRuntimeChannels, broadcastingChannels: () => [],
    renderLogs() {}, renderProbe() {}, updateBroadcastBackendFields() {}, renderRecordings() {},
    applyDirectoryPreferences() {}, currentSelectedDirectoryChannel: () => null,
    updateConnectionSummary() {}, updateDetailActions() {}, updateYpControls() {},
    renderYpSourcesAndMaybeRefresh() {}, text() {}, byId: () => null,
    document: { querySelectorAll: () => [] },
  });
  const renderState = { snapshot: null, clearedConnectionKeys: new Set(['stopped:relay-session']), ypLastUpdatedAt: null, settingsView: null, detailKind: null, ypStarted: true };
  const renderedChannels = [];
  const renderSnapshot = renderSnapshotHarness(renderState, renderedChannels);
  const sourceSnapshot = { channels: [a, stoppedRelay], desktop: { channelRules: ['keep'] } };
  renderSnapshot(sourceSnapshot);
  assert.deepEqual(sourceSnapshot.channels, [a, stoppedRelay], 'Snapshot source arrays are never mutated');
  assert.deepEqual(renderState.snapshot.channels, [a]);
  assert(renderState.clearedConnectionKeys.has('stopped:relay-session'));
  assert.deepEqual(renderedChannels.at(-1), [a], 'renderSnapshot applies filtering before connection rows/details render');
  renderSnapshot({ channels: [a, stoppedRelay], desktop: { channelRules: ['keep'] } });
  assert.deepEqual(renderState.snapshot.channels, [a], 'A subsequent stopped snapshot keeps the cleared row hidden');
  const activeAfterReconnect = { ...stoppedRelay, canDisconnect: true };
  renderSnapshot({ channels: [a, activeAfterReconnect] });
  assert.deepEqual(renderState.clearedConnectionKeys, new Set(), 'Reconnect clears the hidden key');
  assert.deepEqual(renderState.snapshot.channels, [a, activeAfterReconnect], 'The active channel reappears');

  const clearState = { clearingLogs: false, logClearGeneration: 0, connectionClearGeneration: 0, clearedConnectionKeys: new Set(), snapshot: { channels: [a, stoppedRelay], desktop: { channelRules: ['keep'] }, recordings: [{ id: 'keep' }], logs: [{ message: 'old' }] } };
  const sourceChannels = clearState.snapshot.channels;
  const clearCalls = [], clearNotices = [];
  let finishClear, logRenders = 0, connectionRenders = 0, countText = '';
  const clear = appFunction('clearMessagesAndLogs', {
    state: clearState, closeAllContextMenus() {}, byId: (id) => controls.get(id),
    invoke: (...args) => { clearCalls.push(args); return new Promise((resolve) => { finishClear = resolve; }); },
    renderLogs: () => logRenders++, setNotice: (...args) => clearNotices.push(args), errorText: String,
    clearDisconnectedConnections, renderConnections: () => connectionRenders++, currentView: () => 'connections',
    allRuntimeChannels, text: (_id, value) => { countText = value; },
  });
  const clearing = clear();
  assert.equal(await clear(), false, 'Repeated clear requests must coalesce');
  finishClear();
  assert.equal(await clearing, true);
  assert.deepEqual(clearCalls, [['clear_logs']], 'Clear must not invoke channel or settings operations');
  assert.deepEqual(clearState.snapshot.logs, []);
  assert.deepEqual(clearState.snapshot.channels, [a], 'Clearing removes only stopped relay rows and preserves active channels');
  assert.deepEqual(sourceChannels, [a, stoppedRelay], 'Clear leaves the original channel array unchanged');
  assert.notStrictEqual(clearState.snapshot.channels, sourceChannels, 'Clear installs a filtered copy of the channel array');
  assert.deepEqual([...clearState.clearedConnectionKeys], ['stopped:relay-session']);
  assert.deepEqual(clearState.snapshot.desktop.channelRules, ['keep']);
  assert.deepEqual(clearState.snapshot.recordings, [{ id: 'keep' }], 'Clear does not alter recordings');
  assert.equal(clearState.connectionClearGeneration, 1);
  assert.equal(connectionRenders, 1, 'The filtered connection list and details are rendered after clear');
  assert.equal(countText, '1 件の接続チャンネル');
  assert.deepEqual(clearNotices.at(-1), [''], 'A success notice must not replace the notice just cleared');
  assert.equal(logRenders, 1);
  assert.equal(clearState.logClearGeneration, 1);
  const failedClearState = { ...clearState, snapshot: { ...clearState.snapshot, channels: [a, stoppedRelay], logs: [{ message: 'keep on error' }] }, clearedConnectionKeys: new Set(['already:clear']) };
  const failClear = appFunction('clearMessagesAndLogs', {
    state: failedClearState, closeAllContextMenus() {}, byId: (id) => controls.get(id),
    invoke: async () => { throw new Error('failed'); }, renderLogs() {}, renderConnections() {}, clearDisconnectedConnections,
    currentView: () => 'connections', allRuntimeChannels, text() {},
    setNotice: (...args) => clearNotices.push(args), errorText: String,
  });
  assert.equal(await failClear(), false);
  assert.equal(failedClearState.snapshot.logs[0].message, 'keep on error');
  assert.deepEqual(failedClearState.clearedConnectionKeys, new Set(['already:clear']));
  assert.deepEqual(failedClearState.snapshot.channels, [a, stoppedRelay], 'A failed clear leaves connection rows untouched');
  assert.deepEqual([...failedClearState.clearedConnectionKeys], ['already:clear'], 'A failed clear leaves hidden-row keys untouched');
  assert.equal(clearNotices.at(-1)[1], true);
  assert.equal(failedClearState.clearingLogs, false);

  let finishSnapshot;
  const currentAfterClear = [a];
  const refreshState = { snapshot: { logs: [], channels: currentAfterClear }, logClearGeneration: 0, connectionClearGeneration: 0, clearedConnectionKeys: new Set() };
  const refresh = appFunction('refresh', {
    state: refreshState, performance: { now: () => 0 }, updateRefreshControls() {},
    invoke: () => new Promise((resolve) => { finishSnapshot = resolve; }),
    renderSnapshot: (snapshot) => { refreshState.snapshot = snapshot; },
    setNotice() {}, errorText: String, finishRefreshFeedback: async () => {},
  });
  const oldRefresh = refresh();
  refreshState.logClearGeneration++;
  refreshState.connectionClearGeneration++;
  refreshState.clearedConnectionKeys.add('stopped:relay-session');
  finishSnapshot({ logs: [{ message: 'stale before clear' }], channels: [a, { ...stoppedRelay, canDisconnect: true, status: 'connected' }] });
  await oldRefresh;
  assert.deepEqual(refreshState.snapshot.logs, [], 'A refresh already in flight must not resurrect cleared logs');
  assert.deepEqual(refreshState.snapshot.channels, [a], 'A pre-clear refresh must keep the current filtered channels array');
  assert.strictEqual(refreshState.snapshot.channels, currentAfterClear, 'The current filtered array is reused instead of an old response array');
  assert(refreshState.clearedConnectionKeys.has('stopped:relay-session'), 'A stale active response must not remove the cleared key');
  const newRefresh = refresh();
  finishSnapshot({ logs: [{ message: 'new after clear' }] });
  await newRefresh;
  assert.equal(refreshState.snapshot.logs[0].message, 'new after clear', 'New logs must still appear after clearing');

  const noSnapshotState = { snapshot: null, clearingLogs: false, busy: false, recordingBusy: false, logClearGeneration: 0, connectionClearGeneration: 0, clearedConnectionKeys: new Set() };
  let finishEmptyViewRefresh;
  const emptyViewRefresh = appFunction('refresh', {
    state: noSnapshotState, performance: { now: () => 0 }, updateRefreshControls() {},
    invoke: () => new Promise((resolve) => { finishEmptyViewRefresh = resolve; }),
    renderSnapshot: (snapshot) => { noSnapshotState.snapshot = snapshot; },
    setNotice() {}, errorText: String, finishRefreshFeedback: async () => {},
  });
  const staleEmptyViewRefresh = emptyViewRefresh();
  const clearEmptyView = appFunction('clearMessagesAndLogs', {
    state: noSnapshotState, closeAllContextMenus() {}, byId: (id) => controls.get(id),
    invoke: async () => {}, renderLogs() {}, renderConnections() {}, clearDisconnectedConnections,
    currentView: () => 'connections', allRuntimeChannels, text() {}, setNotice() {}, errorText: String,
  });
  assert.equal(await clearEmptyView(), true);
  finishEmptyViewRefresh({ logs: [{ message: 'stale' }], channels: [stoppedRelay] });
  await staleEmptyViewRefresh;
  assert.deepEqual(noSnapshotState.snapshot.channels, [], 'A pre-clear poll cannot repopulate channels when Clear began before the first snapshot');

  assert.match(html, /id="runtime-context-clear"[^>]*title="[^"]*表示メッセージ[^"]*"[^>]*>クリア（切断済み・ログ）/);
  assert.match(app, /byId\("view-connections"\)\?\.addEventListener\("contextmenu", openRuntimeContextMenu\)/, 'The whole Connections view offers Clear');

  const reconnected = { ...stoppedRelay, canDisconnect: true, status: 'connected' };
  const newSession = { ...stoppedRelay, sessionId: 'new-session', status: 'connected' };
  const sameSessionActive = filterClearedConnectionSnapshot({ channels: [reconnected] }, new Set(['stopped:relay-session']));
  assert.deepEqual(sameSessionActive.clearedConnectionKeys, new Set(), 'An active same-session channel is no longer hidden');
  assert.deepEqual(sameSessionActive.snapshot.channels, [reconnected]);
  const replacementSession = filterClearedConnectionSnapshot({ channels: [newSession] }, new Set(['stopped:relay-session']));
  assert.deepEqual(replacementSession.clearedConnectionKeys, new Set(), 'A replacement session prunes the previous key');
  assert.deepEqual(replacementSession.snapshot.channels, [newSession]);
  const absent = filterClearedConnectionSnapshot({ channels: [a] }, new Set(['stopped:relay-session']));
  assert.deepEqual(absent.clearedConnectionKeys, new Set(), 'Keys absent from a fresh snapshot are pruned');
  const preserved = filterClearedConnectionSnapshot({ channels: [a] }, new Set(['stopped:relay-session']), true);
  assert.deepEqual(preserved.clearedConnectionKeys, new Set(['stopped:relay-session']), 'A snapshot carrying the current filtered array preserves its keys');
  const originalChannels = [a, stoppedRelay];
  const clearedSnapshot = clearDisconnectedConnections({ channels: originalChannels }, new Set());
  assert.deepEqual(clearedSnapshot.snapshot.channels, [a]);
  assert.deepEqual(originalChannels, [a, stoppedRelay], 'Clear filtering does not mutate the source channel array');

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
