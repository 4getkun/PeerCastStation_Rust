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

- [PeerCast RT 1.0.4 for Windows x64](https://peercast-rt-downloads.fourgetkun.com/releases/1.0.4/PeerCastRT-1.0.4-windows-x64-installer.7z)
- [7z SHA-256 checksum](https://peercast-rt-downloads.fourgetkun.com/releases/1.0.4/PeerCastRT-1.0.4-windows-x64-installer.7z.sha256) (`5245c9f2e460a8982313bedfc54edaa5a5da349dd9e454d3ce168943779306f5`)
- [PeerCast RT distribution site](https://fourgetkun.com/peercast-rt/)

Extract the archive and run `PeercastRT_1.0.4_x64-setup.exe`. The completion page lets you choose whether to launch the app. An uninstaller is included and appears in Windows Installed Apps; settings and web cache are retained unless their removal is explicitly selected.

Version 1.0.4 improves relay startup, adds a copy button for filtered logs and a direct-favorite removal action, and notifies Windows when a favorite broadcaster appears on a channel-list update. Notifications play one short original chime per batch, with custom rule sounds taking priority. They are limited to once per broadcaster every 12 hours, with history retained across restarts; hidden or muted channels are excluded.

Update artifacts are signed for PeerCast RT's built-in updater. The installer does not have an Authenticode publisher signature, and the 7z format does not guarantee that Chrome or Windows warnings will be absent. Verify the SHA-256 checksum before running it.
