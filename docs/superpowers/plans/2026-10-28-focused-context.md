# Focused context, discoverable guidance and faithful outcomes

Date: 2026-10-28. Owner authorized implementation after the read-only audit: “do all what you think is good BUT you have to be justifiable ... FINISH ALL”. This is approval to proceed after this plan, without a second approval checkpoint. Learning and memory behavior remain unchanged. Source implementation only; installation/live acceptance remain owner-run.

## Owner workflow override (during implementation)
The owner explicitly directed: “fix first and only after all fixes done run the real and unit tests. NO TESTS BEFORE ALL THE FIXING”. This supersedes the test-first/per-task execution sequence below and the repository/skill test-first default for this work. Stop the in-flight Task1 final suite. Finish all production changes and source reviews first; regression tests may be updated with the implementation, but execute no unit, real-model or other validation runs until all fixes are complete. Then run combined tests and recorded-task replays, fix any findings, and revalidate the final source. No pushes of unvalidated implementation.

## Goal
Reduce unnecessary instruction/history input while preserving capabilities, owner authority and continuity. Correct the inspected coding-session and visa-check failures and preserve concrete checked results in answers.

## Architecture
Keep a short permanent interaction/role contract. Store detailed product procedures as registered, versioned packaged abilities, discoverable by a compact catalog and read on demand. Keep planner runtime in supervisor mode with no acting tools: a typed intermediate context-read response lets the host read only the current conversation and installed guidance. Worker/checker use existing observation tools. Use the preceding conversational exchange and a continuity summary instead of filling the window with all old conversation; exact older records stay retrievable. No artificial token/message retention cap, intent keyword classifier, new worker role or runtime privilege expansion.

Separate requested results from FERB conduct in typed conditions. The checker records any correction of a planner proxy against the original request; ambient desktop changes do not prove FERB misconduct. Preserve real coverage gaps and violations. A conclusive negative scheduled check succeeds; inability to check is not satisfied by a blocker report. Report-only repair and evidence inspection must not repeat successful external actions unnecessarily. Partial answers keep the checked substantive report when its truthfulness condition passed; otherwise show concrete confirmed findings without trusting rejected prose.

## Spec and justification
- Latest retry cd7c2dbe: planner 70a0add3 recorded 257,493 input tokens; system 53,681 chars, context 884,679, conversation 739,448. Window-filling history is the dominant avoidable input, so prompt shortening alone cannot solve it.
- Coding inspection 95f3cb3: final requested session findings passed, planner-added unchanged foreground failed. Require FERB conduct, not a motionless owner desktop; structured interpretation records make corrections reviewable.
- Visa 5b5b52b: negative availability was established, checker demanded source coverage and recovery repeated checks; preserve evidence and repair only gaps. af1ae930 marked an inconclusive portal error successful through an “or report blocker” condition: retain the actual check requirement.
- Retry cd7c2dbe: _report dropped checked no-slot facts and led with playback uncertainty. Preserve approved result prose and distinguish caveats.
- Existing abilities.list/read provide confined discovery but planner has no tools. Add confined host context reads rather than enabling acting worker mode.

## Global constraints
CLAUDE.md and AGENTS.md apply. Tests first; one fresh implementer then fresh reviewer per task. No learning/memory pause, retention changes, arbitrary caps, daemon/app restart, live third-party actions or real_claude tests. Keep redaction, historical-data framing, origin/source propagation, S3 exposure, S4 authorization and Stop/cancellation. Read guidance from explicit registered package resources, never caller paths. Preserve pending clarification/approval and schedule scope. No changes to model choice or caching defaults. User authorization supersedes previous window-fill selection policy only; physical runtime limits still apply.

## Review focus
Intermediate context responses cannot execute side effects, leak draft text or cross conversations. Original history stays exact and available. Summary failures never silently erase continuity. Retrieval sources exposed in prior rounds still transfer to the task. Guidance loading works for every affected role including recovery/checker; moving text must not remove security floors or accidentally load all guidance. Checked report invalidated by cleanup/steering must not be reused. Legacy schemas remain readable. No false pass for a genuinely incomplete/blocked check, unauthorized focus action or inaccurate report.

## Task 1 — Outcome semantics and useful partial answers
Files: ferb/orchestrator/{schemas.py,verify.py,supervisor.py,prompts/{supervisor,worker,verifier,dispatch,scheduled}.md}, focused tests.
Interface: backward-compatible result/conduct condition classification; auditable checker interpretation/correction fields with original condition retained. Existing status vocabulary and safety checks remain. Partial reporting uses checked prose only when report truthfulness passed and lifecycle did not invalidate it; concrete passed evidence otherwise.
Test first: reproduce the recorded visa retry report loss; rejected report never displayed as approved, cleanup regression, owner window change versus FERB action, complete negative check versus portal blocker, recovery needing evidence only. Implement; run focused checks, full nice -n 10 uv run pytest -q, uv run mypy ferb/core, git diff --check. Commit citing P16/P27/D34/W4. Fresh read-only review and fixes, then push main.

## Task 2 — Focused history and confined planner retrieval
Files: supervisor.py, schemas.py, continuity.py, context helper module if needed, prompt/streaming and context/exposure/security tests.
Interface: typed context-only intermediate outcome, rejects simultaneous side-effect fields. Host readers for current conversation and packaged abilities/owner skills (only if needed to preserve discovery), no task authority. Each read has exact origins/references and trace. Current exchange selected by recorded conversation boundaries, not token/count constants. Compact running continuity with explicit retrieval/fallback; learning stores untouched.
Test first: long irrelevant older conversation absent from initial call but exact retrieval works; contextual follow-up and schedule histories; cross-conversation denial; intermediate does not emit/write; source exposure accumulates across rounds; cancellation; summary failure visible. Implement and required checks. Commit citing L28/P3/P20/P21/MP2/S3. Fresh review/fix/push.

## Task 3 — Modular permanent guidance
Files: ferb/abilities/, orchestrator/prompts/, _system composition, packaging and prompt/security tests.
Interface: explicit procedure definitions/version/hash, compact discovery index; on-demand planner via Task 2 and worker/checker via abilities.read. Small permanent core carries authority/safety/reporting/discovery; structural role-specific guidance automatically included only where required (e.g. schedule/checker) and detailed domain procedures fetched when relevant. All no-tool internal roles must retain required behavior or receive selected guidance explicitly.
Test first: relevant role can discover/read instruction, unrelated role no longer receives large procedure bodies, packaged resources available, unknown IDs/symlinks refused, external source content cannot become trusted guidance. Preserve original policy obligations and mark unavailable execution accurately. Measure before/after recorded-case initial input and role instruction sizes. Required checks, commit P21/D32/MP2/P16, fresh review/fix/push.

## Task 4 — Recorded owner-task model replay, documentation and whole-change acceptance
Owner clarification during implementation: “rather than just passing test suits ... check on real tasks ive done with ferb ... ensure ... better and not worse”. Add frozen-evidence real-model replay from the owner’s actual recorded tasks, before declaring acceptance of the approach. Cover coding inspection, completed-negative and portal-blocked visa checks, latest visa retry reporting, and unrelated successful tasks/continuity. Compare old and changed prompt/context decisions and measured input. Do not repeat sends/bookings/external actions, run the installed daemon, or commit private traces. Use existing Codex runtime/harness in isolated observation-only/no-action replay; report unavailable historical evidence explicitly. Real model replays are stronger than scripted tests but distinct from live owner-observed app acceptance. Any regressions must be investigated and fixed with scoped re-review before completion.

Update ARCHITECTURE.md, CHANGELOG.md, docs/0.27_PLAN.md, docs/0.27_DECISIONS.toml, OWNER_ACCEPTANCE.md with justification, exact mechanisms, measured reductions, residuals and owner-run scenarios. Complete whole-change fresh review and one fix wave; required final validation. Record source-only status and no claimed live acceptance. Commit and push main.

## Self-review
The main risks are continuity loss and silent policy loss, not just token count. Retrieval must ship before large prompt relocation, and outcome repair must not weaken verification. The existing trace incidents become deterministic integration regressions with scripted models; they prove plumbing, not future model judgment. Live owner acceptance must separately exercise actual model selection, window switches, no-slot/blocked checks, and history follow-up. No private trace payloads or credentials enter committed fixtures.
## Validation scope update — owner, 2026-10-28

The owner subsequently requested **only unit/offline tests**, because real-model replay credits are insufficient.
Do not execute the final recorded-task model replays or live model tests. Keep the earlier replay findings as
historical evidence only; they do not validate the final prompt/history implementation. Report semantic behavior
and total runtime token savings as unverified until the owner chooses to run those checks later.
