# Agent Implementation Runbook

This is the executable work plan for SPECIFICATION.md, not evidence that implementation has started. All 50 task statuses begin not_started because the local Rust repository was not inspected in this review. Existing work may be credited only after P01/P02 establish evidence.

## Operating protocol

1. Load the specification, current work log, this runbook and implementation-plan.json. Do not implement from the old originals.
2. Select the earliest ready task: all depends_on tasks passed with evidence applicable to the current baseline. Do not infer readiness from task number alone.
3. Claim one task and its files. Do not edit a file owned by another active writer. Root coordinates cross-task interfaces.
4. Before editing, restate inputs, scope, acceptance tests and stop conditions in the work record. Run the smallest baseline test.
5. Make only the task-scoped change; write positive and negative tests. Add separate integration tests when prescribed.
6. Run recorded commands discovered from the repository, not guessed script names. Save sanitized output and exact environment/commit/dirty digest.
7. Mark passed only when every acceptance condition is met. Otherwise use failed, blocked or needs_review with the missing evidence.
8. On failure: preserve output; isolate the failing layer; add a regression; make the smallest fix; rerun the task and invalidate downstream evidence affected by the change. Never change expected bytes or loosen a limit solely to turn a test green.
9. Do not bypass a blocked dependency by inventing semantics. Independent ready tasks may continue only with nonoverlapping ownership.
10. Record next action before handoff. No publish/main merge/public-network test is authorized merely by this runbook.

## State model and gates

Allowed task states: not_started -> in_progress -> passed | failed | blocked | needs_review. A failed/blocked/review task may re-enter in_progress after its issue is addressed. passed -> needs_review whenever its relevant inputs change. Record why.

Evidence per task: requirement IDs; actual files/symbols; baseline/current commit and dirty digest; commands; test count/result; fixture hashes; peer build/version; platform/profile; artifact links; outstanding limits. Protect credentials and redact private endpoint details in shared logs.

Task size is defined by one testable contract, not elapsed time. Split a task further if it spans unresolved contracts. Do not combine several phases into one unreviewable change.

Checkpoint gates: P08 codec safety; P21 independent IPv4 relay; P25 actual direct playback; P36 IPv6; P41 package/runtime; P44 release assessment. P27..P30 are independent format tracks after P25; they are not required for the first playable slice, but are required for the full-format release described here. If scope is deferred, record the exclusion explicitly rather than passing the task.

The DAG permits concurrency but does not instruct agents to concurrently edit shared files. No phase automatically authorizes installation of unrecognized executables or testing against public peers.

## P01 — Inventory actual workspace

Prerequisites: none. Requirements: REQ-001, REQ-002, REQ-003.
Change scope: Read-only repository/environment inspection; write inventory artifact only.

Steps:
1. Locate the requested Rust repository; read applicable instructions and current Git branch/status.
2. List running owners/tests and identify unrelated edits before any edit.
3. Record actual compiler/Cargo, OS, existing crates, tests and configured remotes; classify each as observed or unavailable.

Pass only when:
- Inventory includes baseline commit, dirty-file list and ownership uncertainty.
- No claim of missing or completed implementation solely from the supplied work log.

Stop / block when:
- Repository inaccessible, competing writer, or ownership of proposed edits uncertain: do not initialize or overwrite.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P02 — Map existing code to requirements

Prerequisites: P01. Requirements: REQ-001, REQ-004, REQ-070, REQ-072.
Change scope: Inventory and traceability documents only.

Steps:
1. Map all 58 requirement IDs to existing symbols/tests or explicitly missing/unverified.
2. Mark equivalent existing functionality for verification instead of duplicate implementation.
3. Record baseline test commands discovered from repository instructions and their results.

Pass only when:
- Every requirement classified; existing failures retained with logs.
- Each planned change has an identified file owner.

Stop / block when:
- Existing equivalent code differs materially: resolve contract before replacing.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P03 — Freeze first-slice contracts

Prerequisites: P02. Requirements: REQ-020, REQ-022, REQ-026, REQ-031, REQ-038, REQ-072.
Change scope: Protocol field matrix and first-slice design decisions; no production logic.

Steps:
1. Trace pinned handlers AND callers for helo/oleh/ok/quit/chan/host fields, types, requiredness, duplicates and unknowns.
2. Specify position width/rollover, stream generation changes, reconnect cursor and oversized-header strategy.
3. Select one supported media fixture and controlled IPv4 peer topology; explicitly exclude unimplemented profiles.

Pass only when:
- Field matrix has source locators and positive/negative fixtures.
- No OPEN item needed by the first slice remains implicit.

Stop / block when:
- Conflicting sources without a named profile; missing fixture for a required field.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P04 — Establish minimal build and dependency DAG

Prerequisites: P03. Requirements: REQ-002, REQ-010.
Change scope: Existing Cargo workspace and minimum core/app wiring only.

Steps:
1. Preserve existing workspace; add only missing first-slice crates.
2. Pin selected toolchain/dependencies consistent with verified policy.
3. Check dependency direction; provide adapter contracts below servent.

Pass only when:
- Workspace check/test baseline runs using recorded commands.
- No adapter imports concrete servent; no dependency cycle.

Stop / block when:
- Dependency or edition change lacks a reason; package access/install requires unavailable authorization.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P05 — Implement raw ID and scalar types

Prerequisites: P04. Requirements: REQ-020, REQ-021, REQ-022.
Change scope: core ID4 and fixed-width scalar helpers only.

Steps:
1. Represent ID4 as four bytes; support NUL padding without trimming on decode.
2. Use explicit little-endian scalar helpers and exact-width validation.
3. Separate display formatting from wire storage.

Pass only when:
- Tests cover pcp newline, sid NUL, empty/overlong IDs and u16/u32 byte order.
- Malformed widths fail without reading outside payload.

Stop / block when:
- Host-native representation leaks into serialized bytes.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P06 — Implement Atom encoder

Prerequisites: P05. Requirements: REQ-020, REQ-021, REQ-022.
Change scope: core encoder only.

Steps:
1. Encode leaf and parent separately, including empty variants.
2. Keep ordered duplicate children.
3. Reject size/count overflow before writing an invalid header.

Pass only when:
- Independent expected bytes match zero-child parent, zero-byte leaf, nested/duplicate cases.
- Generic encoder does not impose packet-data cap on all leaves.

Stop / block when:
- A round-trip test is the sole evidence of correctness.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P07 — Implement incremental Atom decoder

Prerequisites: P06. Requirements: REQ-025.
Change scope: core decoder and fragmentation tests only.

Steps:
1. Retain unconsumed bytes between reads; distinguish need-more, clean EOF and malformed/truncated input.
2. Decode one tree while leaving following trees buffered.
3. Use checked lengths and iterative/bounded nesting.

Pass only when:
- Every split position of fixed valid examples yields the same tree.
- Concatenated Atoms and EOF at every byte boundary are tested.
- Child order and duplicate IDs survive.

Stop / block when:
- Decoder allocates from unchecked peer length or treats incomplete input as success.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P08 — Enforce layered parser budgets

Prerequisites: P07. Requirements: REQ-025, REQ-026, REQ-027.
Change scope: core parse budgets and negative tests.

Steps:
1. Implement leaf, child, depth, total-node and total-byte budgets independently.
2. Charge unknown fields and skipped trees to the same budget.
3. Test limit-1, limit, limit+1 and cancellation cleanup.

Pass only when:
- Each budget rejects only its own invalid case with a stable error.
- Deep nesting and many small leaves cannot bypass cumulative limits.
- Allocation/retained-input accounting is observable.

Stop / block when:
- Budget reset inside a nested child permits unbounded aggregate input.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P09 — Implement protocol identifiers and IP conversions

Prerequisites: P05. Requirements: REQ-023, REQ-024.
Change scope: core conversion functions, no channel hashing yet.

Steps:
1. Separate canonical protocol ID bytes, .NET GUID layout and display hex.
2. Implement complete-byte address reversal for4/16 byte payloads.
3. Define invalid lengths and IPv4-mapped-address policy explicitly.

Pass only when:
- Non-symmetric GUID fixture matches both layouts.
- IPv4 and IPv6 fixed vectors pass; invalid lengths fail.

Stop / block when:
- GUID hashing conversion is reused indiscriminately for sid/cid fields.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P10 — Implement Station broadcast ID profile

Prerequisites: P09. Requirements: REQ-040, REQ-041.
Change scope: core Station identity helper and tests only.

Steps:
1. Implement SHA512 of .NET-layout BCID, conditional network prefix, BinaryWriter string encoding, MD5 and output GUID conversion.
2. Keep received ChannelIDs opaque; name this Station profile explicitly.
3. Obtain independently executed .NET/upstream expected outputs before certifying parity.

Pass only when:
- Empty/Japanese/127-128-byte boundary strings and IPv4/IPv6 tested.
- Independent reference outputs agree; review-generated Python examples alone do not satisfy parity.

Stop / block when:
- Only the same translated algorithm generated both actual and expected outputs.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P11 — Implement raw quit-code preservation

Prerequisites: P05. Requirements: REQ-038.
Change scope: core errors and protocol mapping table.

Steps:
1. Store raw u32/int wire value with optional known interpretation.
2. Map common quit reasons and observed4000 context explicitly.
3. Separate local stop reason/retry policy from wire parsing.

Pass only when:
- 1000,1001,1003,1008,1009,4000 and unknown values retain exact raw codes.
- Unknown code does not panic or silently become normal shutdown.

Stop / block when:
- Code assumes all valid values are1000..1011.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P12 — Define actor handles and lifecycle ownership

Prerequisites: P04. Requirements: REQ-011, REQ-012, REQ-014.
Change scope: core command/event interfaces and minimal worker harness.

Steps:
1. Separate running worker from cloneable command handle and status snapshot.
2. Assign cancellation owner, join path and generation token.
3. Define terminal outcomes and idempotent stop behavior.

Pass only when:
- Post while worker runs without second mutable borrow or lock across await.
- Stale generation events ignored; double stop and shutdown during startup tested.

Stop / block when:
- A task is spawned without tracked ownership/join behavior.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P13 — Implement content epochs and bounded cache

Prerequisites: P12. Requirements: REQ-027, REQ-050, REQ-051.
Change scope: core content cache only.

Steps:
1. Represent header/body association with stream epoch and transport positions.
2. Apply both time and byte eviction, with separate header budget.
3. Account retained allocations, gaps/overlap and restart according to P03 contract.

Pass only when:
- Time-only, byte-only and simultaneous eviction tests pass with a fake clock.
- No old-epoch body follows a new header.
- Oversized shared backing allocation cannot evade accounting.

Stop / block when:
- Missing/oversized header is silently truncated or stale body reused.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P14 — Implement subscriber admission and writers

Prerequisites: P13. Requirements: REQ-013, REQ-028.
Change scope: bounded subscriber queues and fake writers.

Steps:
1. Use nonblocking admission in fan-out; per-subscriber writer owns network awaits.
2. Enforce byte/count/age limits and explicit slow-peer stop.
3. Release queued buffers on cancellation.

Pass only when:
- One stalled receiver cannot stop a healthy receiver.
- Overflow produces one terminal event; memory returns after subscriber removal.
- Fan-out complexity is documented as O(N).

Stop / block when:
- Channel actor awaits a slow subscriber or queue has no byte budget.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P15 — Implement bounded HTTP relay header parser

Prerequisites: P08, P11, P12. Requirements: REQ-030, REQ-031, REQ-037.
Change scope: pcp HTTP prelude, not direct-player/ingest server.

Steps:
1. Parse supported statuses/headers with byte/line/time limits.
2. Validate channel hex and reconnect numeric fields.
3. Transfer already-read binary tail into Atom decoder.

Pass only when:
- Header and first Atom in one read work.
- Fragmented CRLF, malformed status, oversized header and timeout tested.
- 404 never enters Atom state.

Stop / block when:
- HTTP-to-Atom transition discards buffered bytes.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P16 — Implement relay consumer200 path

Prerequisites: P15, P09. Requirements: REQ-031, REQ-032, REQ-034.
Change scope: pcp client state machine against scripted local peer.

Steps:
1. Connect, send selected network profile and cursor, consume200 and perform helo/oleh.
2. Validate identity/version fields using P03 matrix.
3. Accept permitted ok/control/chan ordering and preserve unrelated fields.

Pass only when:
- Scripted success, bad SID/version, disconnect and cancellation in each state pass.
- No pcnt or redundant raw preamble is sent.

Stop / block when:
- Any required handshake field/ordering is guessed rather than in P03.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P17 — Implement relay consumer503 and404 paths

Prerequisites: P16. Requirements: REQ-032, REQ-033, REQ-038.
Change scope: client alternate status branches.

Steps:
1. After503 perform helo/oleh before collecting host candidates and quit.
2. Validate candidates without connecting automatically outside controlled test scope.
3. Map404/off-air and other statuses to explicit outcomes.

Pass only when:
- 503 transcript includes handshake, host list, quit and close in order.
- Empty/invalid candidates and unknown quit values terminate predictably.

Stop / block when:
- 503 skips handshake or induces an unbounded reconnect loop.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P18 — Implement relay server handshake branches

Prerequisites: P17, P14. Requirements: REQ-032, REQ-033, REQ-034.
Change scope: pcp server with scripted local clients.

Steps:
1. Serve200/503/404 based on channel/slot state.
2. Perform helo validation and oleh; emit Station-compatible ok1 on accepted path.
3. Full path sends candidate hosts then appropriate quit after handshake.

Pass only when:
- Each server path tested independently from client to avoid shared-bug agreement.
- Slots and resources released on every failure/cancel branch.

Stop / block when:
- Client/server tests only connect two copies of the same Rust implementation.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P19 — Implement reverse probe and raw preamble

Prerequisites: P18. Requirements: REQ-035, REQ-036, REQ-037.
Change scope: pcp probe worker and raw preamble codec.

Steps:
1. Use pcp newline Atom, observed peer IP and announced port; check returned SID.
2. Bound connect/read/write and global/per-peer probe concurrency.
3. Test probe-before-oleh ordering and network-profile differences.

Pass only when:
- Matching/mismatched SID, refused/stalled connection and cancellation tested.
- IPv4 probe proven in controlled topology; IPv6 remains separately marked pending.

Stop / block when:
- Probe can target an unrelated arbitrary address or is mistaken for NAT configuration.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P20 — Connect channel packets to cache and relay

Prerequisites: P19, P13. Requirements: REQ-026, REQ-050, REQ-051.
Change scope: pcp packet adapter; reuse contract from P03.

Steps:
1. Validate head/data types, positions and continuation fields.
2. Feed epoch/header/body events to cache; split outgoing body safely under16384 bytes.
3. Relay unknown extensions within budget without corrupting semantic fields.

Pass only when:
- Packet cap boundaries, gaps, restart, metadata changes and fragmented transport tested.
- Known fixture bytes are unchanged across relay transport.

Stop / block when:
- Oversized header strategy or rollover semantics still undefined.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P21 — Prove controlled IPv4 interoperability

Prerequisites: P20, P10. Requirements: REQ-003, REQ-070, REQ-071.
Change scope: test harness and evidence, production changes only for diagnosed defects.

Steps:
1. Pin executable versions/build SHAs of Station and YT separately.
2. Run both directions for200,503,404, probe and reconnect using controlled endpoints.
3. Capture sanitized transcripts and hashes; preserve failures.

Pass only when:
- Every required first-slice matrix row passes against both independent peers.
- No public YP announcement or unauthorized public traffic used.

Stop / block when:
- Reference executable unavailable: mark blocked, never infer pass from source review.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P22 — Implement bounded container detection

Prerequisites: P21. Requirements: REQ-050, REQ-056.
Change scope: format reader interface and first-slice sniffer.

Steps:
1. Use a bounded prefix with replay to chosen reader.
2. Declare supported container/codec subset and ambiguous/unsupported outcome.
3. Do not consume bytes irretrievably during detection.

Pass only when:
- Short reads, misleading signatures, empty input and unsupported format tested.
- Selected fixture reaches its reader with identical bytes.

Stop / block when:
- File extension alone selects an unverified parser.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P23 — Implement first FLV header/body path

Prerequisites: P22. Requirements: REQ-051, REQ-052.
Change scope: FLV subset selected in P03; no implicit enhanced-codec support.

Steps:
1. Parse header/DataOffset/PreviousTagSize and bounded tags.
2. Cache relevant metadata and codec configs with epoch changes.
3. Preserve timestamp/transport distinctions and reject malformed lengths.

Pass only when:
- Fixture includes PreviousTagSize0 and metadata/config ordering.
- Audio-only, changed config, truncated/oversized tag tested.
- Reconstructed bytes parse in an independent reader/player.

Stop / block when:
- 9-byte header alone emitted or extended tags silently treated as supported.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P24 — Implement safe-start selection

Prerequisites: P23, P13. Requirements: REQ-050, REQ-055.
Change scope: format-aware start-point selection.

Steps:
1. Use trusted continuation/safe-boundary markers; distinguish packet fragments from frames.
2. Bound waiting and define audio-only/no-marker fallback or clear failure.
3. Reset selection on format/epoch change.

Pass only when:
- Interframe-first, fragmented keyframe, audio-only, no-keyframe timeout and restart fixtures pass.
- No indefinite waiting or universal IDR claim.

Stop / block when:
- cont==0 is treated as proof of H.264 IDR for every format.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P25 — Implement direct playback and playlists

Prerequisites: P24, P14. Requirements: REQ-060.
Change scope: http /stream and /pls adapters.

Steps:
1. Validate requested ID; acquire/release subscriber through handle.
2. Choose version/media-appropriate framing and MIME; escape playlist fields.
3. Handle client disconnect and on-demand startup/shutdown.

Pass only when:
- Actual player decodes first-slice fixture; slow/cancel paths release resources.
- HTTP framing and playlist escaping have negative tests.

Stop / block when:
- Chunking applied blindly to HTTP1.0/MMS or playback success inferred from HTTP200.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P26 — Define and implement first ingest endpoint

Prerequisites: P25. Requirements: REQ-061, REQ-065.
Change scope: explicitly selected ingest listener contract.

Steps:
1. Choose Station-compatible dedicated URI listener or separately approved/documented Rust /push route.
2. Specify authorization, binding, channel ownership and supported transfer framing.
3. Implement bounded POST ingest feeding existing format pipeline.

Pass only when:
- Authorized/unauthorized POST, conflicting bind, cancellation and malformed chunking tested.
- ffmpeg/OBS workflow tested only for selected real output protocol and format.

Stop / block when:
- A shared /push/{id} endpoint is invented as universal upstream behavior.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P27 — Implement TS support independently

Prerequisites: P25. Requirements: REQ-053, REQ-056.
Change scope: TS parser/fixtures; separate from FLV.

Steps:
1. Bound188-byte framing and PSI reassembly.
2. Handle PAT/PMT changes, continuity/discontinuity and codec startup requirements.
3. Declare supported codec subset.

Pass only when:
- Changed tables, truncated sections and transport gaps tested.
- Independent player decodes late join; PAT/PMT-only success is not enough.

Stop / block when:
- No codec configuration/random-access evidence for claimed playback support.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P28 — Implement ASF/MMS support independently

Prerequisites: P25. Requirements: REQ-054, REQ-056.
Change scope: ASF object and MMS framing parser.

Steps:
1. Validate GUID/u64 sizes before narrowing and nested traversal.
2. Distinguish raw ASF object headers from MMS header frames.
3. Use format-specific response profile.

Pass only when:
- Malformed size/count/truncation and header replacement tested.
- Selected reference/player workflow decodes.

Stop / block when:
- Assumed50-byte minimum or raw ASF path mislabeled MMS-compatible.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P29 — Implement MKV support independently

Prerequisites: P25. Requirements: REQ-051, REQ-055, REQ-056.
Change scope: MKV bounded parser and fixtures only.

Steps:
1. Trace pinned MKV reader and define element/codec subset before code.
2. Bound variable-length elements/header cache; identify safe cluster/frame starts.
3. Integrate epoch and cancellation contract.

Pass only when:
- Unknown/oversized/truncated elements and mid-stream startup tested.
- Independent decoder/player verifies supported subset.

Stop / block when:
- Parser contract not established; do not clone FLV assumptions.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P30 — Implement Ogg and decide audio-only scope

Prerequisites: P25. Requirements: REQ-051, REQ-055, REQ-056.
Change scope: Ogg reader plus explicit MP3 support decision.

Steps:
1. Trace Ogg page/header/serial handling and codec identification.
2. Bound lacing/packet reconstruction and chained-stream state.
3. Document whether MP3 is supported or deferred with visible capability output.

Pass only when:
- Truncated pages, serial/header changes and audio-only startup tested.
- Independent playback validates each claimed codec.

Stop / block when:
- Ogg container support is equated with every codec it can contain.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P31 — Implement host freshness and failure cooldown

Prerequisites: P21. Requirements: REQ-042.
Change scope: servent host registry and separate ignored-endpoint table.

Steps:
1. Use independent monotonic age/cooldown values and bounded maps.
2. Expire via access/insertion and scheduled physical cleanup.
3. Define endpoint identity and session deduplication.

Pass only when:
- Fake-clock tests prove freshness does not accidentally clear failure cooldown or vice versa.
- Expired entries release memory and new host updates refresh only appropriate state.

Stop / block when:
- Single180-second timer controls unrelated concepts.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P32 — Implement named parent-selection policy

Prerequisites: P31. Requirements: REQ-043.
Change scope: servent selector/retry coordination.

Steps:
1. Implement explicitly chosen Station-style weighted randomized policy or ADR-approved alternative.
2. Validate self exclusion, locality and eligibility independently from score.
3. Use seeded RNG tests and bounded attempt/reconnect budget.

Pass only when:
- Deterministic fixture selection and distribution/eligibility checks pass.
- Not described as universal YT/Station strict hierarchy.

Stop / block when:
- LAN subnet assumptions or10-second default appear without source/policy evidence.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P33 — Implement bounded gossip forwarding

Prerequisites: P32. Requirements: REQ-044.
Change scope: pcp/servent broadcast routing boundary.

Steps:
1. Apply destination/group/origin eligibility.
2. Decrement TTL and increment hops with overflow protection.
3. Define bounded loop/storm suppression policy.

Pass only when:
- TTL0/1/2, local destination, unknown group, hop overflow and repeated input tested.
- No forwarding back to excluded source.

Stop / block when:
- TTL and hop semantics reversed or dedup map unbounded.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P34 — Implement controlled YP announcements

Prerequisites: P33. Requirements: REQ-035, REQ-062, REQ-063.
Change scope: YP client, local fake directory only initially.

Steps:
1. Implement raw PCP profile, channel ownership/refcount and shutdown.
2. Use30-second reference update policy and error retry1000ms*3/2 capped 30000.
3. Stop on terminal banned/server-quit/cancel; document jitter if added.

Pass only when:
- Fake-clock update/backoff/terminal-state tests pass.
- No announcement remains after final owner removal.

Stop / block when:
- Test would announce to a public YP without authorized destination.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P35 — Implement directory listing contract

Prerequisites: P34. Requirements: REQ-063.
Change scope: bounded HTTP index.txt parser.

Steps:
1. Obtain representative authorized fixtures and document encoding/columns/escaping.
2. Bound total bytes, rows and per-field lengths.
3. Separate listing from PCP announcement connection lifecycle.

Pass only when:
- Malformed rows/encoding/optional columns and response limits tested.
- Parsed fixtures match independently checked expected metadata.

Stop / block when:
- Column schema or character encoding guessed.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P36 — Prove IPv6 and mixed-family behavior

Prerequisites: P35. Requirements: REQ-024, REQ-034, REQ-036, REQ-071.
Change scope: IPv6 profiles and independent-peer matrix.

Steps:
1. Test header1/100 consistency with channel and socket families.
2. Resolve probe behavior differences with both pinned peers.
3. Exercise IPv4-mapped addresses and invalid family combinations.

Pass only when:
- Both directions/peers,200/503/probe/reconnect rows have explicit results.
- Unsupported configurations reported rather than silently falling back.

Stop / block when:
- No usable IPv6 topology: retain blocked status; do not infer pass fromIPv4.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P37 — Freeze full RPC/UI contract

Prerequisites: P25. Requirements: REQ-064.
Change scope: API inventory and UI call-site audit, no UI copying yet.

Steps:
1. List actual UI method calls and schemas/access requirements.
2. Define error, batch, notification and auth behavior per selected compatibility profile.
3. Map supported/deferred methods to visible capabilities.

Pass only when:
- Each required UI action maps to a tested API contract or explicit unsupported state.
- No success stubs.

Stop / block when:
- Only method-name parity is available.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P37A — Map UI layer boundaries

Prerequisites: P37. Requirements: REQ-080, REQ-085.
Change scope: frontend dependency map and upstream UI coupling inventory only.

Steps:
1. Locate existing frontend framework, assets, network calls, inline handlers and styles.
2. Assign modules to tokens, components, screens, controllers, API adapter and contracts.
3. Choose incremental extraction boundaries; preserve existing UI behavior.

Pass only when:
- Every existing API call has an owning adapter/controller path.
- Allowed/forbidden imports and the first screen migration are documented.

Stop / block when:
- A wholesale rewrite or framework replacement is proposed without demonstrated need.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P37B — Implement typed API adapter and view contracts

Prerequisites: P37A. Requirements: REQ-081, REQ-084.
Change scope: frontend API adapter, typed screen models/events and fake transport tests.

Steps:
1. Centralize JSON-RPC schemas, IDs, auth, errors and cancellation.
2. Define view models and semantic events separate from backend wire objects.
3. Keep permission enforcement in backend; UI availability is informational.

Pass only when:
- Transport/schema/error/cancel tests run without a real daemon.
- No component constructs RPC method names; uncertain mutations are not blindly retried.

Stop / block when:
- Mock success is used to hide unsupported real API behavior.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P37C — Extract presentation controllers

Prerequisites: P37B. Requirements: REQ-081, REQ-084.
Change scope: screen state/use-case controllers, no visual redesign.

Steps:
1. Model loading/empty/error/forbidden/disconnected/stale/pending states.
2. Handle stale reads and duplicate pending actions deterministically.
3. Bind user intent to adapter calls without parsing labels or DOM layout.

Pass only when:
- Fake-adapter tests cover read races, rejected mutations and uncertain responses.
- Controller behavior tests do not require CSS/DOM positioning.

Stop / block when:
- Business state derives from colors, labels or component placement.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P37D — Create themeable pure presentation

Prerequisites: P37C. Requirements: REQ-082, REQ-083.
Change scope: tokens, presentational components and fixture gallery.

Steps:
1. Extract semantic design tokens and local structural styles.
2. Render representative screens/components from props and emit semantic events.
3. Add all required state fixtures plus keyboard/focus/responsive/reduced-motion checks.

Pass only when:
- Fixture UI renders with no daemon or network.
- Production cannot silently use mock transport on real-backend failure.
- Design changes do not edit controller or API modules.

Stop / block when:
- Components import transport/domain implementation or accessibility is lost.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P37E — Enforce boundaries and prove redesign isolation

Prerequisites: P37D. Requirements: REQ-080, REQ-086.
Change scope: architectural checks and controlled theme/layout exercise.

Steps:
1. Add forbidden-import checks matching actual module map.
2. Apply a second theme and one layout variation through presentation files only.
3. Run unchanged controller/API tests and identical semantic interaction tests; record backend/contract hashes.

Pass only when:
- Dependency check catches an intentionally forbidden import in its own negative fixture.
- Backend/API contracts unchanged across exercise; behavior stays equivalent.
- Visual differences reviewed intentionally, not blanket baseline replacement.

Stop / block when:
- Functional failure is suppressed by updating visual snapshots or editing backend logic.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P37F — Write minimal-context design handoff

Prerequisites: P37E. Requirements: REQ-087.
Change scope: design-change guide and evidence links.

Steps:
1. Explain theme/icon/type/spacing/layout/copy edit locations and forbidden change scope.
2. Trace one read and one mutation from UI to domain.
3. Record actual validation commands and a small before/after example.

Pass only when:
- Guide points to existing files and runnable verified commands.
- Theme/layout/new-behavior tiers are distinguishable without oral context.

Stop / block when:
- Guide contains nonexistent paths or claims a test ran without evidence.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P38 — Implement secure management API

Prerequisites: P37. Requirements: REQ-064, REQ-065.
Change scope: rpc adapter and security controls.

Steps:
1. Implement read-only methods before mutations.
2. Apply loopback default, explicit authenticated remote exposure and origin/CSRF checks.
3. Validate mutation input and redact sensitive logs.

Pass only when:
- Auth/access-category tests, malformed params, unknown methods and CSRF/origin tests pass.
- Public relay listener does not expose administrative mutations.

Stop / block when:
- Administrative interface exposed publicly by default or secrets logged.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P39 — Audit licenses and integrate embedded UI

Prerequisites: P38, P37F. Requirements: REQ-066.
Change scope: web assets/license manifest and UI integration.

Steps:
1. Inventory copied source/assets/fonts/dependencies and applicable notices.
2. Resolve distribution obligations before packaging; seek qualified review if uncertain.
3. Embed only audited assets and test actual RPC-driven workflows.

Pass only when:
- Asset/license provenance complete; UI flows pass against implemented API.
- No remote asset dependency silently breaks offline UI.

Stop / block when:
- Reuse/license scope unresolved: do not redistribute assets as cleared.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P40 — Implement persistence and lifecycle controls

Prerequisites: P38. Requirements: REQ-014, REQ-067.
Change scope: configuration/CLI/app lifecycle.

Steps:
1. Version configuration, validate before applying, write atomically and preserve migration backup.
2. Implement controlled bind/config/headless options and OS-specific shutdown.
3. Bound logs and track task/connection/memory metrics.

Pass only when:
- Interrupted write, invalid config, migration and shutdown during active traffic tested.
- Windows andLinux behavior recorded separately.

Stop / block when:
- Migration may overwrite user data without recovery or tasks survive shutdown.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P41 — Validate target packaging

Prerequisites: P39, P40. Requirements: REQ-066, REQ-067, REQ-070.
Change scope: build/package scripts and target evidence.

Steps:
1. Build selected Windows/Linux artifacts using verified toolchains.
2. Record dynamic runtime requirements instead of assuming static linking.
3. Verify embedded UI, config location and clean-machine startup.

Pass only when:
- Each target has artifact hash and startup evidence.
- License notices/source-offer requirements handled per completed audit.

Stop / block when:
- Cross-compilation alone is offered as runtime verification.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P42 — Run parser fuzz and fault campaigns

Prerequisites: P08, P20, P26, P27, P28, P29, P30, P35. Requirements: REQ-025, REQ-027, REQ-056, REQ-070.
Change scope: controlled test harnesses and scoped defect fixes.

Steps:
1. Fuzz supported parsers with bounded resource monitoring and saved seeds.
2. Inject partial reads/writes, disconnects, stalls and malformed nested structures.
3. Fix each defect with a regression; rerun affected gates.

Pass only when:
- Campaign duration/seeds/platform and crashes/hangs recorded.
- No unbounded allocation/task growth for tested failures.

Stop / block when:
- Fuzzer not run or only seed corpus loaded: not a pass.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P43 — Run bounded operational soak

Prerequisites: P36, P41, P42. Requirements: REQ-003, REQ-027, REQ-070, REQ-071.
Change scope: controlled soak and resource report.

Steps:
1. Predeclare duration/load/memory-FD-task thresholds and recovery expectations.
2. Include slow peers, churn, source restart, YP loss and shutdown.
3. Measure retained memory and cleanup, not just absence of crash.

Pass only when:
- Measured run meets predeclared thresholds; logs identify exact tested build/profile.
- Duration stated without translating it into365-day guarantee.

Stop / block when:
- Thresholds changed after observing failure without a new explicit run.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.

## P44 — Publish evidence-based release assessment

Prerequisites: P43. Requirements: REQ-003, REQ-070, REQ-071, REQ-072.
Change scope: status matrix and release assessment only.

Steps:
1. Reconcile every requirement and test row with current build evidence.
2. List remaining unsupported formats/peers/platforms/security items.
3. Prepare small reviewable commits only for verified scoped changes, following repository/user permissions.

Pass only when:
- No required row silently skipped; failures/blockers visible.
- No claim of full ecosystem compatibility or automatic main merge.

Stop / block when:
- Required gates incomplete: release remains blocked even if implementation checklist is full.

Required output: scoped code/tests or analysis artifact, recorded test result, changed-file list and next-ready task.
