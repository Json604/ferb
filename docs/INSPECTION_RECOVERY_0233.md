# Calendar and WhatsApp inspection recovery — 0.23.3

Branch: `astra/version`. Status: offline validation complete; installation and owner-observed acceptance pending.
No personal app was operated for this patch. The audit used existing local task traces and the owner's screenshot.

## What the audit established

The installed 0.23.2 daemon matched its checkout hash (`9e9af9efc0d15d90`). Six tasks from 13:35–14:00 IST on
2026-10-03 exposed both execution mistakes and result-inspection mistakes:

| Request | Actual evidence | Failure |
|---|---|---|
| Today's calendar | The school account showed no events; a later explicitly personal-account check showed three evening meetings | The first answer did not clearly scope its account choice |
| Personal calendar | Browser snapshots explicitly showed the requested personal address and three meetings | Worker closed its tabs before checking; the account extractor returned nothing, which code treated as false. Literal alias equality would also reject a full address |
| Official school messages | FERB searched macOS Messages, then its empty local database | Planner assumed the app from a generic word; user meant WhatsApp |
| WhatsApp, five days | Some real group messages/posters were read | Unrelated community included; other groups/range not fully covered; one recent-message jump left old messages visible |
| Corrected school groups | Club announcements were read again | The larger official source set remained incomplete |
| Group names containing SST | All nine reported names appeared in earlier native snapshots | Checker saw only the final filtered view, could not revisit chats, and had no searchable access to those earlier reads |

Four tasks ended `done` with unknown verification. They should have returned to planning for missing work rather
than appending a long “I couldn't confirm” paragraph to an apparent completion. No recorded message-send/edit
action was found. This is evidence about the recorded execution, not a claim that a final screenshot proves the
absence of earlier actions.

The screenshot shows Mission Control. Some pointer calls reported stale window geometry. It does not establish
that the owner's peeks caused those errors, or that FERB independently switched Spaces. Freshness checks remain
in place; no pointer transport change is justified by this evidence alone. The owner's verdict about independent
cursor/focus/desktop movement remains pending.

## Changes

1. Account extractors produce observations for the checker. Account aliases, addresses and conditions containing
   alternatives are interpreted from evidence by the agent, not decided by exact string equality. Missing extractor
   data is unknown, not a contradiction.
2. The checker gets the worker's installed capabilities. It can navigate, search and scroll to inspect results.
   Its prompt keeps it on inspection: do not send/edit/delete content, submit or repair deliverables. The old
   read-scope allowlist and borrowed-tab action restriction are removed. Registered task ownership, matching
   parent/request/identity/device/tree, cancellation, fresh references and ordinary cleanup remain.
3. `task.evidence` searches actual redacted tool inputs/results from the caller's exact task, or its registered
   worker when checking. Lists have chronological cursors; a selected record has text offsets. No arbitrary task
   selector, model reasoning, recursive evidence calls or screenshot pixels are returned. Earlier source snapshots
   can establish historical observations even after a view changes; changed state or image-only claims need fresh
   reads. An input success is still not proof of the requested result.
4. Both false and unknown conditions return to the orchestrator with structured findings. It chooses another
   attempt, an unresolved owner question or an honest blocker; there is no fixed retry count. Unknown conditions
   cannot yield `done`, and missing conditions survive a success-sounding final blocker summary. Full inspection
   details remain in the trace; the owner-facing unknown-result fallback lists the missing conditions briefly.
5. Prompts require source/account resolution, source-set identification, official membership evidence, date-range
   coverage, search/filter verification and recovery from failed navigation. Owner corrections supersede earlier
   assumptions. No app-name or task-phrase routing rules are added to code.

## Validation

- Ten new regression cases reproduce unknown-result completion, missing historical observations, restricted
  checker navigation and account-alias/absent-extractor failures. They also cover record paging, secret redaction,
  exact-task isolation, exclusion of model/evidence spans, pending/error records and truthful final status.
- Existing browser tests now exercise checker navigation on the worker's alias while retaining the original owner,
  unrelated-task isolation and cleanup. Existing reporting tests cover complete reports, cancellation and retries.
- Focused suite: **47 passed** (3.84 s).
- Full offline suite: **566 passed, 52 deselected** (114.93 s).
- Core mypy and `git diff --check` passed.

These checks use fake models/apps and disposable daemon homes. They establish contracts, not live model judgment,
complete WhatsApp coverage or an undisturbed desktop. Private trace exports remain outside Git; old outcomes were
not rewritten. No extension update is needed; extension 0.10.0 / protocol 5 is unchanged.

## Owner acceptance after installation

From this branch, the owner runs `ferb setup --update` when no task needs to keep running. It reinstalls and restarts
the daemon. Repeat one task at a time, with the owner judging both correctness and disturbance:

1. Ask for today's calendar in the explicitly named personal account. Confirm the address/source, date and events
   match. The task should not fail merely because the account extractor lacks support or a tab was closed.
2. Ask for WhatsApp group names containing SST. Confirm the complete name list, including names no longer visible
   in the last screen, and no message sends/edits.
3. Ask for five days of official school announcements, excluding the unrelated community already corrected.
   Check which groups were included, the date boundary in each, and important items against the source. If the
   official group set is genuinely ambiguous, FERB should ask a specific question; otherwise it should finish the
   missing reads before reporting completion.

Pause after each task for the owner's verdict. Inspect `ferb trace` for `task.evidence`, actual source reads,
date coverage and recovery decisions. Record any independent cursor/focus/Space movement separately from the
owner opening Mission Control. A correct list alone does not validate the five-day coverage task.
