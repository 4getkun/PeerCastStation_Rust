const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const crypto = require('node:crypto');
const { spawn } = require('node:child_process');

function livePage(value) {
  const u = new URL(value);
  if (u.protocol !== 'https:' || u.username || u.password || u.port) throw new Error('Only public HTTPS live pages are supported.');
  const host = u.hostname.toLowerCase().replace(/^www\./, '').replace(/^m\./, '');
  if (host === 'twitch.tv' && /^\/[a-z\d_]{1,25}\/?$/i.test(u.pathname)) return u.href;
  if (host === 'youtu.be' && /^\/[\w-]{11}\/?$/.test(u.pathname)) return u.href;
  if (host === 'youtube.com') {
    if (u.pathname === '/watch' && /^[\w-]{11}$/.test(u.searchParams.get('v') || '')) return u.href;
    if (/^\/(?:@[^/]+|channel\/[\w-]+)\/?$/.test(u.pathname)) u.pathname = u.pathname.replace(/\/$/, '') + '/live';
    if (/^\/(?:@[^/]+|channel\/[\w-]+)\/live\/?$/.test(u.pathname)) return u.href;
  }
  throw new Error('Use a YouTube live page or a Twitch channel URL.');
}
function liveFormats(info, maxHeight = 1080) {
  maxHeight = Math.min(Number.isFinite(maxHeight) && maxHeight > 0 ? maxHeight : 1080, 1080);
  if (info?.is_live !== true || info?.live_status !== 'is_live') throw new Error('This channel is not live.');
  const formats = (info.formats || []).filter(f => /^https:\/\//.test(f.url || '') && /^m3u8/.test(f.protocol || ''));
  const video = formats.filter(f => /^(?:avc1|h264)/i.test(f.vcodec || '') && f.height > 0 && f.height <= maxHeight && (!f.width || f.width <= 1920))
    .sort((a, b) => (b.height - a.height) || ((b.fps || 0) - (a.fps || 0)) || ((b.tbr || 0) - (a.tbr || 0)))[0];
  if (!video) throw new Error('No H.264 live HLS format was found.');
  if (video.acodec && video.acodec !== 'none') {
    if (!/^(?:mp4a|aac)/i.test(video.acodec)) throw new Error('Live audio is not AAC.');
    return [video];
  }
  const audio = formats.filter(f => f.vcodec === 'none' && (!f.acodec || /^(?:mp4a|aac)/i.test(f.acodec)))
    .sort((a, b) => (b.abr || b.tbr || Number(b.format_id) || 0) - (a.abr || a.tbr || Number(a.format_id) || 0))[0];
  if (!audio) throw new Error('No live AAC audio format was found.');
  return [video, audio];
}
function redacted(value) { return String(value).replace(/https?:\/\/[^\s\]\)"']+/gi, '<media-url>'); }
function tool(executable, args) {
  return spawn(executable, args, { windowsHide: true, shell: false, stdio: ['ignore', 'pipe', 'pipe'] });
}
async function resolveLive(config, page, writeLog) {
  const args = ['--ignore-config', '--no-plugin-dirs', '--no-cache-dir', '--skip-download', '--no-playlist', '--dump-single-json', '--match-filter', 'is_live', '--socket-timeout', '15', '--retries', '1', '--extractor-retries', '1'];
  if (config.deno) args.push('--js-runtimes', `deno:${config.deno}`);
  else args.push('--js-runtimes', `node:${process.execPath}`);
  args.push(page);
  const child = tool(config.ytDlp, args);
  let output = '', errors = '';
  child.stdout.on('data', data => { output += data; if (output.length > 16 * 1024 * 1024) child.kill(); });
  child.stderr.on('data', data => { errors = (errors + redacted(data)).slice(-16000); });
  return await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => child.kill(), 75000);
    child.once('error', error => { clearTimeout(timeout); reject(error); });
    child.once('close', code => {
      clearTimeout(timeout);
      if (errors) writeLog(errors);
      if (code !== 0 || !output.trim()) return reject(new Error(`Live metadata failed (${code}). ${errors.slice(-800)}`));
      try { resolve(JSON.parse(output)); } catch { reject(new Error('Invalid live metadata response.')); }
    });
  });
}
function ffmpegArgs(info, formats) {
  const twitch = info.extractor_key === 'TwitchStream';
  const args = ['-hide_banner', '-nostdin', '-loglevel', 'warning', '-stats_period', '2', '-progress', 'pipe:2'];
  // HLS discontinuity correction is disabled by -copyts. Keep it active for Twitch.
  if (twitch) args.push('-dts_delta_threshold', '2');
  else args.push('-copyts', '-start_at_zero');
  formats.forEach(format => {
    const headers = { ...(info.http_headers || {}), ...(format.http_headers || {}) };
    const ua = headers['User-Agent'];
    if (ua && !/[\r\n]/.test(ua)) args.push('-user_agent', ua);
    if (twitch) args.push('-fflags', '+discardcorrupt');
    args.push('-rw_timeout', '20000000', '-i', format.url);
  });
  // Decode only playback audio to PCM; this avoids relying on an installed AAC DirectShow decoder.
  args.push('-map', '0:v:0', '-map', formats.length > 1 ? '1:a:0' : '0:a:0', '-c:v', 'copy', '-c:a', 'pcm_s16le', '-ar', '44100', '-ac', '2', '-avoid_negative_ts', 'make_zero', '-flvflags', 'no_duration_filesize', '-flush_packets', '1', '-f', 'flv', 'pipe:1');
  return args;
}
function recordingArgs(info, formats, destination) {
  const args = ffmpegArgs(info, formats);
  args[args.indexOf('-c:a') + 1] = 'copy';
  args.splice(args.indexOf('-ar'), 4);
  args.splice(args.indexOf('-nostdin'), 1);
  const start = args.indexOf('-flvflags');
  args.splice(start, args.length - start, '-n', '-f', 'matroska', destination);
  // Input clocks can be hours old; recorded files start at zero without encoding.
  if (args.includes('-copyts')) args.splice(args.indexOf('-copyts'), 2);
  return args;
}
function recordingPath(value) {
  if (typeof value !== 'string' || value.length > 32000 || /[\r\n\0]/.test(value) || !path.isAbsolute(value) || path.extname(value).toLowerCase() !== '.mkv') throw new Error('Choose an absolute .mkv file path.');
  if (fs.existsSync(value)) throw new Error('The destination already exists.');
  if (!fs.statSync(path.dirname(value)).isDirectory()) throw new Error('The destination folder does not exist.');
  return value;
}
function createRecorder(config, info, formats, writeLog) {
  let child = null, state = 'idle', watchdog = null;
  function stop() {
    if (!child) return;
    if (state === 'stopping') return;
    state = 'stopping';
    child.stdin.write('q\n', () => {});
    watchdog = setTimeout(() => child?.kill(), 5000);
  }
  return {
    state: () => state,
    start(destination) {
      if (child) throw new Error('Recording is already running.');
      destination = recordingPath(destination);
      state = 'starting';
      const current = spawn(config.ffmpeg, recordingArgs(info, formats, destination), { windowsHide: true, shell: false, stdio: ['pipe', 'ignore', 'pipe'] });
      child = current;
      current.stdin.on('error', () => {});
      current.stderr.on('data', data => {
        const output = String(data);
        if (state === 'starting' && /out_time_(?:us|ms)=[1-9]/.test(output)) state = 'recording';
        writeLog('recording: ' + output);
      });
      current.once('error', error => { state = 'error'; writeLog('recording: ' + error.message); });
      current.once('close', code => {
        clearTimeout(watchdog);
        child = null;
        state = code === 0 ? 'idle' : 'error';
        writeLog(`recording exit=${code}`);
      });
    },
    stop,
    async finish() {
      const current = child;
      if (!current) return;
      const closed = new Promise(resolve => current.once('close', resolve));
      stop();
      await closed;
    },
  };
}
async function main() {
  const page = livePage(process.argv[2]);
  const config = JSON.parse(fs.readFileSync(path.join(__dirname, 'pcr-live-config.json'), 'utf8'));
  for (const name of ['ytDlp', 'ffmpeg', 'deno']) if (config[name] && !path.isAbsolute(config[name])) config[name] = path.resolve(__dirname, config[name]);
  for (const [name, expected] of Object.entries(config.sha256 || {})) {
    if (!config[name] || crypto.createHash('sha256').update(fs.readFileSync(config[name])).digest('hex') !== expected) throw new Error('Bundled live helper checksum mismatch.');
  }
  const logDirectory = config.logDirectory ? path.resolve(config.logDirectory) : path.join(process.env.LOCALAPPDATA || __dirname, 'PeerCast RT', 'live-logs');
  fs.mkdirSync(logDirectory, { recursive: true });
  const log = fs.createWriteStream(path.join(logDirectory, `bridge-${process.pid}.log`));
  const writeLog = message => log.write(redacted(message) + '\n');
  const info = await resolveLive(config, page, writeLog);
  const formats = liveFormats(info, config.maxHeight || 1080);
  const recorder = createRecorder(config, info, formats, writeLog);
  writeLog(JSON.stringify({ provider: new URL(page).hostname, id: info.id, title: info.title, live: true, formats: formats.map(f => ({ id: f.format_id, height: f.height, fps: f.fps, vcodec: f.vcodec, acodec: f.acodec })) }));
  const route = `/${crypto.randomBytes(16).toString('hex')}.flv`;
  let media = null, requested = false;
  const server = http.createServer((req, res) => {
    if (req.url === route + '/record' && !req.headers.origin) {
      if (req.method === 'GET') { res.writeHead(200, {'Content-Type':'text/plain','Cache-Control':'no-store'}); res.end(recorder.state()); return; }
      if (req.method !== 'POST' || !/^text\/plain(?:;|$)/i.test(req.headers['content-type'] || '')) { res.writeHead(405); res.end(); return; }
      let body = '';
      req.setEncoding('utf8');
      req.on('data', data => { body += data; if (body.length > 32000) req.destroy(); });
      req.on('end', () => {
        try {
          if (body) { if (!requested || !media || media.exitCode !== null) throw new Error('Playback is not running.'); recorder.start(body); }
          else recorder.stop();
          res.writeHead(202, {'Content-Type':'text/plain','Cache-Control':'no-store'}); res.end(recorder.state());
        } catch (error) { writeLog(error.message); res.writeHead(409); res.end('Recording request failed.'); }
      });
      return;
    }
    if (req.method !== 'GET' || req.url !== route || requested) { res.writeHead(404); res.end(); return; }
    requested = true;
    media = tool(config.ffmpeg, ffmpegArgs(info, formats));
    res.writeHead(200, { 'Content-Type': 'video/x-flv', 'Cache-Control': 'no-store', 'Connection': 'close' });
    media.stdout.pipe(res);
    media.stderr.on('data', data => writeLog(data));
    media.once('error', error => { writeLog(error.message); res.destroy(); });
    media.once('close', code => { writeLog(`ffmpeg exit=${code}`); res.end(); });
    res.once('close', async () => { if (media) media.kill(); await recorder.finish(); server.close(); log.end(); setTimeout(() => process.exit(0), 150); });
  });
  server.listen(0, '127.0.0.1', () => process.stdout.write(`http://127.0.0.1:${server.address().port}${route}\n`));
  const idle = setTimeout(() => { if (!requested) { writeLog('No player connected.'); server.close(); log.end(); process.exit(1); } }, 60000);
  idle.unref();
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, async () => { if (media) media.kill(); server.close(); await recorder.finish(); log.end(); process.exit(0); });
}
module.exports = { livePage, liveFormats, ffmpegArgs, recordingArgs, recordingPath, createRecorder };
if (require.main === module) main().catch(error => { process.stderr.write(redacted(error.message) + '\n'); process.exit(1); });
