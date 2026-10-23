# PeerCastStation Rust Port — Implementation Contract

Version: 1.4.0-review. Date: 2026-10-24 JST / 2026-10-23 UTC.
Status: source-reviewed working specification; NOT an implementation or interoperability certification.
Language: English. Read this document first, then IMPLEMENTATION_PLAN.md, SOURCE_AUDIT.md and WORKLOG_AND_DECISIONS.md.

## 1. Authority, scope, and terminology

The goal remains a native, memory-safe Rust PeerCast servent with relay, broadcast ingest, direct playback, Yellow Page support, management API, and embedded web UI. Windows and Linux are initial target platforms; other targets require their own build and runtime evidence. Compatibility with PeerCastStation and peercast-yt is the primary target. PeerCast-IM, specific YPs, and named players remain separate test targets, not compatibility inferred from those two repositories.

This revision supersedes the attached v1.3 specification as an implementation guide. It deliberately corrects technical claims; it is NOT a semantically identical translation. Unchanged originals are retained in originals/. No upstream or local Rust application was modified or executed during this review. A historical claim that Rust code already implements a feature is not evidence that it does.

Evidence categories:
- OBSERVED: behavior read in the pinned source paths listed in SOURCE_AUDIT.md. Not a runtime test result.
- REQUIREMENT: intended behavior of this Rust port, identified below by stable IDs.
- PROPOSED POLICY: a local default or design choice, not a universal PCP wire rule. Record changes in an ADR before implementation.
- OPEN: unresolved detail that needs a source trace, fixture, or product decision. Do not silently fill it in.

Reference snapshots:
- C#: kumaryu/peercaststation @ 390c4e847289c90a77dbe40cc04b7bc1cfbddf17.
- C++: plonk/peercast-yt @ b60f176317406e79a5468ba80da8be1d83bb6126.

The snapshots are current default-branch commits retrieved for this review, not claims about a particular released binary (including “YT50”). Pin actual peer binaries independently for interoperability tests. Source paths in the audit are repository-relative.

## 2. Implementation rules for an incoming agent

REQ-001: Inspect the existing Rust repository, its applicable instructions, Git status, toolchain, tests, and active work before changing files. The reported location C:\Github\PeerCastStation_Rust and toolchain 1.98.1 are historical input, not verified environment state. Preserve unrelated edits. Do not initialize over an existing workspace.

REQ-002: Use Rust Edition 2021 as the baseline inherited from the work log. Resolve the original “2021/2024” ambiguity explicitly; do not upgrade editions or declare an MSRV from an unverified toolchain string. Pin dependencies and record the actual compiler/platform used.

REQ-003: Treat source compatibility, API compatibility, wire compatibility, media decoding, and operational reliability as separate acceptance dimensions. Never report “100% compatible,” “all edge cases,” or guaranteed unlimited uptime from a source review, unit tests, or bounded buffers.

REQ-004: Keep user-selected architecture (Tokio, shared immutable packet payloads, explicit lifecycle states, embedded UI, headless operation) while applying the corrections here. Security/resource additions are port design policies. Do not copy an upstream defect merely to mimic its implementation.

## 3. Architecture and ownership

Proposed dependency DAG, where A -> B means A depends on B:
- core -> standard library and selected small utility dependencies only; no networking adapters or servent.
- format -> core.
- pcp -> core.
- http -> core, format.
- yp -> core, pcp.
- servent -> core, pcp, format, http, yp.
- rpc -> core and a narrow servent management handle.
- web -> embedded static assets; no dependency back into rpc/servent.
- app -> servent, rpc, web; composes listeners, configuration and shutdown.

REQ-010: Define adapter-facing commands/events/handles in core or another explicitly introduced lower-level contract crate. Neither pcp, http nor yp may import the concrete servent manager. Keep dependency direction acyclic. Do not make all nine crates contain placeholder abstractions before the first tested vertical slice.

REQ-011: A channel actor owns channel state, its epoch/generation, header snapshot, content cache and subscriber registry. A connection actor owns socket lifecycle and protocol state. Expose cloneable command handles and immutable status snapshots; the running actor is not the public handle.

REQ-012: Do not retain the original SourceStream API in which both run(&mut self) and post(&mut self) require simultaneous exclusive access. A worker consumes itself (or runs under one exclusive owner); a separate handle posts bounded commands. Avoid a lock held across network awaits. Sync is not required merely because a worker runs on Tokio.

REQ-013: Fan-out must not await a slow subscriber in the channel actor. Use bounded per-subscriber admission (try-send or equivalent), an explicit lag/overflow response and per-connection writers. Shared Bytes cloning avoids payload copying, but N-recipient dispatch still costs O(N); reference metadata, serialization and kernel writes are not free.

REQ-014: Give all spawned tasks an owner, cancellation path and join policy. Use monotonic time for deadlines, retry cooldowns and queue age. A generation token prevents stale worker events from modifying a replacement connection/channel. Enums alone do not prevent re-entrancy or races.

## 4. Wire primitives

REQ-020: An Atom is four raw ID bytes followed by a little-endian u32 descriptor. Bit 31 set means a parent with the low 31 bits specifying DIRECT child count; otherwise it is a leaf with that many payload bytes. This is not a network-big-endian bitfield. Preserve the distinction between an empty parent and an empty leaf.

REQ-021: IDs shorter than four bytes are NUL-padded (e.g. sid\0); pcp\n is exactly four bytes, with a newline. Do not use “pcnt” as the connection preamble. Retain child order and duplicate IDs in the generic codec; a map would discard information used by repeated fields. Semantic handlers define cardinality rules for their fields.

REQ-022: Integer leaf payloads use their defined width (u8/u16/u32) and little-endian encoding. Validate width before reading. Strings emitted in the Station-compatible profile use UTF-8 plus terminating NUL, included in leaf length. Preserve raw unknown leaves. Specify invalid-string behavior at semantic boundaries instead of lossy-converting arbitrary media bytes.

REQ-023: Protocol ID bytes and .NET Guid.ToByteArray bytes are DIFFERENT representations. On the audited little-endian C# path, protocol IDs use the canonical hex-order 16 bytes via IDToByteArray. .NET byte layout reverses the first 4-, 2-, and 2-byte groups relative to that order. Use distinct conversion functions and golden tests. Do not apply the channel-hash input conversion to every sid/cid leaf.

REQ-024: C# IP Atom encoding reverses the complete 4- or 16-byte network-order address. C++ explicitly reverses IPv6 bytes and uses its integer IPv4 representation. Test both families against independent fixtures; do not write native socket-address memory directly.

REQ-025: The parser must support arbitrary TCP fragmentation and multiple Atoms per read, checked arithmetic, clean EOF versus truncated input, and bounded unknown-Atom skipping. Validate lengths before allocation. Enforce cumulative bytes, total nodes and depth as well as individual leaf/child limits. Cancellation must release partially buffered trees. A recursive parser without a depth budget is unacceptable.

### Resource policy (not one universal upstream rule)

| Resource | Reviewed baseline | Classification |
|---|---:|---|
| Direct children per parent | 1,024 | Observed C# static/async reader guard; retain as policy |
| Generic leaf payload | 1 MiB = 1,048,576 bytes | Observed C# static/async guard; replaces unsupported 16 MB assertion |
| chan/pkt/data payload | 16,384 bytes | Observed C++ receiver hard limit; validate in packet context |
| Nesting depth | 32 | Proposed safety default |
| Total nodes in one decoded tree | 4,096 | Proposed safety default |
| Encoded bytes in one decoded tree | 4 MiB | Proposed safety default |
| Channel body cache | 8 MiB AND 3 seconds | 8 MiB proposed; C# ContentCollection has a 3-second time window |
| Queue lag | 5 seconds | C# observed newest-minus-oldest enqueue timestamp guard |
| Node freshness | 180 seconds | C# Channel HostsView and insertion filtering |
| Failed source exclusion | 180 seconds | Separate C# PCPSourceStream ignored-URI cooldown |

REQ-026: The media-data limit is not a 16 KiB cap on every Atom or whole parent. Split outbound body data at safe packet boundaries, preserve positions and continuation flags; never truncate it. Oversized required headers need an explicitly tested strategy or a clear unsupported-stream error, not silent corruption.

REQ-027: Treat cache byte/time limits as simultaneous eviction constraints. Body cache, retained headers, partial container buffers, subscriber queues, directory responses, logs, pending connections and host/cooldown maps all need independent limits plus a process-wide budget. Shared Bytes may keep larger backing allocations alive: account for retained allocation cost, not just slice length. New default numerical budgets beyond the table are OPEN and must be defined before public exposure.

REQ-028: A bounded message count does not bound bytes or latency. On lag/byte overflow, isolate the slow connection and report its reason. For the audited Station-style PCP output overflow path, use the skip reason where applicable. Do not reuse that error for unrelated parser failures.

## 5. Connection profiles and state machines

REQ-030: Distinguish HTTP PCP relay, raw PCP control/YP, reverse reachability probe, direct HTTP playback and HTTP ingest. There is no single HTTP bootstrap for every PCP connection, and this is not HTTP Upgrade/101 or WebSocket negotiation.

### HTTP relay client/server

REQ-031: Client sends GET /channel/{32-hex-channel-id}, supported HTTP version, Host/User-Agent and PCP headers, ending headers with CRLF CRLF. Preserve bytes read beyond the HTTP header terminator for the Atom decoder. x-peercast-pos is a reconnect/content cursor, not a wall-clock timestamp. Validate numeric range and wire rollover separately from internal counters.

REQ-032: On 200 OR 503, proceed through helo/oleh before the next branch. On 404, stop as off-air/not found; do not parse an Atom stream. Define handling of other statuses, malformed headers, timeout and cancellation.

Successful path:
1. TCP -> HTTP response 200 -> send/read helo.
2. Receiver validates identity/version; if a reverse ping was requested, it may complete the probe BEFORE sending oleh (observed Station ordering).
3. Receiver emits oleh, then the observed Station ok Atom with integer 1.
4. Continue with chan metadata/header/data and control messages. Preserve legal control interleaving; do not require every next Atom to be media.

Full-relay path:
1. TCP -> HTTP response 503 -> helo/oleh exchange.
2. Receive candidate host Atoms and quit (Station unavailable mapping 1003).
3. Close cleanly, validate candidates, then select another source subject to retry bounds.

REQ-033: A 503 is not an immediate HTTP-only close and not an immediate host list before handshake. Test both serving and consuming this path against each pinned peer. Keep wire-code handling separate from local reconnect decisions.

REQ-034: Negotiate PCP version 1 for IPv4 and 100 for IPv6. C++ derives the relay header from its connection family; Station sends its channel network version and its server checks channel network, header and remote socket family together. Represent network profile explicitly, reject contradictory combinations, and test IPv4-mapped addresses; do not infer full dual-stack compatibility merely from one header.

### Raw control and reverse probe

REQ-035: Raw connection preamble is the pcp\n Atom containing a protocol-version integer, followed by the relevant helo/oleh exchange. It is not inserted before the HTTP relay request or redundantly into its binary phase. YP/control and probe state machines must be independently tested.

REQ-036: A reachability probe connects back to the observed peer address and announced ping port, exchanges pcp\n + helo/oleh, checks the returned SessionID, then closes with quit. It tests reachability; it does NOT configure NAT/UPnP or authenticate a trusted person. Station's audited ping helper writes version 1; C++ has distinct IPv4/IPv6 paths. IPv6 probe interoperability is OPEN until tested, not silently assumed.

REQ-037: Use explicit connect, header-read, handshake, write and shutdown deadlines. Station uses an 18-second relay handshake timeout; a nearby old 5-second comment is not authoritative. Socket sync timeout fields are not proof that all asynchronous operations have deadlines. Bound probe concurrency and restrict callbacks to the observed remote address; do not permit arbitrary host-driven scanning.

### Quit values

REQ-038: Preserve the complete raw integer even when unknown. The common quit base is 1000 with reasons 0..11: normal, skip, already connected, unavailable, loopback, not identified, bad response, bad agent, off air, shutdown, no root, banned. This is a useful subset, not the complete range.

Other observed error bases are BCST=2000, READ=3000, WRITE=4000 and GENERAL=5000. Station can send QUIT + READ = 4000. Do not reject all values outside 1000..1011 or subtract 1000 and assume a known enum. Context-specific mappings need fixtures. Identity/version checks are protocol checks, not cryptographic authentication.

## 6. IDs, host discovery and routing

REQ-040: Accept externally supplied ChannelIDs as opaque validated 16-byte identifiers; never recompute a relayed channel ID from display metadata.

Station-compatible NEW broadcast identity profile:
1. Hash .NET-layout BCID bytes using SHA-512.
2. If network is IPv4 (enum value 0), append NO network prefix. If IPv6 (enum value 1), prefix little-endian i32 value 1.
3. Append the 64 hash bytes without a length prefix.
4. Append name, genre, source in that order using BinaryWriter string encoding: 7-bit encoded UTF-8 BYTE length followed by UTF-8 bytes, no NUL terminator. This is not Atom string encoding. Do not normalize text silently.
5. MD5 the concatenated bytes. Interpret the digest as new Guid(byte[]) and convert separately to textual/protocol representation.

REQ-041: Test empty and Japanese strings, lengths crossing 127/128 bytes, both networks and non-symmetric BCIDs. The C++ broadcast path is DIFFERENT: it starts from broadcastID and calls GnuID::encode with name, genre and bitrate, or generates a random ID when configured. Therefore no universal SHA512/MD5 identity algorithm is claimed. C++-style generation is an optional separate compatibility profile, not required to relay its channels.

REQ-042: Keep live host freshness, failed-endpoint cooldown and reconnect backoff separate. Station's 180-second node age check is in Channel; its 180-second ignored-source setting is in PCPSourceStream. A view filtering old nodes is not necessarily immediate physical deletion. The Rust port should perform bounded periodic cleanup and insertion cleanup using monotonic time.

REQ-043: Parent selection is local policy, not a common wire-order rule. Station uses weighted randomized scoring including locality, receiving/relay-full flags, hops and relay count. C++ pickHits uses caller-configured filters and minimizes hops among eligible hits; LAN matching depends on matchHost and rhost fields. The default ChanHitSearch waitDelay is 0, not universally 10 seconds. Start with an explicitly named Station-compatible selection policy unless an ADR chooses otherwise; test deterministic seeded fixtures. Self/invalid endpoint exclusion, attempt budgets and loop avoidance apply regardless of score.

REQ-044: For eligible bcst forwarding, decrement TTL and INCREMENT hops; do not “decrement hops.” Respect destination, group and origin rules. In the observed Station path forwarding requires TTL > 1 and destination not local. Prevent u8 overflow, repeated forwarding storms and reflection loops; document any extra bounded deduplication policy without falsely attributing it upstream.

## 7. Media and playback

REQ-050: Separate transport packetization, container framing, codec initialization and safe-start markers. A PCP continuation flag is a packet-level hint, not a universal H.264 IDR detector. Preserve incoming flags; only synthesize them when the selected parser can establish the relevant boundary.

REQ-051: Store a bounded header snapshot with epoch, position and codec/container identity. On restart or changed initialization, invalidate incompatible cached body data and send a consistent new header/body sequence. Track transport position independently from media timestamp and local elapsed time. Define gap, overlap, rollover and truncated-header behavior in tests.

REQ-052: FLV output requires the file header and the four-byte PreviousTagSize0 (13 initial bytes for the standard 9-byte DataOffset), then applicable metadata and codec sequence headers. The audited Station code also handles extended tags; the first Rust codec subset must be explicit. Do not claim AVC/AAC support implies support for every FLV codec.

REQ-053: TS cache behavior references PAT/PMT packets, but this alone does not ensure codec initialization or a random-access video frame. Parse/reassemble bounded PSI sections, handle version changes, continuity/discontinuity and malformed packets, and test codec parameter availability. Do not promise artifact-free immediate playback from PAT/PMT alone.

REQ-054: ASF parsing follows GUID + u64 object size and child boundaries, with a 24-byte generic prefix and six additional top-level header bytes before children. Do not use a guessed “50-byte minimum.” Distinguish ASF objects from MMS-framed transport. The audited Station reader forwards complete MMS header frames; a raw ASF-only implementation is not automatically equivalent.

REQ-055: C++ direct playback can skip packets marked cont until a non-cont packet is available, and selects a recent non-cont cache position. Use container-aware safe-start logic and a bounded wait. Audio-only or formats without trustworthy flags must not wait forever for an H.264 keyframe. Re-enter startup handling after an epoch change when required by the selected format; record this as port policy rather than assuming identical upstream logic.

REQ-056: Target FLV, TS, ASF/MMS, MKV and Ogg incrementally. Each needs valid/truncated/oversized/change-of-header fixtures and an actual player test before being listed as supported. MP3/audio-only handling also needs an explicit support decision. No codec transcoding is implied.

## 8. HTTP, YP, RPC and packaging

REQ-060: Implement /stream and /pls routing with validated ChannelIDs, explicit MIME types, playlist escaping and on-demand channel lifecycle. HTTP framing depends on version/media profile; do not apply chunked encoding unconditionally to HTTP/1.0 or MMS-specific responses. Backpressure and client cancellation must release subscriber resources.

REQ-061: Station HTTPPushSourceStream opens a listener at its configured source URI and accepts POST; the reviewed code does not establish a shared POST /push/{channel_id} route. That route is an OPTIONAL Rust API proposal requiring authorization, channel ownership, lifecycle and binding semantics. OBS compatibility also needs its actual output protocol/format tested; HTTP POST alone does not prove compatibility with an OBS workflow.

REQ-062: Keep PCP channel announcement and HTTP directory listing as distinct YP operations. Station has a 30-second announcement update loop and retry delays starting at 1000 ms, multiplied by 3/2 with integer arithmetic and capped at 30000 ms. These are reference implementation policy, not universal timing guarantees. Preserve cancellation, terminal banned/server-quit behavior, reference ownership and network-family isolation. Additional jitter is a documented port policy.

REQ-063: Bound index.txt response size, lines and fields; test encoding, escaping, optional columns and malformed rows against selected real directory fixtures. The precise directory contract remains OPEN. Never automatically announce to public YPs during tests without an explicitly selected authorized test destination.

REQ-064: JSON-RPC method names alone are insufficient for UI reuse. Build a contract inventory from APIHost/JSONRPC and actual UI calls: parameters, defaults, results, errors, access categories and mutations. Initial target names include getVersionInfo, getStatus, getChannels, getSettings and setSettings; unsupported methods return defined errors and are not silently stubbed as success. Notifications/batches/auth behavior require explicit tests.

REQ-065: Separate public relay/play access from administrative RPC and ingest. Default administrative bindings to loopback; require deliberate authenticated remote exposure, origin/CSRF controls for browser mutations, input/path validation, redacted secrets and request limits. SSRF protections must not indiscriminately block valid user-selected LAN relays; define allowed destinations per operation.

REQ-066: Embedded UI and a single distributable executable remain goals. Static linking on every OS is not assumed. Audit licenses of code, copied UI, assets and dependencies before redistribution; preserve notices and determine applicable source-distribution obligations. Source reuse is not made license-free by a Rust rewrite. This review records license evidence, not legal clearance.

REQ-067: Provide versioned configuration, atomic persistence, validation, safe migration/backups, graceful shutdown and bounded structured logs/metrics. SIGINT/SIGTERM apply where supported; Windows shutdown/control behavior needs its own implementation. CLI --port/--config/--headless are intended interfaces, not already existing commands.

## 9. Incremental delivery and acceptance

All phases below are NOT VERIFIED in the Rust project during this review.

1. Inventory existing repository and instructions; establish actual toolchain and baseline tests. Record blockers and preserve edits.
2. Core codec/IDs/resource tests: independent byte fixtures, malformed inputs, fragmentation at every boundary, unknown fields, cumulative/depth limits and memory accounting. The included reference vectors are review aids, not upstream-generated interoperability captures.
3. First vertical slice: one supported media fixture through a Rust relay, with both directions against separately pinned Station and YT peers. Exercise 200, 503-with-handshake, 404, reverse probe, disconnect and reconnect. Begin IPv4; add IPv6 with explicit network tests.
4. Direct playback and first ingest/container path: header/body correctness, audio-only, late join, slow reader, cancellation and changed initialization. Extend formats one at a time.
5. Host routing/YP lifecycle: freshness versus cooldown, hop/TTL changes, retry termination, bounded directory parsing; local controlled YP fixtures first.
6. Management/UI/configuration/package integration with authenticated mutation tests and license inventory.
7. Stress, fault injection, fuzzing and time-bounded soak runs. Record duration, throughput, peer count, memory/FD/task trends, platform and failure thresholds. Do not replace measured results with an uptime guarantee.

REQ-070: For every test record commit, dirty-state digest, OS, compiler, dependencies, peer build SHA/version, profile (IPv4/IPv6), direction, input fixture hash, command, result and log path. Status is passed, failed, not run or blocked. Reused evidence requires unchanged relevant code/dependencies/environment and a documented impact analysis.

REQ-071: The acceptance matrix must cover Rust->Station and Station->Rust, Rust->YT and YT->Rust, serving and consuming 503, both address families, restart/rollover/header change, slow peers and malformed input. Player decoding and YP compatibility are separate rows. A phase cannot be “done” while its required rows are unrun.

REQ-072: Before implementing an OPEN contract, trace the pinned handler and caller or obtain a fixture, then update the audit and ADR. Do not invent an endpoint, timeout, codec guarantee or universal algorithm to make a checklist look complete.

## 10. Task-level execution plan

IMPLEMENTATION_PLAN.md decomposes the phases into 50 dependency-linked tasks. implementation-plan.json is the machine-readable counterpart. Each task defines prerequisites, requirement IDs, allowed change scope, steps, acceptance conditions and stop conditions. All statuses are planning-only, not_started. The specification controls semantics; the runbook controls execution order. If they conflict, stop and reconcile them rather than silently choosing one.

## 11. Presentation/function separation (user-requested)

Goal: changing visual design should not require editing relay/media logic, RPC handlers, wire contracts or application-use-case rules. Separation is a dependency contract, not merely moving CSS to another file. Reuse the existing frontend framework if suitable; this requirement does not mandate a framework migration or a generic design-system product.

### Frontend dependency direction

- Design tokens/themes: colors, typography, spacing, borders, radii, elevation, icon mappings and motion preferences. No network or domain decisions.
- Presentational components: render typed props/view models and emit semantic user events. Depend on tokens and shared accessibility primitives, never directly on JSON-RPC, sockets or servent state.
- Screens/layouts: compose presentational components and bind view models to semantic events. A layout may change placement but cannot redefine command behavior or access checks.
- Application/presentation controllers: own loading/error/empty/selected/pending UI states, map domain data to view models and invoke use cases. No hardcoded styling or DOM-dependent domain rules.
- API adapter: one boundary for JSON-RPC transport, request IDs, schema validation, auth/session handling, error normalization and cancellation. Components do not construct RPC method strings.
- Backend: RPC handlers call management/use-case handles; servent/core own permissions, channel lifecycle, routing, media and persistence. Backend code does not know CSS classes, component names, selected tabs or theme tokens.

Allowed flow: view event -> controller/use case -> API adapter -> RPC handler -> management handle -> domain actor. Result/status flows back through typed view models. Dependencies point inward toward contracts, not from backend/domain toward UI. The Rust web crate packages assets; it does not become the place for relay business logic.

REQ-080: Establish a frontend directory/module map appropriate to the existing project (e.g. design/, components/, screens/, presentation/, api/, contracts/). Treat these as roles, not mandatory names. Document allowed imports and enforce forbidden edges with a dependency/import check or equivalent automated architectural test. Do not add cross-layer bypasses for convenience.

REQ-081: Define one typed API boundary from the verified RPC contract and a separate stable view-model/event interface for each screen. The UI may format a bitrate or date but must not infer a channel's authoritative lifecycle from color/text/CSS. Backend remains the authority for permission and operational validation. Hidden/disabled buttons are not security controls.

REQ-082: Make shared styling themeable through semantic tokens (surface, text, error, spacing, focus, etc.), not scattered inline literals. Keep component-specific structural styling local and documented. Separate text/localization from layout where practical. Preserve responsive layout, keyboard navigation, focus, labels, readable contrast and reduced-motion behavior when changing themes.

REQ-083: Presentational components must render from fixture props without a running PeerCast daemon. Provide component/screen fixtures for normal, empty, loading, partial data, forbidden, disconnected, stale and failed states. A mock adapter is test/dev-only and cannot silently replace a failed real backend in a production build.

REQ-084: Controllers use explicit user intent (start/stop/update settings), not DOM structure or display labels, to dispatch commands. Normalize API failures; prevent duplicate pending mutations; cancel obsolete reads; reject stale responses. Retry only when operation semantics permit it, and never replay a non-idempotent mutation blindly after an uncertain response. Presentation changes must not alter these rules.

REQ-085: Upstream UI reuse is an option, not an instruction to embed coupled code unchanged. Inventory its network calls, inline handlers, styling and dependencies. Wrap or extract the API/controller boundary incrementally and preserve externally observable behavior with tests. Do not rewrite every screen solely for architectural purity; isolate the frequently changed design surfaces first, then migrate behind stable contracts.

REQ-086: Prove separation with a theme/layout change exercise: change tokens and one screen arrangement using only approved presentation files, then run unchanged controller/API/domain tests and equivalent user-flow tests. Record that backend and API-contract files are unchanged. Update visual expectations only after explicit review of intentional design changes; never regenerate baselines to hide functional failures.

REQ-087: Record a design-change guide identifying where to change theme, icon, typography, spacing, layout and copy; list files that should not change for each. Include UI->controller->API->backend tracing for one read flow and one mutation. The guide must be usable by an agent with minimal context and include commands discovered from the actual project, not invented scripts.

Design change tiers:
1. Theme/token-only: presentation files and visual/accessibility checks; no backend change.
2. Layout/component-only: same semantic events/view models; add responsive and interaction checks.
3. New behavior/data: requires a separate contract/use-case change and relevant integration tests. Do not disguise it as a cosmetic change.
