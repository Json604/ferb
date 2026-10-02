# Code tour (0.20.0)

The flows through FERB's code, in the order they run, by `path:function` (no line numbers; `rg -n "def name"`
finds them). The design contract behind every piece is [ARCHITECTURE.md](ARCHITECTURE.md); section numbers (§) refer
to it. `docs/DESIGN_ONE_WORKER.md` describes flows 1 and 4; `docs/PLATFORM_MACOS.md` §12 and
`docs/PLATFORM_BROWSER.md` §13 describe flows 2 and 3.

## 0. Before the flows: types, interfaces, registry

- `ferb/core/types.py`: pure data. `Identity` (`OWNER`: your Mac, browsers, shell; `FERB`: FERB's own profile and
  accounts), `Task`, `Outcome`, `Target`, `DataLabel`, `ActionSpec` (what a tool call *would do*: its kind, target,
  content, data labels; policy decides on this, not on the raw arguments), `ToolContext`, `ToolResult` (`image` goes
  to the model only, `evidence` to FERB's checks only), `ComputerAction`, `BrowserAction`, `PageElement`,
  `PageSnapshot`, `TrailEvent`, `MemoryItem`.
- `ferb/core/interfaces.py`: the seams as Protocols: `Clock`, `AgentRuntime` (`start` / `events` / `continue_` /
  `cancel`), `Capability` (`describe(args) -> ActionSpec`, `run(args, ctx) -> ToolResult`), `BrowserDriver`,
  `ComputerDriver`.
- `ferb/registry.py:build` registers the always-present tools (`fs.*`, `owner.ask`); `add_owner_browsers`,
  `add_identity_browser`, `add_mac`, `add_shell`, `add_trail`, `add_time` add the rest when the daemon has that
  driver (`ferb/daemon/main.py:Daemon._build_agents`, `_build_browsers`, `_build_mac_and_shell`, `_build_timers`).
- A capability's `scopes` (e.g. `{"mac:act"}`) decide who may call it: `ferb/daemon/gateway.py:Gateway.tools_for`
  lists a tool to a worker only if its token holds the scopes and identity. `ferb/policy/rules.py:SCOPES_BY_IDENTITY`
  is the identity floor; `with_reads` adds the read scopes every act scope implies.

## 1. A chat turn → task → worker → tool call → driver → outcome

```mermaid
sequenceDiagram
    actor Owner
    participant CLI as ferb chat (cli/chat.py)
    participant D as Daemon (daemon/main.py)
    participant Sup as Supervisor (orchestrator/supervisor.py)
    participant RT as Runtime (runtimes/codex_exec.py)
    participant MCP as ferb mcp (cli/mcp.py)
    participant GW as Gateway (daemon/gateway.py)
    participant PE as PolicyEngine (policy/engine.py)
    participant Cap as Capability (act/*)
    Owner->>CLI: a message
    CLI->>D: stream chat.send over ~/.ferb/run/ferb.sock
    D->>Sup: send(session_id, text)
    Sup->>RT: _supervise: supervisor.md, no tools
    RT-->>Sup: reply | clarify | task (goal, identity, targets, conditions)
    Sup->>Sup: dag.validate (identity floor, one repair); requests.record
    Sup->>RT: run_tree → _worker → WorkerRuns.run_worker (token, tool list, worker.md)
    RT->>MCP: spawns `ferb mcp` with the token file
    MCP->>D: tools.call → Gateway.call
    GW->>GW: strip effect + why; describe(); effect = max(declared, floor); taint; contact; duplicate send
    GW->>PE: evaluate(ActionRequest)
    PE-->>GW: allow | ask | hold | block
    GW->>Cap: _execute: ledger intend → run → redact → span
    Cap-->>GW: ToolResult
    GW-->>D: redacted result (+ time footer, + image)
    D-->>MCP: tools.call response
    RT-->>Sup: worker JSON (status, summary, evidence, artifacts)
    Sup->>Sup: _reconcile (inside _worker); verify; _final
    Sup-->>CLI: events: plan, subtask, verification, outcome
```

**The CLI.** `ferb/cli/__init__.py:main` parses the command; `chat` → `ferb/cli/chat.py:main_chat` → `run_tty`
(prompt_toolkit; pastes arrive whole and `wrap_pastes` marks them). `Chat.turn` streams `chat.send`
(`ferb/cli/client.py:Client.stream`) and prints each event through `render`. Questions arrive as
`approval.requested` events; `Chat.answer_approval` asks (`question`) and replies with `chat.approve` (yes / no / an
option number) or, for a worker's own question (kind `owner_question`), `chat.answer {approval_id, text}`.

**The daemon.** `ferb/daemon/server.py:Server._connection` checks the peer uid; `_hello` authenticates the role
(`ferb/daemon/auth.py`: CLI, a runtime with a run token, the native host, FERB.app); `_dispatch` / `_stream` route to
the method table built in `ferb/daemon/main.py:Daemon.start`. `Daemon._chat_send` runs the turn as its own asyncio
task and merges the supervisor's events with bus events for that task (tool calls, approvals, holds). Closing chat
detaches the stream; Ctrl-C calls `Daemon._task_stop`.

**Deciding** (`ferb/orchestrator/supervisor.py:Supervisor.send`). `ferb/sanitize/untrusted.py:mark` separates your own
words from pasted or quoted text. `Daemon._availability` (`ferb/orchestrator/availability.py:build`) lists what's
usable now and the scopes a worker may get; `Daemon._environment` adds one informational line
(`ferb/policy/services.py:services_named`, `summary`; the named browser via `Daemon._named_browser` /
`ferb/act/browser/profiles.py:named_in`). `Supervisor._supervise` builds the prompt from `prompts/supervisor.md`
(`_system`), the recent conversation (`recent`), memory (`recall_items`), the environment and the time block
(`ferb/clock/context.py:time_block`), and runs the orchestrator runtime in supervisor mode with no tools. The model
answers (`ferb/orchestrator/schemas.py:SupervisorOutcome`) with `reply`, `clarify` or `task` (`TaskSpec`: goal,
identity, informational `targets`, `success_conditions`), plus `remember`, `timers`, `schedule` entries.
`Supervisor._decide` runs `ferb/orchestrator/dag.py:validate` (the identity floor only); one repair round, then a
plain question (`COULDNT`). `Supervisor.send` then records the request's facts for the gateway
(`ferb/policy/targets.py:Requests.record`: your own words, targets, your email) and calls `run_tree`.

**One worker** (`Supervisor.run_tree` → `_execute` → `_worker`). One task `<tree>.worker` with
`dag.scopes_for(identity, available)`: everything its identity allows that FERB has now. `_context` gives it the
goal, conditions, what you named and the workspace. `ferb/orchestrator/worker.py:WorkerRuns.run_worker` →
`_run_worker_once`: guard check (`ferb/orchestrator/guards.py:TreeGuard.can_start_worker`), a run token
(`ferb/daemon/auth.py:RunTokens.issue`), the tool list (`Gateway.tools_for`), the prompt `prompts/worker.md` plus
context and the time block, then `_run`; `_watch` polls heartbeats (`Activity.stalled`, 180 s) and the tree caps
(`TreeGuard.over`: 4 h, 50 worker runs, 1,000 tool calls; `TreeGuard.call`: the same failing call 3×, loops). A
worker that never loaded its tools and gave up gets one retry. `ferb/runtimes/codex_exec.py:CodexRuntime._cmd` runs
`codex exec` in a read-only sandbox with native tools off, FERB's locked model catalog (`locked_catalog`) and `ferb
mcp` as the only MCP server; `_watch_trace` scans the request trace (`ferb/runtimes/tooltrace.py:tools_in`) and kills
the run on any tool outside the allowlist (`Daemon._runtime_violation` unregisters the runtime). The Claude
equivalent is `ferb/runtimes/claude_sdk.py:ClaudeRuntime` behind `ferb/runtimes/proxy.py`.

**The shim.** `ferb/cli/mcp.py:Shim.list_tools` / `call_tool` forward to `tools.list` / `tools.call` with the token
read from `FERB_RUN_TOKEN_FILE`; `Daemon._tools_call` validates the grant and calls `Gateway.call`.

**The gateway** (`ferb/daemon/gateway.py:Gateway._call`), in order:
1. the tool is in this worker's list and the task is running;
2. `effect` and `why` are popped from the arguments (`ferb/policy/effects.py:is_valid`); the rest validate against
   `cap.Input`; `cap.precheck` may refuse (e.g. an undeclared `mac.click` in a messaging app);
3. `owner.ask` is answered here (flow 4) and never reaches policy;
4. `cap.describe` → `ActionSpec`; `effects.floor_of(spec.kind)` (`FLOOR_BY_KIND`) and `effects.raise_to` give
   `effective = max(declared, floor)`; a send / pay / destroy without a `why` is sent back for one;
5. taint labels from owner-account reads ride on anything the worker types or sends (`ferb/policy/taint.py`);
6. the contact check for calls and sends (`_contact`, `ferb/policy/contacts.py:check`: "Which Rahul?"), then
   `targets.owner_named` (did your own message name where this goes);
7. the duplicate-send check (`_send_again`);
8. a `tool.call` span is started with the effect and reason; `PolicyEngine.evaluate`; `ask` → `_await_owner`;
   Jev's stricter-only check (`_jev_action`); `hold` → `_wait_hold` (the undo window); resource locks
   (`ferb/act/locks.py`) → `_execute`.

**The policy engine** (`ferb/policy/engine.py:PolicyEngine.evaluate`): stop → identity and scopes
(`rules.check_identity`, `check_scopes`) → exposure (`ferb/policy/exposure.py:ExposureTracker.check`) → `navigate`
runs → data flow (`ferb/policy/dataflow.py:check`) → `ferb/policy/autonomy.py:ask_reason` (pay / destroy always;
send unless `owner_named`; injection signals; careful mode) → an unverified account asks once →
`ferb/policy/approvals.py:Approvals.consume` / `request` → the undo hold for `UNDO_WINDOW` kinds (`Holds.hold`).

**Execution and the trace** (`Gateway._execute`): a ledger row before any non-reversible kind runs
(`ferb/orchestrator/ledger.py:Ledger.intend`, `mark`); `cap.run`; an exception marks the row `unknown`; a refused
owner folder triggers the one-time folder question (`_folder_access`); read sources feed taint and exposure; the
result is redacted (`ferb/sanitize/redact.py`), what was on screen is remembered for contact checks (`Seen.record`),
the span is finished (`ferb/store/spans.py:SpanStore.finish`) and the reply carries the time footer
(`ferb/clock/context.py:footer`). `ferb trace` renders `Daemon._trace` → `SpanStore.tree` through
`ferb/cli/admin.py:render_trace`.

**Finishing.** The worker's `summary` is your answer. `Supervisor.verify` (flow 5), `_reconcile` for `unknown`
ledger rows, `_final` builds the outcome; `run_tree` journals `task.answer` / `task.outcome`, ends the task's tabs and
processes (`Daemon._task_end`) and forgets the request (`Requests.forget`).

## 2. A Mac action in the background

`mac.snapshot` → `mac.act` (press / click / focus / set_value / type / edit / select / scroll / key) → a new snapshot.

- **Reading** (`ferb/act/mac/tools.py:MacSnapshot._run` → `ferb/act/mac/driver.py:MacComputerDriver.snapshot`):
  `_expose` sets `AXManualAccessibility` once per Electron app (`ferb/act/mac/accessibility.py:expose_tree`);
  `_walk` → `accessibility.read_tree` does one walk of the window (or the window named by title / id,
  `window_named`) plus the app's open menus, popovers, sheets and dialogs (`open_menus`, `open_panels`), keeping every
  element that exposes an action; `windows` lists the app's windows through the window server and the Accessibility
  routes that reach other desktops; `has_secure_field` finds password fields without reading them. The same walk's
  `AxTree.signature` is the before / after check for actions. With the screen scope, `MacComputerDriver.screenshot`
  (`ferb/act/mac/screenshot.py:capture`, ScreenCaptureKit, one window) adds the image with `marks` drawn on.
- **Acting** (`MacAct._run` → `MacComputerDriver.act`), background first, each rung only after the one before
  verifiably did nothing, with the outcome recorded in the app's learned profile
  (`ferb/act/mac/profile.py:Profiles.record`; `skips` after three misses in a row, `prefers`, `untried`):
  - press: `_press` = `accessibility.press` → `press_ancestor` → `focus` + Return / Space posted to the process
    (`ferb/act/mac/input.py:RealInput.to_process`, `key_steps`);
  - type: `_type` = `focus` and `has_focus` (nothing is typed while a sheet holds the focus) → unicode keystrokes to
    the process (`type_steps`), read back (`_value_changed`) → `_ax_type` (`accessibility.post_text`, the
    Accessibility keyboard, where the profile says it works) → `set_value`;
  - edit: `_edit` = `set_selected_range` + `set_selected_text` → whole-value rewrite; never arrow keys;
  - key: `_key` = a ⌘ shortcut runs its menu item (`menu_item_for_shortcut`, refused while disabled); escape cancels
    the open panel (`cancel`), never blind; else keycodes to the process → the Accessibility keyboard (`post_key`);
    `_key_rules` refuses arrows in a text box and Return where a Send button belongs; `input.switches_apps` /
    `quits_or_hides` refuse ⌘Tab, ⌘Q and friends before anything runs;
  - scroll: `_scroll` = `AXScrollToVisible` / `AXIncrement` on sliders → Page Up / Down with the area focused
    (`scroll_area_of`, `scroll_position`) → the scroll bar (`scroll_bar`, `set_scroll_bar`);
  - select: `_select` = press the pop-up, `popup_menu`, press the item → `set_value`;
  - the foreground, only with `allow_foreground=true`: `_foreground` → `_real` → `_forward` (once per stretch,
    `InputGuard.engage` holds back your first key or click), `RealInput.run` steps aside for you and comes back,
    `restore_owner` puts your app back after 3 s idle (`_restore_later`) or when you step in.
- **Click by position** (`MacClick._run` → `MacComputerDriver.click_at`): the app's own hit test
  (`accessibility.hit_element`) turns the point into a control and presses it; then a click posted to the process,
  only for a window on the current desktop (`screenshot.on_screen`), kept if the signature changed; then the real
  mouse with `allow_foreground`, re-checking that nothing covers the point (`accessibility.hit`).
- **Opening**: `MacOpen._run` → `ferb/act/mac/apps.py:launch` (background), puts you back if macOS switched desktop,
  `MacComputerDriver.ensure_window` reopens a closed window from the app's menu (`accessibility.open_window`).
- **The kind** (`MacAct._kind`) is a fact about the control: focus is navigate; Return is a submit; typing into a
  search box is a search; a press is `mac_input` unless the label hits the floor words (`effects.label_kind`).
- **Learning**: `python -m ferb.act.mac.probe <bundle> --seed` (`ferb/act/mac/probe.py:Probe.run`, `main`) tries every
  mechanism on harmless targets and `Profiles.seed` writes the result into `~/.ferb/device/apps/<bundle>.json`
  (`driver.profiles_dir`).

## 3. A browser action through the extension

`browser.open` / `ferb_browser.open` → `*_snapshot` → `*_act` / `*_scroll` → a new snapshot.

- **The tools** (`ferb/act/browser/tools.py`): `BrowserOpen`, `BrowserSnapshot`, `BrowserAct` (click, dblclick,
  rightclick, hover, type, select, press, scroll, upload; `page_mode`, `expect_dialog`, `files`, `contact`),
  `BrowserScroll` (the page or a `container`), `BrowserFind`, `BrowserWait`, `BrowserClose`, `BrowserTabs` /
  `BrowserUseTab` (owner profiles), `BrowserFillSecret` / `BrowserFillOtp` / `MailboxSend` (FERB's identity).
  `act_kind` gives the kind as a fact about the element (a link navigates, a form's button submits, a floor-word
  label is that action, a search box is a search); `security_page` raises anything non-reversible on an account /
  security page to `account_security`. `_Base._run` checks tab ownership (`owned_by`) and the signed-in account
  (`_account_ok`, `ferb/act/browser/accounts.py`).
- **Choosing the profile**: `ferb/act/browser/ext_bridge.py:Router.open_for` → `ferb/act/browser/resolver.py:Resolver.choose`
  (a named profile wins; else an open tab, your trail, your default browser, ask) and refuses any profile other
  than the one you named (`profiles.named_in`). `_TabDriver._connected` wakes a sleeping profile
  (`profiles.open_known_profile` with no window, then a wake page) and `Daemon._put_owner_back` returns you if a
  browser took the front. `Router.attach` takes over one of your open tabs (`BrowserUseTab`).
- **The bridge**: `_TabDriver._open_in` → `Bridge.call("open")`; `Bridge.call` sends a command with the connection's
  generation and a command id and maps the result: `stale` (reconnect; take a new snapshot), `duplicate` (never
  resent), a timeout → `OutcomeUnknown` (reconciled from the ledger, never retried); `_keepalive_loop` pings while a
  task tab is open. `Hub.serve` / `Hub._handle` are the daemon side of each extension connection: pairing
  (`ferb/act/browser/pairing.py`: HMAC, sequence numbers, one-time tokens), `hello` (refuses extensions below
  `ferb/act/browser/ext_protocol.py:MIN_EXTENSION_VERSION`), results, `detached` events and tab events (→
  `Daemon._on_tab`, flow 7).
- **The native host**: Chrome starts `~/.ferb/bin/ferb-native-host` → `ferb/native_host/main.py:relay`, which only
  copies messages between stdio and the daemon socket (role `native_host`, origin checked by
  `auth.check_native_host`).
- **The extension** (`extensions/chromium/`): `background.js:onDaemon` handles `welcome`, `paired` and commands;
  `onCommand` refuses a stale generation or a seen id, records the id *before* running, then runs `commands[op]`.
  `commands.open` creates the tab inactive in FERB's window of that profile (`ferbWindow`: one per profile, born
  minimized, re-minimized if you restore it; `ferbGroup`). `commands.act` → `attach` (on first use, kept for the
  tab's life: `Page.enable`, focus emulation, auto-attach, a fixed viewport for FERB's own tabs, `ready`) →
  `trusted` (`pointerClick`, `pressKey`, `Input.insertText` after an `isActive` check) — or, with `page_mode`, `inFrame` → `content.js:act`. `commands.scroll` asks
  `content.js:scroller` where the container is and sends a real wheel there (`settled` reads where it landed);
  `commands.snapshot` runs `content.js:snapshot` in every frame (`describe`: `in_view`, `occluded`, `container`,
  states; `pageText`), `ax.js:accessibilitySnapshot` makes AX the primary semantic view and binds backend nodes to
  those DOM references. `composeFrames` uses CDP frame-owner identities, and `topPoint` refreshes offsets before input; `commands.upload` → `content.js:stamp` +
  `DOM.setFileInputFiles`; `commands.screenshot` / `screenshot_part` fetch a JPEG in parts. The
  `chrome.debugger.onEvent` listener answers `Page.javascriptDialogOpening` (dismissed unless the act declared
  `expect_dialog`; `beforeunload` accepted only while leaving) and `onDetach` reports the owner's Cancel; `cdp` allows
  only `protocol.js:CDP_ALLOW`. `content.js:fill` fills FERB's sealed secrets (`ferb/identity/seal.py`) after an
  origin check, and `account` runs the per-site extractors. `_TabDriver.snapshot` turns the frames into a
  `PageSnapshot` (`ferb/act/browser/snapshot.py:element`, `for_model`).

## 4. An approval, an undo window, a worker's question

- **Ask**: `PolicyEngine.evaluate` returns `ask` with an `Approval` (`Approvals.request`) and publishes
  `approval.requested`; `Gateway._await_owner` → `_wait_answer` polls until `ferb chat` (`Chat.answer_approval` →
  `chat.approve` → `Daemon._chat_approve` → `_resolve_approval` → `Approvals.resolve`) or push-to-talk
  (`ferb/voice/controller.py:Voice.transcript`) answers, or the TTL passes (10 min; 1 h for an unattended run). A yes
  marks the destination as approved for data flow (`Requests.approve`) and policy runs again: `Approvals.consume`
  finds the grant, which covers the same effect and target for the rest of the request (an irreversible one is
  used once). A worker told `needs_owner_approval` retries the same call; a declined one is told plainly
  (`_declined`).
- **Hold**: for sends, posts, replies, calls and mailbox sends the verdict is `hold` (`Holds.hold`, 10 s); chat shows
  the countdown (`render`, topic `action.held`); `Daemon._release_loop` → `PolicyEngine.release_due`; `ferb undo`
  / Ctrl-C → `Daemon._undo` → `Holds.undo`; the gateway's `_wait_hold` then runs or reports the cancel.
- **Which contact**: `Gateway._contact` → `contacts.check` against what was on screen (`Seen.shots`); several
  matches → `Approvals.request_choice` (a numbered question), the answer picks one for this action only.
- **`owner.ask`** (`ferb/act/ask.py:OwnerAsk`): `Gateway._ask_owner` builds a `question` approval
  (`Approvals.question`, kind `owner_question`, options or free text), publishes it, waits; chat answers through
  `chat.answer` (`Daemon._chat_answer`); the worker gets `{answer}` or `{declined}`; a scheduled run
  (`Approvals.unattended`) is declined at once.

## 5. Verification

`Supervisor.verify` runs after a `done` worker with success conditions: `ferb/orchestrator/verify.py:Checks.code_checkable`
/ `Checks.run` decide by evidence where code can (`file_exists`, `file_contains`, `tab_url`, `account`); a check code can't decide goes to `_verifier` (a
read-only run with `prompts/verifier.md` and only read scopes, `dag.READ_SCOPES`), instead of immediately failing the task.
`_reconcile` re-reads the target for `unknown` ledger rows (`Ledger.reconcile`; `needs_user` if still unclear).
`_final` combines the worker's summary with what verification found: unmet → failed; undecided → done, said plainly.
`ledger_done` requires a fresh verifier read; tool completion alone cannot verify the requested goal.
A done worker without success conditions has `verified=false`.

## 6. A scheduled run

`Supervisor.send` → `Daemon._schedule_create` (`ferb/orchestrator/schedules.py:Schedules.add`, `Rule.build`,
`ferb/clock/when.py:resolve` computes the first time in code) → a `schedule` timer (`ferb/clock/timers.py:Timers.add`).
`Timers.run` → `tick` → `claim_due` → `Daemon._timer_fired` → `_run_schedule` (next run set up first; a run later
than `Rule.late_limit_s` is skipped and reported; three failures in a row pause it) → `Supervisor.run_scheduled`:
decided afresh from the owner's words with a note that nobody is here, held to the targets recorded when it was
scheduled (`targets.within`), the request marked unattended (questions declined, approvals wait 1 h), then
`run_tree`; the outcome reaches you by notification and the next chat (`Daemon._task_result`). Reminders and timers
take the same path to `_timer_fired`, delivered exactly once (`timers.mark_handled`) even across a crash.

## 7. The trail

`ferb/sense/watchers/*` (`app_focus.py:AppFocusWatcher.run`, `windows.py`, `browser_tabs.py`, `files.py`,
`idle.py`, `app_installs.py`; `owner_input.py:OwnerInput` is the listen-only tap that tells your input from FERB's
marked input, `InputGuard` the filtering tap used only while FERB has an app in front) emit `Observation`s to
`ferb/sense/manager.py:Sense.emit` → `ferb/sense/trail.py:Trail.record`, in a fixed order: paused → exclusions
(`ferb/sense/exclusions.py`, dropped before anything is written) → redact → actor (`Attribution.decide`: FERB's own
app switches, files, tabs and shells are never yours) → an append-only row → bus `trail.event` → Jev tag. Browser
tab events from paired extensions arrive through `Daemon._on_tab`. `Sense._guard` journals a watcher failure for
`ferb doctor`; `pause` / `resume` persist a marker. `Trail.purge` refuses days without a summary.

## 8. Memory

- **Daily**: `Daemon._distill_loop` (hourly) → `ferb/memory/distill.py:Distiller.catch_up` → `run_day`: the
  permanent summary by code first (`write_summary` → `ferb/memory/summary.py:build`), then one model run (`_model`,
  `ferb/memory/distill.md`, no tools) proposing items that `check` verifies against the evidence handles, stored by
  `ferb/memory/items.py:Memory.propose` (`_check` enforces origins: owner-stated active now, observed after 3 separate
  days, inferred waits for `ferb memory confirm`, third-party never about you), then the purge (`Daemon._purge_trail`).
- **In chat**: `Daemon._remember` stores what you state after checking the model's quote against your own typed words;
  `Supervisor.recall_items` → `ferb/memory/recall.py:context_block` (`Memory.recall`, FTS5) puts relevant items in
  the prompt; `ferb/memory/export.py:export` rewrites `~/.ferb/memory/*.md` after every change; `Memory.forget`
  purges an item, its versions, sources and index and rebuilds affected summaries (`summary.rebuild`).

## 9. Push-to-talk

`ferb/voice/push_to_talk.py:TalkKey.key` turns Right Option events from the daemon's tap into hold / release (a hold
under 0.3 s or with another key is dropped). `ferb/voice/controller.py:Voice.on_key` picks a microphone
(`ferb/voice/mic.py:choose`, never Bluetooth) and tells FERB.app to listen; `Voice.transcript` answers a waiting
question, cancels a countdown, or runs the words as a chat turn (`_turn`) and speaks replies
(`ferb/voice/speak.py`). `Daemon._voice_attach` / `_voice_transcript` / `_voice_switch` are FERB.app's methods;
`auth.check_voice_peer` accepts only the installed, signed app. `macapp/FERB/main.swift:Listener.start` opens the
mic through Core Audio and feeds on-device speech recognition; `serve` connects as the `voice` role.

## 10. Owner-verified testing and measurement

- `ferb watch` (`ferb/device/watch.py:run`): `MacTest` steps M1–M13 on TextEdit and Calculator and `BrowserTest`
  steps B1–B12 in a throwaway Chrome driven over its debugging pipe (`watch_ext/`, `watch_page.html`), each run in
  the background by `Session.do`, which asks what you saw and records it next to `Detector` (macOS notifications and
  a frontmost poll). Results: `~/.ferb/watch/<timestamp>.json`.
- `ferb stats` (`ferb/cli/admin.py:stats`) reads outcomes, tool-error reasons and model runs from `~/.ferb/ferb.db`.
- Real tests: `tests/test_mac_background_real.py` and `tests/test_browser_lab_real.py` (`tests/chromelab.py`) assert
  the frontmost app never changes while the drivers work.

## Breakpoints (`FERB_HOME=/tmp/ferb-dev uv run ferb daemon run`, `FERB_HOME=/tmp/ferb-dev uv run ferb chat` in another terminal)

`Daemon._chat_send` (every turn), `Supervisor._supervise` (the exact prompt and parsed outcome), `dag.validate`,
`WorkerRuns._run_worker_once` (scopes, tool list, context), `Gateway._call` right after `effects.raise_to` (every
call's kind, floor and effect), `PolicyEngine.evaluate`, `Gateway._execute` before `cap.run`,
`MacComputerDriver.act` (which rung took it: the detail says so), `MacComputerDriver._forward` / `restore_owner`
(every switch FERB makes), `Bridge.call` (every browser command and how it came back), `Hub._handle`,
`Trail.record`, `Memory.propose`. A pause in `pdb` stops the whole loop; runtimes keep running as subprocesses, so a
long pause trips the 180 s stall watchdog. `ferb logs --tools --policy` shows the same decisions without stopping. For
the extension, inspect the service worker from the browser's extensions page and break in `onCommand`.

## 0.19.0 recovery paths

- `BrowserScreenshot` → `_TabDriver.visual_snapshot`: capture the task tab after checking the field inventory.
  `BrowserPoint` → `_TabDriver.point` → `commands.point` → `content.js:point`: resolve a visible target into an id;
  `BrowserAct` applies the usual policy and trusted input.
- `BrowserLoginCode` → `Router.owner_code` → `Gateway.ask_login_code`: exact HTTPS origin shown in chat;
  `Chat.answer_approval` uses hidden input; `chat.answer` registers redaction; the driver sends a sealed fill.
- `Supervisor.send` consumes structured `control` (stop/retry/continue), validates the task belongs to this chat,
  cancels and waits, restores original context under fresh request authority for follow-ups, and blocks retries of unknown outcomes.
- `observation_digest` removes snapshot generation bookkeeping; `TreeGuard` detects unchanged action loops and
  stops repeated consequential actions after one retry. `Profiles` re-probes failed input mechanisms after an hour.

## 0.20.0 pointer and continuity paths

- `Chat.opening` → `Daemon._chat_attach` → `Continuity.attach`: durable default conversation; explicit new/resume.
- `Supervisor.run_tree` → `Continuity.save`: redacted specification and owner words persisted before work starts.
  `Supervisor._supervise` supplies scoped history and real candidate IDs; `send` restores context, checks unknown
  ledger entries, and creates fresh request authority. `Continuity._import_legacy` recovers older recorded chat tasks.
- `BrowserPoint` → `_TabDriver.point`: opaque visual token plus policy-visible element ID. `_TabDriver._pointer` →
  `Bridge.call` → `pointer.js:exactPointer`: fresh coordinates, trusted frame readiness, balanced gestures.
  `Bridge._cancel_pointer` → `commands.pointer_cancel`: cancellation propagates from Python into extension cleanup.
- `MacPointer` → `MacComputerDriver.pointer_at` → `pointer_steps` → `RealInput.to_process`: exact pixel mapping,
  app/window guards, process-directed input and release cleanup inside the GUI lock.
