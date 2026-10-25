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

The Windows x64 installer is available directly, with a ZIP archive containing the license documents as an alternative:

- [PeerCast RT 1.0.2 installer](https://peercast-rt-downloads.fourgetkun.com/releases/1.0.2/PeercastRT_1.0.2_x64-setup.exe)
- [Installer SHA-256 checksum](https://peercast-rt-downloads.fourgetkun.com/releases/1.0.2/PeercastRT_1.0.2_x64-setup.exe.sha256) (`b6e4c194a7952f472aade4204eb5fda01b239262aa4eb13e9847af0c87656a3f`)
- [License-included installer ZIP](https://peercast-rt-downloads.fourgetkun.com/releases/1.0.2/PeerCastRT-1.0.2-windows-x64-installer.zip)
- [ZIP SHA-256 checksum](https://peercast-rt-downloads.fourgetkun.com/releases/1.0.2/PeerCastRT-1.0.2-windows-x64-installer.zip.sha256) (`e70db2d68383005bf1f1f2c170c40f62cfba4e55c0ccf6a616ec639a3de74fff`)
- [PeerCast RT distribution site](https://fourgetkun.com/peercast-rt/)

Run `PeercastRT_1.0.2_x64-setup.exe` to install the application. Update artifacts are signed for PeerCast RT's built-in updater, but the installer does not have an Authenticode publisher signature; verify the SHA-256 checksum before running it.
