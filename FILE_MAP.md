# File map

What the source folders and files in FERB are for, as of version 0.18.0 (2026-10-02). Package markers and tests
are grouped where they share a purpose. For how the pieces run together,
see [CODE_TOUR.md](CODE_TOUR.md). For the design, see [ARCHITECTURE.md](ARCHITECTURE.md).

Each line is drawn from the file's own docstring and checked against the code. `vibed/` (the abandoned old
approach) and generated folders (`node_modules/`, `__pycache__/`, `macapp/FERB/build/`) are left out.

## Repository root

| File | What it is |
|---|---|
| `ARCHITECTURE.md` | The design contract: the source of truth. Code follows it. |
| `CLAUDE.md` | Cold-start onboarding: the owner's goal, docs map, development rules, verification and next steps. |
| `PROGRESS.md` | Every unit built so far, with dates, results and what's next. |
| `CHANGELOG.md` | Version history (`MAJOR.MINOR.PATCH`). |
| `README.md` | What FERB is, how to install and use it, and the CLI commands. |
| `CAPABILITIES.md` | What FERB can and can't do today, in plain terms. |
| `CODE_TOUR.md` | The code in the order it runs: the main flows, by path and function. |
| `FILE_MAP.md` | This file. |
| `pyproject.toml`, `uv.lock` | Python package definition (`ferb`, version) and locked dependencies. |
| `package.json`, `package-lock.json` | Pins the Codex CLI (0.157.1) repo-locally; `ferb setup` copies it to `~/.ferb/node`. |

## `docs/`: the rework, platform evidence and owner acceptance

| File | What it is |
|---|---|
| `REWORK_PLAN.md` | Rework phases and the rule on hardcoding; procedure memory is still pending. |
| `DESIGN_ONE_WORKER.md` | The design behind one worker per request, effects and `owner.ask`; implementation details live in ARCHITECTURE.md §§4–5. |
| `PLATFORM_MACOS.md` | Mac background-control mechanisms, probe evidence, owner observations and driver rung order. |
| `PLATFORM_BROWSER.md` | Chromium background-control mechanisms, probe evidence and extension design. |
| `OWNER_ACCEPTANCE.md` | Tasks the owner runs through chat and checks with their own eyes; B–H remain pending. |

## `ferb/`: the Python package (daemon, CLI, everything that runs)

### Top-level modules

| File | What it is |
|---|---|
| `__init__.py` | Package marker and `__version__`. |
| `paths.py` | Where FERB lives at runtime: `~/.ferb` (or `$FERB_HOME`) and the socket path. One place, so the daemon, the CLI, the `ferb mcp` shim and the native host always agree. |
| `bus.py` | In-process event bus. Parts of the daemon publish events (`task.started`, `approval.requested`, `voice.heard`, …). Subscribers (`ferb chat`, `ferb logs`, `ferb voice`) receive them through bounded queues. Critical events are never dropped; telemetry may be. |
| `config.py` | Reads and writes `~/.ferb/config.toml` with comments kept. Refuses a config others can read. Writes are atomic. |
| `registry.py` | The one explicit list of the tools FERB loads (files, time, browser, Mac, shell, trail). No auto-discovery. |

### `core/`: shared types, no I/O

| File | What it is |
|---|---|
| `types.py` | Pure data: `Identity`, `Task`, `Outcome`, `ActionSpec`, `Target`, `ToolContext`/`ToolResult`, page and computer actions. |
| `interfaces.py` | The seams, as Protocols: `Clock`, `AgentRuntime`, `Capability`, `BrowserDriver`, `ComputerDriver`. |
| `errors.py` | Errors shared across FERB (`FerbError`, `NotRunning`, `Refused`, `StaleRef`, `DriverError`, …). |
| `ids.py` | Id generators. |

### `daemon/`: the long-running process (launchd starts it at login)

| File | What it is |
|---|---|
| `__main__.py` | `python -m ferb.daemon`: what launchd runs. |
| `main.py` | The daemon itself. Startup and shutdown; every RPC method (`chat.send`, `chat.answer`, `trace.get`, `voice.switch`, `memory.*`, …); wiring of the supervisor, gateway, timers, watchers, voice, browsers and Jev. |
| `server.py` | Owner-only Unix socket, newline-delimited JSON-RPC. The first message must be `auth.hello` with a role. |
| `auth.py` | Connection roles (cli, mcp_shim, native_host, voice), the methods each role may call, and run tokens for workers. |
| `gateway.py` | The tool gateway. Every model action reaches the Mac only through here: token → scopes → the effect gate (declared effect raised to the control's floor) → contact and duplicate-send checks → policy engine → approvals / undo window → locks → the tool → redaction → trace span. Also answers `owner.ask`. |

### `orchestrator/`: turning a chat message into work

| File | What it is |
|---|---|
| `supervisor.py` | One chat turn: reply, clarify or task. Runs one worker per request with every tool its identity allows, then the checks; the worker's summary is FERB's answer. Also reminders and schedules from chat. |
| `worker.py` | Runs one worker: issues its run token, starts the runtime, watches heartbeats and runaway guards, parses its typed result. Mixed into the supervisor. |
| `dag.py` | The identity floor before anything runs: a `ferb` task never gets owner scopes or folders; the worker's scopes are its identity's, minus what isn't usable now. |
| `schemas.py` | JSON schemas and pydantic models for every model output (supervisor outcome, task with targets and conditions, worker result, verifier, reconcile). |
| `availability.py` | Builds the planner's "what workers can do right now" list in code from what's actually connected and permitted. |
| `guards.py` | Runaway guards: the same failing call 3×, loops between steps, and tree caps (4 h, 50 worker runs, 1,000 tool calls). No token or dollar budgets. |
| `ledger.py` | Action ledger: intended → executing → done / failed / unknown. Prevents a lost acknowledgement from becoming a second send. |
| `verify.py` | Code checks after the worker finishes (file, ledger, tab URL, account). The supervisor sends undecided checks to a read-only verifier and reconciles `unknown` actions. |
| `tree.py` | The running task tree, persisted, for the gateway, `ferb trace` and restarts. |
| `schedules.py` | Scheduled and recurring tasks: rules, missed-run handling, pausing after three failures. |
| `stop.py` | Emergency stop: revoke tokens, block actions, cancel held sends, run stop hooks. |
| `prompts/supervisor.md` | Instructions for the supervisor model: reply, clarify or task. |
| `prompts/worker.md` | Instructions for the one worker: what it can do, the effect classes, `owner_ask`, the report that is the owner's answer. |
| `prompts/verifier.md` | Instructions for the read-only checks (verifiers and reconcilers). |

### `runtimes/`: the model processes FERB starts

| File | What it is |
|---|---|
| `codex_exec.py` | Runs `codex exec --json` (pinned 0.157.1) for the planner and workers. Native tools are disabled and the model catalog is locked; the only tools a worker gets come through `ferb mcp`. The model is set here (`MODEL`). |
| `claude_sdk.py` | The same over the Claude Agent SDK (no built-in tools, strict MCP). Used when `config.toml [runtimes]` says `claude`. |
| `proxy.py` | A local proxy for Claude runs that records only tool names per request, to prove nothing extra was offered. |
| `tooltrace.py` | Reads the tool names Codex actually showed the model from its request trace; anything outside the allowlist stops the run. |

### `policy/`: deciding whether an action may happen

| File | What it is |
|---|---|
| `engine.py` | The fixed order of checks: stop → identity and scopes → exposure → navigate runs → data flow → the effect gate (send asks once per request / effect / target unless the owner named it; pay / destroy ask every time) → undo window. The first check that decides wins. |
| `rules.py` | Action kinds (`Kind`), the `REVERSIBLE` set, scopes, identity rules, hard floor, destructive patterns. |
| `effects.py` | The effect classes (navigate / edit / send / pay / destroy), the floor each kind is at least, and the one floor word list for control labels (Send / Pay / Delete …). |
| `targets.py` | What a request is about: the owner's own words and the targets the model understood; whether the owner named a target; a scheduled run's targets. |
| `exposure.py` | A worker that read untrusted content loses shell, AppleScript and file-write for the rest of its run. |
| `taint.py` | Labels a worker that read the owner's account data, so what it then types or sends is checked. |
| `dataflow.py` | Owner data may only go to a target of the request, something the owner named, or something they approved in an ask. |
| `approvals.py` | Approval questions to the owner (yes / no, a numbered choice, or a worker's question with free-text answers), and the undo window for sends. |
| `autonomy.py` | When FERB asks: rarely. The owner's instruction is the approval. |
| `contacts.py` | Never guess between contacts: checks the contact against what was on screen, and asks "Which Rahul?" when needed. |
| `services.py` | The service catalogue (apps and websites of common services) for the environment line, naming checks and the messaging / calling apps. Decides no route. |

### `sanitize/`

| File | What it is |
|---|---|
| `redact.py` | The one redaction pipeline: runs before anything reaches a model, log, trace, journal or memory. |
| `untrusted.py` | Marks pasted and quoted text in a chat message as untrusted data, and separates the owner's own words. Respects `[policy] pasted_text_is_data`. |

### `act/`: the tools (capabilities) and the drivers under them

| File | What it is |
|---|---|
| `ask.py` | `owner.ask`: a worker's question to the owner, answered in chat (every identity, no scopes; the gateway answers it). |
| `locks.py` | Resource locks (tab, account, file, app), always taken in one fixed order. |
| `gui_lock.py` | One real-input actor at a time. |
| **`act/mac/`** | **Controlling Mac apps** |
| `mac/tools.py` | The `mac.*` tools: apps, open, snapshot, menu, act, screenshot, click, applescript. Each classifies its action kind for policy. |
| `mac/driver.py` | Carries out a Mac action in the background, one ranked rung at a time per primitive (press, type, edit, key, scroll, select, click-at-point — `docs/PLATFORM_MACOS.md` §12), recording what landed in the app's learned profile; the foreground (once per stretch, with the owner's input winning) only when the call allows it. |
| `mac/accessibility.py` | One walk of an app window (plus its sheet, the app's open menus, popovers and dialogs) that serves the snapshot, the before / after signature and the password scan: controls by available actions, roles and states as facts, windows listed and addressable; press / set / focus / select text / scroll / menus through Accessibility. |
| `mac/profile.py` | Per-app learned capability profile: which background mechanisms landed or failed, counted, in memory and under `~/.ferb/device/apps/<bundle>.json`; seedable from the probe. |
| `mac/probe.py` | Learns a running app's background capabilities on harmless targets (TextEdit, Calculator, Finder only); `--seed` writes the result into the app's profile. |
| `mac/input.py` | Real mouse and keyboard events, marked as FERB's, under the GUI lock, yielding to the owner. |
| `mac/screenshot.py` | Captures one app window into memory, with numbered marks drawn on the controls. |
| `mac/apps.py` | Running apps: list, launch, find by name or bundle id. Never quits an app. |
| `mac/applescript.py` | AppleScript for one app at a time, wrapped and checked so it can't reach other apps. |
| `mac/resolve.py` | Picks the method per app (AppleScript for scriptable apps, else Accessibility). |
| `mac/ax_browser.py` | Browser bundle ids, plus the rule that acting in a browser through Accessibility is refused (only the extension may act in browsers). The Mac driver and the window watcher import it. |
| **`act/browser/`** | **Controlling browsers** |
| `browser/tools.py` | The `browser.*` tools (the owner's Chrome/Brave profiles) and `ferb_browser.*` (FERB's own profile). Each classifies its action (navigate, search, submit, …). |
| `browser/ext_bridge.py` | Driver through the FERB extension: native-host connections, one bridge per paired profile, waking a profile without a window, tabs, snapshots, actions. |
| `browser/ext_protocol.py` | The extension message format (mirrored in `extensions/chromium/protocol.js`). |
| `browser/pairing.py` | Pairing an extension install to a browser profile: one-time tokens, per-install keys, HMAC and sequence numbers. |
| `browser/profiles.py` | Finds browser profiles from `Local State`, resolves profile names, opens a profile in the background. |
| `browser/resolver.py` | Picks the browser and profile for a site when the owner didn't name one. |
| `browser/accounts.py` | Checks which account is signed in before a consequential action. |
| `browser/snapshot.py` | The shared page-snapshot format and the in-page walker. Password values never leave the page. |
| **`act/fs/`** | |
| `fs/files.py` | The `fs.*` tools: list / read / write / edit / trash inside allowed roots. Handles the owner's Desktop, Documents and Downloads, and Finder's ":" names. |
| **`act/shell/`** | |
| `shell/tools.py` | The `shell.*` tools: run, read, send, kill, list. Owner tasks only; lost when a worker is exposed. |
| `shell/manager.py` | Owns running shell processes per task, and kills them when the task ends. |
| `shell/pty_proc.py` | One process in a pseudo-terminal, with output kept in a ring buffer and a redacted log file. |
| `shell/guard.py` | Pre-checks for commands: allowed folders, refused locations, destructive patterns that always ask. |

### `clock/`: time

| File | What it is |
|---|---|
| `clock.py` | Wall clock and monotonic clock (tests use a fake one). |
| `context.py` | The time block at the top of every model turn, and the `[now … | task +… ]` footer on every tool result. |
| `when.py` | Turns the model's structured "when" (in_minutes, day, date, time) into an absolute time. The model never computes dates. |
| `timers.py` | Durable timers in SQLite: survive restarts and sleep, delivered exactly once. |
| `durations.py` | How long similar tasks took before, shown to workers. |
| `tools.py` | `time.remind_at`, `time.set_timer`, `time.sleep_until` for workers. |

### `sense/`: watching the owner's activity (the trail)

| File | What it is |
|---|---|
| `trail.py` | One path for every observation: pause → exclusions → redaction → actor → saved → published. Metadata only. |
| `manager.py` | Starts and stops the watchers; `ferb pause` / `resume`. |
| `exclusions.py` | Apps, domains and paths the owner excludes: dropped before anything is written. |
| `tools.py` | The `trail.query` tool: workers read the owner's recent activity. |
| `watchers/base.py` | Shared watcher plumbing. |
| `watchers/app_focus.py` | Which app is in front. |
| `watchers/windows.py` | Focused window titles (not browsers). |
| `watchers/browser_tabs.py` | Page loads and tab switches in the owner's browsers. |
| `watchers/idle.py` | Owner idle / active transitions. |
| `watchers/files.py` | File changes in `[sense] watch_folders` only (empty by default). |
| `watchers/app_installs.py` | Apps installed or removed. |
| `watchers/owner_input.py` | Listen-only event tap: is the owner using the mouse or keyboard? It feeds idle, the input guard and the push-to-talk key. |

### `memory/`

| File | What it is |
|---|---|
| `items.py` | Memory items with origins (owner_stated, observed, inferred, third_party) and the rules for each. |
| `recall.py` | Picks the few active items relevant to a message (keyword search). |
| `journal.py` | Append-only journal of what happened (`chat.message`, `task.outcome`, `timer.refused`, …). |
| `summary.py` | The permanent daily summary, written by code from the trail and journal. |
| `distill.py` | Hourly catch-up: for each past day, the summary, then one model run proposing memory items, then the retention purge. |
| `distill.md` | Instructions for that model run. |
| `export.py` | Markdown export of active memory, one file per subject. |

### `decide/`: Jev

| File | What it is |
|---|---|
| `decider.py` | Asks Jev (an external decision API) next to FERB's own rules. In shadow mode it only logs; when enabled per question, only a stricter answer has an effect. |
| `questions.py` | The typed questions FERB asks Jev, with the baseline answer and the allowed effects. |

### `identity/`: FERB's own accounts

| File | What it is |
|---|---|
| `secrets.py` | FERB's secrets in the Keychain (service "ferb"). Never shown to a model. |
| `seal.py` | Encrypts a secret for the extension (AES-GCM, bound to the command and origin). |
| `mailbox.py` | Reads one-time codes from FERB's Gmail inbox. |
| `seed.py` | Finds and verifies FERB's own Chrome profile. |

### `device/`: this Mac

| File | What it is |
|---|---|
| `install.py` | Dedicated Python, the venv copy of FERB, the LaunchAgent; `ferb setup --update`; uninstall. |
| `permissions.py` | Checks macOS permissions (Accessibility, Screen Recording, Input Monitoring, Full Disk Access) without prompting. |
| `scan.py` | Installed apps from their Info.plist, scriptability, browsers present. |
| `profile.py` | The device profile JSON, with its local git history. |
| `selftest.py` | Proves the daemon can read Accessibility and post and see a marked event. |
| `watch.py` | `ferb watch`: 13 Mac and 12 browser interaction probes, owner answers and background detectors, JSON results. |
| `watch_page.html` | Local page with controls for the browser probes. |
| `watch_ext/manifest.json`, `watch_ext/sw.js` | Minimal probe extension loaded only in watch's throwaway Chrome. |

### `voice/`: push-to-talk (daemon side)

| File | What it is |
|---|---|
| `controller.py` | Turns key presses into commands for FERB.app, runs what was heard as a chat turn, speaks the reply, handles spoken yes / no / cancel. |
| `push_to_talk.py` | The Right Option talk key: hold to talk, a tap is ignored, another key makes it a shortcut. |
| `mic.py` | Which microphone to use: never a Bluetooth headset's. |
| `speak.py` | FERB's voice through the Mac's `say`. |

### `cli/`: the `ferb` command

| File | What it is |
|---|---|
| `__init__.py` | Argument parsing and dispatch for every `ferb` subcommand. |
| `chat.py` | `ferb chat`: the line editor, paste handling, loader, inline approvals. |
| `client.py` | The socket client used by chat, MCP and daemon-backed admin commands. |
| `mcp.py` | `ferb mcp`: the stdio MCP shim Codex starts for each worker. It forwards tool calls to the gateway with the run token, and holds no logic. |
| `logs.py` | `ferb logs` and `ferb voice` (live streams from the bus), `ferb voice on|off`. |
| `admin.py` | `ferb trace`, `stats` (direct database read), `memory`, `decide`, `schedules`, `browsers`, `device` and other admin commands. |
| `setup.py` | `ferb setup [--update / --voice / --jev]`, `ferb doctor`, `ferb uninstall`. |

### `native_host/`

| File | What it is |
|---|---|
| `main.py` | The native-messaging host Chrome/Brave start for the extension. It only relays messages to the daemon socket. |

### `store/`

| File | What it is |
|---|---|
| `db.py` | The single SQLite connection (`~/.ferb/ferb.db`, WAL) and migrations. |
| `spans.py` | Writes trace spans (redacted) and reads the span tree for `ferb trace`. |
| `migrations/0001…0010_*.sql` | Table definitions: spans and journal, action ledger, browsers, tasks, trail, memory, Jev decisions, timers, task results, schedules. |

## `extensions/chromium/`: the FERB browser extension (Chrome and Brave)

| File | What it is |
|---|---|
| `manifest.json` | MV3 manifest (version 0.7.0, protocol 3). |
| `background.js` | Service worker: native messaging, pairing keys, FERB's minimized window per profile, task tabs, the debugger session per task tab (trusted input, wheel scrolls, dialogs, uploads, screenshots), tab events. |
| `content.js` | Viewport-aware page snapshot (in_view, scrollers, shadow roots, iframe rects), element lookup, page-mode actions, and the per-site account extractors. |
| `protocol.js` | Message format (mirrors `ferb/act/browser/ext_protocol.py`), the CDP allowlist, the key table. |
| `pair.html`, `pair.js` | The blank pairing page: hands the one-time token to the service worker and closes. |

## `macapp/FERB/`: FERB.app (push-to-talk microphone and speech)

| File | What it is |
|---|---|
| `main.swift` | Background app (no windows, no Dock icon): microphone via Core Audio, on-device speech recognition, chimes; talks to the daemon over the socket. |
| `Info.plist` | Bundle id `com.ferb.app`, `LSUIElement` (no Dock icon), microphone and speech usage strings. |
| `build.sh` | Builds and signs FERB.app with the local certificate (`ferb setup --voice` runs it). |

## `spikes/`: throwaway experiments that settled design questions (never imported by `ferb/`)

`01_codex_mcp`, `02_claude_sdk`, `03_cdp_identity`, `04_extension`, `06_pyobjc`, `08_launchd`, `10_profiles`,
`12_pairing`, `13_mv3_restart`, `14_owner_input`, `15_tcc_launcher`, `16_ext_identity`, `17_voice_audio`,
`18_background_input`, `19_chrome_wake`, `20_whatsapp_search`, `21_gpt6_lockdown`. Findings are recorded alongside
the experiments and in the historical table in PROGRESS.md. Legacy §17 references refer to the pre-rework
ARCHITECTURE.md (`git show b993da5:ARCHITECTURE.md`).

## `tests/`

| File | What it is |
|---|---|
| `conftest.py` | Fixtures: a real daemon (`RunningDaemon`) in a temporary home folder, with fake runtimes and clocks. |
| `fake_extension.py` | A fake extension speaking the real protocol through the real native host (models FERB's window, debugger attach / Cancel, dialogs, pings). |
| `chromelab.py` | A throwaway lab Chrome (own data dir, pipe-driven only to load the unpacked extension), its native-host wiring, and a frontmost-app watcher for real browser tests. |
| `browserkit.py` | Shared browser-test kit (temporary keychain, fake inbox feed). |
| `probe_window.py` | A throwaway window for the real screenshot-and-click tests. |
| `test_acceptance_one_worker.py` | Scripted replays of the eight failures that motivated the one-worker rework, including free-text owner answers. |
| `test_mac_snapshot.py`, `test_background_input.py` | Snapshot facts, learned mechanisms and background/foreground contracts against fake Mac APIs. |
| `test_browser_background.py` | Background browser and extension contracts, including persistent debugger sessions. |
| `test_rework_fixes.py` | Regression checks for the audit fixes, stats and the watch harness. |
| `test_mac_background_real.py`, `test_browser_lab_real.py` | Rebuilt drivers against safe apps / throwaway Chrome, with frontmost-app polling. |
| Other `test_*.py` | Tests by area (policy, gateway, supervisor, time, memory, …); markers decide which run offline. |
| `test_*_real.py` | Real tests against real apps and browsers. Locked down by default; Codex only unless `-m real_claude`. |
