# FERB architecture — 0.24.0

This is the current implementation contract. Historical contracts remain in Git; they are not instructions to
restore removed permission engines. Live acceptance and its limits are recorded in
[docs/OWNER_ACCEPTANCE.md](docs/OWNER_ACCEPTANCE.md).

## 1. Decisions and execution

FERB is one conversation with an orchestrator and task workers. Agents interpret the owner's intent, authorization,
account choice, method, recovery and final-outcome confirmation. Reading and acting share one workflow. Every new
action plan needs meaningful success conditions; invalid plans return to the model for correction.

Code validates arguments and references, binds resources to tasks, implements platform operations, redacts secrets,
records outcomes and honors stop. It does not classify request wording into permissions, withdraw tools after web
reads, delay sends, impose action counts, or ask for routine per-action approval. An OS refusal or invalid coordinate
is a fact to report, not a reason to guess another account or silently switch to foreground input.

“Send Rahul the deck” already authorizes sending. “Prepare the deck for Rahul” requires a choice once the artifact
is ready. Workers check recipients and accounts from evidence, ask the orchestrator when uncertain, and ask the
owner only when the orchestrator cannot settle the choice. Labels such as `effect`, kinds and provenance annotate
execution; they do not decide authorization.

## 2. Folder responsibilities

| Path | Responsibility |
|---|---|
| `ferb/cli/` | Terminal commands, chat rendering, RPC client, setup interaction |
| `ferb/daemon/` | Service composition, authenticated RPC, action gateway, question transport, diagnostics |
| `ferb/orchestrator/` | Plans, run/recovery loop, prompts, task state, ledger, evidence and result inspection |
| `ferb/runtimes/` | Codex/Claude process adapters, structured outputs, token/scratch cleanup |
| `ferb/registry.py` | Compose installed capabilities; do not put execution policy here |
| `ferb/act/` | Browser, Mac, shell, file and owner-question action implementations |
| `ferb/policy/` | Shared action metadata, question/request structures and source labels retained by execution |
| `extensions/chromium/`, `ferb/native_host/` | Paired extension and native messaging transport |
| `ferb/core/` | Shared typed interfaces, errors, clocks/IDs used across layers |
| `ferb/store/`, `ferb/bus.py` | SQLite stores/migrations and event delivery |
| `ferb/memory/`, `ferb/sense/` | Memory, activity collection, daily summaries and daily JSON context |
| `ferb/sanitize/`, `ferb/identity/` | Redaction, exclusions, sealed credentials and FERB's account |
| `ferb/clock/` | Timers and scheduled work |
| `ferb/decide/` | Optional Jev memory/activity enrichment; no task-permission routing |
| `ferb/device/` | Install/update, permission/device facts, diagnostic probes and watch harness |
| `ferb/voice/`, `macapp/` | Push-to-talk control and the small signed microphone app |
| `tests/` | Offline contracts, shared fixtures and explicit disposable-app live tests |
| `spikes/` | Historical experiments, outside the shipped runtime |

Older build-step and section-number comments refer to historical contracts; this document describes current behavior.
Use existing modules for a change. Split by a real responsibility when needed; do not create compatibility wrappers
for dead code or another document for each patch. Public store methods own SQL; consumers do not reach into `_db`.

## 3. Request flow and resource lifetime

`Supervisor.send` retrieves durable chat/task context and relevant memory. The planner also receives matching daily
activity, registry entries and current connection facts. A direct reply may answer a historical question, but a
request to inspect current app state needs new observation; cached findings must be identified as historical.
A task specifies its goal, worker type, preferred identity and success conditions.

`run_tree` owns an attempt and its resources. Workers get the installed registry across reading/acting and account
families; `actions.search` discovers tools. Capability failure returns findings to `_dispatch`, which selects a
new worker/approach, resolves a question or reports a real blocker. Completed artifacts and uncertain action evidence
survive recovery. There is no fixed action count. A silence watchdog detects a lost runtime; owner waits pause it.

Claimed completion runs result checks. False or unknown conditions return to the planner; they are not success.
The checker can navigate/search/scroll to inspect, and its prompt prohibits editing deliverables or sending content.
It can reuse its worker's tab through an explicit handoff without owning unrelated tasks' tabs. `task.evidence`
searches/pages actual redacted observations, including earlier views, through `SpanStore`. Historical evidence can
prove what was seen then; changing current state and image-only claims may need a fresh read.

Task-end cleanup closes ordinary task tabs, releases borrowed tabs, detaches retained tabs, and stops ordinary task
processes. The browser keep-open operation transfers a useful result to the owner without navigation or activation.
A retained class page survives task completion, reconnect and extension reload. `background=true` shell processes
survive task completion and remain listed by `ferb ps`. Cleanup outcomes/errors enter the trace and final reporting;
a release failure cannot silently coexist with a claim that a page was kept open. A lifecycle-invalidated result
returns to the existing orchestrator recovery decision. Closing chat leaves work running;
explicit stop cancels work and recovery. Restarted active tasks become interrupted, not completed.

## 4. Gateway, runtimes and questions

The Unix socket and per-run tokens identify the requesting worker/task. `Gateway.call` validates arguments, supplies
capability-family context, takes resource locks, executes, redacts and records the result. Ledger transitions are
`intended → executing → done | failed | unknown`. A lost acknowledgment may mean the action happened; agents see
that uncertainty before deciding to retry. Complete planner/dispatch decisions and failures are persisted too.

Execution state contains requests, pending questions and stop state. Retired policy evaluators, taint/exposure caches,
contact-resolution caches, undo holds and autonomy presets have been removed. Existing database history is retained;
obsolete historical columns need not become live behavior again.

`owner.ask` routes through the orchestrator, then chat/voice if owner input is still needed. Free-text corrections
remain full text rather than being coerced to the first number. OTP input is separate and sealed. Provider adapters
clean up tokens, temporary configuration and started clients/processes even if startup fails before returning a handle.
An actual provider error identifies the failed runtime, preserving detail for diagnosis.

## 5. Native Mac control

`MacComputerDriver` uses Accessibility, app-directed keyboard events, menus and window screenshots. Each input
attempt uses current control/window evidence. Per-app profiles record observed successes/failures; historical
misses do not disable a mechanism for all controls for an hour. A successful API return alone is not proof of an edit.

`mac.pointer` binds click, hover, drag and scroll to a fresh screenshot and exact window coordinates. Delivery uses
a process/window-targeted route where supported, without intentionally moving the shared pointer or activating the
app. Screen coordinates and window-local coordinates are distinct. Legacy transport limitations are reported.
`mac.click` shares this transport after its semantic AX route. A delivered event is not replayed in the foreground
merely because AX text did not change.

For scroll the agent chooses distance and batch count; the driver reports movement and stop reason. Target a unique
pane or a point inside it. An unchanged viewport is not proof of the end of a lazy-loading list. Foreground input is
an explicit tool argument. `mac.open` requests background launch and never reactivates a saved foreground app after
waiting: the owner may have changed apps intentionally. Privacy exclusions and password handling remain in force.

`python -m ferb.device.probe BUNDLE [--seed]` is a standalone diagnostic for disposable TextEdit/Calculator/Finder
checks. It belongs to device diagnostics, not the runtime action driver. Owner-supervised checks use `ferb watch`.

## 6. Browser control

`browser.*` operates an explicitly selected personal profile; `ferb_browser.*` operates FERB's default account.
The resolver does not choose a personal profile from an open URL, browser preference or last-used profile. Agents
choose from the owner's direction and current inventory. Availability facts do not remove native fallback tools.

Chrome/Brave use the extension, native messaging and per-tab debugger sessions. New task tabs are inactive in a
minimized task window. Borrowed owner tabs retain their normal ownership. Release detaches debugging, dialog and
focus-emulation state without closing borrowed/retained pages. Session updates and command claims are serialized;
a duplicate in-flight command ID cannot execute the action twice, and concurrent tasks cannot lose one another's
state or create competing task windows.

Snapshots combine the browser Accessibility view with DOM-backed references. References include tab, generation
and frame; screenshots preserve exact pixels. Container scrolling carries the frame/target through both transports.
If that target cannot be reached, the tool reports it instead of scrolling the top document and claiming the pane
moved. Account inspection is an explicit agent read, not an automatic prerequisite/helper-page gate.

Extension 0.11.0 uses protocol 6. Setup publishes `~/.ferb/extensions/chromium`; managed installations reload a newer
local version when task resources are idle. Pairing is stored identity; connection is current transport state.
Lost pairing, old protocol and closed browser are distinct diagnostics. A disconnected profile may receive a
windowless wake attempt, never an automatic visible wake tab. No stale foreground-app restoration runs afterward.
Initial unpacked installation/migration remains a browser setup step; arbitrary personal browser restarts are not
part of task execution. Local HTTP(S) pages are supported; invalid schemes/references report actual errors.

## 7. Shell and files

Files use the workspace or supplied absolute path with ordinary OS access, existence/overwrite checks and unique
edit matching. There is no task-identity folder jail or command-word permission classifier. Source/provenance
annotations are descriptive. AppleScript preserves encoding and platform errors without a script-word deny list.

`shell.start` returns a task-owned process ID immediately. `shell.wait` and `shell.read` report actual exit status
and page the complete redacted output log by byte offset. Run coding agents directly, without an untracked `nohup`
or trailing `&`, then inspect startup, session identity, exit status and artifacts. A launcher message is not proof
that the coding session finished. Process-group cancellation reaches child processes.

PTY data is decoded/redacted incrementally before persistence, including secrets split between reads. Logs are
owner-only. The initial output stays readable beyond 256 KiB. Incomplete oversized records are explicitly suppressed
to avoid leaking an unfinished secret or growing memory without limit. Safe space-terminated prompts stream immediately; unfinished words or secret prefixes remain buffered.
Unterminated final output is flushed on process exit, and capture failures are reported with incomplete output. Startup failures close PTY/file descriptors.

## 8. Context, memory and scheduled work

Watchers observe app focus, redacted titles/URLs, idle state, app installs and configured folder events. Exclusions
and pause apply before persistence. Watcher startup failure appears unhealthy; it does not masquerade as running.
`DailyActivity` mirrors relevant events/actions with stable source IDs, what/where/when, actor and task metadata to
`activity/today.jsonl`. It retains the current local day across restarts and resets on the first read/write next day.
Source deletion and sensitivity redaction propagate to this copy, including memory-forget operations.

Every turn retrieves matching memory, daily context and registry entries using bounded lexical search. Long-term
memory and SQLite task history have their own lifecycle. Distillation outages remain retryable. Jev's optional
catalog contains active memory/activity enrichment hooks only. There is no duration estimate inferred from the
union of worker scopes, since all workers receive that union.

Schedules retain the full requested instructions. Lateness and prior failure counts are facts for the orchestrator;
code does not silently skip a late task or pause it after three failures. Explicit cancellation/pause and actual
owner-input requirements still apply. Cross-device synchronization and semantic retrieval remain future work.

## 9. Debugging and validation

| Symptom | Start here |
|---|---|
| Wrong source, stale answer or unnecessary question | `ferb trace TASK --full`; planner/dispatch inputs and current prompts |
| Claimed result differs from final page state | Worker/checker observations, cleanup result, retained-tab state |
| Browser says paired but disconnected | `ferb browsers`, protocol/version and native-host logs |
| Focus/Space/cursor moved | Recorded input route, activation observations and owner verdict; separate owner movement |
| Coding agent allegedly finished | Managed process ID, exit code, saved session and artifact evidence |
| Installed behavior differs from checkout | `ferb doctor`; hash includes prompts and extension resources |
| Missing context or failed summaries | Watcher health, source IDs in daily log, distillation error/retry facts |

`ferb trace TASK --json` exposes the recorded structured trace; `--full` includes complete recorded inputs/results,
not just a display allowlist. Ordinary trace remains compact. Screenshot pixels and unrecorded provider reasoning
are not reconstructed from logs. Treat trace output as private even after secret redaction.

Run focused regressions then the offline suite and core type checks. Shared helpers belong outside test-case modules: `conftest.py` for integration/runtime support, `browserkit.py`/`chromelab.py` for browsers, and `mackit.py` for synthetic Mac controls.
Real browser tests use their own Chrome data directory/native host; they never rewrite `~/.ferb/bin` or kill all native
hosts. Real tests and fake-backed contracts provide different evidence. Installation, real model judgment, task
coverage and an undisturbed desktop need the owner's live check before acceptance is claimed.

## 10. Retained research and limits

Earlier pointer/extension investigation consulted [Apple's cursor documentation](https://developer.apple.com/library/archive/documentation/GraphicsImaging/Conceptual/QuartzDisplayServicesConceptual/Articles/MouseCursor.html),
[CGEvent](https://developer.apple.com/documentation/coregraphics/cgevent),
[Chrome debugger](https://developer.chrome.com/docs/extensions/reference/api/debugger),
[tabs](https://developer.chrome.com/docs/extensions/reference/api/tabs),
[windows](https://developer.chrome.com/docs/extensions/reference/api/windows) and
[extension update lifecycle](https://developer.chrome.com/docs/extensions/develop/concepts/extensions-update-lifecycle).
Implementation comparisons included [Cua's Mac input](https://github.com/trycua/cua/blob/67c2f39af41edc832642178868c567a1afd8d240/libs/cua-driver/rust/crates/platform-macos/src/input/mouse.rs),
[its overlay](https://github.com/trycua/cua/blob/67c2f39af41edc832642178868c567a1afd8d240/libs/cua-driver/rust/crates/platform-macos/src/cursor/overlay.rs),
[Peekaboo](https://github.com/openclaw/Peekaboo/blob/ed7f04d8ee773ef1dd4c40b519b41b731eea6710/Core/PeekabooAutomationKit/Sources/PeekabooAutomationKit/Services/UI/BackgroundInputDriver.swift) and
[MacOS-Use scrolling](https://github.com/Jeomon/MacOS-Use/blob/c88574c0a70534a21e9490e2118f1fce04e16904/macos_use/agent/desktop/service.py#L238).
These are retained research references, not promises of universal background delivery. Required attribution stays
in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Native app/OS support varies. A decorative second cursor does not itself deliver background input. Source coverage,
authorization and recovery are model judgments that can still fail. Provider/service outages and unavailable accounts
can block tasks. Preserve evidence, state the limitation and validate fixes with the owner rather than adding a new
hardcoded rule for each failed task.
