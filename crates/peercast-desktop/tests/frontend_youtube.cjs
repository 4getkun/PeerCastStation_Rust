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
const settingsEmbeddedCss = fs.readFileSync(path.join(root, 'settings-embedded.css'), 'utf8');

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
assert.equal(sourceMatches('twitch', 'SP'), false, 'Twitch selection excludes Yellow Page rows');
const headerCount = appFunction('ypDirectoryHeaderCount');
assert.equal(headerCount('all', 4, 2, 3), 9, 'All-source count includes YP and provider live rows');
assert.equal(headerCount('youtube', 4, 2, 3), 2, 'YouTube source count excludes YP and Twitch rows');
assert.equal(headerCount('twitch', 4, 2, 3), 3, 'Twitch source count excludes YP and YouTube rows');
assert.equal(headerCount('SP', 4, 2, 3), 4, 'A Yellow Page source count excludes provider rows');

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

for (const command of ['youtube_add', 'youtube_remove', 'youtube_set_enabled', 'youtube_refresh', 'twitch_add', 'twitch_remove', 'twitch_set_enabled', 'twitch_refresh']) {
  assert(settings.includes(`"${command}"`), `Settings must wire ${command}`);
}
for (const command of ['youtube_status', 'youtube_open', 'youtube_open_browser', 'twitch_status', 'twitch_open', 'twitch_open_browser']) assert(app.includes(`"${command}"`), `Channel display must wire ${command}`);
for (const id of ['youtube-add-form', 'youtube-subscriptions', 'youtube-refresh']) {
  assert(settingsHtml.includes(`id="${id}"`), `The settings YouTube manager must include #${id}`);
}
assert(!html.includes('YouTubeお気に入りを追加') && !html.includes('id="youtube-add-form"'), 'Channel screen must not contain YouTube registration or management UI');
assert(settingsHtml.includes('追加・監視切替・解除はすぐに保存'), 'The settings panel must explain that YouTube management saves immediately');
assert.match(settingsHtml, /role="tablist"[^>]*aria-label="イエローページの表示"/, 'YP settings need a nested accessible tablist');
assert.match(settingsHtml, /id="tab-yp"[^>]*aria-selected="true"[^>]*data-yp-subtab="yp"/, 'YP must be selected initially');
assert.match(settingsHtml, /id="tab-youtube"[^>]*aria-selected="false"[^>]*data-yp-subtab="youtube"/, 'YouTube must be available as the second tab');
assert.match(settingsHtml, /id="tab-twitch"[^>]*aria-selected="false"[^>]*data-yp-subtab="twitch"/, 'Twitch must be available as the third tab');
assert.match(settingsHtml, /id="panel-yp"[^>]*role="tabpanel"[^>]*aria-labelledby="tab-yp"/, 'YP tab must label its panel');
assert.match(settingsHtml, /id="panel-youtube"[^>]*role="tabpanel"[^>]*aria-labelledby="tab-youtube"[^>]*hidden/, 'YouTube panel starts hidden and is labelled by its tab');
assert.match(settingsHtml, /id="panel-twitch"[^>]*role="tabpanel"[^>]*aria-labelledby="tab-twitch"[^>]*hidden/, 'Twitch panel starts hidden and is labelled by its tab');
for (const id of ['twitch-add-form', 'twitch-subscriptions', 'twitch-refresh']) assert(settingsHtml.includes(`id="${id}"`), `The settings Twitch manager must include #${id}`);
assert(settingsHtml.includes('PCRPlayerで再生できます。録画はプレイヤーの右クリックから。'), 'Twitch settings explain PCRPlayer playback and player-side recording');
for (const source of ['value = "all"', 'value = "youtube"', 'value = "twitch"']) assert(app.includes(source), `The channel source selector must include ${source}`);
assert.match(settings, /function twitchSubscriptionStatus\(channel\)/, 'Twitch settings derive status from the rendered channel state');
assert.match(app, /state\.ypSource === "twitch" && !twitchChannels\(\)\.length[\s\S]*?Twitchチャンネルを登録/, 'Unregistered Twitch filter shows a Twitch-specific prompt');
assert.match(settingsHtml, /id="tab-version"[^>]*><span class="icon icon-info-circle"[^>]*><\/span><span>バージョン<\/span>/, 'The navigation label is shortened while the version panel remains separate');
assert(settingsCss.includes('overflow-x:hidden') && settingsCss.includes('font-size:12px'), 'The embedded category rail must fit a single row with compact readable labels');
assert(settingsEmbeddedCss.includes('overflow-x:hidden') && settingsEmbeddedCss.includes('font-size:12px'), 'The embedded category rail override must also fit narrow hosts');
assert.match(settings, /function activateYellowPagesSubtab\(name, focus = false\)/, 'Settings must switch YP subpanels without rebuilding their contents');
for (const key of ['ArrowLeft', 'ArrowRight', 'Home', 'End']) assert(settings.includes(`event.key === "${key}"`), `YP subtab keyboard navigation must support ${key}`);
assert(!app.includes('youtube_add') && !app.includes('youtube_remove') && !app.includes('youtube_set_enabled'), 'Channel screen must leave YouTube management actions to settings');
assert.match(settings, /name\.textContent = String\(channel\.name/ , 'Registered names must be inserted as text');
assert.match(app, /window\.dispatchEvent\(new CustomEvent\("youtube-snapshot-polled"/);
assert.match(settings, /addEventListener\("youtube-snapshot-polled"/);
assert.match(settings, /youtubeMessage = "YouTube登録を読み込みました。";[\s\S]*\$\("youtube-status"\)\.textContent = state\.youtubeMessage/ , 'The polled snapshot updates the status even when the channel list is unchanged');
assert.match(settings, /dispatchEvent\(new Event\("youtube-snapshot-requested"\)\)/);
assert.match(app, /window\.dispatchEvent\(new CustomEvent\("twitch-snapshot-polled"/);
assert.match(settings, /addEventListener\("twitch-snapshot-polled"/);
assert.match(settings, /dispatchEvent\(new Event\("twitch-snapshot-requested"\)\)/);
assert.doesNotMatch(settings, /invoke\("youtube_status"/, 'Settings must reuse the existing polling stream instead of adding another status poll');
assert.match(settingsCss, /#panel-yellow-pages\.yp-content\{grid-template-rows:max-content max-content;align-content:start\}/, 'The YP management card must not collapse the list/editor row on short windows');
assert(html.includes('id="yp-source"'), 'YouTube uses the existing source selector');
assert(css.includes('.youtube-stream-card') && !css.includes('.youtube-manager'), 'Live cards remain styled and channel-screen manager styling is removed');
assert(settingsCss.includes('.youtube-settings-manager'), 'Settings YouTube manager has scoped styling');
assert(!/youtube(?:Channels|Rows|Manager)[\s\S]{0,1800}\.innerHTML/.test(app), 'YouTube user content must not use HTML insertion');

// Tiny DOM fixture: exercise the real card renderer and delegated YP-list handlers without jsdom.
class FixtureElement {
  constructor(tagName) { this.tagName = tagName; this.children = []; this.dataset = {}; this.attributes = {}; this.parentElement = null; this.textContent = ''; this.className = ''; }
  append(...children) { children.forEach((child) => { child.parentElement = this; this.children.push(child); }); }
  replaceChildren(...children) { this.children = []; children.forEach((child) => this.append(child)); }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  focus() { this.focused = true; }
  closest(selector) {
    for (let element = this; element; element = element.parentElement) {
      if (selector === '[data-youtube-play]' && element.dataset.youtubePlay) return element;
      if (selector === '[data-youtube-browser]' && element.dataset.youtubeBrowser) return element;
      if (selector === '[data-twitch-play]' && element.dataset.twitchPlay) return element;
      if (selector === '[data-twitch-browser]' && element.dataset.twitchBrowser) return element;
      if (selector === '.youtube-stream-card[data-provider]' && element.dataset.provider) return element;
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
const playButton = liveCard.children.find((child) => child.className === 'youtube-stream-actions')?.children.find((child) => child.dataset.youtubePlay);
const browserButton = liveCard.children.find((child) => child.className === 'youtube-stream-actions')?.children.find((child) => child.dataset.youtubeBrowser);
assert(playButton && browserButton, 'A current live card offers PCRPlayer and browser playback');
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
clickHandler({ target: playButton, stopPropagation() {} });
clickHandler({ target: browserButton, stopPropagation() {} });
clickHandler({ target: liveCard, stopPropagation() {} });
clickHandler({ target: liveCard, stopPropagation() {} }); // The two clicks underlying a card double-click.
assert.deepEqual(commands, [['youtube_open', { videoId: 'video-1' }], ['youtube_open_browser', { videoId: 'video-1' }]]);
assert.equal(ypSelectionCount, 0, 'YouTube card clicks and double-clicks never select a YP row');
assert.equal(ypPlaybackCount, 0, 'YouTube card clicks and double-clicks never invoke YP playback');
const contextMenuHandler = appFunction('handleYpResultsContextMenu', {
  isEditableContextTarget: () => false, state: { ypChannels: [] }, byId: () => null,
});
let contextDefaultPrevented = false;
contextMenuHandler({ target: liveCard, preventDefault() { contextDefaultPrevented = true; } });
assert.equal(contextDefaultPrevented, false, 'YouTube cards do not open the YP right-click action menu');

const twitchFixtureState = { ypSource: 'all', ypQuery: '', twitchSnapshot: { channels: [
  { id: 'offline', name: 'Offline', enabled: true, streams: [] },
  { id: 'live', name: 'Twitch <b>name</b>', url: 'https://www.twitch.tv/live', enabled: true, streams: [{ id: 'stream-1', title: '<img src=x>', status: 'live' }] },
  { id: 'disabled', name: 'Disabled', enabled: false, streams: [{ id: 'stream-2', status: 'live' }] },
  { id: 'error', name: 'Error', enabled: true, error: 'failed', streams: [{ id: 'stream-3', status: 'live' }] },
] } };
const twitchRoot = new FixtureElement('div');
appFunction('appendTwitchRows', {
  state: twitchFixtureState,
  twitchStreamRows: appFunction('twitchStreamRows', { state: twitchFixtureState, twitchChannels: appFunction('twitchChannels', { state: twitchFixtureState }) }),
  document: fixtureDocument, appendText: fixtureAppendText,
})(twitchRoot);
assert.equal(twitchRoot.children.length, 1, 'Twitch cards show only enabled, verified live streams');
const twitchCard = twitchRoot.children[0];
assert(twitchCard.className.includes('twitch-stream-card'));
assert.equal(twitchCard.children.find((child) => child.className === 'youtube-stream-copy').children.find((child) => child.className.includes('youtube-stream-title')).textContent, '<img src=x>', 'Twitch titles remain plain text');
const twitchActions = twitchCard.children.find((child) => child.className === 'youtube-stream-actions');
assert(twitchActions.children.some((child) => child.dataset.twitchPlay === 'stream-1'));
assert(twitchActions.children.some((child) => child.dataset.twitchBrowser === 'stream-1'));
clickHandler({ target: twitchActions.children[0], stopPropagation() {} });
clickHandler({ target: twitchActions.children[1], stopPropagation() {} });
clickHandler({ target: twitchCard, stopPropagation() {} });
assert.deepEqual(commands.slice(-2), [['twitch_open', { streamId: 'stream-1' }], ['twitch_open_browser', { streamId: 'stream-1' }]]);
contextDefaultPrevented = false;
contextMenuHandler({ target: twitchCard, preventDefault() { contextDefaultPrevented = true; } });
assert.equal(contextDefaultPrevented, false, 'Twitch cards do not invoke Yellow Page right-click actions');
const twitchSearchState = { ypQuery: 'secret', twitchSnapshot: { channels: [
  { id: 'other', name: 'Other', enabled: true, streams: [{ id: 'other-1', title: 'Unrelated', status: 'live' }] },
  { id: 'match', name: 'Creator', enabled: true, streams: [{ id: 'match-1', title: 'Secret title', status: 'live' }] },
] } };
assert.deepEqual(appFunction('twitchStreamRows', {
  state: twitchSearchState, twitchChannels: appFunction('twitchChannels', { state: twitchSearchState }),
})().map(({ channel }) => channel.id), ['match'], 'Twitch live counts and rows use the shared search filter');
const providerMenuState = { providerContextStream: null, snapshot: { running: true }, busy: false };
const providerMenu = { hidden: true };
const providerPlayItem = { disabled: true, focus() {} };
let providerMenuPlaced = false;
const openProviderMenu = appFunction('handleYpResultsContextMenu', {
  isEditableContextTarget: () => false,
  state: providerMenuState,
  byId: (id) => id === 'provider-context-menu' ? providerMenu : providerPlayItem,
  closeAllContextMenus: () => { providerMenuState.providerContextStream = null; },
  placeContextMenu: (menu) => { providerMenuPlaced = menu === providerMenu; },
});
let providerContextPrevented = false;
openProviderMenu({ target: twitchCard, clientX: 12, clientY: 24, preventDefault() { providerContextPrevented = true; } });
assert.equal(providerContextPrevented, true);
assert.equal(providerMenuPlaced, true);
assert.deepEqual(providerMenuState.providerContextStream, { provider: 'twitch', streamId: 'stream-1' });
assert.equal(providerPlayItem.disabled, false);
assert(html.includes('id="provider-context-menu"') && !html.includes('id="provider-context-record"'), 'Provider menu offers no fake Yellow Page recording action');
const settingsList = new FixtureElement('div');
settingsList.ownerDocument = { activeElement: null };
settingsList.contains = () => false;
settingsList.querySelectorAll = () => [];
const twitchStatusNode = { textContent: '' };
const renderTwitchSubscriptions = settingsFunction('renderTwitchSubscriptions', {
  twitchSubscriptionStatus: settingsFunction('twitchSubscriptionStatus'),
  state: { twitchSnapshot: { channels: [
    { id: 'live', name: 'Live', enabled: true, streams: [{ id: 's1', status: 'live' }] },
    { id: 'offline', name: 'Offline', enabled: true, lastCheckedMs: 100, streams: [] },
    { id: 'disabled', name: 'Disabled', enabled: false, streams: [] },
    { id: 'error', name: 'Error', enabled: true, error: 'offline', streams: [] },
  ] }, twitchMessage: 'ready' },
  $: (id) => id === 'twitch-subscriptions' ? settingsList : twitchStatusNode,
  document: fixtureDocument, renderControls() {},
});
renderTwitchSubscriptions();
assert.deepEqual(settingsList.children.map((row) => row.children.find((child) => child.className.includes('twitch-subscription-status')).textContent), ['配信中', '配信なし', '監視無効', '確認できず'], 'Twitch settings keep offline/disabled/error status visible');
const previouslyFocusedTwitchButton = settingsList.children[0].children.find((child) => child.dataset.twitchAction === 'enabled');
settingsList.ownerDocument.activeElement = previouslyFocusedTwitchButton;
settingsList.contains = (element) => element === previouslyFocusedTwitchButton;
settingsList.querySelectorAll = () => settingsList.children.flatMap((row) => row.children.filter((child) => child.dataset.twitchAction));
renderTwitchSubscriptions();
assert(settingsList.children[0].children.find((child) => child.dataset.twitchAction === 'enabled').focused, 'Twitch list refresh restores focus to the same action and channel');
const polledSettingsState = { twitchSnapshot: null, twitchLoaded: false, twitchLoading: true, twitchMessage: '読み込み中' };
let polledSettingsRenders = 0;
const polledSettingsStatus = { textContent: '' };
const acceptTwitchPolledSnapshot = settingsFunction('acceptTwitchPolledSnapshot', {
  state: polledSettingsState,
  twitchSubscriptionFingerprint: settingsFunction('twitchSubscriptionFingerprint', {
    twitchSubscriptionStatus: settingsFunction('twitchSubscriptionStatus'),
  }),
  $: () => polledSettingsStatus,
  renderTwitchSubscriptions() { polledSettingsRenders += 1; },
  renderControls() {},
});
const twitchPoll = (overrides = {}) => ({ channels: [{ id: 'poll-channel', url: 'https://twitch.tv/channel', name: 'Channel', enabled: true, lastCheckedMs: 100, streams: [], ...overrides }] });
acceptTwitchPolledSnapshot(twitchPoll());
assert.equal(polledSettingsRenders, 1, 'The initial Twitch poll renders the registered channel');
acceptTwitchPolledSnapshot(twitchPoll({ lastCheckedMs: 200 }));
assert.equal(polledSettingsRenders, 1, 'A changed check timestamp alone does not rebuild the settings list');
acceptTwitchPolledSnapshot(twitchPoll({ lastCheckedMs: 300, streams: [{ id: 'live-1', status: 'live' }] }));
assert.equal(polledSettingsRenders, 2, 'A live/offline badge change rebuilds the settings row');
acceptTwitchPolledSnapshot(twitchPoll({ error: 'probe failed', streams: [{ id: 'live-1', status: 'live' }] }));
assert.equal(polledSettingsRenders, 3, 'A channel error change rebuilds the settings row');
acceptTwitchPolledSnapshot(twitchPoll({ error: 'probe failed again', lastCheckedMs: 400, streams: [{ id: 'live-1', status: 'live' }] }));
assert.equal(polledSettingsRenders, 3, 'Error text and check timestamp changes alone do not rebuild the row');
acceptTwitchPolledSnapshot(twitchPoll({ lastCheckedMs: 500 }));
assert.equal(polledSettingsRenders, 4, 'Recovery to offline updates the settings status');
acceptTwitchPolledSnapshot(twitchPoll({ lastCheckedMs: 600 }));
assert.equal(polledSettingsRenders, 4, 'An unchanged offline status does not rebuild the settings list');

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

async function testTwitchStatusPolling() {
  const snapshot = { channels: [{ id: 'twitch-channel', enabled: true, streams: [{ id: 'stream-1', status: 'live' }] }] };
  const pollState = { twitchQueryBusy: false, twitchSnapshot: null, twitchRevision: 0 };
  const calls = [];
  const polled = [];
  let renders = 0;
  const poll = appFunction('fetchTwitchStatus', {
    state: pollState,
    document: { visibilityState: 'visible' },
    window: { dispatchEvent(event) { polled.push(event); } },
    CustomEvent: class { constructor(type, options) { this.type = type; this.detail = options?.detail; } },
    youtubeSnapshotsEqual: appFunction('youtubeSnapshotsEqual'),
    invoke: async (command, args) => { calls.push([command, args]); return snapshot; },
    currentView: () => 'yp', renderYpDirectory: () => { renders += 1; }, errorText: String,
  });
  await poll();
  assert.deepEqual(calls, [['twitch_status', {}]]);
  assert.equal(renders, 1, 'A new Twitch status snapshot refreshes the directory');
  assert.equal(polled[0].type, 'twitch-snapshot-polled', 'Settings receive cached snapshots from the shared poll');
  assert.equal(pollState.twitchSnapshot, snapshot);
}

async function testProviderContextActions() {
  const calls = [];
  const managed = [];
  const actionState = {
    providerContextStream: { provider: 'twitch', streamId: 'stream-1' },
    settingsView: { openYellowPagesSubtab(provider) { managed.push(provider); } },
  };
  const action = appFunction('runProviderContextAction', {
    state: actionState,
    closeAllContextMenus: () => { actionState.providerContextStream = null; },
    requestView: async () => true, currentView: () => 'settings',
    invoke: async (command, args) => { calls.push([command, args]); },
    setNotice() {}, errorText: String,
  });
  await action('play');
  actionState.providerContextStream = { provider: 'twitch', streamId: 'stream-1' };
  await action('browser');
  actionState.providerContextStream = { provider: 'youtube', streamId: 'video-1' };
  await action('play');
  actionState.providerContextStream = { provider: 'youtube', streamId: 'video-1' };
  await action('browser');
  actionState.providerContextStream = { provider: 'twitch', streamId: 'stream-1' };
  await action('manage');
  assert.deepEqual(calls, [
    ['twitch_open', { streamId: 'stream-1' }],
    ['twitch_open_browser', { streamId: 'stream-1' }],
    ['youtube_open', { videoId: 'video-1' }],
    ['youtube_open_browser', { videoId: 'video-1' }],
  ]);
  assert.deepEqual(managed, ['twitch']);
  assert.equal(calls.some(([command]) => command === 'enqueue_recording'), false, 'Provider menu never invokes YP recording');
}

testYoutubeStatusPolling().then(testTwitchStatusPolling).then(testProviderContextActions).then(() => {
  console.log('Frontend YouTube regression: moved management, live rows, unchanged-poll render skipping, changed-poll rendering, and refresh gating passed');
}).catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
