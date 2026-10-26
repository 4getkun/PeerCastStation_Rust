const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const source = fs.readFileSync(path.join(__dirname, '../frontend/app.js'), 'utf8');
const settingsSource = fs.readFileSync(path.join(__dirname, '../frontend/settings.js'), 'utf8');
function formatterFrom(text, name) {
  const match = text.match(new RegExp(`  function ${name}\\(seconds\\) \\{([\\s\\S]*?)\\n  \\}`));
  assert(match, `The ${name} formatter must exist`);
  return new Function('seconds', match[1]);
}
const formatters = [formatterFrom(source, 'formatAppUptime'), formatterFrom(settingsSource, 'formatBackendUptime')];

for (const [input, expected] of [
  [0, '0秒'],
  [59, '59秒'],
  [60, '1分 0秒'],
  [3599, '59分 59秒'],
  [3600, '1時間 0分 0秒'],
  [86401, '24時間 0分 1秒'],
  [null, '—'],
  [undefined, '—'],
  [NaN, '—'],
  ['invalid', '—'],
  [-1, '—'],
]) {
  for (const formatUptime of formatters) {
    assert.equal(formatUptime(input), expected, `Unexpected uptime formatting for ${String(input)}`);
  }
}
assert.match(source, /text\("uptime", running \? formatAppUptime\(snapshot\.uptimeSecs\) : "停止中"\)/,
  'The application uptime display must use its dedicated formatter, leaving channel-duration formatting unchanged');
assert.match(settingsSource, /\$\("backend-uptime"\)\.textContent = snapshot\.running \? formatBackendUptime\(snapshot\.uptimeSecs\) : "—"/,
  'The settings uptime display must use its dedicated formatter');
console.log('Application and settings uptime formatting passed: unit boundaries, multi-day hours, and invalid values');
