# PeerCastStation Rust Port — Continuity and Decisions

Updated: 2026-10-24 JST (2026-10-23 UTC). Revision: 1.4.0-review.
Canonical working contract: SPECIFICATION.md. Evidence: SOURCE_AUDIT.md and source-evidence.json.

## Current verified state

- Two supplied documents were read completely and preserved unchanged in originals/.
- Public C# and C++ upstream source snapshots were pinned and selected relevant implementation functions/callers inspected by dot personally.
- The specification was reorganized and corrected. No Rust implementation, repository initialization, build, peer interoperability, player decoding or soak run was performed in this review.
- Historical local paths and rustc/cargo1.98.1 are REPORTED, not verified. Do not label the project empty, implemented, approved or production-ready based on these attachments.
- Existing user intent remains a complete port, but delivery is incremental and evidence-gated. Open contracts are not permission to guess.

## Historical input, retained without retroactive validation

The original work log records setup, a C# analysis, three review stages and sixteen decisions on2026-10-24. Those entries are preserved verbatim in originals/WORKLOG_AND_DECISIONS.md, including its claimed local clones and toolchain. This review does not verify that those activities occurred. Statements such as “our deserializer enforces” and “Rust replicates” conflicted with unchecked implementation tasks; they are not promoted into completed work.

## ADR disposition

Status vocabulary: retained-intent, amended, superseded, open. These describe document decisions, not code implementation or new user approvals.

| ID | Disposition | Current decision and reason |
|---|---|---|
|0001|amended|Keep nine named crates, add acyclic dependency direction and lower-level adapter contracts. Build a vertical slice before broad scaffolding. REQ-010.|
|0002|retained-intent|Tokio multi-thread runtime and bounded asynchronous I/O; actual dependency/toolchain versions unverified. REQ-002,014.|
|0003|amended|Bytes shares immutable payloads; O(1) clone is not O(1) N-peer fan-out or zero allocation everywhere. REQ-013,027.|
|0004|amended|Enums plus single-owner actors, cancellation, generations and bounded transitions. Enums alone do not guarantee re-entrancy safety. REQ-011..014.|
|0005|amended|Separate observed1024 children/1 MiB generic leaf/16384 packet data/3s retention/5s queue guard from proposed8 MiB and cumulative/depth budgets. REQ-025..028.|
|0006|amended|Embedded UI remains intended, subject to API contract and license inventory. Single executable does not mean all-target static linking. REQ-064..067.|
|0007|amended|HTTP relay uses helo/oleh after200 AND503; raw control/YP is separate. REQ-030..033.|
|0008|amended|Probe preamble is pcp\\n, not pcnt; verify observed peer address/SID and order. No automatic port-forward configuration implied. IPv6 probe differences remain open. REQ-035..037.|
|0009|amended|Per-format bounded initialization cache with epochs; no immediate-decoding guarantee. FLV PreviousTagSize0, TS limits and MMS/ASF distinction explicit. REQ-050..056.|
|0010|amended|SHA512/MD5 is Station NEW-channel profile only; .NET layout and string length encoding specified. YT generation differs. REQ-040..041.|
|0011|amended|Station30s updates and1.5x retry are reference policy, not universal YP protocol; stop/banned/cancel behavior explicit. REQ-062..063.|
|0012|amended|180s host freshness DOES exist in Channel; separate180s failed-source cooldown also exists. TTL decreases while hops increase. REQ-042,044.|
|0013|amended|IPv4 PCP1/IPv6 PCP100 retained, with channel-network/socket-family consistency and probe exceptions/tests. REQ-034..036.|
|0014|amended|Packet continuation is not universal IDR detection. Format-aware bounded safe-start; audio-only behavior required. REQ-050,055.|
|0015|superseded|Replace closed1000..1011 enum with raw-code preservation and contextual mappings including4000. REQ-038.|
|0016|superseded|Replace asserted universal strict hierarchy with explicitly named local selection policy. Station weighted-random differs from YT caller-dependent filtering. REQ-043.|
|0017|new proposed policy|Split running worker from command handle; bound fan-out and task lifecycle. REQ-011..014.|
|0018|new proposed policy|Layered parser/process budgets and security gates; thresholds beyond the specification table remain open before deployment. REQ-025..028,065.|
|0020|user-requested architecture|Separate themes/components, presentation controllers, API adapter and backend; prove a redesign without changing functional contracts. REQ-080..087.|
|0019|new verification rule|No completed status without artifact-linked tests; separate peers, directions, address families and media/API scope. REQ-070..072.|

## Review execution log

1. Read originals and pin upstream refs, including nontruncated repository trees for path discovery.
2. Trace Atom codecs, representation conversions, relay success/full paths, probe, error mappings and network profiles.
3. Trace broadcast ID callers/algorithms in both implementations; compare content retention, queue age, host freshness, failure cooldown and parent selection.
4. Trace container startup/header construction, HTTP ingest listener, direct playback and YP updates/retries; inspect RPC entry points and source license notices.
5. Correct an interim mistaken claim about180s: both node freshness and failure cooldown exist. Preserve the correction rather than hiding it.
6. Refactor specification and ADRs; create36 source-evidence entries, original-file hashes and lightweight review reference vectors.
7. At the user’s additional request, decompose delivery into 50 dependency-linked tasks with scope, steps, acceptance and stop conditions, plus machine-readable status.
8. Run only the document/vector checks recorded in VALIDATION.md. These do not validate a Rust implementation.

## Next agent: exact starting procedure

1. Read SPECIFICATION.md and this current-state section before historical logs.
2. Inspect applicable repository instructions and actual Git/worktree/toolchain; identify active owners and unrelated edits. Do not reset, clean, overwrite or mark reported history verified.
3. Produce an implementation inventory with evidence links and statuses. If code exists, map it to requirement IDs rather than duplicating it.
4. Resolve any first-slice OPEN contracts using pinned callers and independent fixtures. Keep Station and YT profiles separate.
5. Implement/test core codec and Station ID conversion, then one controlled IPv4 relay vertical slice including503; only then expand to more formats/network profiles.
6. Update this log with exact changed files, test commands/results and remaining blockers. A failed or unrun test stays visible.

## Recording template

Date/time and zone:
Repository / branch / commit / dirty-state evidence:
Requirement IDs and files changed:
Test command, environment, fixture/peer versions:
Passed / failed / not run / blocked:
Artifacts:
Next action and owner:

## Working conventions

- English for agent-facing technical documents; Japanese user-facing summaries are separate.
- Keep source facts, proposed port policy and historical reports visibly distinct.
- Use explicit units (bytes, MiB, milliseconds) and stable requirement/ADR IDs.
- Preserve originals and update provenance when upstream revisions change.
- Follow actual repository/tooling instructions. The original references to unnamed “Global Rule #2/#3/#8” are not self-contained authority. Keep their useful intent (bounded resources, explicit state, careful shell quoting) without inventing a global instruction source. On PowerShell, prefer a reviewable script for complex quoting; do not delete existing files as cleanup.

Latest scope addition: the user requested UI design/function separation. Added REQ-080..087 and tasks P37A..P37F. These are requirements for the planned port, not a claim that its existing UI has been changed.
