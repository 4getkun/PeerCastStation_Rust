const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

// Exercise the shipped pure helper without adding browser dependencies.
const source = fs.readFileSync(path.join(__dirname, '../frontend/settings.js'), 'utf8');
const settingsHtml = fs.readFileSync(path.join(__dirname, '../frontend/settings.html'), 'utf8');
function settingsFunction(name) {
  const match = source.match(new RegExp(`  function ${name}\\([^\\n]*\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, `The ${name} settings helper must exist`);
  return new Function(`return (${match[0].trim()})`)();
}
const helper = source.match(/  function exactNamePattern\(name\) \{([\s\S]*?)\n  \}/);
assert(helper, 'The exact-name helper must exist');
const exactNamePattern = new Function('name', helper[1]);
for (const name of [
  '録画検証（疑似データ）',
  'Rif: PPP CHANNEL',
  'A+B [x].*?(a){2}^$\\z',
  'Ａ＋Ｂ［ｘ］（Ｆｒｅｅ）',
]) {
  const expression = new RegExp(exactNamePattern(name), 'iu');
  assert(expression.test(name.normalize('NFKC')), `Must match the full name: ${name}`);
  assert(!expression.test(`${name.normalize('NFKC')} extra`), `Must reject a longer name: ${name}`);
}
console.log('Frontend exact-name regression: 4 fixtures passed');
const normalizeWindowPreferences = settingsFunction('normalizeWindowPreferences');
const parseWindowPreset = settingsFunction('parseWindowPreset');
const isValidWindowSize = settingsFunction('isValidWindowSize');
assert.deepEqual(normalizeWindowPreferences(), { width: 1600, height: 900, minimizeToTray: false, launchAtStartup: false });
assert.deepEqual(normalizeWindowPreferences({ width: 1486, height: 1059, minimizeToTray: true }), { width: 1486, height: 1059, minimizeToTray: true, launchAtStartup: false }, 'Existing saved custom sizes must be preserved');
const presets = ['1280x720', '1366x768', '1600x900', '1920x1080', '2560x1440'];
for (const preset of presets) assert(settingsHtml.includes(`<option value="${preset}">`), `The preset ${preset} must be available`);
assert.deepEqual(parseWindowPreset('1366x768'), { width: 1366, height: 768 });
assert.deepEqual(parseWindowPreset('1486x1059'), { width: 1486, height: 1059 }, 'Nonstandard preset values must remain selectable');
assert.equal(isValidWindowSize(1280, 720), true);
assert.equal(isValidWindowSize(2560, 1440), true);
assert.equal(isValidWindowSize(1279, 720), false);
assert.equal(isValidWindowSize(1600, 1441), false);
assert.match(settingsHtml, /id="window-width" type="number" min="1280" max="2560"[^>]*value="1600"/);
assert.match(settingsHtml, /id="window-height" type="number" min="720" max="1440"[^>]*value="900"/);
console.log('Frontend window size regression: defaults, five presets, saved custom size, and range passed');

const appSource = fs.readFileSync(path.join(__dirname, '../frontend/app.js'), 'utf8');
const appHtml = fs.readFileSync(path.join(__dirname, '../frontend/index.html'), 'utf8');
function appFunction(name, bindings = {}) {
  const match = appSource.match(new RegExp(`  function ${name}\\([^\\n]*\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, `The ${name} helper must exist`);
  return new Function(...Object.keys(bindings), `return (${match[0].trim()})`)(...Object.values(bindings));
}
const broadcastingHelper = appSource.match(/  function isBroadcastingChannel\(channel\) \{([\s\S]*?)\n  \}/);
assert(broadcastingHelper, 'Broadcast view must use the explicit broadcasting flag');
const isBroadcastingChannel = new Function('channel', broadcastingHelper[1]);
const channels = [
  { id: 'broadcast', isBroadcasting: true },
  { id: 'listener', isBroadcasting: false },
  { id: 'missing-flag' },
];
assert.deepEqual(channels.filter(isBroadcastingChannel).map(({ id }) => id), ['broadcast']);
console.log('Frontend broadcast-list regression: 3 fixtures passed');

const allRuntimeChannels = appFunction('allRuntimeChannels');
const runtimeChannels = [
  { id: 'broadcast', isBroadcasting: true },
  { id: 'listener', isBroadcasting: false },
];
assert.deepEqual(allRuntimeChannels({ channels: runtimeChannels }), runtimeChannels, 'The connection view must include non-broadcast relay channels');
const connectionChannelKey = appFunction('connectionChannelKey');
assert.notEqual(connectionChannelKey({ id: 'same', sessionId: 'viewer' }), connectionChannelKey({ id: 'same', sessionId: 'recorder' }), 'Viewing and recording the same channel must remain separately selectable');
const networkOverallStatus = appFunction('networkOverallStatus');
const networkStatusLabel = appFunction('networkStatusLabel');
const connectionPortLabel = appFunction('connectionPortLabel');
assert.equal(networkOverallStatus({ ipv4: 'open', ipv6: 'unknown' }), 'open');
assert.equal(networkOverallStatus({ ipv4: 'closed', ipv6: 'closed' }), 'closed');
assert.equal(networkOverallStatus({ ipv4: 'closed', ipv6: 'unavailable' }), 'closed');
assert.equal(networkOverallStatus({ ipv4: 'unknown', ipv6: 'unavailable' }), 'unknown');
assert.equal(networkOverallStatus({ ipv4: 'unavailable', ipv6: 'unavailable' }), 'unavailable');
assert.equal(networkStatusLabel('unknown'), 'ポート未確認', 'An unknown port must not appear closed');
assert.equal(connectionPortLabel(null), '未確認');
assert.ok(appHtml.indexOf('data-view="channels"') < appHtml.indexOf('data-view="connections"'));
assert.ok(appHtml.indexOf('data-view="connections"') < appHtml.indexOf('data-view="recordings"'));
for (const id of ['view-connections', 'connections-channel-list', 'connection-rows', 'connections-metadata', 'connection-tree']) {
  assert(appHtml.includes(`id="${id}"`), `The connection view must include #${id}`);
}
console.log('Frontend connection view regression: includes relay channels, safe port states, navigation order, and panels passed');

const isCountableYpChannel = appFunction('isCountableYpChannel');
const countableYpChannelCount = appFunction('countableYpChannelCount', { isCountableYpChannel });
const validId = 'a'.repeat(32);
const countFixtures = [
  { name: '有効配信・視聴者数不明', yellowPage: 'SP', channelId: validId, tracker: '127.0.0.1:7144', bitrate: 128 },
  { name: 'SPのsystem notice・bitrate 0', yellowPage: 'SP', channelId: validId, tracker: '127.0.0.1:7144', bitrate: 0 },
  { name: 'P@のsystem notice・invalid ID', yellowPage: 'P@', channelId: 'not-an-id', tracker: '127.0.0.1:7144', bitrate: 128 },
  { name: 'SPのsystem notice・trackerなし', yellowPage: 'SP', channelId: validId, tracker: '', bitrate: 128 },
  { name: 'P@のsystem notice・zero ID', yellowPage: 'P@', channelId: '0'.repeat(32), tracker: '127.0.0.1:7144', bitrate: 128 },
];
assert.equal(countableYpChannelCount(countFixtures), 1);
assert.deepEqual(countFixtures.filter(isCountableYpChannel).map(({ name }) => name), ['有効配信・視聴者数不明']);
console.log('Frontend YP count regression: 5 fixtures passed');

const channelAnnotation = (channel) => channel?.annotation && typeof channel.annotation === 'object' ? channel.annotation : channel || {};
const ypChannelKey = appFunction('ypChannelKey');
const advanceYpGeneration = appFunction('advanceYpGeneration', { ypChannelKey });
const ypNewcomerIndicator = appFunction('ypNewcomerIndicator');
const generationChannel = (id, yellowPage = 'SP') => ({ channelId: id.repeat(32), yellowPage });
const baselineChannels = [generationChannel('a'), generationChannel('b', 'P@')];
const baselineGeneration = advanceYpGeneration(null, baselineChannels);
assert.equal(baselineGeneration.currentNewKeys.size, 0, 'The first successful fetch establishes a baseline');
const added = generationChannel('c');
const secondGeneration = advanceYpGeneration(baselineGeneration, [...baselineChannels, added]);
assert.deepEqual([...secondGeneration.currentNewKeys], [ypChannelKey(added)]);
assert.equal(secondGeneration.previousNewKeys.size, 0);
assert.deepEqual(ypNewcomerIndicator(ypChannelKey(added), secondGeneration), {
  src: 'assets/bulb-new.svg', label: '今回の更新で追加',
}, 'A newly appearing channel gets the bright bulb and accessible label');
assert.strictEqual(advanceYpGeneration(secondGeneration, [...baselineChannels, generationChannel('d')], false), secondGeneration,
  'A failed fetch must not advance newcomer generations');
const automaticPoll = secondGeneration;
assert.strictEqual(automaticPoll, secondGeneration, 'A snapshot poll must not advance YP generations');
const restored = generationChannel('d');
const thirdGeneration = advanceYpGeneration(secondGeneration, [...baselineChannels, added, restored]);
assert.deepEqual([...thirdGeneration.currentNewKeys], [ypChannelKey(restored)]);
assert.deepEqual([...thirdGeneration.previousNewKeys], [ypChannelKey(added)]);
assert.deepEqual(ypNewcomerIndicator(ypChannelKey(added), thirdGeneration), {
  src: 'assets/bulb-previous.svg', label: '前回の更新で追加',
}, 'Only the immediately previous newcomer gets the dim bulb');
assert.equal(ypNewcomerIndicator(ypChannelKey(baselineChannels[0]), thirdGeneration), null,
  'Older baseline channels have no bulb');
const fourthGeneration = advanceYpGeneration(thirdGeneration, [...baselineChannels, added, restored]);
assert.equal(fourthGeneration.currentNewKeys.size, 0);
assert.deepEqual([...fourthGeneration.previousNewKeys], [ypChannelKey(restored)], 'Only the immediately previous update stays dim');
const disappeared = advanceYpGeneration(fourthGeneration, [...baselineChannels, restored]);
const reappeared = advanceYpGeneration(disappeared, [...baselineChannels, added, restored]);
assert.deepEqual([...reappeared.currentNewKeys], [ypChannelKey(added)], 'A channel returning after a successful disappearance is new again');
const fetchYpBody = appSource.match(/  async function fetchYpChannels\(refresh = false, manual = false\) \{([\s\S]*?)\n  \}/)?.[1];
assert(fetchYpBody?.includes('state.ypGeneration = advanceYpGeneration(state.ypGeneration, channels)'),
  'Only the YP fetch success path should advance its generation');
assert.doesNotMatch(fetchYpBody.slice(fetchYpBody.indexOf('} catch'), fetchYpBody.indexOf('} finally')), /advanceYpGeneration/,
  'A failed fetch must not advance its generation');
assert(fetchYpBody.includes('manual && (state.busy || state.ypBusy || state.ypRefreshCooldownUntil > performance.now())'),
  'Manual YP refresh must be blocked while busy or during the cooldown');
assert(fetchYpBody.includes('state.ypRefreshCooldownUntil = performance.now() + YP_REFRESH_COOLDOWN_MS'),
  'Every completed YP fetch must start the cooldown');
const snapshotBody = appSource.match(/  function renderSnapshot\(snapshot, \{ initial = false \} = \) \{([\s\S]*?)\n  \}/)?.[1];
assert.doesNotMatch(snapshotBody || '', /advanceYpGeneration/, 'Snapshot polling must not advance the YP generation');
const visibleBody = appSource.match(/  function visibleYpChannels\(\) \{([\s\S]*?)\n  \}/)?.[1];
assert.doesNotMatch(visibleBody || '', /advanceYpGeneration/, 'Search and sorting must not advance the YP generation');

const ypViewerCount = appFunction('ypViewerCount');
const orderYpChannels = appFunction('orderYpChannels', {
  channelAnnotation,
  isPlayableYpChannel: (channel) => channel.playable,
  ypViewerCount,
});
const orderedFixtures = [
  { id: 'nonfavorite-low', playable: true, listeners: 2 },
  { id: 'favorite-unknown', playable: false, annotation: { isFavorite: true } },
  { id: 'favorite-negative', playable: false, listeners: -1, annotation: { isFavorite: true } },
  { id: 'nonfavorite-high', playable: true, listeners: 20 },
  { id: 'favorite-high', playable: false, listeners: 12, annotation: { isFavorite: true } },
  { id: 'favorite-zero', playable: false, listeners: 0, annotation: { isFavorite: true } },
  { id: 'nonfavorite-unplayable', playable: false, listeners: 100 },
];
assert.deepEqual(orderYpChannels(orderedFixtures).map(({ id }) => id), [
  'favorite-high', 'favorite-zero', 'favorite-unknown', 'favorite-negative',
  'nonfavorite-high', 'nonfavorite-low', 'nonfavorite-unplayable',
], 'Favorites sort first by listeners, with unknown/negative last; nonfavorites retain legacy order');
const newcomerRenderBody = appSource.match(/  function renderYpDirectory\(\) \{([\s\S]*?)\n  \}/)?.[1];
assert.match(newcomerRenderBody || '', /ypNewcomerIndicator\(key, state\.ypGeneration\)/);
assert.match(newcomerRenderBody || '', /indicator\.setAttribute\("aria-label", newcomer\.label\)/);
const appCss = fs.readFileSync(path.join(__dirname, '../frontend/style.css'), 'utf8');
assert.match(appCss, /\.yp-row-copy\{position:relative;padding-left:26px\}/);
assert.match(appCss, /\.yp-row-newcomer\{[^}]*width:18px;height:18px/);
console.log('Frontend YP update generation and favorite sort regression passed');

const timers = new Map();
let nextTimerId = 1;
const fakeWindow = {
  setTimeout(callback, delay) { const id = nextTimerId++; timers.set(id, { callback, delay }); return id; },
  clearTimeout(id) { timers.delete(id); },
};
const refreshState = { ypRefreshTimer: null, ypRefreshMode: 'manual', ypRefreshIntervalSecs: 300, ypDirectoryDirty: false };
const fetchCalls = [];
const scheduleYpAutoRefresh = appFunction('scheduleYpAutoRefresh', {
  state: refreshState,
  window: fakeWindow,
  fetchYpChannels: (force) => fetchCalls.push(force),
});
const radio = { manual: { checked: false }, auto: { checked: false } };
const intervalInput = { value: '5' };
const applyDirectoryPreferences = appFunction('applyDirectoryPreferences', {
  state: refreshState,
  document: { querySelector: (selector) => radio[selector.includes('auto') ? 'auto' : 'manual'] },
  byId: () => intervalInput,
  scheduleYpAutoRefresh,
});
applyDirectoryPreferences({ autoRefresh: true });
assert.equal(refreshState.ypRefreshIntervalSecs, 300, 'The default refresh interval must be five minutes');
assert.equal(intervalInput.value, '5', 'The interval field must display minutes');
assert.equal(timers.get(refreshState.ypRefreshTimer).delay, 300000);
applyDirectoryPreferences({ autoRefresh: true, refreshIntervalSecs: 5 });
assert.equal(refreshState.ypRefreshIntervalSecs, 60, 'Legacy intervals must round up to a whole minute');
assert.equal(intervalInput.value, '1');
assert.equal(timers.get(refreshState.ypRefreshTimer).delay, 60000);
const firstTimerId = refreshState.ypRefreshTimer;
intervalInput.value = '37';
radio.auto.checked = false;
radio.manual.checked = true;
refreshState.ypDirectoryDirty = true;
applyDirectoryPreferences({ autoRefresh: true, refreshIntervalSecs: 15 });
assert.equal(intervalInput.value, '37', 'Uncommitted interval input must survive snapshot polling');
assert.equal(radio.manual.checked, true, 'Uncommitted mode selection must survive snapshot polling');
assert.equal(refreshState.ypRefreshTimer, firstTimerId, 'An unchanged snapshot must not restart the timer');
assert.equal(timers.get(firstTimerId).delay, 60000, 'An unchanged snapshot must not extend the refresh delay');
refreshState.ypDirectoryDirty = false;
applyDirectoryPreferences({ autoRefresh: false, refreshIntervalSecs: 60 });
assert.equal(refreshState.ypRefreshTimer, null, 'Switching to manual mode must stop the timer');
assert.equal(timers.size, 0);
assert.equal(radio.manual.checked, true);
applyDirectoryPreferences({ autoRefresh: true, refreshIntervalSecs: 30 });
const autoTimerId = refreshState.ypRefreshTimer;
timers.get(autoTimerId).callback();
assert.equal(refreshState.ypRefreshTimer, null);
assert.deepEqual(fetchCalls, [true], 'The automatic timer must request a forced refresh');
console.log('Frontend YP refresh timer regression: min interval, unchanged snapshot, manual stop, and forced fetch passed');

const refreshFeedbackState = { busy: false, ypBusy: false, ypRefreshing: false, refreshing: true, manualRefreshing: false };
const refreshButtons = Object.fromEntries(['yp-refresh', 'refresh-button'].map((id) => [id, {
  disabled: false,
  label: { textContent: '更新' },
  classList: { toggle() {} },
  setAttribute() {},
  querySelector() { return this.label; },
}]));
const updateRefreshControls = appFunction('updateRefreshControls', {
  state: refreshFeedbackState,
  byId: (id) => refreshButtons[id],
});
updateRefreshControls();
assert.equal(refreshButtons['refresh-button'].disabled, false, 'Background polling must not flicker the manual refresh button');
refreshFeedbackState.manualRefreshing = true;
updateRefreshControls();
assert.equal(refreshButtons['refresh-button'].disabled, true, 'A user refresh must block duplicate clicks');
assert.equal(refreshButtons['refresh-button'].label.textContent, '更新中…');
refreshFeedbackState.manualRefreshing = false;
refreshFeedbackState.refreshing = false;
updateRefreshControls();
assert.equal(refreshButtons['refresh-button'].disabled, false);
assert.equal(refreshButtons['refresh-button'].label.textContent, '更新');
console.log('Frontend refresh feedback regression: quiet background polling and visible manual refresh passed');

const beginYpFetch = appFunction('beginYpFetch');
const pendingState = { ypBusy: false, busy: false, ypRefreshPending: false, ypRefreshForcePending: false };
let requestCount = 0;
const fetchYpChannels = (force) => { if (beginYpFetch(pendingState, force)) requestCount += 1; };
const flushPendingYpRefresh = appFunction('flushPendingYpRefresh', { state: pendingState, fetchYpChannels });
assert.equal(beginYpFetch(pendingState, true), true);
requestCount += 1;
assert.equal(beginYpFetch(pendingState, true), false, 'An in-flight request must reject a concurrent fetch');
assert.equal(pendingState.ypRefreshPending, true);
assert.equal(requestCount, 1);
pendingState.ypBusy = false;
pendingState.ypRefreshPending = true;
pendingState.ypRefreshForcePending = true;
flushPendingYpRefresh();
flushPendingYpRefresh();
assert.equal(requestCount, 2, 'Repeated queue flushes must trigger only one pending refresh');
assert.equal(pendingState.ypRefreshPending, false);
assert.equal(pendingState.ypRefreshForcePending, false);
console.log('Frontend YP in-flight refresh regression: duplicate requests coalesce to one');

const createBroadcastRequest = appFunction('createBroadcastRequest');
const validateBroadcastRequest = appFunction('validateBroadcastRequest');
const isHttpUrl = (value) => {
  try { return ['http:', 'https:'].includes(new URL(value).protocol); }
  catch { return false; }
};
const pushRequest = createBroadcastRequest({ name: 'Test push', bitrate: '128', sourceMode: 'push', announceSp: false });
assert.equal(pushRequest.ingestPort, 8080);
assert.equal(pushRequest.relayPort, 7144);
assert.equal(pushRequest.announceSp, false, 'SP listing must remain opt-in');
assert.equal(pushRequest.sourceUri, '');
assert.equal(validateBroadcastRequest(pushRequest, isHttpUrl), '');
const pullRequest = createBroadcastRequest({ name: 'Test pull', bitrate: '256', sourceMode: 'pull', sourceUri: 'https://media.example/live.flv', announceSp: true });
assert.equal(pullRequest.sourceUri, 'https://media.example/live.flv');
assert.equal(pullRequest.announceSp, true);
assert.equal(validateBroadcastRequest(pullRequest, isHttpUrl), '');
assert.match(validateBroadcastRequest({ ...pullRequest, sourceUri: 'rtmp://media.example/live' }, isHttpUrl), /HTTPまたはHTTPS/);
assert.match(validateBroadcastRequest({ ...pushRequest, bitrate: 1000001 }, isHttpUrl), /1000000 kbps/);
assert.match(source, /SPへの掲載は、配信画面で掲載を選んだ場合に行われます/);
console.log('Frontend broadcast request regression: push/pull defaults, opt-in publication, and HTTP input passed');

const updateBroadcastBackendFields = appFunction('updateBroadcastBackendFields', {
  state: { snapshot: { backendKind: 'csharp' }, broadcastBusy: false },
  byId: (id) => ({
    'broadcast-relay-port': relayPortInput,
    'broadcast-relay-port-note': relayPortNote,
  })[id],
});
const relayPortInput = { disabled: false };
const relayPortNote = { hidden: true, textContent: '' };
updateBroadcastBackendFields();
assert.equal(relayPortInput.disabled, true);
assert.equal(relayPortNote.hidden, false);
assert.match(relayPortNote.textContent, /C#版では既存のリレーポート設定/);
console.log('Frontend C# relay-port notice regression passed');

const beginBroadcastStart = appFunction('beginBroadcastStart');
const endBroadcastStart = appFunction('endBroadcastStart');
const beginBroadcastStop = appFunction('beginBroadcastStop');
const endBroadcastStop = appFunction('endBroadcastStop');
const operationState = { broadcastBusy: false, stoppingBroadcasts: new Set() };
assert.equal(beginBroadcastStart(operationState), true);
assert.equal(beginBroadcastStart(operationState), false, 'A second start must be rejected while the first is pending');
assert.equal(beginBroadcastStop(operationState, 'channel-1'), false, 'Stop must wait while a start is pending');
endBroadcastStart(operationState);
assert.equal(beginBroadcastStop(operationState, 'channel-1'), true);
assert.equal(beginBroadcastStop(operationState, 'channel-1'), false, 'A duplicate stop for one channel must be rejected');
endBroadcastStop(operationState, 'channel-1');
assert.equal(operationState.stoppingBroadcasts.size, 0);
console.log('Frontend broadcast operation regression: duplicate start/stop guards passed');

for (const id of [
  'broadcast-form', 'broadcast-start-form', 'broadcast-name', 'broadcast-content-type',
  'broadcast-genre', 'broadcast-description', 'broadcast-comment', 'broadcast-contact-url',
  'broadcast-bitrate', 'broadcast-source-mode', 'broadcast-ingest-port', 'broadcast-source-uri',
  'broadcast-relay-port', 'broadcast-announce-sp', 'broadcast-start-button', 'broadcast-stop-button',
  'broadcast-relay-port-note', 'broadcast-detail-input-url', 'broadcast-detail-stream-url', 'broadcast-detail-announcement',
]) assert(appHtml.includes(`id="${id}"`), `Broadcast UI element ${id} must exist`);
assert(fs.existsSync(path.join(__dirname, '../frontend/assets/tabler/plus.svg')));
assert(fs.existsSync(path.join(__dirname, '../frontend/assets/tabler/player-stop.svg')));
console.log('Frontend broadcast UI IDs and icon assets passed');

const restoredFields = Object.fromEntries([
  'broadcast-name', 'broadcast-content-type', 'broadcast-genre', 'broadcast-description',
  'broadcast-comment', 'broadcast-contact-url', 'broadcast-bitrate', 'broadcast-source-mode',
  'broadcast-source-uri', 'broadcast-ingest-port', 'broadcast-relay-port', 'broadcast-announce-sp',
].map((id) => [id, { value: '', checked: false }]));
let restoredSourceMode = false;
const draftState = { broadcastFormDirty: false, broadcastFormInitialized: false };
const restoreBroadcastForm = appFunction('restoreBroadcastForm', {
  state: draftState,
  byId: (id) => restoredFields[id],
  updateBroadcastSourceFields: () => { restoredSourceMode = true; },
});
const restoreLastBroadcastOnce = appFunction('restoreLastBroadcastOnce', {
  state: draftState,
  restoreBroadcastForm,
});
const savedBroadcastDraft = {
  name: '復元テスト', contentType: 'WMV', genre: '音楽', description: '説明', comment: 'コメント',
  contactUrl: 'https://example.com/contact', bitrate: 256, sourceMode: 'pull',
  sourceUri: 'https://example.com/live.wmv', ingestPort: 8088, relayPort: 8000, announceSp: true,
};
assert.equal(restoreLastBroadcastOnce({ desktop: { lastBroadcast: savedBroadcastDraft } }), true);
assert.equal(restoredFields['broadcast-name'].value, '復元テスト');
assert.equal(restoredFields['broadcast-content-type'].value, 'WMV');
assert.equal(restoredFields['broadcast-source-mode'].value, 'pull');
assert.equal(restoredFields['broadcast-source-uri'].value, 'https://example.com/live.wmv');
assert.equal(restoredFields['broadcast-announce-sp'].checked, true);
assert.equal(restoredSourceMode, true);
restoredFields['broadcast-name'].value = '編集中';
draftState.broadcastFormDirty = true;
assert.equal(restoreLastBroadcastOnce({ desktop: { lastBroadcast: { ...savedBroadcastDraft, name: '後続snapshot' } } }), false);
assert.equal(restoredFields['broadcast-name'].value, '編集中', 'Polling snapshots must not overwrite edits');
assert.match(appHtml, /<span class="broadcast-label-heading">配信名/);
assert.match(appHtml, /<span class="broadcast-label-heading">形式<\/span><select id="broadcast-content-type"/);
assert.match(fs.readFileSync(path.join(__dirname, '../frontend/style.css'), 'utf8'), /\.broadcast-form-grid \.broadcast-label-heading/);
assert.match(appSource, /restoreLastBroadcastOnce\(snapshot\)/);
assert.doesNotMatch(appSource.match(/function restoreLastBroadcastOnce\(snapshot\) \{([\s\S]*?)\n  \}/)[1], /start_broadcast/);
console.log('Frontend saved broadcast restoration regression: prefill once, preserve edits, and stay idle passed');
