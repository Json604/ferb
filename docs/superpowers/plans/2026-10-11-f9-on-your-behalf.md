# F9 — delegated conversations and hand-over

## Goal
Carry an explicitly approved conversation across incoming messages and daemon restarts; stop sending when the owner takes over, and hand over a lead with cited evidence. Target tasks 2 and 3 still need owner-controlled live acceptance.

## Architecture
`ConversationStore` holds immutable account/site/contact/topic scope, owner approval, observed messages, turn checkpoints and hand-over receipts. `converse` plan steps use ordinary gateway workers and checkers for each turn, then sleep on durable scoped observations. Scope interpretation and lead judgments stay with agents. Code enforces identity/surface and declared destination boundaries, paused state, origin labels, deduplication and no automatic unknown-outcome retries.

## Spec and global constraints
D30 standing permissions, MP2 source trust, S4 sink checks, P19 unknown outcome and V1–V8 verification remain in force. Owner approval displays the exact contact/account/topic and platform-terms acceptance. A counterpart cannot change scope or resume a paused conversation. No live sends, permissions, installations or native observations during implementation. User directed compile/diff checks during parallel implementation, then one combined final check. No new budgets or retention limits.

Generic GUI controls do not mechanically prove the signed-in account or selected contact. Fresh account/thread reads and verification remain required; claims of exact code enforcement apply only to concrete target fields supplied by capabilities. Shell and untargeted mutations are unavailable to a bound conversation worker. A hand-over can stop subsequent calls; an input already in flight may have happened and remains governed by P19.

## Review focus
Approval replay/tree isolation; events cannot gain owner origin; exact app/thread matching; no double delivery; restart while waiting versus sending; owner interference before the next mutation; no notification-triage activation from shadow measurements.

## Tasks
1. Write regressions for scope, lifecycle, event routing, receipts and interruption. Implement migration 0033 and durable store.
2. Extend plan and worker schemas; execute repeated, independently verified conversation turns; wait durably and produce cited hand-over. Reuse F6 executor and F5 gateway.
3. Wire owner chat/CLI scope changes and notification rules, pending deliveries and read-only conversation evidence. Preserve origin labels and owner-only RPC.
4. Document target-task and security acceptance scenarios, decision coverage and limitations. Compile and diff check; commit D30/MP2/P19/P11/P14/S4.

## Self-review
Final combined regression/security review belongs to F10. No live acceptance is inferred from fixtures. All notifications remain shadow unless separately authorized and measured; explicit owner-requested conversation waits do not depend on triage classifications.
