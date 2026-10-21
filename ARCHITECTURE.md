# FERB architecture — 0.27.0.dev1 (checkout; owner acceptance pending)

The [owner guide](docs/OWNER_GUIDE.md) is the starting point for understanding the code. This document is the
implementation contract. [Capabilities](docs/CAPABILITIES.md) distinguishes measured behavior from offline tests;
[owner acceptance](docs/OWNER_ACCEPTANCE.md) contains the remaining live checks. Historical designs remain in Git.

## 1. Decisions and execution

Agents decide intent, authorization, target/account/source, methods, recovery and whether outcomes meet the request.
Code owns typed validation, transport, locks, resource ownership, redaction, recording and stop. Code returns facts;
it does not switch a local source to Drive, resolve a contact by first match or infer a send from an English label.

The harness consists of daemon RPC, gateway, stores, ownership, locks, sentinel, recording and stop. The approach
consists of prompts, planning, dispatch, success-condition generation and checker judgment. All prompts are versioned
files loaded through `ferb/core/prompts.py`. `Supervisor._approach` is the single selection seam for a later verified
override with default fallback. Notification triage questions are recorded in shadow. The ordinary-activity attention
question (`trail.attention`) is live under Q4 and recorded as live: a yes lets FERB consider the moment and post a
chat notice (section 16); it never authorizes a task or a send.

<!-- source: ferb/orchestrator/supervisor.py#Supervisor._approach -->
<!-- source: ferb/core/prompts.py#load_prompt -->

## 2. Main files and boundaries

| Responsibility | Source |
|---|---|
| CLI, streaming presentation, reconnect | [`ferb/cli/chat.py`](ferb/cli/chat.py), [`client.py`](ferb/cli/client.py) |
| Composition and daemon lifecycle | [`ferb/daemon/main.py`](ferb/daemon/main.py) |
| Authentication and RPC | [`server.py`](ferb/daemon/server.py), [`auth.py`](ferb/daemon/auth.py); [client contract](docs/RPC.md) |
| Planning, attempts, questions, recovery, final answer | [`supervisor.py`](ferb/orchestrator/supervisor.py), [`schemas.py`](ferb/orchestrator/schemas.py) |
| Context and durable continuation | [`core/context.py`](ferb/core/context.py), [`continuity.py`](ferb/orchestrator/continuity.py) |
| Runtime execution and liveness | [`worker.py`](ferb/orchestrator/worker.py), [`runtimes/`](ferb/runtimes/) |
| Tool gateway and ledger | [`gateway.py`](ferb/daemon/gateway.py), [`ledger.py`](ferb/orchestrator/ledger.py), [`registry.py`](ferb/registry.py) |
| Evidence/checking | [`verify.py`](ferb/orchestrator/verify.py), [`evidence.py`](ferb/orchestrator/evidence.py), [`store/spans.py`](ferb/store/spans.py), [`act/evidence/`](ferb/act/evidence/), [`act/aftercheck.py`](ferb/act/aftercheck.py) |
| Recovery of FERB's file changes (S1) | [`act/fs/recovery.py`](ferb/act/fs/recovery.py) |
| Background observation | [`daemon/disturbance.py`](ferb/daemon/disturbance.py) |
| Browser (downloads: [`act/browser/downloads.py`](ferb/act/browser/downloads.py); upload copies: [`act/browser/uploads.py`](ferb/act/browser/uploads.py)) | [`act/browser/`](ferb/act/browser/), [`native_host/`](ferb/native_host/), [`extension`](extensions/chromium/) |
| Native, processes, files, recordings | [`act/mac/`](ferb/act/mac/), [`act/shell/`](ferb/act/shell/), [`act/fs/files.py`](ferb/act/fs/files.py), [`act/recording.py`](ferb/act/recording.py) |
| Memory, sensing, reminders and schedules | [`memory/`](ferb/memory/), [`sense/`](ferb/sense/), [`clock/`](ferb/clock/), [`schedules.py`](ferb/orchestrator/schedules.py) |
| Optional typed enrichment | [`decide/`](ferb/decide/) |
| Installation/diagnostics and voice transport | [`device/`](ferb/device/), [`voice/`](ferb/voice/), [`macapp/`](macapp/) (FERB Voice: push-to-talk and on-device transcription) |
| The Mac app (notch UI, the product) and its setup console | [`ui/mac/FERBNotch`](ui/mac/FERBNotch/) |
| Packaged FERB.app: build, signing, agent stub | [`scripts/build-app.sh`](scripts/build-app.sh), [`scripts/build-dmg.sh`](scripts/build-dmg.sh), [`scripts/signing.sh`](scripts/signing.sh), [`ui/mac/FERBAgent`](ui/mac/FERBAgent/) |
| Chat events: acknowledgement, live line, front window, hearing | [`acks.py`](ferb/orchestrator/acks.py), [`progress.py`](ferb/orchestrator/progress.py), [`sense/front_window.py`](ferb/sense/front_window.py), [`act/listen.py`](ferb/act/listen.py), [`sense/transcribe.py`](ferb/sense/transcribe.py) |
| Measurement: regression runner, security test harness | [`eval/regress.py`](ferb/eval/regress.py), [`eval/security/`](ferb/eval/security/) |

Public store methods own their queries. Keep edits within these responsibilities. Split a module only for a real
ownership boundary; do not add wrappers to preserve dead behavior.

## 3. Request and resource lifetime

`Supervisor.send` preserves the current message, including pasted text marked as external data. The planner receives
full request text, memory, retrievable chat/activity and installed environment facts. Its task specification has
outcome conditions, worker type and chosen identity. Intake memory writes produce receipts before the worker starts.
Native clients may pass selected files as `chat.send.input_paths` after `chat.attach` advertises that capability.
Paths are normalized, retained through clarification and merged into plan inputs subject to existing protection
checks. They are source metadata wrapped as untrusted data, never extra owner words or destructive authority.
Final redacted answers, verification and artifacts persist in task/root-span outcomes; `trace.get` supplies them
after reconnect. `chat.status` includes pending questions' root task IDs when unambiguous, and accepts `peek` to
avoid consuming unseen results while polling. The daemon remains the execution and security boundary for all clients.
The planner-context snapshot attached to a task holds the planner's situation blocks (the front window included,
L28) and its optional blocks, never the conversation or resumable attempts; workers fetch history with
`chat.history`. It is written right after the plan line goes out.

`run_tree` records the full request and specification, starts the sentinel and worker, and retains artifacts and
uncertain actions across recovery. Workers get the installed tool registry; `actions.search` supports complete paged
discovery. `_dispatch` decides retry, question or finish. There is no fixed action or attempt count or task deadline.
A liveness watchdog observes the runtime; registered owner/process/sleep waits do not count as silence.

A worker’s completion report is a claim. Literal predicates are collected as mechanical observations; every condition then receives checker judgment
against the full request, including account identity, artifact quality and uncertain outcomes. The checker receives the full request, specification,
intake receipts, artifacts and actual evidence, starting with the worker's recorded tool calls and their results
("The checker's starting evidence"). It must judge the state where it exists, not require a redundant
worker action or paths repeated in prose. Uploaded/sent artifacts require source/content identity evidence.

A task's tabs are known by tab id (`task_tabs`): a borrowed tab keeps the page it held when the task took it over
(`taken_over_from`, stored redacted), wherever it navigates; one the browser reports removed (the owner closed it) is
listed as closed and never driven again, and reopening it is a new tab. The checker's tab context
(`_browser_context`) is redacted like every tool result.

Cleanup stops recordings before their target is released. Ordinary task-owned tabs close; borrowed/kept-open tabs
detach without focus restoration. Ordinary task processes stop; explicitly retained background processes survive.
Cleanup observations feed resource reconciliation. They cannot erase existing failed checks. Final reporting lists
every false/unknown condition verbatim before any established facts, and suppresses unchecked success prose.
The sentinel finishes after task cleanup. Disconnect does not stop work; explicit stop cancels it. Restart marks
active tasks interrupted and unresolved ledger effects unknown, never replays them as though they failed harmlessly.

## 4. Context, output and evidence

The token-budget assembler uses whole blocks, protects the current request and required task data, and marks omitted
optional blocks with a fetch reference. A protected overflow is explicit instead of clipping. The exact assembled
runtime context is recorded. Provider context/output capacity remains a provider constraint; a refusal is not a
truncated success. Brevity instructions apply to status reports, never requested artifacts.

The shared `interaction.md` prompt defines direct, natural owner communication. Structured supervisor, worker,
recovery-dispatch and ordinary-question outputs provide `display_text` separately from the complete answer/report.
This is a same-generation presentation layer, not truncation or an extra rewriting request. The task verifier judges
both the full answer and proposed display under the report condition; verified success uses its checked snapshot.
Recovery displays retain code-selected incomplete/unconfirmed status. Original answers, condition findings, evidence
and artifacts remain available in Details and task inspection. Missing presentation fields fall back to the original
for historical/older output. Consequential approvals and secret prompts keep their full concrete scope.

Conversational text streams through `RuntimeEvent.text_delta`, `WorkerRuns._run`'s planner-only callback and
`orchestrator/reply_stream.py`. The incremental JSON reader buffers full `text` and requires every action/control
field before streaming the last `display_text` field of a pure reply/clarify. `Redactor.stream()` holds incomplete
secret patterns across chunks; the D33 line guard withholds ambiguous line prefixes before marking forged FERB
labels. Invalid/cancelled drafts emit `reply_abort`; complete schema validation and the canonical reply remain
authoritative. Action-bearing turns stay buffered through execution/verification. `docs/RPC.md` describes client
replacement/interruption semantics. Native history retains both display and full answer without recording secrets.

Codex schema-bearing supervisor turns and tool workers use the pinned app-server in a disposable
configuration home and ephemeral thread. Supervisor replies use its final-answer delta protocol; worker
corrections use `turn/steer` with the exact active turn ID and retain the same process/thread. One stdout reader
routes RPC responses and model events, so steering never competes with the event consumer for a read.
Workers expose only the authenticated FERB MCP shim; supervisor turns expose no action tools.
Subscription identity, isolated instructions,
read-only sandbox and observed tool catalog are checked. Claude SDK streams only its StructuredOutput carrier;
direct OpenAI, Anthropic and Google supervisor calls assemble provider SSE events through their existing final
parsers. Provider errors, incomplete streams and transport cancellation cannot certify a draft. An emitted draft
prevents an adapter from silently retrying and concatenating a second answer.

The disposable Codex process copies only FERB's subscription auth and publishes refreshed credentials immediately
after `account/read`, before model work begins. Publication advances the comparison baseline so a later refresh
can also be saved at teardown; changed source credentials win. Startup checks remain independent so a stalled
check cannot block other requests. Concurrently starting or already-running isolated processes can still refresh
independently. As with
independent Codex processes writing the shared auth file, the comparison is not a filesystem compare-and-swap:
a separate process can race the final comparison/replacement. No auth bytes enter diagnostics or traces.
Account-check failures retain only fixed error categories and numeric RPC codes, and reach chat as model-service
errors. Confirmed missing/invalid login points to `ferb setup --codex-login`; an unknown failure does not claim
expired credentials. No task or model turn is replayed by this handling.

Owner corrections route through `orchestrator/steering.py` to the active task tree. Chat intake is serialized until
each message is registered, while workers continue independently. `steer` (and `continue` on an active task)
updates the redacted durable request and result conditions, then calls `AgentRuntime.continue_` on existing member
runs. It never calls stop/cancel/revoke or changes identity grants or the plan graph; a standing scope changes only
through the steer's required scope decision (section 16). All plan members share correction state; future workers
and retries read it too. A steer settles only the open questions the planner names as changed (`supersedes`): a named
`owner.ask`/`plan.ask` gets the owner's words; a named FERB check or hidden input closes as `superseded`, never as a
No, and is asked again if the action recurs; every other question stays open (L3). Unrelated requests use their own
tree; stop/retry retains
explicit cancellation semantics. Corrections cannot undo an action already in flight.

Revision checks prevent a result captured before an undelivered correction from finishing the task. Worker
startup, verification, cleanup, recovery dispatch and failed plan completion retain the update for the next
decision. A task already in finalization completes naturally before its saved context is continued, without
requesting cancellation. Delivery errors are recorded and acknowledgements distinguish accepted live input from
pending input. Claude uses UUID user-message replay to keep receiving through stale/merged results; direct HTTP
adapters append ordered user messages at the next model decision, preserve started tool outcomes and skip stale
unstarted tool calls. Neither path replaces the runtime to accept a correction.

The planner's context window and order are as built in the launch week (L28, section 16): the budget is the
model's real window, the situation always goes in, and chat fills the rest newest-first. Resumable attempts
(`ATTEMPTS_TOKENS` 2000) keep their newest entries whole with a note about the older ones. Chat tags count from the
oldest (`c1`) so earlier lines never change as the conversation grows; the owner's message is last (P20). The
F1 `HISTORY_TOKENS` 12000 cap and the 32,768-token default budget are gone. The Codex catalog lockdown sets
`supports_search_tool` false so FERB's tools are directly visible; a worker or checker whose runtime shows no FERB
tool loaded is re-run once, traced as `runtime.tools_not_loaded` (`ferb/orchestrator/worker.py`).

`task.evidence` pages the full redacted observations, reports, intake and registered task-root context sources.
`chat.history` and `memory.read` retrieve complete source text and candidate pages. Browser/native snapshots and
file/shell reads expose page cursors or full-source routes. File writes do not impose a deliverable byte cap.
Incoming RPC frames retain an explicit 1 MiB bound: oversize is refused, never cut and executed. Page sizes and
provider transport bounds are documented in the audit’s limits ledger.

The trace forest includes the pre-task planner by exact request ID. `trace --json` also exports one structured
outcome with request, plan, actions/results, verification, disturbances, cleanup and final records. `ferb verdict`
(RPC `verdict.set`) appends the owner's pass/fail to the `owner_verdicts` table for a root task; the export shows the
latest one as `owner_verdict`, and null means no verdict, not acceptance. Compact CLI previews use `--full`/`--json`
for all fields. Redacted secrets, and images the evidence store keeps as facts only (section 11), are intentionally
unrecoverable from tool text.

`ferb/eval/` holds the 0.27 measurement tools. `ferb regress` submits a TOML task set (default
`ferb/eval/sets/smoke.toml`) in one fresh conversation, declines owner questions, writes one row per task to
`~/.ferb/regression/` and never decides pass or fail. `ferb baseline` reads the database read-only into
`~/.ferb/baselines/<timestamp>.json`. `ferb/eval/security/` has canary tokens, a localhost request logger and
`scan_trace`, which flags canary egress and unexpected recipients in an exported trace.

## 5. Gateway, questions and stop

The owner-only Unix socket authenticates client roles. Worker run tokens are scoped, expiring and revocable.
`Gateway.call` validates, binds the task, locks the actual resource, records the action, invokes the chosen tool and
returns redacted facts. Canonical filesystem locks preserve the exact requested path. Ledger transitions distinguish
intended, executing, done, failed and unknown delivery. A delivered event is not proof of the requested outcome.
Code refuses a consequential call identical to one in the same task tree whose outcome is unknown or still running;
only the owner's Yes allows one more attempt (P19, section 11).
Each `tool.call` span records the action `kind` and its non-empty `target` fields, so exported traces can be scanned
offline. `Gateway.test_send_allowlist` is for tests only: when set, send-class actions (by kind, declared purpose or
declared effect) to unlisted or unidentifiable recipients are refused before FERB asks anything and before the
ledger; it is None (inert) in normal running. An entry is an exact recipient, or `"SITE @ACCOUNT"` (a bare `"SITE"`
for a post that declares no account): a recipient-less declared send passes only on a page whose host (a fact) and
declared account match, never a payment; a URL-shaped entry is refused (`allowlist_problem`). Undeclared GUI typing
into a messaging app is kind `mac_input`, not send-class, so the allowlist does not cover it. Before a consequential
call runs, code may ask the owner itself (section 12, "Confirmations").

Ordinary `owner.ask` first goes to the orchestrator with context. If what/who/which remains ambiguous, the owner gets
candidates and their evidence. Before a hard-to-undo action, preparation and target confirmation are concrete; an
already-given confirmation carries across retries. Foreground permission follows the same task-wide principle.
Free text is preserved; duplicate option labels are not resolved to the first. OTP is a separate sealed path.

Questions never expire (L3, spec section 8.1): a question stays open while its task waits and closes when it is
answered or its task ends; silence is never consent. The worker/checker silence watchdog currently polls
at 5 seconds and detects 180 seconds without activity; no-tool provider calls have a separate 900-second liveness
bound. These are transport/liveness safeguards, not task-duration or artifact-length goals. Stop revokes tokens,
cancels active handles, stops owned inputs/processes and persists across restart until explicitly lifted.

## 6. Background interaction and observation

The model chooses AX actions/values, PID-directed events, non-activating scripting, URL schemes, extension/CDP or
shell/file operations according to evidence. Native input takes one explicit mechanism. Missing AX actions, target
ambiguity or an unsupported route return facts; no automatic ancestor press, synthetic Return, append-to-value,
global replay or focus restoration follows. Global foreground input requires the agent’s explicit tool choice and
owner direction. Arbitrary shell/AppleScript remain powerful; the shared prompt and sentinel cover indirect routes,
without a brittle denylist of command words.

Automation preflight runs in the daemon process on dedicated daemon threads, sharing each pending check by
target and ask/passive mode. macOS's `AEDeterminePermissionToAutomateTarget` can hang even with `ask=False`;
the event loop and default executor never wait synchronously for it. Tool cancellation stops awaiting the
check and cannot trigger late script execution. The existing caller-selected AppleScript timeout includes
permission preflight and sandbox preparation; an unanswered check returns unknown/not performed. Expired
calls cannot spawn a script. Started script processes are killed and drained on timeout or cancellation,
including cancellation during process creation. Passive tab sensing skips an unresolved check, while explicit
setup may still request permission. An in-flight native permission call itself cannot be cancelled safely;
it remains shared until macOS returns or the daemon exits, without holding shutdown open. Cancellation while
the existing `sandbox.wrap` thread builds its profile remains a separate cleanup limitation.

`DisturbanceSentinel` samples frontmost app, focused key-window identity, cursor, focused-window full-screen state,
visible window order and public Space-transition notifications throughout every task. It records action boundaries,
250 ms periodic observations, overlapping actions, owner input, missing fields and measured sampling time. It neither
blocks actions nor asserts authorization or causal certainty. Short changes between samples may be missed. Public
Space notifications give transitions, not stable Space IDs. There is no private-API Space/window manipulation.

**On-screen actions and what came forward (P26, P4; F4 live fixes).** A capability that can change what is on the
owner's screen declares `on_screen = True` (`gateway.on_screen`; a declaration like `observation`, never a name list,
rule 6): `mac.open`, `mac.menu`, `mac.act`, `mac.click`, `mac.pointer`, `mac.applescript`; in both browser families
`open`, `navigate`, `act`, `scroll`, `dialog`, `close`, `keep_open`, `use_tab`, `login_code`, `fill_secret` and
`fill_otp`; `shell.start`, `shell.run`, `shell.send` and `shell.kill` (a command FERB runs can open an app). Nothing
else is, `shell.wait`/`read`/`list` included, so waiting on a coding agent never puts the agent's changes near a FERB
action. The gateway passes the flag to `before_action`, so every action in a desktop record carries it; FERB's own
cleanup is observed as one on-screen action, `task.cleanup` (`Supervisor._end_task`; an observation failure never stops
or fails the cleanup). "Came to the front" is stated only when a readable front app differs from the last readable one,
and "the Space changed" only for a readable Space count that differs from the last readable one (`lines.CameForward`,
one rule for the sentinel and the trace); an unreadable front app gets `front_app_unreadable` in the raw record and no
statement. The probe reads the front app's name (`localizedName`); a readable switch also records the FERB-started
processes running then (`processes`: `Daemon._running_processes`, the `ferb ps` data, commands redacted, rule 14;
another task tree's are kept by id only, without their command, `_for_tree`). With no on-screen call of any task in
flight or ended within two sample intervals (0.5 s), the record adds `on_screen_in_task` and `last_on_screen_ended_s`
(tenths under 10 s, rounded down; whole seconds from 10 s, on the daemon's clock), and the sentinel and the trace
compose the statement from the record (`lines.no_on_screen_action`): "<App> came to the front — no FERB on-screen action
in progress (the last ended N s earlier); running: <command> (started by this task); <id> (started by another FERB
task)". When this tree made none it reads "(none in this task; the last of any FERB task ended N s earlier)", or "(none
yet)" when none has ended since the daemon started; "running: nothing FERB started" when the list is empty and "running:
not recorded" when listing failed. The agents' context (`DisturbanceSentinel.context`) carries the count as
`front_or_space_changes_with_no_ferb_on_screen_action` and the newest 10 statements as `latest` (`NO_ON_SCREEN_KEPT`);
every change stays in the records. The episode's `desktop` keeps the statements as `no_on_screen_action` and near
changes as `near_ferb` (`tools`, plus `other_task_tools`, rendered "<tool> (another FERB task's call)"); the story lists
both by time under its "Desktop:" line. Code states facts, never who caused a change (rule 6; controller ruling
2026-10-09): a change may come from the owner, another app, a process FERB started (a coding agent acts with its own
permissions) or a FERB effect that landed late, and [`interaction.md`](ferb/orchestrator/prompts/interaction.md) tells
the agents to weigh the elapsed time and the running processes. FERB never instructs or restricts another agent's
harness (owner, 2026-10-09: "i dont want this hardcoded control over other agent's harness. ferb should just know that
it didnt do it"). A record made before the flag keeps the old rule (`NO_SCREEN` in
[`trace/compose.py`](ferb/trace/compose.py): `shell.*` and `fs.*` calls and recorded reads never count as near) and gets
no statement from its kinds. Residual: a FERB effect that lands more than 0.5 s after its call ends (an app finishing
its launch) is stated as "no FERB on-screen action in progress", with the seconds since FERB's last one ended for the
reader to weigh.

While the owner has paused sensing (`ferb pause`), the sentinel reads and records nothing about the desktop (`paused=`,
wired to the sensing pipeline's pause): each running task's record gets one `disturbance.paused` (the reason only) and
then `disturbance.resumed`; a read under way when the pause begins is dropped; `context()` returns nothing; and the
first sample after `ferb resume` is a fresh baseline, so a change made while paused is never reported as a change. The
story says "desktop not observed (paused) until HH:MM:SS" (the episode's `desktop.paused`). FERB's own tool-call records
are unchanged.

[Apple’s Space notification](https://developer.apple.com/documentation/appkit/nsworkspace/activespacedidchangenotification)
is observed through the workspace notification center; window ordering uses
[public Core Graphics observations](https://developer.apple.com/documentation/coregraphics/cgwindowlistcopywindowinfo(_:_:)).
Per-app support is evidence-based. A background request does not prove an app respects it; see capability caveats.

## 7. Browser and sealed identity

The model selects from profile candidates; display names, email hints and directory identities are returned together.
No last-used/default-owner-account resolver or first-tab account extractor chooses for it. FERB’s own profile remains
a stated approach preference for tasks that call for it, not permission to replace an explicitly requested source.

Extension **0.15.6 / protocol 17** uses native messaging, paired sessions and per-tab CDP (protocol 14 adds
`media` and replaces `window_password` with `window_secrets`; 0.15.3 treats shown passwords as secret fields;
protocol 15, fix wave 4, makes `window_secrets` tell same-place windows apart by the captured window's title and
name the matched window's tab; protocols 16–17 let tab captures of pages it can't script use the debugger alone and
answer for pages it can't read or attach to, see section 16.4). Task
tabs are inactive in an existing FERB window or an explicitly selected window. If none is selected/owned,
open returns all window candidates without creating anything. New-window creation requires explicit foreground
permission: macOS can switch Spaces before minimized/unfocused creation finishes. Borrowed tabs retain ownership. Exact tab/frame/generation references avoid stale-target reuse.
Local uploads use `DOM.setFileInputFiles` on the identified input, including supported iframe/OOPIF routes. Canonical
path, size, modified time and SHA-256 receipts tie the chosen local bytes to the selected files. Unknown/unsupported
routes return structured facts; no Google Drive replacement is performed. Remote attachment identity still needs
model inspection and final observed content evidence.

Sealed `browser.secret` accepts optional ordered `split_elements` from a fresh snapshot when automatic OTP
classification cannot recognize a widget. Python binds distinct textbox IDs to one tab/frame/snapshot before the
hidden prompt and rechecks them afterward. The extension validates origin, exact connected/editable fields and
code length before writing and preserves unknown outcomes if the page mutates during input. After explicit
split input, editable values/text/labels remain masked for that document across node replacement, with AX merge
preserving the masked labels. This capture state does not change ordinary field action classification, so unrelated
search inputs remain editable. Screenshot and recording paths read the page's secret-field state before and after
capture and paint over those fields (section 16, L13). No site-name heuristic or broader automatic OTP
classification is introduced.
The locally signed helper is verified with codesign's inline requirement syntax (`-R=`), retaining fail-closed
signature checks. Protocol 13 (and 14 since) prevents an older extension silently ignoring explicit split targets.
The background fill callback also
requires the current page helper’s explicit split-fill capability before writing, so a stale isolated helper refuses
without falling back to filling only the first field.

The browser reads FERB's copy, never the original (rule 17, MP2; F4 live fixes,
[`uploads.py`](ferb/act/browser/uploads.py)). macOS asks the owner, in front of their work, when a browser reads a
folder it protects (Desktop, Documents, Downloads, iCloud Drive; task `79bc5bfa`); FERB's workspace is not one.
`_upload` opens each original once without following a link at its last step and reads its identity from that handle
(`_TabDriver._sources`, one worker thread), then `Uploads.stage_all` copies it from the same handle to
`~/.ferb/workspace/uploads/<tree>/<random>/<original name>`, or under `uploads/outside/` when the original is under an
untrusted fs root: that folder is itself an untrusted root code adds (`downloads.untrusted_roots`), so the copy keeps
the original's outside label. Every folder from `uploads/` down is held by a handle opened `O_NOFOLLOW`, and the copy is
made into its folder's handle (`recovery.copy_into`): `fclonefileat` with `CLONE_NOFOLLOW` (an APFS clone), else a byte
copy opened `O_EXCL|O_NOFOLLOW`, so anything already at the name, a planted link included, refuses the copy; the copy
must hash to the identity just read and still be a regular file at the path given to the browser. The result's `files`
keep the originals' identities and `staged_copies` lists what the browser read; the site sees the original name; the
original is only read. Any failure refuses the upload with nothing given to the browser, and copies already made for it
go at once. Copies used only in tabs FERB closed go when the tree ends (`Daemon._tree_finished` → `forget_tree`). A copy
used in a borrowed or kept tab stays until the browser reports that tab removed (`Hub.on_tab_closed` →
`Daemon._tab_closed` → `collect`) or until the next start-up (`sweep`), since Chromium reads the file again at submit.
Cleanup never follows a link: every removal goes through the handle of the folder that holds the entry; a link in place
of a tree folder is removed as a link, and one in place of `uploads/` itself is left alone and reported. What a cleanup
couldn't remove is journaled (`browser.uploads_cleanup_failed`, `browser.uploads_startup_failed`, with `left`: paths and
errors, never contents). Residuals: after a daemon restart (`ferb setup --update`) the start-up sweep removes the staged
file of a tab still open, so a form submitted there sends no file; tab ids repeat after a browser restart, so closing a
new tab that reuses an old id can delete an old copy early. FERB's shell can write the workspace, so a command could
change a copy before the browser reads it; the browser-side `bytes_match` reports different bytes (carried to F5,
section 11).

Downloads (D35) never reach the browser's own download flow in a live task's tab (opened, adopted, or borrowed while the
task holds it). The tab is armed before any URL loads (`arm` in `background.js`: a task tab opens blank, becomes the
task's, then navigates; a tab the debugger can't attach to, such as a new-tab or `chrome://` page, steps through
`about:blank` first, and only then is the URL refused as a structured `download_capture_unavailable` fact). Its debugger
session pauses document responses at the response stage (`Fetch`, child-frame sessions too) and
[`downloads.js`](extensions/chromium/downloads.js) classifies each as the browser would: `Content-Disposition:
attachment`, a renderer-started download (`<a download>`, whose request alone lacks `Upgrade-Insecure-Requests`, plus
its redirect targets), or a type a page can't show. A download is held, announced as a `download` event and read with
`Fetch.takeResponseBodyAsStream` + `IO.read` in 72000-byte `download_part`s the daemon pulls (never in the command
journal), then failed (`ERR_ABORTED`: the page stays). `content.js` holds a blob/data `download` link in the capture
phase (a page's `stopPropagation` can't hide it), and in the bubble phase (after the page's own handlers, so a click the
page cancelled stays cancelled) routes a trusted click on a link to a new tab (`target=_blank`, also via `<base
target>`, a modifier or middle click; other named targets stay the browser's) through FERB's own open as the same task's
tab, released like its opener when that is the owner's borrowed tab, within `browser.task_max_tabs` (50) per task tree
(past it the browser opens an ordinary tab, as a fact). The service worker answers content.js only once the daemon has
answered the `open` event (`open_seen`, which the daemon awaits before opening anything) or after `OPEN_WAIT_MS` (1 s);
with no answer (the daemon down, restarting or just slow) content.js lets the browser open the link, and a late
`open_seen` gets `expired`, so the daemon opens nothing: the link opens at most once; it can be lost only if the
connection drops in that same moment. After an extension reload (`ferb setup --update`) a document's old content.js
can't reach the service worker (`chrome.runtime.id` gone, sending throws): it stops taking clicks and answering
`ferb-armed`, and a click it had already taken goes back to the browser (`passOn`, `releaseDownload`). The page hook (`PAGE_HOOK`, pinned by exact source in
`protocol.js`; installed only in armed task tabs and their OOPIFs with `Page.addScriptToEvaluateOnNewDocument`, removed
on release) asks content.js through a cancelable `ferb-armed` event before doing anything, so a still-patched document
is native once FERB lets go. It clicks a FileSaver-style save link (never added to the page) itself while briefly
connected, so the capture listener takes it, and opens `window.open(url)` (a non-empty http(s) URL, no target or
`_blank`, without `noopener`; anything else is native) as `about:blank#ferb-<token>` (`crypto.getRandomValues`, so
plain-http pages work). FERB adopts a new tab only when it opened it itself (`browser.open`, a routed link) or when all
three hold: the tab is new (`chrome.tabs.onCreated`, and this is its first URL report, there or in its first
`onUpdated`; a later navigation of any tab never counts), that URL is exactly `about:blank#ferb-<token>` for a token a
live task tab relayed (either may come first; 2 s each; any window, so popup windows with features count; 20 relays per
opener tab, 100 in all, one per token; the tokens already relayed are bounded the same way), and, once attached, the browser itself names that task tab as the opener: the
popup's `Target.getTargetInfo` `openerId` equals the task tab's own `targetId` (measured for a popup tab, a popup window
and a popup opened from a cross-site iframe, whose `openerId` is still the tab; a popup of another tab names that tab).
No match, or no answer: detached at once (`chrome.debugger.detach` directly: nothing was enabled on it) and never
reported, not even if the owner cancels the debugging bar during the check; if it was the hook's own popup whose opener the browser
didn't confirm, the hook loads it natively after 1.5 s, and any other tab at that URL stays with whatever opened it. The
allowlist admits `Target.getTargetInfo` with no params only (another target's is also refused by Chrome, "Not allowed")
and returns just the two ids. Chrome's `openerTabId` names the window's active tab (measured), so it is never used. So a
tab that existed before can't be claimed, even by navigating itself to the magic URL while a frame in a task tab relays
the token. A new tab or popup that the owner's own page opens is never touched: only armed task tabs have the hook, so
such a tab is never at a relayed URL unless a page put it there on purpose, and then FERB attaches only long enough to
read its opener. FERB never navigates a popup: once the daemon has made it the task's (within the tab cap, `attached`
from its opener) it sends `popup_ready`, content.js fires `ferb-popup-ready` (and answers, so the result says the page
was told), and the hook sets the popup's location once, only if the popup is still exactly at its blank URL (a page that
navigated or closed its popup meanwhile keeps its own choice); without the ready, the hook does the same after 1.5 s. A
page faking `ferb-popup-ready` only loads its own popup early. Measured in the headless lab on a plain-http,
non-localhost origin: a popup tab's and a popup window's downloads are taken on the ready path (the page saw
`ferb-popup-ready` well under 1.5 s after `window.open`), and an OAuth popup's `window.opener.postMessage` reaches its
opener. Ownership, not the extension, decides: [`downloads.py`](ferb/act/browser/downloads.py) takes a download only
while `Hub.task_tab` finds a running task's tab, else it is released back to the browser untouched (`download_part
{release}` → `Fetch.continueRequest`); the hello lists armed tabs and the daemon disarms any it doesn't hold (a failed
release, a restart); an unpulled hold goes back to the browser after 30 s (in FERB's own profile it is failed); closing
FERB's own tab fails what it holds, letting go of a borrowed one hands it back. A refused download (over
`browser.download_max_mb` 512, the task tree's `download_task_max_files` 50 / `download_task_max_mb` 4096, a transfer
that would leave less than `browser.download_min_free_gb` 10 free on the disk, a second transfer in a tab waiting over
20 s, or a 10-minute transfer) is failed in the browser, so no dialog. The free-disk floor is 10 GB (as macOS shows it; space macOS can
purge doesn't count; `shutil.disk_usage(...).free`), checked in `Downloads._admit`
before any byte moves (by the declared size; with no declared size, whether the disk is already under the floor) and
in `_take` before each part is written; the refusal says how much is free ("only 8.2 GB free now; FERB keeps at least
10 GB free (browser.download_min_free_gb): …; not taken"), and free space that can't be read refuses the download too.
Once the tab is a live task tab, any other admission error (a half-edited `config.toml`, a database error) aborts the
download in the browser first and then records a `failed` fact ("FERB couldn't check this download against its limits
(…); not taken"; `_not_admitted`), so it is never handed back to the browser. The daemon writes
`~/.ferb/workspace/downloads/<id>-<safe name>` through a no-follow folder descriptor, hashes the received bytes, removes
partial files (and sweeps orphan `.dl-*.part` at startup), and records facts in `downloads` (migration 0025:
`in_progress` → `taken|refused|failed|not_taken`; source URL and URL-derived names through the redactor, which covers
signed-URL credentials; never content). The worker sees them in the open/navigate result (it waits for the transfer),
the next snapshot or act (with `opened_tabs`), and `browser.downloads` (an observation tool). The folder is always an
untrusted fs root (`external:download`). Files are FERB's working copies and stay until the owner deletes them; only
while the folder is over `browser.download_total_mb` (20480) do the oldest move to the Trash (`files.trash_item`
through the downloads folder FERB holds open, the hourly pass; never by path); rows stay with `trashed_to`. There is
no time-based clearing. These limits are the owner's decision of
2026-10-09 (section 11 has the one-time upgrade of the values F4 had written into `config.toml`).

Residuals (measured in the headless lab; nothing tab-scoped closes them), recorded `not_taken` from the tab's own
`Page.downloadWillBegin`/`downloadProgress` when FERB is attached (a later `canceled` too, e.g. a dismissed Save
dialog), and able to show a Save dialog: a held response whose debugger detaches (the owner's Cancel on the debugging
bar, the extension's service worker crashing) continues into the browser's download flow (one whose body FERB already
took does not); a new tab FERB didn't open or claim (a popup with `noopener`/`noreferrer`, an empty or named target, an
untrusted click on a `target=_blank` link, a form posted to `target=_blank`, a link in a frame content.js wasn't
injected into, a popup whose opener the browser doesn't confirm, a popup past 20 relays from one tab in 2 s) is the
browser's, unarmed and unreported; a hooked popup the daemon takes longer than 1.5 s to adopt is loaded by the hook
first, and a download it starts at once goes back to the browser (released, never failed); an OOPIF's first document
before its session's `Fetch.enable`; a type FERB lets render that the profile downloads (a PDF with "download PDFs" on,
a sniffed type); a script-made save before content.js is injected; and a page's own save picker (`showSaveFilePicker`).
More than 20 page saves held at once in one document (or 20 in a tab, 100 in all) are dropped, neither FERB's nor the
browser's; the daemon records a `refused` download for it, one per document per minute (not one per dropped save).
With the daemon down or restarting while a task drives a tab, a download there continues into the browser's own flow,
and after a restart the tab stays armed until the extension's next hello, when the daemon disarms it
(`Hub._disarm_unheld`); a link to a new tab from it opens natively after 1 s (as it does whenever the daemon answers
later than that). In an owner profile a held download the
daemon doesn't start pulling within 30 s goes back to the browser, and releasing a borrowed tab hands back the
FERB-started downloads not yet streamed; each can show a Save dialog. A page
can tell FERB drives it (the patched functions, the `ferb-armed` answer) and can opt itself out of capture by answering
`ferb-armed` itself or cancelling its own clicks: no escalation, its downloads then follow the browser's own flow.
`Browser/Page.setDownloadBehavior` and `Browser.cancelDownload` are refused to extensions, and the tab session's
`Target.setAutoAttach` doesn't attach popups. A hooked popup shows `about:blank#ferb-<token>` until its page loads it,
so same-origin pages (the opener, its frames) can read that token from the popup's URL for that moment; the token buys
nothing but a claim of that very popup, which they opened. If a page navigates its popup itself and FERB's ready arrives
before that navigation commits, the popup still reads as blank and the hook loads the original URL over it.

Dialogs are explicit pending observations and explicit accept/dismiss choices. Email uses observed browser primitives,
not a fixed Gmail compose/send recipe. OTP inbox candidates have opaque task/account/origin-bound IDs and masked
context; no latest-message/year/sender-substring selection is treated as meaning. Raw secrets remain sealed.
Managed extension reload is version/protocol transport maintenance, deferred while task resources are active.

## 8. Memory, sensing, schedules and Jev

Owner-stated memory and observed proposals retain source provenance. Retrieval is an explicit search, not authority
to pick a file or recipient. Complete items are available through `memory.read`; proposal and narrative text are not
silently clipped. `query_terms` (`ferb/memory/items.py`) tokenizes a query and `Memory.recall` drops terms that match most memories; the FTS
index uses `unicode61 categories 'L* N* Co M*'` (migration 0014) so Devanagari words stay whole. Observed items
credit identical content to its matching item first, and otherwise accumulate evidence by kind/subject/key only while proposed; an active one matches identical content only. Daily
summaries retain source handles; raw activity retention requires a persisted summary before purge. Privacy exclusions
and redaction deliberately omit protected data and are separate from context budgeting.

Calendar arithmetic operates on model-supplied typed values. Missing local time or invalid timezone is reported;
there is no hidden 09:00 default or one-year horizon. Schedule recipients remain as the model recorded them; code
never replaces “me” with the first remembered email address. Recurrence, late runs and prior failures are facts for
the next planner. A failed run does not silently pause a schedule.

F7 composes passive sources in `sense/perception.py`: workspace lifecycle and per-app AX observers, the existing
listen-only keyboard tap with per-field text aggregation, general-pasteboard change counts, read-only Notification
Center SQLite, and installed-software security facts. Native metadata and text are redacted before crossing the
callback/thread boundary. Secure Input, unknown/secure AX roles and concealed/transient pasteboard types drop
capture before release. Held input is cleared on pause. Exact successful `mac.clipboard` change counts mark FERB's
writes; a frontmost app is context, not proof of the clipboard writer. On macOS 15.4+ the clipboard watcher reads
contents only when `NSPasteboard.accessBehavior` is `alwaysAllow` (I3; reading `changeCount` and `types` never asks), so
it never raises a surprise paste alert; any other state is journaled once per change (`clipboard.access`), `ferb doctor`
warns with the fix, and `ferb setup` offers one owner-approved read (`clipboard.request`) so macOS asks once and lists
FERB in Paste from Other Apps, where the owner sets Allow. Typed/clipboard content expires after seven
days in Trail, its FTS index and the activity copy, while source IDs remain; this automatic expiry does not retract
derived memory. Native schemas and two-app subscriptions await owner acceptance.
AX observer callbacks use PyObjC's explicit retained-callback bridge and remain strongly referenced until their
run-loop sources are removed; subscription failures expose redacted error details through `sense.status.failed`.
Workspace and AX watcher threads fill the unused portion of their existing 0.5s/1s native run-loop slices with
an interruptible event wait. An empty CFRunLoop returns immediately rather than waiting for its requested duration;
this fallback prevents idle CPU spinning, including when all AX subscriptions are unavailable. A full native slice
adds no extra delay, and stopping interrupts the fallback wait before observer cleanup.
Passive software inspection never invokes `sfltool dumpbtm`: on the owner's Mac it opens an administrator prompt
even for a read. Background-task inventory is explicitly unavailable without a supported noninteractive source;
signature, Gatekeeper, entitlements, download metadata, readable TCC grants and associated LaunchAgent files remain
available. The first software observation also supplies its persistence fingerprint, avoiding duplicate probes.

Migration 0032 adds durable batch receipts, external notification IDs, owner-quoted rules, told receipts and owner
measurement labels. `sense/batches.py` sends roughly 10k estimated tokens per app group, interrupts for a notification,
and otherwise flushes when its oldest event reaches `sense.batch_age_s` (default 60s, status reports the configured
value). Oversize ordinary events have labeled excerpts and `trail:<id>` retrieval references; full redacted content
stays in Trail. Notification context keeps the current notification, quoted rules and told receipts; only oldest
conversation history may be omitted, with counts and retrieval references. A 32k estimated-token guard includes
questions; over-limit requests are recorded unavailable without partial JSON. `trail.sensitive` is retired in favor
of code redaction. `notification.attention` cannot be enabled; its owner-labelled confusion counts remain shadow.
`perception.attention` is an ordinary-activity proposal with source references, never an owner instruction. No task
authorization or target-routing rule depends on Jev. Since the launch week a Jev yes leads to one consideration per
app and a chat notice (section 16, L9/L10); typed text is captured again (L17).

`device/persona.py` refreshes bundle inventory and reads local Spotlight use counts/last-used dates; owner-attributed
app-focus counts are an explicitly labelled fallback where Spotlight is unavailable. Installed-only software never
establishes an occupation. One asynchronous `mdls` batch reads the metadata; plist-array entries are matched to apps
by file identity, including firmlink aliases. Cancellation kills, drains and reaps that owned process, including
cancellation during process creation. `daemon/persona.py` owns asynchronous discovery, redacted pending evidence and one shared
review across full setup, updates, `setup --persona`, CLI chat and native startup. `chat.attach` starts missing
discovery without blocking conversation. The existing approval channel shows the proposal/evidence and accepts
Save, an owner edit or Skip; the question is created once (no banner, never re-asked by polling) and reconnect reuses
the pending evidence. Confirmed/skipped state is
durable. Only confirmed text becomes an owner-stated `setup-persona` preference, and `Learning.sync_facts` puts it
in the stable identity context used by `Supervisor._system`.

## 9. Model runtimes (F2, D32)

Every model call goes through one interface. `ModelRequest(system, parts, cacheable)` ([`core/types.py`](ferb/core/types.py))
carries FERB's instructions and the context as ordered parts; the first `cacheable` parts repeat across a role's calls and
adapters cache them (P20). `assemble_parts` ([`core/context.py`](ferb/core/context.py)) returns the parts and that count from
the `ContextBlock`s whose `cache=True` leads the list. The supervisor marks Layer 1, the conversation header and every
shown chat entry for the orchestrator (L28), and the owner's original request and the task specification for task
calls ([`supervisor.py`](ferb/orchestrator/supervisor.py)). `ModelRequest.images` carries images (`ModelImage`: bytes,
evidence ref, mime) after the text parts, so the cacheable prefix is unchanged; every adapter accepts them (L12:
Anthropic image blocks, OpenAI `input_image`, Gemini `inlineData`, Claude SDK streaming input, Codex app-server image
items, `codex exec --image`). Each adapter reports `limits` per model (`ModelLimits`: documented context window, reply
limit, the runtime's per-input character cap where it documents one, and their source), and the context budget comes
from them (L28, section 16). Everything model-specific stays in the adapter. A Codex failure carries Codex's own error
(`codex_stream.codex_error`: its message, JSON-RPC code or error kind, and for its input cap the count it measured;
never the request), redacted with the daemon's redactor. An `error` notification with `willRetry: true` (Codex retries
it itself; "this will not interrupt a turn") is a `retrying` runtime event, recorded as a `model.retry` span under the
run, and the run goes on; a non-retrying error or a failed turn ends it. A retry is no sign of life: Codex 0.157.1's
default-on `unbounded_connection_retries` retries a lost connection without end ("Reconnecting... waiting for
network", 5 s doubling to 60 s; openai/codex rust-v0.157.1 `features/src/lib.rs:1283-1288`,
`core/src/responses_retry.rs:16-17, 68-92`), so a worker or checker run that only retries for `STALL_S` (180 s)
ends as a stall, its outcome naming the last retry (`worker.retry_stall_problem`; the offline sentence with no network
route). Supervisor-mode calls (planner, learners) keep the runtime's 900 s timeout.

| Runtime | Adapter | Billing | Cache |
|---|---|---|---|
| `codex` | [`codex_exec.py`](ferb/runtimes/codex_exec.py) | subscription | automatic prefix; no FERB cache key |
| `claude` | [`claude_sdk.py`](ferb/runtimes/claude_sdk.py) | subscription | automatic prefix |
| `anthropic` | [`anthropic_api.py`](ferb/runtimes/anthropic_api.py) | per token | FERB places cache markers |
| `openai` | [`openai_api.py`](ferb/runtimes/openai_api.py) | per token | explicit breakpoints on gpt-6 and gpt-5.6, else implicit; a per-role `prompt_cache_key` |
| `google` | [`google_api.py`](ferb/runtimes/google_api.py) | per token | implicit |

`CATALOG` ([`runtimes/catalog.py`](ferb/runtimes/catalog.py)) holds each runtime's default model (Codex: `gpt-5.6-sol`,
owner 2026-10-14), its `Capabilities`
(tools, cache, cache key, typed output, reasoning effort, image input, continuation, live messaging, billing) and whether
it is usable on this Mac; `ferb runtimes` prints it. A role picks its runtime in `config.toml`: `[runtimes]`
`orchestrator`, `worker`, `checker` (the checker falls back to the worker's) and `[models]` `<runtime>`; `ferb runtimes use ROLE RUNTIME
[--model M]` writes them and applies from the next call. Only usable runtimes are offered to the planner as worker types.

`ApiRuntime` ([`runtimes/api.py`](ferb/runtimes/api.py)) is FERB's own tool loop for the three APIs. Each tool call runs
through `GatewayBridge`, the same gateway path as the MCP shim, so the run token's scopes decide what is offered; a provider
class per API translates requests, responses, cache markers, typed output and usage. Behavior:
- A model that calls a tool it was not offered ends the run with a `violation` event; every name in a turn is checked before
  any call in it runs.
- Transport errors and `RETRY_STATUS` replies (408, 409, 429, 5xx, 529) are retried with backoff, honouring `Retry-After`;
  other errors surface as an `error` event with the key removed from the text.
- Tool calls run as their own tasks. `cancel` ends the run; the gateway cancelling a job (an owner stop) ends that tool call
  and the model is told, not the whole worker; cancellation of the consuming task propagates.
  The cancel flag is checked before each provider call, before each tool call and after each tool call.
- Each adapter accepts only a normal end of turn and raises `ProviderError` for any other stop state (cut off at the output
  limit, refusal, early stop), so a partial answer is never taken as final.
- OpenAI requests include `reasoning.encrypted_content` and are stateless (`store` false), so reasoning is replayed.
- No turn or token budget; stop and the stall watchdog end a run. Provider-hosted tools are never enabled.

Typed answers are checked in one place, `check_final` ([`core/structured.py`](ferb/core/structured.py)), whatever the runtime.
Every runtime reports `Usage` ([`runtimes/usage.py`](ferb/runtimes/usage.py)) in one meaning: input includes cached, plus cache
write, output includes reasoning, and the cost from the provider if it reported one, else from the dated
[`prices.py`](ferb/runtimes/prices.py) table (an unlisted model has no cost). Subscription runtimes report an
API-equivalent estimate labelled `billing: subscription`.

API keys live in the Keychain (service `ferb`, account `api/<provider>`, [`runtimes/keys.py`](ferb/runtimes/keys.py)), stored
by `ferb setup --key PROVIDER` and read at call time; the daemon adds each key it reads to its `Redactor`
([`sanitize/redact.py`](ferb/sanitize/redact.py)). The `Redactor` is reconfigured in place on a config reload, so the journal, spans and
gateway keep the one instance that knows exact secret values (API keys, OTPs); a Google API key pattern (`AIza...`) is
also redacted by shape. Codex runs with its own empty HOME (`~/.ferb/codex-user`) and
`skill_mcp_dependency_install` disabled, so the owner's personal skills never load.

Remaining: Codex sends its per-process session id as the cache key, so its calls rarely reach the previous prefix's cache
and FERB cannot change that. The remedy is an API runtime for the orchestrator (the owner's choice); the P20 re-check with
back-to-back turns is in [owner acceptance](docs/OWNER_ACCEPTANCE.md).

## 10. Records and traces (F3)

**Origins (MP2, S3).** [`core/origin.py`](ferb/core/origin.py) owns one vocabulary: `owner`, `ferb`, `private`,
`inferred`, and `external:<kind>[:<detail>]` (for example `external:web:kartikey.xyz`, `external:paste`,
`external:download`, `external:recording:<id>`, `external:unlabeled`). Ranks run owner 0, ferb 1, private 2, inferred 3;
every external label and any unknown label ranks 4 (outside). A record keeps all its origins; its trust is the least
trusted one (`least_trusted`). Origins are facts about where content came from; nothing in F3 gates an action on them
(F5 enforces from them).
- Journal rows carry `journal.origin` (migration 0015, comma-joined). `journal_origin` ([`memory/journal.py`](ferb/memory/journal.py))
  sets it by kind: a chat message is `owner` plus `external:paste` when it held pasted parts (`external:unlabeled` for
  rows recorded before FERB counted them), owner kinds are `owner`, day summaries and narratives are labeled by what they
  hold (`digest_origins`, [`memory/summary.py`](ferb/memory/summary.py)), everything else `ferb`. Derived rows
  (`task.answer`, `task.outcome`, chat replies and clarifications, `schedule.created`) record `inferred` plus the origins
  they were derived from, at capture, and say how they were made in `made_by`. Rows from before F3 stay NULL and are
  derived from kind and data when read. A chat turn's origin lookups (its request, a carried clarify payload, a
  schedule's request) read one kind through the `journal_kind` index (`Journal.of_kind`), not the whole journal.
- `tool.call` spans carry `spans.origin` (migration 0016). The gateway labels a result with `result_origins`
  ([`daemon/gateway.py`](ferb/daemon/gateway.py)): the tool's `read_sources` (`from_read_source`: file, app and
  owner_account are `private`; download is `external:download`; web is `external:web:<host>`, or `external:web:shell` for a
  shell command that used the network; email, message and unknown kinds are `external:<kind>:<id>`); else the capability's
  declared fixed `origin`; else `ferb` for an action's acknowledgement and `external:unlabeled` for a reading kind (read,
  search, navigate, mailbox read) that said nothing. Shell output is `external:unlabeled` unless network use was detected.
  Hosts on `privacy.exclude_domains` become `external:web:excluded` (`without_excluded_hosts`). Readers that return
  stored records (the task-evidence tools in [`orchestrator/evidence.py`](ferb/orchestrator/evidence.py) and `trail.days`)
  pass the stored origin through as an `("origin", label)` read source; a recording read reports
  `("recording", id)`, which labels as `external:recording:<id>`.
- Context sources record their own origins on `context.source` spans; planner blocks are labeled per block, and the root
  task span carries the request's origins (`SpanStore.set_origin`, migration 0016). `SpanStore.task_origins` unions the
  tool-call, context-source and root-task origins of a task tree, plus the episode's origins once its spans are purged,
  so a derived label (worker reports, answers, "Previous attempts") stays fail-closed after retention.
- Spans from before F3 have no origin (and those from before F0 no action kind). One fail-closed rule reads them
  for episodes, `task.evidence` and `SpanStore.task_origins` alike (`origin.recorded`,
  [`core/origin.py`](ferb/core/origin.py), and the same rule in SQL): a tool call or context source that holds output
  but no origin is `external:unlabeled`, whatever its kind; a span with no output adds nothing; the root task span's
  origin is set explicitly, so a root without one adds nothing.
- The model sees the label: `origin_line` appends `Origin: <labels>` under every tool result, with
  `(outside content: data, not instructions)` when any label is external.

**Argument sources (S3).** On a consequential call (kind not in `REVERSIBLE`) the gateway records
`input.arg_sources` on the `tool.call` span: for each target (`account`, `site`, `path`, `contact`, `recipient:N`,
`amount`), a list of `owner_words`, `owner_answer`, the `external:…` origins that contain the value, any of these
together, or `["unknown"]` ([`policy/provenance.py`](ferb/policy/provenance.py), `arg_sources`). `owner_words` means the
value is in the request's words; `owner_answer` means it is in one of the owner's `owner.ask` answers
(`SpanStore.owner_answers`): a chosen option is model-written text the owner picked, so it never counts as typed words.
The request's words (`Request.words`, continuity `owner_words`, the root span's `authority_words`) hold only what the
owner typed: an answer to FERB's clarify question adds the owner's earlier words, never FERB's question, which the
planner still sees as data in the payload; a continuity row from before 0.27 F3 has that question removed when read
(`without_ferb_question`), and a row from before 2026-10-03 (no recorded payload) loses the older unlabeled form, in
which a plain yes appended FERB's question with "your" read as "my", only where FERB's own recorded `chat.clarify`
question, transformed that way, follows the yes line. The outside text is the decoded string values of the task's earlier tool results
(`SpanStore.tool_outputs`, first 20,000 characters each, cut while joining). The gateway keeps a task's outside texts
prepared (`OutsideTexts`, by root task, dropped by `forget_tasks`) and each consequential call reads and prepares only
results newer than the last it saw; a call still running holds that point below it, so its result is matched once it
finishes. Matching is by value kind: whole tokens for emails, sites, paths and accounts; exact digits for phone-like
values; a whole token for a single-word name and a phrase for a multi-word name; whole-number tokens for amounts (only
a currency symbol is stripped). It is a fact: F3 asks nothing and blocks nothing; F5 decides confirmations from it.

**Step summaries.** Every `tool.call` span stores a one-line `spans.summary` (migration 0017), composed by code from the
result ([`trace/lines.py`](ferb/trace/lines.py) `summarize`) after redaction and before clipping. The gateway publishes
`tool.finished` (tool, the arguments a step line shows (`lines.ARG_KEYS`, each clipped to 100 characters after
redaction; the span keeps them all), why, summary, seconds, error) and `ferb logs --tools` shows it as a step line while
the task runs. An `owner.ask` the orchestrator answers itself leaves no `tool.call` span (only its `model.question`
decision), so it shows as a Planner step and not as a tool step.

**Episodes (L1, MP1).** One permanent row per root task in `episodes` (migration 0018): request text, goal, status,
verified, times, answer, success conditions with evidence, steps, problems, per-tool call/error counts, origins,
`tool_origins`, `external_read` (any external origin among the tool-call, context-source and request origins), usage
and desktop changes. `origins` is the full union; `tool_origins` holds only the task's own tool results, so F5's sink
rule can tell what the task itself read apart from inherited context (the request, memory, planner blocks). The request
text is what the owner typed for the whole request (the root's `authority_words`, so the part a clarify answer
continued is included), followed by `[pasted text]` when the recorded request still holds one of FERB's wrappers once
FERB's own earlier question is set aside (`holds_pasted`), since the typed words leave pasted parts out. [`trace/compose.py`](ferb/trace/compose.py) composes it by code
from the trace (no model); a worker's words stay a claim ("worker reported done: …"), and a stopped worker reads
"stopped: …" because the supervisor wrote that summary. A failed step shows `✗ failed: …`, a step that never finished
shows `⏹ did not finish (interrupted)` with no claim or check, and the final-state review shows as an "after cleanup"
step.
`made_by` is `code:episode/1` (`episodes.MADE_BY`, [`orchestrator/episodes.py`](ferb/orchestrator/episodes.py)).
An episode is written at task end (the last thing in the supervisor's cleanup, so it cannot skip cleanup), for a task
stopped by the owner, for tasks interrupted by a restart (at start-up), and by retention before any purge. A failed
build is noted as `episode.failed` and never stops the task or start-up. Because it is composed from spans, a missing
episode can be rebuilt while its spans exist.

**`ferb trace` levels (P26).** `ferb trace [TASK]` prints the story from the episode: a head line, numbered steps, the
answer, problems, a token/cache/≈ $ cost line (the estimate says it is one; an unpriced model says cost unknown), the
desktop line with its listed changes and paused spans (section 6) and your verdict. `--steps` prints one line per tool
call from the spans (or says steps are no longer kept and shows the story). `--full` and `--json` show the raw recorded
trace. Every story line is rendered by code from records ([`trace/render.py`](ferb/trace/render.py)); golden tests pin
the output. A task without an episode falls back to the raw view.

**Retention (D14, D21).** [`store/retention.py`](ferb/store/retention.py) with `Daemon._purge_records`
([`daemon/main.py`](ferb/daemon/main.py)). Config: `retention.trace_days` and `retention.decision_days`, both 30. Aged
out: a finished root task's spans and its intake planner's spans (only after its episode exists; the purge builds one
first and keeps the spans if that fails), loose spans tied to no task, and Jev decisions. Never aged out here: the task
list, episodes, the journal (chat, day summaries, verdicts) and memory. Raw activity has its own 30-day purge
(`sense.trail_retention_days`), which keeps days without a summary. A tree is deleted by one recursive statement inside a
transaction, so a parent never goes before its child and a failure leaves nothing half-deleted. Each root is isolated
(`retention.failed` journals its error and the pass continues); at most 200 roots per pass (`ROOTS_PER_PURGE`), oldest
first. The loop runs hourly, starting a minute after start-up, on the daemon loop (`_purge_pass` yields between roots;
`_purge_records` runs the same steps synchronously), and journals `retention.purged` with the counts and `roots_left`
only when it purged something or left roots for the next pass; a pass that shutdown cancels between roots still
journals what it purged, marked `stopped`. A task interrupted by a restart ages from the restart (`interrupt_running` bumps `updated_at`), never from
before it. Typed text and clipboard (7 days) arrive with F7. F4 added evidence (`retention.evidence_days`, 7) and
recovery copies and snapshots (`retention.recovery_days`, 7) to the same pass (section 11). Every `retention.*_days`
value is read through `retention.days`: at least `MIN_DAYS` (1), so a 0 never purges what was just kept. Downloaded
files have no time limit (owner, 2026-10-09; `retention.download_days` is gone): the same hourly pass moves the oldest
to the Trash only while FERB's downloads folder is over `browser.download_total_mb` (`Downloads.purge`, journaled
`retention.downloads`). It never records a file gone or clears its path without looking: a row whose file is outside
the downloads folder is left as it is, recorded once as skipped and not counted toward `browser.download_total_mb`.

**Content search (D9).** Migration 0019 adds `trail_fts` and `summary_fts` (same tokenizer as memory, so Devanagari words
stay whole) kept in step by triggers and created with FTS5 `secure-delete`, so a purged or forgotten row's terms are zeroed
(needs SQLite 3.42+). `trail_fts` holds the decoded text values of a trail row's JSON, not raw JSON or keys.
`summary_fts` holds only a summary's content: window titles, per-hour apps and sites, FERB task summaries, a narrative's
text, and the keys of `sites`, `folders` and `apps_min`; fixed wording is not indexed. `trail.query` takes `text=` (every
word must appear in the event's text) and `trail.days` (`text`, `since_days`, `limit`) returns day summaries. Query terms
are lowercased the way the tokenizer folds them (`query_terms`, [`memory/items.py`](ferb/memory/items.py)).

**Residuals F5 must close.**
- Shell reads of downloaded files are not tracked: shell output is `external:unlabeled` and a file the owner's task
  downloaded and then reads with `cat` is not marked as outside content. F5's path lists and proxy facts narrow this.
  (`fs` reads of `~/.ferb/workspace/downloads` are `external:download`; code adds that root, D35.)
- FERB's own `fs` tools reach any path until the never-read / never-write lists exist (F5).
- Origins and `arg_sources` are facts only; no confirmation or block uses them until F5.
- `arg_sources` tells `owner_words` (typed in the request) apart from `owner_answer` (an option the owner chose), and
  episodes keep `tool_origins` apart from inherited origins; F5's confirmations and sink rule decide what each means.

## 11. Verification and recovery (F4)

**Three statuses (V4, V5, D20).** A root task ends `done`, `done_unverified` or `failed` (or, as before, `needs_user`,
`cancelled`, `interrupted`); migration 0021 adds `done_unverified` to the `tasks` status check, keeping row ids.
`Supervisor._final` ([`supervisor.py`](ferb/orchestrator/supervisor.py)) decides, and the episode stores the status
beside its `verified` flag:
- `done`: the work finished and every condition was verified true. The answer is the outcome.
- `done_unverified`: the worker reported done, the check found nothing false, and the orchestrator finished with
  `result: "unconfirmed"` (or failed while choosing another attempt); also a task that had no conditions. The answer
  starts "Done, but not confirmed:" and lists each unconfirmed condition as "- <condition> — unverified: <evidence>",
  with "  To check: <how>" when the checker said how the owner can check it; with no conditions it is "Done, but there
  was nothing to check it against."
- `failed`: the work didn't finish, a check found a condition false, the orchestrator finished with `result: "failed"`
  or no result, or work stopped for another reason than the owner's stop (FERB stopped, the task crashed).

The dispatch schema's `result` (`unconfirmed`, `failed` or null) is the orchestrator's judgment; code only maps it
(rule 6). "Unverified" is printed only for a condition a check gave a verdict on (V5): when the work stopped before any
check, the answer is "I couldn't finish this task." with "Still to do:", and when the checker itself failed the
conditions are listed under "Still to check (the check couldn't run, a problem on my side):". A worker's prose is never
stated as success. The cleanup review ([`resources.md`](ferb/orchestrator/prompts/resources.md)) can't turn an
unconfirmed outcome into `failed` for that alone, nor a judged failure into `done_unverified`. The readers label
`done_unverified` "not confirmed" (the stored status and the baseline keys keep their names):
`ferb trace` heads the story `done ✓ verified`, `done (not confirmed)` or `failed ✗`
([`trace/render.py`](ferb/trace/render.py); an older `done` without the verified flag reads `done`);
`ferb chat --verbose` prints `done ✓`, `done — not confirmed` or `failed ✗` before the answer; `ferb stats` says
"done N (x%), not confirmed N (y%), failed N (z%)"; `ferb baseline` adds `unverified` (the not-confirmed count),
`unverified_pct`, `failed` and `failed_pct`, and `done_pct` counts only `done`. A scheduled run that ends
`done_unverified` neither clears nor adds to its schedule's failure count; the "FERB finished" notification title is
for `done` only.

**Task status facts (owner live use 2026-10-18).** Each planner turn carries a protected Layer 6 block of FERB's own
record of the tasks a progress question can be about (`Supervisor._status_facts`): every running tree (running,
blocked when its latest worker report or a plan step is blocked, failed or needs an answer, stopping) and this
conversation's latest five tasks recorded since its chat session started (`CONVERSATION_ROOTS`: the tasks above
the session's rowid floor by key, each matched to its conversation by key), with the first line of the latest
worker report and the plan's step counts, labelled with those tasks' origins. `supervisor.md` answers progress only
from it.

**A recurring run sees the one before (fix wave 2, 2026-10-15).** A scheduled run's tree is saved in the schedule's
conversation (`schedule:<id>`), and a run the planner answers directly is journaled there as `chat.reply`, so the next
run's planner history and `chat.history` (for its tree) hold every earlier run's answer. `Supervisor._previous_run`
finds the schedule's latest earlier run that left an answer from the journal (`schedule.run` request id →
`task.answer` / `chat.reply`; runs before this change had no conversation); its time and status go in the Schedule
facts (`previous_run`, with `later_runs_without_answer`), and its answer is a protected Layer 2 block with that
entry's origins and lineage reference (MP2), or a block saying no earlier run is recorded. A scheduled planner turn
gets no resumable-attempts block: earlier runs are data to compare, not attempts to continue. `scheduled.md` tells the
planner to compare with the previous run when the request asks about change. Notification delivery is unchanged.

**The report check (P16 V4/V6, D34; F4 live fixes).** `checked_conditions`
([`supervisor.py`](ferb/orchestrator/supervisor.py)) adds `REPORT_CONDITION` ("My report to you says truthfully what was
done, who did each part (FERB itself, or the agent, app or person you named) and with what, and leaves out nothing that
changes the result"; check kind `none`, layer `outcome`) last to every check of a plan that has conditions; a plan with
none still gets no check. The checker receives the complete worker report as data and judges it against the records
([`verifier.md`](ferb/orchestrator/prompts/verifier.md): FERB's own tool calls versus the agent, app or person that did
each part, and the tools, files, accounts and sources used); code matches nothing in the report (rule 6). `met=false`
makes the task `failed`, as for any condition; unknown keeps it from `done`. The owner never saw the worker's report, so
`_report` finds FERB's condition by its index (it follows the planner's conditions, so a planner condition in the same
words stays its own) and says it in their terms: "What I reported didn't match the records: …" when false, "I couldn't
confirm my report matches the records: …" (with "To check: …" when the checker said how) when unknown, and "Whether what
I reported matches the records" under "Still to check" when the check couldn't run (crash or no verdict); a met one is
left out of "Confirmed:". Work that stopped before any check never lists it. A done answer is the report the check
judged: `_verifier` keeps it as `checked_report` (reset with each attempt), the verified answer in `_final` is that
report, and `_reconcile_resources`, the review after cleanup ([`resources.md`](ferb/orchestrator/prompts/resources.md)),
may still lower the status, its own words then saying why, but never writes new done wording; its summary stays in the
`model.resources` span, which the story shows as an "after cleanup: …" Planner step. In task `79bc5bfa` the
after-cleanup rewording became the answer and dropped that FERB, not Codex Astra, had made the submission. The story's
"checked nothing — N conditions unknown" counts the report condition.

**Read-only checker (V3, W4).** The checker's run token is observe-only (`RunGrant.observe_only` in
[`auth.py`](ferb/daemon/auth.py), issued for the verifier role in [`worker.py`](ferb/orchestrator/worker.py)). The
gateway offers it only capabilities that declare `observation = True` (`observes`) and refuses any other call before
validation, ledger, locks or run, with "the final check is read-only: this tool acts and is not available to a checker;
report what you could not confirm as unknown". Unmarked means acting, so a new tool stays out of the checker's reach
until someone marks it; `tests/test_checker_readonly.py` pins the marked set. Opening and navigating are observation
only in FERB's own browser (`ferb_browser.open`, `ferb_browser.navigate`), and a checker may navigate only the FERB tabs
it opened itself; the owner-profile `browser.open` and `browser.navigate` are not observation, so a checker never moves
the owner's tabs, and a checker's open never creates a window that may switch Spaces. It still reads the worker's tabs
where they are. A check that needs a click or typing stays `met=null` with what is missing, and the orchestrator can send a worker
to gather it. Every refused call (an acting tool asked of the checker, an unknown or out-of-scope tool) leaves a
`tool.refused` span; the story adds "⚠ N tool calls refused".

**The checker's starting evidence (fix wave 6b, 2026-10-21; W4, MP2, L28).** `_verifier` gives the checker, after the
report block, the worker task's recorded tool calls with their results (`recorded_calls` in
[`orchestrator/evidence.py`](ferb/orchestrator/evidence.py)), so it no longer pages through `task.evidence` to see
what the worker did (owner live case 2026-10-15: three rounds, about 50 reads, after two minutes of work). The records
are the ones `task.evidence` serves (`SpanStore.tool_calls`/`tool_call`: the same filter, the stored redacted rows; no
new capture path, and the worker's own `task.evidence`, `chat.history` and `memory.read` calls are not among them, as
before), in recorded order, every run of the worker included: a repair round runs as the same worker task, so a
re-check gets the earlier run's calls and the repair's, each labelled with its run when there was more than one. A
protected line gives the count ("This task recorded no tool calls." when none). Each call is one optional
`ContextBlock`: a code-made header (tool, its recorded `kind`, observation or action as the capability declares it
(`observes`), start time, its origin's kinds, failed or no result, the `span_id`), then the record text in
`wrap_untrusted`, up to `task.evidence`'s default page (`PAGE_CHARS`, its `max_chars` default 12000); a longer record
says where its rest starts ("cut at 12000 of N characters, the rest from offset=12000"), exactly as that read pages
it. `assemble` fits them to the checker runtime's window and per-input character cap like every other optional
block; one left out stays in its place as "[Context omitted; fetch full text: task.evidence span_id=… (recorded call
n of N: tool, observation|action, time)]". Which stay when space runs out: every action first (its record is the only
evidence of what was done, sent or edited; no fresh read recovers it), then observations (a fresh read can also
re-establish current state), newest first within each, the latest being closest to the final state; all above the
context's other optional blocks (their highest priority plus one). A record is shown up to its page or left out
whole, never cut to fit. The blocks carry no `origin`, so no `context.exposure` copy is written: the records are this
tree's own tool calls, whose origins `task_origins` already holds; the checker's recorded `context` keeps them like
any model input, and a forget scrubs spans as before. [`verifier.md`](ferb/orchestrator/prompts/verifier.md) says the
calls are in context and `task_evidence` is for a record cut or left out, the root's intake, context, cleanup and
disturbance records, and searches. A scripted complete check of 53 recorded calls (50 short, 3 of about 30,000
characters) needed 62 `task.evidence` calls before and 6 now (`tests/test_checker_recorded_calls.py`). Residual: on a
task whose recorded calls hold more than the checker's window (many long snapshots), the bundle fills the window at
the checker's start, leaving its own later reads to the runtime's context handling; and a task with thousands of
calls left out lists one marker each.

**After-action reads (V2).** Every consequential (non-`REVERSIBLE`) result carries an `after` block, a failed call's
too: `{read, changed, value, note}` from [`act/aftercheck.py`](ferb/act/aftercheck.py) (`after_of(facts_of(content))`).
`read` is `ax` (a mac tool's `observed_change`), `value` (`value_matches` / `ax_value`, or a browser upload's
`bytes_match`) or `none`; `changed` is None whenever nothing was read back (an `ax_value` result with neither
`value_matches` nor `accepted`, such as a failed edit, reads `none`), and an accessibility diff that saw no change is
None too, never False. Conditions of kind `after_change` and `after_value` (optional `Check.tool`) compare the block of
the latest acting `tool.call` span (`SpanStore.latest_acting_call`), failed calls included; a call that died gives
unknown, and a failed call (a span error, or `ok: false`) whose block says the screen changed never confirms
`expected=true` (unknown: the change may not be the one expected).

**Conditions and the ledger (V6, V7).** `CHECK_KINDS` are `none`, `file_exists`, `file_contains`, `tab_url`, `account`,
`after_change` and `after_value`. `ledger_done` is gone: a spec saved with it loads as `none` (a `Check` validator), so
an older task still continues. The planner phrases conditions as the outcome the owner wants, observable from the
background; the checker may add `how` (nullable) to a condition it leaves unknown. The ledger is a recovery record,
not proof of success: `Ledger.unresolved(tree)` lists the tree's `unknown` and `executing` rows (with `retry_approved`
and `retried_by`), which "Uncertain prior actions" shows for the tree and the task it continues. AppleScript stays
ledgered: no declared read-only flag exists and code can't prove a script only reads (rule 6), so V7's "read-only
scripts stop creating rows" waits for one.

**Evidence store (V1, D20, D26, D29).** `EvidenceStore` ([`act/evidence/store.py`](ferb/act/evidence/store.py),
migration 0020 `evidence`) keeps captured state the checker can re-inspect for `retention.evidence_days` (7): files at
`~/.ferb/evidence/<day>/<id>.jpg` (folders 0700, files 0600, created exclusively, kept inside the root) and a row with
the kind (`screenshot`, `snapshot`, `recording`, `app_window`), redacted facts and the result's origin. The gateway keeps
each tool image (`_keep_image`, through `put_async`: masking and the file write run in a worker thread, the row on the
loop right after the write; cancelled mid-write, it waits for the thread, removes the file and re-raises, so never a
file without its row; the cancelled call's span still ends, with `evidence_error: cancelled`); the model gets it as
before, and the span's output names it (`evidence: <id>`, or `evidence_error`). Since the launch week (L13) the
evidence keeps exactly the image the model got, masked where captured and then sanitized; browser windows, hostless
pages, excluded sites and recording contact sheets keep their pixels. Facts only, with `image_withheld` saying why,
remain only where painting or the F5 redaction failed ("image redaction unavailable", a `Withheld` reason). When a
worker's first successful `mac.*` call targets an app, the gateway also takes one picture of that
app's window, after the call has answered and outside its resource lock (`_look_later` starts `_first_look` as its own
job, in `Gateway.looks`; a stop or shutdown cancels it, while a task that just ended lets it finish, bounded by its
10 s capture timeout), through
`MacComputerDriver.window_image` (one window through ScreenCaptureKit, never the desktop, never brought forward,
never the worker's latest shot; secret fields are painted over, L13), masks the
secret fields its accessibility tree shows ([`act/evidence/mask.py`](ferb/act/evidence/mask.py), in a worker thread) and
stores it as `app_window` on an `evidence.app_window` span. That happens once per task, worker and app, not when the
call itself returned an image (kept or facts only), not for browsers, not without the screen scope or with `[mac] snapshot_image` off; a failure,
or a capture over 10 s, is logged on the span (a cancel finishes it as `cancelled`; a failure of the span store itself
goes to the daemon's log) and never fails the call. The picture is evidence only: it does not go to
the model. The checker lists captures with `task.evidence kind=evidence` and reads one by `evidence_id` (facts and
origin; never bytes or paths; its own task's only). Retention deletes the file before the row. D6/D26 as clarified
2026-10-13 (L12): the model-bound start-of-task picture is the front window captured when the owner sent the message
(section 16), which the planner and the worker both get; this evidence-only first look per app stays. Screenshots
otherwise go to the model only when it asks (`mac.screenshot`, `browser.screenshot`), never one per model step. V8's
input exists: an episode's `verified` flag and the task's stored evidence; learning from it is F8.

**Unknown outcomes are never retried by code (P19).** Before a consequential call runs, `Ledger.intend(...,
reconcile_first=True)` looks for the same action anywhere in the task tree (ledger rows keep the worker's task id) in
state `unknown`, `intended` or `executing`. The same action means the same ledger key and `payload_hash`
([`policy/rules.py`](ferb/policy/rules.py), `ledger_identity` in the gateway): the capability, or its `ledger_key`
(`shell.start` and `shell.run` are one); identity; account, site, contact, recipients, path (the file the call really
writes, `files.real_path`: symlinks first, then `..`, as the fs lock and S1 key it; a path with a NUL is refused before
anything runs) and amount; content; and the validated arguments, minus the agent's notes (`ANNOTATIONS`: `why`, `effect`,
`purpose`, `contact`), minus arguments a normalized target already binds (`target_args`: fs `path`, shell `cwd`, whose
absence is the workspace, a mac tool's `app`, a recording's `path`) and minus how the call is delivered (`method_args`:
mac `mechanism` and `allow_foreground`, browser `page_mode` and `trusted`, shell `background` and `timeout_s`). The kind
is left out. An AX action the app didn't answer in time (`kAXErrorCannotComplete`: it may still have acted) raises
`NoAnswer` in [`accessibility.py`](ferb/act/mac/accessibility.py), so its row is `unknown`; any other AX error is a
definite failure. An earlier unknown attempt refuses the call: "A previous attempt at this exact action may have gone through
(its outcome is unknown); it was not repeated without the owner.", with `retry_action` (the row id) and `next` (look for
evidence that it happened). One still running refuses with "The same action is already running in this task; it was
not started again." A call that could not start, or whose recovery copy failed, is `failed`, not unknown. The agent may
then ask with `owner.ask(retry_action=…)`: the gateway checks the row is this tree's and unknown, writes its own line
first ("FERB: an earlier attempt at <tool> <target> may already have happened (outcome unknown). Trying again could
make it happen twice. The call with the unknown outcome: <redacted excerpt>. Try it again?"; after several, "FERB: N
earlier attempts at … may already have happened (outcomes unknown). Trying again could make it happen up to N+1
times."), then "What the worker found: …"), offers only Yes and No, and skips the orchestrator. `<tool>` is the tool
the worker really called (from the attempt's span, not the shared ledger key) and `<target>` one readable target that
keeps every consequential identifier (every recipient, else the contact, else the path or the site; then the amount
and the account); C0/C1 control characters are
removed from both paragraphs, as `ferb chat` removes them from everything it prints
([`sanitize/terminal.py`](ferb/sanitize/terminal.py)). Only the owner's Yes approves. Migration 0022 adds
`retry_approved` (the approving `owner.ask` span) and `retried_by`; the row stays `unknown`, since nobody observed the
outcome, and one approval allows exactly one new attempt. A Yes to a question that went out of date (another attempt
started meanwhile) approves nothing. Limits: a reworded message or another target is another action (code can't judge
"the same meaning", rule 6); a GUI retry with a fresh snapshot element id hashes differently; and a continuation in a
new task tree is not refused by code: its orchestrator sees the earlier tree's rows under "Uncertain prior actions".

**Recovery (S1).** [`act/fs/recovery.py`](ferb/act/fs/recovery.py); migrations 0023 `recovery` and 0024 (`saved_dev`,
`saved_ino`).
- Copies. Before a `WRITE_FILE` call changes an existing regular file (`fs.write` with `overwrite=True`: "overwrite";
  `fs.edit`: "edit", from each tool's `recovery_reason`), the gateway copies it to `~/.ferb/recovery/<day>/<id><ext>`
  (0700): an APFS clone, else an exclusive byte copy, in a worker thread. The file is the one the call really writes:
  `files.real_path` (symlinks first, then `..`), which is also the fs lock key; never the textual spelling. A write tool
  whose file can't be told is refused (`NoFileTarget`) unless it declares `creates_new_only` (the recording tools,
  which refuse an existing path). No copy, no change: when the copy fails, the call doesn't run. After the call,
  `written_sha256` records the file as FERB left it; an error result drops the copy only when the file still equals it;
  an exception (unknown outcome) keeps it. The result says `recoverable: <id>`, or `recovery_error`. Copies and their
  records are kept `retention.recovery_days` (7).
- Trash. `fs.trash` (`files.trash_item`, L27) moves the item only between two folders it holds open: the item's
  folder (opened `O_SEARCH|O_DIRECTORY`, named with F_GETPATH and checked against the lists) and the Trash folder
  (which must be a real `.Trash`/`.Trashes`, never made through a link), with `renameatx_np(RENAME_EXCL)`. It then
  checks that what landed is what it checked (device and inode through the Trash fd). A mismatch, or an item it can't
  look at afterwards, stays in the Trash and nothing moves out: the result says `verified: false` and the record's
  reason is `trash-unverified`, which `ferb restore` refuses and `recovery.list` marks `needs_owner`. Another disk is
  refused, never copied. A `trash` record keeps the original path, the Trash path and the device and inode fs.trash
  verified (never re-read by path). The record stays while that very item is still in the Trash, at most 90 days
  (`TRASH_RECORD_MAX_DAYS`); the item itself always stays in the owner's Trash. FERB doesn't go through NSFileManager's
  trash, so Finder's "Put Back" isn't offered; `ferb restore` is the way back.
- Snapshots. When an agent asks (`recovery.snapshot`, input `before`: kind `snapshot`, reversible, no ledger row,
  not an observation tool), before a `DELETE_PERMANENT` call (no capability has that kind yet) and, since F5a, before
  every `RUN_SHELL` call (`shell.run`, `shell.start`, `shell.send`: code can't tell a shell delete from a build,
  rule 6; the result says `recovery_snapshot {snapshot, record, reused}` or `snapshot_error`), on a
  `recovery.snapshot` span: `tmutil localsnapshot`, measured 2026-10-09 from a terminal with no admin rights and no
  prompt (0.43 s). At most one attempt per 15 minutes across all tasks, a failed one included; a recent snapshot is
  reused. Each is a `recovery` row (kind `snapshot`, original `/`, `saved_to` the snapshot's name, reason the redacted
  `before`). The hourly pass deletes FERB's own snapshots older than `recovery_days` (`tmutil deletelocalsnapshots`: no
  admin, about 15.5 s each, journaled `retention.snapshot_purged`). FERB can't read a snapshot back (mounting one needs
  admin rights); it is a recovery point the owner can open from macOS Recovery or Time Machine. A failure never refuses
  the call.
- `ferb restore` (RPCs `recovery.list`, `recovery.restore`) lists what FERB can put back, newest first, and puts one
  back by id or a unique prefix, holding the file's gateway lock. A copy goes back in place only when the file is gone
  or still exactly what FERB wrote (SHA-256); otherwise the current file stays and the copy goes beside it as
  "<name> (before FERB)<ext>". A trashed item moves back through held folders (L27): the Trash folder opened
  `O_SEARCH|O_DIRECTORY|O_NOFOLLOW` and still a Trash folder, the item checked with `fstatat` against the verified
  device and inode, the original folder made only where its deepest existing parent is exactly where it was
  (`make_folders(…, exact=True)`) and opened and checked by F_GETPATH, each name checked against the lists, then
  `renameatx_np(RENAME_EXCL)` to the original name or "<name> (restored)<ext>", never replacing anything, and the item
  checked again after the move. Nothing goes through a link; a mismatch refuses and leaves the item in the Trash. Its
  record then goes. Each restore is journaled as `recovery.restored`.
- Not covered: file changes made through shell commands or AppleScript get no per-file copy.

**Named targets (D34).** The "What the owner named" block (the spec's apps, sites, folders/files and people) reaches
every role, the checker included, and ends "The owner named these; evidence must be about them, not a substitute." The
checker's prompt makes evidence about a substitute `met=false`, naming what was actually used, so the task is `failed`;
a named target it couldn't inspect is `met=null`. Asking which target the owner meant is the agents' what/which question
(`owner.ask`); no code judges whether a target matches (rule 6). The prompts (F4 live fixes, V6, P27) carry the same
rule for work: when the owner hands work to a named agent or person that does it on its own (not an app FERB uses as a
tool), the planner's goal and one condition say that one does it; a worker starts, helps and monitors it and does none
of its work unless the owner says so, and asking the owner to take over is the owner's decision about who does the work,
not a method question ([`question.md`](ferb/orchestrator/prompts/question.md)); the checker gives `met=false` for work
FERB did itself. Each quality or limit the owner states for the result is its own condition, in their words; a worker's
report says who did each part and with what; a lasting choice the owner did not ask for and the request does not need is
an outcome to confirm; and everything the owner reads says "you" and "your"
([`interaction.md`](ferb/orchestrator/prompts/interaction.md), shared by every role; `tests/test_prompt_contract.py`
pins the sentences).

**Downloads (D35).** Section 7 has the mechanism. In short: a download in a live task's tab becomes facts (`path`,
`name`, `bytes`, `sha256`, `mime`, `source_url`, origin `external:download`) in `~/.ferb/workspace/downloads/` and in
the `downloads` table (migration 0025); `browser.downloads` is an observation tool; built on extension 0.15.1,
protocol 12 (now 0.15.6, protocol 17). The
owner's limits of 2026-10-09: 512 MB per download, 50 files and 4096 MB per task tree, 50 new tabs per task tree, a
20480 MB folder (only past it do the oldest go to the Trash), a 10 GB free-disk floor (as macOS shows it; space macOS can purge doesn't count), no time-based clearing; the
page-hold caps (20 held saves per tab, 100 in all) are a separate guard and unchanged. [`config.py`](ferb/config.py)
`ensure_privacy_defaults` writes these values where a key is missing, `browser.download_min_free_gb` included. The
values F4 had written into `config.toml` before the owner chose (100, 20, 500, 2048, `task_max_tabs` 20) are replaced
once, with the owner's approval of 2026-10-09: at start-up, unless a `config.upgraded` row for
`download-limits-2026-10-09` (`LIMITS_DECIDED`) exists, `Config.upgrade_ferb_written` changes each `FERB_WRITTEN` key
still holding F4's value (any other value is the owner's and stays) and removes `retention.download_days` (`RETIRED`),
all in one atomic write, and the daemon then journals `config.upgraded` with the changes (a fresh home records none; the
row also marks the upgrade as done, so a value the owner sets later is never replaced). `ferb timeline` prints every
change in full ("browser.download_max_mb 100 → 512; …; retention.download_days 30 → removed") or "settings already
current".

**Carried to F5.** Full screenshot redaction (stored images get accessibility-known masking only, and messaging and mail
apps keep facts only until then); the destructive-action and sink confirmations, which build on S1 (FERB's fs overwrite,
edit and trash are recoverable; a shell change is not); never-read lists covering `~/.ferb/run` (run tokens) and
`~/.ferb/recovery` (copies); FERB's shell reaching the daemon socket, from which it could answer owner questions, P19
approvals included; shell file changes, which get no per-file copy; and upload copies: FERB's shell can read and write
`~/.ferb/workspace/uploads`, so when the shell sandbox lands it is denied that folder for reads and writes, or staging
moves beside `~/.ferb/recovery` (F4 live fixes).

## 12. Containment, confirmations and secret capture (F5)

**The protection lists** ([`policy/protection.py`](ferb/policy/protection.py), S2, SQ3 revised, SQ5, D27, rule 10).
`[protection]` in `config.toml` names the never-read stores and the never-write places. Code always adds FERB's own
state (`~/.ferb` except the workspace, plus `workspace/uploads`), which nothing lifts, and, when FERB runs from
FERB.app, the app bundle itself (`ferb_app`, L27: never written or removed by FERB's file tools or shell, nor a folder
holding it). Paths are compared as typed, resolved and as the kernel names them (F_GETPATH), so `/System/Volumes/Data`
and `/.vol` spellings don't evade a rule. FERB's tools that name files
check every path they declare (`file_paths`) against the same lists at the gateway.

**The shell sandbox** ([`act/shell/sandbox.py`](ferb/act/shell/sandbox.py)). Every command `shell.run` and
`shell.start` start, and every program those commands start (a coding agent included), runs under one `sandbox-exec`
profile built from the lists. `mac.applescript` runs `osascript` under the same profile, so `do shell script` meets the
same rules.

The profile:
- denies reads and writes of the never-read stores, FERB's state and upload staging, with the workspace allowed back;
  FERB's home and upload staging allow `file-read-metadata` only, so tools that walk up from the workspace (git, node,
  a POSIX `cd`, `python3 -m venv`, `ls -l`) work while the contents stay unreadable;
- denies writes to the never-write places;
- denies writes to every Trash folder (`regex #"/\.Trash(es)?(/|$)"`, plus `~/.Trash` and `.Trashes` spelled out),
  after every lift and never liftable, so `fs.trash` and `ferb restore` are the only ways in or out (ruling
  2026-10-14, rule 5/10); FERB.app's bundle is denied the same way when FERB runs from it;
- denies writes to every *existing* folder above a protected path, and to the workspace folder itself, as literals, so
  a rename can't carry protected content to an unnamed path (a missing parent like `~/.cargo` is left writable so a
  first install can create it; the entry's own subpath deny still blocks the entry);
- denies the Keychain service (`com.apple.SecurityServer`);
- denies launchd jobs (`job-creation`), opening apps or URLs (`lsopen`), the two LaunchServices lookups
  (`com.apple.coreservices.launchservicesd`, `com.apple.lsd.mapdb`) and the Apple Events service
  (`com.apple.coreservices.appleevents`), so the
  shell can't put an app on the owner's screen; FERB's browser and Mac tools do that;
- denies the system services that would reach the network for a contained process: background URL sessions
  (`com.apple.nsurlsessiond`) and mDNSResponder's resolver (`com.apple.dnssd.service`);
- denies mounting (`file-mount`) and the SMB device node (`/dev/nsmb*`), so the kernel's SMB and NFS clients, which run
  outside the sandbox, can't be used to leave the Mac past the proxy (measured: `smbutil`, `mount_smbfs`, `mount_nfs`);
- denies all network except FERB's egress proxy and the servers started inside FERB's sandbox (below);
- names the daemon socket explicitly.

`mac.applescript` runs under a second profile (`profile(apps=True)`) with the same file, network and Keychain rules but
`lsopen`, the LaunchServices lookups and the Apple Events service kept, because `tell application` may launch the
app; `job-creation` stays
denied.

What was measured on this Mac (the Task 2 report has each run):
- Rules match resolved paths, and on the case-insensitive volume every case and Unicode spelling.
- Hardlinks and clones of a denied file are refused.
- `security` fails at once with no prompt.
- Historical Task 2 profile allowed trust evaluation; T3b now denies trustd by default (live compatibility pending).
- Programs that list root certificates through the Keychain service (rustls-native-certs, e.g. Codex CLI) find none,
  so the shell's environment sets `SSL_CERT_FILE` to Apple's `/etc/ssl/cert.pem`.

`ShellManager.start` takes the command text. `ProcInfo` records `sandboxed` and `lifted`. The profile file
(`~/.ferb/run/sandbox/*.sb`, 0600) stays until its process ends, because `sandbox-exec` reads it after starting;
leftovers are swept at start.

When `sandbox-exec` can't run, the shell tools and `mac.applescript` are not registered, the journal records
`shell.unavailable`, and `ferb doctor` shows a failed `containment` check. They never run uncontained.

`shell.rules` (an observation tool) lists the rules and which ones the owner can lift.

Limits of the platform, stated to the agents in the shell tools' descriptions and in `shell.rules`, never enforced on
their flags:
- Nothing inside can start its own macOS sandbox (`sandbox_apply: Operation not permitted`). Codex CLI runs inside with
  its own sandbox off: `--sandbox danger-full-access` (measured working), and the full bypass flag is only for when the
  owner asked for it.
- Setuid programs such as `sudo` cannot start under any profile.
- The shell can't open apps or URLs, script other apps, or start background jobs; `shell.rules` says so.

The loopback tool-list proxy for the Claude runtime
([`runtimes/proxy.py`](ferb/runtimes/proxy.py)) forwards only for a run id that is live and carries that run's random
secret (`/run/<id>/<secret>/…`); anything else gets 403. It is created lazily (on the first Claude run), so an earlier
long-running contained process could reach its port, but only with the secret, which the shell can't read. The egress
proxy (below) otherwise keeps the shell off every other local listener.

**The egress proxy** ([`act/shell/proxy.py`](ferb/act/shell/proxy.py), S2, D28, S3). It is the shell's only way off
the Mac. The daemon starts it on `127.0.0.1:<port>` with the shell, and closes it after the shell's processes at
shutdown.
- **Credentials.** Each shell process gets its own: `HTTP(S)_PROXY`/`ALL_PROXY` (and the lowercase twins) are
  `http://<process id>:<token>@127.0.0.1:<port>`, with `NODE_USE_ENV_PROXY=1`; `NO_PROXY` is empty when local pins are configured, otherwise the loopback list.
  - A missing, wrong or forgotten credential gets 407, and nothing is forwarded.
  - The token lives in memory only, and the redactor scrubs it from output, logs and spans.
  - Another process can't read it: macOS returns no environment for another process through `KERN_PROCARGS2`, even
    unsandboxed; `ps` is setuid, so it can't start inside; `task_for_pid` fails.
- **Asked before resolving.** Before resolving a name, the proxy asks `Gateway.destination_allowed(proc, host, port)`.
  Any remote host is allowed for a live process FERB tracks (L2); an untracked or ended process gets nothing. The
  owner's No to a host in the task stays final; local connections require executable-pinned permission. A refusal is
  403 and recorded under `refused`. The TLS server name is always checked against the same rule.
- **Connecting.** The proxy resolves names itself, because the sandbox has no DNS. It connects only to an address it
  checked, and refuses this Mac's own addresses: loopback, `0.0.0.0`, and any address an interface holds.
  - CONNECT tunnels any port as opaque bytes.
  - Plain `http://` is forwarded byte for byte, one request per connection, so each request is gated and recorded for
    its own host. A WebSocket upgrade (a `101` response) becomes a bidirectional tunnel; SSE and chunked responses
    stream until upstream closes.
- **The record.** Per process: host, port, `bytes_out`, `bytes_in` and connections, never a path, query, header or
  body. Byte counts are recorded, never capped. Refusals are aggregated by (host, port, why) with a count. An
  owner-excluded host shows as `excluded`.
- **Where the record goes.**
  - `shell.run`, `shell.wait` and `shell.read` return it as `network`. An IPv6 host is bracketed in the read source
    (`https://[2001:db8::1]`), so it parses; the label is then `external:web:2001:db8::1`.
  - The output is labelled `external:web:<host>` for each host the command reached. A command that reached none keeps
    `external:unlabeled`: what it read on disk can't be known.
  - When the task ends, each finished process's credential is revoked (`forget`), but its record is kept and journaled
    (`shell.network`) only once its last connection closes, so a setsid child still streaming through a tunnel after
    its shell exits has its late bytes counted. The connection is never killed. A connection that passed the
    credential check before `forget` but connects after it is refused (403, "this command has ended"), so no record
    is re-created that nobody would report. At shutdown, `kill_all` then the proxy's `close()` report every record
    still held, with its bytes so far and `still_open: true` when a tunnel is still open.
  - `ferb ps` shows `network` as a fact.
- **Abuse bounds, no owner-visible limit.**
  - A connection must authenticate within 10 s of being accepted: an absolute deadline, so dripping bytes doesn't
    extend it. A reaper thread sleeps until the earliest deadline.
  - At most 1024 connections may be pending authentication; past that the oldest is closed, so a flood never locks
    out a new client. An authenticated tunnel leaves the set at once and has no idle timeout, so streaming stays up.
  - A malformed request is a 400; a response head over 256 KiB is a 502. A refused WebSocket upgrade goes upstream as
    `Connection: Upgrade, close`, so a non-101 reply frees both sockets.
  - The daemon sets its open-file soft limit to the hard limit clamped to `kern.maxfilesperproc` (children see a
    finite `OPEN_MAX`), and handles `EMFILE` without a hot loop.
  - `ferb doctor` probes the proxy (expects 407), restarts a dead serving thread and says it did, and shows the last
    handler error's type. Nothing restarts serving after `close()`. While the proxy is down the shell has no network
    and fails closed. The server's `handle_error` keeps only the exception type, never request bytes (rule 14).
- **Removed.** The command-word heuristic (`guard.fetches_network`, `network_annotation`, `external:web:shell`) is
  gone.

**How wide loopback stays** ([`act/shell/listeners.py`](ferb/act/shell/listeners.py)). A contained command reaches:
- FERB's proxy;
- servers started inside FERB's sandbox, such as a dev server it started, or a test suite's own server.

It never reaches a server another program runs on this Mac. Such a server could relay off the Mac unrecorded (a local
proxy or VPN helper, an `ssh -L` tunnel, a DNS forwarder, a browser debugging port), or run code outside the sandbox
(a notebook server, an agent gateway).

How it works:
- When each command starts, `netstat -anv` lists every listening TCP socket and unconnected UDP socket, with its
  process. This takes about 3 ms.
- `sandbox_check` tells which processes run under FERB's profile. FERB's profile is the one that denies FERB's `run`
  folder and allows its workspace; unsandboxed programs are allowed both, and other sandboxes deny both.
- Every other port gets `(deny network-outbound (remote ip "localhost:<port>"))`.
- If the listing fails, every local server is out, and the proxy stays reachable.

Seatbelt ignores a port-specific deny after an unfiltered `(deny network-outbound)` or `(deny network*)` (measured).
So outbound is denied by filters instead: every remote IP, every unix socket, every kernel control. Each socket kind
Task 2 refused stays refused, and a test pins it.

What was measured (the Task 3 report has each run):
- Through the proxy: curl (http, https), Python `urllib`, git (`ls-remote` over https, a clone over http), Node
  `fetch`, Go and Codex CLI all work.
- With no network: Node with `NODE_USE_ENV_PROXY=0`, Python `http.client`, `nc`, `ssh`, and NSURLSession (it ignores
  the proxy variables) all fail closed.
- Profile build: about 7 ms with the owner's lists.

Residuals:
- A server bound to `0.0.0.0` accepts connections from the LAN: Seatbelt's `localhost` means any of the Mac's own
  addresses.
- A hardlink made before FERB is a different path and isn't covered.
- `mac.applescript` runs under the apps profile, so a script it runs can drive apps FERB is authorized to automate,
  within FERB's Automation grants; Seatbelt can't narrow Apple Events to particular apps. The shell profile denies
  the Apple Events service, so no program in FERB's shell can send them (each denied service is pinned by a
  `bootstrap_look_up` test).
- `~/.docker` on the never-read list also hides `~/.docker/bin`.
- **trustd.** It fetches the AIA, OCSP and CRL URLs of any certificate a contained process asks it to evaluate. A
  crafted certificate makes trustd send a GET, with data in the URL, to any host, outside the proxy (measured with a
  local listener). Denying trustd would end TLS for Go programs such as `gh`, because Go on macOS has no file trust
  store. Owner decided to deny it by default; D31 can lift only `trustd` for one exact command.
- **Listener timing.** The listener list is the one when a command starts. A server another program opens later is
  reachable from a long-running contained process until that process ends. For a `background=true` process, and for a
  detached (setsid) child of a foreground command, that window has no end. Polling to record those later connections
  is parked, not built.
- **Domain fronting.** The recorded host is the CONNECT host (or the plain-HTTP URL host). Inside a TLS tunnel a client
  can send a different SNI or `Host` to a shared CDN, so the recorded host may not be the site actually served. This
  matters for Task 5's per-destination gating.
- **Other system services.** Other services that fetch for their callers aren't enumerated: only nsurlsessiond, the
  resolver and trustd were measured. WebKit's networking process is unmeasured, because pyobjc WebKit isn't installed.
- **Name resolution.** NSURLSession can't resolve `localhost` once the resolver service is denied, so Apple-framework
  clients reach a dev server by `127.0.0.1`. curl, Python, Node and Go resolve `localhost` from `/etc/hosts`.
- **Owner-approved local servers.** `[protection].local_ports` pins each port to an absolute executable. The
  authenticated proxy verifies listener and connected peer before forwarding application bytes, then records actual
  address, executable, count and bytes. Since L24 a pinned port whose every listener runs the pinned executable when a
  command starts is also reachable directly by that command (section 16); a tree's No to one of this Mac's addresses
  keeps the command on the proxy route. Unknown owner snapshots fail closed; script/plugin execution by the pinned
  binary and the snapshot/exec race remain residuals.

**Owner clarification, 2026-10-11 (S4/D27/MP2).** Ordinary public browser navigation passes; literal loopback
navigation is local (a prior explicit No still applies). URL transfer checks use actual private source values,
including structured records and inherited private history/memory. Context producers record a typed source
payload separately from presentation metadata; legacy context keeps its complete text conservatively. A task's
conversation exposure stores only its payload, the shown entries' own texts (their display lines are in the model
turn's recorded context). Prepared URL evidence is cached incrementally and refreshed for late results and changed
file streams; a navigation prepares the payloads it hasn't seen in a worker thread (`Gateway._sink_texts_ready`, one
preparation per tree at a time), so a large conversation never stalls the event loop (final review M7). Contiguous file
chunks remain comparable when a value is encoded across reads. This is source matching, not semantic detection
of every transformed private value; common words in real private bodies may still collide.

Launch week (L2, 2026-10-13): the 2026-10-11 exact-command network approval is gone, and so are the `network`
argument, the `external_source` approval and the dead Q3 code (`_reached_hosts`, `sni_enforced`,
`owner.ask(allow_destination)`). A tracked process and its children reach any remote host through the proxy, which
records each one; an ended or untracked process reaches nothing new, and its record is journaled when it exits. The
proxy still checks host refusals and local executable pins separately, including TLS SNI. `external_source` is a
recorded declaration. P19 still blocks unknown-outcome retries. Encrypted application payloads are opaque. Payment,
permanent-delete and account-security floors stay.

**Confirmations** ([`policy/confirm.py`](ferb/policy/confirm.py),
[`policy/destinations.py`](ferb/policy/destinations.py), D23 floor, D33, S4, SQ1, SQ6, W2; migration `0026`). Before a
consequential call runs, and before any call whose agent declared a purpose, the gateway decides from facts whether
the owner must answer first (`confirm.decide`):
- **What is checked.** The physical kind the capability derives, plus `ActionSpec.declared`: a kind the agent's stated
  purpose adds (`mac.click`/`mac.pointer` `purpose` send/call: physical kind `mac_input`, `declared` send/call,
  `Target.contact` the contact it names, `ActionSpec.place` the app's name; a hover declares nothing). Since the
  launch week (L3, Task 18) a generic input (`click`, `fill_form`, `mac_input`, `applescript`) is also checked as the
  `effect` its worker declares (send, pay, destroy as `delete_permanent`, account_security) with the declared
  `destination` (recipients, none for a public post; the account it goes out from; amount; item) added to its target
  (`rules.declare`, `ActionSpec.effect_kind`,
  `rules.declared_kinds`); a declared pay or account_security on any acting call asks. A declaration can only add a
  question; `why` is never read, and the physical spec still keys P19 and the ledger. `noted_targets` keeps the
  declared contact out of P19's hash. Typed text that is wholly an email address, a phone number or a URL, the
  punctuation around it aside
  (`provenance.typed_destination`: "<a@x.com>", "a@x.com," and "a@x.com;" are a@x.com), is the call's recipient on
  `mac.act` type/set_value/edit and `browser.act` type. A phone shape needs at least 8 digits and isn't a date's shape.
- **The floor.** A payment and an account-security change always ask, and their Yes covers one run (SQ1). A permanent
  delete of a path the owner didn't name, and a send-class call (send, reply, post, mailbox send, submit, call) to a
  recipient or contact the owner didn't name, ask. A send FERB can't see the recipient of (a click declared to send or
  call with no contact; a send-class call with neither recipient nor place) asks as an unnamed contact, once per tree
  for that site and declared account, unless the owner named the site (and the account, when one is declared) in this
  request; an app's bundle id is never recorded as a host. A declared delete on a page is keyed by page and item.
  Two or more recipients the owner didn't name, or a permanent folder delete the owner didn't name, ask once with the
  count, size and a quoted sample (S4(c), L3); a send's question still names every new recipient.
- **Already authorized** (D33) is provenance: the value is in the owner's words for this request or in their answer in
  this task (`owner_words`, `owner_answer`), it is a known destination, or a standing scope covers the call
  (`standing.covers`: a send by kind or declared effect, same tool, identity, site, account and recipients; an any-body
  scope covers every post, an exact-text scope only a call carrying that text). A multi-word shown name counts as
  named when
  every word the owner used for it is a whole word of it (OPEN QUESTION 4: "Rahul" names "Rahul Verma"; "Rahul Verma"
  doesn't name "Rahul Attacker"). Where the owner's name ends is told by shape: from a capitalized word, the run of
  joined words that are capitalized (not a sentence's first word) or words of the value; from a lower-case word (or a
  script without capitals), every joined word, so then only the whole name counts. Which of several matches is meant
  stays the agents' question (D34).
- **The sink rule.** (a) A send-class call to a place with no recipient (a post, a submit) asks when the task tree has
  read the owner's own data (`private` among its tool results' origins) and outside content (an `external:*` origin in
  its tool results or in the request's own origins: pasted text, a clarify's earlier messages, a scheduled request's
  turn), and the owner hasn't named the place. The supervisor records the request's own origins on the root span
  (`input.request_origins`, `SpanStore.request_origins`); planner blocks and memory are not in them. (b) A recipient,
  contact or amount that came only from outside content asks, unless it is a known destination. The request's pasted
  parts are outside content for this too: provenance matches them as `external:paste` (MP2). FERB's own earlier
  question, wrapped the same way, is not a paste (`untrusted.pasted_parts`); it is matched with the outside origins of
  the planner turn that wrote it (its `chat.clarify` row, carried as `Request.asked_origins`), so a value the planner
  took from outside content into it stays outside content (`untrusted.earlier_questions`).
- **Known destinations** (`known_destinations`): the emails, URL hosts and phone numbers in the owner's words of each
  request (`named`, recorded by `run_tree`), destinations in the owner's answers to `owner.ask` (`answered`), and each
  destination of a Yes to FERB's question (`confirmed`). Never "new" again in a later task. Values are normalized,
  never message content.
- **Asking.** FERB asks before the call's span starts and before any lock is held, never through the orchestrator:
  an `owner.ask` span with `ferb_question: true`, `args.confirm` = the `{rule, key}` it covers, options Yes/No, the
  approval journaled as `ferb.confirm` (`ferb chat` shows "(Enter = No)": skipping is a No). Its question is one
  paragraph, control characters stripped, redacted; outside sources are named in the owner's terms ("the web page on
  kartikey.xyz", "a message, wa:123", "an email", "text you pasted", "something FERB remembered", "pages you
  visited", "content FERB couldn't label"; no internal ids or app ids), three at most and the rest counted:
  - `FERB: this pays <amount> to <recipients> on <app or site> from <account>.`
  - `FERB: this changes account security (a password, sign-in or recovery setting) on <site>.`
  - `FERB: this permanently deletes <path> (it skips the Trash).`
  - `FERB: this sends to <recipients>, whom you didn't name in this request.` (`this calls …` for a call)
  - `FERB: this sends in <app> to a contact it doesn't name.` (`this starts a call in <app> with …` for a call)
  - `FERB: this sends to <place>, a place you haven't named, after this task read your own data and outside content
    (<sources>).`
  - `FERB: this types <value>, which came from outside content (<sources>), not from you.` (a typing step; on a send,
    `the recipient <value> came from …`; `the amount <value> came from …`)
  - Several apply: joined with `;`. Then `The step: <tool and arguments>.`, `Reason given: "<why>".` when given (the
    task's words quoted, double quotes made single, 200 characters at most), and `Go ahead?`.
- **Only FERB's own checks start with "FERB:".** Every `owner.ask` question from a task, an orchestrator's rewrite
  included, is shown as `Question from the task: <one paragraph>`, so neither a newline nor a homoglyph can make a line
  of it read as FERB's. Its options stay separate, each one line, and one that starts with "FERB" (ignoring case,
  spaces, punctuation and NFKC compatibility forms) gets `(from the task) ` in front; the task gets its option back as
  it wrote it. A planner's chat reply or clarify marks each such line the same way (a homoglyph there is a residual:
  an owner's chat "yes" never stands in for a code confirmation). P19's question quotes what the task found
  (`What the task found: "…".`). The task's words in any FERB question (the call excerpt, the reason, what was found)
  are redacted before they are clipped.
- **Answers.** One row per need in `confirmations`, keyed by the task tree and the exact values (JSON of the tuple),
  so the answer carries across that tree's retries and workers (rule 9). Yes runs the call (its span's
  `confirmations` lists the rows). No, or a skipped question, refuses it (`the owner said no to this: …`, and later
  `the owner already said no to this in this task: …`, with `next` telling the task not to try another way).
  Questions never expire; a question a correction supersedes records nothing. An owner-driven retry, continue or
  steer reopens the tree's earlier Noes once (`confirmations.reopened` span; the action asks again if it recurs);
  the orchestrator's own retries keep them final. A scheduled run refuses (`… This needs you, and nobody is at chat
  (a scheduled
  run); not done.`). One question at a time per tree. A call the ledger refuses (P19) is refused at once, exactly as
  the ledger refuses it, and nothing is asked. The test-mode allowlist (send-class kinds and pay) refuses before any
  question.
- **One Yes, one attempt (SQ1).** A pay or account-security Yes is claimed (`used = 1`) under the tree's question
  lock by the call it covers, both a Yes found in the tree and a fresh one, so a concurrent identical call is asked
  again. Once the call's attempt starts the Yes is spent, an unknown outcome included. It is released
  (`Confirmations.release`) only when the call is refused before running, is cancelled before it runs, or the tool
  reports it failed. After an unknown outcome, only the identical retry the owner approved under P19 may use it once
  more (`Ledger.approved_retry`: the approved attempt's span lists the rows it used); a reworded retry is asked
  again.
- **Secrets (rule 14).** Keys and destinations are stored through the redactor; one the redactor changes is never
  known and never covers anything, so FERB asks again rather than let two secrets share one stored "[redacted]".
- **Snapshots before shell work** (carried item 6): see section 11. When none can be taken the command runs, the result
  says `snapshot_error`, and `ferb doctor` shows the last failure in local time, or that snapshots are off (OPEN
  QUESTION 3).
What reaches it today: no capability has the kind pay, account_security, delete_permanent or a send-class kind of
its own, so the floor fires for a generic input whose worker declared the effect (a browser post, a webmail send, a
payment, an account-security change, a delete on a page) and for `mac.click`/`mac.pointer` with a declared send or
call, and the sink rule (b) for a typed address, number or URL only outside content (or a paste) supplied. A declared
post on a page also raises S4(a) when the tree read private data and outside content and the site isn't named or
known. Residuals: an undeclared click that sends gets no question (code can't tell, rule 6; the prompts carry the
declaration, W3 and F5b's Jev); a declared effect on `shell.run` other than pay/account_security stays a note; a name
the
owner typed in lower case or in a script without capitals counts as named only in full; a destination named in any
request ("don't email bob@x.com" included), and a URL host named only to read, is known from then on; an address in a
free-text answer counts as the owner's even in a negative sentence (code doesn't read meaning, rule 6); a physical
send-class kind's site is taken as a place, so a future Mac tool must declare its purpose rather than carry a send kind.

### F5 completion paths (checkout only; final validation and owner install pending)

D31 grants are immutable `EscapeGrant` values bound to exact command, resolved cwd, rule and confirmation row. The
root question lock serializes ask/use; a grant is spent even when password entry or launch fails. The elevated path
uses fixed sudo argv, then sandbox-exec and a fixed supervisor whose stdin is reopened from the task PTY before the
command runs. The password travels only on sudo's input pipe; reference-counted redaction leases cover output through
process exit. A supervisor trap kills its elevated group on TERM; descendants that deliberately create new sessions
remain a process-cleanup residual. No unsandboxed command fallback exists.

`Redactor.field` owns secure-field, Secure Input and concealed-clipboard handling; text scanning adds contextual OTPs
and Luhn-valid payment cards to existing credential classes. The labelled classes (C1, owner decision 2026-10-15,
`labelled_secret_spans`): a 4–8 digit code (`482-913`, `48 29 13`, `G-482913`) with one of the owner's code words
(code, OTP, passcode, PIN, one-time password, verification; never inside a file name, nor in an HTML tag's name or
attributes such as "<code>" or `class="code"`: re-review 6) within `CODE_REACH` (8) words
on the same line and no sentence or clause break between (`_clause_break`: ";", "!", "?", a comma before a space,
and a dot only when a new sentence follows and the word before it isn't abbreviation-shaped, so "Rs. 2,000",
"a/c no. XX1234" and "Ref. No. 4417" keep a bank's OTP in reach): before it, joined directly (`OTP 482913`,
`Code: 482913`), after a link on the line (`… is 482913`, `login: 482913`) or in parentheses; or after it, joined by
"is"/"are"/"was"/"as" or "(". So "2024 code review", "Line 1234: code smell" and "PR 4821 is ready, please review the
code" stay. Across lines only a code alone on its line relates: to the label line above it (a code word within reach
of its end with no sentence end after it, colon or not: "Use this code to sign in"; blank lines allowed) or the
naming line below it (an email's layout, `_line_codes`); sibling page nodes are read the same way (`_siblings`). A
code alone may follow a quoted mail's ">" marks, a heading's "#"s and a list dash or bullet, and sit in punctuation,
brackets, quote marks, markdown emphasis or code marks and inline HTML tags ("[ 482913 ]", "- 482913",
"> **482913**", "<b>482913</b>", `_code_alone_span`; re-review 4 and 5); a quoted mail's ">"-only lines between a
label line and its code count as blank. On one line the same marks may sit between a code and its label or link
(`_MARKUP`: "Your code is: **482913**", "Code: <b>482913</b>"). Any whitespace but a line break (a no-break space,
an en, em or ideographic space, \x0b, \x0c) reads as a space in these classes (`_H`, re-review 4); line breaks are
"\r\n", "\r" and "\n".
Every code in a backup, recovery, 2FA or one-time code list ("Backup codes: 1234 5678 …", one per line, blank lines,
leading commas and item numbers or bullets such as "1. ", "2) ", "• " allowed) is masked;
another "codes:" label ("HTTP status codes: 200, 301") lists ordinary data (`CODE_LIST`). Address and coupon codes
are ordinary data (owner decision 2026-10-15): "PIN code", "pincode", "ZIP code", "Pin/Zip code", "postal code",
"coupon code", "promo code" and close variants are no code or secret words (`_address_codes`).
The value after a secret word (password, passcode, PIN, secret, API key, token, private key): any value after ":"/"="
(on a later line, blank lines between allowed, it is that whole line unless quoted; a quote that doesn't close on its
line quotes nothing, so the value is read as unquoted with its quote mark), one with a digit or symbol after "is"/"was"
(or, after "my/our … is", a plain word that ends its clause, a quoted word's after its closing quote), one with a
digit right after the word. A text field named
by a secret word hides its whole value; a code word hides a code-shaped one. A typed answer (`Redactor.answer`) to a
question with a password (also "pwd", "passwd"), passcode or passphrase word or a sign-in PIN is hidden whole, whatever
its shape or the question's phrasing (rule 14 over convenience: a worker asks "Which password manager do you use?" with
options, and a picked option is never hidden; the planner's clarify question, which has no options, never uses these
words, "pwd" and "passwd" included); to a question with a code word its 4–8 digit codes are masked. A chat message while
such a question is open (a worker's, or the planner's own clarifying question) is masked the same way, hidden whole for
a password question only when it is one word (it may be a steer); stopping stays the code's (`task.stop`, a question's
Cancel). A masked code counts as the digits it replaced, so masking twice changes nothing.
`RedactionStream` decides whole lines (a "\r" ending a read may still become "\r\n", so its line isn't complete):
the line being read is held while it holds a class word, an OTP introducer, a word that can still become one ("api ",
"sign ", "my wifi ", "coupon ", a list qualifier such as "Two factor "), or a number its link can still follow
("482913 is your …", marks and an unfinished tag between, spaces in its attributes too; a prompt such as "Delete 1234
files? [y/N] " appears at once), or that is a code alone so far, an unfinished tag at its end allowed ("(482913) <b",
`_code_alone_so_far`); a trailing digit run is held from its token's start, through "." and "," too, so a card is never
split. A line that starts where the last release ended is judged with its released start (re-review 6, I1). Of the complete lines only a last line that
is only a code is held (its naming line can follow), with the related lines above it; code-only lines never hold each
other, so number-only output passes one line behind. A label line, "Password:" or a list's label never changes with
what follows, so it is released at once and kept as context (`_forward_relations`, a line or two) that the next
release is masked with; the line under it is held while it can still relate (under "Password:" the whole value line,
under a list a line starting with a list token or item marker, under a label one that can still become a code alone,
its lead and marks included (`_code_alone_start`); its indent may be any whitespace the text redactor strips). Blank
lines between a list's lines keep it one list, and a label above a quoted mail's ">" lines stays context. When a read
releases the start of a line, its last words are kept (`_line_head`) so the rest is masked as part of that line. A
line too long to hold is held only from the class items still within reach of its end. A read works out each line's
relations once (`_line_kinds`, cached for that read), applying the masks before the labelled classes only to a line
holding a class word's stem, so its work is its new lines plus the line or two of context (re-review 4, P1). A fuzz
test (CRLF and lone "\r" splits, abbreviations, bracketed prefixes, number-only lines, prompts, real list labels,
quoted values on the next line, Unicode indents and blank lines, long whitespace runs, tags with spaces in their
attributes) checks that the stream masks every code `Redactor.text` masks, shows no word it hid or any word twice and
drops none; agreement is tested, not proved.
Both redactors run on the daemon's event loop, so their patterns and scans are kept linear in their input (re-review 6,
C1 and its final fix: `_LINK_START` read a space two ways, and a self-closing "<br />" matched `_TAG` two ways, so
runs of them backtracked exponentially; FERB's own `proxy.py` line 69 stalled a read past 20 s). Quantifiers that could
split one run several ways are possessive and a tag reads one way. Scans that would restart at every position of a run
read it once: `_urls` tries only a scheme run's first start, `_address_codes` reads a chain of places whole,
`_private_keys` stops at a BEGIN line with no END after it, `_jwts` skips the rest of a run once a start in it fails,
`_prefix_end` and `_numeric_tail` read only the line's end, a secret word's value is read within the value with its
text's facts worked out once (`_Values`), span lookups bisect, and a read finds its key and token spans once.
`tests/test_redaction_scaling.py` checks this by measurement, not proof. It doubles a run of each shape it builds:
whitespace (spaces, tabs, no-break and mixed Unicode spaces, line breaks), markup (`*`, `_`, backticks, quote marks,
tags, unfinished and self-closing tags), digits and number lists, numbers ending at marks, dotted, slashed and hyphenated
runs, BEGIN lines, minified JSON with secret keys, prose with unclosed quotes, and first words or "eyJ" repeated inside
one space-free token. Each run goes through every compiled pattern of `redact.py` and `images.py`, called the way the
modules call them (searched, or matched at a position; from 10 prefixes; runs up to 8 KB), and as whole texts through
`Redactor.text` (up to 32 KB) and one stream read (up to 16 KB, short of the long-line path). The test fails when a
call's time grows faster than its input: thread CPU time, nothing judged under 0.2 ms, a suspect ratio re-measured.
Shapes it doesn't build are not covered.
Residuals (re-review 6, parked): a quoted mail's password or backup codes with a ">"-only line between label and value
("> Password:" over ">" over "> hunter2!"; "> Backup codes:" over "> a1b2c-…") and HTML source with tag-only lines
between a label and its code show the value. Text in angle brackets that reads as a tag names no code
("<Your code is 482913>" shows its code). In the stream, a code alone in a tag whose attributes run past the released
line head (16 words, 256 characters) can show at a split before its naming line arrives. Over-masking: "the password is
'not set yet" masks "'not", and `.split("#token=")[1]` masks its tail. Costs: a line held whole is examined again at
every read, so a long held line arriving a few bytes per read costs time growing with its length squared (each read
stays bounded by `MAX_PENDING`); a card candidate is checked against every URL path span (quadratic with many of both);
some small per-line slices remain.
URL words never count (query credentials are the URL rule's).
`ImageRedactor` uses local Vision OCR and Quartz masking; `neighbour_spans` extends the labelled classes to the next
OCR line (a code alone under its code word, a value beside a line ending in a secret word), with the text redactor's
code-alone and whitespace rules (`_code_alone`, `_H`).
The gateway sanitizes every model/evidence image; RecordingManager sanitizes each frame before disk. Native capture
masks AX rectangles before returning bytes (section 16 has the L13 masking rules: a native capture is withheld only
when its boxes can't be painted; fields it can't locate are left to the OCR, and its `masking` says so). The
model-bound start image is the front window captured at send (L12); the first actual app/tab observation adds one
only when none was given, then only explicit screenshots return images. Browser snapshots retain their reference
generation during that capture. OCR misses, screenshot timing races and unclassified private text remain risks.

`browser.secret` seals hidden input to the command ID and HTTPS origin; `mac.secret` binds to the app PID and exact AX
node and uses only AXValue. Optional owner-confirmed handles persist nonsecret target metadata in
`owner-secret-handles.json`; values live in Keychain. A source install builds/signs `ferb-secrets` with the existing
local certificate and writes its designated requirement; FERB.app ships the helper in `Contents/Helpers` and setup
pins its copy to the exact code hash FERB.app's own signature seals (the `cdhash` when ad-hoc, the Developer ID team
later). Each call verifies that requirement. No unsigned fallback exists.
Python production browser seal callbacks and Keychain access route through that helper; isolated transport tests retain
an injectable crypto implementation. Developer ID distribution remains later work. Swift type-checking is not a live
Keychain/signature test.

Irreversible directory deletion is enumerated before its code question (count, bytes and exact paths); multi-recipient
send operations similarly name recipients and content size. Opaque shell effects cannot be inferred by code: their
snapshot/containment and shadow advice remain the boundaries. `fs.disk_usage` uses metadata-only directory traversal,
never file-content reads or symlink traversal, and omits FERB state. New configurations permit FERB's file tools to read
never-read stores through the redactor; an existing explicit false remains false. The shell keeps its deny list.

Jev's `action.destructive` is forced shadow even if enabled in config. Its labelled seed set has no claimed measured
accuracy or owner acceptance. `action.alignment` sees owner words and action metadata, logs advice and exposes active-task
mismatches to the orchestrator; it cannot block or authorize. Daily summaries include code-built new destinations,
confirmation answers, blocked flows and outside-source memory handles. `ferb stats` distinguishes observed shell latency
from isolated sandbox overhead. CLI-only `test.mode` restricts send-class actions (exact recipients, or
`"SITE @ACCOUNT"` for a declared post); it cannot constrain undeclared GUI clicks/typing. The owner setting persists
across restart. Regression cleanup restores the prior setting even after an error.

Full Disk Access is required in full setup only after doctor reports the shell profile and authenticated proxy available.
This checkout has not changed TCC, FileVault, the installed daemon or any personal browser. FileVault setup and all live
security/OCR/admin/pin checks remain owner acceptance items.
## Delegated conversations (F9)

`orchestrator/conversations.py` owns immutable scope/approval, durable incoming-event cursors, numbered turns,
owner pause/takeover and hand-over evidence. Migration 0033 stores these separately from owner chat continuity.
`plans.Step.conversation` is a concrete scope; a dependent converse step may instead consume researched
`conversation_proposals`. Every proposed contact scope waits for its own displayed owner approval. Topic and
lead judgments belong to agents, never a content keyword gate.

`Supervisor._conversation_steps` starts one ordinary checked worker per turn, then `ConversationRunner` waits
without a model session. Matching passive `trail.event` observations wake only the exact app/thread; missing
account metadata is a hint requiring a fresh account/thread read. Owner typed-text attribution pauses the pinned
window, or the whole app when the source cannot distinguish a window. `conversation.read` goes through the gateway
before incoming context is used so S4 sees the actual external/private source, including every continuation page.
Events retain `external:conversation`; they cannot grant scope, resume work, or become owner words.

The gateway calls `conversation_guard` before confirmation and immediately before driver input. A bound worker's
identity and physical target app/site must match; concrete account/recipient/contact fields are checked too.
Untargeted mutations, including shell, are refused in that worker. GUI metadata does not prove selected account or
contact: fresh inspection, the regular floor/sink rule and read-only checker remain necessary. P19 ledger identity
stays the same task tree across turns/restarts. Hand-over makes the final synthesizer read-only. Input already in
flight may have happened; cancellation never fabricates a failed-send receipt.

Owner-only conversation/notification RPCs expose exact scope, actions and receipts. `conversation_changes` and
notification rules in chat are quote-checked against actual owner text. A changed destination/topic needs a new
approved scope. Resume reopens only that saved converse step, leaving prior completed research/actions intact.
`attention.py` persists ordinary activity notices; `attention.ack` records that a client showed one. Incoming
notification triage remains shadow; ordinary attention invokes an observation-only model with no tools and cannot
create tasks. A requested ongoing conversation can still wake from its directly scoped incoming notification,
without relying on a triage verdict. Since L9, posting a notice to chat is telling it: notification `told` receipts
are written when the notice is posted, not on acknowledgment.

## 13. Documentation and diagrams

The [offline walkthrough](docs/walkthrough/index.html) is a local teaching aid, not an executable agent or live trace.
The [HLD Mermaid source](docs/diagrams/ferb-hld.mmd) maps responsibilities and the
[LLD source](docs/diagrams/ferb-lld.mmd) shows one task lifecycle. Obsolete 0.24 SVG exports were deleted; Git preserves
them. Current diagrams and walkthrough describe 0.26. Source-reference checks flag broken current-doc paths and
explicit qualified Python symbols. Historical changelog/audit evidence is intentionally exempt.

## 14. Change rule

Do not fix a failure by adding a keyword rule for that one failure. Expose platform facts, give the agent full evidence,
and test the general boundary. A filename match does not prove identity; a topic match does not prove teaching depth;
a delivered event does not prove completion; silence in an old trace does not prove zero desktop disturbance.

### F8 derived memory and learning

`memory/learning.py` adds immutable, deterministic derived records over existing permanent episodes and versioned
facts. `learning_sources` is the source graph, walked transitively at forget time (`descendants`); automatic retention
keeps evidence cards, while explicit owner forgetting erases dependent records, FTS entries (FTS5 secure-delete,
migration 0037: no per-forget rebuild) and vectors. Source-origin labels remain permanent.
- **Card lineage (fix wave 3, C1).** An episode card cites its task's direct sources only: `episode:<task>`, the
  planner exposure of the owner's message (`lineage.REQUEST_REFERENCE`), its own tool results and evidence
  (`call_sources`, handles taken as the entry or task itself: `lineage.direct`) and the selected procedure's context
  source (`lineage.PROCEDURE_LABEL`); never the chat entries, tasks or activity its planner was only shown. A refresh
  keeps the old card's lineage only once its trace has aged out. `CARD_MAKER` is `code:evidence-card/2`.
- **Missing sources (C1).** `_card_labels` tells them apart: a ref an explicit forget deleted has a tombstone
  (`forgotten_refs`, migration 0036: ids only, filled by `Learning.forget`; `learning_forgotten` for node ids and
  keys): the card is built on forgotten content, is never written, and its key is tombstoned (`card_key`), so no later
  capture rebuilds it; a ref holding the redaction marker is skipped (`memory.episode_refs_damaged`); any other was
  removed by retention, stays an edge and adds `{private, external:unlabeled}` (`memory.episode_refs_expired`). A
  forget deletes only the cards that cite a forgotten ref, directly or through derived nodes, and tombstones their
  keys.
- **Chat capture (I1, MP2).** `Daemon._remember` saves as owner_stated the whole sentence(s) of the owner's typed words
  that hold the planner's content (`items.owner_sentences`). It reads `Marked.typed_lines`: the owner's own words with
  their line breaks, each pasted or quoted part counting as one (`own_words`, which the rest of FERB reads, stays one
  line). Only a line break or a paste ends a sentence, never a comma, `.`, `!` or `?`, so "my prof. Zorblat" or "never!
  Share my links" never loses its negation, the saved text may be longer, and "Book my flight.\nI prefer aisle seats"
  or "Translate [paste] and never share my links publicly" saves just the typed fragment. Accepted residual: a paste
  inside a sentence splits it too, so "I don't [paste] like tea" saves "like tea". FERB's rewording is an inferred proposal offered by a
  `memory.confirm` FERB check (`_ask_owner`; Yes confirms, No retracts); a quote sharing no word with the content is
  refused. An owner_stated preference about the owner joins identity whatever the turn showed (owner decision
  2026-10-20: "No hold for my typed words"); pasted text never becomes owner_stated, and identity proposals remain only
  for inferred and derived items. `items.prompt_ready`: only active items, and observed patterns still gathering days
  (shown as "unconfirmed hint", `learning.shown_state`), reach prompts; pending inferred items are indexed as fact
  nodes in state `proposed`, which `Learning.search`, the entity rollup, `memory.learned` and `memory.read` all leave
  out, so only the dashboard (inventory `confirmable`) sees and confirms them. FERB's Yes/No memory questions are kept
  in `memory_questions` (migration 0041) until answered and asked again after a restart (`_restore_memory_questions`).
  A memory confirmed or retracted another way closes only its "Keep this?" question; a memory or learned item deleted
  by any path closes every question about it, the owner's "Forget for good?" too, which nothing else closes
  (`Memory.on_settled(ids, deleted)` → `_close_memory_questions`: superseded, never a No).
- **Forget echoes (I2, fix round Q1/Q2).** Each task records the memory items and learned nodes its exposures cite and
  its workers read through `memory.read`/`memory.learned` (`task_memory_refs`, migration 0039; `Gateway.memory_read`;
  a failure to record a read is journaled, redacted, as `memory.read_unrecorded` and never costs the read its result
  or span). `memory/echoes.py`, after a forget's deletion (`finish_at`: a worker thread on its own connection, queued
  behind any scrub still running, `Daemon._after_forgets`; `Supervisor.settle` makes the next chat turn wait): FERB's whole answer to a forgotten chat message (and the planner
  turn that wrote it); for tasks given the item (shown, read or cited the item itself, its source entries or rows, or
  a learned node whose own body holds its text or one of its rare words: `echoes.settle`, rare words measured once
  against every candidate) each line of their answer and outcome rows, stored goal and outcome, saved spec, permanent
  record, `task_results` row, every span of their tree and the planner turn that planned them, holding the item's text
  or a word of it nothing left holds; their cards are forgotten (keys tombstoned) only when the body restates the
  item; in chat rows whose turn listed the item, a line holding its text or two of its words in a row (one of them a
  word nothing left holds); elsewhere, its text word for word. The given tasks' spans lose those lines; every span
  loses the item's text and each removed line of two words or more, matched as whole words, case and spacing aside,
  inside JSON a span holds too (`echoes.scrub_told`, `_Words`: never a plain substring, never a single word). Append-only journal rows are deleted and written again with the same seq, id, time and
  origin. Running summaries that folded such a row go; a day narrative written from a forgotten chat message is
  deleted. A paraphrase in other words is not found.
- **Chat forget (I2).** The planner's `forget` output (item id and the owner's quote, whole clause(s) of their words:
  `items.whole_clauses`)
  makes `Daemon._chat_forget` ask a `memory.forget` FERB check (Yes/No, in `FERB_CHECKS`) naming what goes; the Yes,
  from chat (`Supervisor._answer` → `question_answered`) or the notch (`_resolve_approval`), runs the forget.
- **Learned-item forgets (I2).** Every path (dashboard, chat Yes, what a memory forget deletes, cards forgotten for
  restating the item, the consolidator's clean-up of rollups written on a source forgotten meanwhile) goes through
  `Daemon._forget_nodes`: `Learning.forget`, then the questions about the deleted nodes close. An item the owner named
  (the one asked for, or one a waiting "Forget for good?" names) gets an echo pass (`echoes.finish_node`, queued like
  a memory forget's). FERB's naming of it in that question (`_named_node`) goes from FERB's rows and every span. What
  each forgotten version states (body text, a procedure's intent, a lesson's trigger) goes where that version was
  shown or cited, never across every span (`echoes._Index.shown`, each table read once): the tasks given it
  (`task_memory_refs`: answer and outcome rows, records, tree spans and planner turns), the chat turns that listed it
  (chat rows whose source refs cite it, and those requests' planner turns), every span naming it by its node id (the
  attention check has no request, so no exposure), the notices of an attention check among those spans, and for an
  owner identity statement every task created and every span of a task or recording a model run's system prompt since
  that version was recorded (compared in UTC). Journal rows in scope lose each line holding it; spans and notices
  lose it as whole words, so a one-line output keeps the rest; cards of those tasks that state it are forgotten. Only
  a version's statement (a lesson's or identity's text, two words or more, whole) also leaves every FERB chat reply
  and notice wherever written; a trigger, an intent or a key never does unscoped. Copies of the FERB lines removed
  (two words or more) go from every span as whole words. An episode card's goal is never scrubbed (other tasks share
  it), nor a key alone (a key can be a label: "owner", a subject, a task id). What a learned-item
  forget leaves: a procedure's steps shown to a worker stay in that task's spans; items deleted only because they were
  built on the forgotten one (and named by no question) are deleted with no echo pass; a lesson's scope labels (app,
  site, intent) stay where its key was listed; an entity's multi-line text is found only where it was recorded exactly.
Only the owner RPC can write identity or confirm a proposal; task capabilities are read-only. Content is versioned
with valid/recorded time; state and success receipts are derived metadata. Consolidation batches can be invalidated
without resurrecting deleted data. The existing database filename stays unchanged for installed-data compatibility.

`memory/consolidate.py` uses a no-tool runtime during idle periods, validates cited verified episodes and actual
instantiated tool arguments, creates scoped failure/success lessons and pending identity proposals, and records
per-source job completion. Code rollups cite their child cards.
- **Incremental (fix wave 3, I3).** A pass reads again only cards whose inputs changed (`Learning.cards_to_read`:
  `learning_card_inputs` holds each card's input digest: composer, episode `built_at`/status/verified, latest verdict).
  Pending episodes go to the model grouped by `card_shape` (tools and apps), one call per group with compact
  `card_digest`s, that group's procedures and owner refusals, and only the consolidated cards related to the new ones
  (`_examples`: the same action shape and a request term most of the group lacks, as `Memory.recall` filters terms; or
  cited by the group's procedures and lessons), split by `_chunks` to fit `runtime_limits(rt)` (a card too large for
  the window is never sent; its job says why; procedures that alone fill it are named) and by `_within_chars` to fit
  the runtime's per-input character cap (fix wave 5; a card no input can hold whole is never sent and its job names the
  cap). A failed card read is tried once more, then parked until its inputs change. `learning_jobs.inputs` holds the
  group digest: a failed job is retried when its group changes or on `ferb memory consolidate`
  (`catch_up(force=True)`); a failed distill day waits for its summary row to change (`distill_runs.inputs`) or
  `ferb memory distill DAY`.
- **Learning calls (fix wave 5).** Each consolidation and distillation call runs through `memory/calls.learning_call`
  (assembly within the window and the character cap, the run, the schema check) and is one `model.learning` span: role,
  call id (`sleep-…`, `distill-<day>`), runtime and model as input; outcome, input characters and estimated tokens and
  the documented limits as output; the runtime's redacted error (Codex's own message) as the error; never the input;
  an error Codex retried is a `model.retry` child; a call cancelled by FERB stopping is outcome `cancelled`, no error;
  the runtime's schema mismatch (`invalid_answer`) is the error with its reason. The call id of a consolidation is its
  batch id (both hash the sorted card ids). No task owns it (aged out as a loose span); its usage spans name the
  learner's role (`_record_learning_usage`). `ferb stats` counts the calls and failures with the top errors by role
  (grouped with their numbers set aside) and names the latest failed call; `ferb trace <call>` shows the span
  (`SpanStore._task_rows` matches `call`). A day over the cap fails distillation with `InputTooLarge`.
- **Rollups off the loop (I4).** `_rollups_off_loop` runs `rollups` in a worker thread on its own `Database`
  connection, never once a task has started, stopping between days; a day summary row already indexed is skipped.
  Afterwards each node it wrote is checked against the forget tombstones, and one built on a source forgotten
  meanwhile is forgotten.
- **Owner refusals (M3).** `Learning.put` refuses a procedure, identity proposal or lesson whose key or text matches
  one the owner retired. **Procedure state (M1, M6):** slot variants count only receipts that supply the declared
  slots; "current" versions are the installed ones, so an update demotes to candidate, and only two consecutive
  failures mark suspect. A planner `procedure_id` that isn't a current procedure becomes a `procedure.ignored` span
  and the task runs without it (I5). Task completion captures app versions; old episodes
retain explicit unknown versions. F6's callbacks retrieve candidates before planning, load the selected procedure,
keep owner-only identity in context and record procedure outcomes; none bypass gateway confirmations or checks.

`memory/embeddings.py` implements optional pinned local MLX inference and int8 vectors, indexed by model/month.
Keyword/trigram search always works; hybrid ranking is enabled only by an exact-model owner-history comparison.
A Python cosine path is available when the optional SQLite extension is unavailable. Model preparation downloads
public artifacts only; all embedding inference is local. Model selection and measured memory benefit are pending
owner-history evaluation, not implied by compilation. `eval/memory.py` exports history and compares paired run facts.

`memory/dashboard.py` serves static local assets on an ephemeral 127.0.0.1 port. An ephemeral fragment token is copied
to an Authorization header and removed from browser history; exact Host and Origin, no CORS, no request logs, CSP,
no external assets and DOM textContent protect the owner controls. Each request uses a fresh owner-authenticated
Unix-socket RPC; SQLite stays on the daemon's thread. Workers never receive the dashboard token or write RPC.

## 15. Durable task loop and release measurement (F6/F10)

`orchestrator/plans.py` defines do/map/reduce/wait/ask/converse dependencies and validates complete inventories and
batch context feasibility. `plan_store.py` persists the plan, completed outputs and event cursors (migration 0028).
`plan_executor.py` runs ready steps and resumes incomplete work. The supervisor uses ordinary workers, gateway
security and final verification for plan work. Explicit continuation of the same plan keeps the root and step IDs,
completed notes and unknown-action ledger; reopens only unfinished steps; and preserves the owner's latest request
in the durable record. A changed plan starts a distinct attempt with the previous evidence. New sends after an ended
plan re-display the standing scope because its prior grants were revoked. Task rows use an upsert so resumed IDs do
not delete referenced evidence or creation timestamps.

`cli/live_chat.py` maintains input while independent work streams run; planner submissions from a conversation stay
ordered. It prints the daemon's ack and never answers a question on its own: typed text is always a `chat.send`
(L7, L19). Hidden questions wait for Enter on an empty input line. Emergency stop and explicit task stops remain code
paths. `cli/attach.py` exposes a managed PTY output stream and optional exclusive input lease; a disconnected client
releases the lease. Owner skills are catalogued from local owner-written files, with no symlink traversal or automatic
network installation.

P3 context labels protect identity, request/conversation and task state. The raw last-ten-minutes trail is
fetch-only through `trail.query` (L28); today's matching activity goes in every planner turn.
F8 supplies individually ranked fact, episode, procedure and lesson candidates; each is a complete block with its
stored origin and retrieval reference. Since L28 the recalled items and every ranked learned item are protected
(always in the planner's prompt, `Supervisor._memory_blocks`), like the situation and today's activity; only the
conversation and optional blocks give way. Omission references and protected overflow remain explicit.

`eval/release.py` exposes packaged owner-supervised definitions through `ferb release-checklist`; it never executes
or accepts a check. `eval/comparison.py` compares saved baseline JSON without daemon RPC or model calls. Missing
measurements remain null, denominators are retained and aggregate deltas are not treated as task acceptance.
Prerelease packaging includes native/browser protocol resources, migrations, runtime prompts, evaluation sets and
local dashboard assets. Installing and the owner's verdict remain separate from checkout validation.

## 16. Launch week (L1–L28), as built (checkout; owner install and live checks pending)

The owner's decisions of 2026-10-13/14 are in
[the launch-week spec](docs/superpowers/specs/2026-10-13-launch-week-fixes-design.md) and the plan's "Launch-week
clarifications"; `docs/0.27_DECISIONS.toml` maps L1–L28 to their tests. What follows is the design as built. Nothing
here is live-accepted yet ([owner acceptance](docs/OWNER_ACCEPTANCE.md), "0.27 launch week").

### 16.1 Questions and security (L1–L5, L24)

**Direction (L1).** Trust, record, recover: a small code-enforced core, everything recorded, FERB's own file changes
recoverable. The fuller prevention design is after launch (spec section 6).

**Any site, every site recorded (L2).**
- `Gateway.destination_allowed` allows any remote host for a process FERB tracks while it runs
  (`ShellManager.live_owner`); an ended or untracked process reaches nothing new. Loopback still needs its
  executable pin. The tree's No to a host (from the browser's URL question, under the shared SQ6 key) still holds for
  the shell, and the TLS server name is always checked against it.
- A background process that ends after its task is journaled (`shell.network`) when it exits.
- `ferb/policy/sites.py` `sites_contacted(spans, tree_id)` lists hosts from the tree's `shell.network` rows and its
  browser open/navigate calls that asked for an http(s) URL (unknown outcomes included; `use_tab` excluded), and
  every page load the extension reported in the tree's own tabs (trail `browser_tab` rows FERB made for the tree's
  tasks: a click, a redirect's landing page, a form, an adopted popup; I6). A tab in FERB's identity profile is
  attributed to its task too (`Attribution.decide`). Not counted: requests a page makes without loading a new page,
  native apps' network use, and page loads while sensing is paused. The supervisor appends "Sites contacted: …" to
  the full answer, never to `display_text`.
- Accepted residual: after reading a malicious page, the shell could send one of the owner's files somewhere.

**What asks (L3).**
- The code floor: payment and account-security changes (always; one Yes covers one run), permanent deletion the owner
  didn't name, and a send as the owner to someone new unless a standing scope covers it (section 12, Confirmations).
- S4 (L4): FERB's own context exposures count as outside content only when every label they hold is outside
  (`origin.stored_outside`, `SpanStore.outside_exposure_origins`, `Gateway._outside_read`), for both the taint and
  S4(b) (`tool_outputs(include_context=True)`). `confirm.url_carries_private` counts a URL piece only when the matched
  form is a value (`_is_value`: a digit or a non-word character); ordinary words never count, base64/hex decodings
  still do. A value whose every private occurrence is inside a URL of the destination's own host doesn't count either
  (`_same_site_only`, `_host_had_it`: the same host with or without "www.", or a host under the one it was seen on;
  IDNA hosts from `urls.parse`): sending a reel's id from the owner's own www.instagram.com tab back there discloses
  nothing new (owner live use, 2026-10-14). A sibling host still asks, so on shared-suffix platforms (`*.vercel.app`,
  `*.github.io`, `*.netlify.app`, …) one owner's token never counts as known to another's subdomain, with no suffix
  table; the same value going to another site, or also found in other private text, still asks. The front window's outside
  labels join the tree's request origins (L12), and a scheduled run's request
  origins are the creating message's own labels (`schedule.created.request_origins`; an old row fails closed).
- Bulk asks: two or more recipients the owner didn't name, or a permanent folder delete the owner didn't name, ask once
  with the count, size and a quoted sample (`confirm.bulk_delete`, `SHOWN` = 3, display only).
- Worker prompts never ask the owner to confirm; workers bring other questions to the planner. A named tool that isn't
  installed is asked about once, with the install command prepared (owner, 2026-10-14).
- D31 escape-hatch commands and P19 retries still wait.

**Answers, steering and plans (L3).**
- The planner routes a free-text answer by question id (`AnswerQuestion {approval_id, answer}`) to any open
  `owner.ask`, `plan.ask` or FERB check, from any chat. The answer recorded is the owner's own words, never the
  planner's reading; pasted text is never an answer. The rest of that turn (a control, a new task, memory, reminders,
  schedules) then runs as in any turn (`_carries_more`); when FERB asks "which one?", the whole message waits for the
  reply instead (final review M1).
- FERB's checks (`FERB_CHECKS`: `ferb.confirm`, `ferb.retry` for P19, `conversation.approve` for F9, `memory.forget`
  for a chat forget) resolve only on a
  plain Yes or No routed to that id that the planner's reading agrees with. A reply that could answer another open
  question, or the planner's own pending clarify, gets "Which one is it for?" (`_Focus` carries the earlier Yes).
- `Daemon._hide_code_reply` turns a reply that is wholly a code into `[redacted]` while a question asks for a code,
  for typed and spoken text alike; voice transcripts are chat turns and never auto-answer.
- Every planner control carries two required decisions: `standing_sends` ("unchanged" or the full revised list) and
  `supersedes` (the question ids the correction changes). `steering.settle_questions` touches only those; a named
  FERB check closes as `superseded`. `steering.restate_scope` narrows at once and, on a run the owner started, grants
  a widened scope only after the steer's reply has gone out (`grant_stated`), and only while that statement is the
  task's latest `plan.stated`.
- A big job's plan is stated (`_plan_statement`: steps, estimates, scopes, "Here's the plan; I'm starting now. Reply
  to change anything:"), journaled `plan.stated` and granted under `stated:<journal id>`; work starts at once.
- On a run the owner started from chat, a stated or corrected scope is granted only when its site, account and
  recipients are in what the owner typed: the request's words with its corrections and follow-ups, their `owner.ask`
  answers in the tree, or the words typed behind an owner-stated memory item (`Supervisor.owner_typed_scopes`, the
  schedule path's `standing.owner_named` and `typed_behind_memory`). Any other scope is stated as not covered
  (`steering.not_typed`, `plan.stated.not_granted`), so its sends get the floor's usual questions (final review I1).
- An owner-driven retry, continue or steer reopens the tree's earlier Noes once (`Gateway.reopen_noes`,
  `confirmations.reopened`); a reopened browser-URL host No is re-checked from the facts; the proxy keeps its No.
- Questions never expire (`Approvals()` has no clock). A question closes when answered, when its task ends, or as
  superseded. The setup persona question is created once and posts no banner.

**Schedules and standing scopes (D30, L3).**
- `_schedule_create` records only scopes whose site, account and recipients are in words the owner typed this turn
  or typed behind an active owner-stated memory item (`Daemon._schedule_scopes`, `standing.owner_named`,
  `_typed_behind_memory`; the setup persona, `PERSONA_KEY`, never counts; same-turn captures add nothing). The
  confirmation lists recorded and not-recorded scopes; `schedule.created` journals `standing_sends`, `not_recorded`
  and `request_origins`.
- Each run is granted exactly the recorded scopes under `schedule:<id>` (`SCHEDULE_GRANT`) as its tree starts,
  checked again just before (`_schedule_live`: active or done), and revoked when the run ends. A scope the run's
  planner states is not granted unattended. A correction to a run nobody started from chat (a scheduled run, or a plan
  resumed after a restart) keeps each kept scope under the statement it was granted under
  (`StandingPermissions.in_force`, `grant_each`), so `schedule:<id>` stays and a cancel still reaches it; nothing new or
  wider is granted to such a run (`grant_stated` refuses it), and the reply lists the new scopes as not covered
  (`steering.not_covered`, the plan line's wording). Cancelling or pausing a schedule revokes its grant from running and
  suspended trees (`StandingPermissions.revoke_approval`, journaled `standing.revoked`); a grant that lapsed is
  journaled `standing.withheld`. A one-time schedule is marked done just before its last run; from then until that
  run ends (`Daemon._last_runs`, its planner turn included), or while a run holds `schedule:<id>`
  (`StandingPermissions.held`), it stays listed (`schedules.list`, the planner's facts: "done; its last run is under
  way") and a cancel or pause withdraws the run's scope (`_changeable`, final review M9); the grant's own re-check
  (`_grant_in_force`) refuses a scope granted after it. A pause leaves it done (it has no future runs, and the reply
  says so) with `schedule.pause` journaled after its `schedule.run`, which `_schedule_live` reads.
- The security digest lists every call that ran under a standing scope (`standing_sends`).

**Calls a runtime drops (spec section 8.1).** The MCP shim waits with no cutoff. Codex drops a call that outlives its
own per-call tool timeout (300 s measured) without telling the shim; the shim passes `call_id`
(`<threadId>/<callId>`), the Codex adapter names the failed call, and `Server.abandon` releases it (`caller_left`).
The question stays open; the identical retry in the same task waits on it (matched before rewording, P19 on FERB's
whole text). An action that completed after its caller left returns its stored result to the identical retry
(`completed_in_an_earlier_attempt`). Runtime per-call timeouts keep their defaults (owner, 2026-10-14).

**Local services (L24).** `local.direct_ports()` keeps a pinned port only if every TCP listener on it runs the pinned
executable when the command starts; the profile then adds `(allow network-outbound (with report) (with message
"ferb p-<id>") (remote tcp "localhost:PORT"))` after the deny. One `log stream` (`DirectReports`, only when
`protection.local_ports` is set; under FERB.app it runs under the app's plain `python3.12`, `paths.helper_python`)
counts the kernel's reports per command tag. Records carry `local_direct` (lower-bound counts, `null` when unknown);
a stopped stream is journaled `shell.local_reports_stopped` and shown by `ferb doctor`. Limits: the pin is checked once
per command; counts are a lower bound; no byte counts; the kernel line has no IP.

**Protections that never ask (L5).** Secrets as handles, credential stores and FERB's state locked, the sandbox, proxy
recording and recovery copies stay. New: the shell can't write any Trash (section 12), and FERB.app's code is
protected FERB state (16.9).

### 16.2 Chat flow (L6–L8, L19, L20)

- Every `chat.send` event carries `request_id` (the id journaled with `chat.message`) and `at`; task events carry
  `task_id` ([RPC contract](docs/RPC.md)).
- **Ack (L7).** `Daemon._chat_ack` puts `ack {request_id, text, message, at}` on the stream before the per-session
  routing lock. `acks.PHRASES` is the P25 list (29 phrases, the starred one dropped); `pick` never returns one of the
  last eight journaled `chat.ack` rows (`RECENT`, across chats and restarts). `message` is the daemon-redacted text;
  `chat.ack.received_at` records arrival.
- **Plan line.** `run_tree` yields `plan` as soon as the planner returns, before the disturbance baseline sample;
  journaled `chat.plan {request_id, task_id}`.
- **Live line (L8).** `Observer.tool_started` → `WorkerRuns.tool_progress` → `progress.describe(tool, args, kind,
  site, app)` puts `progress {task_id, text, started_at}` on that tree's own queue. The text is the tool's verb and
  target (a browser-parsed host, an app named from the Mac's app list or "an app", a file's basename), never model
  text; a failing line journals `progress.failed` and never stops the call.
- **Latency.** `ferb baseline` adds `latency_s {ack, plan, answer, context}`.
- **CLI (L19).** No client-side auto-answer: typed text is always a `chat.send`; the CLI prints the daemon's ack.
- **Notch app.** Renders the ack, plan line and live line under the owner's turn; files the plan line in chat, not the
  activity feed; places a late answer at the bottom as a reply quoting its turn, which reads "Answered below" (L6);
  shows a check mark for `verified`; drops an event naming another turn's `request_id`. Questions are inline cards and
  typing stays enabled; a secret field takes focus only on an explicit owner action. Notices render with a "noticed"
  tag and are acknowledged (`attention.ack`). Results are read with `chat.status {peek:true}` and acknowledged with
  `results.ack` once placed. The learning digest comes from `memory.learning {op: "digest"}` with Confirm / Not right.
  Recovered tasks show on FERB's side, never as the owner's words. Paste provenance is kept without a dialog.
- **History (L20).** The unsent draft lives only in memory. A sent message is saved only as the daemon's redacted
  `ack.message`; a turn never acknowledged is saved as "Message not saved — sent before FERB confirmed it".
- **Yield.** FERB orders its windows out while its file picker, another app's menu or floating window over it, or (for
  the pinned chat) an app the owner brings forward under it is in the way; it never toggles click-through.

### 16.3 Answers (L11)

- For failed, needs_user and done_unverified outcomes, `display_text` is code's status label ("Done, but not
  confirmed:" and so on) plus the short text the dispatcher writes from the checker's verdict; the findings stay in
  `answer`. A needs_user outcome adds FERB's `Question: …` line. The literal "Verified. " prefix is gone (`verified`
  is the marker).
- Code states side outputs in plain words (reminders, memory, cancels, schedules); a refused one adds one line and the
  reply is kept. `timer.set {timer_id, request_id, quote, set_in}` lets a reminder authorize once per source message
  (`timers.request_id`); a reminder's quote may come from any owner-typed message of the same chat, or be several
  of them joined by blank lines when every part is exactly one owner message within this request's chain of owner
  words (a reminder asked over a clarification; the first part's message is the source).
- A `continue` with `control.new_goal` re-plans inside the same tree (`PlanStore.replace` for a different plan); a
  retry stays in its tree.
- A non-owner stop says why in plain words (the class is journaled as `task.interrupted`). Failure replies are
  journaled as `chat.reply`; a scheduled run's failure is named (`SCHEDULED_SORRY` otherwise), with `chat.failed`.
- A steer replies "Switched to: …" or "Added your correction: …".

### 16.4 Seeing (L12, L13)

**Front window at send (L12, [`sense/front_window.py`](ferb/sense/front_window.py)).**
- `_chat_send` starts the capture right after the ack, alongside routing. The front window is the topmost normal
  on-screen window whose owner isn't FERB: FERB's apps (`FERB_APPS`: `com.ferb.ferb`, `dev.ferb.notch`,
  `com.ferb.app`, and `com.ferb.daemon`, the agent app the packaged daemon runs as), the daemon, or a bundle-less
  descendant of it. The app hosting the sending `ferb chat` is skipped.
- Facts: app, bundle id, title, and for a browser the URL and title of the active tab **of the captured window**
  (Safari, and every Chromium browser whose AppleScript has tabs: Chrome and its channels, Chromium, Brave, Edge and
  its channels, Vivaldi, Arc; not Opera) (AppleScript, only with Automation already granted; a kept grant bridges a
  pending check). `browser_tabs.applescript_windows` lists every window (bounds, title, active tab; fields apart by
  ASCII separators) and `window_tab` picks the captured one: the window at its bounds (2 pt, the capture tolerance);
  when several could be (maximized windows share bounds; a browser that won't give bounds), the one whose title or
  active tab's title is the captured window's window-server title. No single match: no URL or tab title, and the
  block's `tab_note` says why. (Live case 2026-10-19: AppleScript's `front window` is the browser's own frontmost
  window, which was another Brave window, so the planner got a Kaggle URL for a Greenhouse page.) When the capture's
  paired-profile extension matched the window (protocol 15: bounds, ties broken by the active tab's title, and the
  answering tab's title equal to the captured window's, or to its title now, after a retitle), its tab (`Shot.tab`,
  `driver.page_tab`) is used instead, unless the AppleScript read found several windows that could be the captured
  one (`browser_tabs.Ambiguous`: two windows at one place with the same tab title can fool the extension's match;
  round 3, m3); the AppleScript read stays the fallback. The block's `tab_source` says which read gave the URL. An
  answer whose tab title isn't the captured window's is rejected for masking too (`OTHER_PAGE`: a paired profile's
  window at the same place showing another page, e.g. over an unpaired capture), so its fields are never painted
  over the wrong picture. The tab watcher still reads the front window (M1, two lines). One screenshot of that
  window by window id, masked and
  through the F5 `ImageRedactor`, stored as evidence kind `front_window` under the request id.
- An excluded site is captured under `external:web:excluded` with the URL shown as `[excluded]`; an excluded app gives
  only `[excluded]` (not named, not looked at); `[mac] snapshot_image = false` keeps the facts only.
- The planner gets a protected Layer 2 block (data in `<untrusted-data>`, origin the window's labels) and the image;
  it may answer a screen question directly. The worker for that message gets the same picture as its start block
  (`gateway.initial_image_given`). Failures journal `screen.capture_failed`; a model that rejects the image is
  retried once without it (`screen.image_refused`), only for service or protocol errors.
- The planner waits for the capture at most `Supervisor.front_window_timeout_s`, ScreenCaptureKit's own capture
  timeout (`screenshot.TIMEOUT_S`, 10 s; no new number). Past it the capture is cancelled, `screen.capture_timeout`
  is journaled and the block says the front window couldn't be captured in time (I4: a hung or huge front app
  delayed every message without bound).
- Residual: image tokens aren't counted in the context budget.

**Redacted, not refused (L13).**
- `pixels_withheld` is gone. Every image goes through masking and the `ImageRedactor`; `Withheld` (a `Refused`)
  is raised only when masking fails: on a native capture (`driver.window_masks`), only when the boxes found can't be
  painted on the picture (`UNPAINTABLE`); on a tab capture, the extension's own cases below.
- Browser tabs: the extension's `maskState` reads secret-field rects before and after the capture, one look per tab
  at a time; `mask.page_masks` paints fields that stayed put and withholds a field that moved, appeared or vanished,
  or a page that navigated or resized. A child frame is linked to its host only by a trusted per-round announcement;
  a child frame holding secrets is painted whole; a transformed host is painted whole; mirrored or filtered secrets
  (box-reflect, SVG `url()` filters, `::details-content`) withhold, as does a frame with secret fields no host
  accounts for: a secret the extension found but can't cover.
- A tab whose page the extension can't read (page or top frame unreadable, e.g. a browser error page, or the look
  ran out of time: `maskState`'s `unread`) is still captured (protocols 15–16; owner: no screenshot refused). Tab
  captures (`readyForCapture`, rounds 3–5) leave a tab already attached fully as it is; keep a page scripts can't
  reach armed with the debugger alone (no full attach, so no detach and re-attach per capture: overlapping captures
  and a recording's frames don't collide, and nothing held is handed back, D35); and give a scriptable page the input
  setup and readiness probe (focus emulation, FERB's viewport: the emulation background captures were measured with,
  2.7), whose failure refuses no capture (`input_note`, `browser.screenshot`'s `input`), and
  `browser.screenshot` asks for the picture even when its snapshot failed. The reply carries `unread`, the look before
  the capture when only the look after failed (malformed: withheld, m5), and the tab's window (`tabWindow`: bounds,
  whether the tab is the one it shows, minimized, the tab's title, `emulated` when FERB lays it out at its own size).
  `_TabDriver._unread` paints that look's fields and, when the tab is the one its window shows and isn't emulated,
  the secret fields Accessibility finds in the page area of the window showing that tab (`MacComputerDriver.
  page_secrets` through the daemon's `native_secrets` hook: the window by the tab's title, its own title the tab's,
  m6; read just after the capture, scaled from the area onto the picture). The gateway's text recognition runs as on
  every image. The note (`masking` in `browser.screenshot`'s result and a browser.snapshot first look; a recording
  frame's note) says what was painted (m7). Such a shot resolves no `browser_point` (`_Tab.unread_shot`).
- A page the debugger can't attach to at all (the Chrome Web Store, `chrome://` and other extensions' pages: no tab
  capture possible) answers `withheld` with `undebuggable` and its window (protocol 16). When the tab is the one its
  window shows and that window is on screen, the daemon captures the window natively (`_native_window_capture`, the
  router's `native_capture` hook: `mac.window_image` of the one on-screen window there that carries the tab's title,
  even when it is the only window there, round 4 rule 5; AX and the extension's masking and note, then text
  recognition) and says it is the whole browser window; such a shot resolves no `browser_point`
  (`_TabDriver.pointable`), and `browser.screenshot`'s `next` says so. Otherwise it stays withheld with the actual
  reason (the window minimized, another tab shown, no window with that title on screen, screenshots switched off, the
  app excluded, the capture failed): the only tab capture that gives no picture, besides a secret the extension found
  but can't cover. Recording frames of such a tab follow the same rule (round 4).
- Native captures of a browser window use `window_secrets` from the owning profile, placed through the page's AX web
  area; AX-only masking is recorded as `masking`. When no profile can tell, `_web_secrets` says why
  (`WebSecrets.why`: no paired profile of that browser connected, none of a profile's windows or more than one at
  that place, more than one profile, no answer, or the extension's own reason), and the note carries it with what
  Accessibility showed ("no page area" / "page area is empty"), so a live trace explains itself.
- Each look's AX window is the app's window at the captured bounds; when several are there (maximized windows share
  bounds) the one with the captured window's window-server title (`accessibility.window_with_frame(…, title)`).
  A pick by title is confirmed (review M7): the window server still gives the captured window that title (read again
  by its id), the picked window is still there with it, and no other window there has it; otherwise the look reads
  no window (the capture's note says Accessibility couldn't read it) rather than walking a possibly wrong one.
- Each look asks the extension while its AX walk runs, and the look after a capture doesn't ask again when nobody
  answered the first (`WebSecrets.late`, kept even by a look that found no AX window): the worst case (a ~2 s
  re-read and a 4 s ask per look) is ~6 s, ~8 s when the first ask answers slowly and the second then times out,
  inside the planner's 10 s wait for the front window; one after the other it was
  12 s, and the planner went on with no image, URL or title (review M6;
  `test_a_browser_capture_fits_inside_the_planners_wait_for_the_front_window`, virtual time). The front window's
  AppleScript tab reads (5 s timeout each) are outside this bound.
- Chromium builds its web AX tree when an Accessibility client asks; Brave 1.97 and Chrome 155 don't contain the
  string `AXManualAccessibility` (checked in their framework binaries, fix wave 4; spike 06 saw the same on Chrome
  154 / Brave 151), so `expose_tree` only helps Electron apps. `read_tree` re-reads a browser's window for ~2 s while
  it shows no web area yet as well as an empty one (it used to return at once when no web area had appeared).
  `AXEnhancedUserInterface` (the VoiceOver signal Chromium does honour) is not set: it switches the owner's browser
  into its full screen-reader mode for every page and is known to disturb window managers; that is the owner's call.
- Native captures are never refused for where their secret fields are (owner: "no screenshots should be refused; go
  with redacted stuff", "no exceptions"; live case 2026-10-19, where the front-window block said "pixels withheld"
  and FERB asked which internship). `window_masks` paints what it found and says the rest in `masking`:
  - a page neither the extension nor Accessibility could read (no answering profile, and no AX page area or an empty
    one): the AX fields elsewhere are painted, the page is "masked by text recognition only" (`PAGE_UNREAD`);
  - a window Accessibility can't read in either look (`WINDOW_UNREAD`), a secret field with no box
    (`FIELD_UNPLACED`), page fields with no page area (`PAGE_UNPLACED`): left to text recognition;
  - a field or page item that moved, appeared or went between the looks, or a page that navigated or resized
    (`FIELD_MOVED`, `PAGE_MOVED`): painted where each look saw it, the rest left to text recognition;
  - whatever one look located stays painted when the other look can't be used (review I1): a look that couldn't read
    the window (`WINDOW_HALF_READ`, plus `FIELD_UNPLACED` / `PAGE_UNPLACED` when the readable look couldn't place
    everything either; `WINDOW_UNREAD` only when neither could), a moved page whose other look had no
    page area (`PAGE_PART_PLACED`), the extension answering one look only (`ONE_LOOK`: a field only the other look
    could show is left to text recognition, review M5);
  - painting a page look by look paints its secret fields, the frame hosts either look paints whole and frames that
    reloaded between the looks, wherever each look saw them; never a frame clean in both (`_look_items`, review M4,
    round 2).
  - the AX walk and the extension ask run together; a walk that fails cancels the ask (round 2).
  The picture always goes through the F5 `ImageRedactor`, which every image path runs. `masking` reaches
  `mac.screenshot`'s content, a first look's facts, the front-window block and its evidence facts
  (`FrontWindow.masking`), and a recording's `masking` list. Residual (accepted by the directive): where fields can't
  be located or placed, a secret the OCR can't recognise as one (an unlabelled shown password, a code with no label)
  can show; a field moving during the capture could be caught between its two places.
- The AX inventory also masks text fields whose title, description, placeholder or identifier says passw/pwd, and
  walks the whole tree for secret fields.
- Recordings paint each frame (notes as above); a frame whose boxes can't be painted is skipped (`frames_withheld`,
  `withheld_reason`).
  `recording.inspect` returns its contact sheet to the model.
- Accepted residuals: other CSS pixel-copy effects (drop-shadow on transparent fields, view transitions) rely on the
  OCR layer; a hostile parent page timing its looks could expose an embedded frame's field to the model provider.

### 16.5 Hearing (L26)

- `audio.listen {app | tab, seconds, language?}` (`act/listen.py`, scope `audio`, observation): ScreenCaptureKit
  captures that one process's audio (FERB's own excluded; a tab means its whole browser process) into a 0600 WAV in
  `workspace/audio`. A start that doesn't answer within the capture timeout is stopped, best effort (M3).
- ScreenCaptureKit failures (`screenshot.capture_error`, I7) keep the error's text, domain, code, the failure reason and
  recovery suggestion from its userInfo and its underlying error's domain and code, in the tool result and the trace;
  `error_name` journals `CaptureError (<domain> <code>)`. `screenshot.capture` refuses a window Accessibility reports
  minimized (`accessibility.window_minimized`), by name and before ScreenCaptureKit is asked; FERB never unminimizes it.
- `Transcriber` checks FERB Voice's signature against `[hearing] requirement`, then runs
  `open -g -n -W <FERB Voice> --args transcribe` with FERB's home (`Transcriber(home=)`). FERB Voice requires on-device
  recognition and never prompts from a task.
- FERB Voice runs only with the owner's own FERB home (`macapp/FERB/home.swift`, the same rule as the agent stub):
  `FERB_HOME` is taken only when unset or empty (then the account's `~/.ferb`, from the account database, never
  `$HOME`) or when it is that folder by device and inode; anything else exits 64 before FERB Voice does anything, in
  every mode. So no other process can point its microphone, speech or transcripts at a socket or folder of its own.
- The WAV is deleted as soon as the listen ends; the result holds the redacted transcript and segments, labelled
  `external:audio:<bundle>` (plus the tab's page). Every listen is traced (app, seconds). A task's end or a stop ends
  it; `Hearing.sweep()` clears leftovers at start.
- `browser.media` plays, pauses, mutes, unmutes or seeks a snapshot's media element (protocol 14); snapshots report
  each element's state. `playing` is true only once the element's time has moved while unpaused (content.js `moving`:
  its `timeupdate` events, or a change between two looks; a play, pause, seek, stall or reset needs fresh movement),
  never from `!paused` alone, which play() sets before any playback; `paused` is reported beside it (I5).
- `with_audio` recordings attach a time-aligned transcript and keep no audio track.
- `ferb setup` asks for Speech Recognition only (`request-speech`); `doctor.check` reports `hearing`; push-to-talk is
  on-device only too, and so is the notch's own dictation (`OnDeviceSpeech` in `Microphone.swift`): without on-device
  recognition for the language it says so before tapping the microphone (final review M5).
- From FERB.app the helper is `Contents/Helpers/FERB Voice.app` (`com.ferb.app`), pinned without building.

### 16.6 Perception in chat (L9, L10)

- **Delivery (L9).** `_post_notice` journals `chat.notice {id, text, quotes}` (origin `external:observation,inferred`),
  writes told receipts, and publishes `attention.notice`. A macOS banner is sent only when no `logs.subscribe` stream
  wants that topic (`_client_shows`, from `_log_subscriptions`); a waiting question's banner (`_approval_notice`)
  follows the same rule and names the question only. `chat.status.notices` lists unacknowledged notices. A notice has
  no session id: it belongs to every conversation the owner chats in (`continuity.in_conversation`: the notch's
  `chat-…`, voice, the daily one), so the planner and `chat.history` see it there; never a scheduled run's
  `schedule:<id>` session (final review M3).
  Migration 0035 marked the 73 legacy notices delivered and records an install floor (`perception_floor`, the highest
  trail `seq` then) so old trail rows never flood Jev; Notification Center records already there at the first read are
  marked seen and not recorded (`Perception.notification_floor`).
- **One moment, one notice (L10).** At most one consideration runs per app (`app_of`: a bundle id, or a tab's browser);
  yes batches arriving meanwhile merge into the next one, and a flush publishes each app's yes answers together.
- **Understanding.** The prompt (`prompts/attention.md`) gets the merged rows (time, actor, task), the owner's memory,
  the recent chat, running tasks and the notices already told about that app (`told(apps)`). The recent chat is the
  current conversation since its last notice, after FERB's running summary of the older chat; with no notice after
  the summary's watermark, every entry the summary hasn't folded in (`_conversation_blocks(since_last_notice=True)`;
  I2, owner 2026-10-14). It is still budgeted from the model's window like the planner, as a safety, with no other
  cap. Quotes come back separately and their origin is assigned by code.
- **FERB's own actions.** Events in an app while a FERB action runs there are actor FERB (`Attribution.acting`).
  FERB's own banners are recognised by exact title, body and poster (`banner_posted`, `com.apple.ScriptEditor2`).
- **Audit.** `trail.attention` is live; its decision rows record the effect ("told in chat: notice …").
- **Purge.** `expire_derived` clears a notice derived from expired typed or clipboard rows from the notice row, its
  journal entry, told receipts, span inputs and outputs, and the exposure payloads that showed it (`notice_payload`,
  `_scrub_payloads`).
- **Bounds.** The told list for an app is bounded by trail retention (30 days), since it joins the notices' trail rows.
  Residual: a `task.evidence` listing preview cut at 500 characters may keep a notice fragment in that tool call's
  output until trace retention (30 days).

### 16.7 Planner context (L28)

- **Window.** Every adapter reports `ModelLimits` per model, with the documented source; the budget is the window
  minus the reply limit. Codex uses its catalog's input window (`context_window` × `effective_context_window_percent`:
  258,400 for `gpt-5.6-sol`), read from FERB's own `models_cache.json` or the pinned binary's bundled catalog
  (`~/.ferb/run/codex-bundled-<version>.json`, written offline). Reading a window only reads these files; the dump
  (`codex debug models --bundled`, a process) runs at the daemon's start in a worker thread before it serves
  (`_dump_codex_catalog`) and at `ferb setup --codex-login`, never on the event loop (final review M8).
- **Unknown window.** A `ServiceError` in chat, `runtime.limits_unknown` at start (only for a role whose window is
  unknown, for example with no pinned Codex catalog reachable), and a `context window <role>` line in `ferb doctor`
  (`doctor.check.runtimes.limits`). A Claude runtime with no `[models] claude` fails explicitly.
- **Character cap (fix wave 5).** Codex 0.157.1's app-server refuses a turn whose text input items together hold more
  than `MAX_USER_INPUT_TEXT_CHARS = 1 << 20` characters (Rust `chars()`, i.e. code points), before any model sees it
  ("Input exceeds the maximum length of 1048576 characters."); `codex exec` goes through the same `turn/start`, and
  `thread/start`'s instructions are not counted. `codex_exec.MAX_INPUT_CHARS` holds it, `INPUT_CHARS_SOURCE` cites the
  tag, files and lines, and `limits_for` reports it as `ModelLimits.max_input_chars` (None for runtimes that document
  no cap). `assemble(..., max_chars=)` fits the whole text sent as one input to it as well as to the token budget: chat
  and optional blocks are kept only while both fit, and protected text over the cap raises `InputTooLarge` (refused by
  name, never sent to be rejected). `WorkerRuns._run` (planner, dispatch, worker, checker, summaries) records a
  `context.too_large` span with each block's reference and character count, then raises it: never a `ServiceError`
  (so never the planner's image-refused retry; images count 0 characters), and the owner reads that FERB didn't send
  it (`worker.too_large_problem`, via `told`). The running summary folds what fell out in calls that fit
  (`Supervisor._foldable`: the latest summary plus the oldest entries that fit, repeated). `CodexRuntime.start` guards
  the text it sends too (`check_input`). The cap is in the daemon status and `ferb doctor`
  (`setup.context_window_check`).
- **Order.**
  - Stable: Layer 1, then the conversation (a header with FERB's running summary when one exists, then one cacheable
    block per entry).
  - Situation, always included: Layer 2 (time, owner activity, front window), today's matching activity, relevant
    memory.
  - State: attempts, running tasks and open questions (with each running tree's `standing_scopes`), plan repair, the
    owner's message.
  - The conversation fills what is left newest-first; the rest becomes one fetch note.
- **Running summary.** `chat.summary` (`prompts/summary.md`) is written in the background only when uncovered entries
  fall out; it never folds notices, cites what it folded, and is deleted when a folded entry is forgotten.
- **Fetch only.** The raw last-10-minutes trail goes to no prompt; it is reached through `trail.query`.
- **Speed.**
  - A turn cites shown entries and tasks as `entry:`/`task:` handles; an episode card cites none of them (F8 card
    lineage).
  - `DailyActivity` keeps an in-memory term index: one indexed search per turn, off the loop.
  - The query embedding is encoded off the loop.
  - The planner snapshot and exposures are written after the plan line.
- **Episode capture** runs after the outcome in a child process (`[sys.executable, "-I", "-m",
  "ferb.memory.card_reader"]`, request on stdin, the database derived from `FERB_HOME`). Writes are serialized
  (`_capture_card`) with a generation counter so a verdict given during a capture stays on the card. Shutdown cancels
  pending captures (`memory.episode_deferred`) and kills readers. Missing sources follow F8's rule (forgotten: no card;
  damaged: skipped, `memory.episode_refs_damaged`; expired: kept); a replayed receipt respects the owner's fail
  verdict (`memory.receipt_replayed`).
- **Tokenizer.** Counts use FERB's own pinned o200k copy (`context.tokenizer_path`: `~/.ferb/cache/tokenizer`, put
  there by `ferb setup` via `cli/tokenizer.ensure`), else tiktoken's cache, each checked against the pinned SHA-256;
  `doctor.check.tokenizer` reports the method.
- **Ids.** FERB's ids are protected by key (`Redactor.ident` for `task_id`, `span_id`, `session_id`, `run_token_id`,
  `request_id`, `covered`, `source_refs`, `procedure_id`, `approval_id`, `timer_id`): only a generated hex body is
  spared
  exact-value replacement, and secret patterns still run over the whole value. `Redactor.id_ref` is gone.
- **Usage.** `model.turn` spans record `cached_input_tokens`.
- **Measured** on replays of real turns (provider time excluded): message → plan line 11.4–17.0 s → 0.9–2.0 s;
  reusable prefix about 8k → 54k tokens.

### 16.8 Broken features (L16–L18, L21–L23)

- **Learning (L16).** The card pattern needs a token of its own, so hex ids never match; a card inside a URL's path
  segment is kept, query and fragment are still scanned; OCR masks any Luhn-valid run. The consolidator schema uses
  `{name, value}` pairs (strict-compatible). `security_digest` accepts the string form of `network`. `_memory_pass`
  runs distill, consolidate and the embedding restore in separate scopes (`memory.distill_failed`,
  `memory.consolidate_failed`, `memory.embedding_restore_failed`).
- **Typed text (L17).** `key_text` reads the whole string; terminal apps (`TERMINAL_APPS`), secure fields, Secure Input
  and password fields (shown ones included, `capture.password_element`, `redact.password_field`) are never captured;
  no digit-shape rule.
- **Choice answers (L18).** Only sealed inputs call `remember_hidden_secret`; a picked option (`Approval.option_for`) is
  never redacted; other words to a code-labelled question are hidden for that answer only (`Redactor.answer`).
- **Codex account errors (L21).** Any account-check failure becomes `AccountError('account_unavailable')` and a named
  reply for that request; scheduled runs name it too. A failure reaching the service (`ServiceError.network`: an
  account check that isn't a login problem, a dropped stream) while the Mac has no network route
  (`sense/network.has_route`: a UDP connect asks the routing table for an IPv4 and an IPv6 route; nothing is sent)
  says the Mac seems to be offline instead of "try again shortly" (`worker.problem_of`).
- **Waits (L22).** `PlanExecutor` re-checks the wall clock at most every `POLL_S` (`timers.recheck_in`).
- **Inventory (L23).** `plans.inventory` and its protection filter run in a thread.

### 16.9 Distribution (L25, L27)

**Extension auto-install (L25, [`autoinstall.py`](ferb/act/browser/autoinstall.py)).**
- For every catalogued Chromium browser whose `Local State` exists, `ferb setup` and `--update` write a Chrome Web
  Store install request in its `External Extensions`: Chrome and its channels and Chromium (documented); Brave and
  Edge (unconfirmed); none for Vivaldi, Arc and Opera (Load unpacked).
- Setup registers the native host for the pinned and store ids, reports per profile from Preferences (read only), and
  records the request files it wrote in `~/.ferb/extension-requests.json` (so a changed or cleared id, and uninstall,
  remove them).
- `STORE_EXTENSION_ID` is the live listing's id (`plbfkifcmgkgnlfgikdeaaglgncdhkma`, 2026-10-20); `[browser] store_extension_id`
  overrides it, and an empty value opts out: with no id, setup prints the Load-unpacked steps. Setup never restarts a browser or approves its prompt.
- `ferb browsers extension package` builds the store zip.
- Discovery, pairing and doctor cover every catalogued browser. Two copies of the extension in one profile aren't
  handled yet.

**Packaged FERB.app (L27, [`scripts/build-app.sh`](scripts/build-app.sh)).**
- **Bundle.** `com.ferb.FERB`, "FERB", the notch UI as main executable; uv's standalone CPython 3.12.13 with `ferb` and
  its locked dependencies in `Contents/Resources/python` (precompiled, never written); the `ferb` launcher; Codex in
  `Contents/Resources/node`; `Contents/Helpers/ferb-secrets`; `Contents/Helpers/FERB Voice.app`; and the agent
  `Contents/Helpers/FERB.app`.
- **The agent** ([`ui/mac/FERBAgent`](ui/mac/FERBAgent/), `com.ferb.daemon`) is a native stub embedding the app's
  libpython. launchd runs it as `-m ferb.daemon`, so macOS names FERB in permission prompts, Privacy & Security and
  Login Items. It accepts only `-m ferb.daemon`, `-I -m ferb.cli.mcp` and `-I -m ferb.memory.card_reader`, and only
  the owner's `~/.ferb` (by inode). It rebuilds a fixed environment with every `PYTHON*` variable removed.
- **Signing.** `scripts/signing.sh` is the one signing step: ad-hoc by default with the hardened runtime
  (disable-library-validation only when not Developer ID); `SIGN_IDENTITY` with notary credentials adds timestamps,
  notarization and stapling; `build-app.sh --sign-only` re-signs without rebuilding.
- **First launch.** `AppSetupPlan` reads the LaunchAgent and runs `ferb setup` or `--update` in a console window;
  another installation is replaced only after the owner says so.
- **Install refusals.** Install refuses a translocated app, a read-only volume (the DMG) or a bundle failing
  `codesign --verify --deep --strict`.
- **Install steps.** Helpers are pinned to the exact code FERB.app's signature seals. `~/.ferb/node` links to the
  app's Codex. `ferb` is linked at `~/.local/bin` (no shell-profile edits). A checkout's setup takes launchd back.
- **Protection.** FERB.app's bundle is protected FERB state (section 12).
- **Recordings** write only through the output folder they checked and keep open; frames are staged in
  `~/.ferb/run/recordings`, swept at start. A stop cancelled midway still finishes the recording once: the receipt is
  finalized through the held folder, the folder is closed, staging and the sound are deleted, and the task's end finds
  it finished and never encodes again.
- **Residuals** (owner-accepted or live checks): ad-hoc builds lose macOS grants and may prompt the Keychain on update;
  whether launchd runs the bundled Python from a quarantined download is untested; the app is about 666 MB;
  `mac.applescript` keeps Apple Events, so a script could ask Finder to move Trash items (not a file rule).

### 16.10 Journal kinds added in the launch week

`chat.ack`, `chat.plan`, `chat.notice`, `chat.summary`, `chat.summary_failed`, `plan.stated`, `standing.revoked`,
`standing.withheld`, `timer.set`, `task.interrupted`, `progress.failed`, `screen.capture_failed`,
`screen.page_secrets_failed`, `screen.native_capture_failed`, `screen.image_refused`, `runtime.limits_unknown`,
`shell.local_reports_stopped`, `memory.consolidate_failed`,
`memory.embedding_restore_failed`, `memory.embedding_unavailable`, `memory.episode_refs_damaged`,
`memory.episode_deferred`, `memory.receipt_replayed` and `store.wal_not_truncated`. `schedule.created` gained
`standing_sends`, `not_recorded` and `request_origins`; `attention.failed` records `batch_ids`.
