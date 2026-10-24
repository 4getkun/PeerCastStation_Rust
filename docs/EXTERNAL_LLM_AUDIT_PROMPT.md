# Code Audit Request Prompt for External LLM: PeerCastStation (Rust Edition)

> **Instructions for Auditor**:  
> You are an expert systems programmer, network protocol specialist, and senior Rust & C# security auditor.  
> You are tasked with performing an independent, deep-dive code audit of **PeerCastStation (Rust Edition)**, an asynchronous, memory-safe rewrite of the C# P2P streaming servent [PeerCastStation](https://github.com/kumaryu/peercaststation) (with backward compatibility for PeerCast-YT).  
>
> Please review this audit guide, examine the target codebase and the specific high-risk areas detailed below, and provide a rigorous, actionable audit report.

---

## 1. Executive Context & Architecture

### 1.1 Project Mission
PeerCastStation is a decentralized peer-to-peer live media streaming servent. It acts as both a P2P overlay node (connecting to parent peers, receiving binary media streams, and relaying them down a broadcast tree to child peers) and a local media server (serving streams over HTTP to players like VLC, MPC, mpv, and browser players).

- **Upstream C# Reference**: `.NET Framework / .NET Core` C# codebase utilizing OWIN, reflection-based JSON-RPC dispatch, and `.NET Guid` binary layouts.
- **Rust Architecture**: Pure asynchronous Tokio workspace (`edition = 2021`), zero-copy memory slicing (`bytes::Bytes`), explicit enum state machines, bounded resource sliding-windows, and a single self-contained binary (< 1.1 MiB) with embedded Web UI assets.

### 1.2 Repository Crate Structure
```text
crates/
├── peercast-core/     # Core domain types: ID4, Atoms, ChannelId (.NET Guid compat),
│                      # ContentRingBuffer, ParentSelection, Gossip, HostRegistry, index.txt
├── peercast-pcp/      # PeerCast Protocol (PCP) engine: RelayClient, RelayServer,
│                      # Reverse reachability Probe, Yellow Page (YP) announce
├── peercast-format/   # Stream demuxers: FLV, MPEG-2 TS (PAT/PMT/PCR), ASF, MKV, Ogg, MP3,
│                      # and SafeStart keyframe selector
├── peercast-http/     # HTTP streaming server: HTTP/1.1 chunked vs raw continuous (MMS/ASF),
│                      # Playlist generator (M3U/PLS/ASX), Embedded UI assets (ETag/304/CSP)
├── peercast-rpc/      # JSON-RPC 2.0 management API: ApiHost parity, controller state machines,
│                      # JavaScript client adapter (peercaststation.js parity)
└── peercast-app/      # Application orchestration: CLI flags precedence, atomic JSON persistence,
                       # schema migrations, signal handling, graceful task drain, server listener
```

---

## 2. Where We Struggled: The Hardest Technical Challenges

To ensure your audit is as efficient and high-value as possible, the following sections detail the **hardest engineering hurdles, historical quirks, and subtle edge cases** encountered during the C#-to-Rust migration. **Please focus your scrutiny primarily on these areas.**

---

### Challenge A: `.NET Guid` Endianness vs. Canonical Big-Endian ChannelID
- **Files of Interest**:
  - `crates/peercast-core/src/channel_id.rs`
  - C# Reference: `PeerCastStation.Core/BroadcastChannel.cs:52` & `AtomCollectionExtensions.cs:201`
- **The Difficulty**:
  In C#, `BroadcastChannel.CreateChannelID` hashes stream metadata using MD5 and constructs a `System.Guid`. On little-endian architectures, `.NET Guid(byte[])` flips bytes `[0..4]`, `[4..6]`, and `[6..8]`. Furthermore, strings written via C# `BinaryWriter.Write(string)` are prefixed with a **7-bit variable-length encoded integer (LEB128)** indicating the byte count, followed by raw UTF-8 bytes.
- **What We Implemented**:
  1. `write_7bit_encoded_int()` and `write_binary_writer_string()` to mirror `BinaryWriter`.
  2. `to_dotnet_guid_bytes()` and `from_dotnet_guid_bytes()` to accurately simulate `.NET Guid` memory layout reversals.
  3. ChannelID derivation: `SHA-512(dotnet_bcid)` -> Optional IPv6 prefix (`1i32` LE) -> 64-byte SHA512 hash -> 7-bit length-prefixed `name`, `genre`, `source` -> `MD5` -> `from_dotnet_guid_bytes`.
- **Audit Questions**:
  1. Are there any edge cases with multi-byte Unicode strings (e.g., Japanese characters, surrogate pairs, emojis) where the 7-bit byte-length calculation or UTF-8 slicing could diverge between `.NET BinaryWriter` and Rust?
  2. Does `from_dotnet_guid_bytes` correctly invert `to_dotnet_guid_bytes` under all bit patterns?
  3. Could any malicious or malformed hex string cause an unexpected panic or boundary out-of-bounds error in `ChannelId::from_hex`?

---

### Challenge B: PCP Wire Framing & Reverse IP Address Endianness
- **Files of Interest**:
  - `crates/peercast-core/src/scalar.rs`
  - `crates/peercast-core/src/atom.rs`
  - `crates/peercast-core/src/id4.rs`
  - C# Reference: `PeerCastStation.Core/Atom.cs:612, 716`
- **The Difficulty**:
  PCP (PeerCast Protocol) is a binary protocol built from 4-byte Tag + 4-byte Length "Atoms". A notorious historical idiosyncrasy in original C++ PeerCast and preserved in C# is that **IP addresses are written in reversed byte order**:
  - IPv4: 4 bytes reversed (`192.168.1.10` becomes `[10, 1, 168, 192]`).
  - IPv6: 16 bytes reversed.
  Additionally, Atom tags (`ID4`) shorter than 4 characters are right-padded with NUL (`\0`), except the connection preamble which is `b"pcp\n"`.
- **What We Implemented**:
  `Scalar::read_ip_reversed()` and `Scalar::write_ip_reversed()`, paired with incremental streaming Atom decoders with strict memory budgets (`AtomBudget`).
- **Audit Questions**:
  1. Does `Scalar::read_ip_reversed` and `Scalar::write_ip_reversed` maintain exact round-trip fidelity for all IPv4 and IPv6 addresses (including loopback, mapped IPv4, and multicast)?
  2. Can deeply nested Atom payloads or maliciously truncated length fields trigger an integer overflow or unbounded allocation in `Atom::decode_stream`?
  3. Are there any ID4 tag definition divergences between C# and Rust (e.g. `trck` sub-atoms: `titl`/`crea`/`albm` vs `TRCK_NAME`/`TRCK_CREATOR`) that could impact downstream metadata consumers?

---

### Challenge C: HTTP Framing — Preventing Disastrous Chunked Transfer on ASF/MMS
- **Files of Interest**:
  - `crates/peercast-http/src/streamer.rs`
  - `crates/peercast-http/src/router.rs`
  - C# Reference: `PeerCastStation.HTTP/HTTPOutputStream.cs:298-340`
- **The Difficulty**:
  Modern web browsers and players expect `Transfer-Encoding: chunked` for live HTTP/1.1 streaming. However, legacy players (especially Windows Media Player / WMP) playing ASF/WMV/WMA or MMS streams **completely crash or fail to play** if `Transfer-Encoding: chunked` is applied.
  Moreover, WMP uses a two-phase handshake:
  1. Phase 1 (no `Pragma: xplaystrm=1`): The server MUST respond with `application/vnd.ms.wms-hdr.asfv1`, send *only* the ASF ContentHeader, and close the connection.
  2. Phase 2 (with `Pragma: xplaystrm=1`): The server responds with `application/x-mms-framed`, `Connection: close`, `Pragma: no-cache, features="broadcast,playlist"`, and sends continuous media packets without chunked encoding.
- **What We Implemented**:
  `resolve_framing()` strictly enforces `HttpFraming::RawContinuous` for any `HTTP/1.0` client or any stream with `application/x-mms-framed` or `video/x-ms-asf`.
- **Audit Questions**:
  1. Is there any code path where `HttpFraming::Chunked` could accidentally be selected for an ASF stream or an HTTP/1.0 client?
  2. Does the stream writer properly handle sudden client disconnects without leaking Tokio tasks or leaving half-closed sockets in `CLOSE_WAIT`?
  3. Are playlist files (`.asx`, `.pls`, `.m3u`) properly sanitized against CRLF header injection (REQ-060)?

---

### Challenge D: Container Demuxers (FLV & MPEG-2 TS) Keyframe & PCR Synchronization
- **Files of Interest**:
  - `crates/peercast-format/src/flv.rs`
  - `crates/peercast-format/src/ts.rs`
  - `crates/peercast-format/src/safestart.rs`
  - C# Reference: `PeerCastStation.FLV/FLVFileParser.cs` & `PeerCastStation.TS/TsPacket.cs`
- **The Difficulty**:
  1. **FLV Extended Timestamps**: FLV tag timestamps store the lower 24 bits in bytes `[4..7]`, but the upper 8 bits in byte `7` (offset 7 in tag header). If shifted incorrectly, playback speeds up or stutters.
  2. **MPEG-2 TS 27 MHz Clock**: TS packets are strictly 188 bytes beginning with sync byte `0x47`. PCR (Program Clock Reference) combines a 33-bit base with a 9-bit extension scaled to 27 MHz: `(base * 300 + ext) / 27,000,000.0`.
  3. **SafeStart Injection**: When a new viewer connects mid-stream, they must receive stream initialization headers (FLV metadata/sequence headers; TS PAT/PMT) and commence playback cleanly at a keyframe rather than an inter-frame.
- **Audit Questions**:
  1. In `flv.rs`, is the PreviousTagSize footer validation (`previous_tag_size == data_size + 11`) vulnerable to underflow if `data_size` is malformed?
  2. In `ts.rs`, does the PAT and PMT parser safely handle split sections spanning across packet boundaries or corrupt `pointer_field` values without panicking?
  3. When bitstream corruption occurs, does the demuxer recover and re-synchronize to the next valid sync word without infinite loops or quadratic scanning?

---

### Challenge E: Dual-Constraint RingBuffer & Epoch Invalidation
- **Files of Interest**:
  - `crates/peercast-core/src/ringbuffer.rs`
  - `crates/peercast-core/src/content.rs`
  - C# Reference: `PeerCastStation.Core/Content.cs:141`
- **The Difficulty**:
  PeerCast broadcasts can run for days or months continuously. Unbounded memory growth (leaking chunks in ring buffers) is strictly forbidden. Furthermore, if a broadcaster stops and restarts, the stream epoch (`stream_id`) increments. Stale packets from older epochs must be purged instantly, or viewers will receive discordant, un-decodeable video slices.
- **What We Implemented**:
  `ContentRingBuffer` enforces two simultaneous eviction constraints:
  1. Time window: 3 seconds (`max_time_window`).
  2. Byte budget: 8 MiB (`max_bytes`).
  3. Epoch purge: When a chunk with `stream_id > current` arrives, the buffer immediately purges all older chunks.
- **Audit Questions**:
  1. Can any combination of timestamps (e.g., negative duration, timestamp rollback within the same epoch, extreme future timestamp) cause the ring buffer to fail to evict or enter an infinite eviction loop?
  2. Does `find_from(stream_id, timestamp, pos)` correctly prevent returning chunks across an epoch boundary?
  3. How does the eviction logic perform under concurrent reads and writes when wrapped in `Arc<RwLock<...>>`?

---

### Challenge F: JSON-RPC 2.0 Management API & Station Client Parity
- **Files of Interest**:
  - `crates/peercast-rpc/src/server.rs`
  - `crates/peercast-rpc/src/protocol.rs`
  - `crates/peercast-rpc/src/security.rs`
  - C# Reference: `PeerCastStation.UI.HTTP/APIHost.cs`
- **The Difficulty**:
  The web UI and third-party tools interact with PeerCastStation via JSON-RPC 2.0 at `/api/1`. The API host must support single calls, batch requests, and notifications (which return no content / HTTP 204). Remote administration requires token authentication, whereas loopback requests are granted Admin access. Crucially, methods that are deferred or unported must not return dummy success; they must return explicit JSON-RPC error objects.
- **Audit Questions**:
  1. Does `handle_request_str` strictly conform to the JSON-RPC 2.0 specification for batch requests containing both method calls and notifications?
  2. Is the access control check (`AccessLevel::Admin` vs `AccessLevel::Playback`) airtight? Can an external remote client forge loopback status (e.g. via `X-Forwarded-For` or IPv4-mapped IPv6)?
  3. Are sensitive settings (passwords, auth tokens) thoroughly redacted from RPC responses and logs?

---

### Challenge G: Asynchronous TCP Listener & Graceful Shutdown Drain
- **Files of Interest**:
  - `crates/peercast-app/src/server.rs`
  - `crates/peercast-app/src/lifecycle.rs`
  - `crates/peercast-app/src/shutdown.rs`
  - `crates/peercast-app/src/main.rs`
- **The Difficulty**:
  When an OS shutdown signal (SIGINT/SIGTERM on Unix; Ctrl+C/Break/Close on Windows) arrives, the application must:
  1. Immediately stop accepting new incoming TCP connections.
  2. Notify all active tasks and peer connections via a broadcast channel.
  3. Drain active tasks within a configured timeout (`shutdown_timeout_secs`).
  4. Ensure all atomic configuration files are flushed to disk.
- **Audit Questions**:
  1. In `crates/peercast-app/src/server.rs`, is the `tokio::select!` loop between `listener.accept()` and `shutdown_rx.recv()` clean? Could an accepted socket be dropped mid-handshake without completing graceful teardown?
  2. Does `TaskTracker` accurately track all spawned background connection tasks?
  3. In `crates/peercast-app/src/persistence.rs`, is the temporary-file-rename atomic write (`peercast.json.tmp` -> `peercast.json`) safe on Windows across file locking and antivirus scanners?

---

## 3. Specific Auditor Checklist

Please systematically evaluate the following checklist and report your findings:

| Category | Specific Check Items |
| :--- | :--- |
| **Protocol Equivalence** | - Are binary atom tags and serialization byte-for-byte identical to C#?<br>- Does `derive_broadcast_channel_id` generate identical hashes to C# `BroadcastChannel.CreateChannelID`?<br>- Is IPv4 and IPv6 byte-reversal consistently applied across all network operations? |
| **Memory & Safety** | - Are there any `unsafe` blocks? (Goal: 0 `unsafe` blocks across workspace).<br>- Are all allocations bounded by strict byte budgets?<br>- Are buffers protected against memory exhaustion from oversized HTTP bodies or PCP atoms? |
| **Concurrency & Async** | - Are locks (`RwLock`, `Mutex`) held across `.await` points?<br>- Are there any possibilities of deadlocks between channel locks and peer locks?<br>- Are cancellation tokens and broadcast shutdown channels properly propagated? |
| **Security & SSRF** | - Does reverse reachability probing validate target addresses to prevent SSRF against internal LANs?<br>- Is remote administration disabled by default and gated by minimum token length?<br>- Are all HTML/XML playlist outputs properly sanitized against header injection? |
| **Error Handling** | - Do demuxers or network parsers ever panic on corrupt or fuzz inputs?<br>- Are all errors represented via typed `thiserror` enums? |

---

## 4. Expected Audit Report Structure

Please organize your output using the following format:

1. **Executive Summary**: Brief verdict on codebase quality, equivalence status, and critical findings.
2. **High-Severity Findings (Critical / High)**: Detailed description of any bugs, security vulnerabilities, or protocol divergences, including file, line numbers, proof of concept, and suggested fix.
3. **Medium & Low-Severity Findings**: Subtle edge cases, potential performance bottlenecks, or missing spec nuances.
4. **Subsystem Verification Scores**: Tabular rating (0-100%) for each crate's functional equivalence to the C# upstream.
5. **Concrete Action Items & Proposed Patches**: Clear diffs or code recommendations to achieve 100% equivalence and bulletproof safety.
