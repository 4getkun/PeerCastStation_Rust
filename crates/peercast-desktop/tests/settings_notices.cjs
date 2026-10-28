const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../frontend/settings.js'), 'utf8');

function sourceFunction(name, bindings) {
  const start = source.match(new RegExp(`  function ${name}\\([^\\n]*\\) \\{`));
  assert(start, `Missing settings helper: ${name}`);
  let end = start.index + start[0].length;
  let depth = 1;
  while (depth && end < source.length) {
    if (source[end] === '{') depth++;
    if (source[end] === '}') depth--;
    end++;
  }
  return new Function(...Object.keys(bindings), `return (${source.slice(start.index, end).trim()});`)(...Object.values(bindings));
}

function fixture() {
  let now = 0;
  let serial = 0;
  const jobs = new Map();
  const elements = new Map(['notice', 'save-status', 'youtube-status', 'twitch-status'].map((id) => [id, { textContent: '', hidden: true, classList: { toggle() {} } }]));
  const state = { dirty: false, busy: false, loading: false, noticeTimer: null, noticeGeneration: 0, statusTimer: null, statusGeneration: 0, providerMessages: new Map(), providerMessageToken: 0 };
  const window = {
    setTimeout(callback, delay) { const id = ++serial; jobs.set(id, { callback, at: now + delay }); return id; },
    clearTimeout(id) { jobs.delete(id); },
  };
  const bindings = { $: (id) => elements.get(id), state, window };
  const notice = sourceFunction('setNotice', bindings);
  const status = sourceFunction('setStatus', bindings);
  const provider = sourceFunction('setProviderMessage', bindings);
  const tick = (ms) => {
    now += ms;
    for (const [id, job] of [...jobs]) if (job.at <= now) { jobs.delete(id); job.callback(); }
  };
  return { state, elements, jobs, tick, notice, status, provider };
}

const saved = fixture();
saved.notice('設定を保存しました。');
saved.status('変更を保存しました');
saved.tick(5000);
assert.equal(saved.elements.get('notice').textContent, '', 'A settings success banner must expire');
assert.equal(saved.elements.get('notice').hidden, true);
assert.equal(saved.elements.get('save-status').textContent, '', 'A saved footer must expire');

const race = fixture();
race.notice('古い通知');
const stale = [...race.jobs.values()][0]?.callback;
race.notice('新しい通知');
stale?.();
assert.equal(race.elements.get('notice').textContent, '新しい通知', 'An old callback cannot clear a replacement');
race.tick(5000);
assert.equal(race.elements.get('notice').textContent, '');

const busy = fixture();
busy.status('保存しています…', { persistent: true });
busy.tick(60000);
assert.equal(busy.elements.get('save-status').textContent, '保存しています…', 'In-progress state must remain until completion');
busy.status('変更を保存しました');
busy.state.dirty = true;
busy.tick(5000);
assert.equal(busy.elements.get('save-status').textContent, '未保存の変更があります', 'Expiry must preserve unsaved edits');

const error = fixture();
error.notice('保存できませんでした', true);
error.tick(5000);
assert.equal(error.elements.get('notice').hidden, false, 'Errors stay readable longer than successes');
error.tick(7000);
assert.equal(error.elements.get('notice').hidden, true);
const providers = fixture();
providers.provider('youtube', 'YouTube登録を更新しました。');
providers.tick(3000);
providers.provider('youtube', 'YouTube登録を更新しました。');
providers.provider('twitch', 'Twitch登録を更新しています…', { persistent: true });
providers.tick(2000);
assert.equal(providers.state.youtubeMessage, '', 'Repeated polling cannot extend success messages');
assert.equal(providers.state.twitchMessage, 'Twitch登録を更新しています…', 'The other provider is independent');
providers.provider('twitch', 'Twitch登録を更新しました。');
providers.tick(5000);
assert.equal(providers.state.twitchMessage, '');
console.log('Settings messages: success expiry, replacement race, processing state, unsaved edits, and error lifetime passed');
