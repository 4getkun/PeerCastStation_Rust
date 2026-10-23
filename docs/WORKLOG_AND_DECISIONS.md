# PeerCastStation Rust Port: Master Work Log & Architectural Decision Records

> **Document Version**: 2.8.0 (Execution Phase: Tasks P01-P34 Completed)  
> **Status**: IN PROGRESS (Tasks P01..P34 PASSED; Next: P35 — Implement bounded directory parsing)  
> **Repository Root**: `C:\Github\PeerCastStation_Rust`  
> **Current Working Contract**: `docs/SPECIFICATION.md` (v1.4.0-review) & `PeerCastStation-Rust-spec-reviewed/IMPLEMENTATION_PLAN.md`  

---

## 1. Task Execution & Evidence Log

### Task P01: Inventory Actual Workspace
- **Prerequisites**: None
- **Requirements Covered**: REQ-001, REQ-002, REQ-003
- **Scope**: Read-only repository and environment verification.
- **Observed Environment**:
  - Git Branch: `main`
  - Baseline Commit: `387c38b`
  - Remote: `origin https://github.com/4getkun/PeerCastStation_Rust.git (fetch & push)`
  - OS: Windows (pwsh 7+)
  - Rust Compiler: `rustc 1.98.1 (48a229cea 2026-09-01)`
  - Cargo: `cargo 1.98.1 (797e8a9bc 2026-08-05)`
  - Python: `Python 3.14.0a2` (used for `verify_review.py`)
  - Target Edition: Rust Edition 2021 (strictly enforced as per REQ-002)
  - Review Package Check: `python -X utf8 PeerCastStation-Rust-spec-reviewed/verify_review.py` passed with **353 checks passed, 58 requirement IDs, 36 pinned source entries, 50 task contracts**.
- **P01 Outcome**: **PASSED**. Workspace baseline established with zero pre-existing code claims.

---

### Task P02: Map Existing Code to Requirements
- **Prerequisites**: P01 passed
- **Requirements Covered**: REQ-001, REQ-004, REQ-070, REQ-072
- **Scope**: Traceability inventory for all 58 requirements (REQ-001 to REQ-087).
- **Mapping Status**:
  - All 58 requirements classified with identified crate/file owners within Cargo Workspace DAG:
    - `crates/peercast-core`: REQ-010, REQ-020..REQ-028, REQ-040, REQ-041.
    - `crates/peercast-pcp`: REQ-030..REQ-038.
    - `crates/peercast-format`: REQ-050..REQ-056.
    - `crates/peercast-http`: REQ-060, REQ-061.
    - `crates/peercast-yp`: REQ-062, REQ-063.
    - `crates/peercast-servent`: REQ-011..REQ-014, REQ-042..REQ-044, REQ-065.
    - `crates/peercast-rpc`: REQ-064.
    - `crates/peercast-web`: REQ-066, REQ-080..REQ-087.
    - `crates/peercast-app`: REQ-067.
- **P02 Outcome**: **PASSED**. Traceability established.

---

### Task P03: Freeze First-Slice Contracts
- **Prerequisites**: P02 passed
- **Requirements Covered**: REQ-020, REQ-022, REQ-026, REQ-031, REQ-038, REQ-072
- **Decisions & Scope for First Slice**:
  - **Atom Wire Layout**: 4-byte ID4 followed by 4-byte Little-Endian u32. MSB bit 31 set indicates Parent (direct child count in lower 31 bits). MSB 0 indicates Leaf (byte length in lower 31 bits).
  - **Resource Guards**:
    - `MAX_CHILDREN = 1024`
    - `MAX_LEAF_LEN = 1048576` (1 MiB generic leaf payload guard)
    - `MAX_DATALEN = 16384` (16 KB media chunk data limit)
    - `MAX_DEPTH = 32`
    - `MAX_TOTAL_NODES = 4096`
    - `MAX_TOTAL_BYTES = 4194304` (4 MiB tree budget)
  - **NUL-terminated Strings**: Included in Atom leaf length.
  - **ID4 Representation**: Raw `[u8; 4]`, preserving exact byte contents without lossy trimming.
- **P03 Outcome**: **PASSED**. Contracts frozen.

---

### Task P04: Establish Minimal Build and Dependency DAG
- **Prerequisites**: P03 passed
- **Requirements Covered**: REQ-002, REQ-010
- **Scope**: Root `Cargo.toml` and minimal `crates/peercast-core` crate scaffolding.
- **Implementation**:
  - Root workspace `Cargo.toml` configured with Edition 2021, resolver "2", and pinned workspace dependencies (`bytes`, `thiserror`, `sha2`, `md-5`).
  - `crates/peercast-core/Cargo.toml` configured with workspace inheritance.
  - Direction verified: `peercast-core` is pure data models and codecs with zero network or Tokio I/O dependencies. No dependency cycles.
- **P04 Outcome**: **PASSED**. Workspace compiles cleanly under `cargo check`.

---

### Task P05: Implement Raw ID and Scalar Types
- **Prerequisites**: P04 passed
- **Requirements Covered**: REQ-020, REQ-021, REQ-022
- **Scope**: `crates/peercast-core/src/id4.rs`, `crates/peercast-core/src/scalar.rs`, and positive/negative test suites.
- **Implementation Details**:
  - `ID4`: Encapsulates `[u8; 4]`. Wire constants defined (`PCP_CONNECT = b"pcp\n"`, `PCP_QUIT = b"quit"`, `PCP_OK = b"ok\0\0"`, `HELO_SESSIONID = b"sid\0"`, etc.). `from_str_padded` pads with `\0` up to 4 bytes; strings > 4 bytes return `Id4Error::TooLong`.
  - `Scalar`: Explicit little-endian readers and writers (`read_u8`, `write_u8`, `read_u16_le`, `write_u16_le`, `read_u32_le`, `write_u32_le`, `read_u64_le`, `write_u64_le`). Exact width validation enforced; out-of-bounds reads rejected with `ScalarError::WidthMismatch`.
  - `read_nul_terminated_utf8` / `write_nul_terminated_utf8`: Mandates terminating NUL byte. Missing NUL returns `ScalarError::MissingTerminatingNul`. Validates UTF-8.
  - `read_ip_reversed` / `write_ip_reversed`: Encodes IPv4 (4 bytes) and IPv6 (16 bytes) in complete byte reversal order to match C# PeerCastStation wire compatibility. Non-4/16 lengths return `ScalarError::InvalidIpLength`.
- **Test Evidence**: `tests/test_p05_scalar.rs` (4 tests: `test_id4_pcp_newline_and_nul_padding`, `test_scalar_fixed_width_le`, `test_scalar_ip_reversal`, `test_scalar_nul_terminated_string`). All passed.
- **P05 Outcome**: **PASSED**.

---

### Task P06: Implement Atom Encoder
- **Prerequisites**: P05 passed
- **Requirements Covered**: REQ-020, REQ-021, REQ-022
- **Scope**: Generic `Atom` serialization in `crates/peercast-core/src/atom.rs`.
- **Implementation Details**:
  - `Atom` enum defined with `Leaf { name: ID4, value: Bytes }` and `Parent { name: ID4, children: Vec<Atom> }`.
  - `encode(&self, buf: &mut impl BufMut)`:
    - Leaf: writes 4-byte `name.0`, 4-byte little-endian `value.len() as u32` (bit 31 clear), followed by `value`.
    - Parent: writes 4-byte `name.0`, 4-byte little-endian `0x8000_0000 | (children.len() as u32)` (bit 31 set), followed by recursively encoding children in order.
    - Preserves child order and duplicate IDs identically.
    - Checks 31-bit descriptor limit (`0x7FFF_FFFF`); returns `AtomError::DescriptorOverflow` on overflow.
    - Generic encoder does not artificially cap generic leaves to 16 KiB (allows large leaves up to 1 MiB or custom limit).
- **Test Evidence**: `tests/test_p06_encoder.rs` tested against independent expected bytes from `reference_vectors.json`:
  - Raw PCP v1 (`7063700a0400000001000000`)
  - Raw PCP v100 (`7063700a0400000064000000`)
  - Normal quit (`7175697404000000e8030000`)
  - Read-error quit 4000 (`7175697404000000a00f0000`)
  - Empty parent (`68656c6f00000080`)
  - Empty leaf (`68656c6f00000000`)
  - One-child parent header (`68656c6f01000080` + child)
  - Nul-padded field ID (`736964001000000000112233445566778899aabbccddeeff`)
  - UTF-8 NUL leaf (`6e616d6504000000e3818200`)
  - 128 KiB leaf encoding without packet-data cap.
- **P06 Outcome**: **PASSED**.

---

### Task P07: Implement Incremental Atom Decoder
- **Prerequisites**: P06 passed
- **Requirements Covered**: REQ-025
- **Scope**: Incremental decoder and fragmentation engine in `crates/peercast-core/src/atom.rs`.
- **Implementation Details**:
  - `Atom::decode(buf: &mut Bytes, budget: &AtomBudget) -> Result<Option<Atom>, AtomError>`
  - Incremental safety guarantee:
    - Retains unconsumed bytes between partial network reads.
    - If input has fewer bytes than needed for a complete tree, returns `Ok(None)` without mutating `buf` or allocating child vectors/payload slices.
    - When a tree is complete, splits exact bytes from `buf` and parses into `Atom`, leaving subsequent concatenated Atoms intact in `buf`.
  - Distinguishes clean EOF (`Atom::decode_eof` with empty buffer returns `Ok(None)`) from truncated input (returns `Err(AtomError::Truncated)`).
- **Test Evidence**: `tests/test_p07_incremental_decoder.rs`:
  - `test_decoder_fragmentation_every_split_position`: Slices a multi-level nested tree at **every single byte boundary** (0..=N) and asserts that incomplete slices return `Ok(None)` without corruption, and complete slices decode identically to the original tree.
  - `test_decoder_concatenated_atoms_streaming`: Concatenates three distinct Atoms into a single buffer and decodes them sequentially.
  - `test_decoder_eof_clean_versus_truncated`: Validates clean EOF vs truncated header vs truncated payload.
  - `test_decoder_preserves_order_and_duplicates`: Confirms duplicate child IDs and ordering survive decode.
- **P07 Outcome**: **PASSED**.

---

### Task P08: Enforce Layered Parser Budgets
- **Prerequisites**: P07 passed
- **Requirements Covered**: REQ-025, REQ-026, REQ-027
- **Scope**: Codec resource safety gate in `crates/peercast-core/src/atom.rs`.
- **Implementation Details**:
  - `AtomBudget` enforces independent limits:
    - `max_children`: 1,024 default.
    - `max_leaf_len`: 1 MiB default (16 KiB in `for_media_packet()`).
    - `max_depth`: 32 default.
    - `max_total_nodes`: 4,096 default.
    - `max_total_bytes`: 4 MiB default.
  - Pre-allocation measurement (`measure_tree`):
    - Traverses wire headers and validates all limits before allocating any `Vec` or payload `Bytes`.
    - If peer advertises `len > max_leaf_len`, rejects immediately with `AtomError::LeafLengthExceeded` without waiting for or allocating the invalid length.
    - Tracks cumulative `nodes_count` and `bytes_count` across the entire tree so nesting and small leaves cannot bypass aggregate budgets.
- **Test Evidence**: `tests/test_p08_layered_budgets.rs`:
  - `test_budget_max_leaf_len_boundary`: Tests limit-1, limit, limit+1, and fast rejection without payload bytes.
  - `test_budget_max_children_boundary`: Tests limit (5) vs limit+1 (6).
  - `test_budget_max_depth_boundary`: Tests nested chains up to depth 3 vs depth 4.
  - `test_budget_max_total_nodes`: Tests 10 nodes vs 11 nodes.
  - `test_budget_max_total_bytes`: Tests 48 bytes vs 58 bytes with cumulative charging.
- **P08 Outcome**: **PASSED**.

---

### Task P09: Implement Protocol Identifiers and IP Conversions
- **Prerequisites**: P05 passed
- **Requirements Covered**: REQ-023, REQ-024
- **Scope**: Canonical hex ID vs .NET Guid layout and byte-reversed IP conversions.
- **Implementation Details**:
  - `ChannelId`: Stores 16 raw canonical big-endian hex bytes (`0123456789abcdef...` maps to `bytes[0..16]`).
  - `.NET Guid` layout translation:
    - `to_dotnet_guid_bytes()` reverses bytes `0..4`, `4..6`, and `6..8`; preserves bytes `8..16`.
    - `from_dotnet_guid_bytes()` restores canonical representation.
  - Address reversal: IPv4 reversed 4 bytes; IPv6 reversed 16 bytes.
- **Test Evidence**: `tests/test_p09_p10_channel_id.rs` (`test_channel_id_dotnet_guid_layout_conversion`) and `tests/test_p05_scalar.rs` (`test_scalar_ip_reversal`).
- **P09 Outcome**: **PASSED**.

---

### Task P10: Implement Station Broadcast ID Profile
- **Prerequisites**: P09 passed
- **Requirements Covered**: REQ-040, REQ-041
- **Scope**: Station identity derivation in `crates/peercast-core/src/channel_id.rs`.
- **Implementation Details**:
  - `derive_broadcast_channel_id`:
    1. Computes SHA-512 over the .NET Guid byte layout of `broadcast_id`.
    2. Prefixes little-endian i32 `1` if network is IPv6 (`ChannelNetworkType::IPv6`); prefixes nothing if IPv4.
    3. Concatenates 64-byte SHA-512 digest.
    4. Encodes name, genre, and source using C# `BinaryWriter` string format (7-bit encoded UTF-8 byte count, followed by raw UTF-8 bytes without NUL terminator).
    5. Computes MD5 digest over the concatenated buffer and interprets as .NET `Guid(byte[])`, converting back to canonical `ChannelId`.
- **Test Evidence**: `tests/test_p09_p10_channel_id.rs`:
  - Tested against all 6 reference test vectors in `reference_vectors.json`:
    - `0-empty` -> `8852c02562a3d36cd274ea3c5ae23d7d`
    - `0-japanese` -> `697d1c4d1ab44fdc9269d4cdcfdc5967`
    - `0-length_boundary` -> `1ee8bb3017038c02e7522508e831ac09`
    - `1-empty` -> `010dbe652e323b6360297cdd22ea5453`
    - `1-japanese` -> `ec5efd997acb9315a186f7d4d293a033`
    - `1-length_boundary` -> `767d964dcbe64ad0a1ab6930f8a5ddb4`
  - All 6 reference vectors passed with 100% exact hex match.
- **P10 Outcome**: **PASSED**.

---

### Task P11: Implement Raw Quit-Code Preservation
- **Prerequisites**: P05 passed
- **Requirements Covered**: REQ-038
- **Scope**: Core errors and protocol mapping table in `crates/peercast-core/src/errors.rs`.
- **Implementation Details**:
  - `QuitReason(pub u32)` wraps raw wire integer.
  - Standard base `1000` mapped (0..11: normal, skip, already connected, unavailable, loopback, not identified, bad response, bad agent, off air, shutdown, no root, banned).
  - Extended error bases mapped (`BCST=2000`, `READ=3000`, `WRITE=4000`, `GENERAL=5000`).
  - Unknown codes preserve exact integer value and do not panic or silently collapse to normal shutdown.
- **Test Evidence**: `tests/test_p11_quit_codes.rs` (tested 1000, 1001, 1003, 1008, 1009, 3000, 4000, 4042, 9999).
- **P11 Outcome**: **PASSED**.

---

### Code Review & Hardening Gate (P04..P11)
- **Scope**: Comprehensive audit of all files in `crates/peercast-core/` and tests against upstream PeerCastStation (C#), peercast-yt (C++), and Global Engineering Guidelines.
- **Review Findings & Applied Refinements**:
  1. **Exact Deficit Accounting in Truncated Decoding (`atom.rs`)**:
     - Upgraded `measure_tree` and `measure_node` to return `MeasureResult { Complete(usize), Incomplete { needed: usize } }`.
     - `decode_eof` now reports the exact byte target required for partial streams (e.g. `needed: 8, available: 5` or `needed: 12, available: 10`) rather than a hardcoded estimate.
     - Added `debug_assert!(cursor.is_empty())` to `parse_tree` to guarantee 100% byte consumption verification.
  2. **Stream Epoch Invalidation & Stale Rejection (`ringbuffer.rs`)**:
     - When an incoming chunk has `stream_id > current`, the buffer immediately purges prior epoch packets.
     - If an out-of-order chunk arrives with `stream_id < current`, it is discarded immediately to prevent dead-stream contamination.
  3. **Host Broadcast ID Parity (`host.rs`)**:
     - Added `pub broadcast_id: Option<ChannelId>` and `with_broadcast_id` builder to match upstream Station `Host.cs` (`Guid BroadcastID`).
  4. **Idiomatic Rust & Clippy Cleanliness**:
     - Elided unnecessary explicit lifetime in `find_children`.
     - Replaced indexed slice access in `ChannelId::from_hex` with `bytes.iter_mut().enumerate()`.
     - Removed redundant reference in `hasher.update(dotnet_bcid)`.
     - Ran `cargo clippy --all-targets`: **0 warnings**.
     - Ran `cargo test`: **21 tests passed (0 failures, 0 warnings)**.
- **Outcome**: **PASSED**. Codebase hardened and verified. Ready for P12..P16.

---

### Task P12: Actor Handles and Lifecycle Ownership
- **Prerequisites**: P04 passed
- **Requirements Covered**: REQ-011, REQ-012, REQ-014
- **Scope**: Actor handle/worker separation, explicit lifecycle phase state machine, generation token, bounded backpressure queue.
- **Implementation Details** (`crates/peercast-core/src/actor.rs`):
  - `WorkerHandle<C>`: Cloneable handle posting bounded commands (`try_post`, `post`) without locking across network awaits.
  - `WorkerContext<C>`: Exclusive receiver owner running the actor event loop (`recv`).
  - `WorkerPhase`: Enum (`Initial`, `Running`, `Stopping`, `Stopped`) enforcing single-enum lifecycle discipline (Global Rule 2).
  - `GenerationToken`: Monotonic identifier filtering stale events from superseded connections/channels.
  - Bounded queue backpressure: Capacity bounded queue returning `ActorError::QueueFull`.
  - Idempotent stop: `handle.stop()` is safe to call repeatedly without error or re-entrancy deadlock.
- **Test Evidence**: `crates/peercast-core/tests/test_p12_actor_lifecycle.rs` (5 tests passing).
- **P12 Outcome**: **PASSED**.

---

### Task P13: Content Epochs and Bounded Cache
- **Prerequisites**: P08 passed
- **Requirements Covered**: REQ-027, REQ-050, REQ-051
- **Scope**: Simultaneous time and byte cache eviction, generation epochs, separate header snapshot, position continuity detection.
- **Implementation Details** (`crates/peercast-core/src/cache.rs`):
  - `BoundedContentCache<C>`: Evaluates byte budget (default 8 MiB) AND time window (default 3s) simultaneously.
  - `HeaderSnapshot`: Retained independently from body data, tracking stream epoch, position, and container format.
  - Stale epoch invalidation: Pushing a new header immediately clears older epoch body data; pushing a body chunk with mismatched epoch is rejected (`CacheError::StaleEpoch`).
  - Retained allocation accounting: Tracks actual allocated capacity of shared `Bytes` rather than merely slice length.
  - `PositionContinuity`: Detects contiguous stream progress, gaps, and overlaps.
  - `FakeClock`: Abstract clock trait enabling deterministic time manipulation in unit tests.
- **Test Evidence**: `crates/peercast-core/tests/test_p13_content_cache.rs` (6 tests passing).
- **P13 Outcome**: **PASSED**.

---

### Task P14: Subscriber Admission and Slow-Peer Isolation
- **Prerequisites**: P13 passed
- **Requirements Covered**: REQ-013, REQ-028
- **Scope**: Nonblocking fan-out dispatch, per-subscriber admission, lag detection, slow-peer isolation with wire skip reason.
- **Implementation Details** (`crates/peercast-core/src/subscriber.rs`):
  - `SubscriberSink` & `SubscriberSource`: Created via bounded channel pair.
  - `SubscriberRegistry`: O(N) fan-out dispatch using nonblocking `try_send`.
  - Lag limit: Compares newest vs oldest packet timestamps (5s limit, A20). If exceeded or queue full, the stalled subscriber is evicted with `QuitReason::SKIP` (1001).
  - Fault isolation: A stalled or hung subscriber never blocks or degrades healthy subscribers or the channel broadcaster.
- **Test Evidence**: `crates/peercast-core/tests/test_p14_subscriber_admission.rs` (2 tests passing).
- **P14 Outcome**: **PASSED**.

---

### Task P15: Bounded HTTP Relay Header Parser
- **Prerequisites**: P08, P11, P12 passed
- **Requirements Covered**: REQ-030, REQ-031, REQ-037
- **Scope**: Bounded HTTP request and response prelude parsing, CRLF fragmentation, binary tail preservation.
- **Implementation Details** (`crates/peercast-pcp/src/http_prelude.rs`):
  - `PcpHttpLimits`: Strict bounds on max header bytes (4 KiB), line count (32), and line length (1 KiB).
  - `parse_relay_request`: Parses `GET /channel/{id} HTTP/1.0`, validating 32-hex ChannelId, `x-peercast-pcp`, and optional `x-peercast-pos`.
  - `parse_relay_response`: Parses status line (200, 503, 404, etc.) and preserves binary tail directly for Atom decoding.
  - `404 Not Found` returns `PcpHttpError::NotFoundOffAir` and is strictly forbidden from entering Atom decoding (REQ-032).
- **Test Evidence**: `crates/peercast-pcp/tests/test_p15_http_prelude.rs` (6 tests passing).
- **P15 Outcome**: **PASSED**.

---

### Task P16: Relay Consumer 200 Path
- **Prerequisites**: P15, P09 passed
- **Requirements Covered**: REQ-031, REQ-032, REQ-034
- **Scope**: Client state machine for HTTP 200 OK relay streaming bootstrap.
- **Implementation Details** (`crates/peercast-pcp/src/relay_client.rs`):
  - `RelayClientStateMachine`: Drives TCP -> GET -> 200 OK -> `helo/oleh` exchange -> `ok(1)` -> `Streaming`.
  - Protocol profile: Version 1 for IPv4, 100 for IPv6 (REQ-034).
  - Interleaved atoms: Accepts valid control packets (`host`, `bcst`, `info`, `trck`) interleaved before and during media streaming.
  - Rejects invalid `ok` values (must be 1).
  - Never emits raw `pcnt` or redundant `pcp\n` binary preambles in HTTP GET request.
- **Test Evidence**: `crates/peercast-pcp/tests/test_p16_relay_client_handshake.rs` (5 tests passing).
- **P16 Outcome**: **PASSED**.

---

### Task P17: Relay Consumer 503 and 404 Paths
- **Prerequisites**: P16 passed
- **Requirements Covered**: REQ-032, REQ-033, REQ-038
- **Scope**: Client alternate status branches, candidate host collection, unknown quit code preservation.
- **Implementation Details** (`crates/peercast-pcp/src/relay_client.rs`, `crates/peercast-core/src/host.rs`):
  - Full-relay 503 path: Exchanges `helo/oleh` before collecting candidate hosts and receiving `quit(1003)` (REQ-032, REQ-033).
  - Single-enum state machine: Explicit phases (`WaitingOleh503`, `CollectingHosts503`) eliminating boolean flags (Global Rule 2).
  - Candidate host validation: `Host::is_valid_candidate` rejects port 0, unspecified IPs (`0.0.0.0`, `::`), and self-session IDs.
  - Bounded candidate storage: Capped at `MAX_CANDIDATE_HOSTS = 64` (Global Rule 3).
  - Preserves exact raw quit codes (e.g. 4000, 9999) without truncation (REQ-038).
  - 404 off-air stops immediately without Atom parsing.
- **Test Evidence**: `crates/peercast-pcp/tests/test_p17_relay_client_503_and_404.rs` (6 tests passing).
- **P17 Outcome**: **PASSED**.

---

### Task P18: Relay Server Handshake Branches
- **Prerequisites**: P17, P14 passed
- **Requirements Covered**: REQ-032, REQ-033, REQ-034
- **Scope**: Server-side request routing (200, 503, 404), helo validation, oleh + ok1 emission, candidate host distribution.
- **Implementation Details** (`crates/peercast-pcp/src/relay_server.rs`):
  - `RelayServerStateMachine`: State machine processing incoming client relay requests.
  - 200 path: Emits HTTP 200 OK prelude, validates client `helo`, emits `oleh` followed by Station-compatible `ok(1)` Atom, enters `Streaming`.
  - 503 path: Emits HTTP 503 Service Unavailable, validates `helo`, emits `oleh`, candidate `host` atoms, and terminating `quit(1003)`.
  - 404 path: Emits HTTP 404 Not Found, terminates immediately without Atom parsing.
  - Resource safety: Client failure or early quit releases reserved slots cleanly.
  - Independent testing: Driven entirely with raw scripted wire bytes to avoid shared-bug agreement.
- **Test Evidence**: `crates/peercast-pcp/tests/test_p18_relay_server_handshake.rs` (6 tests passing).
- **P18 Outcome**: **PASSED**.

---

### Task P19: Reverse Probe and Raw Preamble
- **Prerequisites**: P18 passed
- **Requirements Covered**: REQ-035, REQ-036, REQ-037
- **Scope**: Reachability probe worker, raw preamble codec, concurrency limiter, SSRF prevention.
- **Implementation Details** (`crates/peercast-pcp/src/probe.rs`):
  - Wire preamble: Emits `pcp\n` Atom with protocol version 1, followed by `helo` Atom (REQ-035, A09).
  - Validation: Reads `oleh` response and verifies returned SessionID matches expected peer SessionID.
  - Closes cleanly with `quit(1000)` (QuitReason::NORMAL).
  - Security guard: Probing is strictly restricted to observed remote peer IP (from TCP socket); attempts to probe arbitrary target IPs are rejected with `ProbeError::ForbiddenTarget`.
  - Concurrency limits: `ProbeLimiter` bounds global concurrency (16 max) and per-peer concurrency (1 max per IP) with RAII `ProbeGuard`.
  - Explicit deadlines: Connect deadline (5s) and exchange deadline (10s) (REQ-037).
  - IPv6 probe marked pending per REQ-036.
- **Test Evidence**: `crates/peercast-pcp/tests/test_p19_reverse_probe.rs` (7 tests passing).
- **P19 Outcome**: **PASSED**.

---

### Task P20: Connect Channel Packets to Cache and Relay
- **Prerequisites**: P19, P13 passed
- **Requirements Covered**: REQ-026, REQ-050, REQ-051
- **Scope**: PCP channel packet serialization/deserialization, safe body splitting under 16 KiB, rollover unwrapping, cache adapter.
- **Implementation Details** (`crates/peercast-pcp/src/packet.rs`):
  - `MAX_PACKET_DATALEN = 16384` (16 KiB hard cap, A07, REQ-026).
  - Safe outbound splitting: Body payloads exceeding 16 KiB are cleanly split into sub-packets <= 16,384 bytes, advancing position and setting continuation flags (`cont = 1`) without truncation or corruption.
  - Inbound packet cap: Incoming packets exceeding 16 KiB payload are rejected with `PacketError::PacketDataTooLarge`.
  - Oversized header strategy: Headers exceeding `max_header_bytes` (128 KiB) are rejected with `PacketError::HeaderTooLarge`, preventing silent corruption.
  - 32-bit position rollover unwrapping: `unwrap_wire_pos` seamlessly maps wire `u32` positions across `0xFFFFFFFF -> 0x00000000` into monotonic 64-bit signed cursors.
  - Extension preservation: Unknown packet extensions are retained intact across relay transport (REQ-021).
  - Cache integration: `ChannelPacketAdapter` feeds headers (advancing epochs and invalidating stale body data) and body packets to `BoundedContentCache`.
- **Test Evidence**: `crates/peercast-pcp/tests/test_p20_channel_packet_relay.rs` (6 tests passing).
- **P20 Outcome**: **PASSED**.

---

### Cross-Audit & C# Upstream Parity Review (2026-10-24)
- **Objective**: Conduct comprehensive line-by-line review of Rust implementation cross-referenced directly against upstream C# PeerCastStation (`C:\Github\_upstream_peercaststation`, commit `390c4e84`) and PeerCast-YT (`C:\Github\_upstream_peercast-yt`, commit `b60f1763`).
- **Audit Findings & Architectural Resolutions**:
  1. **Helo / Oleh Version vs Transport Version Discrepancy**:
     - *Upstream Finding*: In `PCPVersion.cs:9,13,14` and `version2.h:23`, `ProtocolVersionIPv4 = 1` is strictly used for HTTP `x-peercast-pcp: 1` and raw stream preamble `pcp\n: 1`. Inside the `helo` and `oleh` Atom `ver\0` child, upstream emits `ServantVersion = 1218`. In `PCPOutputStream.cs:503-506`, Station explicitly asserts: `if ((peer.Host.Extra.GetHeloVersion() ?? 0) < 1200) throw new HandshakeErrorException(StopReason.BadAgentError);`. Emitting `1` inside `helo` would trigger immediate `quit(1007)` (`BADAGENT`) rejection when connecting to real Station nodes.
     - *Resolution*: Defined `pub const SERVANT_VERSION: u32 = 1218;` and `pub const MIN_HELO_VERSION: u32 = 1200;`. Emitted `SERVANT_VERSION` in `build_helo` (`relay_client.rs`), `build_oleh` (`relay_server.rs`), and `build_probe_init` (`probe.rs`). Enforced `client_version >= MIN_HELO_VERSION` in `relay_server.rs`, returning `RelayServerError::BadAgentVersion` and transitioning to `Terminal(QuitReason::BAD_AGENT)` when `< 1200`.
  2. **Channel Packet Tag Structure Parity**:
     - *Upstream Finding*: In both Station (`PCPOutputStream.cs:780-784`, `PCPSourceStream.cs:608-634`) and YT (`servent.cpp:2004-2007`, `pcp.cpp:300-320`), stream payload is ALWAYS carried in leaf child `data` (`ID4::PKT_DATA = b"data"`), regardless of whether it represents header or body. Header vs body is distinguished exclusively by the `type` leaf (`ID4::PKT_TYPE`), containing `PKT_TYPE_HEAD` (`b"head"`) or `PKT_TYPE_DATA` (`b"data"`). Our previous Rust encoder emitted child leaf `head`, which would cause real Station/YT nodes to fail `GetChanPktData()` and drop all headers.
     - *Resolution*: Added `PKT_TYPE_HEAD`, `PKT_TYPE_DATA`, and `PKT_TYPE_META` to `crates/peercast-core/src/id4.rs`. Updated `ChannelPacket::from_atom()` to inspect `stream_type == PKT_TYPE_HEAD` (with backward fallback to `head_data`). Updated `ChannelPacket::to_atoms()` to always emit payload in `PKT_DATA` with `stream_type` set appropriately.
  3. **Outbound Body Splitting Threshold**:
     - *Upstream Finding*: Station defines `MaxBodyLength = 15*1024` (15,360 bytes, `PCPOutputStream.cs:727`) to maintain a 1 KiB margin of safety below YT/Station's hard receiver cap of 16,384 bytes (`MAX_DATALEN = 16384` in `pcp.cpp:318`).
     - *Resolution*: Defined `pub const SPLIT_BODY_DATALEN: usize = 15 * 1024;` in `packet.rs` for outbound chunk splitting, while retaining `MAX_PACKET_DATALEN = 16384` for inbound boundary rejection.
  4. **HTTP 200 Content-Type Parity**:
     - *Upstream Finding*: Station `PCPOutputStream.cs:825` serves `Content-Type: application/x-peercast-pcp` on relay accept.
     - *Resolution*: Changed `RelayServerStateMachine::accept()` from `application/octet-stream` to `application/x-peercast-pcp`.
  5. **HostFlags1 Firewalled Flag Parity**:
     - *Upstream Finding*: `Atom.cs:149, 269` and `PCPSourceStream.cs:701` define `0x08` as `Firewalled` (alias for `Push`).
     - *Resolution*: Added `pub const FIREWALLED: HostFlags1 = HostFlags1(0x08);` and `pub fn is_firewalled(&self) -> bool` to `crates/peercast-core/src/host.rs`.
- **Verification Evidence**:
  - `cargo test --all`: 71/71 tests passing across 16 test suites.
  - `cargo clippy --all-targets`: 0 warnings.
  - Added `test_p18_server_rejects_old_servant_version` and `test_p20_station_format_header_packet_parity`.

---

## 2. Test Execution Summary

Command: `cargo test --manifest-path C:\Github\PeerCastStation_Rust\Cargo.toml --all`
Execution Date: 2026-10-24
Compiler: `rustc 1.98.1 (48a229cea 2026-09-01)`
Clippy: `cargo clippy --manifest-path C:\Github\PeerCastStation_Rust\Cargo.toml --all-targets` -> **0 warnings**

| Test Suite | Location | Tests Passed | Tests Failed | Status |
|---|---|---:|---:|---|
| `test_p05_scalar.rs` | `crates/peercast-core` | 4 | 0 | PASSED |
| `test_p06_encoder.rs` | `crates/peercast-core` | 3 | 0 | PASSED |
| `test_p07_incremental_decoder.rs` | `crates/peercast-core` | 4 | 0 | PASSED |
| `test_p08_layered_budgets.rs` | `crates/peercast-core` | 5 | 0 | PASSED |
| `test_p09_p10_channel_id.rs` | `crates/peercast-core` | 2 | 0 | PASSED |
| `test_p11_quit_codes.rs` | `crates/peercast-core` | 1 | 0 | PASSED |
| `test_content_ringbuffer.rs` | `crates/peercast-core` | 2 | 0 | PASSED |
| `test_p12_actor_lifecycle.rs` | `crates/peercast-core` | 5 | 0 | PASSED |
| `test_p13_content_cache.rs` | `crates/peercast-core` | 6 | 0 | PASSED |
| `test_p14_subscriber_admission.rs` | `crates/peercast-core` | 2 | 0 | PASSED |
| `test_p15_http_prelude.rs` | `crates/peercast-pcp` | 6 | 0 | PASSED |
| `test_p16_relay_client_handshake.rs` | `crates/peercast-pcp` | 5 | 0 | PASSED |
| `test_p17_relay_client_503_and_404.rs` | `crates/peercast-pcp` | 6 | 0 | PASSED |
| `test_p18_relay_server_handshake.rs` | `crates/peercast-pcp` | 7 | 0 | PASSED |
| `test_p19_reverse_probe.rs` | `crates/peercast-pcp` | 7 | 0 | PASSED |
| `test_p20_channel_packet_relay.rs` | `crates/peercast-pcp` | 7 | 0 | PASSED |
| `test_p21_controlled_interop.rs` | `crates/peercast-pcp` | 5 | 0 | PASSED |
| `test_p22_container_detection.rs` | `crates/peercast-format` | 7 | 0 | PASSED |
| `test_p23_flv_parser.rs` | `crates/peercast-format` | 9 | 0 | PASSED |
| `test_p24_safestart.rs` | `crates/peercast-format` | 6 | 0 | PASSED |
| `test_p25_http_playback.rs` | `crates/peercast-http` | 6 | 0 | PASSED |
| `test_p26_http_ingest.rs` | `crates/peercast-http` | 5 | 0 | PASSED |
| `test_p27_ts_parser.rs` | `crates/peercast-format` | 5 | 0 | PASSED |
| `test_p28_ts_safestart.rs` | `crates/peercast-format` | 5 | 0 | PASSED |
| **Total** | | **119** | **0** | **ALL PASSED** |

---

### [2026-10-24] Task P21 Implementation & Interoperability Verification
- **Requirements Covered**: REQ-003, REQ-070, REQ-071
- **Files Added / Changed**:
  - `crates/peercast-pcp/tests/test_p21_controlled_interop.rs`
- **Decisions & Parity Observations**:
  - Implemented 5 loopback integration test vectors covering:
    1. HTTP/1.0 200 relay streaming with real TCP sockets and SHA-256 fixture verification.
    2. HTTP/1.0 503 candidate host distribution and graceful termination.
    3. HTTP/1.0 404 off-air immediate disconnection without atom decoding.
    4. Reverse probe handshake across loopback TCP sockets matching session ID.
    5. Reconnection with `x-peercast-pos` cursor resumes streaming without losing position continuity.

### [2026-10-24] Task P22 Implementation: Bounded Container Detection
- **Requirements Covered**: REQ-050, REQ-056
- **Files Added / Changed**:
  - `crates/peercast-format/Cargo.toml`
  - `crates/peercast-format/src/lib.rs`
  - `crates/peercast-format/src/sniffer.rs`
  - `crates/peercast-format/tests/test_p22_container_detection.rs`
- **Decisions & Parity Observations**:
  - Created new crate `crates/peercast-format` enforcing non-consuming replay via `SniffBuffer`.
  - Detection evaluates signatures for FLV (`FLV` + version 1 + PreviousTagSize0 == 0), MPEG-2 TS (multi-packet 188-byte `0x47` sync checks), Matroska/WebM (EBML ID `0x1A45DFA3`), ASF (GUID `75B22630-668E-11CF-A6D9-00AA0062CE6C`), Ogg (`OggS`), and MP3 (MPEG audio sync word `0xFFE0`).
  - File extension alone is never used to infer unverified parsers.

### [2026-10-24] Task P23 Implementation: First FLV Header and Body Path
- **Requirements Covered**: REQ-051, REQ-052
- **Files Added / Changed**:
  - `crates/peercast-format/src/flv.rs`
  - `crates/peercast-format/tests/test_p23_flv_parser.rs`
- **Decisions & Parity Observations**:
  - Implemented `FlvFileHeader`, `FlvTagHeader`, `FlvTag`, and incremental streaming `FlvStreamParser` resilient to single-byte chunk splits.
  - Mandatory 13-byte FLV prelude (`'F','L','V'`, version 1, flags, DataOffset 9, PreviousTagSize0 = 0) strictly enforced; emitting 9-byte header alone is blocked.
  - Implemented lightweight, bounded AMF0 metadata parser extracting `videodatarate`, `audiodatarate`, `maxBitrate`, `width`, `height`, and total bitrate in kbps.
  - Sequence headers for AAC (`AudioTagHeader` with `AACPacketType == 0`) and AVC (`VideoTagHeader` with `AVCPacketType == 0`) cached and emitted with timestamp 0 in `HeaderSnapshot`.
  - Config changes trigger epoch increment, invalidating stale cached body chunks (REQ-051).
  - Extended FLV tags (ExVideo, ExAudio) explicitly detected and rejected rather than silently treated as standard AVC/AAC.
  - Integration test validated against real upstream `test.flv` fixture from `PeerCastStation/TestFLV/fixtures/test.flv`.

### [2026-10-24] Task P24 Implementation: Safe-Start Selection
- **Requirements Covered**: REQ-050, REQ-055
- **Files Added / Changed**:
  - `crates/peercast-format/src/safestart.rs`
  - `crates/peercast-format/tests/test_p24_safestart.rs`
- **Decisions & Parity Observations**:
  - Separated packet continuation flags from format-level keyframes: `cont == 0` is strictly forbidden from being treated as universal proof of H.264 IDR (REQ-050).
  - Implemented `SafeStartSelector` with state machine: `WaitingForHeader -> SeekingSafePoint -> Streaming`.
  - Packet continuation fragments (`cont_flags.is_fragment()`) are never admitted as starting points.
  - In `VideoKeyframe` mode: skips interframes until true keyframe arrives; if bounded timeout (3s) elapses, fallback admits the next unfragmented packet to avoid stalling indefinite playback (REQ-055).
  - `AudioOnly` mode admits the first unfragmented packet immediately without waiting for video keyframes.
  - Historical cache start-point selection implemented via `select_cache_start_point` scanning `BoundedContentCache`.
  - Stream/epoch restarts reset safe-start selection cleanly.

### [2026-10-24] Task P25 Implementation: Direct Playback and Playlists
- **Requirements Covered**: REQ-060
- **Files Added / Changed**:
  - `crates/peercast-http/Cargo.toml`
  - `crates/peercast-http/src/lib.rs`
  - `crates/peercast-http/src/playlist.rs`
  - `crates/peercast-http/src/router.rs`
  - `crates/peercast-http/src/streamer.rs`
  - `crates/peercast-http/tests/test_p25_http_playback.rs`
- **Decisions & Parity Observations**:
  - Established `crates/peercast-http` crate implementing `/stream/{id}` and `/pls/{id}` routing.
  - Channel ID validated as exact 32-hex characters; invalid hex and short strings rejected with 404 / 400.
  - Framing parity strictly enforced:
    - HTTP/1.1 standard media: `Transfer-Encoding: chunked`.
    - HTTP/1.0: ALWAYS raw continuous streaming (`Connection: close`); chunked encoding is strictly prohibited (REQ-060 stop/block rule).
    - MMS / ASF: ALWAYS raw continuous streaming (`application/x-mms-framed`, `Connection: close`).
  - Playlist builder supports M3U (`audio/x-mpegurl`), PLS (`audio/x-scpls`), and ASX (`video/x-ms-asf`).
  - Negative tests verify playlist escaping: XML special characters in ASX escaped; newline injection (CR/LF) in M3U/PLS stripped to prevent playlist header injection attacks.
  - Real TCP loopback test streams chunked FLV to client, which de-chunks and decodes cleanly via `FlvStreamParser`.
  - Client disconnect terminates streaming loop promptly and releases subscriber resources.

### [2026-10-24] Task P26 Implementation: First Ingest Endpoint
- **Requirements Covered**: REQ-061, REQ-065
- **Files Added / Changed**:
  - `crates/peercast-http/src/ingest.rs`
  - `crates/peercast-http/src/lib.rs`
  - `crates/peercast-http/tests/test_p26_http_ingest.rs`
- **Decisions & Parity Observations**:
  - Parity contract adhered to Station's `HTTPPushSourceStream`: opens a dedicated listener at its configured source URI and accepts HTTP POST. A universal `/push/{id}` route is explicitly NOT invented as standard upstream behavior (REQ-061).
  - Implemented `HttpPushIngestListener` and incremental de-chunking stream decoder `read_chunked_stream` enforcing max header (8 KiB) and max chunk (16 MiB) budgets.
  - Authorized/unauthorized access control tested: bearer token header and query parameter (?auth=...) authenticated; missing or wrong token rejected with 401 Unauthorized.
  - Non-POST methods (GET, PUT) rejected with 405 Method Not Allowed.
  - Port binding conflict handling proven: conflicting bind surfaces `IngestError::BindError` without panicking.
  - Client cancellation and broken-pipe disconnects cleanly caught and resources freed.
  - End-to-end integration verified: simulated ffmpeg/OBS HTTP POST FLV stream feeds de-chunker, which feeds `FlvDemuxer`, successfully extracting `HeaderSnapshot` and media keyframes.

### [2026-10-24] Task P27 Implementation: Independent MPEG-2 TS Demuxer
- **Requirements Covered**: REQ-053, REQ-056
- **Files Added / Changed**:
  - `crates/peercast-format/src/ts.rs`
  - `crates/peercast-format/src/lib.rs`
  - `crates/peercast-format/tests/test_p27_ts_parser.rs`
- **Decisions & Parity Observations**:
  - Implemented standard 188-byte MPEG-2 TS framing parser completely independent from FLV logic.
  - Strict framing bounds enforced: 192-byte and 204-byte packets (e.g. BDAV or DVB-ASI with FEC) are rejected without explicit configuration, satisfying the stop/block criteria.
  - Implemented `PatTable` and `PmtTable` section parsers extracting program mappings, PCR PID, and elementary stream types.
  - Declared supported codec subset explicitly: H.264 video (`0x1B`), MPEG-2 video (`0x02`), AAC audio (`0x0F` ADTS / `0x11` LATM), and MP3 audio (`0x03` / `0x04`). Unsupported stream types are safely flagged rather than crashing.
  - Normalized continuity counter on PAT and PMT packets (Station parity `bytes[3] &= 0xF0`) to build deterministic 376-byte `HeaderSnapshot`.
  - Re-tested reconstructed `HeaderSnapshot` bytes with independent PAT/PMT parsers to confirm round-trip decodability.
  - Verified epoch invalidation and snapshot regeneration upon PMT PID and stream configuration changes (REQ-051, REQ-053).
  - Detected random access points via Random Access Indicator (RAI) in adaptation field on video PIDs.

### [2026-10-24] Task P28 Implementation: Safe-Start Selection for MPEG-2 TS
- **Requirements Covered**: REQ-050, REQ-055
- **Files Added / Changed**:
  - `crates/peercast-format/src/safestart.rs`
  - `crates/peercast-format/src/lib.rs`
  - `crates/peercast-format/tests/test_p28_ts_safestart.rs`
- **Decisions & Parity Observations**:
  - Parity and security stop/block guard strictly satisfied: PAT/PMT packets alone are NEVER treated as proof of immediate artifact-free video playback (REQ-053).
  - Video stream safe start requires Random Access Indicator (RAI) in adaptation field on video PID before admitting media packets. Interframes arriving after PAT/PMT are skipped until a true keyframe arrives.
  - Invariant enforced: media streaming to clients ALWAYS has a valid PAT/PMT `HeaderSnapshot` preceding the chosen media packet.
  - Bounded timeout fallback: if encoder does not emit RAI flags, timeout fallback admits unfragmented packets after 3s to prevent indefinite playback stall (REQ-055).
  - Audio-only TS streams admit the first unfragmented packet immediately without waiting for RAI or video keyframes.
  - PID and PMT changes trigger epoch increments, resetting the safe-start state machine and requiring fresh PAT/PMT emission.

### [2026-10-24] Task P28 Implementation: ASF and MMS/WMSP Framing Demuxer
- **Requirements Covered**: REQ-054, REQ-056 (and REQ-051, REQ-060)
- **Files Added / Changed**:
  - `crates/peercast-format/src/asf.rs`
  - `crates/peercast-format/src/lib.rs`
  - `crates/peercast-format/tests/test_p28_asf_mms_parser.rs`
- **Decisions & Parity Observations**:
  - Parity and security stop/block guard strictly satisfied: ASF parsing follows standard COM GUID and u64 object sizes with a 24-byte generic object prefix and 6 additional top-level header prelude bytes (REQ-054). Guessing an unverified "50-byte minimum" is strictly prohibited and tested.
  - Distinction between raw ASF and MMS-framed transport:
    - Raw ASF objects are parsed directly using `ASF_TOP_LEVEL_HEADER_GUID`.
    - MMS-over-HTTP transport utilizes Windows Media HTTP Streaming Protocol (WMSP) 4-byte frame headers (`$H`, `$D`, `$C`, `$E`, `$M`, `$P`) wrapping 8-byte MMS packet headers.
    - Station parity contract verified: downstream subscribers receiving MMS-framed streams are sent complete WMSP header frames (`$H`, 5,271 bytes in upstream fixture) and complete data frames (`$D`), not stripped raw ASF objects.
  - End-to-end integration verified against real upstream fixture `C:\Github\_upstream_peercaststation\PeerCastStation\TestASF\fixtures\test.asf` (~1.96 MB):
    - Generates 5,271-byte `HeaderSnapshot` matching Station's exact byte count.
    - Correctly extracts metadata: "WMV", `video/x-ms-wmv`, `.wmv`, and 438 kbps (from sum of stream bitrates 33,742 + 404,368 = 438,110 bps / 1000).
    - Accurately emits all 361 content packets with contiguous byte positions matching upstream Station test expectations.
  - Safe-start and audio-only classification:
    - Streams containing video properties are classified as "WMV" (`video/x-ms-wmv`).
    - Streams with audio only are classified as "WMA" (`audio/x-ms-wma`).
    - Generic streams default to "ASF" (`video/x-ms-asf`).
  - Header replacement and epoch tracking (REQ-051):
    - Successive `$H` frames bump content epoch and rebuild the active `HeaderSnapshot`.
  - Incremental streaming and robust framing:
    - Stream feeds chunked at arbitrary byte boundaries (1-byte, 3-byte slices) yield identical framing without data corruption or memory leaks (Global Rule 3).
    - Clean EOF and explicit End-of-Stream (`$E`) transitions transition demuxer into `Ended` state with early-return re-entrancy protection (Global Rule 2).

### [2026-10-24] Task P29 Implementation: Matroska (MKV) and WebM EBML Demuxer
- **Requirements Covered**: REQ-051, REQ-055, REQ-056
- **Files Added / Changed**:
  - `crates/peercast-format/src/mkv.rs`
  - `crates/peercast-format/src/lib.rs`
  - `crates/peercast-format/tests/test_p29_mkv_parser.rs`
- **Decisions & Parity Observations**:
  - Parity and security stop/block guard strictly satisfied: MKV parser contract established independently using EBML variable-size integer encoding (VINT, 1..=8 bytes). No cloning of FLV 9-byte header or tag assumptions (REQ-055).
  - VINT parser supports:
    - Standard length determination: `leading_zeros() + 1` (1..=8 bytes).
    - Unknown/undefined size marker recognition (all data bits 1, e.g. `0x01 0xFF 0xFF 0xFF 0xFF 0xFF 0xFF 0xFF` on Segment elements) for streaming inputs.
    - Zero first-byte rejection (`InvalidVintFirstByte`).
  - Container-level safe-start and keyframe detection (REQ-055):
    - Parity with PeerCast-YT `hasKeyFrame`: `SimpleBlock` (`0xA3`) examines bit 7 of the flags byte (`flags & 0x80 != 0`) to identify keyframes.
    - `BlockGroup` (`0xA0`) identifies keyframes via the absence of `ReferenceBlock` (`0xFB`) child elements.
  - Header snapshot generation and accumulation:
    - Pre-cluster elements (`EBML`, `Segment` header, `SeekHead`, `Info`, `Tracks`, `Tags`, `Void`, `CRC32`) are gathered into a comprehensive `HeaderSnapshot`.
    - When the first `Cluster` element (`0x1F 0x43 0xB6 0x75`) is reached, the accumulated snapshot is emitted with position 0, and subsequent blocks/clusters are streamed contiguously.
  - End-to-end integration verified against real upstream fixtures:
    - `test.mkv` (7.83 MB): parsed successfully, `HeaderSnapshot` established at pos 0, classified as "MKV" (`video/x-matroska`, `.mkv`), verified > 100 media blocks and multiple keyframes.
    - `test.webm` (1.80 MB): parsed successfully, classified as "WEBM" (`video/webm`, `.webm`).
  - Header replacement and epoch tracking (REQ-051):
    - Mid-stream arrival of a new EBML element transitions state machine, bumps `current_epoch`, clears stale header elements, and generates a new `HeaderSnapshotReady` upon next cluster.
  - Incremental streaming and budget protections (Global Rule 3):
    - 5-byte chunked streaming verified to produce identical demux events to monolithic feeds.
    - Oversized elements exceeding budget are safely rejected with `ElementSizeExceeded`.

### [2026-10-24] Task P30 Implementation: Ogg Framing, Codec Scoping, and MP3 Decision
- **Requirements Covered**: REQ-051, REQ-055, REQ-056
- **Files Added / Changed**:
  - `crates/peercast-format/src/ogg.rs`
  - `crates/peercast-format/src/mp3.rs`
  - `crates/peercast-format/src/lib.rs`
  - `crates/peercast-format/tests/test_p30_ogg_mp3_parser.rs`
- **Decisions & Parity Observations**:
  - Stop/block guard strictly satisfied: Supporting the Ogg container does NOT mean claiming support for every codec it can hold (REQ-056). Explicitly scoped supported codecs: Vorbis (`audio/ogg`), Opus (`audio/ogg`), and Theora (`video/ogg`). Unknown codecs (e.g. Skeleton, FLAC, Speex, Daala) return `OggParseError::UnsupportedCodec`.
  - RFC 3533 page parser implementation:
    - Capture pattern `OggS` (`0x4F 0x67 0x67 0x53`), version 0, header flags (continuation, BOS, EOS), 64-bit granulepos, 32-bit serial number, 32-bit sequence number, and standard CRC-32 checksum verification (`0x04C11DB7`).
    - Lacing table reassembly with 255-byte segment spans and bounded packet assembly buffer (Global Rule 3).
  - Codec Header Extraction and Snapshot Generation:
    - Vorbis: gathers identification header (`\x01vorbis`), comment header (`\x03vorbis`), and setup header (`\x05vorbis`) before snapshot emission.
    - Opus: gathers identification header (`OpusHead`) and comment header (`OpusTags`) before snapshot emission.
    - Theora: gathers identification header (`\x80theora`), comment header (`\x81theora`), and setup header (`\x82theora`).
  - Audio-only safe start (REQ-055): Vorbis and Opus streams emit packets immediately after header initialization without waiting for video keyframes.
  - Chained Ogg stream support (REQ-051, RFC 3533):
    - When an EOS page is reached, logical bitstream ends; an immediate BOS with a new serial number transitions the demuxer, bumps `current_epoch`, clears stale header state, and creates a fresh `HeaderSnapshot`.
  - Explicit MP3 support decision:
    - MP3 audio is supported for direct playback (`audio/mpeg`, `.mp3`) and relay without transcoding. Includes MPEG audio frame header parser (`0xFFE` / `0xFFF` sync) and ID3v2 tag skipping.

### [2026-10-24] Task P31 Implementation: Host Freshness and Failure Cooldown
- **Requirements Covered**: REQ-042 (and REQ-043)
- **Files Added / Changed**:
  - `crates/peercast-core/src/host_registry.rs`
  - `crates/peercast-core/src/lib.rs`
  - `crates/peercast-core/tests/test_p31_host_freshness_cooldown.rs`
- **Decisions & Parity Observations**:
  - Stop/block guard strictly satisfied: Single 180-second timer does NOT control unrelated concepts. Station's 180-second node age check is in `Channel` (`NodeLimit = 180000 ms`); its 180-second ignored-source setting is in `PCPSourceStream` (`ignoredTime = 180000 ms`). These are completely separate monotonic timers and configurable thresholds in the Rust port.
  - Separation of Concerns and Invariant Preservation:
    - Host freshness in `HostRegistry`: tracks live peer nodes per channel, records monotonic `last_seen_offset`, calculates node age, and filters stale nodes exceeding `freshness_threshold`.
    - Failure cooldown in `EndpointCooldownTable`: tracks connection failures (timeout, refused, 404, 503) per channel or globally, and enforces `cooldown_duration`.
    - **Crucial Invariant Tested**: Receiving a gossip packet or freshening a host in `HostRegistry` NEVER resets or clears an active failure cooldown in `EndpointCooldownTable`. Conversely, recording a failure in `EndpointCooldownTable` NEVER alters host freshness or node age in `HostRegistry`.
  - Endpoint Identity & Session Deduplication:
    - `EndpointIdentity`: supports channel-scoped (`pcp://{addr}/channel/{cid}`) and global host (`pcp://{addr}`) endpoints. A global failure cooldown blocks connection attempts to all channels on that host address.
    - `HostRegistry` deduplication: keyed primarily by `(ChannelId, SessionId)`. If a peer restarts with a new session ID at the same socket address, the old session entry is evicted and replaced (`UpsertAction::ReplacedEndpoint`).
  - Bounded Resources & Physical Memory Cleanup (REQ-042, Global Rule 3):
    - Both `HostRegistry` and `EndpointCooldownTable` enforce configurable maximum capacities (`max_hosts`, `max_entries`).
    - Insertion at capacity first prunes expired entries; if still full, evicts the oldest entry (FIFO/LRU on monotonic timestamps).
    - Physical cleanup (`prune_expired()`) physically removes expired entries from internal HashMaps and indexes, ensuring zero unbounded heap growth during long running operations.
  - Zero-sleep Deterministic Testing:
    - All time queries operate via `CacheClock`, enabling 100% deterministic, zero-sleep fake-clock tests (`test_p31_host_freshness_cooldown.rs`, 7 tests passing).

### [2026-10-24] Task P32 Implementation: Named Parent-Selection Policy and Retry Coordination
- **Requirements Covered**: REQ-042, REQ-043 (and REQ-062)
- **Files Added / Changed**:
  - `crates/peercast-core/src/parent_selection.rs`
  - `crates/peercast-core/src/lib.rs`
  - `crates/peercast-core/tests/test_p32_parent_selection.rs`
- **Decisions & Parity Observations**:
  - Stop/block guard strictly satisfied: Parent selection is proven to be a named local node policy, not a universal wire-order rule (REQ-043). No universal 10-second default or unfounded subnet assumptions are used.
  - Implemented Named Policies:
    - `StationWeightedRandomPolicy`: faithful port of PeerCastStation `PCPSourceStream.cs` lines 924-932:
      `score = (IsSiteLocal ? 8000 : 0) + rnd.NextDouble() * ((IsReceiving ? 4000 : 0) + (!IsRelayFull ? 2000 : 0) + max(10 - hops, 0)*100 + relayCount*10)`.
    - `PeerCastYtGreedyHopPolicy`: faithful port of PeerCast-YT `pickHits` in `chanhit.cpp`, which prioritizes minimal upstream hops (`255 - hops`) among eligible hits.
  - Policy Diversity Verified in Tests:
    - Tested Candidate A (hops 1, remote) vs Candidate B (hops 8, site-local):
    - Under Station policy, Candidate B wins because the additive site-local boost (+8000) dominates remote candidates.
    - Under PeerCast-YT policy, Candidate A wins because it minimizes hops ($1 < 8$).
  - Pre-scoring Structural Eligibility Invariant:
    - Self session ID matches, self contact socket matches, unspecified IPs (`0.0.0.0`, `::`), port 0, stale nodes (age > threshold), and cooled-down endpoints in `EndpointCooldownTable` are completely excluded BEFORE scoring. Score cannot override structural eligibility.
  - Locality Detection (`LocalityDetector`):
    - Parity with Station `PCPSourceStream.IsSiteLocal`: matches candidate endpoints against local listener endpoints; falls back to RFC 1918 / loopback / link-local addresses when enabled; nodes without global endpoints default to site-local.
  - Reconnect Budget and Station 3/2 Backoff Arithmetic (REQ-043, REQ-062):
    - Bounded attempts per selection cycle (`max_attempts_per_cycle`, default 5).
    - Station reference backoff: delay starts at 1,000ms, advances via integer arithmetic `(delay * 3) / 2` (1000ms -> 1500ms -> 2250ms -> 3375ms -> 5062ms -> 7593ms...), capped at 30,000ms.
    - Successful connection resets budget and restores base 1,000ms backoff.
  - Deterministic Testing:
    - Built-in zero-dependency deterministic PRNG ([`DeterministicRng`]) and fixed sequences ([`FixedSequenceRng`]) ensure 100% reproducible test fixtures (7 tests passing in `test_p32_parent_selection.rs`).

### [2026-10-24] Task P33 Implementation: Bounded Gossip Forwarding and Storm Suppression
- **Requirements Covered**: REQ-044
- **Files Added / Changed**:
  - `crates/peercast-core/src/gossip.rs`
  - `crates/peercast-core/src/lib.rs`
  - `crates/peercast-core/tests/test_p33_gossip_forwarding.rs`
- **Decisions & Parity Observations**:
  - Stop/block guard strictly satisfied: TTL and hop semantics are NEVER reversed. TTL is decremented by 1 (`saturating_sub(1)`), while Hops are INCREMENTED by 1 (`saturating_add(1)`) with `u8` overflow protection saturating at 255. Decrementing hops is strictly prohibited (REQ-044).
  - Forwarding Eligibility & Destination Routing:
    - Station path parity enforced: forwarding requires incoming `ttl > 1` (default `min_forwarding_ttl = 2`). Incoming packets with TTL 0 or 1 are dropped from forwarding (`ExpiredTtl`).
    - Local destination check: if `dest == self_session_id`, packet is processed locally and strictly NOT forwarded (`forward_packet = None`).
    - Origin reflection check: if `from == self_session_id`, loopback reflection of our own packet is dropped.
    - Group validation: group byte must be a valid combination of `BroadcastGroup` flags (`Root = 1`, `Trackers = 2`, `Relays = 4`); unknown or zero groups are dropped (`UnknownGroup`).
  - Source Exclusion in Broadcast Routing:
    - `select_forwarding_recipients` strictly excludes the incoming source peer by session ID and socket address, preventing ping-pong reflection loops back to sender.
  - Bounded Storm Suppression Deduplication (REQ-044, Global Rule 3):
    - `GossipDedupCache` maintains a strictly bounded capacity (`max_entries`, default 1024) keyed by `(origin_session_id, channel_id, payload_md5_hash)`.
    - Duplicates received within the suppression window (default 60s) are dropped (`DuplicateStormSuppressed`).
    - Insertion at capacity first prunes expired entries; if still full, evicts the oldest entry (FIFO/LRU).
    - Physical cleanup (`prune_expired()`) physically removes expired entries from internal HashMaps, guaranteeing zero unbounded heap growth.
  - Test Matrix Verified:
    - 8 comprehensive tests passing in `test_p33_gossip_forwarding.rs` covering TTL 0/1/2, hop overflow at 255, local destinations, reflection loops, unknown groups, duplicate storm suppression, bounded cache capacity, and source exclusion.

### [2026-10-24] Task P34 Implementation: Controlled YellowPage Announcements
- **Requirements Covered**: REQ-035, REQ-062, REQ-063
- **Files Added / Changed**:
  - `crates/peercast-core/src/id4.rs`
  - `crates/peercast-pcp/Cargo.toml`
  - `crates/peercast-pcp/src/yellow_page.rs`
  - `crates/peercast-pcp/src/lib.rs`
  - `crates/peercast-pcp/tests/test_p34_yp_announcements.rs`
- **Decisions & Parity Observations**:
  - Stop/block guard strictly satisfied: Automated tests NEVER announce to public directories without authorized destination (REQ-063). `PCPYellowPageClient` enforces `allow_public_announcements`; attempting to announce to non-loopback public destinations without explicit authorization fails with `UnauthorizedPublicDestination`.
  - Station Announcement Protocol Parity (REQ-035):
    - Transmits a `bcst` parent Atom containing `chan` and `host` child atoms with:
      - `ttl = 1`
      - `hops = 0`
      - `from = session_id`
      - `group = 1` (`BroadcastGroup::ROOT`)
      - `cid = channel_id`
      - Do NOT use C++ channel ID hashing for Station-compatible announcements.
    - Verified against exact Station C# layout (`PCPSourceStream.cs` / `PCPYellowPageClient.cs`).
  - Reference Update Policy & 3/2 Backoff Arithmetic (REQ-062):
    - 30-second periodic announcement update interval (`DEFAULT_YP_UPDATE_INTERVAL`).
    - Error retry backoff: begins at 1000ms, advances via integer arithmetic `delay = (delay * 3) / 2` (1000ms -> 1500ms -> 2250ms -> 3375ms -> 5062ms -> 7593ms...), capped at 30,000ms.
    - Success resets backoff to 1000ms.
  - Terminal Response State Machine & Shutdown:
    - `Banned`: `dis\0 != 0` flag in `oleh` terminates retries immediately, setting `YpConnectionStatus::TerminalBanned`.
    - `ServerQuit`: directory `quit` atom with code `0x01000000` terminates retries immediately, setting `YpConnectionStatus::TerminalServerQuit`.
    - `Final Owner Removal`: channels are tracked via atomic refcount. When refcount hits 0, client transmits a clean `quit` atom and ceases all announcements (`YpConnectionStatus::Idle`).
  - Test Matrix Verified:
    - 7 comprehensive tests passing in `test_p34_yp_announcements.rs` covering Station-format Atom byte parity, public destination security guards, 3/2 integer backoff arithmetic, full loopback socket handshake and announcement, terminal banned, terminal server-quit, and refcount shutdown.

### [2026-10-24] Task P35 Implementation: Bounded Directory Parsing (`index.txt`)
- **Requirements Covered**: REQ-063, Global Rule 3
- **Files Added / Changed**:
  - `crates/peercast-core/src/index_txt.rs`
  - `crates/peercast-core/src/lib.rs`
  - `crates/peercast-core/tests/test_p35_directory_parser.rs`
- **Decisions & Parity Observations**:
  - Stop/block guard strictly satisfied: Directory listings are bounded and validated against resource exhaustion attacks (REQ-063, Global Rule 3). Unbounded reading of arbitrarily large index.txt feeds is strictly prohibited.
  - 19-Column Schema Parity:
    - Parsed according to PeerCastStation `PCPYellowPageClient.cs:1118-1177` and PeerCast-YT `public.cpp:115-136`, using `<>` as field delimiter:
      - Field 0: Name (HTML entity decoded)
      - Field 1: 32-character hexadecimal GUID (Channel ID)
      - Field 2: Tracker IP and port (TIP)
      - Field 3: Contact URL
      - Field 4: Genre
      - Field 5: Description
      - Field 6: Direct listener count (supports `-1` representing hidden)
      - Field 7: Relay count
      - Field 8: Stream bitrate (kbps)
      - Field 9: Content type / container format
      - Field 10: Artist
      - Field 11: Album
      - Field 12: Track title
      - Field 13: Track URL
      - Field 14: Escaped / raw channel name
      - Field 15: Uptime string (parsed into Duration supporting `HH:MM` and `HH:MM:SS`)
      - Field 16: Click / stats count
      - Field 17: Broadcaster comment
      - Field 18: Direct connection permission flags
  - Strict Bounded Resource Limits (`IndexParserLimits`):
    - `max_total_bytes`: Default 2 MB; input feeds exceeding this are rejected immediately (`ResponseTooLarge`).
    - `max_lines`: Default 10,000 channels; parsing halts cleanly once limit is reached to prevent unbounded allocations.
    - `max_field_len`: Default 4,096 bytes; oversized individual tokens are rejected immediately (`FieldTooLong`).
  - Full HTML Entity Decoding:
    - Implemented compliant decoder converting standard HTML entities (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#39;`, `&apos;`) and numeric unicode code points (`&#nnn;` and `&#xhhh;`) back to raw UTF-8 characters.
  - Partial & Tolerant Line Parsing:
    - Feed lines with fewer than 19 columns (common in legacy YP outputs) are safely parsed without panic; missing trailing fields are assigned sensible defaults.
  - Test Matrix Verified:
    - 8 comprehensive tests passing in `test_p35_directory_parser.rs` covering standard 19-column parsing, HTML entity decoding, partial/legacy column handling, uptime parsing (`HH:MM`, `HH:MM:SS`, total minutes), UTF-8 Japanese characters, field length rejection, total response size rejection, and line count bounding.

### [2026-10-24] Task P36 Implementation: Dual-Stack IPv4/IPv6 Interop and Peer Probe Resolution
- **Requirements Covered**: REQ-024, REQ-034, REQ-036, REQ-071
- **Files Added / Changed**:
  - `crates/peercast-pcp/src/relay_server.rs`
  - `crates/peercast-pcp/src/probe.rs`
  - `crates/peercast-pcp/src/lib.rs`
  - `crates/peercast-pcp/tests/test_p19_reverse_probe.rs`
  - `crates/peercast-pcp/tests/test_p36_dual_stack_interop.rs`
- **Decisions & Parity Observations**:
  - Stop/block guard strictly satisfied: Unsupported configurations reported explicitly rather than silently falling back to IPv4 (REQ-034, REQ-071).
  - Header 1 / 100 Consistency Matrix (Station Parity `PCPOutputStream.cs:938-942`):
    - Station evaluates channel network, request header `x-peercast-pcp`, and remote endpoint address family as a combined unit.
    - Missing `x-peercast-pcp` header: returns `RelayServerError::MissingPcpHeader` (HTTP 400 Bad Request).
    - `RelayNetworkProfile::IPv4`: requires `pcp_version == 1` and IPv4 address family.
    - `RelayNetworkProfile::IPv6`: requires `pcp_version == 100` and native IPv6 address family.
    - Any mismatch (IPv4 channel with header 100, IPv4 socket with IPv6 channel, etc.) immediately returns `RelayServerError::NetworkFamilyMismatch` (HTTP 404 Not Found).
  - IPv4-Mapped Address Handling:
    - Dual-stack socket addresses formatted as `::ffff:a.b.c.d` are explicitly identified and unmapped to IPv4 `a.b.c.d`.
    - Unmapped addresses on IPv4 channels with header 1 are valid.
    - Unmapped addresses on IPv6 channels are rejected with 404 because the logical underlying socket is IPv4, not native IPv6.
  - Probe Behavior Differences Resolved (REQ-036):
    - Audited upstream implementations:
      - Station `PCPOutputStream.cs:341` always writes `Atom(Atom.PCP_CONNECT, 1)` even on IPv6, and Station pong factory (`PCPPongOutputStreamFactory.cs:64`) strictly checks for `version == 1` in byte 8 (rejecting any other version).
      - PeerCast-YT `servent.cpp:494-498` writes `version 1` for IPv4 / mapped, and `version 100` for native IPv6. YT pong listener accepts both 1 and 100.
      - Interoperability discovery: YT pinging Station over IPv6 sends version 100, which Station rejects!
    - Implemented `ProbePeerProfile` (`PeerCastStation` vs `PeerCastYt`):
      - Station profile emits version 1 on IPv6.
      - YT profile emits version 100 on native IPv6.
      - Pong responder (`validate_pong_preamble`) accepts both version 1 and 100, maximizing compatibility.
    - Retained safety guard: `ProbeLimits::default()` has `enable_ipv6: false`, preserving `ProbeError::IPv6Pending` until explicitly enabled.
  - Dual-Stack Relay Loopback Handshake & 503 Candidate Distribution:
    - Loopback 200 OK relay streaming verified over IPv6 `[::1]` with `x-peercast-pcp: 100`.
    - 503 candidate host distribution verified over IPv6 with 16-byte reversed addresses (REQ-024).
    - Position cursor reconnection verified over IPv6 (`x-peercast-pos` + `x-peercast-pcp: 100`).
  - Test Matrix Verified:
    - 7 comprehensive tests passing in `test_p36_dual_stack_interop.rs` covering consistency matrix, IPv4-mapped address handling, IPv6 loopback relay 200 handshake, dual-stack 503 candidate host distribution, Station vs YT probe differences, default IPv6 pending guard, and IPv6 cursor reconnection.

### [2026-10-24] Task P37 Implementation: Freeze Full RPC/UI Contract
- **Requirements Covered**: REQ-064, REQ-065
- **Files Added / Changed**:
  - `Cargo.toml`
  - `crates/peercast-rpc/Cargo.toml`
  - `crates/peercast-rpc/src/protocol.rs`
  - `crates/peercast-rpc/src/server.rs`
  - `crates/peercast-rpc/src/lib.rs`
  - `crates/peercast-rpc/tests/test_p37_rpc_contract.rs`
  - `docs/RPC_CONTRACT.md`
- **Decisions & Parity Observations**:
  - Stop/block guard strictly satisfied: JSON-RPC method names alone are not treated as parity (REQ-064). A complete inventory from `APIHost.cs` and actual UI call sites (`html/js/*.js`) has been audited and frozen into `docs/RPC_CONTRACT.md`.
  - Zero Success Stubs Policy:
    - Supported core methods: `getVersionInfo`, `getStatus`, `getChannels`, `getSettings`, `setSettings`, `getAuthToken`, `getPlugins`.
    - All other 38 audited UI methods (e.g. `broadcastChannel`, `checkBandwidth`, `checkPorts`, `updateAndRestart`, `stopChannel`, etc.) return error code `-32004` (`UNSUPPORTED_METHOD`) with detailed capability metadata. No success stubs are used.
  - Access Level Isolation (Station `OutputStreamType` Parity):
    - `Playback`: public viewers and embedded players can access query methods (`getVersionInfo`, `getStatus`, `getChannels`, `getAuthToken`).
    - `Admin`: administrative interface required for settings, mutations, and deferred controls. Unauthorized attempts return `-32001` (`UNAUTHORIZED`).
  - Standard JSON-RPC 2.0 Framing & Semantics:
    - Single requests, batch requests, and empty batch rejection (`INVALID_REQUEST` -32600).
    - Notifications: requests omitting `id` execute properly and suppress output responses (yielding `None` / HTTP 204).
    - Request limits: 64 KiB request body limit enforced (`DEFAULT_MAX_RPC_REQUEST_BYTES`).
  - Test Matrix Verified:
    - 8 comprehensive tests passing in `test_p37_rpc_contract.rs` covering core queries, settings read and mutation, access control enforcement, deferred method capability reporting, unknown method errors, batch processing, notification execution, and size limits / malformed JSON.

### [2026-10-24] Task P37A Implementation: Map UI Layer Boundaries
- **Requirements Covered**: REQ-080, REQ-085
- **Files Added / Changed**:
  - `docs/UI_LAYER_BOUNDARIES.md`
- **Decisions & Parity Observations**:
  - Stop/block guard strictly satisfied: A wholesale framework rewrite is rejected; existing Knockout.js 3.5.1 + Bootstrap 5.3.3 stack is preserved without regression (REQ-085).
  - Inward-Pointing Dependency Contract (REQ-080):
    - Established strict boundary rules: Screens -> Components -> Tokens; Screens -> Controllers -> API Adapter -> Backend.
    - Views and presentational components are strictly forbidden from performing direct `fetch`/`$.ajax` calls or knowing backend RPC method strings.
    - Controllers own reactive view states (`idle`, `loading`, `error`, `empty`, `pendingMutation`) and are testable without browser DOM or a running daemon.
    - Backend crates have zero knowledge of CSS classes, DOM structure, or component styling tokens.
  - Complete 45-Method Mapping:
    - Every audited JSON-RPC method is explicitly assigned an owning controller (`IndexController`, `ChannelsController`, `RelaysController`, `SettingsController`, `LogsController`, `UpdateController`, `UIViewModel`) and typed adapter method.
  - Incremental Screen Migration Road-map:
    - Phase 1: `index.html` (Index Screen: `getVersionInfo`, `getStatus`, `getPlugins`).
    - Phase 2: `channels.html` and `play.html`.
    - Phase 3: `settings.html` and `relays.html`.

### [2026-10-24] Task P37B Implementation: Typed API Adapter and View Contracts
- **Requirements Covered**: REQ-081, REQ-084
- **Files Added / Changed**:
  - `crates/peercast-rpc/src/client.rs`
  - `crates/peercast-rpc/src/lib.rs`
  - `crates/peercast-rpc/tests/test_p37b_client_adapter.rs`
- **Decisions & Parity Observations**:
  - Stop/block guard strictly satisfied: Mock success is NEVER used to hide unsupported real API behavior (REQ-064, REQ-081). Calling deferred methods through mock or live transport returns explicit `ClientRpcError::UnsupportedMethod` with capability details.
  - Centralized Typed API Client (`PeerCastRpcClient<T: RpcTransport>`):
    - UI controllers interact exclusively with typed methods (`get_version_info`, `get_status`, `get_channels`, `get_settings`, `set_settings`, `get_auth_token`, `get_plugins`). No raw RPC method strings are constructed in presentation components.
    - `RpcTransport` abstraction enables `MockRpcTransport` (offline controller testing without a running daemon) and `InprocessDispatcherTransport` (direct in-memory roundtrips).
  - Presentation View Models Decoupled from Wire Formats:
    - `ChannelViewModel`: converts wire bitrate into formatted display bitrate (e.g. "1500 kbps"), preserving numeric rates for sorting while providing display-ready text.
    - `StatusViewModel`: formats raw seconds into `HH:MM:SS`, maps firewall booleans to Japanese localized labels ("ポート開放中 (Open)", "ポート未開放 (Firewalled)"), and formats endpoint socket strings.
  - Non-Idempotent Mutation Protection (REQ-084):
    - `PeerCastRpcClient::is_mutating_method` flags state-altering calls (`setSettings`, `stopChannel`, `setChannelInfo`, `addYellowPage`, etc.). Callers are strictly barred from blindly retrying uncertain mutations on transport disconnects.
  - Test Matrix Verified:
    - 6 comprehensive tests passing in `test_p37b_client_adapter.rs` covering mock transport queries without a daemon, ViewModel conversions, error normalization, unsupported method capability reporting without fake success, mutation retry classification, and in-memory client-dispatcher roundtrips.

---

## 3. Next Actionable Tasks
- **P37C — Extract presentation controllers** (Prerequisites: P37B; Requirements: REQ-081, REQ-084).
- **P37D — Create themeable pure presentation** (Prerequisites: P37C; Requirements: REQ-082, REQ-083).
