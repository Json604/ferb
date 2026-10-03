"""Build step 9, offline: screenshot + click on fake window / capture layers. Only apps on [privacy] exclude_apps
are refused (default: password managers; the owner may edit or empty it); a password field the Accessibility tree
shows, before or after capture, drops the frame; when the tree can't tell, the capture goes ahead. Clicks map image
pixels to the window, refuse a stale shot / moved window / something on top (checked before each event), and yield
to the owner. The image reaches the model and nothing else: not spans, journal, logs or any file.
"""

import asyncio
import base64
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import pytest

from ferb.act.gui_lock import GuiLock
from ferb.act.mac.accessibility import AxTree
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.input import RealInput, Step, Yielded
from ferb.act.mac.screenshot import Window
from ferb.config import DEFAULT_EXCLUDE_APPS, Config
from ferb.core.errors import Refused, StaleRef
from ferb.core.types import Identity
from tests.conftest import RunningDaemon
from tests.test_mac_shell import FakeApps, FakeAx, call, task

BOUNDS = (100.0, 50.0, 800.0, 600.0)              # window, screen points
IMG = (1600, 1200)                                 # captured at 2x


@dataclass
class FakeShots:
    """The capture layer: one window per app pid, a controllable topmost window, and a record of captures."""
    jpeg: bytes = field(default_factory=lambda: b"\xff\xd8\xff" + os.urandom(24))
    bounds: tuple[float, float, float, float] = BOUNDS
    top: int | None = None                         # window id at any point (None: the captured window itself)
    captured: list[int] = field(default_factory=list)
    on_capture: Any = None
    takes_pid_clicks: bool = True                  # a click posted to the app's process changes something

    def front_window(self, pid: int, like: Any = None) -> Window | None:
        return Window(pid * 10, pid, self.bounds, "w")

    def window(self, wid: int) -> Window | None:
        return Window(wid, wid // 10, self.bounds, "w")

    def on_screen(self, wid: int) -> bool:
        return True

    def window_at(self, x: float, y: float, max_layer: int = 0) -> Window | None:
        return Window(self.top if self.top is not None else 110, 0, self.bounds, "w")

    def capture(self, w: Window, marks: Any = None) -> tuple[bytes, int, int]:
        self.captured.append(w.id)
        self.marks = marks
        if self.on_capture:
            self.on_capture()
        return self.jpeg, *IMG


class VisionAx(FakeAx):
    def __init__(self) -> None:
        super().__init__(AxTree())
        self.secure: list[bool | None] = []        # the password-field answer of each walk, in order (then False)
        self.hit_answer: Any = None                # None: Accessibility can't hit-test (the window list decides)
        self.hit_el: Any = None                    # the app's own hit test: the element under the point
        self.clicks = 0                            # clicks posted to the app's process that it took

    def read_tree(self, pid: int, browser: bool = False, window: Any = None) -> AxTree:
        secure = self.secure.pop(0) if self.secure else False
        return AxTree(self.tree.nodes, f"{self.tree.text} clicks={self.clicks}", self.tree.title, secure=secure,
                      window="win")

    def app_window(self, pid: int) -> Any:
        return "win"

    def frame(self, win: Any) -> Any:
        return BOUNDS

    def window_with_frame(self, pid: int, bounds: Any) -> Any:
        return "win"

    def hit(self, x: float, y: float) -> Any:
        return self.hit_answer

    def hit_element(self, pid: int, x: float, y: float) -> Any:
        return self.hit_el

    def describe(self, el: Any) -> str:
        return f"button “{el}”"

    def raise_window(self, pid: int, bounds: Any) -> bool:
        self.raised = getattr(self, "raised", 0) + 1
        return True


def vision(mp: pytest.MonkeyPatch, owner: Any = lambda: False,
           excluded: Any = lambda: set(DEFAULT_EXCLUDE_APPS)) -> tuple[MacComputerDriver, VisionAx, FakeShots,
                                                                         FakeApps, list[str]]:
    ax, shots, apps, posted = VisionAx(), FakeShots(), FakeApps(), []
    apps.apps.append(RunningApp("com.apple.Passwords", "Passwords", 14, False))

    async def no_sleep(s: float) -> None:
        return None
    d = MacComputerDriver("dev", RealInput(owner, GuiLock(), sleep=no_sleep, yield_after_s=0.2, poll_s=0.1),
                          excluded=excluded, ax=ax, apps=apps, shots=shots)
    import ferb.act.mac.driver as drv
    mp.setattr(drv, "SETTLE_S", 0)

    def clicks(x: float, y: float, pid: int | None = None) -> list[Step]:
        where = " to pid" if pid else ""

        def click() -> None:
            posted.append(f"click {x},{y}{where}")
            if pid and shots.takes_pid_clicks:                 # the window is on this Space: the app took it
                ax.clicks += 1
        return [Step("move", lambda: posted.append(f"move {x},{y}{where}")), Step("click", click)]
    mp.setattr(drv, "click_steps", clicks)
    return d, ax, shots, apps, posted


# ---------- what may be captured ----------

def test_only_excluded_apps_are_refused_and_the_list_is_the_owners(monkeypatch: pytest.MonkeyPatch,
                                                                  tmp_path: Path) -> None:
    d, _, shots, *_ = vision(monkeypatch)
    with pytest.raises(Refused, match="exclusion"):
        asyncio.run(d.screenshot("Passwords"))
    assert shots.captured == []                                          # refused before any capture
    for app in ("Notes", "Brave"):                                       # no built-in refusals: browsers too
        asyncio.run(d.screenshot(app))
    assert len(shots.captured) == 2
    emptied, *_ = vision(monkeypatch, excluded=lambda: set())
    asyncio.run(emptied.screenshot("Passwords"))                         # the owner emptied the list: allowed

    c = Config(tmp_path / "home")
    c.ensure_privacy_defaults()
    assert c.get("privacy", "exclude_apps") == DEFAULT_EXCLUDE_APPS
    assert {"com.apple.Passwords", "com.apple.keychainaccess", "com.bitwarden.desktop"} <= c.exclude_apps()
    c.set("privacy", "exclude_apps", [])
    c.ensure_privacy_defaults()
    assert c.exclude_apps() == frozenset()                               # an emptied list stays empty


def test_a_detectable_password_field_drops_the_frame_and_unknown_is_allowed(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, *_ = vision(monkeypatch)
    ax.secure = [True]
    with pytest.raises(Refused, match="password"):
        asyncio.run(d.screenshot("Notes"))
    assert shots.captured == []                                          # seen before: never captured
    ax.secure = [False, True]
    with pytest.raises(Refused, match="password"):
        asyncio.run(d.screenshot("Notes"))                               # appeared during capture: dropped
    assert d._state["com.apple.Notes"].shot is None                      # no shot recorded to click on
    ax.secure = [None, None]
    _, shot, jpeg = asyncio.run(d.screenshot("Notes"))                   # the tree can't tell: allowed
    assert jpeg == shots.jpeg and shot.id == "shot1" and shot.size == IMG


def test_a_browser_tab_the_extension_reports_with_a_password_field_drops_the_frame(
        monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, *_ = vision(monkeypatch)
    answers: list[bool | None] = []
    asked: list[tuple[str, Any]] = []

    async def web(browser: str, bounds: Any) -> bool | None:
        asked.append((browser, bounds))
        return answers.pop(0) if answers else None
    d.web_password = web
    answers[:] = [True]
    with pytest.raises(Refused, match="password"):
        asyncio.run(d.screenshot("Brave"))
    assert shots.captured == [] and asked == [("brave", BOUNDS)]
    answers[:] = [False, True]                                           # appeared while capturing
    with pytest.raises(Refused, match="password"):
        asyncio.run(d.screenshot("Brave"))
    answers[:] = [None, None]                                            # no profile could tell: captured
    asyncio.run(d.screenshot("Brave"))
    asked.clear()
    asyncio.run(d.screenshot("Notes"))
    assert asked == []                                                   # only browsers are asked


# ---------- clicks ----------

def test_clicks_map_image_pixels_to_the_window_and_refuse_stale_moved_or_covered(monkeypatch: pytest.MonkeyPatch) -> None:
    d, _, shots, apps, posted = vision(monkeypatch)
    _, shot, _ = asyncio.run(d.screenshot("Notes"))
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300))              # image px at 2x
    assert r.ok and "background" in r.detail and posted == ["move 300.0,200.0 to pid", "click 300.0,200.0 to pid"]
    assert apps.activated == []                                          # the window took it: nothing moved
    posted.clear()
    shots.takes_pid_clicks = False                                       # it didn't: the real mouse, by leave only
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300))
    assert not r.ok and "allow_foreground" in r.detail and apps.activated == []
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300, allow_foreground=True))
    assert r.ok and posted[-2:] == ["move 300.0,200.0", "click 300.0,200.0"]
    assert getattr(d.ax, "raised", 0) >= 1                               # the window itself was raised first
    assert apps.activated == ["com.apple.Notes"]                         # brought forward for the stretch
    d.restore_owner()
    assert apps.activated == ["com.apple.Notes", "com.brave.Browser"]   # then the owner's app put back, once
    with pytest.raises(Refused, match="outside"):
        asyncio.run(d.click_at("Notes", shot.id, 1600, 10))
    asyncio.run(d.screenshot("Notes"))
    with pytest.raises(StaleRef):
        asyncio.run(d.click_at("Notes", shot.id, 10, 10))              # a newer screenshot supersedes
    posted.clear()
    shots.bounds = (120.0, 50.0, 800.0, 600.0)
    with pytest.raises(StaleRef, match="moved"):
        asyncio.run(d.click_at("Notes", "shot2", 10, 10))
    assert posted == []
    shots.bounds = BOUNDS
    shots.top = 999                                                      # another window over that point
    with pytest.raises(StaleRef, match="another window"):
        asyncio.run(d.click_at("Notes", "shot2", 10, 10, allow_foreground=True))
    assert posted[-1].endswith("to pid")                                 # nothing reached the real mouse


def test_a_click_at_a_point_tries_the_apps_own_hit_test_first(monkeypatch: pytest.MonkeyPatch) -> None:
    """The point becomes an element (AXUIElementCopyElementAtPosition on the app element) and is pressed through
    Accessibility: no mouse event at all, in front or behind."""
    d, ax, shots, apps, posted = vision(monkeypatch)
    _, shot, _ = asyncio.run(d.screenshot("Notes"))
    ax.hit_el = "Export"
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300))
    assert r.ok and "pressed the button “Export” under (400, 300)" in r.detail
    assert posted == [] and apps.activated == [] and ax.calls[-1] == "ax.press"
    assert d.profiles.prefers("com.apple.Notes", "hit_test") and d.profiles.prefers("com.apple.Notes", "ax_press")
    ax.press_ok = False                                                  # the element refuses: the pid click next
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300))
    assert r.ok and posted == ["move 300.0,200.0 to pid", "click 300.0,200.0 to pid"]


def test_the_accessibility_hit_test_decides_when_it_can(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, _, posted = vision(monkeypatch)
    shots.takes_pid_clicks = False                                       # the foreground path (by leave)
    asyncio.run(d.screenshot("Notes"))
    shots.top = 999                                                      # the window list sees an overlay...
    ax.hit_answer = (11, BOUNDS)                                         # ...that ignores clicks: Notes gets it
    assert asyncio.run(d.click_at("Notes", "shot1", 10, 10, allow_foreground=True)).ok and posted
    posted.clear()
    shots.top = None
    for answer in ((12, BOUNDS), (11, (110.0, 60.0, 160.0, 160.0)), (11, None)):   # another app / window / a menu
        ax.hit_answer = answer
        with pytest.raises(StaleRef):
            asyncio.run(d.click_at("Notes", "shot1", 10, 10, allow_foreground=True))
    assert not [p for p in posted if not p.endswith("to pid")]


def test_the_topmost_check_runs_before_every_event(monkeypatch: pytest.MonkeyPatch) -> None:
    d, _, shots, _, posted = vision(monkeypatch)
    shots.takes_pid_clicks = False
    asyncio.run(d.screenshot("Notes"))
    import ferb.act.mac.driver as drv

    def cover() -> None:
        posted.append("move")
        shots.top = 999                                                  # a window pops up after the move
    monkeypatch.setattr(drv, "click_steps", lambda x, y, pid=None: [Step("move", cover if pid is None else
                                                                         lambda: posted.append("move to pid")),
                                                                    Step("click", lambda: posted.append("click"))])
    with pytest.raises(StaleRef):
        asyncio.run(d.click_at("Notes", "shot1", 10, 10, allow_foreground=True))
    assert posted == ["move to pid", "click", "move"]                    # the real click itself never happened


def test_clicks_yield_to_the_owner(monkeypatch: pytest.MonkeyPatch) -> None:
    d, _, shots, apps, posted = vision(monkeypatch, owner=lambda: True)
    shots.takes_pid_clicks = False
    asyncio.run(d.screenshot("Notes"))
    with pytest.raises(Yielded):
        asyncio.run(d.click_at("Notes", "shot1", 10, 10, allow_foreground=True))
    assert not [p for p in posted if not p.endswith("to pid")] and apps.activated == []   # the owner never moved


# ---------- through the gateway ----------

def _fake_mac(d: RunningDaemon, mp: pytest.MonkeyPatch) -> tuple[FakeShots, list[str]]:
    drv, ax, shots, apps, posted = vision(mp)
    mac = d.daemon.mac
    assert mac is not None
    mac.ax, mac.apps, mac.shots, mac.input = ax, apps, shots, drv.input
    return shots, posted


def test_the_image_reaches_the_model_and_nothing_else(daemon: RunningDaemon, monkeypatch: pytest.MonkeyPatch) -> None:
    shots, posted = _fake_mac(daemon, monkeypatch)
    ferb = task(daemon, "f", Identity.FERB, {"fs:read"}, None)
    assert not [t for t in daemon.daemon.gateway.tools_for(ferb) if t["name"] in ("mac.screenshot", "mac.click")]
    owner = task(daemon, "v", Identity.OWNER, {"screen", "mac:act"}, {"use_app"})
    r = call(daemon, owner, "mac.screenshot", app="Notes")
    assert not r["is_error"] and base64.b64decode(r["image"]["data"]) == shots.jpeg
    assert r["image"]["mime"] == "image/jpeg" and r["content"]["shot"] == "shot1"
    r = call(daemon, owner, "mac.click", app="Notes", shot="shot1", x=400, y=300)
    assert not r["is_error"] and posted[-1] == "click 300.0,200.0 to pid", r
    b64 = base64.b64encode(shots.jpeg)
    daemon.run(_checkpoint(daemon))
    for p in daemon.daemon.home.rglob("*"):                             # database, WAL, logs, anything
        if p.is_file() and not p.is_socket():
            data = p.read_bytes()
            assert shots.jpeg[3:] not in data and b64 not in data, p
    assert not daemon.daemon.engine.exposure.state("w-v").exposed        # a notes window is trusted


def test_screenshots_of_browsers_and_messages_expose_the_worker(daemon: RunningDaemon,
                                                                monkeypatch: pytest.MonkeyPatch) -> None:
    _fake_mac(daemon, monkeypatch)
    daemon.daemon.mac.apps.apps.append(RunningApp("com.apple.mail", "Mail", 15, False))   # type: ignore[union-attr]
    for tid, app in (("b", "Brave"), ("m", "Mail")):
        g = task(daemon, tid, Identity.OWNER, {"screen", "browser:read"}, {"use_app"})
        assert not call(daemon, g, "mac.screenshot", app=app)["is_error"]
        assert daemon.daemon.engine.exposure.state(f"w-{tid}").exposed, app


async def _checkpoint(d: RunningDaemon) -> None:
    d.daemon.db.conn.execute("PRAGMA wal_checkpoint(FULL)")


def test_native_browser_controls_do_not_need_a_fallback_permission(daemon, monkeypatch):
    shots, _ = _fake_mac(daemon, monkeypatch)
    shots.top = 120
    g = task(daemon, "e", Identity.OWNER, {"screen", "mac:act", "browser:read"}, {"use_app"})
    r = call(daemon, g, "mac.screenshot", app="Brave")
    assert not r["is_error"], r
    r = call(daemon, g, "mac.click", app="Brave", shot="shot1", x=5, y=5)
    assert not r["is_error"], r


async def _span_inputs(d: RunningDaemon) -> list[str]:
    return [str(r[0]) for r in d.daemon.db.conn.execute("SELECT input_json FROM spans").fetchall()]


# ---------- the image with the tree (owner request, 2026-10-01) ----------

def test_marks_map_controls_onto_the_image() -> None:
    from ferb.act.mac.screenshot import to_pixels
    bounds, size = (100.0, 50.0, 400.0, 300.0), (800, 600)                  # a window captured at 2x
    got = to_pixels([("7", (110, 60, 100, 30)), ("9", (450, 300, 100, 100)), ("off", (2000, 0, 10, 10)),
                     ("flat", (150, 100, 0, 20))], bounds, size)
    assert got == [("7", (20.0, 20.0, 200.0, 60.0)), ("9", (700.0, 500.0, 100.0, 100.0))]   # clipped to the window


def _tree_with_controls(ax: Any) -> None:
    from ferb.act.mac.accessibility import AxNode, AxTree
    ax.tree = AxTree(nodes=[AxNode(None, "button", "Send", None, (120.0, 70.0, 60.0, 20.0), False, True),
                            AxNode(None, "button", "", None, (200.0, 100.0, 30.0, 30.0), False, True)],
                     title="Chat")


def test_a_snapshot_with_the_screen_scope_carries_a_marked_image(daemon: RunningDaemon,
                                                                monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    _tree_with_controls(daemon.daemon.mac.ax)                                   # type: ignore[union-attr]
    g = task(daemon, "i", Identity.OWNER, {"mac:read", "screen"}, {"use_app"})
    r = call(daemon, g, "mac.snapshot", app="Notes")
    assert not r["is_error"] and base64.b64decode(r["image"]["data"]) == shots.jpeg
    ids = [e["id"] for e in r["content"]["elements"]]
    assert shots.marks == [(i.split("e", 1)[1], rect) for i, rect in
                           zip(ids, [(120.0, 70.0, 60.0, 20.0), (200.0, 100.0, 30.0, 30.0)])]
    assert r["content"]["shot"] == "shot1" and r["content"]["image"].startswith("attached")
    daemon.run(_checkpoint(daemon))
    for p in daemon.daemon.home.rglob("*"):                                    # in memory only, as before
        if p.is_file() and not p.is_socket():
            assert base64.b64encode(shots.jpeg) not in p.read_bytes(), p


def test_no_image_without_the_screen_scope_for_browsers_or_when_switched_off(daemon: RunningDaemon,
                                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    _tree_with_controls(daemon.daemon.mac.ax)                                   # type: ignore[union-attr]
    plain = task(daemon, "p", Identity.OWNER, {"mac:read"}, {"use_app"})
    assert "image" not in call(daemon, plain, "mac.snapshot", app="Notes")
    g = task(daemon, "q", Identity.OWNER, {"mac:read", "screen"}, {"use_app"})
    assert "image" not in call(daemon, g, "mac.snapshot", app="Brave")        # browsers: the extension's job
    daemon.daemon.mac.snapshot_images = lambda: False                          # type: ignore[union-attr]
    assert "image" not in call(daemon, g, "mac.snapshot", app="Notes")
    assert shots.captured == []


def test_a_password_field_still_means_no_image_but_the_tree_comes_back(daemon: RunningDaemon,
                                                                     monkeypatch: pytest.MonkeyPatch) -> None:
    shots, _ = _fake_mac(daemon, monkeypatch)
    ax = daemon.daemon.mac.ax                                                   # type: ignore[union-attr]
    _tree_with_controls(ax)
    ax.secure = [True]
    g = task(daemon, "s", Identity.OWNER, {"mac:read", "screen"}, {"use_app"})
    r = call(daemon, g, "mac.snapshot", app="Notes")
    assert not r["is_error"] and "image" not in r and "password" in r["content"]["image"]
    assert len(r["content"]["elements"]) == 2 and shots.captured == []
