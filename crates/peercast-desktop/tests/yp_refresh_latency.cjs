const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const app = fs.readFileSync(path.join(__dirname, '../frontend/app.js'), 'utf8');
const match = app.match(/  async function fetchYpChannels\([^\n]*\) \{([\s\S]*?)\n  \}/);
assert(match);

function harness(fail = false) {
  let now = 0, calls = 0;
  const renders = [];
  const old = [{ name: 'old' }], fresh = [{ name: 'new' }];
  const state = { ypChannels: old, ypRefreshCooldownUntil: 0, ypLoaded: true };
  const bindings = {
    state, YP_REFRESH_COOLDOWN_MS: 30_000, performance: { now: () => now },
    sourcePages: () => [{}], updateRefreshControls() {}, updateYpControls() {}, text() {},
    beginYpFetch: () => { state.ypBusy = true; return true; },
    invoke: async () => { calls++; now += 50; if (fail) throw new Error('network'); return fresh; },
    renderYpDirectory: () => renders.push({ time: now, channels: state.ypChannels }),
    advanceYpGeneration: () => ({}), notifyNewYpChannels() {},
    errorText: (error) => error.message,
    finishRefreshFeedback: async () => { now = Math.max(now, 400); },
    flushPendingYpRefresh() {}, scheduleYpAutoRefresh() {},
  };
  const run = new Function(...Object.keys(bindings), `return (${match[0].trim()})`)(...Object.values(bindings));
  return { run, state, renders, old, fresh, get calls() { return calls; } };
}

(async () => {
  const success = harness();
  await success.run(true, true);
  assert.equal(success.renders.length, 1, 'Refresh should rebuild the directory once after data arrives');
  assert.equal(success.renders[0].time, 50, 'Results must appear immediately after the simulated 50ms response');
  assert.equal(success.renders[0].channels, success.fresh);
  assert.equal(success.state.ypRefreshCooldownUntil, 30_050, 'The 30-second cooldown must remain intact');
  await success.run(true, true);
  assert.equal(success.calls, 1, 'Faster rendering must not permit another request during cooldown');
  const failed = harness(true);
  await failed.run(true, true);
  assert.equal(failed.renders.length, 1);
  assert.equal(failed.state.ypChannels, failed.old, 'A failed refresh must retain the old channel list');
  assert.equal(failed.state.ypError, 'network');
  assert.equal(failed.state.ypBusy, false);
  console.log('YP refresh latency: 50ms simulated response is rendered at 50ms with one directory rebuild; cooldown and failure retention passed');
})().catch((error) => { console.error(error); process.exitCode = 1; });
