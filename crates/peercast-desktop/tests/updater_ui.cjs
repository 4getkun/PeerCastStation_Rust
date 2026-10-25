const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const source = fs.readFileSync(path.join(__dirname, '../frontend/settings.js'), 'utf8');
const html = fs.readFileSync(path.join(__dirname, '../frontend/settings.html'), 'utf8');
function helper(name) {
  const start = source.indexOf(`  function ${name}(`);
  assert(start >= 0, `Expected helper ${name}`);
  const nextFunction = source.indexOf('\n  function ', start + 1);
  const nextWindow = source.indexOf('\n  window.', start + 1);
  const ends = [nextFunction, nextWindow].filter((value) => value >= 0);
  const end = ends.length ? Math.min(...ends) : source.length;
  const text = source.slice(start, end).trim();
  return new Function(`return (${text})`)();
}

const normalize = helper('normalizeUpdateStatus');
const model = helper('updateCardModel');
const renderNotes = helper('renderUpdateNotes');
const phases = ['idle', 'checking', 'upToDate', 'notPublished', 'available', 'downloading', 'ready', 'applying', 'error'];
for (const phase of phases) assert.equal(normalize({ phase }).phase, phase);
assert.equal(normalize({ phase: 'unknown-phase' }).phase, 'error');
assert.equal(normalize({ phase: 'available', currentVersion: 4, version: '<b>1.2</b>', notes: '<img src=x onerror=alert(1)>', receivedBytes: -1, totalBytes: '20' }).currentVersion, '');
assert.equal(normalize({ phase: 'available', receivedBytes: -1, totalBytes: '20' }).receivedBytes, 0);
assert.equal(normalize({ phase: 'available', receivedBytes: -1, totalBytes: '20' }).totalBytes, null);

for (const phase of ['checking', 'downloading', 'applying']) {
  const view = model({ phase });
  assert.equal(view.busy, true, `${phase} should be busy`);
  assert.equal(view.check || view.download || view.install, false, `${phase} must not offer duplicate primary actions`);
}
assert.equal(model({ phase: 'idle' }).check, true);
assert.equal(model({ phase: 'error' }).check, true, 'A failed update check must be retryable');
assert.equal(model({ phase: 'available', version: '1.2' }).download, true);
assert.equal(model({ phase: 'available' }).install, false);
assert.equal(model({ phase: 'downloading', receivedBytes: 50, totalBytes: 200 }).cancel, true);
assert.equal(model({ phase: 'downloading', receivedBytes: 50, totalBytes: 200 }).percent, 25);
assert.equal(model({ phase: 'downloading', receivedBytes: 300, totalBytes: 200 }).percent, 100);
assert.equal(model({ phase: 'downloading', receivedBytes: 512, totalBytes: null }).indeterminate, true);
assert.equal(model({ phase: 'ready', version: '1.2' }).install, true);
assert.equal(model({ phase: 'ready' }).later, true);
assert.equal(model({ phase: 'ready' }).percent, 100);
assert.equal(model({ phase: 'ready' }).indeterminate, false);
assert.equal(model({ phase: 'upToDate' }).download, false);

const notesElement = { textContent: '', set innerHTML(_) { throw new Error('Unsafe HTML insertion'); } };
const hostileNotes = '<img src=x onerror=alert(1)>';
renderNotes(notesElement, hostileNotes);
assert.equal(notesElement.textContent, hostileNotes, 'Release notes must remain plain text');

for (const id of ['update-status', 'update-notes', 'update-progress', 'update-check', 'update-download', 'update-cancel', 'update-install', 'update-later']) {
  assert(html.includes(`id="${id}"`), `Update card needs #${id}`);
}
assert.match(html, /更新を適用するとアプリが再起動し、現在の録画・配信は終了します。/);
assert.match(source, /window\.setTimeout\(pollUpdateStatus, 500\)/, 'Busy updates should poll every 500 ms');
assert.match(source, /document\.hidden/, 'Hidden pages must stop polling');
assert.match(source, /state\.updatePollRequest\) return/, 'Concurrent status requests must coalesce');
for (const command of ['update_status', 'check_update', 'download_update', 'cancel_update_download', 'install_update']) {
  assert(source.includes(`invoke("${command}")`) || source.includes(`runUpdateAction("${command}")`), `The UI must call ${command}`);
}
console.log('Updater UI regression: phases, busy states, progress, actions, and safe notes passed');
