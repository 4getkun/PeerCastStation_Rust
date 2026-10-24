# PeerCastStation (Rust Edition): Index of Key Files & Architectural Directory

- **Last Updated**: 2026-10-24
- **Language**: English
- **Scope**: Comprehensive index and technical summary of all core architecture, protocol, streaming, API, and orchestration files across the workspace.

---

## 1. Architectural Blueprint & Crate Hierarchy

The PeerCastStation Rust codebase is structured into six modular, highly decoupled crates following strict inward-pointing dependencies:

```text
[peercast-app] (Binary & Orchestration: CLI, Lifecycle, Config, TCP Server)
      │
      ├──► [peercast-http] (HTTP Streaming, Framing, Playlists, Embedded Web Assets)
      │          │
      │          ├──► [peercast-format] (Media Demuxers: FLV, TS, ASF, MKV, Ogg, MP3)
      │          │
      ├──► [peercast-rpc] (JSON-RPC 2.0 Management API, Controllers, ViewModels)
      │          │
      ├──► [peercast-pcp] (PCP P2P Overlay Protocol, Handshakes, Probing, YP Announce)
                 │
                 └──► [peercast-core] (Domain Primitives: ID4, Atoms, ChannelID, RingBuffer)
```

---

## 2. Master File Index by Crate

### 2.1 Domain Primitives & Core Servent Logic (`crates/peercast-core`)

| File & Link | Primary Responsibility & Invariants | Key Types & Functions |
| :--- | :--- | :--- |
| [`lib.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/lib.rs) | Crate root exporting core domain models, errors, atom codecs, and ring buffers. | Module exports |
| [`id4.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/id4.rs) | 4-byte FourCC protocol identifiers (`ID4`). Handles wire serialization, NUL-padding (`\0`), and standard PCP message tags (`helo`, `oleh`, `quit`, `chan`, `pcp\n`). | `ID4`, `Id4Error`, `PCP_CONNECT`, `CHAN_PKT` |
| [`atom.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/atom.rs) | Binary PCP Atom structures, hierarchical recursive decoding/encoding, and streaming parser protection with strict memory budgets (`AtomBudget`). | `Atom`, `AtomBudget`, `AtomError`, `decode_stream` |
| [`scalar.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/scalar.rs) | Little-endian numeric primitives, NUL-terminated UTF-8 strings, and historical PeerCast byte-reversed IP address serialization (`read_ip_reversed`, `write_ip_reversed`). | `Scalar`, `ScalarError`, `read_ip_reversed`, `write_ip_reversed` |
| [`channel_id.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/channel_id.rs) | 16-byte Channel / Broadcast / Session ID. Implements .NET `Guid` memory layout emulation (`to_dotnet_guid_bytes`), 7-bit string length encoding, and deterministic SHA-512 + MD5 channel derivation. | `ChannelId`, `ChannelNetworkType`, `derive_broadcast_channel_id` |
| [`content.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/content.rs) | Broadcast media chunk payload container (`Content`), continuation flag bitmask (`ContinuationFlags`: Fragment, InterFrame, AudioFrame). | `Content`, `ContinuationFlags` |
| [`ringbuffer.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/ringbuffer.rs) | Dual-bounded sliding-window media ringbuffer (`ContentRingBuffer`). Enforces 3-second time window and 8 MiB memory budget; immediately purges prior stream epochs on broadcast restart. | `ContentRingBuffer`, `new`, `push`, `evict`, `find_from` |
| [`cache.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/cache.rs) | Stream header caching and immutable snapshot retention (`HeaderSnapshot`). | `BoundedContentCache`, `HeaderSnapshot`, `CacheClock` |
| [`host.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/host.rs) | P2P node descriptor (`Host`), socket endpoints, session IDs, hop counts, and relay capability flags (`HostFlags1`). | `Host`, `HostFlags1` |
| [`host_registry.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/host_registry.rs) | Bounded candidate host tracking table, freshness aging, failure cooldowns, and SSRF-safe host filtering. | `HostRegistry`, `EndpointCooldownTable`, `is_candidate_eligible` |
| [`parent_selection.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/parent_selection.rs) | Servent upstream parent selection policy (`StationWeightedRandomPolicy`), locality detector (`is_site_local`), and exponential backoff retry budget (`ReconnectBudget`). | `ParentSelectionPolicy`, `StationWeightedRandomPolicy`, `ReconnectBudget` |
| [`gossip.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/gossip.rs) | Bounded gossip broadcast forwarding (`bcst` atom), TTL decrement / hop increment, and storm suppression deduplication cache (`GossipDedupCache`). | `BroadcastGroup`, `GossipDedupCache`, `forward_broadcast` |
| [`index_txt.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/index_txt.rs) | Bounded YellowPage directory listing (`index.txt`) parser. Enforces 19-column `<>` schema, HTML entity decoding, and strict line/byte limits. | `DirectoryChannelEntry`, `IndexParserLimits`, `parse_index_txt` |
| [`errors.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/errors.rs) | P2P disconnection quit reasons and protocol error enums (`QuitReason`). | `QuitReason`, `CoreError` |
| [`actor.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/actor.rs) | Internal asynchronous message-passing contracts and actor command envelopes. | `ActorCommand`, `ActorMessage` |
| [`subscriber.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-core/src/subscriber.rs) | Channel output listener subscription management and fan-out dispatch. | `ChannelSubscriber`, `SubscriberRegistry` |

---

### 2.2 PeerCast Protocol (PCP) Networking (`crates/peercast-pcp`)

| File & Link | Primary Responsibility & Invariants | Key Types & Functions |
| :--- | :--- | :--- |
| [`lib.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-pcp/src/lib.rs) | PCP networking crate root exporting clients, servers, probing, and directory services. | Module exports |
| [`http_prelude.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-pcp/src/http_prelude.rs) | Parses HTTP/1.0 request/response preludes preceding PCP binary handshakes. Enforces `PcpHttpLimits` (8 KiB header cap). | `PcpHttpLimits`, `PcpHttpStatus`, `parse_relay_request`, `parse_relay_response` |
| [`relay_client.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-pcp/src/relay_client.rs) | Downstream P2P relay client state machine (`RelayClientStateMachine`). Drives 200 OK / 503 Unavailable handshakes and parses media streams. | `RelayClientStateMachine`, `RelayClientPhase`, `RelayEvent`, `RelayNetworkProfile` |
| [`relay_server.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-pcp/src/relay_server.rs) | Upstream P2P relay server state machine (`RelayServerStateMachine`). Decides 200 relay vs 503 candidate redirection. | `RelayServerStateMachine`, `RelayServerPhase`, `RelayServerError` |
| [`probe.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-pcp/src/probe.rs) | Reverse reachability probe engine (`ReverseReachabilityProbe`). Verifies firewall openness; supports Station vs YT version profiles. | `ReverseReachabilityProbe`, `ProbePeerProfile`, `ProbeResult` |
| [`yellow_page.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-pcp/src/yellow_page.rs) | YellowPage (YP) channel announcer client (`YpClient`). Manages 30-second periodic updates and exponential backoff retry schedules. | `YpClient`, `YpBackoffTracker`, `DEFAULT_YP_UPDATE_INTERVAL` |
| [`packet.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-pcp/src/packet.rs) | Low-level PCP wire framing and packet buffer chunking utilities. | `PcpPacket`, `PacketFramer` |

---

### 2.3 Container Demuxers & Media Formats (`crates/peercast-format`)

| File & Link | Primary Responsibility & Invariants | Key Types & Functions |
| :--- | :--- | :--- |
| [`lib.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-format/src/lib.rs) | Media format parsing facade exporting container demuxers and keyframe selectors. | Module exports |
| [`flv.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-format/src/flv.rs) | Flash Video (FLV) demuxer. Parses 13-byte header prelude, 11-byte tag headers, 32-bit extended timestamps, and AMF0 metadata. | `FlvDemuxer`, `FlvTagHeader`, `FlvTagType`, `FlvFileHeader` |
| [`ts.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-format/src/ts.rs) | MPEG-2 Transport Stream (TS) demuxer. Enforces 188-byte packet sync (`0x47`), 27 MHz PCR clock scaling, and PAT / PMT section reassembly. | `TsDemuxer`, `TsPacketHeader`, `PatTable`, `PmtTable`, `TsStreamType` |
| [`asf.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-format/src/asf.rs) | Advanced Systems Format (ASF/WMV/WMA) and MMS/WMSP framing parser. Handles 24-byte GUID object headers and header snapshots. | `AsfDemuxer`, `AsfGuid`, `AsfObjectHeader`, `AsfHeaderObject` |
| [`mkv.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-format/src/mkv.rs) | Matroska (MKV / WebM) EBML parser. Extracts track headers, video codecs, and cluster keyframe boundaries. | `MkvDemuxer`, `EbmlElement`, `MkvTrack` |
| [`ogg.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-format/src/ogg.rs) | Ogg bitstream framing parser. Validates page checksums, BOS / EOS flags, and packet granule positions. | `OggDemuxer`, `OggPage`, `OggPageHeader` |
| [`mp3.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-format/src/mp3.rs) | MPEG Audio Layer III frame sync parser (`0xFFE0`). Calculates bitrates, sample rates, and padding lengths. | `Mp3Demuxer`, `Mp3FrameHeader`, `MpegVersion`, `MpegLayer` |
| [`safestart.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-format/src/safestart.rs) | Format-aware keyframe safe-start selector (`SafeStartSelector`). Injects stream initialization headers before admitting new viewers. | `SafeStartSelector`, `SafeStartConfig`, `SafeStartMode`, `SafeStartDecision` |
| [`stream_header.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-format/src/stream_header.rs) | Stream header storage and keyframe boundary marker interfaces. | `StreamHeader`, `StreamHeaderTracker` |
| [`demuxer.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-format/src/demuxer.rs) | Common demuxer interface trait and container format classifier. | `Demuxer`, `ContainerFormat`, `DemuxOutput` |

---

### 2.4 HTTP Protocol, Streaming & Embedded UI (`crates/peercast-http`)

| File & Link | Primary Responsibility & Invariants | Key Types & Functions |
| :--- | :--- | :--- |
| [`lib.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-http/src/lib.rs) | HTTP streaming and embedded web presentation layer facade. | Module exports |
| [`router.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-http/src/router.rs) | Fast zero-allocation HTTP request parser (`HttpRequest`) and path dispatcher (`RouteTarget`: stream, playlist, static asset, RPC API). | `HttpRequest`, `RouteTarget`, `HttpMethod`, `HttpVersion` |
| [`streamer.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-http/src/streamer.rs) | Live media HTTP delivery engine (`resolve_framing`). Strictly enforces `RawContinuous` on HTTP/1.0 and ASF/MMS; uses `Chunked` for HTTP/1.1 media. | `HttpFraming`, `resolve_framing`, `resolve_mime_type`, `build_http_200_headers` |
| [`playlist.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-http/src/playlist.rs) | Playlist file generator (`PlaylistBuilder`: M3U, PLS, ASX). Features XML escaping and CRLF header injection sanitization. | `PlaylistBuilder`, `PlaylistFormat`, `PlaylistEntry`, `sanitize_single_line` |
| [`embedded_assets.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-http/src/embedded_assets.rs) | Compile-time binary asset embedding (`include_bytes!`). Serves self-contained Web UI, design tokens, themes, SHA-256 ETags, and strict CSP. | `EmbeddedAsset`, `get_embedded_asset`, `build_asset_http_response` |
| [`ingest.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-http/src/ingest.rs) | HTTP push ingest server (`HttpPushListener`). Validates incoming broadcaster chunked uploads, tokens, and buffer caps. | `HttpPushConfig`, `HttpPushListener`, `MAX_INGEST_CHUNK_SIZE` |

---

### 2.5 JSON-RPC 2.0 Management API (`crates/peercast-rpc`)

| File & Link | Primary Responsibility & Invariants | Key Types & Functions |
| :--- | :--- | :--- |
| [`lib.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-rpc/src/lib.rs) | JSON-RPC 2.0 management API facade. | Module exports |
| [`protocol.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-rpc/src/protocol.rs) | JSON-RPC 2.0 wire protocol data contracts (`RpcRequest`, `RpcResponse`, `RpcErrorObject`, `RpcId`). | `RpcRequest`, `RpcResponse`, `RpcErrorObject`, `RpcId` |
| [`server.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-rpc/src/server.rs) | Core JSON-RPC 2.0 dispatcher (`RpcDispatcher`). Implements Station method table (`getStatus`, `getChannels`, `getSettings`, `setSettings`) with access controls. | `RpcDispatcher`, `RpcState`, `AccessLevel`, `MethodAccessRequired` |
| [`controller.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-rpc/src/controller.rs) | Presentation controllers (`IndexController`, `ChannelsController`, `SettingsController`). Enforces explicit `ScreenState` and sequence guards. | `IndexController`, `ChannelsController`, `SettingsController`, `ScreenState`, `MutationState` |
| [`client.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-rpc/src/client.rs) | Normalized presentation view-models (`StatusViewModel`, `ChannelViewModel`) and retry classifiers (`RetryClassification`). | `PeerCastRpcClient`, `StatusViewModel`, `ChannelViewModel`, `RetryClassification` |
| [`security.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-rpc/src/security.rs) | Security policies, access enforcement, secret redaction, and SSRF destination validation. | `SecurityPolicy`, `SecureRpcHandler`, `redact_secrets`, `validate_relay_destination` |

---

### 2.6 Application Orchestration & Lifecycle (`crates/peercast-app`)

| File & Link | Primary Responsibility & Invariants | Key Types & Functions |
| :--- | :--- | :--- |
| [`lib.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-app/src/lib.rs) | Orchestration crate root exporting configuration, metrics, lifecycle, and server types. | Module exports |
| [`main.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-app/src/main.rs) | Application entry point. Parses CLI flags, initializes logging, runs config persistence, hooks OS signals, and runs HTTP server. | `main` |
| [`server.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-app/src/server.rs) | Asynchronous TCP server listener (`HttpServer`). Accepts incoming connections on port 7144, tracks active tasks, and routes HTTP/RPC traffic. | `HttpServer`, `new`, `run`, `handle_connection` |
| [`cli.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-app/src/cli.rs) | CLI argument parser (`AppCliArgs`). Handles `--port`, `--bind`, `--config`, `--kill`, `--help`, and `--version` with precedence over stored config. | `AppCliArgs`, `parse_from`, `apply_to_config`, `help_text` |
| [`config.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-app/src/config.rs) | Strongly-typed configuration (`AppConfig`, `ServerConfig`, `LogConfig`, `LifecycleConfig`). Validates invariants and manages schema versions. | `AppConfig`, `ServerConfig`, `ConfigValidationError`, `CURRENT_SCHEMA_VERSION` |
| [`persistence.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-app/src/persistence.rs) | Atomic configuration manager (`ConfigManager`). Uses temporary file rename (`.tmp` -> `.json`), validates schemas, and preserves pre-migration backups. | `ConfigManager`, `ConfigPersistenceError` |
| [`lifecycle.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-app/src/lifecycle.rs) | Explicit application lifecycle state machine (`AppLifecycleManager`: `Initial` -> `Running` -> `Stopping` -> `Stopped`) and background task drain tracker (`TaskTracker`). | `AppLifecycleManager`, `LifecyclePhase`, `ShutdownReason`, `TaskTracker` |
| [`shutdown.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-app/src/shutdown.rs) | Platform-specific OS shutdown signal hook. Listens for Ctrl+C, Ctrl+Break, and Close on Windows; SIGINT and SIGTERM on Unix. | `wait_for_os_shutdown_signal`, `PlatformSignalSupport` |
| [`logging.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-app/src/logging.rs) | Bounded circular memory log buffer (`BoundedLogBuffer`). Enforces secret redaction on auth tokens and passwords. | `BoundedLogBuffer`, `LogEntry`, `LogLevel` |
| [`metrics.rs`](file:///C:/Github/PeerCastStation_Rust/crates/peercast-app/src/metrics.rs) | Real-time system metrics tracker (`AppMetrics`). Tracks active connections with RAII guards and records uptime. | `AppMetrics`, `ConnectionGuard`, `MetricsSnapshot` |

---

## 3. Master Documentation & Audit Records (`docs/`)

| Document & Link | Description & Contents |
| :--- | :--- |
| [`SPECIFICATION.md`](file:///C:/Github/PeerCastStation_Rust/docs/SPECIFICATION.md) | Comprehensive engineering specification detailing all 58 requirements (REQ-001 through REQ-087). |
| [`WORKLOG_AND_DECISIONS.md`](file:///C:/Github/PeerCastStation_Rust/docs/WORKLOG_AND_DECISIONS.md) | Exhaustive chronological worklog and architectural decisions recorded across all 50 migration tasks (P01..P44). |
| [`UI_LAYER_BOUNDARIES.md`](file:///C:/Github/PeerCastStation_Rust/docs/UI_LAYER_BOUNDARIES.md) | Inward-pointing UI dependency graph, strict CSP rules, and frontend state machine contracts. |
| [`RPC_CONTRACT.md`](file:///C:/Github/PeerCastStation_Rust/docs/RPC_CONTRACT.md) | JSON-RPC 2.0 schema specification, Station APIHost parity, and access level matrices. |
| [`TARGET_PACKAGING.md`](file:///C:/Github/PeerCastStation_Rust/docs/TARGET_PACKAGING.md) | Production target packaging verification, binary size benchmarks, and dynamic DLL dependencies audit. |
| [`LICENSE_AUDIT.md`](file:///C:/Github/PeerCastStation_Rust/docs/LICENSE_AUDIT.md) | License compliance audit verifying GPL-3.0-or-later and MIT compatibility across all bundled dependencies. |
| [`FUZZ_AND_FAULT_CAMPAIGN.md`](file:///C:/Github/PeerCastStation_Rust/docs/FUZZ_AND_FAULT_CAMPAIGN.md) | Randomized multi-operator fuzzing campaign report (>5,500 iterations) testing atom budgets and container parsers. |
| [`OPERATIONAL_SOAK_REPORT.md`](file:///C:/Github/PeerCastStation_Rust/docs/OPERATIONAL_SOAK_REPORT.md) | High-load operational soak test report verifying 30 peer churn cycles, slow consumer backpressure, and zero task leaks. |
| [`RELEASE_ASSESSMENT.md`](file:///C:/Github/PeerCastStation_Rust/docs/RELEASE_ASSESSMENT.md) | Final production readiness assessment confirming 100% test coverage across all requirements and zero warnings. |
| [`CSHARP_RUST_EQUIVALENCE_AUDIT.md`](file:///C:/Github/PeerCastStation_Rust/docs/CSHARP_RUST_EQUIVALENCE_AUDIT.md) | Deep cross-reading audit comparing C# upstream (`_upstream_peercaststation`) directly with Rust implementations. |
| [`EXTERNAL_LLM_AUDIT_PROMPT.md`](file:///C:/Github/PeerCastStation_Rust/docs/EXTERNAL_LLM_AUDIT_PROMPT.md) | Ready-to-use prompt guide for external AI auditors detailing the 7 hardest implementation challenges and GUI modification rules. |
| [`HARDCODED_VALUES_AUDIT.md`](file:///C:/Github/PeerCastStation_Rust/docs/HARDCODED_VALUES_AUDIT.md) | Comprehensive inventory of 28 hardcoded literals, magic numbers, and static configurations identified across the codebase. |
