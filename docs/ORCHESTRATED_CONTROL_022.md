# FERB 0.22: agent-directed control

The owner's design of 2026-10-03 replaces the effect-gated design. This document is the current contract;
`DESIGN_ONE_WORKER.md`, `REWORK_PLAN.md` and the 0.19–0.21 notes describe historical implementations where they disagree.

## What the owner experiences

Give FERB a task. FERB responds, resolves necessary questions, and keeps working toward the requested result.
Setup collects access to the Mac and connected accounts. Routine actions do not need another permission question.

The agents interpret authorization from the whole request. “Send Rahul the deck” authorizes sending it. “Get the
deck ready for Rahul” requires preparation first; ask about sending only with the actual deck ready for review.
Likewise, completing an assignment does not imply submitting it. These are examples for agent judgment, not
phrase lists in code. Missing facts such as an OTP can still require owner input. A disconnected extension or
missing OS permission is an environment problem, never an owner declining an action.

FERB's own browser profile remains the default. A personal profile is used when the owner directs it, including
natural references such as “scaler Gmail.” Profile choice belongs to the agents; code no longer removes browser
scopes because particular words were missing. Background operation remains the goal, verified by the owner.

## Execution

1. Each turn retrieves relevant memory, matches from today's activity, matching action descriptions, current
   runtime/connection facts, and durable conversation context. Retrieval is bounded; the daily log isn't injected whole.
2. The orchestrator produces a task with `worker_type`, goal, preferred account context, and success conditions.
   `default` uses the configured worker; `codex` and `claude` select the corresponding installed adapter.
3. The worker receives the installed tools across account families. It discovers capabilities through the MCP
   tool list and `actions.search`. It reads and acts in the same attempt.
4. `blocked`, `failed` or `needs_user` reports return to the orchestrator. The orchestrator chooses another runtime
   or approach, asks a necessary question, or reports an external blocker honestly. Prior artifacts, recent reports
   and uncertain ledger actions are passed forward. Resources remain owned by the same task during recovery.
5. A worker's `owner.ask` goes through the orchestrator first. It answers from the request/context when possible;
   otherwise the question reaches the owner's normal FERB chat. OTP entry uses the separate hidden-input path.
6. Requested-result checks inspect outputs. A failed check returns to recovery; inaccessible evidence is unknown,
   not proof of failure or success. Finished tasks release their tabs/processes and revoke run tokens.

There are no fixed tool-call, worker-attempt, task-duration or Claude-turn caps. Explicit stop still cancels input,
runtimes and task-owned resources. A silent worker still has a liveness watchdog; waits on the owner suspend it.
Provider failures and physical disconnections remain real limits.

## Gateway contract

The gateway validates a live run token, running task, available capability and argument shape. It serializes
resource access, executes the capability, records results/uncertain outcomes, redacts output, and exposes errors.
It does not run `PolicyEngine.evaluate`, contact matching, profile phrase checks, exposure-based tool withdrawal,
effect floors, per-action approval, automatic undo delays, duplicate-send permission, or Jev action/result gates.
`effect` and `why` are optional annotations. Older kind/label helpers can still describe trace/ledger metadata;
they do not authorize or reject execution. Legacy policy helpers remain for old records/tests and are not connected
to the execution path. Do not reconnect them to “fix” an agent judgment issue.

Code retains execution integrity: pairing/authentication, correct task/tab ownership, fresh element/screenshot
references, valid gesture coordinates, OS permission errors, process ownership, explicit owner stop, secret sealing,
password-field handling, redaction and setup-configured privacy exclusions. These establish where an operation
can actually land. They do not decide whether the owner meant to authorize the task.

Files/shell no longer use phrase-based protected-path or command deny lists. Normal OS permissions and filesystem
errors apply. Native browser input and local HTTP development URLs are available. No automatic account-helper
page opens before each browser action; the agent chooses any account inspection it needs.

## Daily context

`~/.ferb/activity/today.jsonl` contains redacted JSON objects with `what`, `where`, `when`, `day`, `actor`, `device_id`,
and task/status fields when available. Sources are the existing metadata watchers and gateway actions, including
uncertain outcomes. The file resets on the first record/search after the local date changes. It survives same-day
restarts. Search ranks keyword overlap and recency and returns a bounded number of matches; it is not semantic
embedding search. Existing memory and SQLite trace retention remain separate.

Each installed Mac records its own observations. There is no cross-device synchronization yet. Watchers cannot
observe apps/profiles without the corresponding setup/connection, and never record raw keystrokes or clipboard.

## Setup and deployment

Normal setup collects Accessibility, Screen Recording, Input Monitoring and Full Disk Access, and offers Automation
for currently running scriptable apps. Updating an installation from before 0.22 runs this permission collection once;
`setup.permissions_version = 22` records that it was offered. Skipping a permission is allowed and remains visible
in doctor. macOS can require a new grant for newly installed apps or a changed binary; browser pairing and account
login are still setup requirements. FERB cannot pre-grant those on the owner's behalf.

This checkout does not update the installed daemon until the owner runs `ferb setup --update` or explicitly authorizes it.
No extension protocol change is needed for 0.22 (extension 0.9.0, protocol 5).

## Verification

`tests/test_orchestrated_control.py` exercises routing, recovery, final-choice forwarding, unlimited actions, resource
cleanup, context retrieval, runtime caps, and continuation after failed result checks. Existing integration tests
cover real gateway calls with fake apps, cancellation, secret input, redaction, stale references, ownership and
background transport. `TEST_MIGRATION_022.md` records retired assertions for the superseded permission policy.

Offline tests cannot establish that a real model interprets every authorization correctly or that a task completes
without desktop disturbance. Owner acceptance is pending: read the named scaler Gmail source, hand the actual
requirements to the requested coding session, inspect its completed artifacts, and stop before an unrequested
submission. The owner verifies the result and any desktop movement. Earlier WhatsApp/YouTube scroll acceptance
remains evidence for those driver operations only.
