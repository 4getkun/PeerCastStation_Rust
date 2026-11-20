const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {livePage,liveFormats,ffmpegArgs,recordingArgs,recordingPath,pruneOldLogs,RECORDER_PHASE} = require('./pcr-live-bridge.cjs');
assert.equal(livePage('https://www.youtube.com/@SkyNews'), 'https://www.youtube.com/@SkyNews/live');
assert.equal(livePage('https://www.twitch.tv/twitchplayspokemon'), 'https://www.twitch.tv/twitchplayspokemon');
for(const url of ['file:///C:/video.flv','https://youtube.com.evil.invalid/watch?v=abcdefghijk','https://u:p@youtube.com/watch?v=abcdefghijk','http://www.twitch.tv/name','https://www.twitch.tv/videos/123']) assert.throws(()=>livePage(url));
const video={url:'https://example.invalid/v.m3u8',protocol:'m3u8_native',vcodec:'avc1',acodec:'none',height:720};
const audio={url:'https://example.invalid/a.m3u8',protocol:'m3u8_native',vcodec:'none',acodec:'mp4a.40.2'};
const info={is_live:true,live_status:'is_live',formats:[video,audio]};
assert.deepEqual(liveFormats(info),[video,audio]);
for(const status of ['was_live','post_live','is_upcoming','not_live'])assert.throws(()=>liveFormats({...info,is_live:false,live_status:status}));
assert.throws(()=>liveFormats({...info,formats:[{...video,vcodec:'av01'},audio]}));
const combined={...video,acodec:'mp4a.40.2'};assert.deepEqual(liveFormats({...info,formats:[combined]}),[combined]);
const args=ffmpegArgs(info,[video,audio]);assert(args.includes('1:a:0'));assert.equal(args[args.indexOf('-c:v')+1],'copy');assert.equal(args[args.indexOf('-c:a')+1],'pcm_s16le');assert(args.includes('44100'));
const recorded=recordingArgs(info,[video,audio],'C:\\test.mkv');assert.equal(recorded[recorded.indexOf('-c:a')+1],'copy');assert(!recorded.includes('-copyts'));assert(!recorded.includes('-ar'));assert(recorded.includes('-n'));
const twitch=ffmpegArgs({...info,extractor_key:'TwitchStream'},[combined]);assert(!twitch.includes('-copyts'));assert(twitch.includes('+discardcorrupt'));assert(twitch.includes('-dts_delta_threshold'));
const fullhd={...video,height:1080,width:1920};const uhd={...video,height:2160,width:3840};assert.deepEqual(liveFormats({...info,formats:[uhd,fullhd,video,audio]},2160),[fullhd,audio]);assert.deepEqual(liveFormats({...info,formats:[video,audio]}),[video,audio]);assert.throws(()=>liveFormats({...info,formats:[uhd,audio]}));
for(const invalid of ['relative.mkv','C:\\test.mp4','C:\\bad\nname.mkv',__filename])assert.throws(()=>recordingPath(invalid));
assert.equal(RECORDER_PHASE.IDLE, 'idle');
const logDir = fs.mkdtempSync(path.join(os.tmpdir(), 'pcr-live-logs-'));
try {
  for (let i = 0; i < 5; i += 1) {
    const file = path.join(logDir, `bridge-${1000 + i}.log`);
    fs.writeFileSync(file, `entry ${i}`);
    const t = new Date(Date.now() - (5 - i) * 1000);
    fs.utimesSync(file, t, t);
  }
  fs.writeFileSync(path.join(logDir, 'keep-me.txt'), 'unrelated');
  pruneOldLogs(logDir, 2);
  const remaining = fs.readdirSync(logDir).sort();
  assert.deepEqual(remaining, ['bridge-1003.log', 'bridge-1004.log', 'keep-me.txt']);
} finally {
  fs.rmSync(logDir, { recursive: true, force: true });
}
console.log('Live bridge: provider URL validation, live-only guard, codec selection, log pruning, and remux mapping passed.');
