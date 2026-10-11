# Native usability and recurring OTP follow-ups

## Goal
Resolve the owner’s five observed issues: recurring split OTP failure, composer paste, verbose replies/questions, unsolicited panel expansion, and ambiguous task inspection/stopping.

## Architecture
Keep sealed OTP discovery/fill within the existing browser boundary. Add a separate concise presentation alongside complete original reports. Native chat preserves details, uses native editing commands, and presents independently identified tasks with scoped controls. Background events update state without opening the panel.

## Spec
Use the Zomato task 9de9b3888e56444288c1368aff1f91fa and recent session-analysis conversation as diagnostic evidence. Fix observed causes, not speculative site-specific heuristics. Keep complete reports available on demand. Preserve concrete consent scope and secret isolation. A per-task Stop targets exactly that task; inspection never stops it.

## Global Constraints
Owner explicitly says keep the current task running. No daemon/app restart, install, live browser action, UI focus or cancellation. Existing live-steering commit remains pending activation. Preserve both repositories’ existing work and owner AGENTS edits. Latest request supersedes automatic answer expansion and the previous prohibition on an additional presentation pass. No new arbitrary limits. Build and focused regressions first, one combined end check; live acceptance remains separate.

## Review Focus
Sealed field binding and stale browser extension versions; secrets and redaction; concise text must retain requested facts and uncertainty; exact task identity and out-of-order replies; native paste provenance; background focus and hover state.

## Tasks
1. OTP: inspect trace and installed path; add failing regression reproducing binding failure; fix discovery/fill and run focused tests. Agent otp_persistence owns relevant backend browser files.
2. Presentation: inspect recent replies; add failing tests for concise/original contract and redaction; implement backend additive display_text and streaming integration without action authority; agent concise_presentation owns orchestration/presentation files.
3. Native UI: add failing paste/focus/task regressions; implement native edits, no background expansion, per-task inspect/stop and expandable original details. Agent ui_followups owns frontend files.
4. Integration: review scoped diffs, resolve findings, run combined backend/native checks, update current docs and report. Commit citing D8/D12/D28 (secrets), D3/MP2/P29 (chat/task continuity), D27 (background interaction), P27 (owner-facing questions). Push without merging or force-push. Do not activate while owner task runs.

## Self-Review
Offline regressions and compilation do not establish live Zomato success or the owner’s visual acceptance. Report checkout vs installed state separately and cite exact trace/code evidence for diagnoses.
