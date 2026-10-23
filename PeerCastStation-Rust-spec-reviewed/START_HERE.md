# Start here

This package is a source-reviewed implementation plan, not finished Rust software.

Read in order:
1. SPECIFICATION.md: requirements and explicit uncertainties.
2. WORKLOG_AND_DECISIONS.md: actual verified state versus reported history.
3. IMPLEMENTATION_PLAN.md: 50 bounded implementation tasks and gates.
4. SOURCE_AUDIT.md: 36 pinned source-evidence entries when a requirement needs explanation.

Machine-readable counterparts: implementation-plan.json and source-evidence.json.
Integrity/provenance: manifest.json and originals/.
Review checks: python verify_review.py (Python standard library only).

## Initial agent instruction

Work on the user's actual PeerCastStation Rust repository under its applicable instructions and granted permissions. Do not assume the attached work log proves that implementation, toolchain setup or tests already exist. Begin with P01 and P02, report the verified baseline, and preserve unrelated edits.

Use SPECIFICATION.md as the current working contract. Choose only tasks whose dependencies have current passing evidence. Before each change, identify the task ID, files, inputs, acceptance conditions and stop conditions. Implement one bounded contract at a time, test it, and record the exact result. If a required contract is OPEN, trace the pinned source/caller or obtain a fixture; do not invent behavior.

Keep presentation components and design tokens independent of controllers, API transport and backend domain logic. Follow P37A–P37F for UI separation. A visual change must not silently alter behavior or API contracts.

Reference vectors here are review aids. In particular, the Station ID examples were produced by a Python transcription, not an executed upstream implementation. Obtain independent outputs before claiming compatibility. Never label an unrun, blocked or failed test as passed. Do not upgrade document review checks into product validation.

This package grants no additional permission to install unrecognized software, publish changes, contact public peers, expose administrative services, delete existing data or bypass repository checks. Follow the user's actual authorizations for those actions.
