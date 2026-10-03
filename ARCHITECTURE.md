# FERB architecture — 0.23.2

Current implementation on `astra/version`. The owner's 2026-10-03 design replaces the old read/actor and effect-gate
contracts. Read [agent-directed control](docs/ORCHESTRATED_CONTROL_022.md) first. Historical documents and changelog
entries describe their own versions; they are not instructions to restore removed gates.

## 1. Purpose and principles

FERB is an always-running personal assistant for macOS. One conversation, one voice: FERB. The orchestrator
interprets the owner's request, assigns workers, resolves their questions and changes approach when necessary.
Workers perform both reading and acting. Prepare reviewable work before asking about an unrequested final outcome.
An already requested final outcome does not require another permission question.

The agent decides intent, account choice, authorization, method, retry and escalation. Code implements platform
operations, records facts and keeps operations bound to the correct task and surface. Background operation is the
goal; only the owner's observation proves it on their real apps.

## 2. Processes and trust boundaries

`ferb chat` and push-to-talk connect to the daemon over an authenticated local Unix socket. The LaunchAgent keeps
the daemon alive when chat closes. `ferb mcp` is the worker tool source. Per-run tokens bind requests to the worker
and task; tokens are revoked when the run ends or the owner stops it. Native runtime tools remain contained so
FERB can record actions and cancel them through one execution path.

Codex and Claude are adapters in `ferb/runtimes/`. Provider login, network access, structured output and actual
runtime availability can fail; failures return as facts for recovery. A failed runtime is not globally disabled.
Workers have no fixed task duration or turn count. A silent-worker watchdog detects lost execution, while owner
waits pause its clock. Supervisor model calls still have transport timeouts.

## 3. Accounts and scopes

`owner` and `ferb` identify account context and tool families. They no longer determine which installed tools a
task worker receives. The worker gets the union of registered capability scopes. Availability describes connection
facts without removing disconnected capabilities from discovery. A capability bound to one account family runs
with that family's context even when the task prefers the other identity.

FERB's own profile is the normal browsing default. The agents select personal profiles when requested, including
“scaler Gmail” or a current Brave tab. Profile strings, task ownership and paired installation identity still bind
the actual driver operation. Unrelated personal profiles are not a fallback for a failed connection.

Independent checkers receive read scopes. A registered checker can inspect its own worker's existing browser tab
without losing lazy-loaded content or scroll position. The driver enforces read-only borrowing and task isolation.

## 4. The chat turn and worker recovery

`Supervisor.send` retrieves durable conversation/task context and relevant memory. `_supervise` adds matching daily
activity and registry entries, then runs the orchestrator. A reply answers directly; a clarification asks for missing
information; a task supplies goal, `worker_type`, preferred identity, informational targets and success conditions.

`run_tree` owns resources across attempts. `_worker` runs the selected adapter with the worker prompt and complete
installed tool access. A blocked, failed or question-bearing report goes through `_dispatch`; the model chooses a
new type/approach, asks the owner, or reports a genuine external blocker. Completed artifacts and recent evidence
survive recovery. There are no automatic phrase-based retries or fixed action-count/loop cutoffs.

Result checks run after a claimed completion. Unmet conditions return to the orchestrator for repair. Unknown
inspection is reported as unknown. Final output includes the result, artifacts and verification evidence. Cleanup
releases task-created tabs, detaches borrowed owner tabs and ends ordinary task processes. Background processes
remain visible until explicitly stopped. Closing chat does not cancel the task; explicit stop does.

The checker receives the complete worker report and artifact/evidence references. Its run state is persisted as
done, failed or cancelled, independently of whether the checked condition is true. A retry resets that state to
running. Unresolved false checks remain visible in the final answer even if recovery's prose sounds successful;
complete answers and worker trace reports are retained rather than silently truncating their final sections.

## 5. Action execution and owner questions

`Gateway.call` authenticates the run, validates argument shape and selects the capability. `_call` supplies trace
metadata, resource locks and cancellation. `_execute` creates ledger intent where appropriate, calls the driver,
records its result or unknown outcome, redacts output and publishes `action.observed`.

No per-action permission engine runs here. Reading external content does not withdraw tools. `effect` and `why`
are optional metadata, without floors or mandatory delays. Browser account checks, contact matching and final
outcome decisions belong to the agents. Existing legacy policy helpers are disconnected from execution.

`owner.ask` calls `Supervisor.route_question`. The orchestrator answers if the request/context settles the question;
otherwise the existing chat question transport shows it in FERB's voice. The worker continues after the answer.
The hidden OTP path remains separate so codes do not enter model context or ordinary logs. Unattended scheduled
runs can receive an orchestrator answer; if owner input is still required they report the missing input.

## 6. Mac control

`MacComputerDriver` operates native apps through Accessibility, app-directed keystrokes, menus and window capture.
It prefers observed semantic controls and background input. Snapshots bind element references to fresh windows.
Password fields and configured excluded apps are handled by the existing privacy mechanisms. A background input
that has no visible effect is not reported as a completed task.

`mac.pointer` supports screenshot-bound click, hover, drag and wheel scroll. Coordinates must match the captured
window. Window-targeted delivery is attempted where available; transport limitations are reported rather than
solved by switching Spaces. Native browser fallback is available for the agent to choose. `mac.click` shares this
transport after AX. A dispatch is never automatically replayed as a foreground click because AX did not change.
The event carries screen coordinates and a separate window-local point; see [pointer evidence](docs/BACKGROUND_POINTER_0221.md).

For scroll, the agent chooses distance and batch count. Batches are bounded operations, not task-length limits.
A stopped batch or unchanged viewport is not proof of reaching the bottom. Lazy-loading pages require fresh
inspection. Details and owner evidence: [background scrolling](docs/BACKGROUND_SCROLL_021.md).

## 7. Browser control

Browser extension lifecycle is independent of task authorization: setup publishes a managed local bundle,
extension 0.10+ reloads newer versions when task resources are idle, and the native handshake reports lost pairings.
Initial browser installation/approval and account selection remain distinct. See
[extension lifecycle](docs/EXTENSION_LIFECYCLE_023.md) for migration, diagnostics and live evidence.

Chrome and Brave use the paired Chromium extension, native-messaging bridge and per-tab debugger sessions.
`browser.*` serves owner profiles; `ferb_browser.*` serves FERB's profile. Task tabs are created inactive in a
minimized task window. Existing owner tabs can be attached without taking ownership of the surrounding browser.
Browser opens and windowless wakes never reactivate a saved front app after awaiting the extension. A user can
switch apps during that wait; a browser being in front does not establish that FERB took focus. The extension's
inactive/minimized controls remain the background mechanism. See `docs/BROWSER_FOCUS_0232.md` for the observed race.
A disconnected profile gets a windowless wake attempt; FERB does not open visible wake tabs automatically.

Snapshots combine Accessibility roles and DOM-backed references. Fresh references, frames, generations and exact
tab ownership prevent input landing on the wrong surface. Screenshot points preserve exact pixels, including
cross-frame transforms; visual gestures consume their snapshot. Secret fills are origin-bound and sealed. Browser
account inspection is explicit rather than an automatic helper-page prerequisite. Local HTTP(S) development pages
are allowed. Unsupported schemes and actual transport/protocol failures are still reported.

## 8. Shell and files

Files resolve relative to FERB's workspace or use the supplied absolute path. OS permissions, missing files,
overwrite flags and unique edit matches determine execution. There is no task-identity folder jail or protected-word
path list. Shell commands run through task-owned PTYs with environment cleanup, redaction, polling and cancellation.
Command strings are not classified into permission gates. AppleScript preserves target/argument encoding and normal
execution errors without a script-word deny list. Use these tools within the owner's requested scope.

`shell.start` returns a managed process id immediately. `shell.wait` waits for that process's actual exit (or a
bounded observation timeout), returning its exit code and paged output; `shell.read` includes exit status too.
Workers run coding commands directly, inspect startup evidence and wait on that id, rather than detaching children
with `nohup`/`&` and assuming a launcher message proves success. `background=true` means survive task completion.

## 9. Verification and uncertain outcomes

The action ledger records `intended → executing → done | failed | unknown`. A crash or lost acknowledgment can make
an action unknown. The orchestrator and replacement worker receive those facts and choose fresh inspection before
repeating anything that may already have happened. The gateway does not enforce an automatic reconciliation/retry
policy. Driver transaction deduplication and delivery evidence still prevent accidental transport replay.

`Checks` can inspect files or available driver evidence; conditions needing judgment use the verifier. `met=null`
means inaccessible or inconclusive evidence. A successful click or a ledger row is not proof of the owner's final
result. Failed result checks can trigger agent-chosen repair; a reported blocker never becomes a success claim.

## 10. Always-on context and memory

Existing watchers observe app focus, redacted window/tab metadata, idle state, app installation and watched-folder
changes. They do not capture raw keys or clipboard. Setup-configured exclusions and pause apply before storage.
`DailyActivity` mirrors those observations and gateway actions to `~/.ferb/activity/today.jsonl` with what/where/when,
actor, device and task/status metadata. Same-day restarts preserve it; the first read/write on a new local day resets it.

Every turn searches memory, today's log and the installed registry. Daily/registry retrieval currently uses bounded
keyword ranking, not embeddings. Workers can inspect `actions.search` and existing trail/memory capabilities when
more context is needed. Memory and the longer SQLite audit trail retain their own lifecycle; a daily reset does not
forget explicit preferences or durable task history. Cross-device synchronization is future work.

## 11. Storage and observations

SQLite WAL stores tasks, journal, spans, ledger, memory, pairings, timers, schedules and trail. The bus connects CLI,
voice and telemetry consumers. Tool inputs/results and daily records pass through redaction. Images are returned to
the model rather than stored in ordinary spans. OTPs and configured secret values use their specialized transport.
The daily JSON file is owner-readable only when created; configuration and runtime credentials use owner-only paths.

## 12. Setup and permissions

`ferb setup` installs the dedicated interpreter/environment, scans apps, collects Accessibility, Screen Recording,
Input Monitoring and Full Disk Access, offers Automation for running scriptable apps, and performs daemon checks.
A pre-0.22 update offers this collection once and saves `setup.permissions_version=22`. Skipped/missing permissions
remain visible in doctor. New apps or changed binaries can still require macOS grants. Pair browser profiles and
sign in to accounts during setup. FERB.app separately handles microphone/speech grants for voice.

## 13. CLI

Use `ferb chat`, `ferb stats`, `ferb trace`, `ferb logs` and `ferb watch`. Chat reconnects to durable conversations;
`--new` starts another and `--resume ID` selects one. `ferb stop` persists until `ferb resume-tasks`; task stop is
narrower. `ferb pause`/`resume` control sensing. Memory, schedule, reminder, browser pairing, process and setup
commands remain available. Legacy autonomy/undo controls remain compatible but do not reinstate action gates.

## 14. Testing

`.venv/bin/python -m pytest -q` runs offline contracts. `.venv/bin/python -m mypy ferb/core` checks the core layer.
`tests/test_orchestrated_control.py` covers the new decisions/execution boundary. Retired policy assertions are
listed in [the test migration](docs/TEST_MIGRATION_022.md). Remaining pure policy tests describe legacy helpers,
not a live permission boundary. Live tests require the owner's observation; do not equate fake-driver success
with task completion or no desktop disturbance. See [owner acceptance](docs/OWNER_ACCEPTANCE.md).

## 15. Current gaps

0.22 is pending installation and owner acceptance. Real model decisions about authorization and recovery still
need observation on actual tasks. Native gestures depend on OS/app support; disconnected browser extensions and
provider errors remain possible. Today's context search is lexical. Coding-agent control uses available shell/app
interfaces rather than a dedicated end-to-end coding-session adapter. There is no guarantee an external service
or an unavailable worker can complete every task. Report real blockers and preserve completed work.

## 16. Future work

Cross-device context synchronization, semantic activity retrieval, richer procedure memory and a dedicated coding
session capability can extend this design. They must preserve agent-directed routing and the owner's final-outcome
contract; do not reintroduce phrase-based permissions to compensate for a prompt or driver defect.
