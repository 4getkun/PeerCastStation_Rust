# Source audit and correction ledger

Date: 2026-10-24 JST. Root personally reviewed the two supplied documents and selected upstream functions, call sites and supporting definitions. This is not a line-by-line review of either entire repository. No upstream binaries or local Rust port were built/run. Each item is source evidence, not an executed interoperability result.

## Pinned evidence

### A01
IDs are four raw bytes with NUL padding; pcp\n constant, not pcnt.
Source: [PeerCastStation/PeerCastStation.Core/Atom.cs:41](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.Core/Atom.cs#L41)

### A02
Atom lengths/counts and integer leaves are little-endian.
Source: [PeerCastStation/PeerCastStation.Core/Atom.cs:795](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.Core/Atom.cs#L795)

### A03
Async reader limits: 1024 direct children and 1 MiB leaf. Not 16 MB. Instance AtomReader.Read lacks equivalent guards; do not conflate entry points.
Source: [PeerCastStation/PeerCastStation.Core/StreamExtension.cs:178](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.Core/StreamExtension.cs#L178)

### A04
Protocol IDs use conversion distinct from Guid.ToByteArray hashing layout.
Source: [PeerCastStation/PeerCastStation.Core/AtomCollectionExtensions.cs:177](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.Core/AtomCollectionExtensions.cs#L177)

### A05
Address bytes reversed; UTF-8 string leaf constructor appends NUL.
Source: [PeerCastStation/PeerCastStation.Core/Atom.cs:714](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.Core/Atom.cs#L714)

### A06
IPv6 bytes reversed; raw bytes/strings and parent descriptor explicitly encoded.
Source: [core/common/atom.h:62](https://github.com/plonk/peercast-yt/blob/b60f176317406e79a5468ba80da8be1d83bb6126/core/common/atom.h#L62)

### A07
Receiver rejects chan/pkt/data beyond ChanPacket MAX_DATALEN=16384. Not a generic Atom cap.
Source: [core/common/pcp.cpp:318](https://github.com/plonk/peercast-yt/blob/b60f176317406e79a5468ba80da8be1d83bb6126/core/common/pcp.cpp#L318)

### A08
200/503 both undergo ProcessHandshake. Exact method lookup may be absent; inspect ProcessRelayRequest/ProcessHandshake call sites.
Source: [PeerCastStation/PeerCastStation.PCP/PCPSourceStream.cs:234](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPSourceStream.cs#L234)

### A09
Reverse probe uses pcp\n, helo/oleh, matching SID and quit; helper writes version 1.
Source: [PeerCastStation/PeerCastStation.PCP/PCPOutputStream.cs:331](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPOutputStream.cs#L331)

### A10
Probe precedes oleh in handler; inspect OnHandshakePCPHelo call and body.
Source: [PeerCastStation/PeerCastStation.PCP/PCPOutputStream.cs:385](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPOutputStream.cs#L385)

### A11
Quit mapping includes 4000; subset 1000..1011 is incomplete.
Source: [PeerCastStation/PeerCastStation.PCP/PCPOutputStream.cs:456](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPOutputStream.cs#L456)

### A12
Multiple error bases: 1000/2000/3000/4000/5000; common reasons 1..11.
Source: [core/common/pcp.h:156](https://github.com/plonk/peercast-yt/blob/b60f176317406e79a5468ba80da8be1d83bb6126/core/common/pcp.h#L156)

### A13
Network profile maps IPv4->1, IPv6->100; not the same enum values used in ID hashing.
Source: [PeerCastStation/PeerCastStation.PCP/PCPVersion.cs:16](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPVersion.cs#L16)

### A14
Server validates channel family, PCP header and socket family together.
Source: [PeerCastStation/PeerCastStation.PCP/PCPOutputStream.cs:938](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPOutputStream.cs#L938)

### A15
handshakeFetch chooses PCP header using connection family. Locate PCX_HS_PCP if spacing differs.
Source: [core/common/channel.cpp:444](https://github.com/plonk/peercast-yt/blob/b60f176317406e79a5468ba80da8be1d83bb6126/core/common/channel.cpp#L444)

### A16
Station SHA512/MD5 algorithm uses optional network prefix and BinaryWriter strings.
Source: [PeerCastStation/PeerCastStation.Core/BroadcastChannel.cs:52](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.Core/BroadcastChannel.cs#L52)

### A17
YT derives ID using broadcastID + encode(name,genre,bitrate), or random setting. Not Station algorithm.
Source: [core/common/servhs.cpp:2440](https://github.com/plonk/peercast-yt/blob/b60f176317406e79a5468ba80da8be1d83bb6126/core/common/servhs.cpp#L2440)

### A18
XOR-based encode algorithm; inspect encode body, not a cryptographic identity guarantee.
Source: [core/common/gnuid.cpp:24](https://github.com/plonk/peercast-yt/blob/b60f176317406e79a5468ba80da8be1d83bb6126/core/common/gnuid.cpp#L24)

### A19
3-second content retention; no 8 MiB byte cap established in this collection.
Source: [PeerCastStation/PeerCastStation.Core/Content.cs:123](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.Core/Content.cs#L123)

### A20
5-second newest-minus-oldest timestamp queue guard; not universal socket timeout.
Source: [PeerCastStation/PeerCastStation.PCP/PCPOutputStream.cs:240](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPOutputStream.cs#L240)

### A21
180000 ms node freshness used in HostsView and AddNode/AddSourceNode. Filtering is not always physical removal.
Source: [PeerCastStation/PeerCastStation.Core/Channel.cs:58](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.Core/Channel.cs#L58)

### A22
Separate 180000 ms failed-source exclusion; not the only meaning of 180 seconds.
Source: [PeerCastStation/PeerCastStation.PCP/PCPSourceStream.cs:856](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPSourceStream.cs#L856)

### A23
Station weighted/random source selection, not strict minimum-hop hierarchy.
Source: [PeerCastStation/PeerCastStation.PCP/PCPSourceStream.cs:921](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPSourceStream.cs#L921)

### A24
Caller-conditioned matching and minimum hops. ChanHitSearch::init sets waitDelay=0.
Source: [core/common/chanhit.cpp:608](https://github.com/plonk/peercast-yt/blob/b60f176317406e79a5468ba80da8be1d83bb6126/core/common/chanhit.cpp#L608)

### A25
Forwarding decrements TTL, increments hops; checks destination/group/from and TTL>1.
Source: [PeerCastStation/PeerCastStation.PCP/PCPSourceStream.cs:658](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPSourceStream.cs#L658)

### A26
Writes 9-byte FLV header PLUS PreviousTagSize0 then metadata/audio/video configuration.
Source: [PeerCastStation/PeerCastStation.FLV/FLVContentBuffer.cs:304](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.FLV/FLVContentBuffer.cs#L304)

### A27
PAT/PMT header cache with stream reset when header changes. Not proof of codec random-access readiness.
Source: [PeerCastStation/PeerCastStation.TS/TSContentReader.cs:49](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.TS/TSContentReader.cs#L49)

### A28
ASF object prefix 24 bytes; top-level payload count/reserved six bytes. Reader also handles MMS framing.
Source: [PeerCastStation/PeerCastStation.ASF/ASFContentReader.cs:220](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.ASF/ASFContentReader.cs#L220)

### A29
Direct playback suppresses cont packets until first non-cont. Not universal H.264 parsing.
Source: [core/common/servent.cpp:1805](https://github.com/plonk/peercast-yt/blob/b60f176317406e79a5468ba80da8be1d83bb6126/core/common/servent.cpp#L1805)

### A30
First fragment of keyframe may clear cont; later fragments set cont. Distinguish frame and packet boundary.
Source: [core/common/flv.cpp:245](https://github.com/plonk/peercast-yt/blob/b60f176317406e79a5468ba80da8be1d83bb6126/core/common/flv.cpp#L245)

### A31
Dedicated URI-bound TCP listener; Handshake accepts POST. No shared /push/{id} route established here.
Source: [PeerCastStation/PeerCastStation.HTTP/HTTPPushSourceStream.cs:118](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.HTTP/HTTPPushSourceStream.cs#L118)

### A32
Chunked normal playback branch differs from MMS-specific branch; /stream and /pls mappings present.
Source: [PeerCastStation/PeerCastStation.HTTP/HTTPOutputStream.cs:339](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.HTTP/HTTPOutputStream.cs#L339)

### A33
30-second pool updates; CheckConnection error retry 1000 ms * 3/2 capped 30000, terminal stop/banned/quit paths.
Source: [PeerCastStation/PeerCastStation.PCP/PCPYellowPageClient.cs:670](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.PCP/PCPYellowPageClient.cs#L670)

### A34
RPC method inventory exists; UI compatibility requires full parameter/result/access contract, not five names.
Source: [PeerCastStation/PeerCastStation.UI.HTTP/APIHost.cs:169](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.UI.HTTP/APIHost.cs#L169)

### A35
C# source header says GPL v3 or later. Repository LICENSE.txt also retrieved. Complete asset/dependency clearance not performed.
Source: [PeerCastStation/PeerCastStation.Core/Content.cs:5](https://github.com/kumaryu/peercaststation/blob/390c4e847289c90a77dbe40cc04b7bc1cfbddf17/PeerCastStation/PeerCastStation.Core/Content.cs#L5)

### A36
C++ source header says GPL v2 or later. core/LICENSE retrieved. No permission to discard notices inferred.
Source: [core/common/atom.h:9](https://github.com/plonk/peercast-yt/blob/b60f176317406e79a5468ba80da8be1d83bb6126/core/common/atom.h#L9)

## Important policy changes, not upstream discoveries

- 8 MiB body budget, depth 32, total 4096 nodes/tree and 4 MiB encoded-tree budget are proposed port safety defaults, not universal PCP requirements. Validate against representative peer traffic before release.
- Changing generic leaf maximum from the original 16 MB to 1 MiB aligns the reviewed C# async/static readers, but requires impact tests and is a deliberate semantic correction.
- Actor handles, monotonic timers, global budgets, administrative security, test gates and dependency direction fill missing implementation contracts. They are not claims that upstream already implements them.
- Station-style new ChannelID generation is the default target profile; existing peer ChannelIDs remain opaque. YT-style generation is separate.
- No public network testing, legal clearance, Rust code changes or verified local toolchain are claimed.

## Pending source/fixture work

1. Complete Atom field schema/cardinality, extension compatibility, position rollover and raw-control IPv6 probe fixtures.
2. Compare real reference builds for all handshake/error branches and collect independent serialization/ChannelID vectors.
3. Audit each format parser end-to-end, directory encoding/columns and full RPC/UI contract.
4. Inspect the actual Rust repository before stating implementation status.
5. Complete asset/dependency licensing and target-specific packaging review.

## Review correction retained for transparency

An interim chat update incorrectly said180 seconds was only failed-source exclusion, not node freshness. Subsequent inspection of Channel.NodeLimit, HostsView and AddNode/AddSourceNode established both independent uses. That interim conclusion was corrected in chat and is NOT the conclusion of this deliverable.
