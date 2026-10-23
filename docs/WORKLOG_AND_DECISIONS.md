# PeerCastStation Rust Port: Work Log & Architectural Decision Records (ADR)

> **Document Purpose**: This file serves as the primary master log and continuity document for any subsequent LLM agent or human engineer working on this repository. It records every architectural decision, justification, code-level rationale, phase progression, and immediate next steps.
> **Last Updated**: 2026-10-24
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

During our investigation of `C:\Github\_upstream_peercaststation`, we examined the core architecture, protocols, and data models:

### 2.1 The Atom System (`PeerCastStation.Core/Atom.cs`)
- All binary PCP communication is built around **Atoms**.
- An Atom consists of:
  - `Name`: A 4-byte FourCC identifier (`ID4`), such as `helo`, `oleh`, `chan`, `pkt `.
  - `Length/Flags`: A 32-bit unsigned integer (Little-Endian).
    - If MSB (`0x8000_0000`) is 0: it is a **Leaf Atom**; the lower 31 bits define the byte length of the value.
    - If MSB (`0x8000_0000`) is 1: it is a **Container Atom**; the lower 31 bits specify the number of nested child atoms.
  - Upstream contains a security check: `(len & 0x7FFFFFFF) > 1024` throws `InvalidDataException("Atom has too many children")`.
- **Rust Translation Note**: In Rust, `Atom` is modeled as an `enum`:
  ```rust
  pub enum Atom {
      Leaf { name: ID4, value: bytes::Bytes },
      Container { name: ID4, children: Vec<Atom> },
  }
  ```
  This is compact, memory-safe, and maps directly to pattern matching.

### 2.2 Content Buffer Pipeline (`Content.cs`, `Ringbuffer.cs`)
- Streams are sliced into `Content` objects containing: `Stream` (ID), `Timestamp` (TimeSpan), `Position` (byte offset), `ContFlag` (continuation bitmask), and `Data` (ReadOnlyMemory<byte>).
- `ContentCollection` stores contents in an `ImmutableSortedSet<Content>` and evicts packets older than `PacketTimeLimit` (default 3 seconds) or past stream IDs.
- **Rust Translation Note**: We replace `ImmutableSortedSet` with a dedicated bounded ring buffer (`ContentRingBuffer`) utilizing `bytes::Bytes` and lock-free/parking_lot-guarded sliding windows, preventing GC allocations and unbounded growth.

### 2.3 Handshake & Routing Flow
- **PCP Handshake**:
  1. Client sends `helo` (containing `agnt`, `sid`, `port`, `ver`, `bcid`).
  2. Server responds with `oleh` (containing `rip`, `port`, `sid`).
  3. Server sends `ok` or `quit`.
- **Channel Delivery**:
  1. Header broadcast: When a client connects, it must receive the codec initialization header atom (`head`) before media data atoms (`data`).
  2. Relay Tree: Nodes exchange `bcst` (Gossip broadcast) with `hops` and `ttl` to discover parent nodes, active listeners, and track channel topologies.

---

## 3. Architectural Decision Records (ADR)

### ADR-0001: Multi-Crate Cargo Workspace Structure
- **Context**: Upstream is divided into `PeerCastStation.Core`, `PeerCastStation.PCP`, `PeerCastStation.HTTP`, `PeerCastStation.UI.HTTP`, and format parsers.
- **Decision**: Organize the Rust codebase as a Cargo workspace with granular crates:
  1. `peercast-core`: Pure data types (`ID4`, `Atom`, `Host`, `Content`, `ContentRingBuffer`, errors). Zero network dependencies.
  2. `peercast-pcp`: PCP protocol framing, handshake state machine, PCP source/output streams.
  3. `peercast-format`: Media format inspection (FLV, TS, ASF, MKV, Ogg, Raw).
  4. `peercast-http`: HTTP streaming engine for player clients (`/stream/{id}`, `/pls/`, `/m3u`), chunked HTTP transfer.
  5. `peercast-servent`: Central channel manager, routing, gossip propagation, bandwidth throttling.
  6. `peercast-rpc`: JSON-RPC 2.0 API server.
  7. `peercast-web`: Embedded static Web UI assets.
  8. `peercast-app`: CLI daemon binary entry point.
- **Rationale**: Enables independent unit testing (especially `peercast-core` without Tokio runtime), accelerates compilation, and provides clean separation of concerns for subsequent AI agents working on specific layers.

### ADR-0002: Asynchronous Runtime & Networking Stack (Tokio)
- **Context**: PeerCast requires high-concurrency TCP connection management, timers, and streaming broadcast.
- **Decision**: Adopt `tokio` (multi-threaded runtime) with `tokio-util` (Framed codec abstractions).
- **Rationale**: Tokio is the de facto standard in production Rust for network systems, providing unmatched performance, rich ecosystem support (`tracing`, `hyper`, `axum`), and robust cancellation/timer mechanisms.

### ADR-0003: Memory Management & Zero-Copy Packet Model (`bytes::Bytes`)
- **Context**: Streaming media packets arrive continuously and must be distributed to N downstream listeners simultaneously. Upstream C# copies bytes or relies on GC-managed `ReadOnlyMemory<byte>`.
- **Decision**: Represent all packet data using `bytes::Bytes`.
- **Rationale**: `bytes::Bytes` uses an atomic reference counter (`Arc`) around a heap slice. Slicing and cloning a `Bytes` instance is an $O(1)$ pointer increment without memory duplication. This eliminates packet copying across all fan-out relay loops.

### ADR-0004: Explicit Enum-Based Asynchronous State Machines & Re-entrancy Guards
- **Context**: Ad-hoc boolean flags (`isConnecting`, `isStreaming`, `isClosed`) lead to race conditions, multi-firing bugs, and undefined connection states.
- **Decision**: Mandate strict Rust `enum` state representations for all connection and channel lifecycles. All transition functions must implement early-return guards if an event occurs in an unexpected phase.
- **Rationale**: Enforces compile-time exhaustiveness checks and adheres strictly to Global Engineering Rule #2.

### ADR-0005: Resource Bounding & Longevity by Design (Ringbuffer & Connection Limits)
- **Context**: Long-running streaming nodes can suffer from memory leaks, unbounded node lists, and descriptor exhaustion.
- **Decision**:
  - Enforce explicit capacity limits on `ContentRingBuffer` (both maximum byte count and sliding time window).
  - Impose maximum child limits on Atom parsing (`1024` child atoms, `16 MB` max payload).
  - Enforce strict concurrency limits on incoming listeners (`max_relays`, `max_directs`).
  - Drop slow/stalled downstream sockets immediately when their read cursor falls outside the ring buffer window.
- **Rationale**: Adheres to Global Engineering Rule #3. Guarantees that memory footprint remains constant regardless of stream length or viewer count.

### ADR-0006: Web UI Embedding & Headless-First Architecture
- **Context**: PeerCastStation includes an HTML management interface.
- **Decision**: Embed static assets (`channels.html`, `relays.html`, `settings.html`, css, js) directly into the binary using `rust-embed`. Serve via an embedded HTTP server alongside the JSON-RPC endpoints.
- **Rationale**: Delivers a single, frictionless, portable binary with no external asset folder dependencies.

---

## 4. Step-by-Step Execution Log

| Date | Phase | Action / Milestone | Result / Notes |
| :--- | :--- | :--- | :--- |
| **2026-10-24** | **Setup** | Workspace verification & directory setup | Initialized repository at `C:\Github\PeerCastStation_Rust`. Shallow cloned upstream reference to `C:\Github\_upstream_peercaststation`. |
| **2026-10-24** | **Analysis** | C# Codebase architecture & protocol deep-dive | Analyzed `Atom.cs`, `Ringbuffer.cs`, `Content.cs`, `Channel.cs`, `PCPOutputStream.cs`, `HTTPOutputStream.cs`, and upstream test suites. |
| **2026-10-24** | **Docs** | Authored `docs/SPECIFICATION.md` | Comprehensive architectural and protocol specification created in English. |
| **2026-10-24** | **Docs** | Authored `docs/WORKLOG_AND_DECISIONS.md` | Master continuity log and ADR framework created for future LLM agents. |

---

## 5. Current State & Verification Status

- **Repository Structure**:
  - `docs/SPECIFICATION.md` (Architecture, Protocol, State Machines, Roadmap)
  - `docs/WORKLOG_AND_DECISIONS.md` (This document)
- **Next Phase Ready**: Phase 1: Creating Cargo Workspace and implementing `peercast-core`.

---

## 6. Actionable Next Steps for Future LLM Agents

When resuming work on this repository, proceed in the following prioritized order:

### Priority 1: Initialize Cargo Workspace & `peercast-core`
1. Create root `Cargo.toml` defining the workspace members:
   ```toml
   [workspace]
   members = [
       "crates/peercast-core",
       "crates/peercast-pcp",
       "crates/peercast-format",
       "crates/peercast-http",
       "crates/peercast-servent",
       "crates/peercast-rpc",
       "crates/peercast-web",
       "crates/peercast-app",
   ]
   resolver = "2"
   ```
2. Implement `crates/peercast-core`:
   - `id4.rs`: 4-byte identifier (`ID4`), ASCII conversion, standard constants (`PCP_HELO`, `PCP_CHAN`, etc.).
   - `atom.rs`: `Atom` enum, recursive binary serializer (`write_to`), and deserializer (`read_from`).
   - `content.rs`: `Content`, `ContentFlags`.
   - `ringbuffer.rs`: `ContentRingBuffer` with bounded memory capacity and sliding time-to-live eviction.
   - `host.rs`: Node descriptors (`Host`, `HostFlags`).
   - `tests/test_atom.rs`: Unit tests verifying serialization/deserialization against upstream byte patterns.

### Priority 2: Implement `peercast-pcp` Wire Codec & Handshake
1. Implement `tokio_util::codec::Decoder` and `Encoder` for PCP Atoms in `crates/peercast-pcp`.
2. Implement `HandshakeState` state machine for `helo` <-> `oleh` negotiation.
3. Test with mock streams.

---

## 7. Engineering Protocols & Conventions for Agents

Subsequent LLM agents working on this project must strictly observe:

1. **PowerShell Execution Rigor (Global Rule #8)**:
   - NEVER run inline one-liners with quotes/variables like `pwsh -Command "node -e '...'"` or complex multi-line bash scripts directly.
   - For any complex script or multi-line command, write a temporary `.ps1` or `.rs` file, execute it, and clean it up.
2. **Resource Boundedness (Global Rule #3)**:
   - Ensure every collection, channel, and buffer has an explicit maximum capacity limit.
   - Implement clean drop/teardown handlers for socket connections.
3. **State Machine Integrity (Global Rule #2)**:
   - Never use combinational booleans (`is_active && !is_closed && is_connecting`) to represent asynchronous lifecycles. Use exhaustive Rust enums.
   - Guard entry points against re-entrant calls.
4. **Log Updates**:
   - At the conclusion of every turn/milestone, update Section 4 (Execution Log) and Section 5 (Current State) of this document to ensure seamless handover to future agents.
