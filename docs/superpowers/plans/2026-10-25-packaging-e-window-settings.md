# Packaging Phase E: The FERB Window and Settings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A regular FERB window with a sidebar (Tasks, Schedules & Reminders, Noticed, Memory, Settings) and a
Settings view (⌘,) with seven pages, all bare-minimum SwiftUI over daemon RPCs, where settings reach `config.toml`
only through the owner's own connection.

**Architecture:** The daemon gains a small settings registry (`ferb/daemon/settings.py`: listed privacy and retention
keys, checked values) behind `settings.get` / `settings.set`, plus `tasks.list`, `notices.list`, a `last_result` on
`schedules.list` and a redacted diagnostic file (`reports.diagnostic`), each a CLI-role method only (rule 10). The app
gains pure row decoders in `NotchCore` (`WindowRows.swift`), one store that turns each button into one RPC
(`FERBWindowStore`), a window controller with a `NavigationSplitView`, and the Settings tabs. Settings → AI,
Permissions and Browsers embed plan C's setup views on plan C's `SetupModel`; Settings → Notifications uses plan D's
`notifications.categories`. The menu bar (plan D's) and ⌘, open the window; the old sound/pointer window goes (its
controls move into Settings → General).

**Tech Stack:** Python 3.12, tomlkit, pytest; Swift 6 tools in Swift 5 mode, SwiftUI and AppKit on macOS 14,
`swift test` (package `ui/mac/FERBNotch`).

**Spec:** `docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md` (decision L34, approved by the owner on
2026-10-25: "all 7 parts in full, juat design them bare min. focus on correct working"). This plan builds "Phase E —
The FERB window and Settings" and the window/Settings parts of "What a new user experiences" §4. Read both before any
task. Plans C and D (`docs/superpowers/plans/2026-10-25-packaging-c-native-setup.md`,
`…-d-notifications-menubar.md`) are built before this one.

## Global Constraints

- Read `CLAUDE.md` first. Never read, search or reference `vibed/`. Work on `main`; commit messages cite `L34` (and
  `rule 10` where settings writes are involved).
- Every screen is bare minimum: plain SwiftUI (`Form`, `List`, `Section`, `Button`, `Toggle`, `Stepper`, `TextField`,
  `Text`, `LabeledContent`), system colours only, SF Symbols only in the sidebar; no animation. The owner polishes the
  look later with Codex.
- The notch stays the main interface ("notch as primary main use"); the window adds detail and never replaces a notch
  flow.
- Rule 10: the app writes `config.toml` through owner-only methods: this plan's `settings.set`, plan C's
  `runtimes.use`, plan D's `notifications.categories`. Every method this plan adds is in `Role.CLI` only
  (`ferb/daemon/auth.py`), never `MCP_SHIM`, `VOICE` or `NATIVE_HOST`. A key not in `SETTINGS`
  (`ferb/daemon/settings.py`) can't be written through `settings.set`: `[protection]`, `[secrets]`, `[models]`,
  `[mailbox]`, `[identity]`, `[feedback]`, `[runtimes]` (plan C's `runtimes.use`), `[banners]` (plan D's), `[privacy]
  exclude_paths` and `[sense] watch_folders` stay out.
- Rule 5: code checks every value's type and range; rule 6: no keyword or label list decides anything.
- Rule 14: the diagnostic file is redacted in every part; FERB sends it nowhere ("sending it anywhere is a later owner
  decision"). API keys are typed only in plan C's AI view (sealed Keychain path); this plan never takes or shows one.
- Rule 17: nothing in the window acts on the owner's screen by itself; only the owner's clicks open System Settings,
  Finder, a file or the window.
- No new behaviour limits. A retention value's minimum is the existing `retention.MIN_DAYS` (1); the Tasks page size
  (`TASK_PAGE = 50`, at most `TASK_PAGE_MAX = 200` per call) is display paging only.
- `task.stop` with an empty `task_ids` list stops every running task (`main.py:3192`): the window always sends exactly
  one id.
- Each task: failing test first, then code, then the focused tests. Python tests run as
  `nice -n 10 uv run pytest <files> -q`; Swift as `swift test --package-path ui/mac/FERBNotch --filter <Class>`.
  Before the last commit of the plan: `nice -n 10 uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`,
  `swift test --package-path ui/mac/FERBNotch`.
- Other agents have uncommitted work in this checkout: commit only the files your task names (`git add <paths>`), check
  `git diff --cached --stat` before each commit, and end each message with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Interfaces with other phases (assumptions; confirm against plans B/C/D)

Written against the plans as committed on 2026-10-25; the controller confirms them before execution.

- **Plan B (stable signing):** nothing consumed.
- **Plan C (native setup)** — confirm against plan C (its Task 7-8 and "Interfaces for other phases"):
  - C1. `AppSetupController` (rewritten by C) holds `private lazy var model = SetupModel(...)`, has
    `isPackaged`, `startAtLaunch()`, `runSetupFromMenu()`, `stop()`, `onFinished: (() -> Void)?`, and no console. This
    plan makes `model` internal (`lazy var model`) so Settings shares the one `SetupModel`.
  - C2. `SetupModel` (FERBNotch): `ai: AISnapshot?` (`current: String?`), `runner: SetupCommandRunning`
    (`run(_:line:done:) -> Bool`, `isRunning`, `cancel()`), `refreshAI() async`, `refreshBrowsers() async`,
    `refreshPermissions(probeHearing:) async`, `startPolling()`, `stopPolling()`.
  - C3. Views `AIStepView(model:)`, `PermissionsStepView(model:)`, `BrowsersStepView(model:)` (each
    `@ObservedObject var model: SetupModel`, no navigation buttons inside). Settings → AI, Permissions and Browsers
    embed them; API keys stay in `AIStepView` (sealed path), sign-in is its "Sign in with ChatGPT".
  - C4. NotchCore names this plan must not redeclare: `PermissionRow`, `BrowserRow`, `AISnapshot`, `RuntimeRow`,
    `APIProvider`, `GrantStatus`, `SetupStep`.
  - C5. Test support: `Tests/NotchCoreTests/Support/ScriptedRPC.swift` (`replies`, `failures`, `events`, `calls`,
    `methods()`, `params(of:)`) and `Support/SetupFakes.swift`. Task 5 adds a reply queue and recorded waits to
    `ScriptedRPC`; nothing else in it changes.
- **Plan D (notifications, menu bar)** — confirm against plan D (its Tasks 4 and 8):
  - D1. RPC `notifications.categories {set?: {category: bool}}` → `{categories: {question, reminder, alert,
    task_finished, loop}}`, writing `[banners]`; NotchCore `BannerCategory: String, CaseIterable` with `title`.
    Settings → Notifications uses both.
  - D2. `PanelCoordinator.hasWindow: Bool`, `onOpenWindow: (() -> Void)?` (D's menu shows "Open FERB Window" when
    `hasWindow`), `perform(.settings)` → `showSettings()`. This plan sets `hasWindow` and `onOpenWindow`, and replaces
    `showSettings()`'s old window with `onOpenSettings?()`.
- **Produced for plans F and G:** `GeneralSettingsPage` (Task 6) has sections "Notch", "FERB's pointer" and "Sounds";
  F adds "Startup", G adds "Updates". `FERBAppActions` (Task 6) carries `beforeUninstall: (() -> Void)?`, which F sets
  to remove the login item before `ferb uninstall` runs.

## Review Focus

- The daemon isn't running when the owner opens the window: every page must say why (the RPC error) instead of showing
  empty lists that read as "nothing here". Pinned in Task 5 (`testRefreshWithoutTheDaemonShowsWhyNotAnEmptyList`).
- A Settings write fails (daemon down, or FERB refuses the value): the control must show the stored value again and the
  problem line says why. Pinned in Task 5 (`testAFailedSettingWriteRestoresTheShownValueAndSaysWhy`).
- Stopping one task from the window: an empty `task_ids` stops everything, so the window must send exactly the one id,
  and never for a task that already ended. Pinned in Task 5 (`testStoppingOneTaskSendsOnlyItsID`).
- Forget is permanent: the window must show the daemon's own list of what goes (`will_delete`) and send
  `confirm: true` only after the owner's Forget. Pinned in Task 5 (`testForgetAsksWithTheDaemonsListBeforeDeleting`).
- A site pasted as a URL (`https://www.example.com/x`) or with a leading dot or capitals in "Sites FERB ignores": the
  daemon must refuse the URL with a clear message and store `.Example.COM` as `example.com`. Pinned in Task 1
  (`test_sites_are_hostnames_and_a_url_is_refused`).

---

### Task 1: `settings.get` / `settings.set` — listed keys, checked values, owner's connection only (rule 10)

**Files:**
- Create: `ferb/daemon/settings.py`
- Modify: `ferb/daemon/auth.py` (`METHODS[Role.CLI]`, `:30-43`)
- Modify: `ferb/daemon/main.py` (registration after `self.server.unary['reports.build'] = ...` at `:533`; handlers in
  a new section just above `# ---------- methods ----------` at `:4667`)
- Create: `tests/test_settings_rpc.py`
- Modify: `docs/0.27_SECURITY_TESTS.md` (append a scenario at the end)

**Interfaces:**
- Produces: RPC `settings.get {}` → `{"values": {"<section>.<key>": value, ...}}`; RPC `settings.set {"key": str,
  "value": any}` → `{"key": str, "value": stored}` or a `-32004` refusal naming the key and why. Journal kind
  `settings.changed {key, value}` with origin `owner`.
- Produces (Python): `ferb.daemon.settings.SETTINGS: dict[str, Setting]`, `read(config) -> dict[str, Any]`,
  `write(config, name, value) -> Any`, `BadSetting(FerbError, ValueError)`.

- [ ] **Step 1: Write the failing tests** (create `tests/test_settings_rpc.py`)

```python
"""L34 Phase E: the FERB window's Settings change config.toml only through `settings.set`, a method of the owner's own
connection (the CLI role), only for listed keys (rule 10), with every value checked (rule 5)."""

import plistlib
import tomllib
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.core.types import Identity
from ferb.daemon import auth
from ferb.daemon.server import UNAUTHORIZED
from ferb.daemon.settings import SETTINGS
from tests.conftest import RunningDaemon

ROOT = Path(__file__).resolve().parent.parent


def _call(daemon: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    c = Client(daemon.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


def _config(daemon: RunningDaemon) -> dict[str, Any]:
    path = daemon.daemon.home / "config.toml"
    return tomllib.loads(path.read_text()) if path.exists() else {}


async def _of_kind(daemon: RunningDaemon, kind: str) -> list[Any]:
    return daemon.daemon.journal.of_kind(kind)       # type: ignore[union-attr]


def test_settings_get_lists_every_setting(daemon: RunningDaemon) -> None:
    values = _call(daemon, "settings.get")["values"]
    assert set(values) == set(SETTINGS)
    assert values["sense.window_titles"] is True
    assert values["retention.evidence_days"] == 7
    assert "com.1password.1password" in values["privacy.exclude_apps"]


def test_a_setting_is_checked_written_and_journaled(daemon: RunningDaemon) -> None:
    assert _call(daemon, "settings.set", {"key": "retention.trace_days", "value": 14}) == {
        "key": "retention.trace_days", "value": 14}
    assert _config(daemon)["retention"]["trace_days"] == 14
    rows = daemon.run(_of_kind(daemon, "settings.changed"))
    assert rows[-1].data == {"key": "retention.trace_days", "value": 14} and "owner" in rows[-1].origin


@pytest.mark.parametrize("key,value", [
    ("sense.window_titles", "yes"), ("retention.trace_days", 0), ("retention.trace_days", True),
    ("retention.trace_days", 2.5), ("privacy.exclude_apps", "com.x.y"), ("privacy.exclude_apps", ["ok", "bad\nline"]),
    ("privacy.exclude_apps", [""]), ("privacy.exclude_domains", "example.com"),
])
def test_a_value_of_the_wrong_kind_is_refused_and_nothing_changes(daemon: RunningDaemon, key: str, value: Any) -> None:
    before = _config(daemon)
    with pytest.raises(RpcError, match=key.replace(".", r"\.")):
        _call(daemon, "settings.set", {"key": key, "value": value})
    assert _config(daemon) == before


def test_sites_are_hostnames_and_a_url_is_refused(daemon: RunningDaemon) -> None:
    stored = _call(daemon, "settings.set", {"key": "privacy.exclude_domains",
                                            "value": [".Example.COM", "news.site.org", "example.com"]})
    assert stored["value"] == ["example.com", "news.site.org"]
    with pytest.raises(RpcError, match="no https:// or path"):
        _call(daemon, "settings.set", {"key": "privacy.exclude_domains", "value": ["https://www.example.com/x"]})
    assert _config(daemon)["privacy"]["exclude_domains"] == ["example.com", "news.site.org"]


@pytest.mark.parametrize("key,value", [
    ("protection.never_write", ["/tmp"]), ("feedback.address", "x@example.org"), ("secrets.github", "token"),
    ("models.codex", "gpt-x"), ("mailbox.address", "a@example.org"), ("privacy.exclude_paths", []),
    ("identity.profile", "Profile 9"), ("runtimes.worker", "claude"), ("banners.question", False),
    ("sense.watch_folders", ["/"]), ("", True),
])
def test_keys_outside_the_list_cannot_be_written_from_the_app(daemon: RunningDaemon, key: str, value: Any) -> None:
    before = _config(daemon)
    with pytest.raises(RpcError, match="can't be changed from FERB's window"):
        _call(daemon, "settings.set", {"key": key, "value": value})
    assert _config(daemon) == before


def test_only_the_owners_connection_may_read_or_change_settings() -> None:
    for method in ("settings.get", "settings.set"):
        assert auth.allowed(auth.Role.CLI, method)
        for role in (auth.Role.MCP_SHIM, auth.Role.VOICE, auth.Role.NATIVE_HOST):
            assert not auth.allowed(role, method), (role, method)


def test_a_worker_token_cannot_change_settings_over_the_socket(daemon: RunningDaemon) -> None:
    """Rule 10: a worker's MCP shim (a run token) is refused, as the shell is refused the socket (test_shell_sandbox)."""
    async def issue() -> str:
        return daemon.daemon.tokens.issue("t-settings", "w-settings", set(), Identity.OWNER, 60)
    token = daemon.run(issue())
    before = _config(daemon)
    shim = Client(daemon.sock, role="mcp_shim", token=token)
    try:
        with pytest.raises(RpcError) as refused:
            shim.call("settings.set", {"key": "sense.window_titles", "value": False})
        assert refused.value.code == UNAUTHORIZED
    finally:
        shim.close()
    assert _config(daemon) == before


def test_no_tool_is_named_for_settings_and_ferb_never_operates_the_window(daemon: RunningDaemon) -> None:
    """The window belongs to FERB.app, one of FERB's own apps: every Mac tool refuses it (driver.OWN_APP)."""
    from ferb.act.mac.driver import MacComputerDriver
    from ferb.device.install import APP_BUNDLE_ID
    assert not any(str(name).startswith("settings") for name in daemon.daemon.registry.capabilities)
    info = plistlib.loads((ROOT / "ui" / "mac" / "FERBNotch" / "Info.plist").read_bytes())
    assert info["CFBundleIdentifier"] == APP_BUNDLE_ID
    assert MacComputerDriver._own_app(APP_BUNDLE_ID, -1)
```

- [ ] **Step 2: Run them to see them fail**

Run: `nice -n 10 uv run pytest tests/test_settings_rpc.py -q`
Expected: FAIL at collection: `ModuleNotFoundError: No module named 'ferb.daemon.settings'`.

- [ ] **Step 3: Create `ferb/daemon/settings.py`**

```python
"""Own the settings FERB.app's window may change in config.toml through `settings.set` (L34 Phase E). Must not let
anything but the owner's connection write them, or let an unchecked value through.

Rule 10 (FERB never changes its own config): these writes come only through `settings.set`, a method of the CLI role
alone (auth.py). That role is proven by the owner's uid on the daemon's socket, which FERB's shell and AppleScript
can't reach (the sandbox names the socket last: sandbox.py, test_shell_sandbox), no tool calls, and FERB's Mac tools
never operate FERB.app (driver.OWN_APP). Each value is checked here (rule 5). A key not in SETTINGS can't be written
this way: [protection], secrets and API keys, models, the mailbox and identity, the feedback address, exclude_paths
and watch_folders stay in config.toml and the CLI; the runtimes have `runtimes.use` and the banner switches
`notifications.categories`, each owner-only and checked where it is defined."""

import re
from dataclasses import dataclass
from typing import Any, Callable

from ferb.config import DEFAULT_EXCLUDE_APPS, Config
from ferb.core.errors import FerbError
from ferb.store.retention import MIN_DAYS

# A site as [privacy] exclude_domains holds it (sense/exclusions.py matches the host and its subdomains)
HOST = re.compile(r"[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*")
APP_CHARS = 200          # an app's bundle id or name, as [privacy] exclude_apps holds it


class BadSetting(FerbError, ValueError):
    """A key the app may not write, or a value of the wrong kind."""


def _bool(value: Any) -> bool:
    if not isinstance(value, bool):
        raise BadSetting("must be true or false")
    return value


def _days(value: Any) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < MIN_DAYS:
        raise BadSetting(f"must be a whole number of days, at least {MIN_DAYS}")
    return value


def _apps(value: Any) -> list[str]:
    if not isinstance(value, list):
        raise BadSetting("must be a list of apps")
    out: list[str] = []
    for item in value:
        text = item.strip() if isinstance(item, str) else ""
        if not text or len(text) > APP_CHARS or any(ord(c) < 32 for c in text):
            raise BadSetting(f"has {item!r}, which isn't an app's bundle id or name")
        if text not in out:
            out.append(text)
    return out


def _hosts(value: Any) -> list[str]:
    if not isinstance(value, list):
        raise BadSetting("must be a list of sites")
    out: list[str] = []
    for item in value:
        host = item.strip().lower().strip(".") if isinstance(item, str) else ""
        if not HOST.fullmatch(host):
            raise BadSetting(f"has {item!r}, which isn't a site name like example.com (no https:// or path)")
        if host not in out:
            out.append(host)
    return out


@dataclass(frozen=True)
class Setting:
    section: str
    key: str
    check: Callable[[Any], Any]
    default: Any

    @property
    def name(self) -> str:
        return f"{self.section}.{self.key}"


SETTINGS: dict[str, Setting] = {s.name: s for s in (
    Setting("privacy", "exclude_apps", _apps, list(DEFAULT_EXCLUDE_APPS)),
    Setting("privacy", "exclude_domains", _hosts, []),
    Setting("sense", "window_titles", _bool, True),
    Setting("sense", "trail_retention_days", _days, 30),
    Setting("retention", "trace_days", _days, 30),
    Setting("retention", "decision_days", _days, 30),
    Setting("retention", "evidence_days", _days, 7),
    Setting("retention", "recovery_days", _days, 7),
    Setting("retention", "resource_days", _days, 30),
)}


def _plain(value: Any) -> Any:
    """A tomlkit item as a plain value (its strings, numbers, booleans and arrays)."""
    return value.unwrap() if hasattr(value, "unwrap") else value


def read(config: Config) -> dict[str, Any]:
    """Every listed setting's value: config.toml's, else its default."""
    doc = config.load()
    values: dict[str, Any] = {}
    for name, s in SETTINGS.items():
        table = doc.get(s.section)
        raw = table.get(s.key) if table is not None else None
        values[name] = _plain(raw) if raw is not None else s.default
    return values


def write(config: Config, name: str, value: Any) -> Any:
    """Check one value and write it in one atomic save (Config.set); returns what was stored."""
    setting = SETTINGS.get(name)
    if setting is None:
        raise BadSetting(f"{name or '(no key)'} can't be changed from FERB's window")
    try:
        checked = setting.check(value)
    except BadSetting as e:
        raise BadSetting(f"{name} {e}") from None
    config.set(setting.section, setting.key, checked)
    return checked
```

- [ ] **Step 4: Allow the two methods for the CLI role only**

In `ferb/daemon/auth.py`, in `METHODS[Role.CLI]`, add `"settings.get", "settings.set"` to the set (after
`"reports.build"`, keeping plan C's and D's names already there).

- [ ] **Step 5: Register and implement the handlers**

In `ferb/daemon/main.py`, after `self.server.unary['reports.build'] = self._reports_build` (`:533`), add:

```python
        self.server.unary['settings.get'] = self._settings_get        # the FERB window's Settings (L34 Phase E)
        self.server.unary['settings.set'] = self._settings_set
```

Just above `    # ---------- methods ----------` (`:4667`), add:

```python
    # ---------- the FERB window and Settings (L34 Phase E) ----------

    async def _settings_get(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Settings: the values the app may change through settings.set (settings.SETTINGS)."""
        from ferb.daemon import settings
        return {"values": settings.read(self.config)}

    async def _settings_set(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Settings: one checked value written to config.toml (rule 10: the owner's own connection only, auth.py);
        the daemon reads it from its next use, as it does every config value."""
        from ferb.daemon import settings
        name = str(params.get("key") or "")
        stored = settings.write(self.config, name, params.get("value"))
        self._journal().append("settings.changed", {"key": name, "value": stored}, origin="owner")
        return {"key": name, "value": stored}
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `nice -n 10 uv run pytest tests/test_settings_rpc.py tests/test_auth.py -q`
Expected: PASS.

- [ ] **Step 7: Record the security scenario**

Append to `docs/0.27_SECURITY_TESTS.md`:

```markdown
### FERB's settings change only through the owner's own app or CLI (rule 10, L34 Phase E)

- **Source:** the FERB window's Settings (L34) write `config.toml` through the daemon. Rule 10: nothing lets FERB
  change its own config.
- **Channel:** a worker that read an injected page tries to empty the apps FERB ignores, stop window-title recording's
  companion settings, shorten retention or edit `[protection]`: through its MCP shim (`settings.set` with a run token),
  the shell (`nc -U ~/.ferb/run/ferb.sock`, or editing `config.toml`), AppleScript (System Events on FERB.app) or GUI
  clicks in FERB's own window.
- **Rule:** `settings.get`/`settings.set` belong to the CLI role only (the owner's uid on the socket); a run token is
  refused (`-32001`); the shell can't reach the socket or `~/.ferb`; Mac tools never operate FERB.app; only the keys in
  `ferb/daemon/settings.py` SETTINGS can be written that way, each value checked; `[protection]`, secrets, models,
  mailbox, identity, the feedback address, `exclude_paths` and `watch_folders` can't be written from the app.
- **Tests:** `tests/test_settings_rpc.py`, `tests/test_shell_sandbox.py::test_the_shell_cannot_reach_ferbs_state_tokens_or_socket`,
  `tests/test_own_apps.py`.
```

- [ ] **Step 8: Commit**

```bash
git add ferb/daemon/settings.py ferb/daemon/auth.py ferb/daemon/main.py tests/test_settings_rpc.py docs/0.27_SECURITY_TESTS.md
git diff --cached --stat
git commit -m "feat(L34, rule 10): settings.get/settings.set for the FERB window: listed keys, checked values, the owner's connection only

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The window's lists: `tasks.list`, `notices.list`, and a schedule's last result

**Files:**
- Modify: `ferb/daemon/main.py` (constants near `RESTORE_LIST` at `:101`; `_schedules_list` at `:3494`; registration
  after Task 1's lines; handlers in Task 1's section)
- Modify: `ferb/daemon/auth.py` (`METHODS[Role.CLI]`)
- Create: `tests/test_window_rpcs.py`

**Interfaces:**
- Produces: RPC `tasks.list {"limit"?: int 1-200 (default 50), "before"?: str}` → `{"tasks": [{"task_id", "goal",
  "status", "created_at", "updated_at", "answer", "verdict": "pass"|"fail"|null}], "more": bool}` (root tasks, newest
  first; `before` is the last row's `created_at`).
- Produces: RPC `notices.list {}` → `{"notices": [{"id", "text", "quotes": [{"text", "origin"}], "at", "shown":
  bool}]}` newest first; it acknowledges nothing.
- Produces: each `schedules.list` row gains `"last_result": {"at", "status", "answer"} | null`.

- [ ] **Step 1: Write the failing tests** (create `tests/test_window_rpcs.py`)

```python
"""L34 Phase E: the FERB window's Tasks, Noticed and Schedules pages read the daemon: root tasks newest first with
their answers and the owner's verdicts, every notice FERB posted (reading acknowledges nothing), and each schedule's
latest finished run."""

import json
from datetime import timedelta
from typing import Any

import pytest

from ferb.cli.client import Client, RpcError
from ferb.daemon import auth
from tests.conftest import RunningDaemon


def _call(daemon: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    c = Client(daemon.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


def _on_loop(daemon: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


def _root(daemon: RunningDaemon, task_id: str, minutes: int, status: str = "done", answer: str | None = None,
          outcome: dict[str, Any] | None = None, request_id: str = "") -> None:
    d = daemon.daemon
    at = (d.clock.now() + timedelta(minutes=minutes)).isoformat()

    def seed() -> None:
        assert d.db is not None
        d.db.conn.execute(
            "INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
            "outcome_json, created_at, updated_at) VALUES (?, ?, NULL, ?, ?, 'ferb', 'worker', '[]', ?, ?, ?, ?)",
            (task_id, task_id, request_id or "r-" + task_id, f"goal of {task_id}", status,
             json.dumps(outcome or {}), at, at))
        if answer is not None:
            d.db.conn.execute("INSERT INTO task_results (task_id, at, status, goal, answer, shown) "
                              "VALUES (?, ?, ?, ?, ?, 1)", (task_id, at, status, f"goal of {task_id}", answer))
    _on_loop(daemon, seed)


def _verdict(daemon: RunningDaemon, task_id: str, verdict: str) -> None:
    """A verdict row as `verdict.set` writes it (its learning side effects aren't this test's)."""
    d = daemon.daemon
    _on_loop(daemon, lambda: d.db.conn.execute(                                  # type: ignore[union-attr]
        "INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES (?, ?, ?, '', ?)",
        (f"v-{task_id}-{verdict}", task_id, verdict, d.clock.now().isoformat())))


def test_tasks_come_newest_first_with_their_answers_and_verdicts(daemon: RunningDaemon) -> None:
    _root(daemon, "t-old", 1, answer="Renamed 4 files.")
    _root(daemon, "t-new", 2, status="failed", outcome={"summary": "The folder was protected."})
    _verdict(daemon, "t-old", "fail")
    _verdict(daemon, "t-old", "pass")                                            # the latest verdict counts
    rows = _call(daemon, "tasks.list")["tasks"]
    mine = [r for r in rows if r["task_id"] in ("t-old", "t-new")]
    assert [r["task_id"] for r in mine] == ["t-new", "t-old"]
    assert mine[0]["answer"] == "The folder was protected." and mine[0]["verdict"] is None
    assert mine[1]["answer"] == "Renamed 4 files." and mine[1]["verdict"] == "pass"
    assert mine[1]["goal"] == "goal of t-old" and mine[1]["status"] == "done"


def test_tasks_page_by_created_at(daemon: RunningDaemon) -> None:
    for n in range(5):
        _root(daemon, f"t-{n}", 10 + n)
    first = _call(daemon, "tasks.list", {"limit": 2})
    assert [r["task_id"] for r in first["tasks"]] == ["t-4", "t-3"] and first["more"] is True
    second = _call(daemon, "tasks.list", {"limit": 2, "before": first["tasks"][-1]["created_at"]})
    assert [r["task_id"] for r in second["tasks"]] == ["t-2", "t-1"]


@pytest.mark.parametrize("limit", [0, 201, "50", True])
def test_a_page_size_outside_1_to_200_is_refused(daemon: RunningDaemon, limit: Any) -> None:
    with pytest.raises(RpcError, match="limit must be a whole number from 1 to 200"):
        _call(daemon, "tasks.list", {"limit": limit})


def test_a_sealed_value_in_an_answer_never_reaches_the_window(daemon: RunningDaemon) -> None:
    sealed = "Vantorel-Quill-8812"
    _root(daemon, "t-sealed", 3, answer=f"done with {sealed}")
    _on_loop(daemon, lambda: daemon.daemon.redact.remember_hidden_secret(sealed))   # type: ignore[attr-defined]
    row = next(r for r in _call(daemon, "tasks.list")["tasks"] if r["task_id"] == "t-sealed")
    assert sealed not in json.dumps(row)


def test_noticed_lists_every_notice_and_reading_acknowledges_nothing(daemon: RunningDaemon) -> None:
    notices = daemon.daemon.attention_notices
    assert notices is not None
    _on_loop(daemon, lambda: notices.add("n-1", [], "Your build finished.", [{"text": "BUILD OK"}]))
    _on_loop(daemon, lambda: daemon.daemon.clock.advance(5))                    # type: ignore[attr-defined]
    _on_loop(daemon, lambda: notices.add("n-2", [], "Mail from the bank arrived."))
    _on_loop(daemon, lambda: notices.acknowledge(["n-1"]))
    rows = _call(daemon, "notices.list")["notices"]
    assert [(r["id"], r["shown"]) for r in rows] == [("n-2", False), ("n-1", True)]   # newest first
    assert rows[1]["quotes"][0]["text"] == "BUILD OK"
    assert [n["id"] for n in _call(daemon, "chat.status", {"peek": True})["notices"]] == ["n-2"]   # still unseen


def test_a_schedule_shows_its_latest_finished_run(daemon: RunningDaemon) -> None:
    from ferb.orchestrator.schedules import Rule
    d = daemon.daemon
    first = d.clock.now() + timedelta(days=1)
    sid = _on_loop(daemon, lambda: d.schedules.add("Post the weekly update", "every day post the weekly update", {},
                                                   Rule.build("daily", first), first).id)
    assert _call(daemon, "schedules.list")["schedules"][0]["last_result"] is None
    _on_loop(daemon, lambda: d.journal.append("schedule.run", {"schedule_id": sid, "request_id": "req-run-1",   # type: ignore[union-attr]
                                                               "late": False}))
    _root(daemon, "t-run-1", 5, answer="Posted it.", request_id="req-run-1")
    row = _call(daemon, "schedules.list")["schedules"][0]
    assert row["last_result"]["status"] == "done" and row["last_result"]["answer"] == "Posted it."


def test_only_the_owners_connection_may_read_the_lists() -> None:
    for method in ("tasks.list", "notices.list"):
        assert auth.allowed(auth.Role.CLI, method)
        for role in (auth.Role.MCP_SHIM, auth.Role.VOICE, auth.Role.NATIVE_HOST):
            assert not auth.allowed(role, method), (role, method)
```

- [ ] **Step 2: Run them to see them fail**

Run: `nice -n 10 uv run pytest tests/test_window_rpcs.py -q`
Expected: FAIL (`tasks.list not allowed for cli`, `last_result` missing).

- [ ] **Step 3: Implement**

`ferb/daemon/auth.py`, in `METHODS[Role.CLI]`: add `"tasks.list", "notices.list"`.

`ferb/daemon/main.py`, after `RESTORE_LIST = 100 ...` (`:101`), add:

```python
TASK_PAGE, TASK_PAGE_MAX = 50, 200                 # the FERB window's Tasks page (L34 Phase E): rows per page, most per call
```

After Task 1's two registration lines, add:

```python
        self.server.unary['tasks.list'] = self._tasks_list
        self.server.unary['notices.list'] = self._notices_list
```

Replace `_schedules_list` (`:3494-3497`) with:

```python
    async def _schedules_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"schedules": [{"id": s.id, "request": s.request, "rule": s.rule.describe(), "state": s.state,
                               "next_run": s.next_run.isoformat() if s.next_run else None, "failures": s.failures,
                               "watch": s.watch, "last_result": self._schedule_last_result(s.id)}
                              for s in self._listed_schedules()]}

    def _schedule_last_result(self, sid: str) -> dict[str, Any] | None:
        """A schedule's latest finished run as the owner was told it (L34 Phase E): the result of the root task a
        `schedule.run` request made; None before any run finished."""
        assert self.db is not None
        row = self.db.conn.execute(
            "SELECT r.at, r.status, r.answer FROM task_results r JOIN tasks t ON t.id = r.task_id "
            "WHERE t.parent_id IS NULL AND t.request_id IN (SELECT json_extract(data_json, '$.request_id') FROM journal "
            "WHERE kind = 'schedule.run' AND json_extract(data_json, '$.schedule_id') = ?) ORDER BY r.at DESC LIMIT 1",
            (sid,)).fetchone()
        return {"at": row["at"], "status": row["status"], "answer": self._redact_text(row["answer"])} if row else None
```

In Task 1's section, after `_settings_set`, add:

```python
    async def _tasks_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Tasks: root tasks newest first, `limit` at a time before `before` (the `created_at` the last page ended at),
        each with its status, what FERB answered (its result, else its outcome) and the owner's latest verdict."""
        if self.db is None:
            raise FerbError("daemon not started")
        limit = params.get("limit", TASK_PAGE)
        if isinstance(limit, bool) or not isinstance(limit, int) or not 1 <= limit <= TASK_PAGE_MAX:
            raise FerbError(f"limit must be a whole number from 1 to {TASK_PAGE_MAX}")
        before = str(params.get("before") or "9999")           # sorts after every ISO time
        rows = self.db.conn.execute(
            "SELECT t.id, t.goal, t.status, t.created_at, t.updated_at, t.outcome_json, r.answer, "
            "(SELECT v.verdict FROM owner_verdicts v WHERE v.task_id = t.id ORDER BY v.seq DESC LIMIT 1) AS verdict "
            "FROM tasks t LEFT JOIN task_results r ON r.task_id = t.id "
            "WHERE t.parent_id IS NULL AND t.created_at < ? ORDER BY t.created_at DESC, t.rowid DESC LIMIT ?",
            (before, limit + 1)).fetchall()
        out = []
        for r in rows[:limit]:
            outcome = json.loads(r["outcome_json"] or "{}")
            answer = r["answer"] or outcome.get("answer") or outcome.get("summary") or ""
            out.append({"task_id": r["id"], "goal": self._redact_text(str(r["goal"])), "status": r["status"],
                        "created_at": r["created_at"], "updated_at": r["updated_at"],
                        "answer": self._redact_text(str(answer)), "verdict": r["verdict"]})
        return {"tasks": out, "more": len(rows) > limit}

    async def _notices_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Noticed: every notice FERB posted, newest first, as clients render them (AttentionNotices.view), and whether
        a client has shown it. Reading acknowledges nothing, so the notch still shows a new one in chat."""
        if self.attention_notices is None:
            return {"notices": []}
        rows = self.attention_notices.list(pending=True) + self.attention_notices.list(pending=False)
        rows.sort(key=lambda r: str(r["created_at"]), reverse=True)
        return {"notices": [self.attention_notices.view(r) | {"shown": r["delivered_at"] is not None} for r in rows]}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `nice -n 10 uv run pytest tests/test_window_rpcs.py tests/test_owner_answers.py -q -k "window or schedule"`
Expected: PASS (`test_owner_answers.py` uses `schedules.list`; its rows gain a key only).

- [ ] **Step 5: Commit**

```bash
git add ferb/daemon/main.py ferb/daemon/auth.py tests/test_window_rpcs.py
git diff --cached --stat
git commit -m "feat(L34): tasks.list, notices.list and a schedule's last result for the FERB window

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Settings → Advanced and AI backends: the diagnostic file, `ferb setup --cli`, `ferb setup --codex-logout`

**Files:**
- Modify: `ferb/daemon/reports.py` (constants after `SPAN_LABEL` at `:37`; `purge` at `:206`; new `diagnostic`)
- Modify: `ferb/daemon/main.py` (`_reports_build` at `:4765`; new `_report_facts`, `_reports_diagnostic`;
  registration)
- Modify: `ferb/daemon/server.py` (`LONG_WAITS` at `:39`)
- Modify: `ferb/daemon/auth.py` (`METHODS[Role.CLI]`)
- Modify: `ferb/cli/__init__.py` (setup parser at `:149-161`, dispatch at `:269-282`; plan C added `--service` there)
- Modify: `ferb/cli/setup.py` (new `cli_setup`, `codex_logout` after `codex_login`)
- Create: `tests/test_advanced_settings.py`

**Interfaces:**
- Produces: RPC `reports.diagnostic {"app_version"?: str}` → `{"path": str, "summary": str}`; the file is
  `~/.ferb/reports/FERB-diagnostic-YYYY-MM-DD-HHMMSS.txt` (folder 0700, file 0600) with sections `Summary`, `Health`,
  `Recording`, `Recent tasks`, `FERB's recorded problems`. Journal kind `diagnostic.built {file}`.
- Produces: `ferb setup --cli` (FERB.app only: links `~/.local/bin/ferb`, prints `cli_status()`, exit 0 when the link is
  FERB's) and `ferb setup --codex-logout` (`codex logout` in `~/.ferb/codex-home` only). Both non-interactive, so
  plan C's `SetupCommandRunning` can run them.
- Produces (Python): `reports.diagnostic(db_path, health, recording, facts, *, home, now, walk, text,
  app_version="") -> dict[str, str]`; `reports.purge(folder, now, days=KEEP_DAYS, names=NAME)`.

- [ ] **Step 1: Write the failing tests** (create `tests/test_advanced_settings.py`)

```python
"""L34 Phase E, Settings → Advanced and AI: the diagnostic file is written on this Mac with every part redacted and is
sent nowhere; `ferb setup --cli` links FERB.app's command; `ferb setup --codex-logout` signs out FERB's own Codex only."""

import io
import json
import os
import re
import stat
import subprocess
import uuid
from pathlib import Path
from typing import Any

import pytest

from ferb.cli import setup as setup_cmd
from ferb.cli.client import Client, RpcError
from ferb.daemon import auth
from ferb.daemon.reports import DIAGNOSTIC_SECTIONS
from ferb.daemon.server import INTERNAL_ERROR
from ferb.device.install import BUNDLE_CLI, Installer
from tests.conftest import RunningDaemon
from tests.support.app_bundle import make_ferb_app

SEALED = "Morvane-Tessel-2290"
API_KEY = "sk-ant-api03-" + "Qx4" * 12


def _on_loop(daemon: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


def _fake_health(daemon: RunningDaemon, note: str = "fine") -> None:
    async def health(session: Any, params: dict[str, Any]) -> dict[str, Any]:
        assert params == {"selftest": False, "hearing": False}
        return {"permissions": {"accessibility": "granted"}, "note": note, "version": "test"}

    async def recording(session: Any, params: dict[str, Any]) -> dict[str, Any]:
        return {"paused": False, "watchers": ["app_focus"]}
    daemon.daemon._doctor_check = health                 # type: ignore[method-assign]
    daemon.daemon._sense_status = recording              # type: ignore[method-assign]


def _export(daemon: RunningDaemon, **params: Any) -> dict[str, Any]:
    c = Client(daemon.sock, timeout=30)
    try:
        return c.call("reports.diagnostic", params)
    finally:
        c.close()


def test_the_diagnostic_file_has_its_sections_and_is_owner_only(daemon: RunningDaemon) -> None:
    _fake_health(daemon)
    r = _export(daemon, app_version="0.28.0 (202611010900)")
    path = Path(r["path"])
    assert path.parent == daemon.daemon.home / "reports"
    assert re.fullmatch(r"FERB-diagnostic-\d{4}-\d{2}-\d{2}-\d{6}\.txt", path.name)
    assert stat.S_IMODE(path.stat().st_mode) == 0o600 and stat.S_IMODE(path.parent.stat().st_mode) == 0o700
    text = path.read_text()
    heads = [text.index(f"\n## {name}\n") for name in DIAGNOSTIC_SECTIONS]
    assert heads == sorted(heads)
    assert "App version: 0.28.0 (202611010900)" in text and '"accessibility": "granted"' in text
    assert '"app_focus"' in text
    built = _on_loop(daemon, lambda: daemon.daemon.journal.of_kind("diagnostic.built"))
    assert built and built[-1].data["file"] == str(path)


def test_every_part_of_the_diagnostic_file_is_redacted(daemon: RunningDaemon) -> None:
    _fake_health(daemon, note=f"{SEALED} {API_KEY}")
    _on_loop(daemon, lambda: daemon.daemon.redact.remember_hidden_secret(SEALED))     # type: ignore[attr-defined]
    _on_loop(daemon, lambda: daemon.daemon.db.conn.execute(                            # type: ignore[union-attr]
        "INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, ?, ?)",
        (uuid.uuid4().hex, daemon.daemon.clock.now().isoformat(), INTERNAL_ERROR,
         json.dumps({"method": "x", "error": "KeyError", "message": SEALED, "frames": []}))))
    text = Path(_export(daemon)["path"]).read_text()
    assert SEALED not in text and API_KEY not in text and "sk-ant" not in text and "[redacted]" in text


def test_old_diagnostic_files_go_and_failure_reports_stay(daemon: RunningDaemon) -> None:
    _fake_health(daemon)
    folder = daemon.daemon.home / "reports"
    folder.mkdir(parents=True, exist_ok=True)
    old_diag = folder / "FERB-diagnostic-2025-01-01-120000.txt"
    old_report = folder / "FERB-report-2025-01-01-t-old.txt"
    past = daemon.daemon.clock.now().timestamp() - 40 * 86400
    for f in (old_diag, old_report):
        f.write_text("old")
        os.utime(f, (past, past))
    _export(daemon)
    assert not old_diag.exists() and old_report.exists()


def test_a_redactor_without_a_text_path_writes_no_diagnostic_file(daemon: RunningDaemon) -> None:
    _fake_health(daemon)
    daemon.daemon.redact = lambda value: value          # type: ignore[assignment]
    with pytest.raises(RpcError, match="couldn't redact"):
        _export(daemon)


def test_only_the_owners_connection_may_export_a_diagnostic_file() -> None:
    assert auth.allowed(auth.Role.CLI, "reports.diagnostic")
    for role in (auth.Role.MCP_SHIM, auth.Role.VOICE, auth.Role.NATIVE_HOST):
        assert not auth.allowed(role, "reports.diagnostic")


def _env(tmp_path: Path, bundle: bool = True) -> tuple[setup_cmd.Env, io.StringIO]:
    app = make_ferb_app(tmp_path / "Applications") if bundle else None
    inst = Installer(tmp_path / ".ferb", launch_agents=tmp_path / "LaunchAgents", bundle=app, cli_dir=tmp_path / "bin")
    out = io.StringIO()
    return setup_cmd.Env(inst, lambda: None, lambda prompt: "", out), out     # type: ignore[arg-type,return-value]


def test_install_command_line_tool_links_ferb_apps_command(tmp_path: Path) -> None:
    env, out = _env(tmp_path)
    assert setup_cmd.cli_setup(env) == 0
    link = tmp_path / "bin" / "ferb"
    assert link.is_symlink() and link.resolve() == (env.inst.bundle / BUNDLE_CLI).resolve()   # type: ignore[operator]
    assert "`ferb` command: " in out.getvalue() and ".zprofile" in out.getvalue()


def test_install_command_line_tool_leaves_another_ferb_alone(tmp_path: Path) -> None:
    env, out = _env(tmp_path)
    (tmp_path / "bin").mkdir()
    (tmp_path / "bin" / "ferb").write_text("#!/bin/sh\necho mine\n")
    assert setup_cmd.cli_setup(env) == 1
    assert (tmp_path / "bin" / "ferb").read_text() == "#!/bin/sh\necho mine\n"
    assert "left" in out.getvalue()


def test_install_command_line_tool_is_for_ferb_app_only(tmp_path: Path) -> None:
    env, out = _env(tmp_path, bundle=False)
    assert setup_cmd.cli_setup(env) == 1 and "FERB.app" in out.getvalue()


def test_sign_out_of_chatgpt_signs_out_ferbs_own_codex_only(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    env, out = _env(tmp_path)
    codex_home = env.inst.home / "codex-home"
    codex_home.mkdir(parents=True)
    (codex_home / "auth.json").write_text("{}")
    monkeypatch.setattr("ferb.runtimes.codex_exec.codex_binary", lambda home, *a: tmp_path / "codex")
    seen: list[tuple[list[str], str]] = []

    def run(argv: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[str]:
        seen.append((argv, env["CODEX_HOME"]))
        (codex_home / "auth.json").unlink()
        return subprocess.CompletedProcess(argv, 0, "", "")
    assert setup_cmd.codex_logout(env, run=run) == 0
    assert seen == [([str(tmp_path / "codex"), "logout"], str(codex_home))]
    assert "Signed out" in out.getvalue()


def test_sign_out_without_a_sign_in_changes_nothing(tmp_path: Path) -> None:
    env, out = _env(tmp_path)
    assert setup_cmd.codex_logout(env, run=lambda *a, **k: pytest.fail("ran codex")) == 0
    assert "isn't signed in" in out.getvalue()
```

- [ ] **Step 2: Run them to see them fail**

Run: `nice -n 10 uv run pytest tests/test_advanced_settings.py -q`
Expected: FAIL at collection: `ImportError: cannot import name 'DIAGNOSTIC_SECTIONS'`.

- [ ] **Step 3: The diagnostic file in `ferb/daemon/reports.py`**

After `SPAN_LABEL = ...` (`:37`), add:

```python
# Settings → Advanced → Export a diagnostic file (L34 Phase E): FERB's state for debugging, one text file per export
DIAGNOSTIC = re.compile(r"FERB-diagnostic-\d{4}-\d{2}-\d{2}-\d{6}\.txt")
DIAGNOSTIC_SECTIONS = ("Summary", "Health", "Recording", "Recent tasks", "FERB's recorded problems")
RECENT_TASKS = 20
```

Replace `purge` (`:206-216`) with:

```python
def purge(folder: Path, now: datetime, days: int = KEEP_DAYS, names: re.Pattern[str] = NAME) -> int:
    """Removes FERB's own files of one kind (`names`: failure reports, or diagnostic files) last written more than
    `days` ago; how many."""
    if not folder.is_dir():
        return 0
    cutoff = now.timestamp() - days * 86400
    gone = 0
    for p in folder.iterdir():
        if names.fullmatch(p.name) and not p.is_symlink() and p.is_file() and p.stat().st_mtime < cutoff:
            p.unlink(missing_ok=True)
            gone += 1
    return gone
```

After `build` (before `_redacted`), add:

```python
def diagnostic(db_path: Path, health: dict[str, Any], recording: dict[str, Any], facts: dict[str, Any], *,
               home: Path, now: datetime, walk: Callable[[Any], Any], text: Callable[[str], str],
               app_version: str = "") -> dict[str, str]:
    """Settings → Advanced → Export a diagnostic file (L34 Phase E): versions, the daemon's own health check, what it
    records, its latest tasks by id and status, and its recorded problems, every part redacted as a failure report is
    (rule 14; never raw stderr). FERB sends nothing. Runs off the event loop on its own connection; diagnostic files
    older than KEEP_DAYS go first. Returns {path, summary}."""
    db = Database(db_path)
    try:
        tasks = [dict(r) for r in db.conn.execute(
            "SELECT id, status, created_at FROM tasks WHERE parent_id IS NULL ORDER BY created_at DESC, rowid DESC "
            "LIMIT ?", (RECENT_TASKS,))]
        rows = problems(db, None, None)
    finally:
        db.close()
    mac = platform.mac_ver()[0] or "unknown"
    summary = [("FERB version", ferb.__version__), ("macOS version", mac),
               ("App version", lines.clip(app_version, 60) if app_version else "unknown"),
               ("Extension version", "; ".join(facts.get("extensions") or []) or "unknown"),
               ("AI runtime and model", "; ".join(facts.get("runtimes") or []) or "unknown"),
               ("Written", now.astimezone().isoformat(timespec="seconds"))]

    def shown(value: Any) -> str:
        return text(json.dumps(walk(value), ensure_ascii=False, indent=2, sort_keys=True, default=str))
    parts = [text("FERB diagnostic file"), "",
             "## Summary", text("\n".join(f"{key}: {value}" for key, value in summary)), "",
             "## Health", shown(health), "",
             "## Recording", shown(recording), "",
             "## Recent tasks",
             *([text(f"{t['created_at']}  {t['id']}  {t['status']}") for t in tasks] or ["(none recorded)"]), "",
             "## FERB's recorded problems",
             f"FERB's own records of requests that failed inside it or answered slowly: the latest {PROBLEMS_LAST}.",
             *([text(f"{at}  {kind}  {json.dumps(walk(data), ensure_ascii=False)}") for at, kind, data in rows]
               or ["(none recorded)"]), ""]
    folder = home / FOLDER
    purge(folder, now, names=DIAGNOSTIC)
    path = write(folder, f"FERB-diagnostic-{now.astimezone():%Y-%m-%d-%H%M%S}.txt", "\n".join(parts))
    return {"path": str(path), "summary": text(f"FERB {ferb.__version__}, macOS {mac}")}
```

- [ ] **Step 4: The RPC**

`ferb/daemon/server.py`: replace `LONG_WAITS = {"reports.build": 120.0}` with
`LONG_WAITS = {"reports.build": 120.0, "reports.diagnostic": 60.0}` (keep any entry another plan added).

`ferb/daemon/auth.py`: in `METHODS[Role.CLI]`, add `"reports.diagnostic"`.

`ferb/daemon/main.py`: after Task 2's registration lines add
`        self.server.unary['reports.diagnostic'] = self._reports_diagnostic`. In `_reports_build`, delete the line
`        from ferb.runtimes.catalog import CATALOG, ROLES` and replace the block from `        runtimes = []` through the
`facts = {...}` assignment with `        facts = self._report_facts()`. Then add, directly above `_reports_build`:

```python
    def _report_facts(self) -> dict[str, Any]:
        """The runtimes and extension versions a failure report (L35) and a diagnostic file (L34 Phase E) name."""
        from ferb.runtimes.catalog import CATALOG, ROLES
        runtimes = []
        for role in ROLES:
            name = self._runtime_for(role)
            model = self._model_for(name) or (CATALOG[name].default_model if name in CATALOG else "")
            runtimes.append(f"{role}: {name} {model}".rstrip())
        bridges = self.hub.bridges.items() if self.hub is not None else []
        return {"runtimes": runtimes,
                "extensions": [f"{browser} {folder}: {br.version}" for (browser, folder), br in bridges if br.version]}
```

And in Task 1's section, after `_notices_list`:

```python
    async def _reports_diagnostic(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Settings → Advanced → Export a diagnostic file: reports.diagnostic writes it; FERB sends nothing (sending it
        anywhere is a later owner decision). Params: `app_version`, the app's version."""
        from ferb.daemon import reports
        if self.db is None:
            raise FerbError("daemon not started")
        text = getattr(self.redact, "text", None)
        if not callable(text):                     # never written unredacted
            raise FerbError("FERB couldn't redact the diagnostic file, so it wrote none")
        health = await self._doctor_check(session, {"selftest": False, "hearing": False})
        recording = await self._sense_status(session, {})
        built = await asyncio.to_thread(
            reports.diagnostic, self.db.path, health, recording, self._report_facts(), home=self.home,
            now=self.clock.now(), walk=self.redact, text=text, app_version=str(params.get("app_version") or ""))
        self._journal().append("diagnostic.built", {"file": built["path"]})
        return built
```

- [ ] **Step 5: `ferb setup --cli` and `ferb setup --codex-logout`**

`ferb/cli/__init__.py`, after the `--codex-login` argument, add:

```python
    sp.add_argument("--codex-logout", action="store_true",
                    help="sign out FERB's own Codex (your personal Codex login is never touched)")
    sp.add_argument("--cli", action="store_true",
                    help="link the `ferb` command at ~/.local/bin to this FERB.app (Settings → Advanced)")
```

and before `            if args.cmd == "setup" and args.codex_login:` add:

```python
            if args.cmd == "setup" and args.codex_logout:
                return setup_cmd.codex_logout(env)
            if args.cmd == "setup" and args.cli:
                return setup_cmd.cli_setup(env)
```

`ferb/cli/setup.py`, after `codex_login`, add (`BUNDLE_CLI` joins the existing `from ferb.device.install import (...)`
list):

```python
def codex_logout(env: Env, run: Callable[..., Any] = subprocess.run) -> int:
    """Settings → AI → Sign out of ChatGPT (L34 Phase E): `codex logout` in FERB's own Codex home only; the owner's
    personal Codex login is never touched."""
    import os
    from ferb.runtimes.codex_exec import codex_binary
    home = env.inst.home / "codex-home"
    if not (home / "auth.json").exists():
        env.say("FERB's Codex isn't signed in; nothing to sign out of.")
        return 0
    try:
        binary = codex_binary(env.inst.home)
    except Exception as e:
        env.say(f"FERB's pinned Codex isn't installed: {e}")
        return 1
    rc = run([str(binary), "logout"], env={**os.environ, "CODEX_HOME": str(home)}).returncode
    gone = not (home / "auth.json").exists()
    env.say("Signed out: FERB can't answer through Codex until you sign in again." if rc == 0 and gone else
            "Codex didn't sign out; nothing changed.")
    return 0 if rc == 0 and gone else 1


def cli_setup(env: Env) -> int:
    """Settings → Advanced → Install command-line tool (L34 Phase E): `ferb` on PATH as a link to this FERB.app's
    launcher (Installer.link_cli leaves anything else called `ferb` alone). Setup never edits shell profiles."""
    inst = env.inst
    if inst.bundle is None:
        env.say("The command-line tool is linked from FERB.app; a checkout runs `uv run ferb`.")
        return 1
    inst.link_cli()
    link = inst.cli_link
    ok = link is not None and link.is_symlink() and link.resolve() == (inst.bundle / BUNDLE_CLI).resolve()
    env.say(inst.cli_status() or "")
    if ok:
        env.say('To use it in a terminal, add  export PATH="$HOME/.local/bin:$PATH"  to ~/.zprofile.')
    return 0 if ok else 1
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `nice -n 10 uv run pytest tests/test_advanced_settings.py tests/test_failure_report.py -q`
Expected: PASS (the failure-report tests cover `_report_facts` and the unchanged `purge` default).

- [ ] **Step 7: Commit**

```bash
git add ferb/daemon/reports.py ferb/daemon/main.py ferb/daemon/server.py ferb/daemon/auth.py ferb/cli/__init__.py ferb/cli/setup.py tests/test_advanced_settings.py
git diff --cached --stat
git commit -m "feat(L34): a redacted local diagnostic file, ferb setup --cli and --codex-logout for Settings

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The window's rows in `NotchCore`

**Files:**
- Create: `ui/mac/FERBNotch/Sources/NotchCore/WindowRows.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/WindowRowsTests.swift`

**Interfaces:**
- Produces (public, `NotchCore`; none of plan C's names): `WindowTime.parse(_:) -> Date?`;
  `TaskHistoryRow(_ row:)` (`id`, `goal`, `status`, `createdAt`, `createdAtText`, `answer`, `verdict`, `isRunning`);
  `ScheduleRow(_:)` (`id`, `request`, `rule`, `state`, `nextRun`, `failures`, `lastResult: ScheduleRow.LastResult?`,
  `actions: [String]`); `ReminderRow(_:)`; `NoticeRow(_:)` (`quotes: [NoticeQuote]`, `shown`); `MemoryRow(_:)`;
  `SettingValue` (`init?(_ any:)`, `json`); `SettingsSnapshot` (`init(values:)`, `init(_ reply:)`, `bool`, `int`,
  `strings`); `HealthLine.lines(_ doctor:)`.

- [ ] **Step 1: Write the failing tests** (create `Tests/NotchCoreTests/WindowRowsTests.swift`)

```swift
import XCTest
@testable import NotchCore

/// L34 Phase E: the FERB window reads the daemon's replies into rows. A row without its id is dropped; any other
/// missing field reads as empty, so an older daemon still shows what it has.
final class WindowRowsTests: XCTestCase {
    func testDaemonTimesWithAndWithoutMicrosecondsParse() {
        XCTAssertNotNil(WindowTime.parse("2026-10-25T14:03:00.123456+05:30"))
        XCTAssertNotNil(WindowTime.parse("2026-10-25T14:03:00+05:30"))
        XCTAssertNil(WindowTime.parse("yesterday"))
        XCTAssertNil(WindowTime.parse(nil))
    }

    func testTaskRowsKeepTheirAnswerVerdictAndPagingTime() throws {
        let row = try XCTUnwrap(TaskHistoryRow(["task_id": "t-1", "goal": "Rename the invoices", "status": "done",
                                                "created_at": "2026-10-25T14:03:00+05:30", "answer": "Renamed 4.",
                                                "verdict": "pass"]))
        XCTAssertEqual(row.id, "t-1")
        XCTAssertEqual(row.createdAtText, "2026-10-25T14:03:00+05:30")
        XCTAssertNotNil(row.createdAt)
        XCTAssertEqual(row.verdict, "pass")
        XCTAssertFalse(row.isRunning)
        XCTAssertTrue(TaskHistoryRow(["task_id": "t-2", "status": "running"])!.isRunning)
        XCTAssertNil(TaskHistoryRow(["goal": "no id"]))
        XCTAssertNil(TaskHistoryRow(["task_id": "", "goal": "empty id"]))
    }

    func testScheduleRowsOfferOnlyTheActionsTheirStateAllows() throws {
        let active = try XCTUnwrap(ScheduleRow(["id": "sc-1", "request": "Post the update", "rule": "every day at 09:00",
                                                "state": "active", "next_run": "2026-10-26T09:00:00+00:00",
                                                "failures": 2, "last_result": ["at": "2026-10-25T09:01:00+00:00",
                                                                               "status": "done", "answer": "Posted."]]))
        XCTAssertEqual(active.actions, ["pause", "cancel"])
        XCTAssertEqual(active.failures, 2)
        XCTAssertEqual(active.lastResult?.answer, "Posted.")
        XCTAssertEqual(ScheduleRow(["id": "sc-2", "state": "paused"])?.actions, ["resume", "cancel"])
        XCTAssertEqual(ScheduleRow(["id": "sc-3", "state": "done"])?.actions, ["cancel"])
        XCTAssertNil(ScheduleRow(["id": "sc-4", "last_result": NSNull()])?.lastResult)
    }

    func testRemindersNoticesAndMemory() throws {
        let reminder = try XCTUnwrap(ReminderRow(["id": "tm-1", "kind": "reminder", "at": "2026-10-25T18:00:00+00:00",
                                                  "message": "Call the bank"]))
        XCTAssertEqual(reminder.message, "Call the bank")
        let notice = try XCTUnwrap(NoticeRow(["id": "n-1", "text": "Your build finished.", "shown": true,
                                              "quotes": [["text": "BUILD OK", "origin": "external:observation"]],
                                              "at": "2026-10-25T10:00:00+00:00"]))
        XCTAssertEqual(notice.quotes, [NoticeQuote(text: "BUILD OK", origin: "external:observation")])
        XCTAssertTrue(notice.shown)
        let item = try XCTUnwrap(MemoryRow(["id": "m-1", "kind": "fact", "subject": "owner", "key": "editor",
                                            "content": "Uses VS Code", "status": "active", "origin": "owner_stated"]))
        XCTAssertEqual(item.content, "Uses VS Code")
    }

    func testSettingValuesKeepBooleansApartFromNumbers() throws {
        let json = try JSONSerialization.jsonObject(with: Data(#"""
            {"values": {"sense.window_titles": true, "retention.trace_days": 1,
                        "privacy.exclude_domains": ["example.com"]}}
            """#.utf8)) as! [String: Any]
        let snapshot = SettingsSnapshot(json)
        XCTAssertEqual(snapshot.values["sense.window_titles"], .bool(true))
        XCTAssertEqual(snapshot.values["retention.trace_days"], .int(1))       // a 1 from JSON stays a number
        XCTAssertTrue(snapshot.bool("sense.window_titles"))
        XCTAssertEqual(snapshot.int("retention.trace_days"), 1)
        XCTAssertEqual(snapshot.strings("privacy.exclude_domains"), ["example.com"])
        XCTAssertEqual(SettingValue(true)?.json as? Bool, true)
        XCTAssertEqual(SettingValue(["a", "b"])?.json as? [String], ["a", "b"])
    }

    func testHealthLinesFromTheDaemonsSelfTestAndContainment() {
        let lines = HealthLine.lines(["selftest": [["name": "accessibility", "ok": true, "detail": ""],
                                                   ["name": "screenshot", "ok": false, "detail": "no window"]],
                                      "containment": ["profile_ok": true, "proxy_alive": false, "reason": ""]])
        XCTAssertEqual(lines.map(\.name), ["accessibility", "screenshot", "Shell containment"])
        XCTAssertEqual(lines.map(\.ok), [true, false, false])
        XCTAssertEqual(lines[1].detail, "no window")
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `swift test --package-path ui/mac/FERBNotch --filter WindowRowsTests`
Expected: FAIL to build: `cannot find 'WindowTime' in scope`.

- [ ] **Step 3: Create `Sources/NotchCore/WindowRows.swift`**

```swift
import Foundation

/// The FERB window's rows (L34 Phase E), read from the daemon's replies. A row the daemon sent without its id is
/// dropped; every other missing field reads as empty, so an older daemon still shows what it has.
public enum WindowTime {
    /// The daemon's times are Python `isoformat()`: an offset, and microseconds unless they are zero.
    public static func parse(_ value: Any?) -> Date? {
        guard let text = value as? String, !text.isEmpty else { return nil }
        let full = ISO8601DateFormatter()
        full.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = full.date(from: text) { return date }
        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        return plain.date(from: text)
    }
}

private func text(_ value: Any?) -> String { value as? String ?? "" }
private func number(_ value: Any?) -> Int { (value as? NSNumber)?.intValue ?? 0 }
private func flag(_ value: Any?) -> Bool { (value as? NSNumber)?.boolValue ?? false }

/// A root task (`tasks.list`).
public struct TaskHistoryRow: Identifiable, Equatable {
    public let id: String
    public let goal: String
    public let status: String
    public let createdAt: Date?
    /// The daemon's own `created_at`, sent back as `before` for the next page.
    public let createdAtText: String
    public let answer: String
    public let verdict: String?

    public init?(_ row: [String: Any]) {
        guard let id = row["task_id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        goal = text(row["goal"])
        status = text(row["status"])
        createdAtText = text(row["created_at"])
        createdAt = WindowTime.parse(row["created_at"])
        answer = text(row["answer"])
        verdict = row["verdict"] as? String
    }

    /// Still running: the owner can stop it; a verdict is for a task that ended.
    public var isRunning: Bool { status == "running" || status == "planned" }
}

/// A schedule (`schedules.list`) with its latest finished run.
public struct ScheduleRow: Identifiable, Equatable {
    public struct LastResult: Equatable {
        public let at: Date?
        public let status: String
        public let answer: String
    }

    public let id: String
    public let request: String
    public let rule: String
    public let state: String
    public let nextRun: Date?
    public let failures: Int
    public let lastResult: LastResult?

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        request = text(row["request"])
        rule = text(row["rule"])
        state = text(row["state"])
        nextRun = WindowTime.parse(row["next_run"])
        failures = number(row["failures"])
        if let last = row["last_result"] as? [String: Any] {
            lastResult = LastResult(at: WindowTime.parse(last["at"]), status: text(last["status"]),
                                    answer: text(last["answer"]))
        } else {
            lastResult = nil
        }
    }

    /// What the owner may do now (`schedules.change` actions).
    public var actions: [String] {
        switch state {
        case "active": return ["pause", "cancel"]
        case "paused": return ["resume", "cancel"]
        default: return ["cancel"]
        }
    }
}

/// A pending reminder or timer (`reminders.list`).
public struct ReminderRow: Identifiable, Equatable {
    public let id: String
    public let kind: String
    public let at: Date?
    public let message: String

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        kind = text(row["kind"])
        at = WindowTime.parse(row["at"])
        message = text(row["message"])
    }
}

/// Something FERB noticed (`notices.list`); its quotes are outside text, shown as quotes (MP2).
public struct NoticeRow: Identifiable, Equatable {
    public let id: String
    public let text: String
    public let quotes: [NoticeQuote]
    public let at: Date?
    public let shown: Bool

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        self.text = row["text"] as? String ?? ""
        quotes = (row["quotes"] as? [[String: Any]] ?? []).compactMap { quote in
            guard let words = quote["text"] as? String, !words.isEmpty else { return nil }
            return NoticeQuote(text: words, origin: quote["origin"] as? String ?? "")
        }
        at = WindowTime.parse(row["at"])
        shown = flag(row["shown"])
    }
}

/// A memory item (`memory.list`).
public struct MemoryRow: Identifiable, Equatable {
    public let id: String
    public let kind: String
    public let subject: String
    public let key: String
    public let content: String
    public let status: String
    public let origin: String

    public init?(_ row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.id = id
        kind = text(row["kind"])
        subject = text(row["subject"])
        key = text(row["key"])
        content = text(row["content"])
        status = text(row["status"])
        origin = text(row["origin"])
    }
}

/// One setting's value (`settings.get`). JSON's booleans are told apart from its numbers.
public enum SettingValue: Equatable {
    case bool(Bool), int(Int), string(String), strings([String])

    public init?(_ any: Any) {
        switch any {
        case let words as String: self = .string(words)
        case let list as [String]: self = .strings(list)
        case let n as NSNumber where CFGetTypeID(n) == CFBooleanGetTypeID(): self = .bool(n.boolValue)
        case let n as NSNumber: self = .int(n.intValue)
        default: return nil
        }
    }

    public var json: Any {
        switch self {
        case .bool(let value): return value
        case .int(let value): return value
        case .string(let value): return value
        case .strings(let value): return value
        }
    }
}

public struct SettingsSnapshot: Equatable {
    public var values: [String: SettingValue]

    public init(values: [String: SettingValue] = [:]) { self.values = values }

    public init(_ reply: [String: Any]) {
        values = (reply["values"] as? [String: Any] ?? [:]).compactMapValues(SettingValue.init)
    }

    public func bool(_ key: String) -> Bool { if case .bool(let v)? = values[key] { return v }; return false }
    public func int(_ key: String) -> Int { if case .int(let v)? = values[key] { return v }; return 0 }
    public func strings(_ key: String) -> [String] { if case .strings(let v)? = values[key] { return v }; return [] }
}

/// One line of Settings → Advanced → Check FERB (`doctor.check` with its self-test).
public struct HealthLine: Identifiable, Equatable {
    public var id: String { name }
    public let name: String
    public let ok: Bool
    public let detail: String

    public static func lines(_ doctor: [String: Any]) -> [HealthLine] {
        var out = (doctor["selftest"] as? [[String: Any]] ?? []).compactMap { row -> HealthLine? in
            guard let name = row["name"] as? String else { return nil }
            return HealthLine(name: name, ok: flag(row["ok"]), detail: text(row["detail"]))
        }
        if let containment = doctor["containment"] as? [String: Any] {
            let ok = flag(containment["profile_ok"]) && flag(containment["proxy_alive"])
            let reason = text(containment["reason"])
            out.append(HealthLine(name: "Shell containment", ok: ok,
                                  detail: ok ? "" : (reason.isEmpty ? "sandbox or proxy down" : reason)))
        }
        return out
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `swift test --package-path ui/mac/FERBNotch --filter WindowRowsTests`
Expected: PASS. If `testDaemonTimesWithAndWithoutMicrosecondsParse` fails on the six-digit fraction, keep the test and
make `WindowTime.parse` cut the fraction to three digits before parsing (the test is the requirement).

- [ ] **Step 5: Commit**

```bash
git add ui/mac/FERBNotch/Sources/NotchCore/WindowRows.swift ui/mac/FERBNotch/Tests/NotchCoreTests/WindowRowsTests.swift
git diff --cached --stat
git commit -m "feat(L34): the FERB window's rows, read from the daemon's replies

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `FERBWindowStore` — one RPC per action

**Files:**
- Modify: `ui/mac/FERBNotch/Tests/NotchCoreTests/Support/ScriptedRPC.swift` (plan C's)
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/FERBWindowStore.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/FERBWindowStoreTests.swift`

**Interfaces:**
- Consumes: Task 4's rows; `DaemonRPCTransport`; `TaskRecordFiles` (`FERBNotch/TaskRecordFiles.swift`); plan D's
  `BannerCategory` and `notifications.categories`.
- Produces: `enum FERBSection: Hashable, CaseIterable, Identifiable { tasks, schedules, noticed, memory, settings }`
  (`title`, `symbol`); `@MainActor final class FERBWindowStore: ObservableObject` with `refresh(_:)`,
  `loadTasks(more:)`, `loadStopState()`, `loadSchedules()`, `loadNotices()`, `loadMemory()`, `loadSettings()`,
  `loadBanners()`, `setVerdict(_:_:)`, `stop(_:)`, `stopEverything()`, `resumeEverything()`, `change(_:_:)`,
  `cancel(_:)`, `confirm(_:)`, `retract(_:)`, `forgetPreview(_:) -> [String]?`, `forget(_:)`, `set(_:_:)`,
  `toggle(_:) -> Binding<Bool>`, `banner(_:) -> Binding<Bool>`, `checkRuntime(_:)`, `pauseRecording(_:)`,
  `checkHealth()`, `inspect(_:)`, `exportDiagnostic(appVersion:) -> URL?`; published `problem`, `note` and the lists.
- Produces (tests): `ScriptedRPC.queue(_:_:)` (replies given in order before `replies`) and `ScriptedRPC.waits`.

- [ ] **Step 1: A reply queue and recorded waits in plan C's `ScriptedRPC`**

In `Tests/NotchCoreTests/Support/ScriptedRPC.swift`, add to the class (keep everything plan C wrote):

```swift
    /// Replies given in order before `replies` answers (L34 E: a page, then the next; a preview, then the delete).
    var queued: [String: [[String: Any]]] = [:]
    /// The wait each caller asked for, by method (`call(_:params:wait:)`).
    private(set) var waits: [String: Int] = [:]

    func queue(_ method: String, _ items: [[String: Any]]) { queued[method, default: []] += items }

    func call(_ method: String, params: [String: Any], wait seconds: Int) async throws -> [String: Any] {
        waits[method] = seconds
        return try await call(method, params: params)
    }
```

and make the first lines of `call(_:params:)` after `recorded.append(method, params)`:

```swift
        if var next = queued[method], !next.isEmpty {
            let reply = next.removeFirst()
            queued[method] = next
            return reply
        }
```

- [ ] **Step 2: Write the failing tests** (create `Tests/NotchCoreTests/FERBWindowStoreTests.swift`)

```swift
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 Phase E: each window button is one RPC; a failure is the window's problem line, never a crash or a silent
/// empty list.
@MainActor
final class FERBWindowStoreTests: XCTestCase {
    private func task(_ id: String, _ status: String = "done", at: String = "2026-10-25T10:00:00+00:00") -> [String: Any] {
        ["task_id": id, "goal": "goal \(id)", "status": status, "created_at": at, "answer": "", "verdict": NSNull()]
    }

    func testRefreshTasksReadsTheDaemonsPageAndStopState() async {
        let rpc = ScriptedRPC()
        rpc.replies["tasks.list"] = ["tasks": [task("t-2"), task("t-1")], "more": true]
        rpc.replies["ping"] = ["stopped": true]
        let store = FERBWindowStore(rpc: rpc)
        await store.refresh(.tasks)
        XCTAssertEqual(store.tasks.map(\.id), ["t-2", "t-1"])
        XCTAssertTrue(store.moreTasks)
        XCTAssertTrue(store.stopped)
        XCTAssertNil(store.problem)
    }

    func testShowingOlderTasksAsksForTheNextPageBeforeTheLastRow() async {
        let rpc = ScriptedRPC()
        rpc.queue("tasks.list", [["tasks": [task("t-2", at: "2026-10-25T10:00:02+00:00")], "more": true],
                                 ["tasks": [task("t-1", at: "2026-10-25T10:00:01+00:00")], "more": false]])
        let store = FERBWindowStore(rpc: rpc)
        await store.loadTasks()
        await store.loadTasks(more: true)
        XCTAssertEqual(store.tasks.map(\.id), ["t-2", "t-1"])
        XCTAssertEqual(rpc.params(of: "tasks.list").last?["before"] as? String, "2026-10-25T10:00:02+00:00")
        XCTAssertFalse(store.moreTasks)
    }

    func testStoppingOneTaskSendsOnlyItsID() async {
        let rpc = ScriptedRPC()
        rpc.replies["tasks.list"] = ["tasks": [task("t-run", "running"), task("t-done")], "more": false]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadTasks()
        await store.stop(store.tasks[0])
        await store.stop(store.tasks[1])                      // ended: nothing to stop
        XCTAssertEqual(rpc.params(of: "task.stop").count, 1)
        XCTAssertEqual(rpc.params(of: "task.stop").first?["task_ids"] as? [String], ["t-run"])
    }

    func testVerdictsGoToVerdictSet() async {
        let rpc = ScriptedRPC()
        rpc.replies["tasks.list"] = ["tasks": [task("t-1")], "more": false]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadTasks()
        await store.setVerdict(store.tasks[0], "fail")
        XCTAssertEqual(rpc.params(of: "verdict.set").first?["task_id"] as? String, "t-1")
        XCTAssertEqual(rpc.params(of: "verdict.set").first?["verdict"] as? String, "fail")
    }

    func testStopEverythingAndResume() async {
        let rpc = ScriptedRPC()
        let store = FERBWindowStore(rpc: rpc)
        await store.stopEverything()
        XCTAssertTrue(store.stopped)
        await store.resumeEverything()
        XCTAssertFalse(store.stopped)
        XCTAssertEqual(rpc.methods(), ["stop.engage", "stop.lift"])
    }

    func testScheduleActionsAndReminderCancel() async {
        let rpc = ScriptedRPC()
        rpc.replies["schedules.list"] = ["schedules": [["id": "sc-1", "request": "Post", "state": "active"]]]
        rpc.replies["reminders.list"] = ["pending": [["id": "tm-1", "kind": "reminder", "message": "Call"]]]
        rpc.replies["schedules.change"] = ["text": "Paused: Post"]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadSchedules()
        await store.change(store.schedules[0], "pause")
        await store.cancel(store.reminders[0])
        XCTAssertEqual(rpc.params(of: "schedules.change").first?["id"] as? String, "sc-1")
        XCTAssertEqual(rpc.params(of: "schedules.change").first?["action"] as? String, "pause")
        XCTAssertEqual(rpc.params(of: "reminders.cancel").first?["id"] as? String, "tm-1")
    }

    func testForgetAsksWithTheDaemonsListBeforeDeleting() async throws {
        let rpc = ScriptedRPC()
        rpc.queue("memory.forget", [["confirmed": false, "will_delete": ["memory item “Uses VS Code”", "2 trail row(s)"]],
                                    ["confirmed": true, "deleted": [String]()]])
        let item = try XCTUnwrap(MemoryRow(["id": "m-1", "content": "Uses VS Code", "status": "active"]))
        let store = FERBWindowStore(rpc: rpc)
        let list = await store.forgetPreview(item)
        XCTAssertEqual(list, ["memory item “Uses VS Code”", "2 trail row(s)"])
        XCTAssertNil(rpc.params(of: "memory.forget").first?["confirm"])      // the preview deletes nothing
        await store.forget(item)
        XCTAssertEqual(rpc.params(of: "memory.forget").last?["confirm"] as? Bool, true)
    }

    func testAFailedSettingWriteRestoresTheShownValueAndSaysWhy() async {
        let rpc = ScriptedRPC()
        rpc.replies["settings.get"] = ["values": ["sense.window_titles": true]]
        rpc.failures["settings.set"] = DaemonRPCError.remote("sense.window_titles must be true or false")
        let store = FERBWindowStore(rpc: rpc)
        await store.loadSettings()
        await store.set("sense.window_titles", .bool(false))
        XCTAssertTrue(store.settings.bool("sense.window_titles"))
        XCTAssertEqual(store.problem, "sense.window_titles must be true or false")
    }

    func testASettingWriteShowsWhatTheDaemonStored() async {
        let rpc = ScriptedRPC()
        rpc.replies["settings.set"] = ["key": "privacy.exclude_domains", "value": ["example.com"]]
        let store = FERBWindowStore(rpc: rpc)
        await store.set("privacy.exclude_domains", .strings([".Example.COM"]))
        XCTAssertEqual(store.settings.strings("privacy.exclude_domains"), ["example.com"])
        XCTAssertEqual(rpc.params(of: "settings.set").first?["value"] as? [String], [".Example.COM"])
    }

    func testBannerSwitchesGoThroughPlanDsCategories() async {
        let rpc = ScriptedRPC()
        rpc.replies["notifications.categories"] = ["categories": ["question": true, "reminder": true, "alert": true,
                                                                  "task_finished": false, "loop": true]]
        let store = FERBWindowStore(rpc: rpc)
        await store.loadBanners()
        XCTAssertEqual(store.banners["task_finished"], false)
        await store.setBanner(.reminder, false)
        XCTAssertEqual(rpc.params(of: "notifications.categories").last?["set"] as? [String: Bool], ["reminder": false])
    }

    func testRefreshWithoutTheDaemonShowsWhyNotAnEmptyList() async {
        let rpc = ScriptedRPC()
        rpc.failures["schedules.list"] = DaemonRPCError.unavailable
        rpc.failures["reminders.list"] = DaemonRPCError.unavailable
        let store = FERBWindowStore(rpc: rpc)
        await store.refresh(.schedules)
        XCTAssertEqual(store.problem, DaemonRPCError.unavailable.localizedDescription)
        XCTAssertTrue(store.schedules.isEmpty)
    }

    func testCheckingTheRuntimeWaitsForItsAnswer() async {
        let rpc = ScriptedRPC()
        rpc.replies["runtimes.check"] = ["ok": false, "detail": "no answer"]
        let store = FERBWindowStore(rpc: rpc)
        await store.checkRuntime("codex")
        XCTAssertEqual(rpc.params(of: "runtimes.check").first?["runtime"] as? String, "codex")
        XCTAssertEqual(rpc.waits["runtimes.check"], FERBWindowStore.runtimeWait)
        XCTAssertEqual(store.note, "codex didn't answer: no answer")
    }

    func testExportingADiagnosticFileWaitsAndReturnsItsPath() async {
        let rpc = ScriptedRPC()
        rpc.replies["reports.diagnostic"] = ["path": "/tmp/FERB-diagnostic-2026-10-25-101500.txt", "summary": ""]
        let store = FERBWindowStore(rpc: rpc)
        let url = await store.exportDiagnostic(appVersion: "0.28.0 (1)")
        XCTAssertEqual(url?.path, "/tmp/FERB-diagnostic-2026-10-25-101500.txt")
        XCTAssertEqual(rpc.params(of: "reports.diagnostic").first?["app_version"] as? String, "0.28.0 (1)")
        XCTAssertEqual(rpc.waits["reports.diagnostic"], FERBWindowStore.diagnosticWait)
    }

    func testInspectWritesTheTraceAndOpensIt() async throws {
        let rpc = ScriptedRPC()
        rpc.replies["trace.get"] = ["task_id": "t-1", "spans": [[String: Any]]()]
        var opened: [URL] = []
        let store = FERBWindowStore(rpc: rpc)
        store.open = { opened.append($0) }
        let row = try XCTUnwrap(TaskHistoryRow(task("t-1")))
        await store.inspect(row)
        XCTAssertEqual(rpc.params(of: "trace.get").first?["task_id"] as? String, "t-1")
        XCTAssertEqual(opened.count, 1)
        XCTAssertEqual(opened.first?.pathExtension, "json")
    }
}
```

- [ ] **Step 3: Run them to see them fail**

Run: `swift test --package-path ui/mac/FERBNotch --filter FERBWindowStoreTests`
Expected: FAIL to build: `cannot find 'FERBWindowStore' in scope`.

- [ ] **Step 4: Create `Sources/FERBNotch/FERBWindowStore.swift`**

```swift
import AppKit
import SwiftUI
import NotchCore

/// The FERB window's sections (L34 Phase E). The notch stays the quick interface; this window holds the detail.
enum FERBSection: Hashable, CaseIterable, Identifiable {
    case tasks, schedules, noticed, memory, settings

    var id: FERBSection { self }

    var title: String {
        switch self {
        case .tasks: return "Tasks"
        case .schedules: return "Schedules & Reminders"
        case .noticed: return "Noticed"
        case .memory: return "Memory"
        case .settings: return "Settings"
        }
    }

    var symbol: String {
        switch self {
        case .tasks: return "checklist"
        case .schedules: return "calendar"
        case .noticed: return "eye"
        case .memory: return "brain"
        case .settings: return "gearshape"
        }
    }
}

/// The FERB window's data (L34 Phase E): each page reads the daemon when it opens, and each button is one RPC. Nothing
/// here decides anything; a refusal is shown as FERB's own words.
@MainActor
final class FERBWindowStore: ObservableObject {
    /// Seconds the app waits: a runtime's tiny check call, a diagnostic file's health check, the full self-test.
    static let runtimeWait = 60
    static let diagnosticWait = 60
    static let healthWait = 120

    let rpc: DaemonRPCTransport
    let records = TaskRecordFiles()
    /// Opens a file the owner asked to see (Inspect); tests replace it.
    var open: (URL) -> Void = { NSWorkspace.shared.open($0) }

    @Published var tasks: [TaskHistoryRow] = []
    @Published var moreTasks = false
    @Published var schedules: [ScheduleRow] = []
    @Published var reminders: [ReminderRow] = []
    @Published var notices: [NoticeRow] = []
    @Published var proposedMemory: [MemoryRow] = []
    @Published var activeMemory: [MemoryRow] = []
    @Published var settings = SettingsSnapshot()
    @Published var banners: [String: Bool] = [:]
    @Published var health: [HealthLine] = []
    @Published var stopped = false
    @Published var recordingPaused = false
    @Published var watchers: [String] = []
    /// Why the last request failed, in FERB's words (or the connection's); nil after a request that worked.
    @Published var problem: String?
    /// The result of the last action, one line ("Paused: …", "codex answered.").
    @Published var note: String?

    init(rpc: DaemonRPCTransport) { self.rpc = rpc }

    /// One RPC. A failure becomes the problem line and nil; never a crash or an empty list that reads as "nothing".
    @discardableResult
    func ask(_ method: String, _ params: [String: Any] = [:], wait: Int? = nil) async -> [String: Any]? {
        do {
            let reply: [String: Any]
            if let wait { reply = try await rpc.call(method, params: params, wait: wait) }
            else { reply = try await rpc.call(method, params: params) }
            problem = nil
            return reply
        } catch {
            problem = error.localizedDescription
            return nil
        }
    }

    func refresh(_ section: FERBSection) async {
        note = nil
        switch section {
        case .tasks: await loadTasks(); await loadStopState()
        case .schedules: await loadSchedules()
        case .noticed: await loadNotices()
        case .memory: await loadMemory()
        case .settings: await loadSettings()
        }
    }

    // MARK: Tasks

    func loadTasks(more: Bool = false) async {
        var params: [String: Any] = [:]
        if more, let last = tasks.last, !last.createdAtText.isEmpty { params["before"] = last.createdAtText }
        guard let reply = await ask("tasks.list", params) else { return }
        let rows = (reply["tasks"] as? [[String: Any]] ?? []).compactMap(TaskHistoryRow.init)
        tasks = more ? tasks + rows : rows
        moreTasks = reply["more"] as? Bool ?? false
    }

    func loadStopState() async {
        guard let reply = await ask("ping") else { return }
        stopped = reply["stopped"] as? Bool ?? false
    }

    func setVerdict(_ row: TaskHistoryRow, _ verdict: String) async {
        guard await ask("verdict.set", ["task_id": row.id, "verdict": verdict]) != nil else { return }
        await loadTasks()
    }

    /// Exactly this task: `task.stop` with no ids stops every running task.
    func stop(_ row: TaskHistoryRow) async {
        guard row.isRunning else { return }
        guard await ask("task.stop", ["task_ids": [row.id]]) != nil else { return }
        await loadTasks()
    }

    func stopEverything() async {
        guard await ask("stop.engage") != nil else { return }
        stopped = true
    }

    func resumeEverything() async {
        guard await ask("stop.lift") != nil else { return }
        stopped = false
    }

    func inspect(_ row: TaskHistoryRow) async {
        guard let trace = await ask("trace.get", ["task_id": row.id]) else { return }
        do { open(try await records.write(trace)) } catch { problem = error.localizedDescription }
    }

    // MARK: Schedules & Reminders

    func loadSchedules() async {
        if let reply = await ask("schedules.list") {
            schedules = (reply["schedules"] as? [[String: Any]] ?? []).compactMap(ScheduleRow.init)
        }
        if let reply = await ask("reminders.list") {
            reminders = (reply["pending"] as? [[String: Any]] ?? []).compactMap(ReminderRow.init)
        }
    }

    func change(_ schedule: ScheduleRow, _ action: String) async {
        guard let reply = await ask("schedules.change", ["id": schedule.id, "action": action]) else { return }
        await loadSchedules()
        note = reply["text"] as? String
    }

    func cancel(_ reminder: ReminderRow) async {
        guard await ask("reminders.cancel", ["id": reminder.id]) != nil else { return }
        await loadSchedules()
    }

    // MARK: Noticed

    func loadNotices() async {
        guard let reply = await ask("notices.list") else { return }
        notices = (reply["notices"] as? [[String: Any]] ?? []).compactMap(NoticeRow.init)
    }

    // MARK: Memory

    func loadMemory() async {
        if let reply = await ask("memory.list", ["status": "proposed"]) {
            proposedMemory = (reply["items"] as? [[String: Any]] ?? []).compactMap(MemoryRow.init)
        }
        if let reply = await ask("memory.list", ["status": "active"]) {
            activeMemory = (reply["items"] as? [[String: Any]] ?? []).compactMap(MemoryRow.init)
        }
    }

    func confirm(_ item: MemoryRow) async {
        guard await ask("memory.confirm", ["id": item.id]) != nil else { return }
        await loadMemory()
    }

    func retract(_ item: MemoryRow) async {
        guard await ask("memory.retract", ["id": item.id, "reason": "the owner, in FERB's window"]) != nil else { return }
        await loadMemory()
    }

    /// Forget is permanent: first the daemon's own list of what would go, for the owner to read.
    func forgetPreview(_ item: MemoryRow) async -> [String]? {
        guard let reply = await ask("memory.forget", ["id": item.id]) else { return nil }
        return reply["will_delete"] as? [String] ?? []
    }

    /// Only after the owner's Forget on that list.
    func forget(_ item: MemoryRow) async {
        guard await ask("memory.forget", ["id": item.id, "confirm": true]) != nil else { return }
        await loadMemory()
    }

    // MARK: Settings (AI, Permissions and Browsers are plan C's SetupModel)

    func loadSettings() async {
        if let reply = await ask("settings.get") { settings = SettingsSnapshot(reply) }
        await loadBanners()
        if let reply = await ask("sense.status") {
            recordingPaused = reply["paused"] as? Bool ?? false
            watchers = reply["watchers"] as? [String] ?? []
        }
    }

    /// Shown at once; put back if FERB refuses, and then shows what FERB stored (it may normalize it).
    func set(_ key: String, _ value: SettingValue) async {
        let before = settings.values[key]
        settings.values[key] = value
        guard let reply = await ask("settings.set", ["key": key, "value": value.json]) else {
            settings.values[key] = before
            return
        }
        if let stored = reply["value"].flatMap(SettingValue.init) { settings.values[key] = stored }
    }

    func toggle(_ key: String) -> Binding<Bool> {
        Binding(get: { self.settings.bool(key) }, set: { on in Task { await self.set(key, .bool(on)) } })
    }

    /// Plan D's banner switches (config `[banners]`, through `notifications.categories`).
    func loadBanners() async {
        guard let reply = await ask("notifications.categories") else { return }
        banners = reply["categories"] as? [String: Bool] ?? [:]
    }

    func setBanner(_ category: BannerCategory, _ on: Bool) async {
        guard let reply = await ask("notifications.categories", ["set": [category.rawValue: on]]) else { return }
        banners = reply["categories"] as? [String: Bool] ?? banners
    }

    func banner(_ category: BannerCategory) -> Binding<Bool> {
        Binding(get: { self.banners[category.rawValue] ?? true },
                set: { on in Task { await self.setBanner(category, on) } })
    }

    func checkRuntime(_ name: String) async {
        guard let reply = await ask("runtimes.check", ["runtime": name], wait: Self.runtimeWait) else { return }
        let ok = reply["ok"] as? Bool ?? false
        note = ok ? "\(name) answered." : "\(name) didn't answer: \(reply["detail"] as? String ?? "no answer")"
    }

    func pauseRecording(_ paused: Bool) async {
        guard let reply = await ask(paused ? "sense.pause" : "sense.resume") else { return }
        recordingPaused = reply["paused"] as? Bool ?? paused
    }

    func checkHealth() async {
        guard let reply = await ask("doctor.check", ["selftest": true], wait: Self.healthWait) else { return }
        health = HealthLine.lines(reply)
    }

    func exportDiagnostic(appVersion: String?) async -> URL? {
        var params: [String: Any] = [:]
        if let appVersion { params["app_version"] = appVersion }
        guard let reply = await ask("reports.diagnostic", params, wait: Self.diagnosticWait),
              let path = reply["path"] as? String else { return nil }
        return URL(fileURLWithPath: path)
    }
}
```

- [ ] **Step 5: Run the tests to see them pass**

Run: `swift test --package-path ui/mac/FERBNotch --filter FERBWindowStoreTests`, then
`swift test --package-path ui/mac/FERBNotch` (plan C's and D's tests still use `ScriptedRPC` unchanged).
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ui/mac/FERBNotch/Tests/NotchCoreTests/Support/ScriptedRPC.swift ui/mac/FERBNotch/Sources/FERBNotch/FERBWindowStore.swift ui/mac/FERBNotch/Tests/NotchCoreTests/FERBWindowStoreTests.swift
git diff --cached --stat
git commit -m "feat(L34): FERBWindowStore: one daemon request per window action, failures shown, never an empty guess

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The window: sidebar, Tasks, Schedules & Reminders, Noticed, Memory, Settings → General; menu and ⌘,

**Files:**
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/FERBWindow.swift` (controller, selection, actions, root view)
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/WindowPages.swift` (Tasks, Schedules, Noticed, Memory)
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/SettingsPages.swift` (`SettingsTab`, `SettingsView`,
  `GeneralSettingsPage`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/SoundFeedback.swift` (delete `FeedbackSettingsView`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/PanelCoordinator.swift` (as plan D left it: `cursor`, `settingsWindow`,
  `showSettings()`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/AppSetupController.swift` (as plan C rewrote it: `model`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/Application.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/FERBWindowViewTests.swift`

**Interfaces:**
- Consumes: Task 5's `FERBWindowStore`, `FERBSection`; plan C's `AppSetupController`, `SetupModel`; plan D's
  `PanelCoordinator.hasWindow`, `onOpenWindow`, `showSettings()`.
- Produces: `@MainActor final class FERBWindowController` (`init(store:actions:)`, `show(_:tab:)`, `select(_:tab:)`,
  `selection`); `@MainActor final class FERBWindowSelection: ObservableObject` (`section: FERBSection?`,
  `settingsTab: SettingsTab`); `struct FERBAppActions` (`feedback`, `cursor`, `appVersion`, `setup: SetupModel?`,
  `openSetup`, `runFerb`, `beforeUninstall`); `struct FERBWindowView`;
  `enum SettingsTab { general, ai, permissions, browsers, notifications, privacy, advanced }`; `struct SettingsView`;
  `struct GeneralSettingsPage` (sections "Notch", "FERB's pointer", "Sounds"); `PanelCoordinator.onOpenSettings`;
  `PanelCoordinator.cursor` (internal); `AppSetupController.model` (internal).

- [ ] **Step 1: Write the failing tests** (create `Tests/NotchCoreTests/FERBWindowViewTests.swift`)

```swift
import AppKit
import SwiftUI
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 Phase E: every section and Settings tab builds and lays out with real rows, and ⌘, lands on Settings.
@MainActor
final class FERBWindowViewTests: XCTestCase {
    func filledStore() -> FERBWindowStore {
        let store = FERBWindowStore(rpc: ScriptedRPC())
        store.tasks = [TaskHistoryRow(["task_id": "t-1", "goal": "Rename the invoices", "status": "done",
                                       "answer": "Renamed 4 files."])!,
                       TaskHistoryRow(["task_id": "t-2", "goal": "Watch the build", "status": "running"])!]
        store.schedules = [ScheduleRow(["id": "sc-1", "request": "Post the update", "rule": "every day at 09:00",
                                        "state": "active", "last_result": ["status": "done", "answer": "Posted."]])!]
        store.reminders = [ReminderRow(["id": "tm-1", "kind": "reminder", "message": "Call the bank"])!]
        store.notices = [NoticeRow(["id": "n-1", "text": "Your build finished.",
                                    "quotes": [["text": "BUILD OK", "origin": "external:observation"]]])!]
        store.proposedMemory = [MemoryRow(["id": "m-1", "content": "Prefers mornings", "status": "proposed"])!]
        store.activeMemory = [MemoryRow(["id": "m-2", "content": "Uses VS Code", "status": "active"])!]
        store.settings = SettingsSnapshot(["values": ["sense.window_titles": true, "retention.trace_days": 30,
                                                      "privacy.exclude_apps": ["com.1password.1password"]]])
        store.banners = ["question": true, "reminder": false]
        return store
    }

    func actions() -> FERBAppActions {
        FERBAppActions(feedback: SoundFeedback(), cursor: CursorController(makePanels: false))
    }

    func lays(_ view: some View) {
        let hosting = NSHostingView(rootView: view.frame(width: 900, height: 600))
        hosting.frame = NSRect(x: 0, y: 0, width: 900, height: 600)
        hosting.layout()
        XCTAssertGreaterThan(hosting.fittingSize.width, 0)
    }

    func testEverySectionLaysOut() {
        let store = filledStore()
        for section in FERBSection.allCases {
            let selection = FERBWindowSelection()
            selection.section = section
            lays(FERBWindowView(store: store, selection: selection, actions: actions()))
        }
    }

    func testSettingsLaysOutAtGeneral() {
        let selection = FERBWindowSelection()
        selection.section = .settings
        lays(SettingsView(store: filledStore(), selection: selection, actions: actions()))
        lays(GeneralSettingsPage(feedback: SoundFeedback(), cursor: CursorController(makePanels: false)))
    }

    func testSettingsOpensTheWindowAtTheTabAsked() {
        let controller = FERBWindowController(store: filledStore(), actions: actions())
        controller.select(.settings, tab: .privacy)
        XCTAssertEqual(controller.selection.section, .settings)
        XCTAssertEqual(controller.selection.settingsTab, .privacy)
        controller.select(.tasks)
        XCTAssertEqual(controller.selection.section, .tasks)
        XCTAssertEqual(controller.selection.settingsTab, .privacy)        // the tab is kept for next time
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `swift test --package-path ui/mac/FERBNotch --filter FERBWindowViewTests`
Expected: FAIL to build: `cannot find 'FERBWindowView' in scope`.

- [ ] **Step 3: Create `Sources/FERBNotch/FERBWindow.swift`**

```swift
import AppKit
import SwiftUI
import NotchCore

@MainActor
final class FERBWindowSelection: ObservableObject {
    @Published var section: FERBSection? = .tasks
    @Published var settingsTab: SettingsTab = .general
}

/// What Settings needs from the rest of the app. Plan C's setup model and its `ferb` runner exist only in the installed
/// FERB.app: in a development build they are nil and their pages say so.
@MainActor
struct FERBAppActions {
    let feedback: SoundFeedback
    let cursor: CursorController
    var appVersion: String? = NotchModel.appVersion
    /// Plan C's: Settings → AI, Permissions and Browsers embed its step views on this model.
    var setup: SetupModel?
    /// Set Up FERB… (plan C's setup window).
    var openSetup: (() -> Void)?
    /// Runs FERB.app's own `ferb` with these arguments (plan C's runner, one at a time): each output line, then the exit
    /// status. False when another run is under way.
    var runFerb: ((_ arguments: [String], _ line: @escaping (String) -> Void,
                   _ done: @escaping (Int32) -> Void) -> Bool)?
    /// Runs before `ferb uninstall` (plan F removes FERB.app's login item here).
    var beforeUninstall: (() -> Void)?
}

/// The regular FERB window (L34 Phase E). It opens only when the owner asks (menu bar, ⌘,): their own foreground
/// action (rule 17).
@MainActor
final class FERBWindowController: NSObject, NSWindowDelegate {
    let store: FERBWindowStore
    let selection = FERBWindowSelection()
    private let actions: FERBAppActions
    private var window: NSWindow?

    init(store: FERBWindowStore, actions: FERBAppActions) {
        self.store = store
        self.actions = actions
    }

    func select(_ section: FERBSection, tab: SettingsTab? = nil) {
        selection.section = section
        if let tab { selection.settingsTab = tab }
    }

    func show(_ section: FERBSection, tab: SettingsTab? = nil) {
        select(section, tab: tab)
        if window == nil {
            let created = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 600),
                                   styleMask: [.titled, .closable, .miniaturizable, .resizable],
                                   backing: .buffered, defer: false)
            created.title = "FERB"
            created.isReleasedWhenClosed = false
            created.delegate = self
            created.contentView = NSHostingView(rootView: FERBWindowView(store: store, selection: selection,
                                                                         actions: actions))
            created.center()
            created.setFrameAutosaveName("FERBWindow")
            window = created
        }
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}

struct FERBWindowView: View {
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var selection: FERBWindowSelection
    let actions: FERBAppActions

    var body: some View {
        NavigationSplitView {
            List(FERBSection.allCases, selection: $selection.section) { section in
                Label(section.title, systemImage: section.symbol)
            }
            .navigationSplitViewColumnWidth(min: 170, ideal: 190)
        } detail: {
            VStack(alignment: .leading, spacing: 8) {
                if let problem = store.problem { Text(problem).foregroundStyle(.red).textSelection(.enabled) }
                if let note = store.note { Text(note).foregroundStyle(.secondary).textSelection(.enabled) }
                page
            }
            .padding(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .task(id: selection.section) { await store.refresh(selection.section ?? .tasks) }
        }
        .frame(minWidth: 760, minHeight: 480)
    }

    @ViewBuilder private var page: some View {
        switch selection.section ?? .tasks {
        case .tasks: TasksPage(store: store)
        case .schedules: SchedulesPage(store: store)
        case .noticed: NoticedPage(store: store)
        case .memory: MemoryPage(store: store)
        case .settings: SettingsView(store: store, selection: selection, actions: actions)
        }
    }
}
```

- [ ] **Step 4: Create `Sources/FERBNotch/WindowPages.swift` and `Sources/FERBNotch/SettingsPages.swift`**

`WindowPages.swift`:

```swift
import SwiftUI
import NotchCore

private func when(_ date: Date?) -> String { date?.formatted(date: .abbreviated, time: .shortened) ?? "" }
private func words(_ status: String) -> String { status.replacingOccurrences(of: "_", with: " ") }

struct TasksPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("Tasks").font(.title2)
                Spacer()
                if store.stopped {
                    Text("FERB is stopped").foregroundStyle(.secondary)
                    Button("Resume FERB") { Task { await store.resumeEverything() } }
                } else {
                    Button("Stop everything") { Task { await store.stopEverything() } }
                }
                Button("Refresh") { Task { await store.loadTasks() } }
            }
            List {
                if store.tasks.isEmpty { Text("No tasks yet.").foregroundStyle(.secondary) }
                ForEach(store.tasks) { row in TaskHistoryRowView(row: row, store: store) }
                if store.moreTasks { Button("Show older tasks") { Task { await store.loadTasks(more: true) } } }
            }
        }
    }
}

struct TaskHistoryRowView: View {
    let row: TaskHistoryRow
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(row.goal.isEmpty ? row.id : row.goal).bold().lineLimit(2)
                Spacer()
                Text(words(row.status)).foregroundStyle(.secondary)
            }
            Text(when(row.createdAt)).font(.caption).foregroundStyle(.secondary)
            if !row.answer.isEmpty { Text(row.answer).lineLimit(4).textSelection(.enabled) }
            HStack {
                Button("Inspect") { Task { await store.inspect(row) } }
                if row.isRunning {
                    Button("Stop") { Task { await store.stop(row) } }
                } else {
                    Button("Pass") { Task { await store.setVerdict(row, "pass") } }
                    Button("Fail") { Task { await store.setVerdict(row, "fail") } }
                    if let verdict = row.verdict { Text("Your verdict: \(verdict)").foregroundStyle(.secondary) }
                }
            }
        }
        .padding(.vertical, 4)
    }
}

struct SchedulesPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        List {
            Section("Schedules") {
                if store.schedules.isEmpty {
                    Text("No schedules. Ask FERB in the notch, e.g. “every weekday at 9, …”.").foregroundStyle(.secondary)
                }
                ForEach(store.schedules) { s in
                    VStack(alignment: .leading, spacing: 4) {
                        Text(s.request).bold()
                        Text("\(s.rule) · \(s.state)").foregroundStyle(.secondary)
                        if let next = s.nextRun { Text("Next run: \(when(next))") }
                        if let last = s.lastResult {
                            Text("Last run (\(words(last.status))): \(last.answer)").lineLimit(3).textSelection(.enabled)
                        }
                        if s.failures > 0 { Text("Failed \(s.failures) time(s) in a row").foregroundStyle(.red) }
                        HStack {
                            ForEach(s.actions, id: \.self) { action in
                                Button(action.capitalized) { Task { await store.change(s, action) } }
                            }
                        }
                    }
                    .padding(.vertical, 4)
                }
            }
            Section("Reminders") {
                if store.reminders.isEmpty { Text("No reminders waiting.").foregroundStyle(.secondary) }
                ForEach(store.reminders) { r in
                    HStack {
                        VStack(alignment: .leading) {
                            Text(r.message)
                            Text(when(r.at)).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Cancel") { Task { await store.cancel(r) } }
                    }
                }
            }
        }
    }
}

struct NoticedPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        List {
            if store.notices.isEmpty { Text("FERB hasn't noticed anything yet.").foregroundStyle(.secondary) }
            ForEach(store.notices) { n in
                VStack(alignment: .leading, spacing: 4) {
                    Text(n.text).textSelection(.enabled)
                    // Outside text FERB quotes is data, shown as a quote (MP2)
                    ForEach(Array(n.quotes.enumerated()), id: \.offset) { _, quote in
                        Text("“\(quote.text)”").italic().foregroundStyle(.secondary).textSelection(.enabled)
                    }
                    Text(when(n.at)).font(.caption).foregroundStyle(.secondary)
                }
                .padding(.vertical, 4)
            }
        }
    }
}

struct MemoryPage: View {
    @ObservedObject var store: FERBWindowStore
    @State private var forgetting: MemoryRow?
    @State private var willDelete: [String] = []

    var body: some View {
        List {
            Section("Waiting for you to confirm") {
                if store.proposedMemory.isEmpty { Text("Nothing waiting.").foregroundStyle(.secondary) }
                ForEach(store.proposedMemory) { item in
                    HStack {
                        Text(item.content).textSelection(.enabled)
                        Spacer()
                        Button("Confirm") { Task { await store.confirm(item) } }
                        Button("Not right") { Task { await store.retract(item) } }
                    }
                }
            }
            Section("What FERB knows") {
                if store.activeMemory.isEmpty { Text("FERB doesn't know anything about you yet.").foregroundStyle(.secondary) }
                ForEach(store.activeMemory) { item in
                    HStack {
                        VStack(alignment: .leading) {
                            Text(item.content).textSelection(.enabled)
                            Text("\(item.kind) · \(item.subject)").font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Forget…") {
                            Task {
                                if let list = await store.forgetPreview(item) { willDelete = list; forgetting = item }
                            }
                        }
                    }
                }
            }
        }
        .alert("Forget this for good?", isPresented: Binding(get: { forgetting != nil },
                                                               set: { if !$0 { forgetting = nil } }),
               presenting: forgetting) { item in
            Button("Forget", role: .destructive) { Task { await store.forget(item); forgetting = nil } }
            Button("Cancel", role: .cancel) { forgetting = nil }
        } message: { _ in
            Text(willDelete.isEmpty ? "FERB deletes this memory." : "FERB deletes: " + willDelete.joined(separator: "; "))
        }
    }
}
```

`SettingsPages.swift` (this task's `SettingsView` has the General tab; Task 7 adds the other six):

```swift
import AppKit
import SwiftUI
import NotchCore

enum SettingsTab: Hashable, CaseIterable, Identifiable {
    case general, ai, permissions, browsers, notifications, privacy, advanced

    var id: SettingsTab { self }

    var title: String {
        switch self {
        case .general: return "General"
        case .ai: return "AI"
        case .permissions: return "Permissions"
        case .browsers: return "Browsers"
        case .notifications: return "Notifications"
        case .privacy: return "Privacy"
        case .advanced: return "Advanced"
        }
    }
}

struct SettingsView: View {
    @ObservedObject var store: FERBWindowStore
    @ObservedObject var selection: FERBWindowSelection
    let actions: FERBAppActions

    var body: some View {
        TabView(selection: $selection.settingsTab) {
            GeneralSettingsPage(feedback: actions.feedback, cursor: actions.cursor)
                .tabItem { Text(SettingsTab.general.title) }.tag(SettingsTab.general)
        }
    }
}

/// Settings → General: the notch's shortcuts, FERB's pointer and sounds (moved here from the old sound window). Plans
/// F and G add "Startup" (open at login) and "Updates".
struct GeneralSettingsPage: View {
    @ObservedObject var feedback: SoundFeedback
    let cursor: CursorController
    @AppStorage("cursor.enabled") private var cursorEnabled = true

    var body: some View {
        Form {
            Section("Notch") {
                LabeledContent("Open / close FERB", value: "Double-tap right Option, or Control–Option–Space")
                LabeledContent("Dictation", value: "Hold right Command + Option")
                LabeledContent("Submit / new line", value: "Return / Shift–Return")
            }
            Section("FERB's pointer") {
                Toggle("Show FERB's pointer when it acts on screen", isOn: $cursorEnabled)
                    .onChange(of: cursorEnabled) { _, on in cursor.enabled = on }
            }
            Section("Sounds") {
                Toggle("Play feedback sounds", isOn: $feedback.enabled)
                Slider(value: $feedback.volume, in: 0...0.5) { Text("Volume") }.disabled(!feedback.enabled)
                Button("Play completion sound") { feedback.play(.completed) }.disabled(!feedback.enabled)
            }
        }
        .formStyle(.grouped)
    }
}
```

- [ ] **Step 5: Wire the menu bar, ⌘, and the shared setup model**

`SoundFeedback.swift`: delete `struct FeedbackSettingsView` (the whole struct); its controls now live in
`GeneralSettingsPage`.

`AppSetupController.swift` (plan C's version): change `private lazy var model = SetupModel(` to
`lazy var model = SetupModel(` — Settings → AI, Permissions and Browsers share this one model with the setup window.

`PanelCoordinator.swift` (plan D's version):
- Delete `    private var settingsWindow: NSWindow?`.
- Next to plan D's `var onOpenWindow: (() -> Void)?`, add:

```swift
    var onOpenSettings: (() -> Void)?            // the FERB window at Settings (L34 Phase E): ⌘, and Settings…
```

- Change `    private let cursor = CursorController()` to `    let cursor = CursorController()`.
- Replace the body of `@objc private func showSettings()` (the old "FERB Settings" window) with:

```swift
    @objc private func showSettings() {
        if !model.isPinned { model.collapse() }
        onOpenSettings?()
    }
```

`Application.swift`: add a property `    private var ferbWindow: FERBWindowController?`. After plan C's
`appSetup.onFinished = { ... }` line and before `coordinator?.start()`, add:

```swift
        if let coordinator {
            var actions = FERBAppActions(feedback: coordinator.model.feedback, cursor: coordinator.cursor)
            if appSetup.isPackaged {
                let setup = appSetup.model
                actions.setup = setup
                actions.openSetup = { [appSetup] in appSetup.runSetupFromMenu() }
                actions.runFerb = { [setup] arguments, line, done in setup.runner.run(arguments, line: line, done: done) }
            }
            let window = FERBWindowController(store: FERBWindowStore(rpc: coordinator.model.rpc), actions: actions)
            coordinator.hasWindow = true                      // plan D's menu shows Open FERB Window
            coordinator.onOpenWindow = { [window] in window.show(.tasks) }
            coordinator.onOpenSettings = { [window] in window.show(.settings) }
            ferbWindow = window
        }
```

- [ ] **Step 6: Run the tests to see them pass**

Run: `swift test --package-path ui/mac/FERBNotch --filter FERBWindowViewTests`
Expected: PASS. Then `swift test --package-path ui/mac/FERBNotch` (nothing else used `FeedbackSettingsView`).

- [ ] **Step 7: Commit**

```bash
git add ui/mac/FERBNotch/Sources/FERBNotch/FERBWindow.swift ui/mac/FERBNotch/Sources/FERBNotch/WindowPages.swift ui/mac/FERBNotch/Sources/FERBNotch/SettingsPages.swift ui/mac/FERBNotch/Sources/FERBNotch/SoundFeedback.swift ui/mac/FERBNotch/Sources/FERBNotch/PanelCoordinator.swift ui/mac/FERBNotch/Sources/FERBNotch/AppSetupController.swift ui/mac/FERBNotch/Sources/FERBNotch/Application.swift ui/mac/FERBNotch/Tests/NotchCoreTests/FERBWindowViewTests.swift
git diff --cached --stat
git commit -m "feat(L34): the FERB window: Tasks, Schedules & Reminders, Noticed, Memory and Settings → General; ⌘, opens it

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Settings → AI, Permissions, Browsers, Notifications, Privacy, Advanced

**Files:**
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/SettingsPages.swift`
- Modify: `ui/mac/FERBNotch/Tests/NotchCoreTests/FERBWindowViewTests.swift`

**Interfaces:**
- Consumes: Task 5 store methods; Task 6 `FERBAppActions`, `SettingsTab`, `GeneralSettingsPage`; plan C's
  `AIStepView`, `PermissionsStepView`, `BrowsersStepView`, `SetupModel` (`ai?.current`, `refreshAI()`,
  `refreshBrowsers()`, `startPolling()`, `stopPolling()`); plan D's `BannerCategory`.
- Produces: `AISettingsPage`, `PermissionsSettingsPage`, `BrowsersSettingsPage`, `NotificationsSettingsPage`,
  `PrivacySettingsPage`, `AdvancedSettingsPage`, `StringListEditor`, `DaysStepper`, `FerbOutput`.

- [ ] **Step 1: Add the failing tests** (append inside `FERBWindowViewTests`)

```swift
    func testEverySettingsPageLaysOutInADevelopmentBuild() {
        let store = filledStore()
        lays(AISettingsPage(store: store, actions: actions()))
        lays(PermissionsSettingsPage(actions: actions()))
        lays(BrowsersSettingsPage(actions: actions()))
        lays(NotificationsSettingsPage(store: store))
        lays(PrivacySettingsPage(store: store))
        lays(AdvancedSettingsPage(store: store, actions: actions()))
        for tab in SettingsTab.allCases {
            let selection = FERBWindowSelection()
            selection.section = .settings
            selection.settingsTab = tab
            lays(SettingsView(store: store, selection: selection, actions: actions()))
        }
    }

    func testFERBsOwnCommandsShowTheirOutputAndUninstallRunsItsHookFirst() {
        var order: [String] = []
        var packaged = actions()
        packaged.beforeUninstall = { order.append("login item") }
        packaged.runFerb = { arguments, line, done in
            order.append(arguments.joined(separator: " "))
            line("`ferb` command: /Users/o/.local/bin/ferb")
            done(0)
            return true
        }
        let output = FerbOutput()
        output.run(["setup", "--cli"], with: packaged)
        XCTAssertEqual(output.lines, ["`ferb` command: /Users/o/.local/bin/ferb"])
        XCTAssertEqual(output.status, 0)
        output.uninstall(with: packaged)
        XCTAssertEqual(order, ["setup --cli", "login item", "uninstall"])
        XCTAssertNil(actions().runFerb)                                    // a development build runs nothing
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `swift test --package-path ui/mac/FERBNotch --filter FERBWindowViewTests`
Expected: FAIL to build (`AISettingsPage`, `FerbOutput` … missing).

- [ ] **Step 3: The six pages** (`SettingsPages.swift`)

Replace `SettingsView.body` with:

```swift
    var body: some View {
        TabView(selection: $selection.settingsTab) {
            GeneralSettingsPage(feedback: actions.feedback, cursor: actions.cursor)
                .tabItem { Text(SettingsTab.general.title) }.tag(SettingsTab.general)
            AISettingsPage(store: store, actions: actions)
                .tabItem { Text(SettingsTab.ai.title) }.tag(SettingsTab.ai)
            PermissionsSettingsPage(actions: actions)
                .tabItem { Text(SettingsTab.permissions.title) }.tag(SettingsTab.permissions)
            BrowsersSettingsPage(actions: actions)
                .tabItem { Text(SettingsTab.browsers.title) }.tag(SettingsTab.browsers)
            NotificationsSettingsPage(store: store)
                .tabItem { Text(SettingsTab.notifications.title) }.tag(SettingsTab.notifications)
            PrivacySettingsPage(store: store)
                .tabItem { Text(SettingsTab.privacy.title) }.tag(SettingsTab.privacy)
            AdvancedSettingsPage(store: store, actions: actions)
                .tabItem { Text(SettingsTab.advanced.title) }.tag(SettingsTab.advanced)
        }
    }
```

Append to the file:

```swift
/// Output of FERB.app's own `ferb` commands run from Settings (plan C's runner): each line as it arrives, then the exit
/// status. Uninstall runs plan F's hook first.
@MainActor
final class FerbOutput: ObservableObject {
    @Published private(set) var lines: [String] = []
    @Published private(set) var status: Int32?
    @Published private(set) var running = false

    func run(_ arguments: [String], with actions: FERBAppActions, then: (() -> Void)? = nil) {
        guard let runFerb = actions.runFerb else { return }
        lines = []
        status = nil
        running = true
        let started = runFerb(arguments, { [weak self] line in self?.lines.append(line) },
                              { [weak self] code in
                                  self?.status = code
                                  self?.running = false
                                  then?()
                              })
        if !started {
            running = false
            lines = ["FERB is already running a setup step; try again when it finishes."]
        }
    }

    func uninstall(with actions: FERBAppActions) {
        actions.beforeUninstall?()
        run(["uninstall"], with: actions)
    }
}

private struct OutputLines: View {
    @ObservedObject var output: FerbOutput
    var body: some View {
        ForEach(Array(output.lines.enumerated()), id: \.offset) { _, line in
            Text(line).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
        }
    }
}

private struct OnlyInTheApp: View {
    var body: some View {
        Text("This is set in the installed FERB.app.").foregroundStyle(.secondary)
    }
}

/// Settings → AI: plan C's Choose your AI (sign in with ChatGPT, use Claude, an API key into the Keychain), whether the
/// service answers, and signing FERB's own Codex out. This page never shows or sends a key (rule 14).
struct AISettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    let actions: FERBAppActions
    @StateObject private var output = FerbOutput()

    var body: some View {
        if let setup = actions.setup {
            Form {
                Section { AIStepView(model: setup) }
                Section("Check") {
                    Button("Check connection") { Task { await store.checkRuntime(setup.ai?.current ?? "codex") } }
                    Button("Sign out of ChatGPT") {
                        output.run(["setup", "--codex-logout"], with: actions) { Task { await setup.refreshAI() } }
                    }
                    .disabled(output.running)
                    OutputLines(output: output)
                }
            }
            .formStyle(.grouped)
            .task { await setup.refreshAI() }
        } else {
            Form { OnlyInTheApp() }.formStyle(.grouped)
        }
    }
}

/// Settings → Permissions: plan C's rows, each as FERB's background service (or FERB.app) itself sees it, with Grant
/// and the System Settings pane; it re-reads while shown.
struct PermissionsSettingsPage: View {
    let actions: FERBAppActions

    var body: some View {
        if let setup = actions.setup {
            Form { PermissionsStepView(model: setup) }
                .formStyle(.grouped)
                .onAppear { setup.startPolling() }
                .onDisappear { setup.stopPolling() }
        } else {
            Form { OnlyInTheApp() }.formStyle(.grouped)
        }
    }
}

/// Settings → Browsers: plan C's browsers and profiles (not installed / installed / on / paired) and Add to….
struct BrowsersSettingsPage: View {
    let actions: FERBAppActions

    var body: some View {
        if let setup = actions.setup {
            Form { BrowsersStepView(model: setup) }
                .formStyle(.grouped)
                .task { await setup.refreshBrowsers() }
        } else {
            Form { OnlyInTheApp() }.formStyle(.grouped)
        }
    }
}

/// Settings → Notifications: which kinds FERB posts while the notch is closed (plan D's categories).
struct NotificationsSettingsPage: View {
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        Form {
            Section("Notify me about") {
                ForEach(BannerCategory.allCases, id: \.rawValue) { category in
                    Toggle(category.title, isOn: store.banner(category))
                }
            }
            Section {
                if let url = URL(string: "x-apple.systempreferences:com.apple.preference.notifications") {
                    Button("Open macOS notification settings") { NSWorkspace.shared.open(url) }
                }
            }
        }
        .formStyle(.grouped)
    }
}

/// Settings → Privacy: what FERB records, the apps and sites it ignores, how long it keeps records, Pause recording.
struct PrivacySettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    @State private var newApp = ""
    @State private var newSite = ""

    var body: some View {
        Form {
            Section("Recording") {
                Toggle("Pause recording what happens on this Mac",
                       isOn: Binding(get: { store.recordingPaused }, set: { on in Task { await store.pauseRecording(on) } }))
                Toggle("Record window titles", isOn: store.toggle("sense.window_titles"))
                LabeledContent("Recording now", value: store.watchers.isEmpty ? "nothing" : store.watchers.joined(separator: ", "))
            }
            Section("Apps FERB ignores") {
                StringListEditor(items: store.settings.strings("privacy.exclude_apps"),
                                 placeholder: "Bundle id or app name, e.g. com.1password.1password",
                                 newItem: $newApp) { list in Task { await store.set("privacy.exclude_apps", .strings(list)) } }
            }
            Section("Sites FERB ignores") {
                StringListEditor(items: store.settings.strings("privacy.exclude_domains"),
                                 placeholder: "Site, e.g. bank.example.com",
                                 newItem: $newSite) { list in Task { await store.set("privacy.exclude_domains", .strings(list)) } }
            }
            Section("How long FERB keeps records") {
                DaysStepper(title: "Activity trail", key: "sense.trail_retention_days", store: store)
                DaysStepper(title: "Task traces", key: "retention.trace_days", store: store)
                DaysStepper(title: "Jev decisions", key: "retention.decision_days", store: store)
                DaysStepper(title: "Screenshots and evidence", key: "retention.evidence_days", store: store)
                DaysStepper(title: "Recovery copies", key: "retention.recovery_days", store: store)
                DaysStepper(title: "Resource log", key: "retention.resource_days", store: store)
            }
        }
        .formStyle(.grouped)
    }
}

struct StringListEditor: View {
    let items: [String]
    let placeholder: String
    @Binding var newItem: String
    let save: ([String]) -> Void

    var body: some View {
        ForEach(items, id: \.self) { item in
            HStack {
                Text(item).textSelection(.enabled)
                Spacer()
                Button("Remove") { save(items.filter { $0 != item }) }
            }
        }
        HStack {
            TextField(placeholder, text: $newItem)
            Button("Add") {
                let text = newItem.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !text.isEmpty else { return }
                save(items + [text])
                newItem = ""
            }
        }
    }
}

struct DaysStepper: View {
    let title: String
    let key: String
    @ObservedObject var store: FERBWindowStore

    var body: some View {
        let days = store.settings.int(key)
        Stepper("\(title): \(days) day\(days == 1 ? "" : "s")",
                value: Binding(get: { days }, set: { value in Task { await store.set(key, .int(value)) } }),
                in: 1...Int.max)
    }
}

/// Settings → Advanced: logs, a health check, the diagnostic file, the command-line tool, uninstall.
struct AdvancedSettingsPage: View {
    @ObservedObject var store: FERBWindowStore
    let actions: FERBAppActions
    @StateObject private var output = FerbOutput()
    @State private var confirmUninstall = false

    private var logs: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".ferb/logs", isDirectory: true)
    }

    var body: some View {
        Form {
            Section("Logs") {
                Button("Open FERB's logs folder") { NSWorkspace.shared.open(logs) }
            }
            Section("Health") {
                Button("Check FERB") { Task { await store.checkHealth() } }
                ForEach(store.health) { line in LabeledContent(line.name, value: line.ok ? "OK" : line.detail) }
            }
            Section("Diagnostic file") {
                Button("Export a diagnostic file…") {
                    Task {
                        if let url = await store.exportDiagnostic(appVersion: actions.appVersion) {
                            NSWorkspace.shared.activateFileViewerSelecting([url])
                        }
                    }
                }
                Text("Written on this Mac with secrets removed; FERB sends it nowhere.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            if actions.runFerb != nil {
                Section("Command-line tool and uninstall") {
                    Button("Install command-line tool") { output.run(["setup", "--cli"], with: actions) }
                        .disabled(output.running)
                    Button("Uninstall FERB…") { confirmUninstall = true }.disabled(output.running)
                    OutputLines(output: output)
                }
            }
        }
        .formStyle(.grouped)
        .alert("Uninstall FERB?", isPresented: $confirmUninstall) {
            Button("Uninstall", role: .destructive) { output.uninstall(with: actions) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("FERB stops its background service and removes its browser connection, its login item and the `ferb` " +
                 "link. Your data in ~/.ferb stays. Then quit FERB and drag it to the Trash.")
        }
    }
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `swift test --package-path ui/mac/FERBNotch --filter FERBWindowViewTests`
Expected: PASS. Then `swift test --package-path ui/mac/FERBNotch`.

- [ ] **Step 5: Commit**

```bash
git add ui/mac/FERBNotch/Sources/FERBNotch/SettingsPages.swift ui/mac/FERBNotch/Tests/NotchCoreTests/FERBWindowViewTests.swift
git diff --cached --stat
git commit -m "feat(L34): Settings → AI, Permissions, Browsers, Notifications, Privacy and Advanced

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Documentation, decision registry, owner checks and the full suite

**Files:**
- Modify: `README.md` (Everyday use: new subsection)
- Modify: `ARCHITECTURE.md` (new section after the L34 sections plans A-D added under §16; §16.10 journal kinds)
- Modify: `docs/RPC.md` (plan C's § "FERB.app's setup window and Settings (L34)")
- Modify: `CHANGELOG.md` (the L34 entry)
- Modify: `docs/0.27_DECISIONS.toml` (`[L34] tests`)
- Modify: `docs/OWNER_ACCEPTANCE.md` (append a section)

- [ ] **Step 1: README** — add under "Everyday use":

```markdown
### The FERB window and Settings (L34)

The notch stays the quick way in. For detail, choose **Open FERB Window** in FERB's menu bar menu (or press ⌘, for
Settings). The window has a sidebar:

- **Tasks:** every task, newest first, with FERB's answer; **Inspect** opens its full record; **Pass**/**Fail** give
  your verdict; **Stop** stops a running task; **Stop everything** stops FERB until you press **Resume FERB**.
- **Schedules & Reminders:** each schedule's rule, next run and last result, with Pause, Resume and Cancel; pending
  reminders with Cancel.
- **Noticed:** everything FERB noticed and told you; outside text it quotes is shown as a quote.
- **Memory:** what FERB thinks it learned (Confirm or Not right) and what it knows (**Forget…** shows exactly what
  goes before anything is deleted).
- **Settings:** General (shortcuts, FERB's pointer, sounds), AI (the setup window's Choose your AI, Check connection,
  Sign out of ChatGPT), Permissions and Browsers (the setup window's steps), Notifications (which kinds FERB posts),
  Privacy (Pause recording, window titles, the apps and sites FERB ignores, how long records are kept) and Advanced
  (logs, Check FERB, **Export a diagnostic file**, Install command-line tool, Uninstall).

Settings change `~/.ferb/config.toml` only through the app (or the CLI), never through FERB itself: the protection
lists, secrets, models and the feedback address stay in `config.toml`. **Export a diagnostic file** writes
`~/.ferb/reports/FERB-diagnostic-<date>-<time>.txt` with secrets removed and shows it in Finder; FERB sends it
nowhere.
```

- [ ] **Step 2: ARCHITECTURE.md** — add after the last L34 section:

```markdown
### The FERB window and Settings (L34 Phase E)

- **App:** `FERBWindowController` (`ui/mac/FERBNotch/Sources/FERBNotch/FERBWindow.swift`) opens one regular window
  (menu bar **Open FERB Window**, ⌘, at Settings). `FERBWindowStore` turns each button into one RPC and shows a failure
  as the problem line. Rows are decoded in `NotchCore/WindowRows.swift`. Settings → AI, Permissions and Browsers embed
  plan C's step views on the setup window's own `SetupModel`; Notifications uses plan D's `notifications.categories`;
  Advanced runs `ferb setup --cli` and `ferb uninstall` through plan C's runner. The window belongs to FERB.app, so
  every Mac tool refuses it (`driver.OWN_APP`).
- **Daemon:** `tasks.list` (root tasks, newest first, 50 per page), `notices.list` (acknowledges nothing),
  `schedules.list` rows with `last_result`, `settings.get`/`settings.set` (`ferb/daemon/settings.py`: privacy and
  retention keys only, each value checked; journal `settings.changed`), `reports.diagnostic` (the redacted local
  file; journal `diagnostic.built`). All are CLI-role methods only (rule 10).
- **CLI:** `ferb setup --cli` (link `ferb` from FERB.app) and `ferb setup --codex-logout` (FERB's own Codex home).
- **Debugging:** a refused setting names its key in the reply and the problem line; `ferb timeline` shows
  `settings.changed`.
```

In §16.10 add `settings.changed` and `diagnostic.built`, one line each. In `docs/RPC.md`, under plan C's L34 section,
add rows for `settings.get`, `settings.set`, `tasks.list`, `notices.list`, `reports.diagnostic` and the
`schedules.list` `last_result` field, with the params and replies in Tasks 1-3's Interfaces.

- [ ] **Step 3: CHANGELOG.md** — under the L34 entry add:

```markdown
Phase E, the FERB window and Settings (L34):
- A regular FERB window (menu bar **Open FERB Window**; ⌘, opens Settings in it): Tasks with answers, Inspect,
  verdicts and stop; Schedules & Reminders with each schedule's last result; Noticed; Memory with confirm and forget;
  Settings with General, AI, Permissions, Browsers, Notifications, Privacy and Advanced. The old sound and pointer
  window is gone; its switches are in Settings → General.
- New owner-only RPCs: `tasks.list`, `notices.list`, `settings.get`, `settings.set` (privacy and retention keys only,
  each value checked; rule 10) and `reports.diagnostic` (a local, redacted diagnostic file). `schedules.list` rows
  carry `last_result`. New `ferb setup --cli` and `ferb setup --codex-logout`.
```

- [ ] **Step 4: Decision registry** — in `docs/0.27_DECISIONS.toml`, add to `[L34] tests` (the entry plans A-D
  created): `"tests/test_settings_rpc.py"`, `"tests/test_window_rpcs.py"`, `"tests/test_advanced_settings.py"`,
  `"ui/mac/FERBNotch/Tests/NotchCoreTests/WindowRowsTests.swift"`,
  `"ui/mac/FERBNotch/Tests/NotchCoreTests/FERBWindowStoreTests.swift"`,
  `"ui/mac/FERBNotch/Tests/NotchCoreTests/FERBWindowViewTests.swift"`; append to its `note`: ` Phase E rulings (not the
  owner's): settings.set writes only privacy and retention keys; the notch shortcut is shown, not changeable; Codex
  sign-out is ferb setup --codex-logout.`

- [ ] **Step 5: Owner checks** — append to `docs/OWNER_ACCEPTANCE.md`:

```markdown
## The FERB window and Settings (L34 Phase E) — pending live checks (2026-10-25)

Run after `ferb setup --update` (or on the second macOS user with a fresh DMG install), the owner watching:

- **Window:** menu bar → Open FERB Window. Each sidebar section shows real rows (or says FERB isn't running). Inspect
  on a finished task opens its record; Pass on it shows "Your verdict: pass".
- **Stop one task:** start a long task in the notch, press its **Stop** in the window. **Expected:** only that task
  stops; other tasks keep going.
- **Settings write:** Privacy → add `https://www.example.com/x` to Sites FERB ignores. **Expected:** refused with "no
  https:// or path". Add `.Example.COM`: the list shows `example.com`; `config.toml` has it; `ferb timeline` shows
  `settings.changed`.
- **Rule 10:** ask FERB in the notch to "turn off window title recording in your settings". **Expected:** FERB can't
  (no tool; the shell is refused the socket and `~/.ferb`; FERB.app can't be clicked by FERB); the setting is
  unchanged.
- **Diagnostic file:** Advanced → Export a diagnostic file. **Expected:** Finder shows
  `~/.ferb/reports/FERB-diagnostic-….txt`; it holds no secret you know is stored (search it for one); nothing was
  sent.
- **Command-line tool:** Advanced → Install command-line tool. **Expected:** the output names `~/.local/bin/ferb`.
- **AI:** Sign out of ChatGPT, then Sign in with ChatGPT again. **Expected:** `~/.ferb/codex-home/auth.json` goes and
  comes back; your personal `~/.codex` is untouched.
```

- [ ] **Step 6: Full verification**

Run, in order:
- `nice -n 10 uv run pytest -q` — Expected: PASS (all).
- `uv run mypy ferb/core` — Expected: `Success`.
- `swift test --package-path ui/mac/FERBNotch` — Expected: PASS.
- `git diff --check` — Expected: no output.

- [ ] **Step 7: Commit**

```bash
git add README.md ARCHITECTURE.md docs/RPC.md CHANGELOG.md docs/0.27_DECISIONS.toml docs/OWNER_ACCEPTANCE.md
git diff --cached --stat
git commit -m "docs(L34): the FERB window and Settings: README, ARCHITECTURE, RPC, CHANGELOG, registry and owner checks

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
