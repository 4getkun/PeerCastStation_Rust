# PCRPlayer live playback

`pcrplayer-live.patch` applies to the unmodified corresponding source archive shipped
with PeerCast RT 1.1.2. It preserves the upstream file encodings and line endings.
Apply it from that archive's `source` directory with:

```powershell
git -c core.autocrlf=false apply --check path/to/pcrplayer-live.patch
git -c core.autocrlf=false apply path/to/pcrplayer-live.patch
```

`pcrplayer-live.sha256.json` records the exact before/after files. The 1.2.0 source
archive already contains the resulting C++ source, dependency archives, runtime
files, and build instructions. Do not apply this patch to that already patched
archive.

The helper accepts current YouTube/Twitch live pages, selects H.264 video up to
1920×1080, and serves it only on a random loopback URL. Playback audio is PCM;
recordings retain the source H.264/AAC in Matroska. Recording starts a separate
HLS reader and therefore adds network traffic while recording.

`runtime-lock.json` pins the official Node.js binary and the FFmpeg source archive.
`build-ffmpeg.ps1` / `build-ffmpeg.sh` build the minimal LGPL FFmpeg used by the
helper. The corresponding-source release includes the source tarball under
`runtime-source/`; do not commit that tarball or the runtime executables here.
Use `tools/prepare-live-player.ps1` to prepare a portable PCRPlayer bundle before
the existing desktop installer packaging. It copies no player settings or logs.
