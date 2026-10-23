# PeerCastStation Rust Port: Work Log & Architectural Decision Records (ADR)

> **Document Purpose**: This file serves as the primary master log and continuity document for any subsequent LLM agent or human engineer working on this repository. It records every architectural decision, justification, code-level rationale, phase progression, and immediate next steps.
> **Last Updated**: 2026-10-24 (Post-Review Refinement)
> **Repository Root**: `C:\Github\PeerCastStation_Rust`
> **Upstream Reference**: `https://github.com/kumaryu/peercaststation` (Cloned locally for reference at `C:\Github\_upstream_peercaststation`)
> **Rust Toolchain**: `rustc 1.98.1` / `cargo 1.98.1` (Edition 2021)

---

## Table of Contents
1. [Project Mission & Background](#1-project-mission--background)
2. [Upstream C# Codebase Deep-Dive Analysis](#2-upstream-c-codebase-deep-dive-analysis)
3. [Architectural Decision Records (ADR)](#3-architectural-decision-records-adr)
   - [ADR-0001: Multi-Crate Cargo Workspace Structure](#adr-0001-multi-crate-cargo-workspace-structure)
   - [ADR-0002: Asynchronous Runtime & Networking Stack (Tokio)](#adr-0002-asynchronous-runtime--networking-stack-tokio)
   - [ADR-0003: Memory Management & Zero-Copy Packet Model (`bytes::Bytes`)](#adr-0003-memory-management--zero-copy-packet-model-bytesbytes)
   - [ADR-0004: Explicit Enum-Based Asynchronous State Machines & Re-entrancy Guards](#adr-0004-explicit-enum-based-asynchronous-state-machines--re-entrancy-guards)
   - [ADR-0005: Resource Bounding & Longevity by Design (Ringbuffer & Connection Limits)](#adr-0005-resource-bounding--longevity-by-design-ringbuffer--connection-limits)
   - [ADR-0006: Web UI Embedding & Headless-First Architecture](#adr-0006-web-ui-embedding--headless-first-architecture)
   - [ADR-0007: Two-Stage Handshake (HTTP Request followed by Binary Atom Stream)](#adr-0007-two-stage-handshake-http-request-followed-by-binary-atom-stream)
   - [ADR-0008: Active Ping Probe Protocol for Port Reachability Verification](#adr-0008-active-ping-probe-protocol-for-port-reachability-verification)
   - [ADR-0009: Codec Initialization Header Cache for Late Joiners](#adr-0009-codec-initialization-header-cache-for-late-joiners)
4. [Step-by-Step Execution Log](#4-step-by-step-execution-log)
5. [Current State & Verification Status](#5-current-state--verification-status)
6. [Actionable Next Steps for Future LLM Agents](#6-actionable-next-steps-for-future-llm-agents)
7. [Engineering Protocols & Conventions for Agents](#7-engineering-protocols--conventions-for-agents)

---

## 1. Project Mission & Background

### 1.1 Context
PeerCast is a decentralized peer-to-peer (P2P) audio/video streaming network. Broadcasters stream content to viewers, who in turn relay the stream to other viewers, creating a dynamic distribution tree without requiring high-bandwidth central servers.
**PeerCastStation** (created by kumaryu in C# / .NET) is the modern standard implementation, offering enhanced stability, multi-format media support (FLV, MPEG-TS, WMV/ASF, MKV), IPv6 support, and a comprehensive JSON-RPC / HTML interface.

### 1.2 Motivation for Rust Port
While the .NET version is mature and feature-rich, porting PeerCastStation to Rust delivers crucial advantages:
- **Zero-Cost Abstractions & Minimal Memory Footprint**: Crucial for running 24/7 on lightweight routers, Raspberry Pi, VPS instances, and NAS devices.
- **Fearless Concurrency without Garbage Collection Pauses**: High-throughput media packet forwarding benefits enormously from predictable latency free of GC pauses.
- **Single Self-Contained Binary**: Combining the daemon, HTTP player server, PCP engine, and embedded Web UI into a single native executable with no runtime dependencies (.NET Runtime / Mono).

---

## 2. Upstream C# Codebase Deep-Dive Analysis

During our thorough review of `C:\Github\_upstream_peercaststation`, we examined every critical subsystem:

### 2.1 The Two-Stage Handshake Discovery (`PCPSourceStream.cs` & `PCPOutputStream.cs`)
- **Key Insight**: A common misconception is that PeerCast connects purely over raw binary Atoms. Inspection of `PCPSourceStream.cs` reveals that all relay connections start with an HTTP request:
  ```http
  GET /channel/{channel_id} HTTP/1.0\r\n
  User-Agent: {agent}\r\n
  x-peercast-pcp: 1\r\n
  x-peercast-pos: {stream_pos}\r\n
  \r\n
  ```
- **Responses**:
  - `200 OK`: Transition socket directly into binary Atom mode.
  - `503 Service Unavailable`: Upstream relay slots are full. Upstream sends `PCP_HOST` atoms with alternate relay candidates and disconnects.
  - `404 Not Found`: Off-air.
- **Implication for Rust**: The connection codec must support an initial HTTP parser phase before upgrading the stream into `tokio_util::codec::Framed<_, AtomCodec>`.

### 2.2 Active Ping Probe (`PCPOutputStream.cs` L331-L382)
- Upstream tests whether connecting downstream relays have open ports by performing an **active reverse TCP probe**:
  - Upstream opens a TCP connection to downstream's advertised IP and ping port.
  - Upstream sends `PCP_CONNECT` (`pcnt`) and `PCP_HELO`.
  - Downstream returns `PCP_OLEH` with its `SessionID`.
  - If `SessionID` matches, downstream is verified as `Firewalled = false` (Port Open).

### 2.3 The Header Caching Mechanism (`FLVContentBuffer.cs`, `TSContentReader.cs`)
- Video/audio streams cannot be rendered by late-connecting viewers without codec initialization headers (SPS/PPS in H.264, AudioSpecificConfig in AAC, PAT/PMT in MPEG-TS).
- Upstream sniffs incoming streams, extracts these headers, and holds them in `Channel.ContentHeader`.
- Whenever a new player or relay connects, this header is transmitted *first*, preceding any live content packets.

### 2.4 Backpressure & 5-Second Lag Overflow (`PCPOutputStream.cs` L243)
- Upstream monitors queue latency:
  `if (!queue.TryPeek(out var nxtMsg) || (msg.Timestamp - nxtMsg.Timestamp).TotalMilliseconds <= 5000)`
- If a downstream relay lags by more than 5,000 milliseconds, an `Overflow` message is emitted and the socket is dropped with `StopReason::SendTimeoutError`.

---

## 3. Architectural Decision Records (ADR)

### ADR-0001: Multi-Crate Cargo Workspace Structure
- **Decision**: Organize the Rust codebase into granular crates: `peercast-core`, `peercast-pcp`, `peercast-format`, `peercast-http`, `peercast-yp`, `peercast-servent`, `peercast-rpc`, `peercast-web`, `peercast-app`.
- **Rationale**: Isolates pure data structures (`peercast-core`) from network I/O, allowing blazing-fast unit tests and clear task division for AI agents.

### ADR-0002: Asynchronous Runtime & Networking Stack (Tokio)
- **Decision**: Adopt Tokio with multi-thread runtime and `tokio-util` Framed codecs.
- **Rationale**: Standard in production-grade Rust for network I/O, timers, and actor-like channels.

### ADR-0003: Memory Management & Zero-Copy Packet Model (`bytes::Bytes`)
- **Decision**: Model all media packet payloads as `bytes::Bytes`.
- **Rationale**: $O(1)$ reference-counted cloning across unlimited relay fan-out without heap reallocations.

### ADR-0004: Explicit Enum-Based Asynchronous State Machines & Re-entrancy Guards
- **Decision**: Mandate Rust `enum` state machines for connections and channels. Guard against re-entrant function execution (Rule #2).

### ADR-0005: Resource Bounding & Longevity by Design (Ringbuffer & Connection Limits)
- **Decision**: Enforce strict 8 MB / 3-second capacity on `ContentRingBuffer`, max 1024 children per Atom, and 5-second queue overflow drop (Rule #3).

### ADR-0006: Web UI Embedding & Headless-First Architecture
- **Decision**: Embed upstream static HTML/JS assets into the binary using `rust-embed`.

### ADR-0007: Two-Stage Handshake (HTTP Request followed by Binary Atom Stream)
- **Context**: Upstream relays initiate connections using HTTP/1.0 request headers before switching to binary Atoms.
- **Decision**: `peercast-pcp` implements a two-stage framing pipeline:
  1. An initial HTTP request/response line parser.
  2. Upon receiving `200 OK`, transitions the stream to `AtomCodec`.
- **Rationale**: Guarantees 100% interoperability with upstream PeerCastStation and original PeerCast nodes.

### ADR-0008: Active Ping Probe Protocol for Port Reachability Verification
- **Context**: Nodes must autonomously verify whether their ports are accessible from the global Internet.
- **Decision**: Implement the reverse TCP ping probe sequence in `peercast-pcp`:
  - Upstream connects back using `pcnt` + `helo` to confirm downstream port status before advertising it to the network as a relayable host.

### ADR-0009: Codec Initialization Header Cache for Late Joiners
- **Context**: Stream players joining midway fail to decode video without initial SPS/PPS and metadata.
- **Decision**: `peercast-format` parses incoming streams to capture codec initialization packets (`ContentHeader`). The channel actor caches this header and guarantees it is sent as the very first packet to any newly connected relay or player.

---

## 4. Step-by-Step Execution Log

| Date | Phase | Action / Milestone | Result / Notes |
| :--- | :--- | :--- | :--- |
| **2026-10-24** | **Setup** | Workspace verification & directory setup | Initialized repository at `C:\Github\PeerCastStation_Rust`. Shallow cloned upstream reference to `C:\Github\_upstream_peercaststation`. |
| **2026-10-24** | **Analysis** | Upstream code deep-dive (Initial) | Analyzed `Atom.cs`, `Ringbuffer.cs`, `Content.cs`, `Channel.cs`. |
| **2026-10-24** | **Docs** | Authored `SPECIFICATION.md` & `WORKLOG_AND_DECISIONS.md` v1.0 | Initial comprehensive architectural specification and ADR log created. |
| **2026-10-24** | **Review** | Upstream code deep-dive (Thorough Self-Review) | Inspected `PCPSourceStream.cs`, `PCPOutputStream.cs`, `FLVContentBuffer.cs`, `HTTPOutputStream.cs`, `PCPYellowPageClient.cs`. Discovered two-stage handshake, reverse ping probe, and 5-second overflow limit. |
| **2026-10-24** | **Refine** | Specification & Worklog v1.1 Refinement | Updated `docs/SPECIFICATION.md` and added ADR-0007, ADR-0008, ADR-0009 to `docs/WORKLOG_AND_DECISIONS.md`. |

---

## 5. Current State & Verification Status

- **Documentation**:
  - `docs/SPECIFICATION.md` v1.1.0: Complete, refined, wire-accurate specification.
  - `docs/WORKLOG_AND_DECISIONS.md`: Complete work log with 9 comprehensive ADRs.
- **Next Milestone**: Ready for **Phase 1 Implementation** (`peercast-core`).

---

## 6. Actionable Next Steps for Future LLM Agents

### Priority 1: Initialize Workspace & Implement `peercast-core`
1. Create root `Cargo.toml` with workspace configuration:
   ```toml
   [workspace]
   members = [
       "crates/peercast-core",
       "crates/peercast-pcp",
       "crates/peercast-format",
       "crates/peercast-http",
       "crates/peercast-yp",
       "crates/peercast-servent",
       "crates/peercast-rpc",
       "crates/peercast-web",
       "crates/peercast-app",
   ]
   resolver = "2"
   ```
2. In `crates/peercast-core`:
   - `id4.rs`: FourCC identifier struct `ID4([u8; 4])` with ASCII Display, equality, and standard constants.
   - `atom.rs`: `Atom` enum (Leaf & Container) with recursive binary serializer and deserializer adhering to the 1024-child and 16 MB bounds.
   - `content.rs`: `Content` struct (`stream_id`, `pos`, `timestamp`, `cont_flags`, `data: Bytes`, `serial`).
   - `ringbuffer.rs`: `ContentRingBuffer` implementing sliding-window eviction (> 3s or > 8 MB).
   - `host.rs`: Node representation (`Host`, `HostFlags1`).
   - `tests/test_atom.rs`: Unit tests matching upstream `TestCore/test_core_atom.rb`.

### Priority 2: Implement `peercast-pcp` Handshake & Wire Codec
- Implement two-stage HTTP-to-Atom handshake and `tokio_util::codec` encoder/decoder.

---

## 7. Engineering Protocols & Conventions for Agents

1. **PowerShell Scripting Rigor (Global Rule #8)**:
   - NEVER execute inline one-liners with quotes/variables (`node -e "..."` or complex pwsh command strings). Always write a temporary `.ps1` or `.rs` file, run it, and delete it.
2. **Resource Boundedness (Global Rule #3)**:
   - Ensure every collection and channel has an explicit maximum bound.
3. **State Machine Rigor (Global Rule #2)**:
   - Model asynchronous lifecycles with exhaustive Rust `enum` states. Prevent re-entrant state transitions.
4. **Log Continuity**:
   - Update Section 4 and Section 5 of this document after completing any phase or milestone.
