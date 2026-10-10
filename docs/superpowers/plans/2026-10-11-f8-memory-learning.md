# F8 memory and learning (D2, D18, D19, P10, P17, S7, MP1–MP7, M4, M8)

## Goal
Make verified task experience reusable with inspectable evidence, permanent origin labels, owner-controlled identity,
scoped lessons, and a local dashboard. Retrieve candidates before planning without granting them authority.

## Architecture
Extend the existing permanent SQLite database (rather than moving an installed database). Derived records and their
source graph sit beside existing L0 trail, L1 episodes and L2 versioned facts. The sleep consolidator is the only model
writer. Task agents get read-only retrieval and procedure details. Owner CLI RPCs own confirm/edit/forget/rollback.
A local dashboard uses an ephemeral bearer token. Embeddings stay local and remain off until measured against the
owner's queries; the benchmark and model-artifact hash checks are implementation, the measured adoption is acceptance.

## Spec
1. Deterministic derived IDs, valid/recorded time, maker/model/prompt, immutable content versions, evidence cards,
   recursive deletion including indexes/vectors and derived children, and auditable reversible consolidation batches.
2. Capture app versions at task completion. Induce only from independently verified outcomes; failure lessons cite
   failure/success contrasts and explicit scope. Procedures are text tool templates with expect lines and typed slots.
   Learned requires three verified successes and two slot values, with the latest on current app versions. An app
   change or consecutive failures marks suspect. Outside-derived action/destination steps require owner confirmation.
3. L4 is owner-only; inferred facts remain labelled hints. Pending identity proposals produce at most one digest/day.
4. Unicode keyword/title-trigram search plus time/app/person filters; local vector rank fusion only after evaluation.
   Agents decide relevance and selected procedure, preserving step checks and final verification.
5. Searchable local dashboard for all layers and explicit confirm/edit/forget, evidence, history, and rollback.

## Global constraints
Latest owner instruction authorizes immediate parallel implementation and compilation checks, then one end check.
No install/restart, live model tests, fabricated verification, new product caps, or new source authority. Existing
retention is unchanged. Embedding adoption and release sign-off require actual measured owner evidence.

## Review focus
Origin laundering; source deletion after retention; stale app versions; unverified successes; rollback restoring
forgotten content; URL token leaks/CSRF/XSS; concurrent consolidation; no procedure bypass of gateway/checks.

## Tasks
1. Add migration0030 and `ferb/memory/learning.py`: graph, versions, episodes, procedures, identity, rollback/forget.
   First author regression cases for poisoning, learned thresholds, deletion and rollback.
2. Add `consolidate.py`, retrieval/embedding evaluation, read-only capability; wire daemon and F6 callback interfaces.
3. Add token-protected dashboard, CLI/RPC owner controls and digest. Add transport/privacy regressions.
4. Document implementation and pending real-history evaluation; compile code and diff check. Commit citing decisions.

## Self-review
No stored action becomes permission. No current app version is retroactively asserted for old evidence. A rejected or
forgotten record cannot be resurrected by rollback. Full evidence stays fetchable; ranking limits are not storage caps.
