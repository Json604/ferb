# Packaging Phase D: notifications and menu bar state (L34) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** FERB.app posts FERB's notifications itself (its name and icon, five switchable categories, a click opens the
notch at the item) with `osascript` kept only as the fallback, and the menu bar icon and menu show the daemon's real
state (ready, working, needs you, paused, not running → Restart).

**Architecture:** The daemon keeps deciding *what* to tell and *when* (its existing rules, the idle hold, the owner's
category switches) and writes each banner's title and body from redacted data. A new `Banner` record carries a category
(`question`, `reminder`, `alert`, `task_finished`, `loop`), a click target and a `when_hidden` flag. While FERB.app,
allowed by macOS to notify, holds the new `notifications.subscribe` stream, `_deliver` hands each banner to it instead of
running `osascript`; FERB.app posts it with `UNUserNotificationCenter` unless the notch is already showing it
(`when_hidden`), and a click opens the notch at the question, the task or today's chat. The menu bar derives one state
from facts the notch already reads (`chat.status`, now also `paused` and `stopped`, plus whether the socket answers)
and rebuilds its menu from a pure `StatusMenu.entries`.

**Tech Stack:** Python 3.12 (asyncio daemon), Swift 6 toolchain in Swift 5 language mode, AppKit, UserNotifications,
Combine; `uv run pytest`, `swift test` in `ui/mac/FERBNotch`.

**Spec:** `docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md` (L34, approved 2026-10-25: "all 7 parts in
full, just design them bare min. focus on correct working"). Read "Everyday use" (notifications, menu bar) and
"Phase D".

## Global Constraints

- Read `CLAUDE.md` first. Never read, search or reuse `vibed/`. Work on `main`; every commit message cites `L34`.
  Commit only the task's own files by path (`git add <paths>`, then `git diff --cached --stat`): other agents keep
  uncommitted work in this checkout.
- Phase D starts after Phase C is merged: it reuses `DaemonService`, `AppBundle` and `SystemAppPermissions`
  (`ui/mac/FERBNotch/Sources/FERBNotch/SetupServices.swift`) and the test fake `ScriptedRPC`
  (`Tests/NotchCoreTests/Support/ScriptedRPC.swift`).
- Secrets (rule 14): the daemon writes every banner's title and body with `_redact_text` from data that is already
  redacted; FERB.app posts them verbatim, adds no content of its own, and never logs a banner's text (`NSLog` names only
  a failure's error). A click target holds ids only.
- The daemon decides what to tell; FERB.app's only choice is to hold back a `when_hidden` banner while the notch is
  showing it. Code, not a model, makes every decision here.
- `osascript` stays the fallback whenever no FERB.app that macOS lets notify holds `notifications.subscribe` (none
  running, notifications off or undecided, a development build without an app bundle, a crash).
- Categories are switched in config.toml `[banners]` with boolean keys `question`, `reminder`, `alert`,
  `task_finished`, `loop` (default true; Phase E's Settings → Notifications writes them through
  `notifications.categories`). A switched-off category posts nothing, from FERB.app or osascript; chat, logs and the
  journal keep everything. Notices ("FERB noticed") have no switch and stay as today: in the notch when it is connected,
  a banner otherwise.
- Focus modes are respected: banners use the default interruption level (no time-sensitive entitlement).
- Background first (rule 17): posting a notification never activates FERB or moves focus; only the owner's click on a
  notification or a menu item opens the notch.
- The menu follows the spec (Open FERB, Open FERB Window, Pause FERB, Settings…, Quit) plus the state line, **Restart
  FERB** when FERB isn't running, **Resume FERB** when paused, **Lift Emergency Stop** when the emergency stop is on, and
  **Set Up FERB…** in the packaged app. **Open FERB Window** appears only once Phase E sets `hasWindow`. The old Type a
  request, Dictate a request, Conversation history items and the gesture line leave the menu.
- Pause FERB is `sense.pause` (the CLI's `ferb pause`: nothing recorded, no real mouse or keyboard input); Resume is
  `sense.resume`; Lift Emergency Stop is `stop.lift`; Restart is Phase C's `DaemonService.restart()`, and with no
  LaunchAgent installed it opens Set Up FERB….
- No new behaviour limits. Display and transport details only: two refused connections in a row (the connect loop
  retries every 2 s; launchd restarts a crashed daemon within 5 s) before "isn't running"; a poster queue of 100 unread
  banners, past which osascript posts; FERB.app re-subscribes 5 s after the stream ends.
- Don't install anything, touch `~/.ferb`, or restart the owner's daemon (rule 23). Acceptance is the owner's live check
  on a second macOS user account (rule 20).
- Each task: failing test first, then the focused tests. Before the plan's final commit: `nice -n 10 uv run pytest -q`,
  `uv run mypy ferb/core`, `git diff --check`, and `cd ui/mac/FERBNotch && swift test`.

## Review Focus

- **A development build or `swift test` (no app bundle).** Expected: no crash on `UNUserNotificationCenter`, no
  subscription, the daemon keeps osascript. Pinned in Task 5 (`testWithoutANotificationCenterThePosterNeverSubscribes`).
- **Notifications off or never decided for FERB.app.** Expected: FERB.app never claims the poster role (else banners
  would vanish) and osascript keeps posting. Pinned in Task 2
  (`test_an_app_macos_doesnt_let_notify_is_refused_and_osascript_stays`) and Task 5
  (`testAnUnauthorizedAppNeverSubscribes`).
- **FERB.app quits or crashes while subscribed.** Expected: the very next banner goes through osascript. Pinned in Task
  2 (`test_osascript_is_the_fallback_once_ferb_app_leaves`).
- **A secret inside a task's answer.** Expected: never in the banner FERB.app receives. Pinned in Task 2
  (`test_a_banner_never_carries_a_secret`).
- **A question answered elsewhere** (the notch, `ferb chat`) **or the daemon restarting.** Expected: the question's
  banner is withdrawn; one refused connection shows "Connecting…", not "isn't running". Pinned in Task 5
  (`testAnAnsweredQuestionWithdrawsItsBanner`), Task 6 (`testAnAnsweredQuestionTellsThePosterToWithdrawIt`) and Task 4
  (`testOneMissIsNotNotRunning`).

## Interfaces for other phases

Phase E (Settings → Notifications, the FERB window) consumes these. All are CLI-role methods (`ferb/daemon/auth.py`),
documented in `docs/RPC.md` § "FERB.app's setup window and Settings (L34)".

| Method | Params | Reply |
|---|---|---|
| `notifications.subscribe` (stream) | `{authorized: true}` | Events `notifications.subscribe.event` with params `{id: str, category: "question"\|"reminder"\|"alert"\|"task_finished"\|"loop"\|"", title: str, body: str, target: {kind: "question"\|"task"\|"reminder", approval_id?, task_id?, timer_id?}, when_hidden: bool}`. Refused unless `authorized` is true. While at least one is open the daemon posts no osascript banner. |
| `notifications.categories` | `{set?: {category: bool}}` | `{categories: {question: bool, reminder: bool, alert: bool, task_finished: bool, loop: bool}}`; `set` is checked whole (unknown names or non-booleans refuse it) and writes config.toml `[banners]`; journaled `banners.switched`. |
| `chat.status` (changed) | as before | adds `paused: bool` (`sense.pause` is on) and `stopped: bool` (the emergency stop is on). |

Config key for the Settings switches: **`[banners]` `question`, `reminder`, `alert`, `task_finished`, `loop`**
(booleans, default true).

Swift pieces: `BannerCategory` (NotchCore; `rawValue`, `title` for each switch's label), `SystemAppPermissions
.notifications()` / `.requestNotifications()` (Phase C) for the authorization row, `PanelCoordinator.hasWindow` and
`onOpenWindow` (set both to add **Open FERB Window**), `PanelCoordinator.perform(_ action: MenuAction)`,
`MenuAction.settings` → `showSettings()` (Phase E replaces that window), `StatusMenu.entries(state:paused:stopped:
appName:canSetUp:hasWindow:)`.

---

### Task 1: `ferb/daemon/banners.py`: categories, switches and FERB.app as the poster

**Files:**
- Create: `ferb/daemon/banners.py`
- Test: `tests/test_banners.py`

**Interfaces:**
- Produces (`ferb.daemon.banners`): `SECTION = "banners"`, `CATEGORIES: tuple[str, ...]`, `QUEUE_SIZE = 100`,
  `@dataclass(frozen=True) Banner(title: str, text: str, category: str = "", target: dict[str, str] = {},
  when_hidden: bool = False, ident: str = "")`, `enabled(get, category) -> bool`, `categories(get) -> dict[str, bool]`,
  `set_categories(config, changes) -> dict[str, bool]` (raises `FerbError`), `class Posters` with `connected: bool`,
  `open() -> asyncio.Queue[dict[str, Any]]`, `close(queue) -> None`, `deliver(item) -> bool`.

- [ ] **Step 1: Write the failing tests** (`tests/test_banners.py`)

```python
"""L34 D: banner categories, the owner's switches ([banners] in config.toml) and FERB.app as the poster."""
import asyncio
from pathlib import Path

import pytest

from ferb.config import Config
from ferb.core.errors import FerbError
from ferb.daemon import banners
from ferb.daemon.banners import Banner, Posters


def test_every_category_posts_until_the_owner_switches_it_off(tmp_path: Path) -> None:
    config = Config(tmp_path)
    assert banners.CATEGORIES == ("question", "reminder", "alert", "task_finished", "loop")
    assert banners.categories(config.get) == {c: True for c in banners.CATEGORIES}
    assert banners.set_categories(config, {"loop": False, "reminder": False}) == {
        "question": True, "reminder": False, "alert": True, "task_finished": True, "loop": False}
    assert not banners.enabled(config.get, "loop") and banners.enabled(config.get, "question")
    assert config.get("banners", "loop") is False
    assert banners.enabled(config.get, "")                     # "FERB noticed": no switch governs it


def test_switches_refuse_unknown_categories_and_non_booleans(tmp_path: Path) -> None:
    config = Config(tmp_path)
    for bad in ({"weather": False}, {"loop": "off"}, {}, ["loop"], {"loop": False, "x": True}):
        with pytest.raises(FerbError):
            banners.set_categories(config, bad)
    assert not (tmp_path / "config.toml").exists()             # a refusal writes nothing


def test_a_poster_takes_banners_only_while_connected() -> None:
    async def run() -> None:
        posters = Posters()
        assert not posters.connected and posters.deliver({"id": "x"}) is False
        queue = posters.open()
        assert posters.connected and posters.deliver({"id": "a"}) is True
        assert await queue.get() == {"id": "a"}
        posters.close(queue)
        assert not posters.connected and posters.deliver({"id": "b"}) is False
    asyncio.run(run())


def test_a_poster_that_stopped_reading_falls_back(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(banners, "QUEUE_SIZE", 1)

    async def run() -> None:
        posters = Posters()
        posters.open()
        assert posters.deliver({"id": "1"}) is True
        assert posters.deliver({"id": "2"}) is False           # full: osascript posts this one
    asyncio.run(run())


def test_a_banner_names_its_category_target_and_id() -> None:
    b = Banner("FERB reminder", "call Mom", "reminder", {"kind": "reminder", "timer_id": "t1"}, False, "reminder:t1")
    assert (b.category, b.target["timer_id"], b.ident, b.when_hidden) == ("reminder", "t1", "reminder:t1", False)
    assert Banner("FERB finished", "done") == Banner("FERB finished", "done", "", {}, False, "")
```

- [ ] **Step 2: Run them to verify they fail**

Run: `nice -n 10 uv run pytest -q tests/test_banners.py`
Expected: FAIL — `ModuleNotFoundError: No module named 'ferb.daemon.banners'`.

- [ ] **Step 3: Implement `ferb/daemon/banners.py`**

```python
"""Own FERB's macOS banner categories, the owner's switches for them, and FERB.app as their poster (L34 D). Must not
decide what to tell the owner: callers decide that; this only routes what they hand it.

While FERB.app holds a `notifications.subscribe` stream (macOS lets it notify), each banner goes to it and FERB.app posts
it with its own name and icon (UNUserNotificationCenter); otherwise the daemon posts it with osascript (Script Editor's
name). The owner switches categories off in Settings ([banners] in config.toml); chat, logs and the journal keep
everything either way."""

import asyncio
from dataclasses import dataclass, field
from typing import Any, Callable

from ferb.core.errors import FerbError

SECTION = "banners"
CATEGORIES = ("question", "reminder", "alert", "task_finished", "loop")
# banners one FERB.app connection may leave unread; past that it stopped reading, and osascript posts the rest
QUEUE_SIZE = 100


@dataclass(frozen=True)
class Banner:
    title: str
    text: str
    category: str = ""                                      # one of CATEGORIES; "" for one no switch governs
    target: dict[str, str] = field(default_factory=dict)    # what a click opens: {"kind": "question"|"task"|"reminder", …}
    when_hidden: bool = False                               # FERB.app posts it only while the notch isn't showing it
    ident: str = ""                                         # the notification's id: a repost replaces it; "" for a new one


def enabled(get: Callable[..., Any], category: str) -> bool:
    """A category posts unless the owner set it to false; a banner with no category always posts."""
    return category not in CATEGORIES or get(SECTION, category, True) is not False


def categories(get: Callable[..., Any]) -> dict[str, bool]:
    return {c: enabled(get, c) for c in CATEGORIES}


def set_categories(config: Any, changes: Any) -> dict[str, bool]:
    """The owner's switches from Settings → Notifications, checked whole before anything is written."""
    if not isinstance(changes, dict) or not changes:
        raise FerbError("set names the categories to switch: " + ", ".join(CATEGORIES))
    unknown = sorted(str(k) for k in changes if k not in CATEGORIES)
    if unknown:
        raise FerbError(f"unknown banner categories {unknown}; they are {', '.join(CATEGORIES)}")
    if not all(isinstance(v, bool) for v in changes.values()):
        raise FerbError("each category is switched with true or false")
    for category, on in changes.items():
        config.set(SECTION, category, on)
    return categories(config.get)


class Posters:
    """FERB.app's open `notifications.subscribe` streams, one queue each."""

    def __init__(self) -> None:
        self._queues: list[asyncio.Queue[dict[str, Any]]] = []

    @property
    def connected(self) -> bool:
        return bool(self._queues)

    def open(self) -> asyncio.Queue[dict[str, Any]]:
        queue: asyncio.Queue[dict[str, Any]] = asyncio.Queue(maxsize=QUEUE_SIZE)
        self._queues.append(queue)
        return queue

    def close(self, queue: asyncio.Queue[dict[str, Any]]) -> None:
        self._queues = [q for q in self._queues if q is not queue]

    def deliver(self, item: dict[str, Any]) -> bool:
        """Hand the banner to every open poster; False when none took it (none connected, or each stopped reading)."""
        took = False
        for queue in self._queues:
            try:
                queue.put_nowait(item)
                took = True
            except asyncio.QueueFull:
                pass
        return took
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nice -n 10 uv run pytest -q tests/test_banners.py`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ferb/daemon/banners.py tests/test_banners.py
git commit -m "feat(L34): banner categories, the owner's [banners] switches and FERB.app as the poster"
```

---

### Task 2: The daemon routes banners to FERB.app, osascript as the fallback

**Files:**
- Modify: `ferb/daemon/main.py` (`__init__` `_held` ~`:416`; registration ~`:527-535`; `_notify_safely`, `_notify`,
  `_flush_held` ~`:2884-2917`; new `_deliver`, `_notifications_subscribe`, `_notifications_categories`)
- Modify: `ferb/daemon/auth.py`
- Modify: `docs/RPC.md`
- Create: `tests/test_app_notifications.py`
- Modify: `tests/test_schedules.py` (`test_non_urgent_notifications_wait_while_the_owner_is_away`)

**Interfaces:**
- Consumes: Task 1's `Banner`, `Posters`, `enabled`, `categories`, `set_categories`.
- Produces:
  - `Daemon.posters: banners.Posters`; `Daemon._held: list[Banner]`.
  - `async Daemon._notify(title: str, text: str, urgent: bool = False, *, category: str = "",
    target: dict[str, str] | None = None, when_hidden: bool = False, ident: str = "") -> None`
  - `async Daemon._deliver(banner: Banner) -> None` (poster first, else `_notify_safely(title, text)`, unchanged).
  - RPCs `notifications.subscribe` (stream), `notifications.categories`; journal kind `banners.switched {set}`.

- [ ] **Step 1: Write the failing tests** (`tests/test_app_notifications.py`)

```python
"""L34 D: FERB.app posts FERB's banners while it holds `notifications.subscribe`; osascript is the fallback; the owner's
category switches; nothing secret in a banner; each banner's category and click target."""
import json
import threading
import time
from datetime import timedelta
from types import SimpleNamespace
from typing import Any, Callable, Iterator

import pytest

from ferb.cli.client import Client, RpcError
from ferb.core import origin as origin_
from ferb.daemon.banners import Banner
from ferb.sense.trail import Observation
from tests.conftest import RunningDaemon


class Poster:
    """FERB.app's subscription, faked: a thread reads the stream into `items`."""

    def __init__(self, daemon: RunningDaemon, authorized: bool = True) -> None:
        self.client = Client(daemon.sock)
        self.items: list[dict[str, Any]] = []
        self.error: Exception | None = None
        self.thread = threading.Thread(target=self._read, args=(authorized,), daemon=True)
        self.thread.start()

    def _read(self, authorized: bool) -> None:
        try:
            for item in self.client.stream("notifications.subscribe", {"authorized": authorized}):
                self.items.append(item)
        except Exception as e:                    # the daemon's refusal, or this side closing
            self.error = e

    def close(self) -> None:
        self.client.interrupt()                   # wakes the reader; the daemon sees the connection end
        self.thread.join(2)
        self.client.close()


def until(check: Callable[[], bool], seconds: float = 5) -> None:
    end = time.monotonic() + seconds
    while not check():
        if time.monotonic() > end:
            raise AssertionError("never happened")
        time.sleep(0.05)


def settle() -> None:
    time.sleep(0.3)                               # long enough for a banner that would arrive to arrive


def on_loop(daemon: RunningDaemon, fn: Callable[[], Any]) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


@pytest.fixture
def notes(daemon: RunningDaemon) -> list[tuple[str, str]]:
    """osascript, faked: what it would have posted."""
    seen: list[tuple[str, str]] = []

    async def note(title: str, text: str) -> None:
        seen.append((title, text))
    daemon.daemon.notify = note
    return seen


@pytest.fixture
def poster(daemon: RunningDaemon) -> Iterator[Poster]:
    p = Poster(daemon)
    until(lambda: daemon.daemon.posters.connected)
    yield p
    p.close()


# ---------- routing ----------

def test_with_ferb_app_connected_a_banner_goes_to_it_and_not_to_osascript(
        daemon: RunningDaemon, notes: list[tuple[str, str]], poster: Poster) -> None:
    daemon.run(daemon.daemon._notify("FERB reminder", "call Mom", urgent=True, category="reminder",
                                     target={"kind": "reminder", "timer_id": "t1"}, ident="reminder:t1"))
    until(lambda: bool(poster.items))
    assert poster.items == [{"id": "reminder:t1", "category": "reminder", "title": "FERB reminder", "body": "call Mom",
                             "target": {"kind": "reminder", "timer_id": "t1"}, "when_hidden": False}]
    assert notes == []


def test_osascript_is_the_fallback_once_ferb_app_leaves(daemon: RunningDaemon, notes: list[tuple[str, str]],
                                                        poster: Poster) -> None:
    poster.close()
    until(lambda: not daemon.daemon.posters.connected)
    daemon.run(daemon.daemon._notify("FERB reminder", "call Mom", urgent=True, category="reminder"))
    assert notes == [("FERB reminder", "call Mom")]


def test_an_app_macos_doesnt_let_notify_is_refused_and_osascript_stays(daemon: RunningDaemon,
                                                                      notes: list[tuple[str, str]]) -> None:
    refused = Poster(daemon, authorized=False)
    until(lambda: refused.error is not None)
    assert isinstance(refused.error, RpcError) and not daemon.daemon.posters.connected
    daemon.run(daemon.daemon._notify("FERB alert", "Price dropped", urgent=True, category="alert"))
    assert notes == [("FERB alert", "Price dropped")]
    refused.close()


def test_a_switched_off_category_posts_nothing_anywhere(daemon: RunningDaemon, notes: list[tuple[str, str]]) -> None:
    client = Client(daemon.sock)
    try:
        assert client.call("notifications.categories")["categories"] == {
            "question": True, "reminder": True, "alert": True, "task_finished": True, "loop": True}
        assert client.call("notifications.categories", {"set": {"reminder": False}})["categories"]["reminder"] is False
        with pytest.raises(RpcError, match="unknown banner categories"):
            client.call("notifications.categories", {"set": {"weather": True}})
    finally:
        client.close()
    d = daemon.daemon
    daemon.run(d._notify("FERB reminder", "call Mom", urgent=True, category="reminder"))
    daemon.run(d._notify("FERB noticed", "Your Claude plan renewed"))          # no switch governs a notice
    assert notes == [("FERB noticed", "Your Claude plan renewed")]
    assert d.config.get("banners", "reminder") is False
    entries = on_loop(daemon, lambda: d._journal().of_kind("banners.switched"))
    assert [e.data for e in entries] == [{"set": {"reminder": False}}]


def test_a_banner_never_carries_a_secret(daemon: RunningDaemon, poster: Poster) -> None:
    d = daemon.daemon
    daemon.call_soon(d._add_secret_value, "hunter2-correct-horse")
    daemon.run(d._notify("FERB finished", "Logged in with hunter2-correct-horse", category="task_finished",
                         target={"kind": "task", "task_id": "t1"}))
    until(lambda: bool(poster.items))
    assert "hunter2-correct-horse" not in json.dumps(poster.items)
    assert poster.items[0]["id"].startswith("banner:")


def test_a_banner_held_while_the_owner_was_away_goes_to_ferb_app_when_they_are_back(daemon: RunningDaemon,
                                                                                   poster: Poster) -> None:
    d = daemon.daemon
    d.owner_input.running = True
    d.owner_input.last_owner_wall = time.time() - 3600                          # away for an hour
    try:
        daemon.run(d._notify("FERB finished", "done", category="task_finished"))
        assert poster.items == [] and d._held == [Banner("FERB finished", "done", "task_finished")]
        d.owner_input.last_owner_wall = time.time()                             # back
        until(lambda: bool(poster.items), seconds=15)                           # the held queue is read every 10 s
        assert poster.items[0]["body"] == "done"
    finally:
        d.owner_input.running = False
```

In `tests/test_schedules.py`, `test_non_urgent_notifications_wait_while_the_owner_is_away`, add
`from ferb.daemon.banners import Banner` to the imports and change the assertion to:

```python
    assert notes == [("FERB reminder", "call Mom")] and d._held == [Banner("FERB finished", "done")]
```

- [ ] **Step 2: Run them to verify they fail**

Run: `nice -n 10 uv run pytest -q tests/test_app_notifications.py tests/test_schedules.py -k "banner or osascript or category or held or wait_while"`
Expected: FAIL — `AttributeError: 'Daemon' object has no attribute 'posters'`, `_notify() got an unexpected keyword
argument 'category'`.

- [ ] **Step 3: Implement**

`ferb/daemon/auth.py`: add `"notifications.subscribe", "notifications.categories",` to the `Role.CLI` set.

`ferb/daemon/main.py`:

Add to the module imports (next to the other `ferb.daemon` imports):

```python
from ferb.daemon import banners
from ferb.daemon.banners import Banner
```

In `__init__`, replace the `self._held` line with:

```python
        self._held: list[Banner] = []                        # banners waiting for the owner to be back (13)
        self.posters = banners.Posters()                     # FERB.app's notifications.subscribe streams (L34 D)
```

Registration, after the `notifications.label` line:

```python
        self.server.unary['notifications.categories'] = self._notifications_categories
        self.server.streams['notifications.subscribe'] = self._notifications_subscribe
```

Replace `_notify` and `_flush_held` with (keep `_notify_safely` exactly as it is: it is the osascript path):

```python
    async def _notify(self, title: str, text: str, urgent: bool = False, *, category: str = "",
                      target: dict[str, str] | None = None, when_hidden: bool = False, ident: str = "") -> None:
        """Time awareness in code (13): while the owner is idle, non-urgent banners wait until they're back. A category
        the owner switched off in Settings posts nothing (L34 D); chat, logs and the journal still have it."""
        if not banners.enabled(self.config.get, category):
            return
        banner = Banner(title, text, category, dict(target or {}), when_hidden, ident)
        if not urgent and self.owner_idle():
            self._held.append(banner)
            return
        await self._deliver(banner)

    async def _flush_held(self) -> None:
        while True:
            await asyncio.sleep(10)
            if self._held and not self.owner_idle():
                held, self._held = self._held, []
                for banner in held:
                    await self._deliver(banner)

    async def _deliver(self, banner: Banner) -> None:
        """FERB.app posts it while it is the poster (its name and icon; a click opens the notch at the item), else
        osascript does. The title and body are redacted here, from the redacted data callers pass (rule 14)."""
        import uuid
        if self.posters.connected and self.posters.deliver({
                "id": banner.ident or f"banner:{uuid.uuid4().hex[:12]}", "category": banner.category,
                "title": self._redact_text(banner.title), "body": self._redact_text(banner.text),
                "target": dict(banner.target), "when_hidden": banner.when_hidden}):
            return
        await self._notify_safely(banner.title, banner.text)

    async def _notifications_subscribe(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """L34 D: FERB.app, allowed by macOS to post notifications, takes FERB's banners while this stream is open; the
        daemon posts no osascript banner meanwhile. FERB.app leaving ends it, and osascript is back at once."""
        if params.get("authorized") is not True:
            raise FerbError("FERB.app may not post notifications yet; FERB keeps posting them through osascript")
        queue = self.posters.open()
        try:
            while True:
                yield await queue.get()
        finally:
            self.posters.close(queue)

    async def _notifications_categories(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 D: which banner categories post (Settings → Notifications); `set` switches some (config [banners])."""
        if "set" in params:
            switched = banners.set_categories(self.config, params["set"])
            self._journal().append("banners.switched", {"set": params["set"]})
            return {"categories": switched}
        return {"categories": banners.categories(self.config.get)}
```

- [ ] **Step 4: Document** — add to the `docs/RPC.md` L34 table (create the section header as in Phase C if absent):

```markdown
| `notifications.subscribe` | `{authorized: true}` | Stream of `notifications.subscribe.event` with `{id, category, title, body, target, when_hidden}`: FERB.app, allowed by macOS to notify, posts each one with UNUserNotificationCenter (unless `when_hidden` and the notch is showing it); while at least one stream is open the daemon posts no osascript banner. `category`: `question`, `reminder`, `alert`, `task_finished`, `loop`, or `""` (a notice); `target`: `{kind: "question", approval_id, task_id}`, `{kind: "task", task_id}` or `{kind: "reminder", timer_id}`. The title and body are written from redacted data. Refused unless `authorized` is true |
| `notifications.categories` | `{set?: {category: bool}}` | `{categories: {question, reminder, alert, task_finished, loop}}`; `set` is checked whole and written to config.toml `[banners]`; journaled `banners.switched`. A switched-off category posts nothing, from FERB.app or osascript |
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `nice -n 10 uv run pytest -q tests/test_app_notifications.py tests/test_schedules.py tests/test_attention_understanding.py tests/test_auth.py tests/test_banners.py`
Expected: PASS (the existing own-banner and failed-banner tests in `test_attention_understanding.py` still go through
`_notify_safely`).

- [ ] **Step 6: Commit**

```bash
git add ferb/daemon/main.py ferb/daemon/auth.py docs/RPC.md tests/test_app_notifications.py tests/test_schedules.py
git commit -m "feat(L34): banners go to FERB.app while it holds notifications.subscribe; osascript is the fallback; [banners] switches"
```

---

### Task 3: Each banner's category and target; FERB.app's rows are FERB's; `chat.status` says paused and stopped

**Files:**
- Modify: `ferb/daemon/main.py` (`_timer_fired` ~`:2816`, `_task_result` ~`:2848`, `_approval_notice` ~`:2872`,
  `_loop_notice`/`_tell_owner`/`_post_notice` ~`:3027-3062`, `_chat_status` ~`:2919`)
- Modify: `ferb/sense/perception.py` (`_own_banner`)
- Modify: `ferb/device/install.py` (`DEV_APP_BUNDLE_ID`)
- Modify: `docs/RPC.md` (`chat.status` row)
- Test: `tests/test_app_notifications.py`

**Interfaces:**
- Consumes: Task 2's `_notify(..., category=, target=, when_hidden=, ident=)` and `posters`.
- Produces:
  - Banners: reminder/timer `{category: "reminder", target: {kind: "reminder", timer_id}, ident: "reminder:<id>"}`;
    alert `{category: "alert", target: {kind: "task", task_id}}`; task finished `{category: "task_finished", target:
    {kind: "task", task_id}, ident: "task:<id>", when_hidden: true while FERB.app is the poster}`; question
    `{category: "question", target: {kind: "question", approval_id, task_id: <root>}, ident: "question:<approval id>",
    when_hidden: true}`; loop `{category: "loop", title: "FERB loop", target: {kind: "task", task_id: <tree>}, ident:
    "loop:<notice id>", when_hidden: true}`.
  - `_tell_owner(ident, refs, text, notice_origin, *, category: str = "", task_id: str = "")`,
    `_post_notice(notice, *, category: str = "", task_id: str = "")`.
  - `ferb.device.install.DEV_APP_BUNDLE_ID = "dev.ferb.notch"`.
  - `chat.status` adds `paused: bool`, `stopped: bool`.

- [ ] **Step 1: Write the failing tests** (append to `tests/test_app_notifications.py`)

```python
# ---------- what each banner is ----------

def test_a_reminder_goes_to_ferb_app_with_its_timer(daemon: RunningDaemon, notes: list[tuple[str, str]],
                                                    poster: Poster) -> None:
    d = daemon.daemon
    timer = on_loop(daemon, lambda: d.timers.add("reminder", d.clock.now() + timedelta(minutes=1), "call Mom"))
    daemon.run(d._timer_fired(timer))
    until(lambda: bool(poster.items))
    item = poster.items[0]
    assert (item["id"], item["category"], item["title"], item["body"], item["when_hidden"]) == (
        f"reminder:{timer.id}", "reminder", "FERB reminder", "call Mom", False)
    assert item["target"] == {"kind": "reminder", "timer_id": timer.id}
    assert notes == []


def test_a_finished_task_goes_to_ferb_app_even_when_a_chat_showed_it(daemon: RunningDaemon,
                                                                     notes: list[tuple[str, str]],
                                                                     poster: Poster) -> None:
    d = daemon.daemon
    daemon.run(d._task_result({"task_id": "t9", "status": "done", "answer": "Renamed 12 files."}, shown=True))
    daemon.run(d._task_result({"task_id": "t10", "status": "cancelled", "answer": "Stopped."}, shown=False))
    daemon.run(d._task_result({"task_id": "t11", "status": "done", "answer": "It's $40 now."}, shown=True, alert=True))
    until(lambda: len(poster.items) == 2)
    settle()
    finished, alert = poster.items
    assert (finished["id"], finished["category"], finished["title"], finished["when_hidden"]) == (
        "task:t9", "task_finished", "FERB finished", True)
    assert finished["target"] == {"kind": "task", "task_id": "t9"}
    assert (alert["category"], alert["title"], alert["when_hidden"], alert["target"]) == (
        "alert", "FERB alert", False, {"kind": "task", "task_id": "t11"})
    assert len(poster.items) == 2 and notes == []           # a task the owner stopped is never a banner


def test_without_ferb_app_a_shown_answer_stays_quiet_as_before(daemon: RunningDaemon,
                                                               notes: list[tuple[str, str]]) -> None:
    d = daemon.daemon
    daemon.run(d._task_result({"task_id": "t1", "status": "done", "answer": "Shown in chat."}, shown=True))
    daemon.run(d._task_result({"task_id": "t2", "status": "done", "answer": "Nobody saw it."}, shown=False))
    assert notes == [("FERB finished", "Nobody saw it.")]


def test_a_question_goes_to_ferb_app_even_with_its_chat_attached(daemon: RunningDaemon,
                                                                 notes: list[tuple[str, str]],
                                                                 poster: Poster) -> None:
    d = daemon.daemon
    d._attached_trees.add("t1")
    daemon.call_soon(d._approval_notice, {"approval_id": "a1", "task_id": "t1.w2", "reason": "Send it?",
                                          "summary": "Send the email to Sam?", "capability": "mail.send"})
    daemon.call_soon(d._approval_notice, {"approval_id": "p1", "task_id": None, "reason": "Confirm your profile",
                                          "capability": "setup.persona"})        # never a banner (L3)
    until(lambda: bool(poster.items))
    settle()
    assert poster.items == [{"id": "question:a1", "category": "question", "title": "FERB needs you",
                             "body": "FERB needs your OK: Send the email to Sam?",
                             "target": {"kind": "question", "approval_id": "a1", "task_id": "t1"},
                             "when_hidden": True}]
    assert notes == []


def test_a_loop_notice_goes_to_ferb_app_as_a_loop_banner(daemon: RunningDaemon, notes: list[tuple[str, str]],
                                                         poster: Poster) -> None:
    d = daemon.daemon
    daemon.run(d._loop_notice("tree-1", "keep", "I keep having to redo this: keep the tab playing."))
    until(lambda: bool(poster.items))
    item = poster.items[0]
    assert (item["category"], item["title"], item["when_hidden"]) == ("loop", "FERB loop", True)
    assert item["target"] == {"kind": "task", "task_id": "tree-1"} and item["id"].startswith("loop:")
    assert item["body"] == "I keep having to redo this: keep the tab playing."
    assert notes == []


def test_a_notice_stays_in_the_notch_while_ferb_app_is_connected(daemon: RunningDaemon,
                                                                 notes: list[tuple[str, str]],
                                                                 poster: Poster) -> None:
    daemon.run(daemon.daemon._tell_owner("n-renewed", [], "Your Claude plan renewed.", origin_.FERB))
    settle()
    assert poster.items == [] and notes == []


def test_ferb_apps_own_notification_rows_are_ferbs_and_keep_no_body(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    assert d.perception is not None

    def arrive(external_id: str, bundle: str) -> None:
        d.perception.notification(Observation("notification", {      # type: ignore[union-attr]
            "external_id": external_id, "bundle_id": bundle, "title": "FERB finished", "body": "Renamed 12 files.",
            "conversation": "FERB finished", "conversation_source": "title"}, {"actor": "unknown"}))
    for external_id, bundle in (("nc-app", "com.ferb.FERB"), ("nc-dev", "dev.ferb.notch"),
                                ("nc-other", "net.whatsapp.WhatsApp")):
        daemon.call_soon(arrive, external_id, bundle)
    rows = on_loop(daemon, lambda: {json.loads(r["data_json"])["external_id"]: (r["actor"], json.loads(r["data_json"]))
                                    for r in d.db.conn.execute(          # type: ignore[union-attr]
                                        "SELECT actor, data_json FROM trail WHERE source='notification'")})
    assert rows["nc-app"][0] == "ferb" and "body" not in rows["nc-app"][1]
    assert rows["nc-app"][1]["ferb_banner"] == {"poster": "com.ferb.FERB"}
    assert rows["nc-dev"][0] == "ferb"
    assert rows["nc-other"][0] == "unknown"


def test_chat_status_says_whether_ferb_is_paused_or_stopped(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    original = d.sense
    client = Client(daemon.sock)
    try:
        status = client.call("chat.status", {"peek": True})
        assert status["paused"] is False and status["stopped"] is False
        d.sense = SimpleNamespace(paused=True)                  # type: ignore[assignment]
        assert client.call("chat.status", {"peek": True})["paused"] is True
        d.sense = original
        client.call("stop.engage")
        assert client.call("chat.status", {"peek": True})["stopped"] is True
        client.call("stop.lift")
    finally:
        d.sense = original
        client.close()
```

- [ ] **Step 2: Run them to verify they fail**

Run: `nice -n 10 uv run pytest -q tests/test_app_notifications.py`
Expected: the new tests FAIL — items arrive with `category: ""` and no target; questions and finished tasks with an
attached chat send nothing; `nc-app` is recorded as `unknown`; `chat.status` has no `paused`.

- [ ] **Step 3: Implement the call sites** in `ferb/daemon/main.py`

`_timer_fired`, the notify call:

```python
            await self._notify("FERB reminder" if t.kind == "reminder" else "FERB timer", f"{t.message}{late}",
                               urgent=True, category="reminder", target={"kind": "reminder", "timer_id": str(t.id)},
                               ident=f"reminder:{t.id}")                       # a reminder is for now
```

`_task_result`, replace the `if alert: … elif …:` block with:

```python
        task = {"kind": "task", "task_id": str(ev["task_id"])}
        if alert:
            await self._notify("FERB alert", answer, urgent=True, category="alert", target=task)
        elif ev.get("status") != "cancelled" and (self.posters.connected or (
                not shown and not self._client_shows("task.result"))):
            # L34 D: with FERB.app as the poster it decides by the notch: a banner while the notch isn't showing the
            # answer, whichever chat showed it; without it, as before (L9)
            await self._notify("FERB finished" if ev.get("status") == "done" else "FERB", answer,
                               category="task_finished", target=task, when_hidden=self.posters.connected,
                               ident=f"task:{ev['task_id']}")
```

and add to its docstring: `With FERB.app as the poster (L34 D) a finished task always goes to it, marked when_hidden.`

`_approval_notice`, replace its body below the docstring with:

```python
        tid = str(data.get("task_id") or "")
        if data.get("capability") == "setup.persona":
            return
        text = f"FERB needs your OK: {str(data.get('summary') or data.get('reason', ''))}"   # the short line first
        loop = asyncio.get_running_loop()
        if self.posters.connected:          # L34 D: FERB.app shows it unless the notch already does
            approval = str(data.get("approval_id") or "")
            loop.create_task(self._notify("FERB needs you", text, urgent=True, category="question",
                                          target={"kind": "question", "approval_id": approval,
                                                  "task_id": tid.split(".", 1)[0]},
                                          when_hidden=True, ident=f"question:{approval}"))
            return
        if tid.split(".", 1)[0] in self._attached_trees or self._client_shows("approval.requested"):
            return
        loop.create_task(self._notify("FERB needs you", text, urgent=True, category="question"))
```

`_loop_notice`, pass the loop's category and tree:

```python
        await self._tell_owner(f'loop-{step_id}-{uuid.uuid4().hex[:12]}', [f'task:{tree_id}'], text, notice_origin,
                               category="loop", task_id=tree_id)
```

`_tell_owner` gains keyword arguments and passes them on:

```python
    async def _tell_owner(self, ident: str, refs: list[str], text: str, notice_origin: str, *, category: str = "",
                          task_id: str = "") -> None:
```

with its last line `await self._post_notice(notice, category=category, task_id=task_id)`.

`_post_notice` gains the same keywords; replace its last two lines with:

```python
        self._publish('attention.notice', view)
        if self.posters.connected:
            # L34 D: FERB.app shows notices in the notch; a loop's pause or back-off is also a banner while the notch
            # isn't showing it
            if category == "loop":
                await self._notify("FERB loop", view['text'], category="loop",
                                   target={"kind": "task", "task_id": task_id}, when_hidden=True,
                                   ident=f"loop:{view['id']}")
        elif not self._client_shows('attention.notice'):
            await self._notify('FERB noticed', view['text'], category=category)
```

Signature: `async def _post_notice(self, notice: dict[str, Any], *, category: str = "", task_id: str = "") -> None:`.

`_chat_status`, add two keys to the returned dict (after `"loops": …`):

```python
                "paused": bool(self.sense is not None and self.sense.paused),        # L34 D: the menu bar's state
                "stopped": bool(self.state.stopped)}
```

- [ ] **Step 4: FERB.app's rows are FERB's**

`ferb/device/install.py`, after `APP_BUNDLE_ID`:

```python
DEV_APP_BUNDLE_ID = "dev.ferb.notch"        # the development notch build (ui/mac/FERBNotch/build.sh)
```

`ferb/sense/perception.py`, in `_own_banner`, right after its existing `data = observation.data` line (before the
`for banner in self.banners` loop) insert:

```python
        from ferb.device.install import APP_BUNDLE_ID, DEV_APP_BUNDLE_ID
        if str(data.get("bundle_id", "")).lower() in (APP_BUNDLE_ID.lower(), DEV_APP_BUNDLE_ID.lower()):
            # FERB.app posts only FERB's own banners (L34 D): every row of it is FERB's, by its poster alone, and its
            # words stay with the answer or question they came from
            kept = {k: v for k, v in data.items() if k != "body"} | {"ferb_banner": {"poster": str(data["bundle_id"])}}
            return Observation(observation.source, kept, {"actor": "ferb"})
```

and extend the docstring: `Any row FERB.app (or its development build) posted is FERB's (L34 D).`

- [ ] **Step 5: Document** — in `docs/RPC.md`, append to the `chat.status` row's result text:
  `; also `paused` (sense.pause is on) and `stopped` (the emergency stop is on), for FERB.app's menu bar (L34 D)`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `nice -n 10 uv run pytest -q tests/test_app_notifications.py tests/test_attention_understanding.py tests/test_loops.py tests/test_schedules.py tests/test_watches.py tests/test_notices_in_chat.py`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add ferb/daemon/main.py ferb/sense/perception.py ferb/device/install.py docs/RPC.md tests/test_app_notifications.py
git commit -m "feat(L34): each banner's category and click target; FERB.app's rows are FERB's; chat.status says paused and stopped"
```

---

### Task 4: NotchCore: banners, click targets and the menu bar's state

**Files:**
- Create: `ui/mac/FERBNotch/Sources/NotchCore/Banners.swift`
- Create: `ui/mac/FERBNotch/Sources/NotchCore/MenuBarState.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/NotificationTests.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/MenuBarStateTests.swift`

**Interfaces:**
- Produces (public, NotchCore): `BannerCategory` (`question`, `reminder`, `alert`, `taskFinished = "task_finished"`,
  `loop`; `title`), `NotificationTarget` (`question(approvalID:taskID:)`, `task(String)`, `reminder(String)`, `notch`;
  `init(_ target: [String: Any])`, `init(userInfo:)`, `userInfo: [String: String]`), `BannerItem` (`init?(_ event:)`;
  `id`, `category`, `title`, `body`, `target`, `whenHidden`), `NotificationGate.shouldPost(_:notchShowing:)`,
  `NotificationGate.questionID(_:)`, `DaemonLink` (`state`, `succeeded()`, `failed(unreachable:)`, `restarting()`),
  `MenuBarState` (`derive(link:paused:stopped:questions:running:)`, `symbol`, `statusLine`), `MenuAction`,
  `MenuEntry`, `StatusMenu.entries(state:paused:stopped:appName:canSetUp:hasWindow:)`.

- [ ] **Step 1: Write the failing tests**

`NotificationTests.swift`:

```swift
import XCTest
@testable import NotchCore

/// L34 D: banners as the daemon sends them, and what a click opens.
final class NotificationTests: XCTestCase {
    func testABannerArrivesAsTheDaemonWroteIt() {
        let item = BannerItem(["id": "question:a1", "category": "question", "title": "FERB needs you",
                               "body": "FERB needs your OK: Send the email to Sam?",
                               "target": ["kind": "question", "approval_id": "a1", "task_id": "t1"],
                               "when_hidden": true])!
        XCTAssertEqual(item.category, .question)
        XCTAssertEqual(item.title, "FERB needs you")
        XCTAssertEqual(item.body, "FERB needs your OK: Send the email to Sam?")
        XCTAssertEqual(item.target, .question(approvalID: "a1", taskID: "t1"))
        XCTAssertTrue(item.whenHidden)
        XCTAssertNil(BannerItem(["category": "question", "title": "x", "body": "y"]))       // no id
        XCTAssertNil(BannerItem(["id": "x", "body": "y"]))                                   // no title
        XCTAssertNil(BannerItem(["id": "n", "category": "", "title": "FERB noticed", "body": "x"])!.category)
    }

    func testOnlyBannersTheNotchShowsWaitForItToClose() {
        let question = BannerItem(["id": "q", "title": "t", "body": "b", "when_hidden": true])!
        let reminder = BannerItem(["id": "r", "title": "t", "body": "b", "when_hidden": false])!
        XCTAssertFalse(NotificationGate.shouldPost(question, notchShowing: true))
        XCTAssertTrue(NotificationGate.shouldPost(question, notchShowing: false))
        XCTAssertTrue(NotificationGate.shouldPost(reminder, notchShowing: true))
    }

    func testAClickedNotificationFindsItsItemAgain() {
        for target in [NotificationTarget.question(approvalID: "a1", taskID: "t1"),
                       .question(approvalID: "a2", taskID: nil), .task("t9"), .reminder("r1"), .notch] {
            let info: [AnyHashable: Any] = target.userInfo.reduce(into: [:]) { $0[$1.key] = $1.value }
            XCTAssertEqual(NotificationTarget(userInfo: info), target)
        }
        XCTAssertEqual(NotificationTarget(["kind": "task"]), .notch)              // no id: just the notch
        XCTAssertEqual(NotificationGate.questionID("a1"), "question:a1")
    }

    func testEveryCategoryHasASettingsTitle() {
        XCTAssertEqual(BannerCategory.allCases.map(\.rawValue), ["question", "reminder", "alert", "task_finished", "loop"])
        for category in BannerCategory.allCases { XCTAssertFalse(category.title.isEmpty) }
    }
}
```

`MenuBarStateTests.swift`:

```swift
import XCTest
@testable import NotchCore

/// L34 D: the menu bar icon and menu show the daemon's real state.
final class MenuBarStateTests: XCTestCase {
    func testOneMissIsNotNotRunning() {
        var link = DaemonLink()
        XCTAssertEqual(link.state, .connecting)
        link.failed(unreachable: true)
        XCTAssertEqual(link.state, .connecting)
        link.failed(unreachable: true)
        XCTAssertEqual(link.state, .unreachable)
        link.succeeded()
        XCTAssertEqual(link.state, .connected)
        link.failed(unreachable: false)                          // a dropped connection reconnects
        XCTAssertEqual(link.state, .connecting)
        link.failed(unreachable: true)
        link.restarting()
        link.failed(unreachable: true)
        XCTAssertEqual(link.state, .connecting)                  // Restart starts the count again
    }

    func testTheStateShowsWhatMattersMost() {
        XCTAssertEqual(MenuBarState.derive(link: .unreachable, paused: true, stopped: true, questions: 2, running: 1),
                       .notRunning)
        XCTAssertEqual(MenuBarState.derive(link: .connecting, paused: false, stopped: false, questions: 0, running: 0),
                       .connecting)
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: true, stopped: false, questions: 1, running: 3),
                       .needsYou(1))
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: true, stopped: true, questions: 0, running: 3),
                       .stopped)
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: true, stopped: false, questions: 0, running: 3),
                       .paused)
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: false, stopped: false, questions: 0, running: 2),
                       .working(2))
        XCTAssertEqual(MenuBarState.derive(link: .connected, paused: false, stopped: false, questions: 0, running: 0),
                       .ready)
        XCTAssertEqual(MenuBarState.notRunning.statusLine, "FERB isn't running — Restart")
        XCTAssertEqual(MenuBarState.working(1).statusLine, "Working on 1 task")
        XCTAssertEqual(MenuBarState.needsYou(2).symbol, "exclamationmark.bubble")
    }

    func testTheMenuOffersWhatTheStateNeeds() {
        func titles(_ state: MenuBarState, paused: Bool = false, stopped: Bool = false, setUp: Bool = true,
                    window: Bool = false) -> [String] {
            StatusMenu.entries(state: state, paused: paused, stopped: stopped, appName: "FERB", canSetUp: setUp,
                               hasWindow: window).compactMap {
                if case .item(let title, _, _) = $0 { return title }
                return nil
            }
        }
        XCTAssertEqual(titles(.ready), ["Open FERB", "Pause FERB", "Settings…", "Set Up FERB…", "Quit FERB"])
        XCTAssertEqual(titles(.notRunning), ["Open FERB", "Restart FERB", "Settings…", "Set Up FERB…", "Quit FERB"])
        XCTAssertEqual(titles(.paused, paused: true, setUp: false, window: true),
                       ["Open FERB", "Open FERB Window", "Resume FERB", "Settings…", "Quit FERB"])
        XCTAssertEqual(titles(.needsYou(1), paused: true, stopped: true),
                       ["Open FERB", "Lift Emergency Stop", "Resume FERB", "Settings…", "Set Up FERB…", "Quit FERB"])
        XCTAssertEqual(titles(.connecting), ["Open FERB", "Settings…", "Set Up FERB…", "Quit FERB"])
        let entries = StatusMenu.entries(state: .working(2), paused: false, stopped: false, appName: "FERB",
                                         canSetUp: false, hasWindow: false)
        XCTAssertEqual(entries.first, .status("Working on 2 tasks"))
        XCTAssertTrue(entries.contains(.item("Settings…", .settings, key: ",")))
        XCTAssertTrue(entries.contains(.item("Quit FERB", .quit, key: "q")))
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd ui/mac/FERBNotch && swift test --filter "NotificationTests|MenuBarStateTests"`
Expected: FAIL to compile — `cannot find 'BannerItem' in scope`, `cannot find 'DaemonLink' in scope`.

- [ ] **Step 3: Implement `Banners.swift`**

```swift
import Foundation

/// FERB's notification categories (L34 D), named as the daemon names them; the owner switches them in Settings
/// (`notifications.categories`, config.toml [banners]).
public enum BannerCategory: String, CaseIterable {
    case question, reminder, alert, taskFinished = "task_finished", loop

    public var title: String {
        switch self {
        case .question: return "Questions while the notch is closed"
        case .reminder: return "Reminders"
        case .alert: return "Alerts from things FERB watches"
        case .taskFinished: return "Tasks finishing"
        case .loop: return "Loops pausing or backing off"
        }
    }
}

/// What clicking a notification opens in the notch.
public enum NotificationTarget: Equatable {
    case question(approvalID: String, taskID: String?)
    case task(String)
    case reminder(String)
    case notch

    public init(_ target: [String: Any]) {
        let kind = target["kind"] as? String
        if kind == "question", let id = target["approval_id"] as? String, !id.isEmpty {
            let task = target["task_id"] as? String
            self = .question(approvalID: id, taskID: task?.isEmpty == false ? task : nil)
        } else if kind == "task", let id = target["task_id"] as? String, !id.isEmpty {
            self = .task(id)
        } else if kind == "reminder", let id = target["timer_id"] as? String, !id.isEmpty {
            self = .reminder(id)
        } else {
            self = .notch
        }
    }

    /// Read back from a clicked notification's userInfo.
    public init(userInfo: [AnyHashable: Any]) {
        var target: [String: Any] = [:]
        for (key, value) in userInfo { if let key = key as? String { target[key] = value } }
        self.init(target)
    }

    /// Kept in the notification's userInfo: ids only.
    public var userInfo: [String: String] {
        switch self {
        case .question(let id, let task): return ["kind": "question", "approval_id": id, "task_id": task ?? ""]
        case .task(let id): return ["kind": "task", "task_id": id]
        case .reminder(let id): return ["kind": "reminder", "timer_id": id]
        case .notch: return ["kind": "notch"]
        }
    }
}

/// One banner from `notifications.subscribe`. The daemon wrote its text from redacted data; FERB.app posts it as is.
public struct BannerItem: Equatable {
    public let id: String
    public let category: BannerCategory?
    public let title: String
    public let body: String
    public let target: NotificationTarget
    public let whenHidden: Bool

    public init?(_ event: [String: Any]) {
        guard let id = event["id"] as? String, !id.isEmpty, let title = event["title"] as? String,
              let body = event["body"] as? String else { return nil }
        self.id = id
        category = BannerCategory(rawValue: event["category"] as? String ?? "")
        self.title = title
        self.body = body
        target = NotificationTarget(event["target"] as? [String: Any] ?? [:])
        whenHidden = event["when_hidden"] as? Bool ?? false
    }
}

public enum NotificationGate {
    /// A banner marked when_hidden (a question, a task's answer, a loop) posts only while the notch isn't showing: open
    /// on screen, it is already in front of the owner. Reminders and alerts always post.
    public static func shouldPost(_ item: BannerItem, notchShowing: Bool) -> Bool { !(item.whenHidden && notchShowing) }

    /// The id a question's banner has (the daemon's `question:<approval id>`), so an answer withdraws it.
    public static func questionID(_ approvalID: String) -> String { "question:" + approvalID }
}
```

- [ ] **Step 4: Implement `MenuBarState.swift`**

```swift
import Foundation

/// Whether the notch app reaches the daemon (L34 D). One refused connection isn't "not running": launchd restarts a
/// crashed daemon within its 5 s throttle, and the connect loop retries every 2 s, so FERB counts as not running from the
/// second refusal in a row.
public struct DaemonLink: Equatable {
    public enum State: Equatable { case connecting, connected, unreachable }

    public private(set) var state: State = .connecting
    private var misses = 0
    public static let missesBeforeNotRunning = 2

    public init() {}

    public mutating func succeeded() {
        misses = 0
        state = .connected
    }

    /// `unreachable`: the socket refused or isn't there (`DaemonRPCError.unavailable`); anything else is a dropped
    /// connection, which reconnects.
    public mutating func failed(unreachable: Bool) {
        guard unreachable else {
            misses = 0
            state = .connecting
            return
        }
        misses += 1
        state = misses >= Self.missesBeforeNotRunning ? .unreachable : .connecting
    }

    /// The owner chose Restart: connecting again until it answers.
    public mutating func restarting() {
        misses = 0
        state = .connecting
    }
}

/// The menu bar's one state, most urgent first.
public enum MenuBarState: Equatable {
    case connecting, notRunning, needsYou(Int), stopped, paused, working(Int), ready

    public static func derive(link: DaemonLink.State, paused: Bool, stopped: Bool, questions: Int,
                              running: Int) -> MenuBarState {
        switch link {
        case .connecting: return .connecting
        case .unreachable: return .notRunning
        case .connected: break
        }
        if questions > 0 { return .needsYou(questions) }
        if stopped { return .stopped }
        if paused { return .paused }
        if running > 0 { return .working(running) }
        return .ready
    }

    /// SF Symbols available on macOS 14.
    public var symbol: String {
        switch self {
        case .connecting, .ready: return "waveform"
        case .working: return "ellipsis.circle"
        case .needsYou: return "exclamationmark.bubble"
        case .paused, .stopped: return "pause.circle"
        case .notRunning: return "exclamationmark.triangle"
        }
    }

    public var statusLine: String {
        switch self {
        case .connecting: return "Connecting to FERB…"
        case .ready: return "FERB is ready"
        case .working(let n): return n == 1 ? "Working on 1 task" : "Working on \(n) tasks"
        case .needsYou(let n): return n == 1 ? "FERB needs you: 1 question" : "FERB needs you: \(n) questions"
        case .paused: return "Paused: FERB records nothing"
        case .stopped: return "Stopped: tasks are blocked"
        case .notRunning: return "FERB isn't running — Restart"
        }
    }
}

public enum MenuAction: String, Equatable {
    case openFERB, openWindow, pause, resume, liftStop, restart, settings, setUp, quit
}

public enum MenuEntry: Equatable {
    case status(String)
    case separator
    case item(String, MenuAction, key: String)
}

public enum StatusMenu {
    /// The menu (spec: Open FERB, Open FERB Window, Pause FERB, Settings…, Quit): the state line first, then what the
    /// state needs. `hasWindow`: Phase E's FERB window exists. `canSetUp`: the packaged FERB.app (Set Up FERB…).
    public static func entries(state: MenuBarState, paused: Bool, stopped: Bool, appName: String, canSetUp: Bool,
                               hasWindow: Bool) -> [MenuEntry] {
        var entries: [MenuEntry] = [.status(state.statusLine), .separator, .item("Open FERB", .openFERB, key: "")]
        if hasWindow { entries.append(.item("Open FERB Window", .openWindow, key: "")) }
        if state == .notRunning {
            entries.append(.item("Restart FERB", .restart, key: ""))
        } else if state != .connecting {
            if stopped { entries.append(.item("Lift Emergency Stop", .liftStop, key: "")) }
            entries.append(paused ? .item("Resume FERB", .resume, key: "") : .item("Pause FERB", .pause, key: ""))
        }
        entries.append(.item("Settings…", .settings, key: ","))
        if canSetUp { entries.append(.item("Set Up FERB…", .setUp, key: "")) }
        entries += [.separator, .item("Quit \(appName)", .quit, key: "q")]
        return entries
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd ui/mac/FERBNotch && swift test --filter "NotificationTests|MenuBarStateTests"`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ui/mac/FERBNotch/Sources/NotchCore/Banners.swift ui/mac/FERBNotch/Sources/NotchCore/MenuBarState.swift \
  ui/mac/FERBNotch/Tests/NotchCoreTests/NotificationTests.swift ui/mac/FERBNotch/Tests/NotchCoreTests/MenuBarStateTests.swift
git commit -m "feat(L34): NotchCore banners, click targets and the menu bar's state"
```

---

### Task 5: `NotificationPoster`: FERB.app subscribes, posts, withdraws and opens on click

**Files:**
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/NotificationPoster.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/Support/NotificationFakes.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/NotificationPosterTests.swift`

**Interfaces:**
- Consumes: Task 4's `BannerItem`, `NotificationGate`, `NotificationTarget`; Phase C's `AppBundle.isApp`, `ScriptedRPC`.
- Produces (FERBNotch):
  - `@MainActor protocol NotificationCenterPosting: AnyObject { func authorized() async -> Bool; func post(id: String,
    title: String, body: String, userInfo: [String: String], thread: String); func withdraw(ids: [String]) }`
  - `@MainActor final class UserNotificationCenter: NotificationCenterPosting` (only built when `AppBundle.isApp`)
  - `@MainActor final class NotificationPoster` with `init(center: NotificationCenterPosting?)`,
    `configure(notchShowing:open:)`, `start(rpc:notchShowing:open:)`, `stop()`, `subscribeOnce(rpc:) async -> Bool`,
    `receive(_:)`, `withdrawQuestion(_:)`, `clicked(_:)`.
  - `final class NotificationClicks: NSObject, UNUserNotificationCenterDelegate` with `weak var poster`.

- [ ] **Step 1: Write the fakes and the failing tests**

`Support/NotificationFakes.swift`:

```swift
import Foundation
@testable import FERBNotch

@MainActor
final class FakeNotificationCenter: NotificationCenterPosting {
    var allowed = true
    private(set) var posted: [(id: String, title: String, body: String, userInfo: [String: String], thread: String)] = []
    private(set) var withdrawn: [String] = []

    func authorized() async -> Bool { allowed }
    func post(id: String, title: String, body: String, userInfo: [String: String], thread: String) {
        posted.append((id, title, body, userInfo, thread))
    }
    func withdraw(ids: [String]) { withdrawn += ids }
}
```

`NotificationPosterTests.swift`:

```swift
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 D: FERB.app as the daemon's poster.
@MainActor
final class NotificationPosterTests: XCTestCase {
    private let question: [String: Any] = [
        "id": "question:a1", "category": "question", "title": "FERB needs you", "body": "FERB needs your OK: Send it?",
        "target": ["kind": "question", "approval_id": "a1", "task_id": "t1"], "when_hidden": true]
    private let reminder: [String: Any] = [
        "id": "reminder:r1", "category": "reminder", "title": "FERB reminder", "body": "call Mom",
        "target": ["kind": "reminder", "timer_id": "r1"], "when_hidden": false]

    func testFERBAppPostsWhatTheDaemonSendsExactlyAsWritten() async {
        let center = FakeNotificationCenter()
        let rpc = ScriptedRPC()
        rpc.events["notifications.subscribe"] = [question, reminder]
        let poster = NotificationPoster(center: center)
        poster.configure(notchShowing: { false }, open: { _ in })
        let subscribed = await poster.subscribeOnce(rpc: rpc)
        XCTAssertTrue(subscribed)
        XCTAssertEqual(rpc.params(of: "notifications.subscribe").first?["authorized"] as? Bool, true)
        XCTAssertEqual(center.posted.map(\.id), ["question:a1", "reminder:r1"])
        XCTAssertEqual(center.posted[0].title, "FERB needs you")
        XCTAssertEqual(center.posted[0].body, "FERB needs your OK: Send it?")
        XCTAssertEqual(center.posted[0].userInfo, ["kind": "question", "approval_id": "a1", "task_id": "t1"])
        XCTAssertEqual(center.posted[1].thread, "reminder")
    }

    func testAnOpenNotchHoldsBackOnlyWhatItAlreadyShows() async {
        let center = FakeNotificationCenter()
        let rpc = ScriptedRPC()
        rpc.events["notifications.subscribe"] = [question, reminder]
        let poster = NotificationPoster(center: center)
        poster.configure(notchShowing: { true }, open: { _ in })
        _ = await poster.subscribeOnce(rpc: rpc)
        XCTAssertEqual(center.posted.map(\.id), ["reminder:r1"])
    }

    func testAnUnauthorizedAppNeverSubscribes() async {
        let center = FakeNotificationCenter()
        center.allowed = false
        let rpc = ScriptedRPC()
        let poster = NotificationPoster(center: center)
        let subscribed = await poster.subscribeOnce(rpc: rpc)
        XCTAssertFalse(subscribed)
        XCTAssertTrue(rpc.calls.isEmpty)
    }

    func testWithoutANotificationCenterThePosterNeverSubscribes() async {
        let rpc = ScriptedRPC()
        let poster = NotificationPoster(center: nil)
        let subscribed = await poster.subscribeOnce(rpc: rpc)
        XCTAssertFalse(subscribed)
        poster.start(rpc: rpc, notchShowing: { false }, open: { _ in })
        try? await Task.sleep(for: .milliseconds(50))
        XCTAssertTrue(rpc.calls.isEmpty)
        poster.receive(question)                                 // nothing to post with: no crash
        poster.stop()
    }

    func testAClickOpensTheNotchAtItsItemEvenBeforeTheNotchIsReady() {
        let poster = NotificationPoster(center: FakeNotificationCenter())
        poster.clicked(["kind": "task", "task_id": "t9"])        // a click that launched FERB.app
        var opened: [NotificationTarget] = []
        poster.configure(notchShowing: { false }, open: { opened.append($0) })
        XCTAssertEqual(opened, [.task("t9")])
        poster.clicked(["kind": "question", "approval_id": "a1", "task_id": ""])
        XCTAssertEqual(opened.last, .question(approvalID: "a1", taskID: nil))
    }

    func testAnAnsweredQuestionWithdrawsItsBanner() {
        let center = FakeNotificationCenter()
        let poster = NotificationPoster(center: center)
        poster.withdrawQuestion("a1")
        XCTAssertEqual(center.withdrawn, ["question:a1"])
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd ui/mac/FERBNotch && swift test --filter NotificationPosterTests`
Expected: FAIL to compile — `cannot find type 'NotificationCenterPosting' in scope`.

- [ ] **Step 3: Implement `NotificationPoster.swift`**

```swift
import AppKit
import NotchCore
import UserNotifications

/// Posting notifications, so tests stand in for UNUserNotificationCenter (it needs a real app bundle).
@MainActor
protocol NotificationCenterPosting: AnyObject {
    func authorized() async -> Bool
    func post(id: String, title: String, body: String, userInfo: [String: String], thread: String)
    func withdraw(ids: [String])
}

/// UNUserNotificationCenter for FERB.app (L34 D): FERB's own name and icon; Focus modes respected (macOS applies them to
/// the default interruption level); a click comes back through `NotificationClicks`. Built only when `AppBundle.isApp`.
@MainActor
final class UserNotificationCenter: NotificationCenterPosting {
    private let center = UNUserNotificationCenter.current()

    func authorized() async -> Bool {
        let status = await center.notificationSettings().authorizationStatus
        return status == .authorized || status == .provisional
    }

    func post(id: String, title: String, body: String, userInfo: [String: String], thread: String) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.userInfo = userInfo
        content.threadIdentifier = thread
        content.sound = .default
        center.add(UNNotificationRequest(identifier: id, content: content, trigger: nil)) { error in
            if let error { NSLog("FERB: a notification wasn't posted (%@).", error.localizedDescription) }
        }
    }

    func withdraw(ids: [String]) {
        center.removeDeliveredNotifications(withIdentifiers: ids)
        center.removePendingNotificationRequests(withIdentifiers: ids)
    }
}

/// FERB.app as the daemon's poster (L34 D): while macOS lets FERB.app notify, it holds `notifications.subscribe`, so the
/// daemon stops using osascript; each banner is posted unless the notch is already showing it; a click opens the notch at
/// its item. Without a notification center (a development build run without a bundle) it never subscribes, and the
/// daemon keeps osascript.
@MainActor
final class NotificationPoster {
    private let center: NotificationCenterPosting?
    private var loop: Task<Void, Never>?
    private var notchShowing: () -> Bool = { false }
    private var opener: ((NotificationTarget) -> Void)?
    private var pending: NotificationTarget?
    /// The wait before subscribing again after the daemon went away or macOS didn't allow notifications.
    static let retry: Duration = .seconds(5)

    init(center: NotificationCenterPosting?) { self.center = center }

    func configure(notchShowing: @escaping () -> Bool, open: @escaping (NotificationTarget) -> Void) {
        self.notchShowing = notchShowing
        opener = open
        if let pending {
            self.pending = nil
            open(pending)
        }
    }

    func start(rpc: DaemonRPCTransport, notchShowing: @escaping () -> Bool,
               open: @escaping (NotificationTarget) -> Void) {
        configure(notchShowing: notchShowing, open: open)
        guard center != nil, loop == nil else { return }
        loop = Task { [weak self] in
            while !Task.isCancelled {
                _ = await self?.subscribeOnce(rpc: rpc)
                do { try await Task.sleep(for: Self.retry) } catch { return }
            }
        }
    }

    func stop() {
        loop?.cancel()
        loop = nil
    }

    /// One subscription, until the daemon ends it or the connection drops. False when macOS doesn't let FERB.app notify
    /// (or there is no center): then FERB.app doesn't claim the poster role and osascript keeps posting.
    @discardableResult
    func subscribeOnce(rpc: DaemonRPCTransport) async -> Bool {
        guard let center, await center.authorized() else { return false }
        do {
            for try await event in rpc.stream("notifications.subscribe", params: ["authorized": true]) { receive(event) }
        } catch {}
        return true
    }

    func receive(_ event: [String: Any]) {
        guard let center, let item = BannerItem(event),
              NotificationGate.shouldPost(item, notchShowing: notchShowing()) else { return }
        center.post(id: item.id, title: item.title, body: item.body, userInfo: item.target.userInfo,
                    thread: item.category?.rawValue ?? "ferb")
    }

    /// A question answered anywhere: its banner goes from Notification Center.
    func withdrawQuestion(_ approvalID: String) { center?.withdraw(ids: [NotificationGate.questionID(approvalID)]) }

    func clicked(_ userInfo: [AnyHashable: Any]) {
        let target = NotificationTarget(userInfo: userInfo)
        if let opener { opener(target) } else { pending = target }
    }
}

/// UNUserNotificationCenter's delegate: banners also show while FERB.app is the active app, and a click reaches the
/// poster. Installed before launching finishes, so a click that launched FERB isn't lost.
final class NotificationClicks: NSObject, UNUserNotificationCenterDelegate {
    weak var poster: NotificationPoster?

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .list, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        let info = response.notification.request.content.userInfo
        Task { @MainActor [weak self] in
            self?.poster?.clicked(info)
            completionHandler()
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd ui/mac/FERBNotch && swift test --filter "NotificationPosterTests|NotificationTests"`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ui/mac/FERBNotch/Sources/FERBNotch/NotificationPoster.swift \
  ui/mac/FERBNotch/Tests/NotchCoreTests/Support/NotificationFakes.swift \
  ui/mac/FERBNotch/Tests/NotchCoreTests/NotificationPosterTests.swift
git commit -m "feat(L34): FERB.app posts FERB's banners with UNUserNotificationCenter and opens the notch on click"
```

---

### Task 6: The menu bar shows the daemon's real state; the notch opens at a clicked item

**Files:**
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/NotchModel.swift` (state properties, `menuBarState`, `open(_:)`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/DaemonConnection.swift` (`connectDaemon`, `refreshDaemonStatus`,
  `resolveApproval`, `unreachable(_:)`)
- Modify: `ui/mac/FERBNotch/Sources/NotchCore/DaemonRPC.swift` (`.unavailable` message)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/PanelCoordinator.swift` (status item, menu, actions, poster wiring)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/Application.swift` (poster, click delegate)
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/MenuBarModelTests.swift`

**Interfaces:**
- Consumes: Tasks 4–5; Phase C's `DaemonService`, `AppBundle`.
- Produces: `NotchModel.daemonLink: DaemonLink`, `.daemonPaused`, `.daemonStopped`, `.menuBarState: MenuBarState`,
  `.onApprovalResolved: ((String) -> Void)?`, `.open(_ target: NotificationTarget)`; `PanelCoordinator.poster`,
  `.hasWindow`, `.onOpenWindow`, `.perform(_:)`, `.open(_:)`.

- [ ] **Step 1: Write the failing tests** (`MenuBarModelTests.swift`)

```swift
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// L34 D: the facts behind the menu bar, and a clicked notification opening the notch at its item.
@MainActor
final class MenuBarModelTests: XCTestCase {
    private func makeModel(_ rpc: ScriptedRPC) -> NotchModel {
        NotchModel(rpc: rpc, preferences: UserDefaults(suiteName: "MenuBarModelTests-" + UUID().uuidString)!)
    }

    private func status(running: [[String: Any]] = [], paused: Bool = false, stopped: Bool = false) -> [String: Any] {
        ["running": running, "approvals": [[String: Any]](), "results": [[String: Any]](), "paused": paused,
         "stopped": stopped]
    }

    func testTwoRefusedConnectionsMeanFERBIsntRunning() async {
        let rpc = ScriptedRPC()
        rpc.failures["chat.status"] = DaemonRPCError.unavailable
        let model = makeModel(rpc)
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .connecting)
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .notRunning)
        rpc.failures["chat.status"] = nil
        rpc.replies["chat.status"] = status(running: [["task_id": "t1", "goal": "Rename files"]])
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .working(1))
    }

    func testPausedAndStoppedComeFromTheDaemon() async {
        let rpc = ScriptedRPC()
        let model = makeModel(rpc)
        rpc.replies["chat.status"] = status(paused: true)
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .paused)
        rpc.replies["chat.status"] = status(stopped: true)
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .stopped)
        rpc.replies["chat.status"] = status()
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.menuBarState, .ready)
    }

    func testAClickedQuestionOpensTheNotchAtItsField() {
        let model = makeModel(ScriptedRPC())
        model.pendingApprovals = [DaemonApproval(["approval_id": "a1", "kind": "owner_question", "reason": "Send it?"])!]
        model.open(.question(approvalID: "a1", taskID: nil))
        XCTAssertTrue(model.expanded)
        XCTAssertEqual(model.questionFocus?.id, "a1")
    }

    func testAClickedTaskOpensTheNotchAtTasks() {
        let model = makeModel(ScriptedRPC())
        model.runningTasks = ["t9": "Rename files"]
        model.open(.task("t9"))
        XCTAssertTrue(model.expanded && model.showingTasks)
        model.collapse()
        model.open(.task("gone"))                                // a task this notch doesn't know: just the notch
        XCTAssertTrue(model.expanded)
        XCTAssertFalse(model.showingTasks)
    }

    func testAnAnsweredQuestionTellsThePosterToWithdrawIt() {
        let model = makeModel(ScriptedRPC())
        var withdrawn: [String] = []
        model.onApprovalResolved = { withdrawn.append($0) }
        model.pendingApprovals = [DaemonApproval(["approval_id": "a1", "kind": "owner_question", "reason": "Send it?"])!]
        model.receiveBusEvent(["topic": "approval.resolved", "data": ["approval_id": "a1"]])
        XCTAssertEqual(withdrawn, ["a1"])
        XCTAssertTrue(model.pendingApprovals.isEmpty)
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd ui/mac/FERBNotch && swift test --filter MenuBarModelTests`
Expected: FAIL to compile — `value of type 'NotchModel' has no member 'menuBarState'`.

- [ ] **Step 3: Implement the model side**

`NotchModel.swift`, after `@Published var connectionStatus = …`:

```swift
    /// L34 D: the daemon as the menu bar shows it.
    @Published var daemonLink = DaemonLink()
    @Published var daemonPaused = false
    @Published var daemonStopped = false
    /// L34 D: a question was answered (here or elsewhere), so its banner, if any, is withdrawn.
    var onApprovalResolved: ((String) -> Void)?

    var menuBarState: MenuBarState {
        MenuBarState.derive(link: daemonLink.state, paused: daemonPaused, stopped: daemonStopped,
                            questions: pendingApprovals.count, running: runningTasks.count)
    }
```

and, after `focusQuestion(_:)`:

```swift
    /// A notification was clicked (L34 D): the notch opens at its item — the question's field, the task in Tasks, or
    /// today's chat for a reminder.
    func open(_ target: NotificationTarget) {
        if !viewingToday { selectDay(Date()) }
        expand()
        switch target {
        case .question(let id, _):
            focusQuestion(id)
        case .task(let id):
            showingTasks = taskItems.contains { $0.id == id }
            onPresentationChange?(true)
        case .reminder, .notch:
            break
        }
    }
```

`DaemonConnection.swift`:

In `connectDaemon()`, change the loop body's `do`/`catch`:

```swift
                do {
                    try await self.attachDaemon()
                    self.daemonLink.succeeded()
                    await self.refreshDaemonStatus()
                    for try await event in self.rpc.stream("logs.subscribe", params: [:]) {
                        guard !Task.isCancelled else { return }
                        self.receiveBusEvent(event)
                    }
                    self.connectionStatus = "Reconnecting to FERB…"
                } catch {
                    self.connectionStatus = error.localizedDescription
                    self.daemonLink.failed(unreachable: Self.unreachable(error))
                }
```

In `refreshDaemonStatus()`, right after `let status = try await rpc.call("chat.status", params: ["peek": true])`:

```swift
            daemonLink.succeeded()
            daemonPaused = status["paused"] as? Bool ?? false
            daemonStopped = status["stopped"] as? Bool ?? false
```

and its final `catch` becomes:

```swift
        } catch {
            connectionStatus = error.localizedDescription
            daemonLink.failed(unreachable: Self.unreachable(error))
        }
```

At the top of `resolveApproval(_:)`:

```swift
        if let approvalID { onApprovalResolved?(approvalID) }
```

Add to the extension:

```swift
    /// The socket refused or isn't there: FERB's daemon isn't running (or is restarting).
    static func unreachable(_ error: Error) -> Bool {
        if case DaemonRPCError.unavailable = error { return true }
        return false
    }
```

`DaemonRPC.swift`: `case .unavailable: return "FERB isn't running. Choose Restart FERB in FERB's menu bar."`

- [ ] **Step 4: Run the model tests**

Run: `cd ui/mac/FERBNotch && swift test --filter "MenuBarModelTests|DaemonConnectionTests|QuestionsAndNoticesTests"`
Expected: PASS.

- [ ] **Step 5: Implement the menu bar** in `PanelCoordinator.swift`

Add `import Combine` to the imports and `NSMenuDelegate` to the class's conformances:

```swift
final class PanelCoordinator: NSObject, NSWindowDelegate, NSMenuDelegate, @preconcurrency QLPreviewPanelDataSource, QLPreviewPanelDelegate {
```

Add properties next to `onSetUp`:

```swift
    var poster: NotificationPoster?              // set by AppDelegate before start() (L34 D)
    var hasWindow = false                        // Phase E: the FERB window exists ("Open FERB Window")
    var onOpenWindow: (() -> Void)?              // Phase E
    private let service = DaemonService()
    private var statusRefresh: AnyCancellable?
    private var statusRefreshQueued = false
```

In `start()`, right after `configureMenu()`:

```swift
        poster?.start(rpc: model.rpc, notchShowing: { [weak self] in self?.notchShowing ?? false },
                      open: { [weak self] target in self?.open(target) })
        model.onApprovalResolved = { [weak self] id in self?.poster?.withdrawQuestion(id) }
        statusRefresh = model.objectWillChange.sink { [weak self] _ in self?.queueStatusRefresh() }
```

Replace `configureMenu()`, `addItem(_:action:to:key:)` and the selectors `typeRequest`, `voicePreview`,
`recentExamples`, `quit`, `setUpFromMenu` with:

```swift
    private func configureMenu() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        let menu = NSMenu()
        menu.delegate = self
        item.menu = menu
        statusItem = item
        updateStatusItem()
        rebuildMenu(menu)
        if let appMenu = NSApp.mainMenu?.items.first?.submenu {
            let settings = NSMenuItem(title: "Settings…", action: #selector(showSettings), keyEquivalent: ",")
            settings.target = self
            appMenu.insertItem(settings, at: 0)
            appMenu.insertItem(.separator(), at: 1)
        }
    }

    /// The notch is in front of the owner: open, not stepped back, and the displays awake.
    private var notchShowing: Bool { model.bodyVisible && !model.isYielded && !screensAsleep }

    /// A clicked notification: the owner reached for FERB, so it comes forward at that item.
    func open(_ target: NotificationTarget) {
        beginInteraction()
        model.open(target)
    }

    private func queueStatusRefresh() {
        guard !statusRefreshQueued else { return }
        statusRefreshQueued = true
        DispatchQueue.main.async { [weak self] in        // objectWillChange fires before the change lands
            guard let self else { return }
            self.statusRefreshQueued = false
            self.updateStatusItem()
        }
    }

    private func updateStatusItem() {
        let state = model.menuBarState
        statusItem?.button?.image = NSImage(systemSymbolName: state.symbol, accessibilityDescription: state.statusLine)
        statusItem?.button?.toolTip = state.statusLine
    }

    func menuNeedsUpdate(_ menu: NSMenu) { rebuildMenu(menu) }

    private func rebuildMenu(_ menu: NSMenu) {
        menu.removeAllItems()
        for entry in StatusMenu.entries(state: model.menuBarState, paused: model.daemonPaused,
                                        stopped: model.daemonStopped, appName: AppName.current,
                                        canSetUp: onSetUp != nil, hasWindow: hasWindow) {
            switch entry {
            case .status(let text):
                let line = NSMenuItem(title: text, action: nil, keyEquivalent: "")
                line.isEnabled = false
                menu.addItem(line)
            case .separator:
                menu.addItem(.separator())
            case .item(let title, let action, let key):
                let item = NSMenuItem(title: title, action: #selector(menuAction(_:)), keyEquivalent: key)
                item.target = self
                item.representedObject = action.rawValue
                menu.addItem(item)
            }
        }
    }

    @objc private func menuAction(_ sender: NSMenuItem) {
        guard let raw = sender.representedObject as? String, let action = MenuAction(rawValue: raw) else { return }
        perform(action)
    }

    func perform(_ action: MenuAction) {
        switch action {
        case .openFERB: showFromMenu()
        case .openWindow: onOpenWindow?()
        case .pause: setPaused(true)
        case .resume: setPaused(false)
        case .liftStop: liftStop()
        case .restart: restartDaemon()
        case .settings: showSettings()
        case .setUp: onSetUp?()
        case .quit: NSApp.terminate(nil)
        }
    }

    /// Pause FERB is `ferb pause` (sense.pause): nothing is recorded and no real mouse or keyboard input until Resume.
    private func setPaused(_ paused: Bool) {
        Task { [weak self] in
            guard let self else { return }
            do {
                _ = try await self.model.rpc.call(paused ? "sense.pause" : "sense.resume", params: [:])
                self.model.daemonPaused = paused
            } catch { self.model.promptError = error.localizedDescription }
        }
    }

    private func liftStop() {
        Task { [weak self] in
            guard let self else { return }
            do {
                _ = try await self.model.rpc.call("stop.lift", params: [:])
                self.model.daemonStopped = false
            } catch { self.model.promptError = error.localizedDescription }
        }
    }

    /// FERB isn't running — Restart: launchd starts the installed background service again; with none installed, the
    /// packaged app opens Set Up FERB… and a development build says how.
    private func restartDaemon() {
        Task { [weak self] in
            guard let self else { return }
            if await self.service.restart() {
                self.model.daemonLink.restarting()
            } else if let setUp = self.onSetUp {
                setUp()
            } else {
                let alert = NSAlert()
                alert.messageText = "FERB's background service isn't installed."
                alert.informativeText = "From a checkout, run `ferb setup`."
                NSApp.activate(ignoringOtherApps: true)
                alert.runModal()
            }
        }
    }
```

Keep `@objc func showFromMenu()` and `@objc private func showSettings()` as they are. In `tearDown()`, add before
`model.disconnectDaemon()`:

```swift
        poster?.stop()
        statusRefresh = nil
```

- [ ] **Step 6: Wire the app** — in `Application.swift` add `import UserNotifications`, two properties on `AppDelegate`:

```swift
    private let poster = NotificationPoster(center: AppBundle.isApp ? UserNotificationCenter() as NotificationCenterPosting : nil)
    private let clicks = NotificationClicks()
```

and at the very start of `applicationDidFinishLaunching(_:)`:

```swift
        clicks.poster = poster
        if AppBundle.isApp { UNUserNotificationCenter.current().delegate = clicks }    // before launching finishes
```

and right after `coordinator = PanelCoordinator()`:

```swift
        coordinator?.poster = poster
```

- [ ] **Step 7: Run all Swift tests and build**

Run: `cd ui/mac/FERBNotch && swift test && ./build.sh`
Expected: all PASS; `Built: …/build/FERB Notch.app`.

- [ ] **Step 8: Commit**

```bash
git add ui/mac/FERBNotch/Sources/FERBNotch/NotchModel.swift ui/mac/FERBNotch/Sources/FERBNotch/DaemonConnection.swift \
  ui/mac/FERBNotch/Sources/NotchCore/DaemonRPC.swift ui/mac/FERBNotch/Sources/FERBNotch/PanelCoordinator.swift \
  ui/mac/FERBNotch/Sources/FERBNotch/Application.swift ui/mac/FERBNotch/Tests/NotchCoreTests/MenuBarModelTests.swift
git commit -m "feat(L34): the menu bar shows FERB's real state with Pause, Restart and Lift Emergency Stop; a notification click opens the notch at its item"
```

---

### Task 7: Docs, the decision record, full verification and the owner's live check

**Files:**
- Modify: `README.md` (§ "Everyday use")
- Modify: `ARCHITECTURE.md` (§ 16.15 FERB for everyone (L34))
- Modify: `CHANGELOG.md`
- Modify: `docs/0.27_DECISIONS.toml` (`[L34]` tests)
- Modify: `docs/OWNER_ACCEPTANCE.md`

- [ ] **Step 1: README** — after the first paragraph of "## Everyday use" add:

```markdown
FERB.app posts FERB's notifications itself, with FERB's name and icon: a question while the notch is closed, a reminder,
an alert from something FERB watches, a task's answer that arrives while the notch is closed, and a loop pausing or
backing off. Clicking one opens the notch at that question or task. Focus modes apply. Each kind can be switched off
(config.toml `[banners]`, Settings → Notifications). Until you allow notifications for FERB (in Set Up FERB…'s
Permissions step, or System Settings → Notifications → FERB), and whenever FERB.app isn't running, FERB posts them
through macOS's script notifications (shown as Script Editor), as before. The menu bar icon shows FERB's state: ready,
working, needs you, paused, or "FERB isn't running — Restart"; its menu has Open FERB, Pause FERB (nothing is recorded
until Resume), Settings… and Quit.
```

- [ ] **Step 2: ARCHITECTURE.md** — inside `### 16.15 FERB for everyone (L34)` add:

```markdown
#### Phase D — notifications and menu bar state

The daemon decides what to tell, as before (`_task_result`, `_approval_notice`, `_timer_fired`, `_post_notice`, the idle
hold), now as a `Banner` (`ferb/daemon/banners.py`) with a category (`question`, `reminder`, `alert`, `task_finished`,
`loop`), a click target and `when_hidden`. `_notify` drops a category the owner switched off (config `[banners]`,
`notifications.categories`); `_deliver` hands the banner, title and body redacted, to FERB.app while it holds
`notifications.subscribe` (`Posters`), else runs osascript (`_notify_safely`, unchanged). With FERB.app as the poster a
question goes to it even when its chat is attached, and a finished task even when a chat showed it, both `when_hidden`;
FERB.app (`NotificationPoster`) posts through `UNUserNotificationCenter` unless the notch is showing (open, not stepped
back, displays awake), withdraws a question's banner when it is answered, and opens the notch at the item on a click
(`NotchModel.open`). FERB.app subscribes only when macOS lets it notify. Notification Center rows from `com.ferb.FERB` or
`dev.ferb.notch` are FERB's own (perception `_own_banner`). The menu bar derives `MenuBarState` from `DaemonLink` (two
refused connections in a row = not running), `chat.status`'s `paused` and `stopped`, the open questions and running
tasks; `StatusMenu.entries` builds the menu (Pause = `sense.pause`, Restart = `DaemonService.restart`, Lift Emergency
Stop = `stop.lift`). Debugging: journal `banners.switched`; `ferb logs` shows each `approval.requested`/`task.result`
that a banner follows.
```

- [ ] **Step 3: CHANGELOG.md** — add (or append to the L34 entry):

```markdown
## L34 FERB for everyone — Phase D: notifications and menu bar — 2026-10-25 — checkout; owner live check pending

- FERB.app posts FERB's banners with UNUserNotificationCenter (FERB's name and icon, Focus respected) while it holds
  the new `notifications.subscribe`; osascript remains the fallback. Clicking a banner opens the notch at its item; an
  answered question's banner is withdrawn.
- Five categories with switches in config.toml `[banners]` (`notifications.categories`): question, reminder, alert,
  task_finished, loop.
- `chat.status` adds `paused` and `stopped`. The menu bar icon and menu show ready, working, needs you, paused, stopped
  or "FERB isn't running — Restart"; Pause FERB, Resume FERB, Lift Emergency Stop and Restart FERB act on it.
- Tests: tests/test_banners.py, tests/test_app_notifications.py, tests/test_schedules.py, NotificationTests.swift,
  MenuBarStateTests.swift, NotificationPosterTests.swift, MenuBarModelTests.swift.
```

- [ ] **Step 4: Decision record** — append to `[L34]`'s `tests` in `docs/0.27_DECISIONS.toml` (Phase C or A created
  the entry; if it is absent, add it as in Phase C Task 9 Step 4):

```toml
         "tests/test_banners.py", "tests/test_app_notifications.py",
         "ui/mac/FERBNotch/Tests/NotchCoreTests/NotificationTests.swift",
         "ui/mac/FERBNotch/Tests/NotchCoreTests/MenuBarStateTests.swift",
         "ui/mac/FERBNotch/Tests/NotchCoreTests/NotificationPosterTests.swift",
         "ui/mac/FERBNotch/Tests/NotchCoreTests/MenuBarModelTests.swift"
```

- [ ] **Step 5: Owner acceptance** — append to `docs/OWNER_ACCEPTANCE.md`:

```markdown
## Notifications and menu bar (L34 Phase D) — pending live check (2026-10-25)

Checkout validation is in the changelog entry; nothing here is installed or observed yet. Run on the second macOS user
account after Phase C's check, the owner watching; pause between supervised actions and record what the owner saw.

- Before allowing notifications: ask "remind me in 1 minute to stretch"; the reminder arrives as Script Editor's banner.
  Then allow Notifications in Set Up FERB… → Permissions: the next reminder arrives as **FERB** with FERB's icon.
- Close the notch and ask for a task that needs a question on a disposable target; the question arrives as a FERB
  banner; clicking it opens the notch at the question's field. Answer it in the notch: the banner leaves Notification
  Center. With the notch open, a new question shows no banner.
- Close the notch during a longer disposable task: its answer arrives as "FERB finished"; clicking opens Tasks. Record
  whether anything took focus when a banner arrived (nothing should).
- Turn on a Focus mode that silences FERB: no banner shows; turn it off.
- Switch reminders off (Settings → Notifications once Phase E exists, or `notifications.categories {set: {reminder:
  false}}`): a reminder posts no banner and is still in the chat; switch it back on.
- Menu bar: the icon reads ready, then working during a task, needs you while a question waits. Pause FERB → paused
  (Resume FERB in the menu); `launchctl bootout gui/$(id -u)/com.ferb.daemon` → within a few seconds "FERB isn't running
  — Restart"; Restart brings it back to ready.
- Quit FERB.app with the daemon running and trigger a reminder: it arrives through Script Editor (the fallback).
```

- [ ] **Step 6: Full verification**

Run: `nice -n 10 uv run pytest -q && uv run mypy ferb/core && git diff --check && (cd ui/mac/FERBNotch && swift test)`
Expected: all PASS, no whitespace errors. Report checkout validation, installation status (not installed) and the
owner's live results (pending) separately.

- [ ] **Step 7: Commit**

```bash
git add README.md ARCHITECTURE.md CHANGELOG.md docs/0.27_DECISIONS.toml docs/OWNER_ACCEPTANCE.md
git commit -m "docs(L34): Phase D notifications and menu bar in README, ARCHITECTURE, CHANGELOG, the decision record and pending live checks"
```
