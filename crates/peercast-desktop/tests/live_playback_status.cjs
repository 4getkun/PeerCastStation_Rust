const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const app = fs.readFileSync(path.join(__dirname, '../frontend/app.js'), 'utf8');
const main = fs.readFileSync(path.join(__dirname, '../src/main.rs'), 'utf8');
function extract(name, bindings) {
  const match = app.match(new RegExp(`  (?:async )?function ${name}\\([^\\n]*\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, name);
  return new Function(...Object.keys(bindings), `return (${match[0].trim()})`)(...Object.values(bindings));
}
function harness(platform = 'linux') {
  const state = { capabilities: { platform } }, timers = new Map(), notices = [], calls = [];
  let next = 0, status = { generation: 1, state: 'running', message: '' }, deferred;
  const window = { setTimeout(fn) { timers.set(++next, fn); return next; }, clearTimeout(id) { timers.delete(id); } };
  const invoke = async (command, args) => { calls.push([command, args]); return deferred ? deferred : status; };
  const stop = extract('stopLivePlaybackWatch', { state, window });
  const watch = extract('watchLivePlayback', { state, window, invoke, stopLivePlaybackWatch: stop, setNotice: (...args) => notices.push(args) });
  return { state, timers, notices, calls, stop, watch, setStatus(value) { status = value; }, defer(value) { deferred = value; }, async tick() {
    assert.equal(timers.size, 1, 'Exactly one timer belongs to the latest launch');
    const [id, fn] = timers.entries().next().value; timers.delete(id); await fn();
  } };
}
(async () => {
  let h = harness();
  assert.equal(h.timers.size, 0, 'No polling before launch');
  for (const invalid of [null, undefined, 0, -1, 1.5, '1']) h.watch(invalid);
  assert.equal(h.timers.size, 0);
  h.watch(1); await h.tick();
  assert.equal(h.notices.length, 0, 'Running does not claim actual playback success');
  assert.equal(h.timers.size, 1);
  h.setStatus({ generation: 1, state: 'failed', message: 'safe exit diagnostic' });
  await h.tick();
  assert.deepEqual(h.notices, [['safe exit diagnostic', true]]);
  assert.equal(h.timers.size, 0, 'A late failure is reported once then polling ends');
  h.watch(1); assert.equal(h.timers.size, 0, 'Repeated completed generation cannot notify again');
  h.watch(2); h.setStatus({ generation: 2, state: 'exited', message: '' }); await h.tick();
  assert.equal(h.timers.size, 0); assert.equal(h.notices.length, 1, 'User closing mpv is not an error');

  h = harness(); h.watch(1);
  let release; h.defer(new Promise(resolve => { release = resolve; }));
  const oldPoll = h.tick();
  h.watch(2); h.watch(1);
  release({ generation: 1, state: 'failed', message: 'obsolete failure' }); await oldPoll;
  assert.equal(h.notices.length, 0, 'Stale in-flight response cannot overwrite the latest launch');
  assert.equal(h.state.livePlaybackGeneration, 2);
  h.defer(null); h.setStatus({ generation: 3, state: 'failed', message: 'another launch' }); await h.tick();
  assert.equal(h.timers.size, 0); assert.equal(h.notices.length, 0, 'Backend supersession stops old polling');

  h = harness(); h.watch(1);
  h.defer(new Promise(resolve => { release = resolve; }));
  const canceled = h.tick(); h.stop();
  release({ generation: 1, state: 'failed', message: 'after dismissal' }); await canceled;
  assert.equal(h.notices.length, 0); assert.equal(h.timers.size, 0, 'Cancellation invalidates outstanding requests');
  h.state.livePlaybackWatchDisposed = true; h.watch(2); assert.equal(h.timers.size, 0);
  assert.match(app, /addEventListener\("pagehide", \(\) => \{\s*state.livePlaybackWatchDisposed = true;\s*stopLivePlaybackWatch\(\);/);

  h = harness(); h.watch(1);
  h.defer(Promise.reject(Error('raw URL or command must not surface'))); await h.tick();
  assert.equal(h.timers.size, 0);
  assert.equal(h.notices.length, 1);
  assert(!h.notices[0][0].includes('raw URL'), 'IPC failures use a fixed safe diagnostic');

  h = harness('windows'); h.watch(1); assert.equal(h.timers.size, 0, 'Windows never polls');
  for (const platform of ['windows', 'linux']) {
    let watched = [], launched = 0;
    const launch = extract('launchLivePlayback', { state: { capabilities: { platform } }, livePlaybackReady: () => true,
      invoke: async () => { launched++; return platform === 'linux' ? 7 : null; }, watchLivePlayback: value => watched.push(value) });
    await launch('youtube_open', { videoId: 'fixture' });
    assert.equal(launched, 1); assert.deepEqual(watched, platform === 'linux' ? [7] : []);
  }
  assert.match(main, /fn live_playback_status\([\s\S]*?window.label\(\) != "main"[\s\S]*?live_player::launch_status\(\)/);
  for (const provider of ['youtube', 'twitch']) assert.match(main, new RegExp(`async fn ${provider}_open\\([\\s\\S]*?Result<Option<u64>, String>`));
  console.log('Live playback status: late failure, exit, supersession, repeated launch, stale response, cancellation, IPC scope, and no Windows polling passed');
})().catch(error => { console.error(error); process.exitCode = 1; });
