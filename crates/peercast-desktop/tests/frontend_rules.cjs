const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

// Exercise the shipped pure helper without adding browser dependencies.
const source = fs.readFileSync(path.join(__dirname, '../frontend/settings.js'), 'utf8');
const settingsHtml = fs.readFileSync(path.join(__dirname, '../frontend/settings.html'), 'utf8');
assert.match(settingsHtml, /<details id="saved-rules-disclosure" class="saved-rules-disclosure">\s*<summary>/,
  'Saved channel rules must start in the native, collapsed disclosure');
assert.match(settingsHtml, /<summary>[\s\S]*id="rules-count"[\s\S]*<\/summary>[\s\S]*id="rules-list"/,
  'The saved rule names must stay inside the user-controlled disclosure');
assert.match(source, /state\.selectedRule = null;\s*state\.recording =/,
  'Loading saved preferences must not preselect and expose the first rule');
assert.match(source, /\$\("saved-rules-disclosure"\)\.addEventListener\("toggle", handleRuleDisclosureToggle\)/,
  'The native disclosure close action must clear any selected rule');
assert.match(source, /state\.selectedRule = state\.rules\.length - 1; \$\("saved-rules-disclosure"\)\.open = true;/,
  'Explicit rule creation must reveal the list and select the new rule');
console.log('Frontend channel-rule disclosure regression: collapsed load, hidden names on return, explicit add passed');
function settingsFunction(name, bindings = {}) {
  const match = source.match(new RegExp(`  function ${name}\\([^\\n]*\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, `The ${name} settings helper must exist`);
  return new Function(...Object.keys(bindings), `return (${match[0].trim()})`)(...Object.values(bindings));
}
{
  const ruleState = { selectedRule: 0, dirty: false };
  const rule = { name: 'Before edit' };
  const editor = { name: 'Edited NG name' };
  let commits = 0;
  let syncs = 0;
  const clearRuleSelection = settingsFunction('clearRuleSelection', {
    state: ruleState,
    commitRuleEditor: () => { commits += 1; rule.name = editor.name; },
    syncRuleFields: () => { syncs += 1; },
  });
  const disclosure = { open: false };
  const handleToggle = settingsFunction('handleRuleDisclosureToggle', {
    $: () => disclosure,
    clearRuleSelection,
  });
  handleToggle();
  assert.equal(rule.name, 'Edited NG name', 'Closing the disclosure must commit the active editor into the existing rule');
  assert.equal(ruleState.selectedRule, null, 'Closing the disclosure must clear the selection so the editor does not expose its name');
  assert.equal(commits, 1);
  assert.equal(syncs, 1);
  assert.equal(ruleState.dirty, false, 'Disclosure close must not mark settings dirty when no other action did so');

  const controls = { 'saved-rules-disclosure': { open: true }, 'page-title': {}, 'page-description': {} };
  const root = { querySelectorAll: () => [] };
  const activate = settingsFunction('activate', {
    meta: { appearance: ['外観', '説明'] },
    state: { selectedRule: null, updateViewActive: false },
    clearRuleSelection: () => {},
    $: (id) => controls[id],
    root,
    ensureUpdateStatus: () => {},
    stopUpdatePolling: () => {},
  });
  activate('appearance');
  assert.equal(controls['saved-rules-disclosure'].open, false,
    'Leaving the rules page must collapse the list even when the user never selected a row');
}
console.log('Frontend channel-rule disclosure behavior: manual close clears selection and leaving page always collapses');
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
assert.deepEqual(normalizeWindowPreferences(), { width: 1600, height: 900, presets: [], logicalSize: false, minimizeToTray: false, launchAtStartup: false });
const originalWindow = { width: 1486, height: 1059, logicalSize: true, presets: [{ id: 'p1', name: '<img src=x onerror=alert(1)>', width: 3840, height: 2160 }] };
const normalizedWindow = normalizeWindowPreferences(originalWindow);
assert.deepEqual(normalizedWindow, { ...originalWindow, presets: [{ ...originalWindow.presets[0] }], minimizeToTray: false, launchAtStartup: false }, 'Saved user presets and DPI mode must load');
assert.notEqual(normalizedWindow.presets, originalWindow.presets, 'Normalization must clone the presets array');
assert.notEqual(normalizedWindow.presets[0], originalWindow.presets[0], 'Normalization must clone preset entries');
assert.deepEqual(normalizeWindowPreferences(JSON.parse(JSON.stringify(normalizedWindow))), normalizedWindow, 'Named presets must survive save/load serialization');
assert.deepEqual(normalizeWindowPreferences({ width: 1486, height: 1059, minimizeToTray: true }), { width: 1486, height: 1059, presets: [], logicalSize: false, minimizeToTray: true, launchAtStartup: false }, 'Saved dimensions must remain intact');
const presets = ['1280x720', '1366x768', '1600x900', '1920x1080', '2560x1440', '3840x2160'];
for (const preset of presets) assert(settingsHtml.includes(`<option value="${preset}">`), `The standard preset ${preset} must be available`);
assert.match(settingsHtml, /<select id="window-preset"[\s\S]*<option value="">手動で指定<\/option>[\s\S]*<optgroup id="window-custom-presets" label="登録したプリセット"/);
assert.match(settingsHtml, /id="window-preset-name" type="text" maxlength="40"/);
assert.match(settingsHtml, /id="window-preset-add"[^>]*>現在のサイズを登録/);
assert.match(settingsHtml, /id="window-preset-remove"[^>]*>選択した登録を削除/);
assert.deepEqual(parseWindowPreset('1366x768'), { width: 1366, height: 768 });
assert.deepEqual(parseWindowPreset('1486x1059'), { width: 1486, height: 1059 }, 'Nonstandard preset values must remain selectable');
assert.equal(isValidWindowSize(1280, 720), true);
assert.equal(isValidWindowSize(7680, 4320), true);
assert.equal(isValidWindowSize(1279, 720), false);
assert.equal(isValidWindowSize(7681, 4320), false);
assert.equal(isValidWindowSize(1600, 4321), false);
assert.match(settingsHtml, /id="window-width" type="number" min="1280" max="7680"[^>]*value="1600"/);
assert.match(settingsHtml, /id="window-height" type="number" min="720" max="4320"[^>]*value="900"/);
assert.match(settingsHtml, /<input id="window-logical-size" type="checkbox"><span>Windowsの表示倍率に合わせる（論理サイズ）/);
assert.match(source, /\$\("window-logical-size"\)\.addEventListener\("change", updateWindowLogicalSize\)/,
  'Changing the checkbox must update the saved window preferences');
assert.match(source, /\$\("window-logical-size"\)\.checked = state\.window\.logicalSize/,
  'Loading saved preferences must synchronize the checkbox');
const logicalWindowState = { window: { width: 1486, height: 1059, logicalSize: false }, dirty: false };
const logicalWindowControl = { checked: true };
const updateWindowLogicalSize = settingsFunction('updateWindowLogicalSize', {
  state: logicalWindowState,
  $: () => logicalWindowControl,
  setDirty: () => { logicalWindowState.dirty = true; },
});
updateWindowLogicalSize();
assert.deepEqual(logicalWindowState.window, { width: 1486, height: 1059, logicalSize: true }, 'Enabling logical size must preserve physical dimension values');
assert.equal(logicalWindowState.dirty, true, 'Changing logical-size mode must mark the preferences for saving');
logicalWindowState.dirty = false;
logicalWindowControl.checked = false;
updateWindowLogicalSize();
assert.deepEqual(logicalWindowState.window, { width: 1486, height: 1059, logicalSize: false }, 'Disabling logical size must preserve the same dimensions');
assert.equal(logicalWindowState.dirty, true);
assert.match(source, /window:\s*\{ \.\.\.state\.window \}/, 'Saved preferences must include the logical-size setting');
const addWindowPreset = settingsFunction('addWindowPreset', { isValidWindowSize });
const removeWindowPreset = settingsFunction('removeWindowPreset');
const applyWindowPreset = settingsFunction('applyWindowPreset');
const base = { width: 1920, height: 1080, logicalSize: true, presets: [] };
const namedPresetResult = addWindowPreset(base, '  4K <img>  ', 'preset-4k', 3840, 2160);
assert.equal(namedPresetResult.error, undefined, 'A valid named preset within the maximum size must be accepted');
assert.deepEqual(namedPresetResult.window.presets, [{ id: 'preset-4k', name: '4K <img>', width: 3840, height: 2160 }]);
assert.deepEqual(base.presets, [], 'Adding must use a fresh array and preserve the source settings');
const applied = applyWindowPreset(namedPresetResult.window, namedPresetResult.preset);
assert.deepEqual([applied.width, applied.height, applied.logicalSize], [3840, 2160, true], 'Applying a named size must leave global DPI mode unchanged');
const removed = removeWindowPreset(applied, 'preset-4k');
assert.deepEqual([removed.width, removed.height, removed.presets], [3840, 2160, []], 'Removing a selected preset must preserve current dimensions');
assert.notEqual(removed.presets, applied.presets, 'Removing must use a fresh presets array');
assert.match(addWindowPreset(base, ' '.repeat(1), 'empty').error, /1〜40文字/);
assert.match(addWindowPreset(base, 'x'.repeat(41), 'long').error, /1〜40文字/);
assert.match(addWindowPreset(base, 'bad size', 'bad', 1279, 720).error, /サイズ/);
assert.match(addWindowPreset({ ...base, presets: [{ id: 'old', name: 'Custom', width: 1280, height: 720 }] }, ' custom ', 'new').error, /同じ名前/);
assert.match(addWindowPreset(base, 'Custom', 'i'.repeat(81)).error, /ID/);
assert.match(addWindowPreset({ ...base, presets: Array.from({ length: 32 }, (_, i) => ({ id: `${i}`, name: `${i}`, width: 1280, height: 720 })) }, '33rd', '33').error, /32件/);
assert.match(source, /option\.textContent = preset\.name/, 'User-controlled preset names must be assigned as text, never HTML');
assert.match(source, /const discard = \$\("discard-dialog"\)\.returnValue === "discard";[\s\S]*if \(discard\)[\s\S]*applySnapshot\(state\.snapshot, true\)/, 'Cancel and discard must reload the saved snapshot, including presets');
assert.match(source, /state\.window = \{ \.\.\.snapshotWindow \}/, 'Loading/canceling must replace unsaved preset arrays with normalized snapshot state');
console.log('Frontend named window presets: validation, immutable add/remove, DPI preservation, safe labels, serialization and discard checks passed');

const appSource = fs.readFileSync(path.join(__dirname, '../frontend/app.js'), 'utf8');
const appHtml = fs.readFileSync(path.join(__dirname, '../frontend/index.html'), 'utf8');
for (const id of ['yp-context-menu', 'yp-context-play', 'yp-context-record', 'yp-context-favorite', 'yp-context-ng']) {
  assert(appHtml.includes(`id="${id}"`), `The channel context menu must include #${id}`);
}
assert.doesNotMatch(appHtml, /id="selected-play"/, 'The detail pane must not repeat the list play action');
assert.match(appSource, /play\.className = "yp-play-button"/, 'The list-row play action must remain available');
assert.match(appSource, /addEventListener\("contextmenu"/,
  'Right-clicking a channel must open the direct action menu');
assert.match(appSource, /invoke\("set_channel_preference"/,
  'Favorite and NG actions must be persisted without navigating to settings');
assert(appHtml.includes('id="yp-context-favorite-label"'), 'Favorite context action must have a replaceable label');
function appFunction(name, bindings = {}) {
  const match = appSource.match(new RegExp(`  function ${name}\\([^\\n]*\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, `The ${name} helper must exist`);
  return new Function(...Object.keys(bindings), `return (${match[0].trim()})`)(...Object.values(bindings));
}
const isDirectFavoriteRule = appFunction('isDirectFavoriteRule');
const directFavoriteRuleIndex = appFunction('directFavoriteRuleIndex', { isDirectFavoriteRule });
const removeDirectFavoriteRule = appFunction('removeDirectFavoriteRule', { isDirectFavoriteRule });
const directQuickFavorite = {
  id: 'quick-a1', name: 'Artist', enabled: true, pattern: '^Artist$', fields: ['name'],
  useRegex: true, normalizeWidth: true, matchCase: false, favorite: true, hidden: false,
  mute: false, textColor: null, backgroundColor: null, soundPath: null,
};
const broadNg = { ...directQuickFavorite, id: 'broad-ng', name: 'All Artist streams', pattern: '^Artist.*$', favorite: false, hidden: true };
const exactNg = { ...directQuickFavorite, id: 'manual-ng', favorite: false, hidden: true };
const anotherExactFavorite = { ...directQuickFavorite, id: 'settings-exact', name: 'Artist' };
const ypScopedRule = { ...broadNg, id: 'other-yp', name: 'Other page', pattern: '^Other Page$', fields: ['yellowPage'] };
assert.equal(directFavoriteRuleIndex([directQuickFavorite], 'Artist'), 0, 'A directly created exact-name favorite should be recognized');
assert.equal(directFavoriteRuleIndex([broadNg], 'Artist'), -1, 'A broad pattern must never be treated as the direct favorite');
assert.equal(directFavoriteRuleIndex([anotherExactFavorite], 'Artist'), -1, 'A settings rule with an exact pattern must remain user-owned');
assert.equal(directFavoriteRuleIndex([exactNg], 'Artist'), -1, 'An exact NG rule must not be treated as a favorite');
const duplicateDirectFavorite = { ...directQuickFavorite, id: 'quick-a2' };
const overlappingRules = [broadNg, directQuickFavorite, duplicateDirectFavorite, exactNg, anotherExactFavorite, ypScopedRule];
assert.equal(directFavoriteRuleIndex(overlappingRules, 'Artist'), 1, 'A direct favorite remains identifiable when broad favorite/NG rules also match');
const afterDirectFavoriteRemoval = removeDirectFavoriteRule(overlappingRules, 'Artist');
assert.deepEqual(afterDirectFavoriteRemoval.map(({ id }) => id), ['broad-ng', 'manual-ng', 'settings-exact', 'other-yp'],
  'One removal must remove duplicate direct favorites while preserving broad, NG, manual exact, and other-Yellow-Page rules');
assert.equal(directFavoriteRuleIndex([directQuickFavorite, { ...directQuickFavorite, id: 'quick-a2' }], 'Artist'), 0,
  'Existing direct exact entries prevent another direct favorite registration');
const sameNameAcrossPages = [{ name: 'Artist', yellowPage: 'SP' }, { name: 'Artist', yellowPage: 'P@' }];
assert.deepEqual(sameNameAcrossPages.map(({ name }) => directFavoriteRuleIndex([directQuickFavorite], name)), [0, 0],
  'The existing direct rule is name-wide across Yellow Pages');
assert.equal(directFavoriteRuleIndex([directQuickFavorite], 'Ａｒｔｉｓｔ'), 0,
  'Names are normalized before matching the direct exact-name rule');
console.log('Frontend direct favorite removal regression: overlapping rule preservation and same-name pages passed');
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
assert.match(appHtml, /class="yp-heading-channel"[\s\S]*id="yp-header-count"/);
assert.match(appHtml, /id="status-count-divider" class="status-divider"/);
assert.match(appSource, /renderYpHeaderCount\(filtered\)/);
const ypHeaderCount = { textContent: '' };
const renderYpHeaderCount = appFunction('renderYpHeaderCount', {
  state: { ypSource: 'all' },
  countableYpChannelCount,
  text: (_id, value) => { ypHeaderCount.textContent = value; },
});
renderYpHeaderCount(countFixtures);
assert.equal(ypHeaderCount.textContent, '1 件の配信', 'The channel header count must exclude YouTube detail counts');
const youtubeOnlyHeader = appFunction('renderYpHeaderCount', {
  state: { ypSource: 'youtube' },
  countableYpChannelCount,
  text: (_id, value) => { ypHeaderCount.textContent = value; },
});
youtubeOnlyHeader(countFixtures);
assert.equal(ypHeaderCount.textContent, '0 件の配信', 'The YouTube source must not retain the prior channel count or add a YouTube subcount');
const footerCount = { hidden: false };
const footerDivider = { hidden: false };
const visibilityById = (id) => id === 'yp-count' ? footerCount : footerDivider;
appFunction('syncYpFooterCountVisibility', { currentView: () => 'yp', byId: visibilityById })();
assert.equal(footerCount.hidden, true);
assert.equal(footerDivider.hidden, true, 'The divider must hide with the duplicate YP footer count');
appFunction('syncYpFooterCountVisibility', { currentView: () => 'connections', byId: visibilityById })();
assert.equal(footerCount.hidden, false, 'Other views must restore the status footer count');
assert.equal(footerDivider.hidden, false);
console.log('Frontend YP count regression: count rules, search/source header updates, and footer visibility passed');

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
