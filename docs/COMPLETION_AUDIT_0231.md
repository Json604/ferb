# 0.23.1 — completion reporting and coding-process monitoring

## Recorded run and owner verdict

The 2026-10-03 audit examined Coursera task `c89a45bef5154ac5a2daf513723ae622` and FMI redo task
`64bd83fcd2fa44c4a605b9d62e2b47be` from installed 0.23. Source snapshots confirmed the requested scaler profile,
Coursera due dates, Gmail instructions, form and assignment brief. Eight reminder rows remained pending. Claude's
saved session recorded Opus 5.5, launched with high effort, writing the redo files and completing. The two-page PDF
rendered readably, and independent calculations matched the main figures. The old PDF hash matched. No submission
was recorded. The owner confirmed: “it didnt disturb me at all.” No native input actions were recorded in these runs.

The requested local exports contain the owner/FERB conversation, FERB's exact Claude prompt, Claude's conversational
replies and a separate tool-action list. Private transcripts remain outside Git under `~/.ferb/exports/`.

## Findings and repairs

1. Coursera's checker found two omitted durations, marked the condition false, and recovery finished with
   success-sounding prose. Chat showed that prose even though the task was failed. Unresolved false checks now
   remain explicit in the final answer regardless of recovery's wording. The recovery prompt directs correction
   of repairable omissions. Agent judgment still chooses retry/ask/finish; no intent gate or forced retry count.
2. The checker received only the first 1,500 characters of the worker report. The FMI checker even noted the
   cutoff while approving completion. It now receives the complete report and artifact/evidence references;
   missing evidence must remain unknown. Complete owner answers, checker evidence and worker trace reports are
   retained, so a long report cannot lose its final caveat or artifact section.
3. Checker runs emitted completion but stayed running in the database. Verification now persists its actual
   terminal state and result, including invalid output and cancellation; a retry resets it to running. A completed
   check with a false finding is a completed checker run, not proof the parent task succeeded. Existing historical
   task records are not rewritten to make the old run look successful.
4. The first detached Claude launch returned a launcher exit code and “started,” but three minutes later there
   was no log or session. A direct tracked retry completed. `shell.start` now exposes immediate managed launch;
   `shell.wait` returns when that process exits or an observation timeout expires, with output and exit code.
   `shell.read` includes exit codes too. Instructions prefer these controls over detached wrappers and timed sleeps.
   This does not claim to supervise arbitrary detached grandchildren or guarantee a coding model follows instructions.

## Validation and rollout

Regression coverage includes success-sounding failed recovery, complete long reports and trailing caveats,
checker completion/error/cancellation/retry, real local PTY start/wait/nonzero exit, process ownership and cleanup.
These tests use scripted models and local shell commands. No additional live coding session, browser task or
submission is started by this patch. Installation requires the owner's rollout approval; extension 0.10.0 is unchanged.

Validation: 541 offline tests passed, 52 deselected in 113.41 s; core mypy passed (5 source files), as did
`git diff --check`. Ten added regression cases cover the audited failure contracts. The full workflow's owner
no-disturbance verdict applies to the earlier 0.23 run, not a new live test of this patch.
