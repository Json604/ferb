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
| 2 | Browser control | Yes — FERB's own Chrome profile via CDP; owner's Chrome and Brave via the FERB browser extension, Accessibility fallback; Safari watch-only (AppleScript read of tabs and URLs) (section 9) |
| 3 | FERB identity | Yes — own email + password, Keychain-backed, used only in FERB's own profile, never visible to models |
| 4 | CLI | Yes — `ferb chat` (talk to orchestrator) and `ferb logs` (live log/trail viewer), plus admin commands |
| 5 | Monitoring | Yes — event trail of Mac + browser activity; no continuous LLM watching; screenshots only on demand |
| 6 | Time | Yes — time header per model call, timestamp footer on every tool result, durable timers |
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

1. **Important behavior lives in code, not prompts.** Orchestration, policy, fallback, budgets, secrets, identity, time — all code.
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
- **FERB.app** is reserved for later blocks and not built in Block 1: the future home of the Swift audio service
  (section 20), the stop hotkey, Keychain item access anchored to a signed app identity, and the Safari extension.
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
    worker.py           one task, scoped capabilities, budget, heartbeat
    tree.py             task tree persistence
    budgets.py          aggregate time / tokens / cost / steps per task tree; fan-out limits; cancellation
    ledger.py           action ledger: intent → executing → done | failed | unknown; reconciliation
    verify.py           supervisor-owned verification against the plan's success conditions
    stop.py             emergency stop: revoke tokens, block actions, cancel workers, kill owned processes
    prompts/            prompt templates (text only; no logic)
  runtimes/
    claude_sdk.py       ClaudeRuntime (Agent SDK)
    codex_exec.py       CodexRuntime (codex exec --json); supervisor and worker modes
    proxy.py            localhost tool-list proxy for the Claude runtime (records tool names only)
  decide/
    decider.py          RuleDecider, LLMDecider, JevDecider, ShadowDecider
    questions.py        the typed questions FERB asks (section 12)
  policy/
    rules.py            scopes, identity rules, hard floor, destructive-pattern lists
    intent.py           does this action match what the owner asked for? (section 10.2)
    exposure.py         per-worker exposure to untrusted content; capability removal (10.2)
    dataflow.py         owner data may only go to destinations the request involves (10.2)
    autonomy.py         careful / balanced / hands-off levels + learned trust from traces (10.4)
    approvals.py        single-use, expiring approvals bound to the exact action payload; undo windows
    engine.py           fixed evaluation order over the checks above -> ALLOW / HOLD / ASK / BLOCK with reason
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
      screenshot.py     on-demand window / screen capture
      snapshot.py       AX tree → shared element-list format (same as browser snapshots)
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
    gui_lock.py         single GUI actor at a time
  sense/
    watchers/
      app_focus.py      frontmost app changes (NSWorkspace)
      windows.py        focused window titles (Accessibility)
      browser_tabs.py   extension tab_events; AppleScript read for Safari and for unextended profiles
      files.py          FSEvents on configured folders
      idle.py           owner idle / active
      owner_input.py    owner is using mouse/keyboard right now → GUI work yields
      app_installs.py   app install / removal → device.changed
    exclusions.py       exclusion list applied before any trail write
    trail.py            append-only activity trail writer
  device/
    scan.py             setup + rescan: apps, scriptability, AX support, browsers, extension status
    permissions.py      TCC permission checks + open-settings helpers
    profile.py          DeviceProfile load/save/diff; method resolution
    install.py          dedicated interpreter + venv (FERB installed as a copy), LaunchAgent install/update/uninstall
    selftest.py         self-test run inside the daemon: AX read, event tap, one marked (harmless) input event
  identity/
    secrets.py          KeychainSecretStore (FERB identity only)
    mailbox.py          FERB's inbox (read OTPs); send requires approval
    seed.py             verify FERB's Chrome profile holds exactly FERB's account (section 8 step 3); copies nothing
  clock/
    clock.py            SystemClock, FakeClock
    context.py          time header + tool footer builders
    durations.py        measured duration stats from spans
    timers.py           durable timers (SQLite) → bus events
  memory/
    journal.py          append-only episodic log
    items.py            distilled memory items (source of truth)
    distill.py          journal/trail → proposed items
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
macapp/                 RESERVED (not built in Block 1): FERB.app — audio service, stop hotkey, Keychain anchoring,
                        Safari extension (later blocks)
tests/
  fakes/                FakeClock, FakeRuntime, FakeBrowserDriver, recorded responses
```

Runtime data in `~/.ferb/`:

```
~/.ferb/
  config.toml           runtimes, budgets, policy toggles, exclusion lists
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
    deadline: datetime | None
    budget: Budget

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
```

- Stable bus events: `task.created`, `task.started`, `task.finished`, `tool.called`, `approval.requested`,
  `approval.resolved`, `trail.event`, `device.changed`, `browser.connected`, `browser.disconnected`,
  `browser.profiles_changed`, `browser.profile_paired`,
  `timer.fired`, `memory.proposed`, `memory.committed`.
- Every bus event has a class. **Critical** (never dropped): `task.*`, `approval.*`, `device.changed`,
  `browser.profile_paired`, `timer.fired`, `memory.committed`. **Telemetry** (may be dropped under pressure):
  `tool.called`, `trail.event`, `browser.connected`, `browser.disconnected`, `browser.profiles_changed`,
  `memory.proposed`. Also critical: `policy.blocked`, `worker.exposed`, `action.held` (undo countdown),
  `action.released`, `action.undone`, `stop.engaged`, `stop.lifted`. Subscriber queues are bounded and the publisher never blocks: when a queue is full, the
  oldest telemetry is dropped and the subscriber is told how many; if a queue is full of critical events, that
  subscriber is disconnected with an explicit overflow error (it must resubscribe and re-read state) rather
  than silently losing one.

## 7. Orchestrator flow

1. `ferb chat` message → supervisor (orchestrator runtime, persistent session per chat).
2. Supervisor returns one typed outcome: **`reply`** (just answer), **`clarify`** (ask one question), or **`plan`**.
   Ordinary conversation never spawns workers.
3. A plan lists subtasks with identity, role (reader / actor), scopes, dependencies (a DAG), deadlines and
   **success conditions**. Code validates it: DAG has no cycles, identity rules, scopes ⊆ allowed, depth ≤ 2,
   fan-out ≤ 6 per parent, aggregate budget set for the whole tree.
4. **Reader/actor split:** readers open untrusted content and return typed fields to the supervisor; actors act
   and only receive those fields, never raw pages, emails or messages (section 10.2).
5. Workers run concurrently; each has a run token, heartbeat and cancellation. A worker failure is returned as a
   typed `Outcome`, never raised, so independent siblings keep going; dependents are skipped.
6. Tool calls → `ferb mcp` → gateway → auth → identity + scope → intent / exposure / dataflow checks →
   resource locks → ledger intent → capability → sanitize → span → result + time footer.
7. Locks: `gui_lock` for real mouse/keyboard, plus resource locks (tab, account, file, app) from `act/locks.py`,
   acquired in a fixed order, held across observe → act → verify, never across model calls or approval waits.
8. Supervisor **verifies** each outcome against the plan's success conditions using fresh reads; the worker's
   own claim is evidence, not verification.
9. Retries: definite failures retry per policy. `unknown` outcomes (e.g. Send clicked, connection lost) are
   **reconciled first** (re-read the target: did it happen?) and never blindly retried.
10. Budgets are aggregate per task tree: worker runs, retries and verification count; background distillation
    and shadow decisions have their own separate budget.
11. Owner priority: while the owner uses mouse/keyboard, GUI actions pause; extension and background work continue.
12. `ferb stop` (or the stop hotkey) halts everything immediately (section 10.5).

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

## 8. Setup and device profile

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
Watchers emit `device.changed` on app install/removal; profile rescans that app only.

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

### 9.4 Extension contract

- Permissions (Chromium MV3): `tabs`, `scripting`, `debugger`, `nativeMessaging`, `tabGroups` (the "FERB" group),
  `alarms` (the 30 s reconnect), `storage` (install id, seen command ids), host access to all sites.
- The extension **only executes** daemon commands and **reports** state; it holds no policy and no task logic.
- Message protocol (JSON, versioned, validated on both sides):
  - daemon → ext: `open`, `navigate`, `close`, `list_tabs`, `snapshot`, `act`, `wait`, `screenshot`, `attach_debugger`,
    `detach_debugger`, `cdp` (allowlisted), `account` (run the site's account extractor, 9.3), `wake_close` (close
    exactly the wake page); control: `welcome` (connection generation), `paired` (key), `refused` (e.g. protocol
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
  When a task needs a temporarily unavailable paired browser, the daemon wakes the extension immediately by opening
  a unique blank local page in that profile (`--profile-directory`, e.g. `http://127.0.0.1:<port>/wake/<token>`;
  not `about:blank`, whose fragment is dropped so it couldn't be told apart from the owner's tabs), and closes
  exactly that page after the reconnect (spike 13).
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
- The value travels **sealed**: AES-GCM with a key set up at pairing (per install), a fresh nonce per seal, and the
  fill command's id and the allowed origin as associated data, so a sealed value can't be replayed into another
  command or origin. Only the extension's fill call unseals it; the native host and every message and log carry
  ciphertext (safety test 21). This protects FERB's own messages and logs, not against malware running as the
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
  **Pasted or quoted content in the owner's chat message** (terminal bracketed paste, fenced code blocks, `>`
  quotes) is wrapped as untrusted data — tags carrying a random id — before the supervisor sees it; the intent is
  derived only from the owner's own words around it, and code checks that every intent target (recipient, site,
  account, path) appears in those words.
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
- **Injection signals**: content that tries to issue instructions (Jev `content.injection_suspected`) marks the
  worker exposed immediately and is reported in `ferb logs`.

### 10.3 When FERB asks (the only confirmations)

Only when a mistake is costly **and** the action was not explicitly requested, or something looks wrong:
- spending money / payments (always, every autonomy level),
- permanent deletion (always, every autonomy level),
- intent mismatch the owner may still want, surprising destination, or injection signals on the path.
Approvals are single-use, expire in minutes, and are bound to the exact action (account, target, payload hash);
any change voids them.

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

### 10.6 Sanitization pipeline (`sanitize/redact.py`)

Runs before anything reaches a model, trace, log, exception message, journal, memory or export:
password/OTP field values, secrets, auth tokens in URLs, exclusion-list apps/domains, shell output matching
secret patterns, debugger events. Key-name redaction exempts FERB's own internal id fields (`task_id`,
`span_id`, `session_id`, `run_token_id` — ids, never token values); their values are still scanned for secret
patterns. Screenshots are never stored unless a task requires it, and are not sent
to models when the frame contains a password or excluded site.

## 11. Monitoring trail

- Watchers: app focus (NSWorkspace), window titles (Accessibility), browser tabs (extension `tab_event`s;
  AppleScript read-only when an extension is missing), file events (FSEvents on configured folders), idle time,
  owner input, app installs.
- Order of processing for every event (fixed):
  1. deterministic exclusions + redaction (`sense/exclusions.py`, `sanitize/`) — nothing excluded goes further;
  2. assign `id`, `actor` (owner / ferb / unknown; FERB's own tabs and actions are tagged with their `task_id`);
  3. persist to the append-only `trail` table and publish `trail.event`;
  4. only then: async Jev tagging (`trail.relevance`, `trail.sensitive`). A `sensitive` verdict triggers
     *additional* redaction of the stored row; it is never the first line of protection.
- `actor == "ferb"` events are excluded from habit-based resolution and from owner-preference distillation.
- `ferb pause` stops all watchers. Screenshots: on demand, by a task with `screen` scope; not persisted unless required.
- Consumers: resolver (site → browser + profile), distillation, `trail.query` capability.

## 12. Jev placement

All via `Decider`. Initially `ShadowDecider(primary=Rule/LLM, shadow=Jev)` logs both; switch per question after traces show agreement.

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

- Header on every model turn (owner decision: per turn, not per API request): now (weekday + tz), task start + elapsed, deadline + remaining, owner last active, last trail event age.
- Footer on every tool result: `[now 15:42:10 IST | task +11m | deadline in 18m]`.
- Measured durations: median/p90 for similar past tasks from spans, when available.
- Timers: `set_timer`, `remind_at`, `sleep_until`; SQLite-backed; survive restarts and sleep (overdue timers fire with `overdue_by`).
  Delivery: due → claimed (row lock) → delivered → acknowledged by its consumer. A claimed-but-unacknowledged timer
  is re-delivered after restart with the same timer id; consumers ignore ids they already handled.
- Elapsed time, timeouts, heartbeats and budgets use `Clock.monotonic()`; timestamps and schedules use `Clock.now()`.
- `FakeClock` in all tests.

## 14. Memory

**Source of truth is SQLite. Markdown is a generated, read-only view.**

| Layer | Store | Written by | Purpose |
|-------|-------|-----------|---------|
| Spans / tasks | SQLite | gateway, orchestrator | What FERB did |
| Trail | SQLite | watchers | What the owner did |
| Journal | SQLite (append-only) | daemon | Chats, outcomes, approvals, daily trail summaries — from first run |
| Memory items | SQLite `memory_items` | `distill` proposals → `commit` | Distilled facts/preferences/people/projects/env with provenance and validity |
| Markdown | `~/.ferb/memory/` folder | `export` only | Human-readable view, regenerated from SQLite (no git history kept) |

Rules:
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

## 15. CLI

| Command | Purpose |
|---------|---------|
| `ferb chat` | Conversation with the orchestrator; approvals inline |
| `ferb logs [--tasks --trail --tools --browsers --policy --actions]` | Live stream from the bus (redacted) |
| `ferb trace <task-id>` | Span tree for one task |
| `ferb setup [--update]` / `ferb doctor` | Onboarding (`--update`: reinstall FERB into the venv + restart the agent) / health, permissions, extension connectivity |
| `ferb uninstall [--purge]` | Remove the LaunchAgent (and native-host files); `--purge` also deletes `~/.ferb` after confirmation |
| `ferb device` / `ferb browsers` | Device profile / browser drivers + connection status |
| `ferb timeline`, `ferb memory …` | Memory journey |
| `ferb pause` / `ferb resume` | Stop / start monitoring |
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
9. *Skipped — deferred to the voice block.* System audio (timeboxed, ~1 hour, throwaway script — not FERB code): with the `catap` Python package, record 10 s
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
8. Orchestrator multi-worker (DAG, reader/actor, budgets, verification, reconciliation).
9. `sense` watchers + trail (fixed processing order, actor attribution) + exclusions.
10. `memory` (items with trust levels, distill, recall, purge-forget, export) + timeline.
11. Deferred: Safari control and FERB.app (later block).
12. `decide` with Jev in shadow.

**Gate:** owner-account actions (sending, posting, editing in the owner's accounts) are enabled only after the
section 21 safety tests pass.

## 19. Known gaps (not solved in Block 1)

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
- Global daily cost ceiling UI; trail retention/encryption (plaintext SQLite, rely on FileVault); provider outage modes.
- Hand-editable markdown memory (two-way sync) deferred.
- FERB never hides or fakes automation signals (it uses no remote debugging, so none is set by launch).
  `fill_secret` refuses, never types, when a site rejects a directly set value.
- FERB's identity needs the owner's Chrome running with the FERB profile loaded; FERB may open it (9.2).
- The FERB profile shares the owner's Chrome process: separate cookies and storage, not a separate browser
  process.
- The secret passes through more components than a CDP fill did (native host and extension); both are
  high-privilege, reviewed components.
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
"what is this song?" or "what is this video saying?". Not built in Block 1; this section fixes its shape so
nothing in Block 1 blocks it.

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
- **Spike results (fill in after spike 9, deferred to the voice block):** route coverage, permission behavior, DRM behavior.

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
10. Approval bound to payload: changing recipient or amount after approval voids it.
11. Lost acknowledgement after Send → ledger `unknown` → reconciliation, no duplicate send.
12. Wrong profile / wrong account: action refused when the verified account differs from the resolved one.
13. Stale `ElementRef` after navigation or reconnect is rejected.
14. Competing workers on the same tab/file: resource locks serialize them; no deadlock.
15. Disconnect/reconnect of the extension: no replay of old-generation commands.
16. Unpaired extension cannot act or write trail events.
17. Forgotten memory is not regenerated by distillation (sources purged) and is absent from FTS and exports.
18. `actor == "ferb"` trail events never become owner preferences or resolver history.
19. Timer claimed but unacknowledged before a crash is delivered exactly once to its consumer.
20. The FERB identity profile is refused at pairing and flagged by `doctor` when its Chrome accounts are not
    exactly FERB's account; cookie and password stores are never read.
21. Along the extension path, a secret or one-time code value never appears in native-host or extension messages,
    logs or errors (it travels sealed, 10.1); a sealed value is refused under another command id or origin.