# PeerCastStation_Rust

A high-performance, memory-safe, cross-platform Rust port of [PeerCastStation](https://github.com/kumaryu/peercaststation) (a modern PeerCast P2P streaming servent).

---

## Overview

PeerCast is a decentralized peer-to-peer (P2P) audio/video streaming protocol that distributes media streams among viewers without centralized server bandwidth bottlenecks. **PeerCastStation_Rust** is a complete, native Rust rewrite designed for maximum throughput, low memory footprint, zero-copy packet forwarding, and 24/7/365 long-term reliability.

### Key Highlights
- **100% PeerCast Protocol (PCP) Compatibility**: Seamlessly interoperate with existing PeerCast nodes (PeerCast v0.1216+, PeerCastStation, PeerCast-IM) and Yellow Pages (YP).
- **Zero-Copy Streaming**: Built on `bytes::Bytes` for $O(1)$ packet fan-out to downstream relays and local media players.
- **Strict Bounded Resources**: Hard capacity limits on ring buffers, sliding-window retention, and connection caps to eliminate unbounded memory growth.
- **Single Self-Contained Binary**: Headless daemon with embedded Web UI assets (`rust-embed`) and JSON-RPC 2.0 API.

---

## Documentation

Comprehensive project documentation is maintained in the `docs/` directory:

- 📖 **[Architecture & Technical Specification (docs/SPECIFICATION.md)](docs/SPECIFICATION.md)**:
  Detailed system architecture, upstream C# component mapping, PCP binary wire format, state machines, and implementation roadmap.
- 📝 **[Work Log & Architectural Decision Records (docs/WORKLOG_AND_DECISIONS.md)](docs/WORKLOG_AND_DECISIONS.md)**:
  Continuous work log, upstream analysis notes, Architectural Decision Records (ADRs), and instructions for subsequent LLM agents and human developers.

---

## Workspace Structure (Target Blueprint)

- `crates/peercast-core`: Core primitives (`ID4`, `Atom`, `Content`, `ContentRingBuffer`, `Host`).
- `crates/peercast-pcp`: PCP protocol framing, handshake engine (`helo`/`oleh`), source/output streams.
- `crates/peercast-format`: Media format inspection and container demuxing (FLV, TS, ASF, MKV, OGG, RAW).
- `crates/peercast-http`: HTTP streaming engine for player clients (`/stream/{id}`, `/pls/`, `/m3u`).
- `crates/peercast-servent`: Central channel manager, routing tree, gossip propagation, and bandwidth throttling.
- `crates/peercast-rpc`: JSON-RPC 2.0 API server.
- `crates/peercast-web`: Embedded static HTML/JS Web UI.
- `crates/peercast-app`: CLI daemon binary (`pecastation-rs`).

---

## License

GPLv3 (compatible with upstream PeerCastStation).

## Downloads

The Windows x64 download is one 7z archive containing the installer, checksums, and a `licenses/` folder:

- [PeerCast RT 1.1.2 for Windows x64](https://peercast-rt-downloads.fourgetkun.com/releases/1.1.2/PeerCastRT-1.1.2-windows-x64-installer.7z)
- [7z SHA-256 checksum](https://peercast-rt-downloads.fourgetkun.com/releases/1.1.2/PeerCastRT-1.1.2-windows-x64-installer.7z.sha256) (`0131566f92448d51e3be8b448ed3fe09ec89ad8919811cdbf536694fd72891b8`)
- [Bundled PCRPlayer corresponding source archive for PeerCast RT 1.1.2](https://peercast-rt-downloads.fourgetkun.com/releases/1.1.2/PCRPlayer-PeerCastRT-1.1.2-sources.7z)
- [yt-dlp 2026.08.19 corresponding source archive](https://peercast-rt-downloads.fourgetkun.com/releases/1.1.2/yt-dlp-2026.08.19-sources.7z)
- [PeerCast RT distribution site](https://fourgetkun.com/peercast-rt/)

The window size uses physical pixels by default; enable the logical-size checkbox in Appearance to follow Windows display scaling. The Connections context menu can clear transient messages and the in-memory log without changing active connections or channel rules.

Extract the archive and run `PeercastRT_1.1.2_x64-setup.exe`. The completion page lets you choose whether to launch the app. An uninstaller is included and appears in Windows Installed Apps; settings and web cache are retained unless their removal is explicitly selected.

Version 1.1.2 adds named window-size presets (up to 32 custom sizes), moves YouTube registration into Settings > Yellow Pages, and adds Play/Record actions to the channel-list context menu. The duplicate detail-pane play button is removed. Small Windows icons and the bundled PCRPlayer's default fonts, caption, and initial posting-pane visibility are updated. Existing PCRPlayer font settings are retained.

## YouTube favorites

YouTube favorites monitoring accepts up to 20 public channel URLs in `@handle` or `UC...` form. While the app is running, it checks every five minutes; manual checks have a 30-second cooldown. The list distinguishes live streams, upcoming streams, and check errors. Windows notifications and sounds are identified by `channelId` and limited to one notification per channel every 12 hours. Viewing opens in a browser, and YouTube streams are not recording targets. Continuous monitoring and notification delivery have not been operationally guaranteed.

Before building a Windows installer, run `pwsh -File tools/prepare-youtube-helper.ps1 -ReleaseVersion 1.1.2`. This prepares the hash-pinned helper, license notices, and corresponding-source archive used by the existing packaging and publishing scripts. `tools/yt-dlp/lock.json` records all upstream source URLs and SHA-256 hashes; update it together with the helper version and embedded digest when changing yt-dlp.

Version 1.0.4 improves relay startup, adds a copy button for filtered logs and a direct-favorite removal action, and notifies Windows when a favorite broadcaster appears on a channel-list update. Notifications play one short original chime per batch, with custom rule sounds taking priority. They are limited to once per broadcaster every 12 hours, with history retained across restarts; hidden or muted channels are excluded.

Version 1.0.5 adds channel reconnect/disconnect and recording playback, Explorer, and Recycle Bin actions to context menus. It fixes FLV recording timestamps and saves WMV streams as `.wmv` without re-encoding. Recorded paths use ordinary Windows notation in the UI. Update requests now negotiate HTTP/2, which the download host requires; version 1.0.4 needs a one-time manual update because its HTTP/1.1 client cannot retrieve the update manifest.

Update artifacts are signed for PeerCast RT's built-in updater. The installer does not have an Authenticode publisher signature, and the 7z format does not guarantee that Chrome or Windows warnings will be absent. Verify the SHA-256 checksum before running it.
