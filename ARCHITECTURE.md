# FERB — Architecture Contract (Block 1)

This file is the contract for anyone (human or coding agent) writing FERB code.
Do not change module boundaries, interfaces, identity rules, or dependency rules without the owner's approval.

Revision 2 changes: browser layer redesigned (FERB extension for Chrome/Brave/Safari, CDP only for FERB's own
profile), owner vs FERB identity boundary, memory source of truth moved to SQLite with markdown as an exported view.
Revision 3 changes: system-audio listening reserved (slot + spike only; built in the voice block, section 20).
Revision 4 changes: browser profiles are first-class (discovery from `Local State`, deterministic profile resolver,
extension install + pairing per profile), extension page/debugger modes and CDP method allowlist (sections 9.3–9.6).
Revision 5 changes: device-neutral naming (`ComputerDriver`, Mac as first implementation), `device_id` on tasks,
spans, trail events, device profiles and browser profiles; future "brain + nodes" direction recorded (section 19).
Revision 6 changes: safety and reliability pass. Autonomy model where the owner's instruction is the approval and
security runs in the background (section 10); exposure tracking + reader/actor split against prompt injection;
runtime native tools disabled so the gateway cannot be bypassed; origin-bound secrets; sanitization pipeline;
action ledger with "outcome unknown"; emergency stop and owner-input priority; resource locks; trail actor/id;
memory trust levels and real forgetting; runtime feature flags; monotonic clock; direct-answer path; journal from
step 1; contradictions from revision 5 fixed (owner CDP, trail redaction order, profile resolution); safety tests (section 21).
Revision 7 changes: Safari watch-only in Block 1 (AppleScript read of tabs and URLs); acting in Safari, the Safari
extension and its FERB.app handler deferred to a later block; FERB.app reserved for later (audio, stop hotkey,
Keychain locking). Undo window shown in the CLI only (no system notifications in Block 1).

## 0. Block 1 scope

| # | Feature | In this block |
|---|---------|---------------|
| 1 | Mac control | Yes — AppleScript/CLI → Accessibility → screenshot+click, chosen per app from the device profile |
| 2 | Browser control | Yes — FERB's own Chrome profile via the FERB extension (paired as identity FERB); owner's Chrome and Brave via the same extension, Accessibility read fallback, screenshot + click as the last fallback; Safari watch-only (AppleScript read of tabs and URLs, when the owner has granted Automation) (section 9) |
| 3 | FERB identity | Yes — own email + password (Keychain) and one-time codes from its inbox, filled sealed into origin-checked fields in FERB's own profile, never visible to models; sending email from its own mailbox under the normal autonomy rules |
| 4 | CLI | Yes — `ferb chat` (talk to orchestrator) and `ferb logs` (live log/trail viewer), plus admin commands |
| 5 | Monitoring | Yes — event trail of Mac + browser activity; no continuous LLM watching; screenshots only on demand |
| 6 | Time | Yes — time header per model turn (with measured task durations), timestamp footer on every tool result, durable timers and reminders (delivered exactly once, as macOS notifications and in the CLI) |
| 7 | Memory journey | Yes — everything from day one in SQLite; distilled memory items with provenance and trust levels; markdown exported as a read-only view |
| – | Jev | Yes — behind `Decider`, shadow mode first |
| – | Safari control (extension, acting in Safari) | **No** — later block; Safari is watch-only in Block 1 |
| – | System audio listening ("what is this song / what is it saying") | **Reserved** — slot + spike only; built with the voice block (section 20) |
| – | Voice, phone, Linux/Windows, self-improving overrides, Dream loop, Universal Memory | **No** (later blocks; interfaces must not preclude them) |

Decisions and defaults:
- Python 3.12. Runtimes: Claude (Agent SDK) = orchestrator, Codex (`codex exec --json`) = workers; switchable in config.
- FERB never reads, types, or stores the owner's credentials. FERB never reads any browser cookie stores. FERB's
  own identity is a dedicated Chrome profile holding only FERB's account, verified at setup and never copied.
- Low friction by default: the owner's instruction is the approval; security runs automatically in the
  background; explicit confirmation only for the narrow cases in section 10.3. Autonomy level: `balanced`.
- Codex authenticated via ChatGPT login for testing.

## 1. Rules for writing code

1. **Important behavior lives in code, not prompts.** Orchestration, policy, fallback, runaway guards, secrets, identity, time — all code.
2. **One uniform shape for pluggable things.** Tools are `Capability`, watchers are `Watcher`, model backends are `AgentRuntime`, browsers are `BrowserDriver`.
3. **Explicit registration.** Everything loaded is listed in `ferb/registry.py`. No auto-discovery, no import-time side effects.
4. **Interfaces only at real boundaries**: runtimes, decider, memory, clock, secrets, drivers. Elsewhere: plain functions + dataclasses.
5. **Composition over inheritance.** No class hierarchy deeper than Protocol → implementation.
6. **Everything is traced.** Every task, worker, model call and tool call is a `Span` with IDs, times, inputs, outputs, cost.
7. **Injected time, IDs and model calls** so any task can be replayed with `FakeClock` and recorded responses.
8. **No secret ever enters a prompt, trace, log, or memory.** Only `SecretRef` handles.
9. **Typed everything** (`pydantic` at process/tool/extension boundaries, dataclasses inside). `mypy --strict` on `ferb/core`.
10. **Small files, one concept each.** If a file needs a table of contents, split it.
11. **Extensions are hands, not brains.** Browser extensions execute commands and report state; every decision, policy check and trace happens in the daemon.
12. **No path around the gateway.** Every action by a model goes through `ferb mcp` → gateway. Runtime-native tools
    (shell, file edit, web fetch, subagents, plugins) are disabled and tested as disabled (section 21).
13. **Sanitize before anything leaves the daemon.** Every snapshot, tool result, error and trace passes the
    redaction pipeline (`sanitize/`) before it reaches a model, a log, the journal, or an export.
14. **Failures are values, not exceptions,** across worker boundaries: a worker returns a typed failed/unknown
    outcome; it never raises into the supervisor's task group.

### Dependency rules

```
cli, native host, extensions ──► (socket / native messaging) ──► daemon
core      ← imports nothing from ferb except core
everything else ← may import core; never imports cli
drivers (act/*) ← never import orchestrator
runtimes  ← never import drivers (they reach tools only via MCP)
policy    ← imports core only (Decider arrives as an injected interface); called by the gateway, never bypassed
sanitize  ← imports core only; applied by the gateway and every writer (traces, journal, memory, export)
```

## 2. Process model

```
┌────────────┐  ┌────────────┐        Unix socket (JSON-RPC)
│ ferb chat  │  │ ferb logs  │ ──────────────────────────────┐
└────────────┘  └────────────┘                               ▼
                                     ┌─────────────────────────────────────┐
┌──────────────────────┐  stdio MCP  │ FERB daemon (launchd user agent)     │
│ Claude (Agent SDK)   │◄──────────► │  orchestrator · policy · bus · clock │
│ Codex (codex exec)   │  ferb mcp   │  watchers · timers · memory · traces │
└──────────────────────┘  shim + run │  device profile · tool gateway       │
                          token      │  GUI lock · browser resolver         │
                                     └──────────────────┬──────────────────┘
                                                        │ socket
                                                        ▼
                                              ┌───────────────────┐
                                              │ ferb-native-host  │
                                              │ (per browser conn)│
                                              └─────────▲─────────┘
                                                        │ native messaging
                                              ┌─────────┴──────────────────────┐
                                              │ FERB extension in the owner's  │
                                              │ Chrome + Brave profiles (OWNER)│
                                              │ and in Chrome's "FERB" profile │
                                              │ (identity FERB, FERB's creds)  │
                                              └────────────────────────────────┘
```

- **Daemon** is the only long-lived process.
- **`ferb mcp`** is a thin stdio MCP shim spawned by runtimes. It forwards tool calls to the daemon with a
  **run token** (`FERB_RUN_TOKEN`) that identifies task/worker, so the daemon enforces that worker's scopes and identity.
- **`ferb-native-host`** is the native-messaging host the Chromium browsers launch for the extension; it only
  relays JSON messages to the daemon socket.
- **FERB.app** (`macapp/FERB`, built by `ferb setup --voice`): a background Swift app with no window, Dock icon or
  menu-bar item, signed with a local certificate. Today it is push-to-talk's microphone and on-device speech (20.1).
  Later: system-audio listening (20), the stop hotkey, Keychain item access anchored to its signed identity, and the
  Safari extension.
- **FERB's identity is a dedicated "FERB" profile in the owner's normal Chrome**, running the same FERB extension.
  The daemon binds that profile to identity `FERB` at pairing (9.6); the extension itself never decides identity.
  No profile copies: a copied profile loses its Google session within minutes, while the original keeps it
  (build step 5, spike 16).
- **FERB never launches a browser under CDP or remote debugging** (no `--remote-debugging-pipe`/`port`). All browser
  control goes through the extension; CDP exists only as the extension's `chrome.debugger` with its allowlist (9.4).
- **Socket authentication:** the daemon socket is owner-only (`0600`, in a `0700` directory). Every connection
  declares a role (`cli`, `mcp_shim`, `native_host`) and proves it: CLI by socket ownership,
  shims by a run token, extensions by paired `install_id` + per-install key. Each role has a fixed method list.
  Run tokens are scoped to one worker, expire with it, and are revoked by `ferb stop`. Messages are
  newline-delimited JSON-RPC 2.0 with a maximum size of 1 MiB; a larger message is answered with an error and
  the connection is closed.

## 3. Directory layout

```
ferb/
  core/                 pure types and interfaces; no I/O
    types.py            Task, Outcome, Span, TrailEvent, SecretRef, RiskLevel, Identity
    interfaces.py       all Protocols (section 5)
    errors.py
    ids.py              IdSource implementations (random; deterministic for tests)
  daemon/
    main.py             startup / shutdown lifecycle
    server.py           socket JSON-RPC for CLI, native host
    gateway.py          tool gateway: run token → scopes + identity → policy → capability → trace
    auth.py             connection roles, run tokens (issue / expire / revoke), per-role method lists
  registry.py           THE explicit list of capabilities, watchers, runtimes, deciders, drivers
  paths.py              runtime locations (~/.ferb or $FERB_HOME, socket path): shared by daemon and CLI
  config.py             config.toml load/save (tomlkit; atomic temp-file + rename; stays 0600; refuses loose perms)
  bus.py                in-process event bus (typed events)
  orchestrator/
    supervisor.py       plans, spawns workers, merges, verifies, reports
    worker.py           one task, scoped capabilities, heartbeat, runaway guards
    tree.py             task tree persistence
    availability.py     what the planner may use right now (connected profiles, FERB's browser, permissions) + "Unavailable"
    guards.py           runaway guards per task tree (repeated failing action, loops, generous time / worker-run /
                        tool-call caps) and worker heartbeats; no token or dollar budgets
    dag.py              plan validation (cycles, fan-out, depth, identity/scopes, own-words targets) + graph order
    schemas.py          typed plan / worker / reader / verifier / reconciliation outputs (strict JSON schemas)
    ledger.py           action ledger: intent → executing → done | failed | unknown; reconciliation
    verify.py           supervisor-owned verification against the plan's success conditions
    stop.py             emergency stop: revoke tokens, block actions, cancel workers, kill owned processes
    prompts/            prompt templates (text only; no logic)
  runtimes/
    claude_sdk.py       ClaudeRuntime (Agent SDK)
    codex_exec.py       CodexRuntime (codex exec --json); supervisor and worker modes
    proxy.py            localhost tool-list proxy for the Claude runtime (records tool names only)
  decide/
    decider.py          RuleDecider (FERB's existing logic), JevClient, ShadowDecider (logs both; stricter-only
                        effects for enabled questions), the enable rule; LLMDecider isn't built (no question needs it)
    questions.py        the typed questions FERB asks (section 12): answers, baselines, which answers are stricter
  policy/
    rules.py            scopes, identity rules, hard floor, destructive-pattern lists
    intent.py           does this action match what the owner asked for? (section 10.2)
    exposure.py         per-worker exposure to untrusted content; capability removal (10.2)
    dataflow.py         owner data may only go to destinations the request involves (10.2)
    autonomy.py         careful / balanced / hands-off levels + learned trust from traces (10.4)
    approvals.py        single-use, expiring approvals bound to the exact action payload; undo windows
    engine.py           fixed evaluation order over the checks above -> ALLOW / HOLD / ASK / BLOCK with reason
    services.py         how to reach a named service or app: correction > owner usage > app > paired web (9.3)
    contacts.py         never guess between contacts: the selected contact was on screen; several match -> ask (10.3)
  sanitize/
    redact.py           one redaction pipeline: secrets, password/OTP fields, tokens in URLs, exclusions
  act/
    mac/
      driver.py         MacComputerDriver (implements ComputerDriver): takes an action, resolves the method, calls the parts below
      resolve.py        per-app method choice from DeviceProfile (AppleScript/CLI → AX → screenshot)
      apps.py           launch, activate, hide, quit, list running apps
      applescript.py    run AppleScript; read an app's scripting dictionary
      accessibility.py  read AX tree, find elements, press / set value
      input.py          real mouse + keyboard events (takes gui_lock)
      screenshot.py     on-demand capture of one app window (never the screen), in memory only
      snapshot.py       AX tree → shared element-list format (same as browser snapshots)
      tools.py          mac.* capabilities (apps, open, snapshot, act, applescript, screenshot, click); OWNER only
    browser/
      resolver.py       site → (browser, profile, identity, driver) selection (section 9.3)
      profiles.py       profile discovery from Local State + name → profile resolver (section 9.5)
      pairing.py        extension install ↔ browser profile pairing (section 9.6)
      tools.py          browser.* (owner) and ferb_browser.* (FERB) capabilities; policy kind from what the element does
      accounts.py       per-site account extractors (Gmail, YouTube first) for the pre-action account check
      ext_bridge.py     driver via the FERB extension: owner profiles and the FERB identity profile (identity
                        comes from the pairing binding, never from the extension)
      ax_browser.py     Accessibility driver (fallback for any browser)
      snapshot.py       unified page snapshot format (DOM + AX → elements with ids, roles, labels, rects)
      ext_protocol.py   extension message schema (pydantic); mirrored by extensions/chromium/protocol
    fs/
      files.py          read / write / edit / list inside allowed roots (replaces runtime-native file tools)
    locks.py            resource locks: tab, account, file, app; fixed ordering; never held across model calls
    shell/
      manager.py        ShellManager: start / read / send / wait / kill / list; task ownership + cleanup
      pty_proc.py       one process in a PTY: ring buffer + log file
      guard.py          command pre-check helpers used by policy (destructive patterns, cwd jail)
      tools.py          shell.* capabilities (run, read, send, kill, list); OWNER only, lost when exposed
    gui_lock.py         single GUI actor at a time
  sense/
    watchers/
      base.py           Watcher shape, focused app (Accessibility, read fresh), FSEvents stream thread
      app_focus.py      frontmost app changes (app name + bundle id)
      windows.py        focused window titles (Accessibility; not browsers; `[sense] window_titles` switch)
      browser_tabs.py   extension tab_events; AppleScript read for Safari and for unextended profiles
      files.py          FSEvents on configured folders
      idle.py           owner idle / active
      owner_input.py    owner is using mouse/keyboard right now → GUI work yields
      app_installs.py   app install / removal → device.changed
    exclusions.py       exclusion list applied before any trail write, bus event or tag
    tools.py            trail.query (OWNER, `trail:read`; browser / window rows expose the reader)
    trail.py            the pipeline (exclude → redact → id + actor → persist → publish → tag), attribution,
                        resolver history, the retention purge
    manager.py          starts / stops every watcher; `ferb pause` / `ferb resume` (survives restarts)
  device/
    scan.py             setup + rescan: apps, scriptability, AX support, browsers, extension status
    permissions.py      TCC permission checks + open-settings helpers
    profile.py          DeviceProfile load/save/diff; method resolution
    install.py          dedicated interpreter + venv (FERB installed as a copy), LaunchAgent install/update/uninstall
    selftest.py         self-test run inside the daemon: AX read, event tap, one marked (harmless) input event
  identity/
    secrets.py          KeychainSecretStore (FERB identity only)
    mailbox.py          FERB's inbox (read OTPs) and sending from it: normal autonomy rules — requested sends go
                        with the undo window, an unnamed recipient asks, a send outside the request is blocked
    seed.py             verify FERB's Chrome profile holds exactly FERB's account (section 8 step 3); copies nothing
  clock/
    clock.py            SystemClock, FakeClock
    context.py          time header + tool footer builders
    durations.py        measured task durations (per route + role, median / p90) for the time header
    timers.py           durable timers (SQLite): claim → deliver → acknowledge, overdue after sleep / restart; tools
                        time.set_timer / time.remind_at / time.sleep_until
    when.py             the owner's "when" (tomorrow at 5, next Friday, in 2 hours) -> an absolute time, in code
  memory/
    journal.py          append-only episodic log
    items.py            distilled memory items (source of truth): origin rules, versions, conflicts, forget = purge
    summary.py          the permanent daily summary, by code (no model), and distillation's evidence handles
    distill.py          daily: summary first, then one model run (narrative + proposals checked in code), then purge
    distill.md          the distiller's prompt
    recall.py           keyword (FTS5) retrieval; embeddings later behind the same function
    export.py           items → markdown files (read-only view, regenerated; no history kept)
  store/
    db.py               single SQLite connection (WAL) + migrations
    spans.py            span writer (through the redaction seam) + span tree query for `ferb trace`
    migrations/
  native_host/
    main.py             ferb-native-host: stdio native messaging ↔ daemon socket
  cli/
    mcp.py              `ferb mcp`: stdio MCP shim, forwards tool calls to the gateway with the run token
    chat.py             `ferb chat`
    logs.py             `ferb logs`
    client.py           socket client shared by all commands (daemon-unavailable handling)
    setup.py            `ferb setup [--update]`, `ferb doctor`, `ferb uninstall [--purge]` flows
    admin.py            setup, doctor, trace, memory, timeline, device, pause, browsers
extensions/
  chromium/             one MV3 extension for Chrome + Brave
    manifest.json
    background.js       service worker: native messaging, tab control, chrome.debugger sessions
    content.js          DOM snapshot + element lookup (no decisions)
    protocol.ts|js      message schema (mirrors ferb/act/browser/ext_protocol)
macapp/FERB/            FERB.app (Unit 4): push-to-talk microphone + on-device speech (20.1); later: system audio (20),
                        stop hotkey, Keychain anchoring, Safari extension
tests/
  fakes/                FakeClock, FakeRuntime, FakeBrowserDriver, recorded responses
```

Runtime data in `~/.ferb/`:

```
~/.ferb/
  config.toml           runtimes, policy toggles, exclusion lists
  ferb.db               tasks, spans, events, trail, timers, journal, memory_items (SQLite, WAL)
  memory/               markdown export of memory_items (generated, read-only, no history)
  device/<device_id>.json  device profile (one per device; only this Mac in Block 1), committed to a local git
                        repo in ~/.ferb/device/ (history of the Mac's changes; never pushed anywhere)
  python/ venv/ bin/    dedicated interpreter, FERB's venv (a copy of FERB, updated by `ferb setup --update`)
  .ferb-home            marker written by setup; `ferb uninstall --purge` refuses a folder without it
  chrome/identity/      Chrome user-data-dir for FERB's own identity
  logs/                 rotated daemon + process logs
  run/ferb.sock         daemon socket
```

## 4. Core types (sketch)

```python
class Identity(Enum):
    FERB = "ferb"           # FERB's own profile + credentials
    OWNER = "owner"         # owner's logged-in browsers/apps; no credentials ever

@dataclass(frozen=True)
class SecretRef:
    key: str                # e.g. "ferb/email/password"  (FERB identity only)

@dataclass(frozen=True)
class SecretSpec:           # stored beside the Keychain item; the binding is checked at fill time
    ref: SecretRef
    allowed_origins: list[str]    # e.g. ["https://accounts.example.com"]
    account_hint: str             # which FERB account this belongs to

class RiskLevel(Enum): LOW = 1; MEDIUM = 2; HIGH = 3

@dataclass
class Task:
    id: str
    parent_id: str | None
    goal: str
    device_id: str          # which device it acts on (only this Mac in Block 1)
    identity: Identity      # every task declares who it acts as
    scopes: set[str]        # e.g. {"browser:read", "browser:act", "mac:read", "shell"}; "audio" reserved
    role: Literal["reader", "actor", "mixed"]
    request_id: str         # the owner message this task serves (intent checks compare against it)
    success_conditions: list[str]   # from the plan; supervisor verifies against these
    deadline: datetime | None     # the tree's runaway time limit (4 h); no token or dollar budgets

@dataclass
class Outcome:
    task_id: str
    status: Literal["done", "failed", "unknown", "cancelled", "needs_user"]
    summary: str
    artifacts: list[str]
    evidence: list[str]     # span ids / snapshot refs the worker cites
    # verification is NOT set by the worker; the supervisor records it separately (orchestrator/verify.py)

@dataclass
class ActionIntent:         # ledger row, written BEFORE a consequential action executes
    id: str                 # also the idempotency key sent to drivers
    task_id: str
    capability: str
    target: dict            # device, browser/profile/account, recipient, path ...
    payload_hash: str
    state: Literal["intended", "executing", "done", "failed", "unknown"]

@dataclass
class Span:
    id: str; parent_id: str | None; kind: str
    task_id: str | None     # the task this span belongs to (None for daemon-level spans); `ferb trace` queries it
    device_id: str | None   # device where a tool call ran (None for pure model calls)
    started_at: datetime; ended_at: datetime | None
    input: dict; output: dict | None; cost_usd: float | None; error: str | None

@dataclass
class TrailEvent:
    id: str                 # assigned before persistence
    at: datetime
    device_id: str          # device that observed it
    actor: Literal["owner", "ferb", "unknown"]   # FERB's own activity never counts as owner habit
    task_id: str | None     # set when actor == "ferb"
    source: str             # "app_focus" | "window" | "browser_tab" | "file" | "idle" | ...
    data: dict              # redacted per exclusion list before write

@dataclass
class MemoryItem:
    id: str
    kind: Literal["fact", "preference", "person", "project", "env"]
    subject: str            # e.g. "owner", "project:ferb", "person:<slug>"
    key: str                # what it's about (e.g. "dentist"): versions of one fact share kind + subject + key
    content: str
    source_ids: list[str]   # journal / span / trail ids it came from
    observed_at: datetime
    valid_from: datetime
    valid_to: datetime | None     # set when superseded or no longer true
    supersedes: str | None
    confidence: float
    origin: Literal["owner_stated", "observed", "inferred", "third_party"]
    status: Literal["proposed", "active", "retracted"]
```

## 5. Interfaces

```python
class Clock(Protocol):
    def now(self) -> datetime: ...              # timezone-aware wall time: timestamps, schedules
    def monotonic(self) -> float: ...           # elapsed time + timeouts; unaffected by clock changes
    async def sleep_until(self, t: datetime) -> None: ...

class AgentRuntime(Protocol):
    name: str
    features: "RuntimeFeatures"   # live_messaging, continuation, per_call_hooks, usage_visibility,
                                  # native_tools_contained (must be True to be registered: every native
                                  # tool is removed or made inert by the sandbox, and the model-visible
                                  # tool list ⊆ the runtime's declared allowlist)
    async def start(self, task: Task, system: str, context: str, run_token: str) -> "RunHandle": ...
    def events(self, run: "RunHandle") -> AsyncIterator["RuntimeEvent"]: ...
    async def continue_(self, run: "RunHandle", message: str) -> None: ...   # raises Unsupported if no continuation
    async def cancel(self, run: "RunHandle") -> None: ...
    # Unsupported guarantees fail explicitly; nothing is approximated silently.

class Decider(Protocol):
    async def decide(self, question: "Question[T]", state: dict) -> "Decision[T]": ...
    # Decision = value: T, confidence: float | None, backend: str

class Capability(Protocol):
    name: str
    description: str
    scopes: set[str]
    identities: set[Identity]     # which identities may call it
    risk: RiskLevel
    reversible: bool
    Input: type[BaseModel]
    async def run(self, args: BaseModel, ctx: "ToolContext") -> "ToolResult": ...
    def describe(self, args: BaseModel) -> "ActionSpec": ...   # kind, target, content, data labels for policy

class Watcher(Protocol):
    source: str
    async def run(self, emit: Callable[[TrailEvent], None], stop: asyncio.Event) -> None: ...

class SecretStore(Protocol):     # FERB identity only
    def has(self, ref: SecretRef) -> bool: ...
    def spec(self, ref: SecretRef) -> SecretSpec: ...
    # No reveal method for callers. Filling happens in the extension of the FERB profile: the daemon checks the
    # target frame's origin (as the extension reports it) against spec.allowed_origins, then the extension re-checks
    # location.origin in the same call that sets the value on that exact element, and forgets the value.

class MemoryStore(Protocol):
    async def recall(self, query: str, k: int = 8, at: datetime | None = None) -> list[MemoryItem]: ...
    async def propose(self, item: MemoryItem) -> None: ...
    async def commit(self, ids: list[str], reason: str) -> None: ...      # proposed → active; supersession handled here
    async def retract(self, id: str, reason: str) -> None: ...

class BrowserDriver(Protocol):
    name: str                          # "ext:chrome" | "ext:brave" | "ax:<browser>"
    identity: Identity
    features: "DriverFeatures"         # trusted_input_background, parallel_actions, network_events, dom_snapshot
    async def tabs(self) -> list["TabRef"]: ...
    async def open(self, url: str, background: bool = True) -> "TabRef": ...
    async def snapshot(self, tab: "TabRef") -> "PageSnapshot": ...          # structured, sanitized, has a generation
    async def act(self, el: "ElementRef", action: "BrowserAction", intent_id: str) -> "ActionResult": ...
    async def wait(self, tab: "TabRef", until: "WaitCondition", timeout: float) -> bool: ...
    async def screenshot(self, tab: "TabRef") -> bytes: ...                 # only if features.screenshot_background or tab visible
    async def close(self, tab: "TabRef") -> None: ...
    # TabRef = device, browser, profile folder, connection generation, tab id.
    # ElementRef = TabRef + frame + snapshot generation + element id. Stale refs (navigation, reconnect) are rejected.

class ComputerDriver(Protocol):     # OS-neutral; Block 1 implementation: act/mac/driver.py (MacComputerDriver)
    device_id: str
    os: Literal["macos", "windows", "linux"]
    async def act(self, app: str, action: "ComputerAction") -> "ActionResult": ...   # method resolved via DeviceProfile

class ShellManager(Protocol):
    async def start(self, cmd: list[str], cwd: Path, owner_task: str, long_lived: bool = False) -> "ProcHandle": ...
    async def read(self, h: "ProcHandle", since: int = 0) -> tuple[str, int]: ...
    async def send(self, h: "ProcHandle", data: str) -> None: ...
    async def wait(self, h: "ProcHandle", timeout: float) -> int | None: ...
    async def kill(self, h: "ProcHandle") -> None: ...
    def list(self) -> list["ProcInfo"]: ...
```

## 6. Registry and bus

- `registry.py` builds a `Registry` at startup — one explicit line per capability, watcher, runtime, decider, driver.
- Config selects runtimes per role:

```toml
[runtimes]
orchestrator = "codex"    # "claude" | "codex" — owner's current choice: GPT for both (Claude fully built/tested)
worker       = "codex"

[browser]
prefer_for_actions = ["brave", "chrome"]   # order when a site is logged in in several

[identity]
browser = "chrome"
profile = "Profile 22"     # the owner's Chrome profile folder bound to identity FERB at pairing

[mailbox]
provider = "gmail"
address  = "ferb@example.com"   # FERB's own address; setup checks the FERB profile holds exactly this account

[privacy]
# Written on first start; the owner may edit or empty any list (an empty list stays empty).
# Apps FERB never reads, acts on, captures or records: password managers only by default.
exclude_apps = ["com.1password.1password", "com.agilebits.onepassword7", "com.bitwarden.desktop",
                "com.apple.Passwords", "com.apple.keychainaccess"]
exclude_domains = []                                  # never recorded (a domain covers its subdomains)
exclude_paths = ["~/.ssh", "~/Library/Keychains", "~/.ferb"]   # never recorded, even inside a watched folder

[jev]
enabled = []                   # questions whose stricter answers act (`ferb decide enable`); all others: shadow mode

[sense]
window_titles = true           # record focused window titles (redacted); false: app names only
watch_folders = []             # the only folders whose file events are recorded (under the home folder)
idle_after_s = 120
trail_retention_days = 30      # raw trail kept this long once each day is summarized (section 14)
```

- Stable bus events: `task.created`, `task.started`, `task.finished`, `tool.called`, `approval.requested`,
  `approval.resolved`, `trail.event`, `device.changed`, `browser.connected`, `browser.disconnected`,
  `browser.profiles_changed`, `browser.profile_paired`,
  `timer.fired`, `memory.proposed`, `memory.committed`.
- Every bus event has a class. **Critical** (never dropped): `task.*`, `approval.*`, `device.changed`,
  `browser.profile_paired`, `timer.fired`, `memory.committed`. **Telemetry** (may be dropped under pressure):
  `tool.called`, `trail.event`, `browser.connected`, `browser.disconnected`, `browser.profiles_changed`,
  `memory.proposed`. Also critical: `policy.blocked`, `worker.exposed`, `action.held` (undo countdown),
  `action.released`, `action.undone`, `stop.engaged`, `stop.lifted`, `sense.paused`, `sense.resumed`,
  `sense.watcher_failed`, `contact.selected` (who a call or message went to, step 16a), `plan.extended` (the
  owner's yes to a route found while exploring), `voice.heard` / `voice.said` (push-to-talk: what the owner said, as
  recognized, and what FERB said back — shown live by `ferb voice`). Subscriber queues are bounded and the publisher never blocks: when a queue is full, the
  oldest telemetry is dropped and the subscriber is told how many; if a queue is full of critical events, that
  subscriber is disconnected with an explicit overflow error (it must resubscribe and re-read state) rather
  than silently losing one.

## 7. Orchestrator flow

1. `ferb chat` message → supervisor (orchestrator runtime, persistent session per chat).
2. Supervisor returns one typed outcome: **`reply`** (just answer), **`clarify`** (ask one question), or **`plan`**.
   Ordinary conversation never spawns workers.
3. A plan lists subtasks with identity, role (reader / actor), scopes, dependencies (a DAG) and
   **success conditions**. Code validates it: DAG has no cycles, identity rules, scopes ⊆ allowed, depth ≤ 2,
   fan-out ≤ 6 per parent.
4. **Reader/actor split:** readers open untrusted content and return typed fields to the supervisor; actors act
   and only receive those fields, never raw pages, emails or messages (section 10.2).
5. Workers run concurrently; each has a run token, heartbeat and cancellation. A worker failure is returned as a
   typed `Outcome`, never raised, so independent siblings keep going; dependents are skipped.
6. Tool calls → `ferb mcp` → gateway → auth → identity + scope → intent / exposure / dataflow checks →
   resource locks → ledger intent → capability → sanitize → span → result + time footer.
7. Locks: `gui_lock` for real mouse/keyboard, plus resource locks (tab, account, file, app) from `act/locks.py`,
   acquired in a fixed order, held across observe → act → verify, never across model calls or approval waits.
8. Supervisor **verifies** each outcome against the plan's success conditions using fresh reads; the worker's
   own claim is evidence, not verification. A `ledger_done` check (the subtask's actions completed) only applies to
   steps that acted: when the action ledger has nothing for the step (it only read — "WhatsApp is running"), the
   condition is a reading one and goes to the reading verifier (owner's report 2026-10-01).
9. Retries: definite failures retry per policy. `unknown` outcomes (e.g. Send clicked, connection lost) are
   **reconciled first** (re-read the target: did it happen?) and never blindly retried.
10. **No token or dollar budgets anywhere** (owner decision: they don't matter for the owner's experience). Only
    invisible runaway guards per task tree, there to catch stuck tasks: the same failing action repeated 3 times,
    a loop between steps (the same 2–4 calls repeated 5 times with the same results — scrolling through a page that
    keeps changing isn't a loop), a stalled worker (no sign of life for 180 s, not counting waits on
    the owner), and generous caps — 4 hours, 50 worker runs, 1,000 tool calls. The owner hears about a guard only
    when it actually stops something. Distillation and shadow decisions run unmetered.
11. Owner priority: FERB's Mac input goes to apps in the background wherever it can (9.4, spike 18), so it never meets
    the owner's typing. Whenever an app has to be brought forward, FERB's input there always yields to the owner
    (10.5): the owner's keystrokes never land in the app FERB is working in. (`[mac] yield_to_owner`, the owner's
    2026-09-30 switch, is retired: it only governed foreground input, which now always yields — owner decision
    2026-10-01.)
12. `ferb stop` (or the stop hotkey) halts everything immediately (section 10.5).
13. **Environment summary** (build step 16): each turn the planner gets a short summary of this Mac — macOS version,
    the paired browser profiles FERB can use, and the installed apps relevant to what the owner named — plus the
    route code chose for each named service (9.3). The stable part sits with the stable prompt; which of those apps
    are running now changes, so it goes at the end with the time block (section 13).
14. **Availability, in code** (owner decision, 2026-09-30): the planner's "what workers can do right now" list is
    built each turn (`orchestrator/availability.py`) from the tools registered and what's live — the owner's
    browsers only when a paired profile is connected (named), FERB's own browser only when paired and connected,
    Mac apps / screenshots only with the Accessibility / Screen Recording permissions. Anything missing gets one
    short line, e.g. "Unavailable: FERB's own browser (not connected — run `ferb doctor`)", so FERB tells the owner
    plainly what's missing and how to fix it. Plan validation uses the same scopes as the backstop; a plan that still
    needs a missing capability after repair ends in "I can't do that right now. Unavailable: …".

Build step 14a specifics (owner decisions after testing):
- FERB speaks as FERB (never "supervisor"), without internal jargon; `ferb chat --verbose` shows the internals.
- An invalid plan goes back to the planning model with the problems (up to 2 repairs), never to the owner; if only the
  owner can resolve it, the model asks one plain question. Still invalid: a plain apology, details in the journal.
- Readers may return analysis and summaries (up to 2,000 characters a field); when a task ends, FERB writes the
  owner-facing answer from the readers' fields (as data) and the outcome (`task.answer` in the journal).
- Continuity: every turn sees the last day's conversation from the journal (across chat sessions and restarts) and
  the last day's tasks with their status (running, stopped by the owner, interrupted by a restart, …).
- A turn outlives its chat: closing chat never stops a task; its answer is kept (`task_results`), announced by a macOS
  notification, and shown when chat next opens, with anything still running and any question still waiting. An
  approval asked while no chat is open also sends a notification. Ctrl-C stops the task ("stopped by you"; a send
  held for undo is cancelled too). A restart records interrupted tasks for the owner.
- Chat takes one request at a time (a "several at once" chat was tried on 2026-09-30 and removed at the owner's
  request: questions arriving mid-typing were confusing). The daemon itself still runs turns independently.
- A loader while FERB thinks or works (owner request, 2026-10-01): one animated terminal line, "FERB is thinking…"
  and then "FERB is working… (12s)" once a task starts; cleared before every printed line and every question; never
  in piped output.

Build step 8 specifics:
- Acting needs seeing: an actor's act scope brings its read scope (`browser:act` → `browser:read`, `mac:act` →
  `mac:read`, `fs:write` → `fs:read`), since the act tools work on element ids from its own snapshots (found in a
  real run: a plan granting only `browser:act` left the worker unable to open or read the page). Readers get read
  scopes, plus `browser:act` / `mac:act` when they must click to read — open an email, a "View here" link (owner's
  run 2026-10-01: a reader without them couldn't open the email it was sent to read). **Owner decision
  (2026-10-01), after the risk was laid out:** readers keep these three widenings — opening a Mac app to look,
  clicking to read, typing into a search box — accepting that a steered reader could follow a link that carries what
  it read; everything consequential stays refused for readers in the gateway.
  In Mac apps a reader's press counts as "clicking to read" only on a labeled button, link, tab or menu item whose
  label isn't consequential (send, call, delete, block, forward, share, allow, install, pay, sign out, …); unlabeled
  or risky ones stay refused. The reader's own instructions (`prompts/reader.md`) say exactly this — a reader told
  "you can't change anything" gave up without trying (owner's run 2026-10-01).
- Depth ≤ 2 means: request → subtasks → the verification / reconciliation workers under a subtask; workers can't
  start workers. Readers can't act: the gateway blocks any non-reversible kind for a reader task, whatever its scopes.
  A reader may still click (the gateway keeps it to opening and looking); other acting scopes the planner gives a
  reader (fs:write) become their read-only forms in code before validation, instead of a model repair round.
  Opening an app in the background (`mac.open`) is part of reading — a reader that finds an app closed opens it and
  looks, rather than reporting "not running" (owner decision 2026-10-01).
- A step whose input field an earlier reader didn't find (empty) is skipped with that reason, never run on an
  empty value; `{reader.field}` in a goal is read as "that input".
- Reader output is typed fields (string / number / boolean / string list / email / URL); strings default to 500
  characters and the plan may raise a field to at most 2,000. Actors get them wrapped as untrusted data.
- Verification: each success condition is checked by code where its form allows (file exists / contains, the
  subtask's ledger rows done, a tab's URL, the signed-in account) and by a read-only verifier worker only where it
  needs judgment; the path taken and the evidence are recorded per condition.
- Retries: a definite actor failure is retried once (not after a guard stopped it); `unknown` is reconciled by a read-only worker
  (happened → done, didn't → failed, unclear → the owner decides), never retried; readers aren't retried.
- Runaway guards (step 10 above) replace budgets; plans carry no budget and no per-subtask deadline (every subtask
  shares the tree's 4-hour limit). A worker with no sign of life for 180 s is stalled and cancelled; time waiting
  on the owner (undo windows, real input pausing for the owner) doesn't count.
- The task tree is persisted in SQLite; after a restart, subtasks that were running become `interrupted`.

Runtime-native tools: each runtime is launched with its own shell, file, web, subagent and plugin tools
disabled wherever the runtime allows, and inherited user settings and rules ignored; FERB provides equivalents
through MCP (`act/fs`, `act/shell`, browser). A native tool the runtime cannot remove must be made inert by the
runtime's OS sandbox: Codex workers run with `--sandbox read-only`, `approval_policy = never`, MCP tools
pre-approved, and a pinned non-code-mode model (code-mode models expose a JS tool runner and built-in subagents
that cannot be disabled); its non-removable `apply_patch` therefore cannot write. Each runtime declares the exact
list of tools its model may see; the list actually sent to the model (from the runtime's request trace) must be
a subset of it. A runtime that fails this check, or whose sandbox cannot be verified, is not registered
(spikes 1–2, test 21.1). The check runs on every run at no token cost: Codex from its request trace, Claude through
a proxy bound to localhost, used only for FERB's Claude runtime, which records **only the tool names** of each
request — never auth headers, prompts or replies. A run that shows a tool outside the allowlist is killed and the
runtime unregistered.
When Codex is the supervisor it gets no FERB tools, runs in the read-only sandbox (its non-removable `apply_patch`
can't write) on the pinned `gpt-5.5`, and keeps conversation continuity by re-sending the chat history each turn
(ephemeral: no FERB conversation is written into the owner's `~/.codex` session history).
Speed, within the owner's Codex subscription (owner decisions, 2026-10-01): supervisor-mode runs (planner, answer
writer, distillation — no tools) use `model_reasoning_effort = low`, FERB's own instructions in place of Codex's
built-in ones (`model_instructions_file`; about half the input tokens) and none of Codex's environment / permissions /
apps / collaboration blocks, and end at `turn.completed` without waiting for Codex to exit. Tasks (worker mode) keep
their normal settings. FERB runs Codex with its own home (`~/.ferb/codex-home`, signed in once with
`ferb setup --codex-login`) so the owner's personal Codex settings and skills never load; until then the default home
is used and `ferb doctor` says so. The prompt stays bounded: the last 8 conversation entries verbatim (≤ 400
characters each), the rest of the day as one summarized line. Measured floor: the model service answers a trivial
request in about 4 s through Codex whatever the model or prompt size; Codex picks a new cache key per run, so prompt
caching hits only sometimes. When the model service fails, FERB records its message and tells the owner the reason in
plain words (usage limit, not signed in, connection).

## 8. Setup and device profile

**Progressive setup (owner rule, 2026-09-30, build step 15).** `ferb setup` keeps only what macOS requires: the
dedicated interpreter, the LaunchAgent, the TCC permissions macOS needs to be granted up front, the device scan and
the self-test. Everything else is asked the first time a task needs it, and remembered:
- facts the owner gives (e.g. their email, the first time they say "email me") are stored as owner-stated memory;
- permissions (e.g. "Allow FERB to access ~/Projects?" the first time a task needs a folder outside the allowed
  roots; Automation for an app) go to config only after the owner's explicit yes, and only for a target the owner
  named in their own words — content FERB reads can never grant or even trigger one;
- never during an unattended (scheduled) run: needed info is collected when the schedule is created, or the
  question is queued for the owner's next chat and the step is skipped.
FERB's identity profile (`ferb setup --identity`) and the Jev key (`ferb setup --jev`) are their own commands, run
when first needed. Optional TCC permissions and per-app Automation are asked on first use.

`ferb setup` (idempotent; `ferb doctor` re-checks everything). Setup never quits or kills the owner's browsers —
when a browser must be closed it asks the owner to quit it and waits — and it does not touch Brave (FERB's
identity profile is in Chrome only; the Brave pairing check is a separate, owner-scheduled step before build
step 6). Build order: step 3 builds permissions, device profile, interpreter, LaunchAgent, self-test, doctor and
uninstall; step 5 the identity-profile account check; step 6 the native host, extension, pairing (including the
FERB profile) and External Extensions.

1. TCC permissions: Accessibility, Screen Recording, Automation (per app); Input Monitoring if Accessibility alone
   doesn't allow the owner-input listen-only tap; Full Disk Access optional.
   FERB cannot grant these — it opens the right System Settings pane and polls until granted.
2. Automation (per-app) permission: request for apps the owner selects at setup; any other app is asked on first use.
3. Browsers (details in 9.5 and 9.6):
   - Discover installed browsers and every profile in each (Chromium: `Local State`). Safari: watched read-only via
     AppleScript (needs Automation permission for Safari); no profiles, no extension.
   - Install `ferb-native-host` and write its native-messaging manifest into Google Chrome's
     `NativeMessagingHosts` folder, which both Chrome and Brave read (Brave 151 ignores its own folder; measured
     before build step 6). Also write it into Brave's own folder: harmless today, and it keeps working if a Brave
     update switches folders. `ferb doctor` checks that each paired browser actually reaches the host, so a change
     shows up as a doctor failure rather than silently. The host is installed outside macOS-protected folders
     (`~/Desktop`, `~/Documents`, `~/Downloads`) — a browser can't execute it from there — e.g. in `~/.ferb/bin`.
   - Owner ticks the profiles to enable now (default: most recently used). FERB opens each ticked profile on the
     extension install page, then runs pairing; waits for `browser.profile_paired`.
   - Drop the External Extensions file so unticked profiles offer the extension the next time they are opened.
   - Profiles without the extension are watched read-only via AppleScript.
   - Set up FERB's identity profile. The owner creates a dedicated "FERB" profile in their normal Chrome and signs
     FERB's Google account into it, holding no other accounts or site logins. Setup finds it in `Local State`,
     checks that its Chrome accounts (`Local State` + `Preferences` → `account_info`) are exactly FERB's account,
     shows it to the owner for confirmation, installs the extension there and pairs it as identity **FERB**. Nothing
     is copied, Chrome is never quit, and cookies and saved passwords are never read. Setup then verifies the
     session through the extension (the inbox loads as FERB's account); `ferb doctor` re-verifies it and, if the
     session or the account check fails, tells the owner what to fix.
     FERB credentials go to Keychain; their `SecretSpec`s go to the owner-only `config.toml`.
4. Assign this Mac a stable `device_id` (generated once, stored in `~/.ferb/config.toml`). Scan device →
   `DeviceProfile` (apps, scriptability, AX support, browsers + driver availability); commit to git.
5. Install launchd agent; start daemon; run the self-test **through the launchd-started process** (the one that
   will really act), not the setup process: AX read, one input event, background tab open/snapshot/close per browser.
6. **Daemon interpreter and permissions.** TCC grants attach to the executable launchd runs, not to FERB or the
   terminal (spike 8). The daemon therefore runs on a **dedicated Python interpreter installed under `~/.ferb`**
   (e.g. `uv python install` into a fixed path), used only by FERB; FERB's venv is built on it and nothing else
   uses it, so the Accessibility and Screen Recording grants don't extend to any other Python on the Mac. It changes
   only when FERB is upgraded. `ferb setup` asks the owner to grant that interpreter; `ferb doctor` checks the
   permissions **from inside the daemon** (never from the CLI's process) and tells the owner to re-grant when an
   upgrade has invalidated them.

**Mac app method resolution:** AppleScript / CLI → Accessibility → screenshot + click.
Watchers emit `device.changed` on app install/removal (FSEvents on /Applications, /Applications/Utilities,
~/Applications); the profile rescans that app only, and the change is recorded in the trail (actor unknown).

## 9. Browser layer

### 9.1 Modes

| Mode | Browser | Identity | Driver | Credentials |
|------|---------|----------|--------|-------------|
| FERB browsing | Chrome's "FERB" profile (owner's normal Chrome) | `FERB` | `ext:chrome`, bound to identity FERB at pairing | FERB's, via Keychain + `fill_secret`; the profile's own sessions (e.g. Google, signed in by the owner) |
| Act as owner | Owner's Chrome, Brave | `OWNER` | `ext:*` → `ax:*` fallback | None — existing sessions only |
| Watch | Owner's Chrome, Brave, Safari | — | Chrome, Brave: extension events, AppleScript/AX read if extension missing; Safari: AppleScript read of tabs and URLs | None |

### 9.2 Driver order per browser

| Browser | Primary | Fallback 1 | Fallback 2 |
|---------|---------|------------|------------|
| Chrome, Brave | FERB extension: content-script snapshot + `chrome.debugger` for actions | Accessibility (the web AX tree is built on demand; if the first query returns it empty, re-query for up to ~2 s. `AXManualAccessibility` is unsupported since at least Chrome 154 / Brave 151) | Screenshot + click |

If the FERB extension is installed in a browser but no profile of it is paired (an unpaired install still says
hello), FERB tells the owner — pairing the owner's real profiles waits for the §21 gate (`[gates] owner_profiles`),
after which `ferb chat` offers to pair — and doesn't fall back to screenshots of that browser (build step 14a).
Windows on other Spaces or in full screen are found (all windows, not just on-screen ones; when Accessibility lists
none, the app's main / focused window — spike 18).
**Input order, every app (owner decision 2026-10-01, spike 18):**
1. Accessibility, with the app in the background: press buttons, set a text box's whole value, focus.
2. Keystrokes posted to the app's own process (`CGEventPostToPid`), still in the background — typing, and keys such
   as Escape or a shortcut. Typing is checked by reading the box back; a key, by whether the window changed. An app
   whose keystrokes don't arrive is remembered (for the daemon's life) and goes to step 3.
3. Last resort — clicks by position, and apps that ignore background keystrokes: bring the app forward **once per
   stretch of work** (raise that exact window, wait for it to be on screen), keep it there while the task keeps
   acting in it, and put the owner's app back once — when the task moves to another app or has done nothing there
   for 3 s. While the app is forward, the owner's input wins (10.5).
FERB reads the app the owner is in from Accessibility's focused application; opening an app (`mac.open`) takes the
owner back to their own desktop at once, and when the app is running with no window, reopens it from the app's own
menu ("Open main window", or its entry in the Window menu) through Accessibility — still in the background.
**Never "bring it to the front" (owner's report 2026-10-01):** the planner's and workers' instructions say apps work
from the background and goals are what to read or do, never "bring X to the front"; keys that switch the owner's apps
or desktops (⌘Tab, Control+arrows) and AppleScript `activate` / `reopen` are refused. An element "click" is a press
in the background first. Snapshots include a button's value when it has one (a chat row's latest message).
**Electron apps** (Slack, Discord, VS Code…) build their full Accessibility tree only when asked: FERB sets
`AXManualAccessibility` once per app process before reading it, so their controls can be pressed in the background
(Slack's hidden tree made a press fail and FERB click in the foreground). After opening an app FERB waits up to 10 s
for its titled main window (Electron apps first show an untitled panel).
**The running-app list stays current:** macOS refreshes it only when the process gives its event system a turn, which
the daemon's asyncio loop never did — an app launched after the daemon started stayed "not running". The daemon now
runs one zero-wait turn every 0.2 s (under 1 ms), and a lookup by bundle id asks macOS directly. App names are
matched ignoring invisible direction marks (WhatsApp's name is "\u200eWhatsApp").
**Editing and sending (owner decision 2026-10-01, every app):** buttons over keystrokes, and whole values over
cursor editing. Typing into a search box is a search (it sends nothing; a reader may do it). Arrow keys while a text box has focus are refused ("set the whole value again instead"). Return /
Enter in a text box is refused when the window has a Send button (press it instead) — and always in a communication
app, where Return would send without the countdown or the contact check (10.3). After a send, the worker checks by
reading the chat, never by sending again: a second send to the same contact (or the same recipients) in one task
needs the owner's OK, with both texts shown.
The image with the tree (owner request, 2026-10-01): with the `screen` scope, `mac.snapshot` also returns an image of
that one window with each control's box outlined and numbered ("12" = element s<gen>e12), drawn in memory before
encoding, so the model acts on what it sees. Same rules as `mac.screenshot`: excluded apps and windows showing a
password field give no image (the tree still comes back, with the reason); never saved; browsers are read through the
extension instead. The image counts as the latest screenshot for `mac.click`. `[mac] snapshot_image = false` turns it
off. Browser snapshots don't carry marked images yet (marks need the page's viewport size from the extension).

Browsers prefer the extension: `mac.screenshot` / `mac.click` (screenshot + click) are the fallback for what the
extension can't do (browser UI such as permission prompts, canvas-only pages). While a paired owner profile of that
browser is connected, those tools need a stated `fallback_reason`, recorded with the call; with none connected (or
for Safari) they're used directly.

The FERB profile is opened like any paired profile: FERB may start Chrome detached if it isn't running, never
quits or kills it, and wakes a disconnected extension with its blank local page (9.4).

Not used: Chrome's live-session CDP attach to owner browsers (it would bypass the extension's allowlist and the
boundary in section 2). If ever needed, it requires a separate, explicitly approved design.

Known capability differences (drivers report these in `features`; planner and resolver read them):
- Chromium extension: trusted input in background tabs via `chrome.debugger` with focus emulation — while
  attached, the page reports `visibilityState: "visible"` though the tab stays inactive; input starts only after a
  readiness probe confirms the tab accepts it — plus parallel actions across tabs and network/load events.
- Safari: watch-only in Block 1; acting in Safari (extension, AX input) is deferred to a later block.
- All features are **conditional until a spike confirms them** on this machine. Example: the ordinary extension
  screenshot API captures only the visible tab, so background screenshots need debugger mode or are unavailable.

### 9.3 Resolver (site → browser + profile)

The unit is **browser + profile** (e.g. `brave/Profile 3`), never just a browser.
If the owner names a profile ("in my Kartikey profile"), resolve it first with 9.5 and restrict to it.
For an `OWNER` task that names a site:
1. A tab with that site is already open in some browser + profile → use it.
2. Trail shows the owner uses that site in a browser + profile → use it (most recent / most frequent).
3. Owner's default browser, last-used profile.
4. Ask in chat.
If the site is open in two profiles, ask which (and offer to remember the choice as an alias).
If the chosen profile has no paired extension → offer on-demand install (9.6) or fall back to `ax:*`.
If the site is available in several browsers and the task is action-heavy, prefer by `browser.prefer_for_actions`
— but an explicitly named profile always wins over preference.
After loading, the extension reports a login-state hint. Before a consequential action, verify the **account**
(e.g. which Gmail address), not just "signed in", with a per-site **account extractor** (Gmail and YouTube first):
a mismatch with the resolved account refuses the action (safety test 12). On a site without an extractor the
account can't be verified: FERB asks the owner once per site + profile and remembers the answer as learned trust
(10.4), so it doesn't ask again. If signed out → stop and tell the owner. FERB never logs in as the owner.
The `ax:*` fallback stops if it cannot establish which profile/account it is looking at.

**Whose browser (owner decision, 2026-09-30; takes precedence over the web-route rules below).** The owner's browser
profiles are used only when the owner's own words say so: "my browser", "my Brave" / "my Chrome", "in Brave", "my
<profile name> profile", "this / the current / the open (…) tab", or "my <service>" ("my LinkedIn", "my Gmail").
Checked in code (`policy/services.py:owner_browser_named`); a plan giving an "owner" subtask browser scopes without it
is refused by plan validation. Otherwise work on a website ("post this on LinkedIn", "find X on YouTube") uses FERB's
own browser and email (identity "ferb"). A browser is never a Mac-app route ("in my Brave" is the owner's Brave
profile, not the Brave app), and each service is announced once.

**The owner's open tabs.** When the owner's words point at their browser, a worker can list the open tabs of the
paired profiles (`browser.tabs`: host, active) and take one over for the task (`browser.use_tab`) — e.g. "keep
scrolling the comments in my current YouTube tab". An attached tab is the owner's: FERB forgets it when the task ends
and never closes it. Everything else about acting in it (intent, data flow, account checks, undo) is unchanged.

**Services and apps (build step 16, owner decisions 2026-09-30).** When a task names a service or app ("video call X
on WhatsApp", "play it on YouTube"), code — not the model — picks how to reach it, in `policy/services.py`:
0. the owner's explicit correction ("use the web version"), stored as an owner-stated preference (key
   `route:<service>`), always wins;
1. how the owner actually uses it: the most used of its app / its website per browser profile over the last 14 days of
   the owner's own trail (FERB's own actions never count), ties broken by the most recent;
2. an installed native app (device profile);
3. the website in a paired browser profile;
4. nothing workable → ask once and remember.
A service is known from a catalogue of ~30 common services (their Mac apps and websites), else by the name of an
installed app, else by the name of a site in the trail; anything else is unknown → ask once and remember. A website
route is only ever a browser profile FERB can drive (paired). The route is shown in plain words when a task starts
("Using the WhatsApp app"). Calls and messages the owner asked for go ahead without a prompt, held with an undo
countdown like sends ("Calling Rahul… undo", section 10.3). A contact name matching more than one person is never
guessed — enforced in code (10.3, build step 16a).

### 9.4 Extension contract

- Permissions (Chromium MV3): `tabs`, `scripting`, `debugger`, `nativeMessaging`, `tabGroups` (the "FERB" group),
  `alarms` (the 30 s reconnect), `storage` (install id, seen command ids), host access to all sites.
- The extension **only executes** daemon commands and **reports** state; it holds no policy and no task logic.
- Message protocol (JSON, versioned, validated on both sides):
  - daemon → ext: `open`, `navigate`, `close`, `list_tabs`, `snapshot`, `act`, `wait`, `screenshot`, `attach_debugger`,
    `detach_debugger`, `cdp` (allowlisted), `account` (run the site's account extractor, 9.3), `wake_close` (close
    exactly the wake page), `frame_origin` (a frame's origin, from its isolated world), `fill` (a sealed secret into
    one element, 10.1), `read_source` (page source, refused for every URL except FERB's inbox feed), `window_password` (whether the
    active tab of this profile's window at exact screen bounds has a password field, any frame: yes / no / can't
    tell, never content; the screenshot check, build step 9); control: `welcome` (connection generation), `paired` (key), `refused` (e.g. protocol
    mismatch)
  - ext → daemon: `result`, `error`, `tab_event` (created/updated/activated/removed), `hello` (`install_id`,
    extension/protocol version, owned resources). The extension can't know its profile folder: the daemon takes
    browser + profile from the pairing binding of that `install_id` (9.6).
- Two execution modes:
  - **Page mode (default):** snapshots, reading, navigation and simple actions via injected content scripts.
    No debugger banner, no browser-wide commands.
  - **Debugger mode:** only for actions needing trusted input (clicks/typing on protected pages) or precise
    load/network waits. `chrome.debugger` attaches to one tab for those actions, then detaches
    (Chrome shows "<extension> started debugging this browser" in every window of that profile while attached,
    and for ~5 s after detach).
- **CDP allowlist lives in the extension**, not only the daemon, and is exact: named methods, allowed parameter
  shapes, and filtered return fields (input dispatch, DOM/AX reads, page lifecycle events, screenshot, and focus
  emulation (`Emulation.setFocusEmulationEnabled`) for background input only).
  Never forwarded, even if the daemon asks: cookie and storage access (`Network.getAllCookies`, `Storage.*`),
  other targets (`Target.*`), and arbitrary script evaluation (`Runtime.evaluate` with free-form expressions).
  Allowlist changes require owner approval (section 16).
- **Snapshots are sanitized in the extension before sending:** password and OTP field values, hidden inputs and
  tokens in URLs are replaced with markers; the daemon's `sanitize/` pipeline runs again on arrival.
- **Reliability semantics:** every command has a request id, the connection generation, a timeout and is
  cancellable. Duplicate request ids are ignored. After a reconnect (e.g. the MV3 service worker was restarted),
  the extension re-announces state; pending commands from the old generation are never replayed — they become
  `unknown` in the ledger and are reconciled. Snapshot size is bounded (element and text caps).
  The extension registers `runtime.onStartup` so it connects after a browser restart. `hello` carries the
  extension/protocol version; the daemon refuses a mismatch (Chrome keeps running a stale unpacked extension
  until it is reloaded). The native host exits as soon as its daemon connection drops, and the extension queues
  outgoing messages until reconnected.
  The extension records each command id as seen before executing it (in `chrome.storage.session`, which survives
  worker restarts) and tags what it creates with that id; `hello` re-announces owned resources, and the daemon
  reconciles `unknown` from them, never resending. After a worker death the extension reconnects on the next tab
  event or a 30 s `chrome.alarms` tick; until then the daemon treats that browser as temporarily unavailable.
  When a task needs a temporarily unavailable paired browser, the daemon wakes it without a window first: the
  profile is loaded with no startup window — through `open -g -b <browser> --args --profile-directory=…
  --no-startup-window` when the browser isn't running, and through the browser's own binary with the same flags when
  it is (macOS passes `open`'s arguments only at launch, so a running browser never saw them — owner's run
  2026-10-01) — which starts the extension and reconnects it without moving the owner (spike 19). Only if
  it hasn't connected within 8 s (the profile was already loaded with its extension asleep) does the daemon open a
  unique blank local page in that profile, still through `open -g` (e.g. `http://127.0.0.1:<port>/wake/<token>`;
  not `about:blank`, whose fragment is dropped so it couldn't be told apart from the owner's tabs), and close
  exactly that page after the reconnect (spike 13). Never the browser's binary with a URL — that took focus — except
  for pages the owner must see (pairing).
- **The browser the owner names (owner's report 2026-10-01):** "my school gmail in chrome" names Chrome and the
  profile whose name or account matches ("school", its school.edu account; everyday words like "work" only as "work profile"
  / "my work"). FERB uses exactly that — the planner is told so, and the router refuses any other profile for that
  request (opening, listing and taking over tabs alike) — or, when it isn't connected, says why (no extension / not paired) and that `ferb browsers pair <name>`
  connects it. It never substitutes another browser. Whether FERB's extension is installed in a profile is read from
  that profile's settings on disk. Chrome removed installing unpacked extensions from the command line in version
  137, so `ferb browsers pair` walks the owner through it: it opens `chrome://extensions` in that profile, copies the
  extension folder's path, shows the three clicks (Developer mode, Load unpacked, ⌘⇧G + paste), waits until Chrome
  has it, then pairs as usual.
- **Moving the owner and taking them back (owner decision 2026-10-01):** on macOS any new browser window can pull the
  browser to the front — a long-standing Chromium problem nobody can fix from outside. So FERB avoids creating
  windows (the windowless wake; the extension creates a profile's first window *minimized* and keeps it for later
  tabs), and when a browser step does pull the browser forward, FERB takes the owner straight back to their app and
  desktop, without announcing it. (A confirmation before such a switch may come with FERB's own app later.)
- Tabs FERB opens are background (`active: false`) and grouped in a "FERB" tab group; closed at task end unless kept.
- Snapshots use one format for all drivers (`snapshot.py`): elements with stable ids, role, label, value, rect, visibility.
  Screenshots only when the snapshot is insufficient (canvas-heavy pages) or requested.
- Native host and daemon accept messages only from the FERB extension IDs listed in config.
- Every message from an extension carries its `install_id` and is authenticated with its per-install key;
  the daemon maps it to browser + profile (9.6).

### 9.5 Profile discovery and name resolution

Problem this solves: browsers identify profiles by folder (`Default`, `Profile 3`), while people use display names
("Kartikey"). An agent guessing either one fails. Resolution is **deterministic code that runs before any model**.

- **Discovery:** read each Chromium browser's `Local State` → `profile.info_cache`
  (folder → display name, signed-in email, etc.). Read-only; never touch cookies or other profile data.
  Re-read on startup and whenever the file changes (FSEvents) → `browser.profiles_changed`. Browsers write
  `Local State` lazily, so a new profile may appear only after the browser writes it (e.g. on quit); the FSEvents
  re-read picks it up.
- **Table:** `browser_profiles(device_id, browser, folder, display_name, email, install_id?, paired_at?, aliases[])`.
- **Match order for a spoken name:**
  1. exact display name (case-insensitive),
  2. email or email local part,
  3. saved alias (e.g. "work profile" → `brave/Profile 3`; stored as a memory item when the owner clarifies).
  Any step that matches more than one profile → **ambiguous**, never first-match.
  If deterministic steps end ambiguous or empty, `Decider` question `browser.profile_match` may **suggest** a
  candidate, shown to the owner for one-tap confirmation; it never selects on its own.
  No match → reply "no profile called X; found: …", never a guess.
- **Open:** launch with `--profile-directory=<folder>`; a running browser takes over the request. If the browser
  isn't running, FERB may start it, detached (never as its own child with a timeout), and FERB never quits or kills
  the owner's browser. Only folders returned by discovery are ever passed to `--profile-directory` (an unknown
  folder silently creates a new profile). Profiles are identified by folder or paired `install_id`, never by
  window title.
- Renames are harmless: identity is the folder, display names refresh from `Local State`.

### 9.6 Extension installation and pairing

- **Chromium install paths:**
  - Setup-selected profiles: FERB opens `--profile-directory=<folder>` on the extension's install page.
  - Everything else: External Extensions JSON (macOS requires the extension to be on the Chrome Web Store,
    unlisted is enough); each profile shows Chrome's enable prompt when next opened: the extension is installed
    disabled until the owner accepts it. Brave uses the same mechanism in its own data folder (spike 10).
    Enterprise force-install
    policy is **not** used (shows "managed by your organization", not removable).
  - On demand: a task needs an unpaired profile → FERB asks in chat → opens that profile on the install page →
    waits for pairing → continues the task.
  - Development (build step 6): load unpacked manually. An unlisted Chrome Web Store listing comes before the
    owner's real profiles are paired (External Extensions needs it; spike 10).
- **Pairing:** after install, FERB opens `--profile-directory=<folder>` on the extension's own pairing page
  (`chrome-extension://<id>/pair.html#token=…`, blank, closes itself), so only the extension in that profile can
  read the one-time token. The extension sends `{install_id, token}` to the daemon, and the daemon binds
  `install_id → browser + folder`. Tokens expire in minutes and are single-use.
- Extension generates `install_id` once and keeps it in extension-local storage (per profile).
- A connection with an unknown `install_id` may only send pairing and health messages: no trail events, no actions.
- Pairing also exchanges a per-install key used to authenticate every later message: each carries a strictly
  increasing `seq` and an HMAC with that key.
- Every binding carries an identity: `OWNER` for the owner's profiles, `FERB` for the profile named in `[identity]`.
  The resolver never routes an `OWNER` task to the FERB profile, nor a `FERB` task to an owner profile. Pairing
  or re-pairing the FERB profile needs the owner's confirmation, and is refused unless the profile's Chrome
  accounts are exactly FERB's account.

## 10. Identity, secrets and safety

Principle: **the owner's instruction is the approval; security runs in the background.** Explicit confirmations
are rare (10.3). Everything else is enforced automatically by the gateway.

### 10.1 Identity (keeps the two account sets separate)

- Every task declares `identity`.
  - `FERB` tasks: the FERB profile (extension driver, tools `ferb_browser.*`), `fill_secret`, FERB mailbox,
    `act/fs` inside FERB's workspace only.
    No shell, no AppleScript/Mac control, no access to owner browsers.
  - `OWNER` tasks: owner browsers (`ext:*` / `ax:*`, tools `browser.*`), Mac control, shell, files — subject to 10.2.
    Never the FERB profile, FERB secrets or FERB mailbox.
  - A task cannot switch identity; the supervisor uses separate workers and passes typed fields between them.
- Child workers never receive more authority than their parent task.
- FERB credentials: Keychain only, described by `SecretSpec`. `fill_secret(target, ref)`: the daemon checks the
  target frame's origin (reported by the extension) against `allowed_origins`, then sends the value along
  Keychain → daemon → native host (relay only, signed messages) → extension → the isolated world of the checked
  frame, which re-checks `location.origin` in the same call that sets it on that exact element. The value is never
  logged or stored along the way; FERB records `ref.key` only.
- The value travels **sealed**: AES-GCM with a key set up at pairing (per install; derived on both sides with
  HKDF-SHA256 from the pairing key, label "ferb seal v1", so it never crosses the wire), a fresh nonce per seal, and the
  fill command's id and the allowed origin as associated data, so a sealed value can't be replayed into another
  command or origin. Only the extension's fill call unseals it; the native host and every message and log carry
  ciphertext (safety test 21). The inbox feed the daemon reads codes from travels sealed the same way, in the other
  direction (extension → daemon, bound to its command id and the feed URL), so a code never crosses in clear. This protects FERB's own messages and logs, not against malware running as the
  owner's user.
- One-time codes: read from FERB's inbox (Gmail's Atom feed of unread mail, read through the FERB profile) and set
  straight into a field whose origin is allowed for that account's `SecretSpec`, along the same path — never
  shown to a model, logged or stored; refused when the newest message holds more than one candidate code.
- Owner credentials: never read, typed, stored, or requested. No access to the owner's cookie stores.

### 10.2 Automatic protections (never prompt the owner)

- **Intent match** (`policy/intent.py`): every consequential action is compared with the owner's request
  (`Task.request_id`): recipient, target account/site, content, amount, path. Matches → proceeds.
  Does not match (asked to summarize, worker tries to send / run a command / message someone new) → blocked
  and logged. Deterministic checks first; Jev `action.matches_request` for fuzzy cases, which can only block.
  The typed intent (allowed action kinds, recipients, sites/accounts, paths, amounts) is derived **only from the
  owner's message, at plan time, before any untrusted content is read**. It includes the action kinds the request
  clearly implies (e.g. "book a table" implies form fill and submit). It is recorded once per request and workers
  can never change it. An action kind not in it is blocked; a requested kind with a target outside it asks.
  **Names the owner uses count** (build step 14a, `policy/names.py`): an app by its name ("Brave" → the Brave app's
  bundle id, matched against the device profile; exactly one match), a site by its name or a known alias ("Slack" →
  slack.com, "Gmail" → mail.google.com) or its domain. They still have to be in the owner's own words.
  **Pasted or quoted content in the owner's chat message** (terminal bracketed paste, fenced code blocks, `>`
  quotes) is wrapped as untrusted data — tags carrying a random id — before the supervisor sees it; the intent is
  derived only from the owner's own words around it, and code checks that every intent target (recipient, site,
  account, path) appears in those words. A path counts when each of its folder / file names is in their words, however
  written ("notes/2025 folder in desktop" names Desktop/notes/2025; the usual home folders may be implied — owner's report
  2026-10-01). Paths resolve where the owner means them: "Desktop/…", "Documents/…", "Downloads/…" are their folders
  (other relative paths are FERB's workspace), and a name Finder shows with "/" is found by its on-disk ":" form
  (notes/2025 is notes:2025), so the folder-access question names the real folder. The owner's home prefix (/Users/<them>, ~) is never a name they must have typed. The folder-access question also comes for a folder named in the owner's own words this request even if the plan left it out of the intent — never for a bare home folder (all of Desktop), never from content FERB read. When the whole message is a paste — or a paste with only a greeting typed
  around it ("hey " + the paste; owner's report 2026-10-01) — `ferb chat` asks, in the owner's own terminal, whether
  it's their request; a yes sends it as their words.
  **Misspelled names (owner decision 2026-10-01):** before planning, a word in the owner's own words that is one or
  two letters off exactly one installed app or catalogue service (a swap counts as one), and isn't itself a word
  (the Mac's dictionary, plus everyday tech words), is read as that name and FERB says so ("Using Slack, from
  'salck'"); close to several, FERB asks "Did you mean …?" and the answer continues the request. Pasted text is never
  corrected. When a plan still names something the owner didn't type, FERB says the real reason: it only appears in
  pasted text (not acted on), or it couldn't find it in what they said (which did they mean?); a planner mistake is
  called one ("a problem on my side, not with how you asked"), without its internals.
  **The owner's switch (2026-10-01):** `[policy] pasted_text_is_data` (default true). The owner turned it off: what
  they paste counts as their own words — no paste question, no tags — until they turn it back on. Every other rule
  (exposure, the intent's targets, approvals) still applies.
- **Exposure** (`policy/exposure.py`): a worker that has read untrusted content (web pages, emails, messages,
  downloaded files, anything not from a trusted source) is *exposed*. Exposed workers lose shell, AppleScript and
  file-write capabilities for the rest of their run. If those are needed, the supervisor starts a clean worker
  and hands it only typed fields. Owner-marked trusted sources (own notes, chosen senders) do not expose.
- **Reader/actor split** (section 7, step 4): raw untrusted text never reaches a worker that can act.
- **Data flow** (`policy/dataflow.py`): data read from owner accounts may only go to destinations the request
  involves. Pasting it into any other site, account or identity is blocked. Sending content to the model
  providers themselves is inherent to the design and accepted.
- **Undo windows**: sends/posts are held ~10 s before executing. The pending send is shown with a countdown in
  `ferb chat` and `ferb logs`, and `ferb undo` cancels it. No system notifications in Block 1.
- **Reversible actions just run**: drafts, navigation, search, opening tabs, moving files to Trash.
- **Exploring beyond the plan** (owner decision, 2026-09-30): workers may open and read apps and sites outside the
  plan's list — never apps on `[privacy] exclude_apps` (`mac.open` included) — with a short stated reason for each
  (`why`, a field every tool takes; the gateway strips it before the tool runs and records it in that call's span).
  Without a reason such a call is refused. Actions (click, type, send, call, write) stay limited to the plan's apps
  and sites; one outside them goes to the owner as a route change (10.3). Exploring counts toward the runaway guards
  like any tool call.
- **Injection signals**: content that tries to issue instructions (Jev `content.injection_suspected`) marks the
  worker exposed immediately and is reported in `ferb logs`.

### 10.3 When FERB asks (the only confirmations)

Only when a mistake is costly **and** the action was not explicitly requested, or something looks wrong:
- irreversible actions (below): **every time**, even within the same request, at every autonomy level;
- intent mismatch the owner may still want, surprising destination, or injection signals on the path.

**Approval scope (owner decision, 2026-09-30, build step 14a).** One approval covers the same target and action type
for the rest of the request (e.g. approving input to the Brave app once covers every further input to Brave in that
request). A different target (another recipient, site, account, path or amount) is a new question. Everything else
just runs, with no prompts. While FERB waits for an answer, the tool call waits (up to 10 minutes; the owner's time
never counts as a stall) and the task continues once approved.

**Irreversible actions always ask, one call at a time**, never covered by an earlier approval:
- the fixed floor, enforced by code from each capability's declared reversibility and kind: payments, permanent
  deletion (including destructive shell commands such as `rm -rf` and `git push --force`), and account / security
  changes (passwords, 2FA, recovery settings, account deletion — browser actions on those pages);
- on top of that, the planning model may mark action types irreversible for a request (e.g. submitting an
  application), and Jev's `action.risk` = high (once enabled) makes FERB ask. They can only add to the floor, never
  remove anything from it.

**Routes found while exploring** (owner decision, 2026-09-30). An action outside the plan's apps / sites is a route
change: the worker says in plain words what it found and what it wants to do (its `why`), FERB adds what it would do
from code, and asks: "Your Brave had no call button. The WhatsApp app does. Call Rahul from there? (FERB would start
a call in WhatsApp.) [y/N]" (a notification when chat is closed; unattended runs wait 1 hour, then skip). The task
continues after a yes, and that app / site joins the request's plan for the rest of the request (`plan.extended` is
journaled; the owner's yes is the only thing that can extend a recorded intent). A no keeps the plan as it was. FERB
never offers what policy forbids anyway: first the checks that would block whatever the owner said — stop, identity,
exposure, an action type the request never asked for, owner data going to a destination the request as recorded
doesn't involve (21.9) — and those are simply blocked.

**Calls and messages (build step 16).** A call or message the owner asked for is not a question: like a send, it is
held for the undo window with a countdown ("Calling Rahul… undo") and then goes. Clicking through the app's own
buttons on the way (opening a chat, picking a contact) is part of a call / message request (`submit` within the
named sites / apps); the call or send itself is its own kind, contact-checked and held. One the owner didn't ask for follows
the same rules as an unrequested send.

**Contacts are never guessed** (build step 16a, owner decisions 2026-09-30), in code (`policy/contacts.py`, the
gateway). A call — or a message sent in an app or web chat when the owner named a person, not an address — must say
who it's for: `contact` = name (as the owner said it; must be a recipient in the request's intent), selected (the
contact's name as shown) and detail (a number, username or subtitle as shown). The gateway keeps what this task's
snapshots of that app / tab showed (the last 10 each, in memory only) and checks:
- the selected contact and its detail were on screen (typed text, e.g. the search box, doesn't count) — otherwise
  refused;
- if one screen showed more than one different entry matching the name, and nothing in the owner's words picks one
  (this request, their messages over the last day, and what they explicitly told FERB to remember about that name,
  e.g. "Rahul from work", "Rahul always means Rahul Sharma"), FERB asks: "Which Rahul did you mean? 1) … 2) …"
  (a notification when chat is closed; unattended runs wait 1 hour, then skip). A pick other than the worker's
  selection is refused with the owner's choice, and the retry of that one action uses it;
- the answer applies to that one action only: nothing is stored. A lasting mapping exists only when the owner states
  it as a memory;
- the undo countdown names the exact contact: "📞 Calling Rahul Verma (+91 98…) in 10 s"; `contact.selected` is
  journaled.
- screenshot clicks: in messaging / calling apps (the catalogue's WhatsApp, Slack, Zoom, Teams, FaceTime, Messages,
  Telegram, Discord, Signal, Skype) `mac.click` must declare `purpose` — call / send (with `contact`: the same check
  and countdown) or other. An undeclared click there is refused before any policy step; an "other" click on a point
  Accessibility identifies as a call / send control is refused. A screenshot of such an app also records the
  window's Accessibility text as evidence of who was on screen (FERB's checks only, never sent to the model).

### 10.4 Autonomy levels

- `careful` / `balanced` (default) / `hands-off`, set with `ferb autonomy`. The hard floor (payments, permanent
  deletion) applies at every level.
- Learned trust: when traces show the owner approved the same kind of action repeatedly, FERB offers to stop
  asking for that kind. Stored as an owner-stated preference; revocable.
- Unverifiable accounts (9.3): approving the "can't verify the account on <site> in <profile>" question once is
  remembered for that site + profile (config `[policy] trusted_accounts`, owner-only, revocable).

### 10.5 Emergency stop and owner priority

- `ferb stop` immediately revokes all run tokens, blocks new actions at the
  gateway, cancels workers, detaches debuggers and kills task-owned processes. `ferb resume-tasks` lifts it.
  The stop survives a daemon crash/restart (a `STOPPED` marker in `~/.ferb`) until the owner lifts it.
  (`ferb pause` is separate: it only stops monitoring.)
- Owner input wins: the `owner_input` watcher is a listen-only event tap; every input event FERB posts carries a
  private marker, so unmarked events are the owner (the system idle timer can't be used — FERB's own input resets
  it). GUI actions are made of small steps (one keystroke / one mouse event) and check before each; they yield
  within one step and resume only after a quiet period (default 1.5 s). A yielded action reports where it stopped.
- While FERB has an app forward (9.4 step 3), a second, filtering tap is on: the owner's first key press or click is
  held back (never delivered to FERB's app), FERB puts the owner's app back at once, waits until they've been quiet
  for the quiet period, brings its app forward again and carries on. Mouse movement alone isn't held back; it just
  makes FERB step aside. The filtering tap is off outside those windows. Background input (9.4 steps 1–2) doesn't
  pause for the owner — it never reaches what they're typing in.

### 10.6 Sanitization pipeline (`sanitize/redact.py`)

Runs before anything reaches a model, trace, log, exception message, journal, memory or export:
password/OTP field values, secrets, auth tokens in URLs, exclusion-list apps/domains, shell output matching
secret patterns, debugger events. Key-name redaction exempts FERB's own internal id fields (`task_id`,
`span_id`, `session_id`, `run_token_id` — ids, never token values); their values are still scanned for secret
patterns. Screenshots are never stored (saving one for a task that needs it is future work) and are not captured
for apps on `[privacy] exclude_apps` or when the window's Accessibility tree shows a password field (build step 9).
Runtimes keep no transcript on disk (Codex `--ephemeral`, Claude `--no-session-persistence`).

## 11. Monitoring trail

- Watchers: app focus (Accessibility's focused application), window titles (Accessibility; browsers left to the
  tab watcher, where domain exclusions can apply), browser tabs (extension `tab_event`s: page loads and tab
  switches; for Safari, and Chrome / Brave with no connected profile, an AppleScript read of the front tab while
  that browser is frontmost, only if the owner already granted Automation — never asked for), file events
  (FSEvents, only in `[sense] watch_folders`), idle / active transitions, owner input, app installs.
- **Metadata only:** app names, window titles, tab host / masked URL / title, file path + kind of change, idle state.
  Never keystrokes, clipboard, file contents or screen content.
- Order of processing for every event (fixed):
  1. deterministic exclusions + redaction (`sense/exclusions.py`, `sanitize/`) — an excluded event is dropped
     completely (no row, no bus event, no tag, no count; private / incognito tabs likewise);
  2. assign `id`, `actor` (owner / ferb / unknown; FERB's own tabs and actions are tagged with their `task_id`).
     FERB reports its own acts as they happen: the Mac driver's app switches (and putting the owner's app back),
     paths its file tools touch, tabs it opened, everything in its identity profile. File changes under a running
     FERB shell command's folder are `unknown`. The table refuses a row without an actor;
  3. persist to the append-only `trail` table and publish `trail.event`;
  4. only then: async Jev tagging (`trail.relevance`, `trail.sensitive`). A `sensitive` verdict triggers
     *additional* redaction of the stored row; it is never the first line of protection.
- `actor == "ferb"` events are excluded from habit-based resolution and from owner-preference distillation.
- `ferb pause` stops all watchers, the owner-input tap included; nothing is recorded, what FERB knew about the owner's
  open tabs is dropped (only tabs FERB owns stay tracked), and real mouse / keyboard input is refused (FERB can't
  yield without the tap; Accessibility actions still work) until `ferb resume`. The pause survives restarts.
  Screenshots: on demand, by a task with `screen` scope; not persisted.
- Rows are append-only (a trigger refuses changes to what / when / who; only data, relevance and sensitive may be
  updated by later tagging).
- **Retention:** 30 days of raw trail (`[sense] trail_retention_days`) plus a permanent daily summary per day in the
  journal (section 14), so the full journey is kept. The purge deletes a day only once its summary exists and
  refuses any day not yet summarized; summaries are written from build step 11, so nothing is purged before then.
- Consumers: resolver (site → browser + profile), distillation, `trail.query` capability.

## 12. Jev placement

All via `Decider`. `ShadowDecider(primary=Rule, shadow=Jev)`: FERB's existing logic decides; Jev's answer is logged
next to it (`decisions` table: question, baseline, Jev's answer, confidence, latency, error, mode, effect; a reference
to what was decided, never the payload). **Jev never approves**: it's only asked after FERB's own checks pass, and an
enabled question acts only on a stricter answer (the Effect column); anything else is ignored.

- **Jev API:** `POST https://api.typesafe.ai/v1/systemone`, `Authorization: Bearer <key>`, body `{model, state,
  questions}` with typed questions (noul = yes/no probability, choice, score); one typed answer per question id, plus
  usage. The model is pinned (`jev-1.13.0`, never `jev-latest`: calibration differs between versions); questions about
  the same state go in one request; 429 / 529 retry with exponential backoff.
- **Key:** Keychain only (service `ferb`, account `jev/api_key`), stored by `ferb setup --jev` through `security`'s
  own prompt; never in config, chat, logs or the database. Without a key, Jev is off: no calls, no rows.
- **Shadow mode** (every question until the owner enables it): calls go to a bounded background queue and never
  delay a task; `trail.*` questions are sampled 1 in 10 (Jev is billed per call); everything once enabled.
- **Enabling** (`ferb decide enable <question>`): where FERB has real logic (matches_request, risk, loop.control,
  task.route), only after 200 answered samples at ≥ 95% agreement; where it has none, `ferb decide stats` shows Jev's
  flag rate with a few redacted examples (read from the source, which may be gone) and the owner decides.
  `ferb decide disable` returns it to shadow mode.
- **Unavailable:** down, slow (enabled questions wait ~2 s at most), rate-limited past the retries, wrong-shaped, no
  key — FERB's own decision stands, the reason is logged, nothing fails.
- **Data:** callers build Jev's state from data that already passed exclusions and redaction (trail rows only at
  pipeline step 4); it is redacted once more before sending.

| Question | Type | Asked | Baseline | Effect when enabled (stricter only) |
|----------|------|-------|----------|-------------------------------------|
| `action.matches_request` | yes / no / unsure | gateway, after FERB's checks pass | yes | no → block; unsure → ask |
| `action.risk` | low / medium / high | same request | capability risk | raised to high → ask |
| `content.injection_suspected` | yes / no | untrusted tool results | no | yes → worker exposed |
| `browser.login_state` | signed_in / signed_out / unknown | owner-browser snapshots | unknown | signed_out → stop, tell the owner |
| `browser.profile_match` | one listed profile / none | only after deterministic matching fails | none | a suggestion the owner confirms |
| `loop.control` | continue / retry / escalate / stop | every 25 tool calls of a worker | continue | stop / escalate → worker stopped |
| `memory.worth_keeping` | yes / no | before distillation proposes | yes | no → dropped |
| `trail.relevance` | ignore / note / summarize | trail step 4 | note | stored on the row only |
| `trail.sensitive` | yes / no | trail step 4 | no | yes → extra redaction of the row |
| `task.route` | browser / mac / shell / mixed | plan time | from scopes | logged only |

Earlier sketch of the questions (kept for reference):

| Question | Type |
|----------|------|
| `trail.relevance` | ignore / note / summarize |
| `trail.sensitive` | yes / no — extra redaction after write; exclusions already ran before write |
| `task.route` | browser / mac / shell / mixed |
| `browser.login_state` | signed_in / signed_out / unknown (from snapshot) |
| `browser.profile_match` | suggest one candidate / none — owner confirms; only after deterministic matching fails |
| `action.matches_request` | yes / no / unsure — can only block or escalate, never approve on its own |
| `content.injection_suspected` | yes / no — marks the reading worker exposed |
| `loop.control` | continue / retry / escalate / stop |
| `memory.worth_keeping` | yes / no + kind |
| `action.risk` | low / medium / high — **may only raise** the level set by policy rules |

## 13. Time

**Time awareness (owner rules, 2026-09-30, build step 15).**
- Only absolute timestamps are stored. Relative ages are computed fresh each time a prompt is built
  (`clock/context.py`), never stored in history, and shown together with the absolute time ("3 days ago (Sat 26 Sep,
  14:02)") for recalled memories, recent conversation and tasks, sends and trail items.
- After a gap of more than 1 hour since the owner last talked to FERB, the prompt gets a short "since we last talked"
  summary: a few lines, notable items only (tasks that finished, replies to email FERB sent, reminders that fired).
- Idle time and time of day are given to FERB as facts. Behaviours that depend on them are code, not prompts: while
  the owner is idle, non-urgent notifications (a finished task's answer) are held until they're back; reminders and
  questions that need them are sent at once.
- **Prompts stay bounded and cache-friendly:** stable instructions and the tool list come first, then the bounded
  recent window (tagged items with absolute times) and the owner's message; everything time-dependent — the time
  header, relative ages of the tagged items, the "since we last talked" lines — goes at the very end.

- Header on every model turn (owner decision: per turn, not per API request): now (weekday + tz), task start + elapsed, deadline + remaining, owner last active, last trail event age.
- Footer on every tool result: `[now 15:42:10 IST | task +11m | deadline in 18m]`.
- Measured durations: median/p90 for similar past tasks from spans, when available.
- Timers: `set_timer`, `remind_at`, `sleep_until`; SQLite-backed; survive restarts and sleep (overdue timers fire with `overdue_by`).
  Build step 14 specifics:
  - Due times are checked against the wall clock at least every 30 s (the monotonic clock stops while the Mac sleeps).
  - Exactly once (safety test 19): a due timer is claimed, delivered, then acknowledged; a claimed-but-unacknowledged
    timer is delivered again after a crash, and its consumer skips a timer id it already handled.
  - Reminders come from the owner's own words in chat (the supervisor returns them with a quote that code checks
    against the owner's own words; the time must be in the future and within a year) or from a task's
    `time.set_timer` / `time.remind_at`. Delivery: a macOS notification (osascript; the OS's, not FERB UI), `ferb chat`
    (live while open, otherwise shown the next time it opens), `ferb logs`, and the journal. `ferb reminders`
    lists / cancels them.
  - `time.sleep_until` pauses a worker within the task's time limit (not a stall); a restart interrupts the task and
    cancels its sleep.
  - A timer never starts work on its own; scheduled and recurring tasks are build step 15.
  Delivery: due → claimed (row lock) → delivered → acknowledged by its consumer. A claimed-but-unacknowledged timer
  is re-delivered after restart with the same timer id; consumers ignore ids they already handled.
- **Code computes dates, never the model** (owner decision, 2026-09-30). For reminders (chat and `time.remind_at`),
  schedules' first runs and `time.sleep_until`, the model outputs only what the owner said — `when`: in_minutes,
  in_days, day (today / tomorrow / day_after_tomorrow / end_of_month / a weekday / `next_<weekday>` = that day of
  next week), date (`MM-DD` or `YYYY-MM-DD`), time (`HH:MM` local) — and `clock/when.py` computes the absolute time
  from the clock in the Mac's time zone: a span ("in 2 hours") is exact; a wall time ("tomorrow at 9") stays 09:00
  across a daylight-saving change; a time skipped by the change moves forward by the gap; a day without a time is
  09:00; a weekday or time already gone today means the next one. The confirmation line shows the computed date and
  time ("⏰ reminder … set for Fri 09 Oct 17:00").
- **Fuzzy time words have no fixed rule** (owner decision, 2026-09-30): "next Friday", "morning", "evening", "end of
  the week", "this weekend" — the model makes its best guess from context (including the day and time it's said) and
  chooses the fields; FERB always shows the computed date. When the owner corrects one, their meaning is stored as an
  owner-stated preference (`time:<phrase>`), listed in the planner's facts and used from then on, and the wrong
  reminder is cancelled and replaced (a schedule: cancelled and scheduled again).
- Elapsed time, timeouts, heartbeats and guards use `Clock.monotonic()`; timestamps and schedules use `Clock.now()`.
- `FakeClock` in all tests.

## 14. Memory

**Source of truth is SQLite. Markdown is a generated, read-only view.**

| Layer | Store | Written by | Purpose |
|-------|-------|-----------|---------|
| Spans / tasks | SQLite | gateway, orchestrator | What FERB did |
| Trail | SQLite | watchers | What the owner did |
| Journal | SQLite (append-only) | daemon | Chats, outcomes, approvals, daily trail summaries (permanent) — from first run |
| Memory items | SQLite `memory_items` | `distill` proposals → `commit` | Distilled facts/preferences/people/projects/env with provenance and validity |
| Markdown | `~/.ferb/memory/` folder | `export` only | Human-readable view, regenerated from SQLite (no git history kept) |

Rules:
- **Trail retention (owner decision):** raw trail rows are kept 30 days; each day gets a permanent summary in the
  journal (`trail.day_summary`, written by distillation from build step 11), so the whole journey survives. The
  purge refuses any day without its summary. `forget` still purges a summary's sources when asked.
- Every item links to its sources (`source_ids`). Updating a fact never overwrites: it sets `valid_to` on the old item
  and creates a new one with `supersedes`. Recall defaults to items valid now; `at=` gives historical recall.
- Retrieval: SQLite FTS5 keyword search first; embeddings later behind `recall()` without interface change.
- **Trust and activation:** `owner_stated` items activate on commit. `observed` items need repeated evidence from
  `actor == "owner"` trail events. `inferred` items stay proposed until the owner confirms. `third_party` content
  (web pages, emails, messages) can become a fact *about the world* but never an owner preference or instruction.
- Normal recall returns only `active` items valid now. Conflicting active items are both returned with sources
  and flagged; the newer owner-stated one wins for decisions.
- Owner edits via `ferb memory edit|retract|forget` → SQLite → re-export. Hand-editing markdown is not supported in Block 1.
- **`forget` means purge, not hide:** delete the item, every derived item, and the source records that produced it
  (journal entries, trail rows) unless the owner keeps them; rebuild the FTS index; SQLite `secure_delete` on;
  regenerate the markdown export (no history). Backups outside FERB are out of scope and stated as such.
- Retraction (`retract`) is the soft version: kept for history, never recalled.
- `ferb timeline [--since …]` renders the journal; `ferb memory show <subject>` renders items with sources.

Build step 11 specifics (owner decisions):
- **Origins in code.** `owner_stated`: `ferb memory add` / `edit`, or chat capture — the supervisor returns what the
  owner stated with the owner's exact words, and code keeps it only if that quote is in the owner's own words (not
  pasted text); active at once and supersedes any earlier version. `observed`: sources must all be trail rows with
  actor owner; active once they span 3 separate days. `inferred`: proposed until `ferb memory confirm`.
  `third_party`: never a preference, never about the owner. Nothing about the owner or their preferences may rest on
  FERB's own activity (trail rows with actor ferb, FERB's task outcomes) — safety test 18.
- **Versions.** kind + subject + key name one fact; a newer version closes the old (`valid_to`) and links it
  (`supersedes`). Only the owner's statement supersedes something the owner stated; otherwise both stay active and
  recall flags the conflict, the owner-stated one first.
- **Daily job (hourly catch-up, no budget).** For each past day not done: (1) the summary (`trail.day_summary`), built
  by code from the day's trail and journal — always written, needs no model; (2) one model run on the worker runtime
  from config (no tools, typed output): a narrative (`trail.day_narrative`) and proposals citing evidence handles
  (E owner activity, C owner's words, P pasted third-party text, F FERB's own activity), checked in code before
  `propose`; a failure is retried on later runs (3 attempts per day), the summary stays; (3) the purge, which only
  deletes days that have their summary. Measured on a fixture day: Codex ≈ 10k input + 150 output tokens per day,
  Claude ≈ 2k (mostly cached) + 500.
- **Forget.** Lists what it will delete and asks. Deletes the item, all its versions, items built on the same
  sources, the sources (journal entries, trail rows) unless `--keep-sources`; days whose rows went get their summary
  rebuilt from what remains (the narrative is dropped), or deleted if no rows remain; a summary that was itself a
  source is deleted. Then the FTS index is rebuilt, the WAL checkpointed (truncate), the export regenerated. A
  one-way fingerprint (hash of kind, subject, key, content) stops distillation proposing it again; the owner may
  state it again.
- Recall for a supervisor turn: up to 8 active items matching the owner's own words, labeled by origin.

## 15. CLI

| Command | Purpose |
|---------|---------|
| `ferb chat [--verbose] [--speak]` | Conversation with FERB: a line editor (paste as one message, Option-Enter for a new line), approvals inline, results of tasks that finished while chat was closed; Ctrl-C stops the running task, closing chat doesn't; `--verbose` shows plans, workers and tools; `--speak`: FERB also says its own words out loud (the Mac's voice, `say`; behind an interface for a later TTS API) |
| `ferb logs [--tasks --trail --tools --browsers --policy --actions --timers --voice]` | Live stream from the bus (redacted) |
| `ferb voice [on\|off]` | No argument: push-to-talk as a live conversation (what FERB heard, what it said back). `on` / `off`: start or stop push-to-talk without restarting FERB — off also stops FERB speaking and quits FERB.app (20.1) |
| `ferb trace [task] [--full]` | What a task did, step by step: each step's goal and result, every tool call with what it opened / typed / ran and what came back (terminal output as plain text). Default: the latest task; a short id works (owner's request 2026-10-01) |
| `ferb setup [--update] [--codex-login] [--voice]` / `ferb doctor` | Onboarding (`--update`: reinstall FERB into the venv + restart the agent; `--codex-login`: sign in FERB's own Codex home; `--voice`: build, sign and enable FERB.app for push-to-talk) / health, permissions, extension connectivity, push-to-talk |
| `ferb uninstall [--purge]` | Remove the LaunchAgent (and native-host files); `--purge` also deletes `~/.ferb` after confirmation |
| `ferb device [scan]` / `ferb browsers` | Device profile (apps, scriptable apps, browsers with live pairing status; `scan` rescans) / browser drivers + connection status |
| `ferb browsers pair\|unpair [--browser chrome\|brave] <profile>` | Pair a profile (installing the extension there first, guided, if it's missing — 9.3) / forget its pairing |
| `ferb reminders [cancel <id>]` | Pending reminders and timers; cancel one |
| `ferb timeline`, `ferb memory list [--proposed\|--all] \| show \| add \| confirm \| edit \| retract \| forget` | Memory journey |
| `ferb pause` / `ferb resume` | Stop / start monitoring |
| `ferb decide stats \| enable <q> \| disable <q>` / `ferb setup --jev` | Jev's answers next to FERB's (shadow mode); enabling a question; the Jev key into the Keychain |
| `ferb stop` / `ferb resume-tasks` | Emergency stop of all actions / lift it |
| `ferb undo` | Cancel a pending send/post during its undo window (section 10.2) |
| `ferb autonomy [careful\|balanced\|hands-off]` | Show / set autonomy level |
| `ferb ps` / `ferb kill <id>` | Background processes |

## 16. Adding a feature (checklist)

1. New file in the right module implementing the matching Protocol.
2. Declare `scopes`, `identities`, `risk`, `reversible`.
3. Add one line to `registry.py`.
4. New events → add to section 6. New extension messages → add to 9.4 and bump protocol version on both sides.
5. Test with fakes (`FakeClock`, `FakeBrowserDriver`, `FakeRuntime`) + one eval task.
6. Consequential? Declare how its intent is described to `policy/intent.py` and whether it writes an `ActionIntent`.
7. Output passes through `sanitize/` (automatic via gateway — do not bypass).
8. Add or extend a safety test in section 21 if it touches accounts, secrets, files outside the workspace, or shell.
9. Touches secrets or crosses identities → stop and ask the owner.

## 17. Day-1 spikes (before building modules)

1. `codex exec --json` calling a stdio MCP tool non-interactively without full sandbox bypass, **with Codex's own
   shell and file tools disabled**; prove a shell attempt fails.
2. Claude Agent SDK session with the `ferb mcp` shim as the only tool source: built-in tools, subagents, plugins and
   inherited settings disabled; prove native Bash/Read/Write/WebFetch attempts fail. Confirm auth path.
3. *Superseded by spike 16.* CDP over `--remote-debugging-pipe` to `ferb-identity`; login + `fill_secret` on a
   real site. (A copied profile lost its Google session; spike 16 kept it 35 min via the extension in the original
   "FERB" profile.)
4. Chromium extension in Brave and Chrome: native messaging round trip; background tab open; snapshot;
   `chrome.debugger` trusted click + typing **in a background tab** on a site with bot protection.
5. *Skipped — Safari control deferred to a later block.* Safari extension in FERB.app: build, enable (unsigned-extension developer setting for local dev), DOM snapshot,
   script click on a real site; AX + real input fallback on the same site.
6. pyobjc: frontmost app, window title, AX tree of Chrome with and without `AXManualAccessibility`; TCC status checks.
7. *Skipped — Safari control deferred to a later block.* Safari extension handler → daemon socket from inside the app sandbox (may require the socket in a shared app-group container).
8. launchd agent start/stop + socket from CLI.
9. *Run 2026-10-01 (owner's voice block).* System audio (timeboxed, ~1 hour, throwaway script — not FERB code): with the `catap` Python package, record 10 s
   of one app (browser playing YouTube) and of the whole system mix, on speakers, wired headphones and AirPods.
   Confirm the Audio Capture prompt appears, that denial yields silence (so `doctor` must check for real signal),
   and whether DRM playback (Apple Music, Netflix in Safari) comes through silent. Record results in section 20.
10. Profiles: parse Chrome + Brave `Local State`; open a named profile via `--profile-directory` with the browser
    already running; External Extensions file → confirm whether every profile shows the enable prompt, and whether
    Brave honors the same mechanism (and its folder path).
11. *Skipped — Safari control deferred to a later block.* Safari profiles: list them; confirm the extension is enabled per Safari profile; pairing works per profile.
12. Pairing flow end to end in two Chrome profiles + one Brave profile; unknown `install_id` blocked from acting.
13. MV3 service worker termination: kill it mid-command; confirm reconnect, generation bump, no replay.
14. Owner-input detection latency (how fast GUI actions yield when the owner touches the mouse).
15. TCC grant on a stable launcher vs the interpreter (build step 3).
16. FERB identity through the extension in its original Chrome profile: Google session survives 30+ min of use.
17. Voice input (owner-added 2026-10-01; throwaway): a minimal background app bundle (no windows, Dock icon or menu-bar
    item; `LSUIElement`) built with `swiftc` and ad-hoc signed — the microphone prompt and capture; on-device speech
    recognition of a spoken sentence (latency, accuracy, works offline?); the push-to-talk key from the daemon's
    existing event tap (hold Right Option to talk, release to send; taps under ~0.3 s ignored; another key pressed while
    it's down = a shortcut, the recording discarded); and whether permissions survive a rebuild ad-hoc signed vs signed
    with a local self-signed certificate (created only if needed — owner approved).
18. Background input (owner-added 2026-10-01): can FERB act in an app that stays in the background — (a) Accessibility:
    set a text box's value and press buttons; (b) keystrokes posted to the app's process (`CGEventPostToPid`) — tested
    in WhatsApp (draft-only: the New Chat search box, never a chat, never Send) and TextEdit.
19. Waking a browser profile without moving the owner (owner-added 2026-10-01): an event-based detector (macOS's
    active-Space-changed and app-activated notifications, timestamped) validated against the old wake; then one
    variant per run with the owner watching — "FERB Test" first, then (owner's permission) their own profile and
    FERB's.

## 18. Build order inside Block 1

1. `core`, `store`, `bus`, `clock` (wall + monotonic), daemon + socket + `auth`, **minimal journal**,
   `ferb logs`, `ferb trace`.
2. `sanitize/`, `policy/` (rules, intent, exposure, dataflow, autonomy, approvals), ledger, `ferb stop`.
3. `device` (setup, permissions, profile) + `ferb setup/doctor`. Measure first whether a small stable launcher
   executable in `~/.ferb/bin` (spawning Python as a child) would keep TCC grants across interpreter upgrades,
   including how ad-hoc signing affects that; record the result (section 8, step 6 stays the plan).
4. `runtimes` with native tools disabled + `ferb mcp` shim + gateway; `act/fs`; `ferb chat` with reply/clarify
   and a single worker. Safety tests for gateway bypass pass before continuing.
5. `identity/` layer: Keychain secrets, mailbox/OTP, identity profile verification, `browser.*` tools and their
   policy kinds (driver-independent).
6. Built and reported as **6a** then **6b** (owner decision; plan approved before each).
   Chromium extension (page mode, sanitized snapshots, reliability semantics; then debugger mode + allowlist)
   + native host + `ext_bridge` + profile discovery/resolver + pairing + site resolver, **including the FERB
   identity profile** (paired as FERB; `fill_secret`/`fill_otp` along the extension path). Step 5's real tests
   (session, OTP, 21.4) run here against the extension.
7. `act/mac` + `gui_lock` + resource locks + owner-input yield; `act/shell`; `ax_browser`.
8. Orchestrator multi-worker (DAG, reader/actor, runaway guards, verification, reconciliation).
9. Screenshot + click (vision) for apps with poor Accessibility support: on-demand capture of the app's front window
   only (`act/mac/screenshot.py`, ScreenCaptureKit's single-window filter: never the rest of the screen, never windows
   on top), image results that reach the model and nothing else (not spans, journal, logs or files; saving a
   screenshot is out of this step), clicks by image coordinates through the same marked, yielding real input. The
   only refusals: apps on `[privacy] exclude_apps`, and a window whose Accessibility tree shows a password field
   (checked before and after capture; the frame is dropped). A browser's web tree is built on demand, so the check
   waits up to ~2 s for web content; for Chrome / Brave the extension's `window_password` also answers for the tab on
   screen (the profile whose window has those exact bounds). When nothing can tell, the capture goes ahead.
   Before each click event: the window hasn't moved and the system's hit test (Accessibility) says the click reaches
   it; if Accessibility can't answer, the topmost normal window there must be it.
10. `sense` watchers + trail (fixed processing order, actor attribution) + exclusions.
11. `memory` (items with trust levels, distill, recall, purge-forget, export) + timeline.
12. Deferred: Safari control and FERB.app (later block).
13. `decide` with Jev in shadow.
14. Time and small gaps (owner-added, 2026-09-30): durable timers — `set_timer`, `remind_at`, `sleep_until`,
    SQLite-backed, surviving restarts and sleep (overdue timers fire with `overdue_by`), delivered exactly once
    (safety test 19); `clock/durations.py` (measured task durations in the time header); sending from FERB's mailbox
    (normal autonomy: in the request → the 10 s undo window; a recipient the owner didn't name → asks; a send the
    request didn't include → blocked, owner decision); `ferb device`; the §0 table fix.
14a. UX fixes (owner-added after testing, 2026-09-30; built before step 15): invalid plans repaired silently (never an
    internal error in chat); readers may return analysis and FERB writes the final answer; `ferb chat` with a real
    line editor (bracketed paste, multi-line); continuity from the journal (recent turns, running / recent /
    interrupted tasks); names the owner uses (apps, sites) count as intent targets; approval scope + the irreversible
    floor (10.3); a tool call waits for the owner's answer and continues; windows on other Spaces / full screen;
    installed-but-unpaired extensions are reported; FERB speaks as FERB, without internal jargon (`--verbose` shows
    it). Ctrl-C stops the running task ("stopped by you"); closing chat never stops tasks — a task that finishes while
    chat is closed sends a macOS notification and chat shows its result next time.
15. Scheduled and recurring tasks (owner-added, 2026-09-30): "every morning at 9, summarize my unread email", "check this
    price tomorrow and email me". The intent is recorded from the owner's words when the task is scheduled and each run
    is held to that intent (a run's plan may only stay within it); `ferb schedules` lists, cancels, pauses, resumes.
    Once, daily, weekdays, weekly, or every N minutes (N >= 15). Unattended approvals: notify, wait up to 1 hour, then
    skip that step and report it. Missed runs: one run when the Mac wakes, marked late; skipped if later than half the
    repeat interval (at most 12 hours); one-off schedules run up to 12 hours late. Three failures in a row pause a
    schedule. Also in this step: progressive setup (§8) and time awareness (§13).
16. Environment-aware resolution (owner-added, 2026-09-30; plan shown before building): when a task names a service or
    app ("video call X on WhatsApp"), FERB picks how to reach it without asking — (1) how the owner actually uses it,
    from the trail; (2) an installed native app, from the device profile; (3) the website in a paired browser profile;
    (4) ask only if none works. The planner gets a short environment summary each turn (macOS, relevant installed and
    running apps, paired browser profiles). A correction ("use the web version") is stored as an owner-stated
    preference and followed from then on. The chosen route is shown in plain words when starting. Recorded in §9.3
    and §7. Calls/messages the owner asked for run with an undo countdown.
16a. Contacts in code (owner-added, 2026-09-30): a call / message to a person names its contact (name as the owner
    said it, selected + detail as the screen shows them); the gateway checks both were on screen in this task; several
    matching entries with nothing in the owner's words to pick one → a numbered "Which Rahul?"; the answer is for that
    one action; the countdown names the exact contact. No automatic name → contact mappings (section 10.3).
    Screenshot clicks in messaging / calling apps must declare their purpose (call / send / other).

**Gate:** owner-account actions (sending, posting, editing in the owner's accounts) are enabled only after the
section 21 safety tests pass.

## 19. Known gaps (not solved in Block 1)

- For later (owner note, 2026-09-30; not built): read-only planner tools for large, trusted data the planner needs
  only sometimes (e.g. "is app X installed?" against the device profile). Never give the planner direct access to
  trail data: titles there are set by websites.

- Prompt injection: reduced by intent matching, exposure tracking, reader/actor split and data-flow limits
  (section 10.2), not eliminated. Typed reader outputs narrow but do not close the channel.
- Per-resource grants with process isolation for arbitrary code (needed for multi-user) — later.
- Storage failure behavior: bus queues are bounded; telemetry may be dropped under pressure, but ledger, approval
  and journal writes are never dropped — if they cannot be written, the gateway refuses the action.
  Disk-full handling beyond that is later.
- Safari is watch-only in Block 1; acting, the Safari extension and its FERB.app handler come in a later block.
- Keychain item access is not yet restricted to FERB's own signed binary; that comes with FERB.app.
- The extension is a high-privilege component: pin extension IDs, no remote code, review every protocol change.
- Captchas/bot checks: real browsers + trusted input reduce but do not eliminate them; FERB stops and asks when blocked.
- Crash mid-task: the ledger marks in-flight consequential actions `unknown` and reconciles them on restart;
  full checkpoint/resume of tasks comes with the overrides block.
- Trail encryption (plaintext SQLite, rely on FileVault); provider outage modes.
- Tab events from the extension carry page loads and tab switches; a tab that changes only its title isn't recorded.
  Browser window titles are left out of the window watcher (their domains can't be checked).
- Hand-editable markdown memory (two-way sync) deferred.
- FERB never hides or fakes automation signals (it uses no remote debugging, so none is set by launch).
  `fill_secret` refuses, never types, when a site rejects a directly set value.
- FERB's identity needs the owner's Chrome running with the FERB profile loaded; FERB may open it (9.2).
- The FERB profile shares the owner's Chrome process: separate cookies and storage, not a separate browser
  process.
- The secret passes through more components than a CDP fill did (native host and extension); both are
  high-privilege, reviewed components.
- Mac control in Block 1 build step 7: AppleScript (scriptable apps, one app per script) and Accessibility, with
  real mouse/keyboard only as the fallback. Screenshot + click (vision) is build step 9: it needs image results
  to reach the model. The Accessibility browser fallback reads only; acting in a browser needs the extension.
- Shell output is untrusted only for commands that fetch from the network (curl, wget, ssh, git clone/pull, …,
  matched on each pipeline segment's command word): running or reading one exposes the worker. Other commands'
  output is trusted, so a program that fetches content some other way isn't caught; reader/actor splits (step 8)
  are the further mitigation.
- Data-flow labels (safety test 9) use substring matching: a value read from an owner account (≥ 8 characters)
  that reappears verbatim in an action's content carries its label. A paraphrase, summary, translation or
  reformatting of that data doesn't match and isn't caught; the reader/actor split and intent matching remain the
  other defenses.
- Screenshot + click has no built-in refusals beyond `[privacy] exclude_apps` and a password field the Accessibility
  tree shows (owner's choice): a window whose tree can't be read is captured, and browsers can be captured and
  clicked through the Mac layer. Clicking a browser this way skips the extension's profile and account checks
  (9.3); the policy engine still gates it as `mac_input`. A screenshot of a browser exposes the worker (web
  content), as does one of a mail or messaging app. Chrome and Brave expose no web content to Accessibility unless
  an assistive technology (e.g. VoiceOver) has enabled it (`AXManualAccessibility` / `AXEnhancedUserInterface` are
  refused), so a web page's password field is detected through the extension (`window_password`) in paired
  profiles; in unpaired profiles it's usually "can't tell" and the frame is captured. Safari exposes it to
  Accessibility.
- Trusted (debugger-dispatched) input works in a page's top frame only; elements inside iframes get page-mode
  (scripted) events, which some sites ignore. Extending trusted input to iframes needs frame-offset handling.
  Secret fills are unaffected: they set the value on the exact element in its own frame.
- Background input in owner browsers uses focus emulation, so the page believes it is visible during trusted-input
  actions (only while the debugger is attached for them). This is a deliberate trade-off, not an automation-signal
  change.
- The Codex worker runtime (`codex exec`) is a stopgap: it is pinned to `gpt-5.5` (the only non-code-mode model it
  can be locked down on) and carries ~25–40k tokens of Codex's own instructions per run. Planned replacement: a
  direct-API runtime (FERB's own agent loop) where the model sees only FERB's tools. When `gpt-5.5` is retired
  the allowlist check fails and the Codex runtime stays unregistered until a new lockdown recipe or the
  replacement exists.

### Future direction: brain + nodes (do not build in Block 1; do not block it)

When phone apps (iOS/Android) or other computers arrive, FERB splits into one **brain** and many **nodes**:
- **Brain:** orchestrator, policy, gateway, memory, traces, timers, Jev. Runs in the daemon on the Mac today;
  may move to a small server so it is reachable when the Mac sleeps (also the multi-user shape).
- **Nodes:** each app = chat UI + that device's drivers and watchers. Mac node = today's drivers, watchers, FERB.app.
  Android node (deep control via Accessibility service), iOS node (Shortcuts / App Intents, notifications).
  Nodes are hands and eyes only — same rule as extensions — and pair with the brain the same way (one-time token).
- **What changes then:** brain ↔ node transport becomes an authenticated, encrypted network connection; nodes
  announce their capabilities on connect, so the registry lists capabilities per `device_id`.
- **Rules for Block 1 code so this stays possible:** never assume a single device (always carry `device_id`);
  OS-specific code only under `act/mac/`, `sense/watchers/`, `device/`, `macapp/`; brain modules never import
  OS-specific modules directly — only interfaces (`ComputerDriver`, `BrowserDriver`, `Watcher`).

## 20. Reserved: system audio listening (built in the voice block)

Goal: FERB can hear what the Mac is playing — speakers, wired headphones or AirPods — to answer
"what is this song?" or "what is this video saying?". Not built yet (Unit 5, after push-to-talk); spike 9 proved the
capture (results below), and this section fixes its shape.

- **Capture:** Core Audio process taps (macOS 14.2+), per app or global mix, FERB's own process excluded.
  Taps capture the post-mix signal and follow the output route, so headphones need no special handling.
  No virtual audio drivers (BlackHole etc.). ScreenCaptureKit audio only as a fallback.
- **Where:** Swift service in `FERB.app` (`macapp/Audio/`), streaming PCM to the daemon over the socket.
  Handles tap recreation on output-device change, sleep/wake and source-app restarts.
- **Capabilities (planned):** `audio.listen(source: app | system, seconds, mode: song | speech | raw)`,
  `audio.recent(seconds)` from an optional RAM-only rolling buffer (30–60 s; `ferb audio on|off`; never written to disk).
- **Processing:** song ID via ShazamKit; speech via a transcription provider behind an interface
  (on-device Apple speech or cloud — decided in the voice block); transcript goes to the orchestrator.
- **Policy:** new `audio` scope; call apps (Zoom, FaceTime, Meet, etc.) excluded by default; trail records metadata only
  ("audio from Brave 14:02–14:09"), never content.
- **Setup/doctor:** Audio Capture permission added when built; `doctor` verifies a non-silent signal, not just tap creation.
- **Shared with voice:** the same audio service will carry microphone input; the system-audio tap is what lets FERB
  separate its own output and background media from the owner's voice.
- **Spike results (spikes 9 + 17, 2026-10-01; `spikes/17_voice_audio`):**
  - System audio: a global tap (FERB's own process excluded) and a per-app tap (Apple Music) both captured real signal
    through the owner's Bluetooth headphones; Apple Music's DRM playback came through (not silent). With nothing
    playing the tap delivers no frames at all (so "silent" = no frames, not zeros). The system-audio permission survived
    an ad-hoc rebuild. The MacBook speakers route was also captured (it's the digital mix, not sound through a mic, so
    it's clean whatever the output). AirPods: not tested (the owner's headphones were the route).
  - Microphone: `AVAudioEngine` delivered **no buffers** in a background (`LSUIElement`) app, with permission granted;
    a Core Audio IOProc on the input device works — FERB captures that way. The owner's Bluetooth headset mic gave
    near-silence (peak 0.0005) while the built-in mic gave real signal: prefer the built-in mic when the default input
    is a Bluetooth headset (configurable).
  - Speech: on-device `SFSpeechRecognizer` is available; a recorded sentence transcribed exactly in 0.23 s, offline.
    Live path (IOProc → buffer request, on-device), owner's voice, built-in mic: "… what's the weather in Delhi
    tomorrow" exact both times, text ready 0.02 s after release; the name "FERB" is misheard ("For", "Siri") — fine for
    push-to-talk (the key is the trigger), a wake word will need its own detector. Two traps found and fixed: the
    recognizer often returns an **empty final result** after `endAudio()` though its partial results had the words —
    keep the last partial when the final is empty; and load the speech model before listening (≈0.15 s, at launch in
    FERB.app). Never open a Bluetooth headset's mic (it drops the headset into low-quality call mode): choose the
    input by reading device properties only, built-in mic when the default input is Bluetooth (owner rule).
  - Talk key, from a listen-only event tap: hold Right Option → "send" (1 s hold), taps of 0.04–0.08 s ignored, a hold
    with C → "shortcut: discard"; each decision within 1 ms of the key event.
  - Signing: ad-hoc signed, a real rebuild (new CDHash) reset microphone + speech permissions to "not asked"; signed
    with a local self-signed certificate ("FERB Local Code Signing", login Keychain, created with the owner's approval)
    the designated requirement is the bundle id + certificate leaf, and permissions survived a rebuild with a new CDHash.
    FERB.app is signed with that certificate.
- **Spike 18 results (2026-10-01, background input):** with WhatsApp on another desktop and the owner's app in front
  the whole time (checked after every step):
  - Accessibility reads the window even when `AXWindows` is empty (WhatsApp on another Space lists none):
    `AXMainWindow` / `AXFocusedWindow` give it. Button presses work in the background (a tab, New Chat; the app's
    "Open main window" menu item reopened a closed window without coming forward).
  - (a) Setting a text box's value through Accessibility worked, and WhatsApp reacted (its search filtered to "No
    results found", seen in a capture of its window). (b) Keystrokes posted to its process worked the same way, after
    focusing the box through Accessibility. TextEdit: both worked too (its autocorrect capitalized the posted text).
  - The front app never changed, in either app.
  - Caution: Escape posted to WhatsApp closed its whole window, not the panel — a background key can do more than
    intended, so keys are checked by what changed, and closing things is done by pressing their button.
  - Not tested, on purpose: pressing WhatsApp's Send button (it would send a real message from the owner's account);
    it's an ordinary Accessibility press like the ones that worked.

- **Spike 19 results (2026-10-01, owner watching; detector and owner agreed on every run):**
  - A focus-only poll (every 50 ms) missed moves the owner saw — measured wrongly; replaced by the event detector.
  - The old wake (the browser's binary with a URL): Chrome activated, the owner moved. Detector recorded it.
  - (a) `open -g … --profile-directory=… --no-startup-window`, Chrome closed: nothing moved, no window, and the
    extension in that profile connected.
  - (b) the same for FERB's own profile while Chrome ran the owner's profile: nothing moved, FERB's extension
    connected ("FERB's own profile, connected").
  - Opening a window in any way (`open -g` with a URL, the extension's unfocused `windows.create`): Chrome came
    forward and the owner moved, Chrome closed or open. Matches the open Chromium bugs on macOS focus stealing.
  - Not yet run with the owner watching: the extension's first window created minimized (built, extension 0.6.6).

### 20.1 Push-to-talk (Unit 4, owner decisions 2026-10-01)

- **The key:** hold Right Option to talk, release to send. A hold under 0.3 s is ignored; another key pressed while it's
  down makes it a shortcut and the recording is discarded. Read from the daemon's existing listen-only event tap
  (`OwnerInput`; FERB-marked events never count). Pressing it also stops FERB speaking.
- **FERB.app** (`macapp/FERB`, built by `ferb setup --voice` into `~/.ferb/FERB.app`): a background app — no windows,
  Dock icon or menu-bar item (`LSUIElement`) — signed with the local "FERB Local Code Signing" certificate so
  Microphone and Speech Recognition permissions survive rebuilds. It is the only part that opens a microphone: on the
  daemon's "start" it opens the chosen input with a Core Audio IOProc and feeds on-device `SFSpeechRecognizer`
  (model loaded at launch; an empty final result falls back to the last partial; "FERB" as a contextual string); on
  "stop" it returns the text; on "discard" it drops everything. Audio is never written anywhere. A soft sound marks
  start and stop; macOS shows its own microphone indicator.
- **Which mic (owner rule):** never open a Bluetooth headset's microphone, not even to probe it (it drops the headset
  into low-quality call mode). The daemon chooses from the device list FERB.app reports — read from device properties
  only — and picks the built-in mic whenever the default input is Bluetooth (`[voice] mic = auto | builtin |
  default`); FERB.app refuses to open a Bluetooth input as a second guard.
- **Who may speak for the owner:** only the installed FERB.app. A `voice` connection is accepted only when the peer
  runs as the owner, its executable is `~/.ferb/FERB.app/Contents/MacOS/FERB`, and its code signature satisfies
  the requirement recorded at setup (bundle id + the local certificate's leaf). The voice role can only attach and send
  transcripts.
- **What's said:** the transcript is the owner's own words (they held the key), sent as a chat turn in the "voice"
  session — same journal, continuity and every policy rule as typed chat. Replies to voice requests are always spoken
  (`say` now, swappable), and also appear in `ferb logs`. While FERB is waiting on a question, a spoken "yes", "no" or
  a number answers the oldest one.
- **Sounds (owner decision 2026-10-01):** the start sound plays only once a hold passes 0.3 s (a tap or a Right Option
  shortcut makes no sound); `[voice] sounds = false` turns both sounds off. Turning push-to-talk off also stops FERB
  speaking replies to voice requests still running.
- **Setup / doctor:** `ferb setup --voice` builds, signs and installs FERB.app, records the signature requirement,
  asks for Microphone and Speech Recognition once, and turns `[voice] push_to_talk` on; the daemon opens FERB.app at
  start when push-to-talk is on. `ferb doctor` reports whether FERB.app is connected and its permissions.

## 21. Safety invariant tests (must pass before owner-account actions are enabled)

Each test proves a promise in this file; all run in CI with fakes, and the marked ones also on the real machine.

1. Gateway bypass (real, Claude and Codex): the tool list FERB offers equals the runtime's declared allowlist,
   and the model-visible list is within it (equal for Claude; a subset for Codex, which loads MCP tools on
   demand); native shell / file-write / web / subagent attempts fail; for a tool that cannot be removed (Codex
   `apply_patch`), a write attempt fails under the enforced sandbox; reading a file outside the workspace and
   fetching a URL both return nothing.
2. Revoked or expired run token cannot call any tool; `ferb stop` revokes all within one second (real).
3. `FERB` task cannot reach owner profiles, shell or AppleScript; `OWNER` task cannot reach the FERB profile or
   use `fill_secret`/mailbox.
4. `fill_secret` (through the extension) refuses when the target frame origin is not in `allowed_origins` (e.g.
   lookalike domain, iframe).
5. Secrets and password/OTP values never appear in snapshots, spans, logs, journal, memory or exports.
6. Extension refuses non-allowlisted CDP methods (`Network.getAllCookies`, `Runtime.evaluate`, `Target.*`).
7. Intent mismatch is blocked: summarize-only task tries to send / run shell / message a new recipient.
8. Exposed worker loses shell/AppleScript/file-write; injected page text cannot reach an actor worker.
9. Data-flow: owner email content cannot be typed into an unrelated site.
10. Approval scope: one approval covers the same target and action type within a request; a different recipient or
    amount is a new question; irreversible actions (the floor, plus any the model or Jev flag) ask every time.
11. Lost acknowledgement after Send → ledger `unknown` → reconciliation, no duplicate send.
12. Wrong profile / wrong account: action refused when the verified account differs from the resolved one.
13. Stale `ElementRef` after navigation or reconnect is rejected.
14. Competing workers on the same tab/file: resource locks serialize them; no deadlock.
15. Disconnect/reconnect of the extension: no replay of old-generation commands.
16. Unpaired extension cannot act or write trail events.
17. Forgotten memory is not regenerated by distillation (sources purged) and is absent from FTS and exports.
18. `actor == "ferb"` trail events never become owner preferences or resolver history (resolver rule 2 reads only
    `actor == "owner"` tab events; FERB's tabs and its identity profile are `ferb`).
19. Timer claimed but unacknowledged before a crash is delivered exactly once to its consumer.
20. The FERB identity profile is refused at pairing and flagged by `doctor` when its Chrome accounts are not
    exactly FERB's account; cookie and password stores are never read.
21. Along the extension path, a secret or one-time code value never appears in native-host or extension messages,
    logs or errors (it travels sealed, 10.1); a sealed value is refused under another command id or origin.
22. A screenshot contains only the target window (not the screen, not windows on top); apps on `[privacy]
    exclude_apps` and windows whose Accessibility tree shows a password field are never captured; the frame reaches
    the model and no span, journal, log or file; a click lands only when the system's hit test says it reaches the
    captured window, and yields to the owner like all real input.
23. Exclusions run before anything else: an excluded app, domain, path or private tab reaches no trail row, bus
    event or tag; every stored event has an actor; `ferb pause` stops every watcher and the owner-input tap and
    records nothing (surviving a restart); the purge never deletes a day without its journal summary.
24. Jev never approves: in shadow mode it has no effect; an enabled question acts only on a stricter answer, and
    it's never asked about an action FERB already blocked. Trail data reaches Jev only after exclusions and
    redaction. Jev unavailable (down, slow, rate-limited, wrong-shaped, no key) leaves FERB's own decision in place
    and fails no task. The key lives only in the Keychain.
