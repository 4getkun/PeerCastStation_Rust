const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const root = path.join(__dirname, '../frontend');
const app = fs.readFileSync(path.join(root, 'app.js'), 'utf8');
const html = fs.readFileSync(path.join(root, 'index.html'), 'utf8');
const css = fs.readFileSync(path.join(root, 'style.css'), 'utf8');
const settings = fs.readFileSync(path.join(root, 'settings.js'), 'utf8');
const settingsHtml = fs.readFileSync(path.join(root, 'settings.html'), 'utf8');
const settingsCss = fs.readFileSync(path.join(root, 'settings.css'), 'utf8');

function appFunction(name, bindings = {}) {
  const match = app.match(new RegExp(`  (?:async )?function ${name}\\([^\\n]*\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, `The ${name} helper must exist`);
  return new Function(...Object.keys(bindings), `return (${match[0].trim()})`)(...Object.values(bindings));
}
function settingsFunction(name, bindings = {}) {
  const match = settings.match(new RegExp(`  (?:async )?function ${name}\\([^\\n]*\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, `The ${name} settings helper must exist`);
  return new Function(...Object.keys(bindings), `return (${match[0].trim()})`)(...Object.values(bindings));
}

const sourceMatches = appFunction('youtubeSourceMatches');
assert.equal(sourceMatches('all', 'SP'), true);
assert.equal(sourceMatches('SP', 'SP'), true);
assert.equal(sourceMatches('SP', 'P@'), false);
assert.equal(sourceMatches('youtube', 'SP'), false, 'YouTube selection excludes Yellow Page rows');

const stateLabel = appFunction('youtubeStateLabel');
assert.equal(stateLabel({ enabled: true, lastCheckedMs: 100 }, { status: 'live' }), '配信中');
assert.equal(stateLabel({ enabled: true, lastCheckedMs: 100 }, { status: 'upcoming' }), '予約');
assert.equal(stateLabel({ enabled: true, error: 'refresh failed' }, { status: 'live' }), '確認できず', 'A stale cached live result must not mask a channel error');
assert.equal(stateLabel({ enabled: false, lastCheckedMs: 100 }, { status: 'live' }), '監視無効', 'Disabled channels must not be labeled live');
const refreshState = settingsFunction('youtubeRefreshState');
const hasMonitoredChannel = { channels: [{ id: 'a', enabled: true }] };
assert.deepEqual(refreshState({ ...hasMonitoredChannel, refreshing: true, lastRefreshMs: 10_000 }, 50_000), { disabled: true, remainingSeconds: 0 });
assert.deepEqual(refreshState({ ...hasMonitoredChannel, refreshing: false, lastRefreshMs: 50_000 }, 79_999), { disabled: true, remainingSeconds: 1 });
assert.deepEqual(refreshState({ ...hasMonitoredChannel, refreshing: false, lastRefreshMs: 50_000 }, 80_000), { disabled: false, remainingSeconds: 0 });
assert.deepEqual(refreshState({ channels: [], refreshing: false, lastRefreshMs: null }, 80_000), { disabled: true, remainingSeconds: 0 }, 'Refreshing with no registered channels is disabled');
assert.equal(stateLabel({ enabled: true, error: 'offline' }, null), '確認できず');
assert.equal(stateLabel({ enabled: true, lastCheckedMs: null }, null), '確認待ち');
assert.equal(stateLabel({ enabled: true, lastCheckedMs: 100 }, null), '配信なし');
assert.equal(stateLabel({ enabled: false, lastCheckedMs: 100 }, null), '監視無効');

const rowsState = {
  ypQuery: '',
  youtubeSnapshot: { channels: [
    { id: 'live', name: '配信者A', enabled: true, streams: [{ id: 'v1', title: 'ライブ', status: 'live' }] },
    { id: 'upcoming', name: '配信者B', enabled: true, streams: [{ id: 'v2', title: '予約枠', status: 'upcoming' }] },
    { id: 'disabled', name: '停止中', enabled: false, streams: [{ id: 'v3', title: '隠す配信', status: 'live' }] },
    { id: 'invalid', name: '対象外', enabled: true, streams: [{ id: 'v4', title: '終了', status: 'ended' }] },
  ] },
};
const rows = appFunction('youtubeStreamRows', { state: rowsState, youtubeChannels: appFunction('youtubeChannels', { state: rowsState }) });
assert.deepEqual(rows().map(({ channel, stream }) => [channel.id, stream?.status || null]), [
  ['live', 'live'], ['upcoming', 'upcoming'], ['disabled', null], ['invalid', null],
]);
const searchState = {
  ypQuery: '予約',
  youtubeSnapshot: { channels: [
    { id: 'a', name: '配信者A', enabled: true, streams: [{ id: 'a1', title: 'ライブ', status: 'live' }] },
    { id: 'b', name: '配信者B', enabled: true, streams: [{ id: 'b1', title: '予約枠', status: 'upcoming' }] },
  ] },
};
const searched = appFunction('youtubeStreamRows', { state: searchState, youtubeChannels: appFunction('youtubeChannels', { state: searchState }) });
assert.deepEqual(searched().map(({ channel }) => channel.id), ['b'], 'Search matches stream titles without exposing unrelated streams');

for (const command of ['youtube_add', 'youtube_remove', 'youtube_set_enabled', 'youtube_refresh']) {
  assert(settings.includes(`"${command}"`), `Settings must wire ${command}`);
}
for (const command of ['youtube_status', 'youtube_open']) assert(app.includes(`"${command}"`), `Channel display must wire ${command}`);
for (const id of ['youtube-add-form', 'youtube-subscriptions', 'youtube-refresh']) {
  assert(settingsHtml.includes(`id="${id}"`), `The settings YouTube manager must include #${id}`);
}
assert(!html.includes('YouTubeお気に入りを追加') && !html.includes('id="youtube-add-form"'), 'Channel screen must not contain YouTube registration or management UI');
assert(settingsHtml.includes('追加・監視切替・解除はすぐに保存'), 'The settings panel must explain that YouTube management saves immediately');
assert(!app.includes('youtube_add') && !app.includes('youtube_remove') && !app.includes('youtube_set_enabled'), 'Channel screen must leave YouTube management actions to settings');
assert.match(settings, /name\.textContent = String\(channel\.name/ , 'Registered names must be inserted as text');
assert.match(app, /window\.dispatchEvent\(new CustomEvent\("youtube-snapshot-polled"/);
assert.match(settings, /addEventListener\("youtube-snapshot-polled"/);
assert.match(settings, /youtubeMessage = "YouTube登録を読み込みました。";[\s\S]*\$\("youtube-status"\)\.textContent = state\.youtubeMessage/ , 'The polled snapshot updates the status even when the channel list is unchanged');
assert.match(settings, /dispatchEvent\(new Event\("youtube-snapshot-requested"\)\)/);
assert.doesNotMatch(settings, /invoke\("youtube_status"/, 'Settings must reuse the existing polling stream instead of adding another status poll');
assert.match(settingsCss, /#panel-yellow-pages\.yp-content\{grid-template-rows:max-content max-content;align-content:start\}/, 'The YP management card must not collapse the list/editor row on short windows');
assert(html.includes('id="yp-source"'), 'YouTube uses the existing source selector');
assert(css.includes('.youtube-stream-card') && !css.includes('.youtube-manager'), 'Live cards remain styled and channel-screen manager styling is removed');
assert(settingsCss.includes('.youtube-settings-manager'), 'Settings YouTube manager has scoped styling');
assert(!/youtube(?:Channels|Rows|Manager)[\s\S]{0,1800}\.innerHTML/.test(app), 'YouTube user content must not use HTML insertion');

// Tiny DOM fixture: exercise the real card renderer and delegated YP-list handlers without jsdom.
class FixtureElement {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.dataset = {}; this.attributes = {}; this.parentElement = null; this.textContent = ''; this.className = ''; }
  append(child) { child.parentElement = this; this.children.push(child); }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  closest(selector) {
    for (let element = this; element; element = element.parentElement) {
      if (selector === '[data-youtube-video]' && element.dataset.youtubeVideo) return element;
      if (selector === '.yp-row' && element.className.split(/\s+/).includes('yp-row')) return element;
      if (selector === '.yp-play-button' && element.className.split(/\s+/).includes('yp-play-button')) return element;
      if (selector.startsWith('input') && element.tagName === 'input') return element;
    }
    return null;
  }
}
const fixtureDocument = { createElement: (tag) => new FixtureElement(tag) };
const fixtureState = { ypSource: 'all', ypQuery: '', youtubeSnapshot: { channels: [
  { id: 'error', name: '確認失敗', enabled: true, error: 'offline', lastCheckedMs: 1, streams: [{ id: 'stale', title: '古い配信', status: 'live' }] },
  { id: 'live', name: '配信者 <script>alert(1)</script>', enabled: true, lastCheckedMs: 2, streams: [{ id: 'video-1', title: '<img src=x onerror=alert(1)>', status: 'live' }] },
] } };
const fixtureChannels = appFunction('youtubeChannels', { state: fixtureState });
const fixtureStreams = appFunction('youtubeStreamRows', { state: fixtureState, youtubeChannels: fixtureChannels });
const fixtureAppendText = appFunction('appendText', { document: fixtureDocument });
const fixtureRoot = new FixtureElement('div');
appFunction('appendYoutubeRows', {
  state: fixtureState, youtubeStreamRows: fixtureStreams, document: fixtureDocument,
  appendText: fixtureAppendText, youtubeStateLabel: stateLabel,
})(fixtureRoot);
assert.equal(fixtureRoot.children.length, 2);
const errorCard = fixtureRoot.children[0];
assert(!errorCard.className.includes('is-live'), 'Cached live status must not style an errored channel as live');
assert.equal(errorCard.children.find((child) => child.className.includes('youtube-stream-status')).textContent, '確認できず');
const liveCard = fixtureRoot.children[1];
const liveCopy = liveCard.children.find((child) => child.className.includes('youtube-stream-copy'));
assert.equal(liveCopy.children.find((child) => child.className.includes('youtube-stream-title')).textContent, '<img src=x onerror=alert(1)>', 'Titles remain plain text in the rendered DOM');
const openButton = liveCard.children.find((child) => child.dataset.youtubeVideo);
assert(openButton, 'A current live card offers browser playback');
const commands = [];
let ypSelectionCount = 0;
let ypPlaybackCount = 0;
const clickHandler = appFunction('handleYpResultsClick', {
  state: { ...fixtureState, ypChannels: [] },
  invoke: (command, args) => { commands.push([command, args]); return Promise.resolve(); },
  text: () => {}, errorText: String,
  playYpChannel: () => { ypPlaybackCount += 1; },
  selectYpChannel: () => { ypSelectionCount += 1; },
});
const click = { target: openButton, stopPropagation() {} };
clickHandler(click);
clickHandler({ target: liveCard, stopPropagation() {} });
clickHandler({ target: liveCard, stopPropagation() {} }); // The two clicks underlying a card double-click.
assert.deepEqual(commands, [['youtube_open', { videoId: 'video-1' }]]);
assert.equal(ypSelectionCount, 0, 'YouTube card clicks and double-clicks never select a YP row');
assert.equal(ypPlaybackCount, 0, 'YouTube card clicks and double-clicks never invoke YP playback');
const contextMenuHandler = appFunction('handleYpResultsContextMenu', {
  isEditableContextTarget: () => false, state: { ypChannels: [] }, byId: () => null,
});
let contextDefaultPrevented = false;
contextMenuHandler({ target: liveCard, preventDefault() { contextDefaultPrevented = true; } });
assert.equal(contextDefaultPrevented, false, 'YouTube cards do not open the YP right-click action menu');

async function testYoutubeStatusPolling() {
  const sameSnapshot = {
    channels: [{ id: 'channel-a', enabled: true, streams: [] }],
    intervalMinutes: 5, refreshing: false, lastRefreshMs: null, nextRefreshMs: 100,
  };
  const equalSnapshot = {
    nextRefreshMs: 100, lastRefreshMs: null, refreshing: false, intervalMinutes: 5,
    channels: [{ streams: [], enabled: true, id: 'channel-a' }],
  };
  const changedSnapshot = {
    channels: [{ id: 'channel-a', enabled: true, streams: [{ id: 'v-live', title: 'live', status: 'live' }] }],
    intervalMinutes: 5, refreshing: false, lastRefreshMs: null, nextRefreshMs: 100,
  };
  const pollState = { youtubeQueryBusy: false, youtubeSnapshot: null, youtubeRevision: 0 };
  let response = sameSnapshot;
  let listRenders = 0;
  const polledSnapshots = [];
  const poll = appFunction('fetchYoutubeStatus', {
    state: pollState,
    document: { visibilityState: 'visible' },
    window: { dispatchEvent(event) { polledSnapshots.push(event.detail); } },
    youtubeSnapshotsEqual: appFunction('youtubeSnapshotsEqual'),
    invoke: async () => response,
    currentView: () => 'yp',
    renderYpDirectory: () => { listRenders += 1; },
  });
  await poll();
  assert.equal(listRenders, 1, 'The first snapshot populates the channel list');
  assert.equal(polledSnapshots.length, 1, 'Settings receives the existing poll snapshot for its cooldown and refreshing state');
  response = equalSnapshot;
  await poll();
  assert.equal(listRenders, 1, 'An equivalent snapshot with reordered JSON keys skips the list render');
  assert.equal(polledSnapshots.length, 2, 'Settings receives unchanged poll snapshots to update cooldown state');
  response = changedSnapshot;
  await poll();
  assert.equal(listRenders, 2, 'A changed stream snapshot redraws the channel list');
}

testYoutubeStatusPolling().then(() => {
  console.log('Frontend YouTube regression: moved management, live rows, unchanged-poll render skipping, changed-poll rendering, and refresh gating passed');
}).catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
