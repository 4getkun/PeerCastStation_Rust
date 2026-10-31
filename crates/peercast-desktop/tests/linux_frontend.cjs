const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const app = fs.readFileSync(path.join(__dirname, '../frontend/app.js'), 'utf8');
const settings = fs.readFileSync(path.join(__dirname, '../frontend/settings.js'), 'utf8');
const html = fs.readFileSync(path.join(__dirname, '../frontend/settings.html'), 'utf8');
function extract(source, name, bindings) {
  const match = source.match(new RegExp(`  (?:async )?function ${name}\\([^\\n]*\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, name);
  return new Function(...Object.keys(bindings), `return (${match[0].trim()})`)(...Object.values(bindings));
}
const linux = { platform: 'linux', liveServices: false, updater: false, externalPlayer: true };
(async () => {
  for (const [source, names] of [
    [app, ['checkForApplicationUpdate', 'scheduleApplicationUpdateChecks', 'fetchYoutubeStatus', 'fetchTwitchStatus']],
    [settings, ['loadYoutubeSettings', 'loadTwitchSettings', 'youtubeSettingsMutation', 'twitchSettingsMutation', 'ensureUpdateStatus', 'runUpdateAction', 'scheduleUpdatePoll', 'pollUpdateStatus']],
  ]) {
    for (const name of names) {
      let calls = 0;
      await extract(source, name, { state: { capabilities: linux }, invoke: () => { calls++; throw Error('Unexpected IPC'); } })();
      assert.equal(calls, 0, `${name}: no optional-service IPC on Linux, including repeated direct invocation`);
    }
  }
  const nodes = new Map();
  const $ = id => { if (!nodes.has(id)) nodes.set(id, {}); return nodes.get(id); };
  extract(settings, 'renderUpdaterControls', { state: { capabilities: linux }, $ })();
  assert.match($('update-status').textContent, /利用できません/);
  for (const id of ['update-check', 'update-download', 'update-install', 'update-cancel']) {
    assert.equal($(id).disabled, true);
    assert.equal($(id).hidden, true);
  }
  extract(settings, 'activateYellowPagesSubtab', { state: { capabilities: linux } })('youtube');
  extract(settings, 'activateYellowPagesSubtab', { state: { capabilities: linux } })('twitch');
  const declaration = settings.match(/  const defaultArguments = ([^\n]+);/)[1];
  const defaultArguments = capabilities => new Function('platformCapabilities', `return ${declaration}`)(capabilities);
  assert.equal(defaultArguments(linux), '"<stream/>"');
  assert.equal(defaultArguments({ platform: 'windows' }), '"<stream/>" "<channelname/>" "<contact/>"');
  assert.match(html, /id="linux-services-note"[^>]*hidden>このプラットフォームではYouTube\/Twitchは未対応/);
  for (const capabilities of [linux, { ...linux, liveServices: true, liveDependencies: { monitorReady: false, playbackReady: false, reasonCode: 'missing', message: 'missing' } }, { platform: 'windows', liveServices: true, updater: true }, null]) {
    const calls = [];
    const state = { capabilities: { platform: 'unknown', liveServices: false, updater: false } };
    const start = extract(app, 'startApplication', {
      state, applyLiveDependencies() {}, text() {},
      invoke: async name => { calls.push(name); return capabilities; },
      refresh: async () => calls.push('refresh'),
      scheduleApplicationUpdateChecks: () => { if (state.capabilities.updater) calls.push('updates'); },
      fetchYoutubeStatus: () => calls.push('youtube'),
      fetchTwitchStatus: () => calls.push('twitch'),
      window: { setInterval: () => {} },
      YOUTUBE_STATUS_POLL_MS: 5000,
      setNotice: () => calls.push('notice'), errorText: String,
    });
    await start();
    assert.equal(calls[0], 'platform_capabilities');
    assert(calls.includes('refresh'), 'PeerCast works even if capability query fails');
    for (const optional of ['updates', 'youtube', 'twitch']) assert.equal(calls.includes(optional), optional === 'updates' ? capabilities?.updater === true : capabilities?.liveServices === true, optional);
  }
  const ready = { monitorReady: true, playbackReady: true, reasonCode: 'ready', message: 'ready' };
  for (const dependencies of [ready, { ...ready, playbackReady: false, reasonCode: 'missing' }, { ...ready, monitorReady: false, playbackReady: false, reasonCode: 'invalid' }]) {
    const state = { capabilities: { ...linux, liveServices: true, liveDependencies: dependencies }, youtubeRevision: 0, twitchRevision: 0 };
    assert.equal(extract(app, 'livePlaybackReady', { state })(), dependencies.playbackReady);
    assert.equal(extract(app, 'livePlayerName', { state })(), 'mpv');
    for (const provider of ['youtube', 'twitch']) {
      const calls = [];
      await extract(app, provider === 'youtube' ? 'fetchYoutubeStatus' : 'fetchTwitchStatus', {
        state, document: { visibilityState: 'visible' },
        invoke: async name => { calls.push(name); return { channels: [] }; },
        youtubeSnapshotsEqual: () => true, window: { dispatchEvent() {} }, CustomEvent: class {}, errorText: String,
      })();
      assert.deepEqual(calls, [`${provider}_status`], 'Registration state remains readable even with missing or invalid dependencies');
    }
  }
  assert.equal(extract(app, 'livePlayerName', { state: { capabilities: { platform: 'windows' } } })(), 'PCRPlayer');
  assert.equal(extract(app, 'livePlaybackReady', { state: { capabilities: { platform: 'linux', liveServices: true } } })(), false);
  assert.match(settings, /現在のライブをmpvで再生できます。YouTube\/Twitchの録画はこの版では対応していません/);
  const main = fs.readFileSync(path.join(__dirname, '../src/main.rs'), 'utf8');
  assert.match(main, /"liveDependencies": peercast_desktop::live_runtime::dependencies\(\)/);
  assert.equal((main.match(/if !peercast_desktop::live_runtime::dependencies\(\).monitor_ready/g) || []).length, 2, 'Both workers gate before consuming due requests');
  assert(!main.includes('Mutex::new(Err("Linux版では未対応です"'));
  let release, launches = 0;
  const playbackState = {};
  const launch = extract(app, 'launchLivePlayback', {
    state: playbackState, livePlaybackReady: () => true,
    invoke: () => { launches++; return new Promise(resolve => { release = resolve; }); },
  });
  const first = launch('youtube_open', { videoId: 'one' });
  await launch('youtube_open', { videoId: 'one' });
  assert.equal(launches, 1, 'Repeated clicks share the pending launch guard');
  release(); await first;
  const second = launch('youtube_open', { videoId: 'one' });
  assert.equal(launches, 2, 'A completed launch can be retried');
  release(); await second;
  const failed = extract(app, 'launchLivePlayback', {
    state: playbackState, livePlaybackReady: () => true, invoke: async () => { throw Error('fixture failure'); },
  });
  await assert.rejects(failed('youtube_open', { videoId: 'one' }), /fixture failure/);
  assert.equal(playbackState.pendingLivePlayback.size, 0, 'Failure releases the guard');
  console.log('Linux frontend: ready/missing/invalid, readable registrations, fail-closed playback/startup, bounded worker gate, disabled updater, mpv labels, and Windows compatibility passed');
})().catch(error => { console.error(error); process.exitCode = 1; });
