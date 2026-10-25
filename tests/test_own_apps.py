"""FERB's Mac tools never operate FERB's own apps (rule 8, rule 9, D31, W2).

The send-to-developer review: `MacComputerDriver.app()` refused only the owner's exclusion list, so a worker could
press the notch's Approve button, click it, or type into it, and answer FERB's own question for the owner. Every Mac
tool now refuses FERB's own apps (`front_window.FERB_APPS`, the daemon's own pid); a pointer click on a point where a
FERB window would take it, and keyboard input while a FERB app is in front, are refused; an AppleScript for one, or a
System Events script naming one, is refused, never asked. Fakes only: no real app, window or key event."""

import asyncio
from types import SimpleNamespace
from typing import Any

import pytest

import ferb.act.mac.driver as drv
from ferb.act.gui_lock import GuiLock
from ferb.act.mac.accessibility import AxNode, AxTree
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.driver import MacComputerDriver, Shot, _AppState
from ferb.act.mac.input import RealInput, Step
from ferb.act.mac.screenshot import Window
from ferb.core.errors import Refused
from ferb.core.types import ComputerAction

OWN = "FERB never operates its own app; questions are answered by the owner"
NOTCH, CODE = RunningApp("dev.ferb.notch", "FERB", 900, False), RunningApp("com.microsoft.VSCode", "Code", 41154, True)
TEXTEDIT = RunningApp("com.apple.TextEdit", "TextEdit", 5120, False)


class Apps:
    """Like the real apps module: `running()` lists Regular apps only, so the notch (an accessory, LSUIElement app)
    isn't in it; `pids_of` reads every running app whatever its activation policy (round 3, H1). `find` searches
    every app, so `app()`'s own refusal is exercised (the real `find` never resolves the notch at all)."""

    def __init__(self, front: RunningApp) -> None:
        self.apps, self.front, self.activated = [NOTCH, CODE, TEXTEDIT], front, []
        self.accessory = {NOTCH.pid}

    def find(self, q: str) -> RunningApp | None:
        hits = [a for a in self.apps if q.lower() in (a.bundle_id.lower(), a.name.lower(), f"pid:{a.pid}")]
        return hits[0] if len(hits) == 1 else None

    def candidates(self, q: str) -> list[RunningApp]:
        return [a for a in self.apps if q.lower() in (a.bundle_id.lower(), a.name.lower(), f"pid:{a.pid}")]

    def running(self) -> list[RunningApp]:
        return [a for a in self.apps if a.pid not in self.accessory]

    def pids_of(self, bundle_ids: Any) -> set[int]:
        return {a.pid for a in self.apps if a.bundle_id.casefold() in bundle_ids}

    def frontmost(self) -> RunningApp | None:
        return self.front

    def activate(self, bid: str, pid: int | None = None) -> bool:
        self.activated.append(bid)
        return True


class Ax:
    """Two windows: the notch's question (an Approve button) and VS Code's editor. `hit` is the system's hit test."""

    def __init__(self) -> None:
        self.pressed: list[str] = []
        self.hits: dict[tuple[int, int], int] = {}
        self.posted: list[str] = []

    def read_tree(self, pid: int, browser: bool = False, window: Any = None, offset: int = 0) -> AxTree:
        if pid == NOTCH.pid:
            node = AxNode("approve", "button", "Approve", None, (0, 0, 10, 10), False, True, "AXButton", ("press",))
        else:
            node = AxNode("editor", "textbox", "Editor", "", (0, 0, 10, 10), False, True, "AXTextArea")
        return AxTree([node], "", "w")

    def expose_tree(self, pid: int) -> bool:
        return True

    def press(self, el: Any) -> bool:
        self.pressed.append(el)
        return True

    def focus(self, el: Any) -> bool:
        return True

    def has_focus(self, pid: int, el: Any) -> bool:
        return True

    def value(self, el: Any) -> str:
        return ""

    def hit(self, x: float, y: float) -> tuple[int, tuple[float, float, float, float]] | None:
        pid = self.hits.get((int(x), int(y)))
        return None if pid is None else (pid, (0.0, 0.0, 400.0, 300.0))

    def center(self, n: AxNode) -> tuple[float, float]:
        return 5.0, 5.0

    def post_text(self, pid: int, text: str) -> bool:                     # the AX keyboard API, to the app's process
        self.posted.append(f"ax {pid} {text}")
        return True


def _driver(monkeypatch: pytest.MonkeyPatch, front: RunningApp = CODE) -> tuple[MacComputerDriver, Ax, list[str]]:
    async def no_sleep(s: float) -> None:
        return None
    posted: list[str] = []
    monkeypatch.setattr(drv, "type_steps", lambda t, pid=None: [Step("type", lambda: posted.append(f"type {t}"))])
    monkeypatch.setattr(drv, "key_steps", lambda c, pid=None: [Step(c, lambda: posted.append(f"key {c}"))])
    monkeypatch.setattr(drv, "click_steps", lambda x, y, pid=None: [Step("click", lambda: posted.append("click"))])
    monkeypatch.setattr(drv, "SETTLE_S", 0)
    ax = Ax()
    d = MacComputerDriver("dev", RealInput(lambda: False, GuiLock(), sleep=no_sleep), ax=ax, apps=Apps(front))
    return d, ax, posted


def test_every_mac_tool_refuses_the_notch_by_its_app(monkeypatch: pytest.MonkeyPatch) -> None:
    """`app()` refuses FERB's own apps, so a snapshot, a press on the notch's Approve button, a menu, a script or a
    secret never reaches them; the daemon's own process too."""
    import os
    d, ax, posted = _driver(monkeypatch)
    for name in ("dev.ferb.notch", "FERB", "pid:900", "DEV.FERB.NOTCH"):
        with pytest.raises(Refused, match=OWN):
            d.app(name)
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.snapshot("FERB"))
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.act("FERB", ComputerAction("press", "s1e1", mechanism="ax"), "w1", "t1"))
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.press_menu("FERB", ["Edit", "Paste"]))
    d.apps.apps.append(RunningApp("org.python.python", "Python", os.getpid(), False))
    with pytest.raises(Refused, match=OWN):
        d.app(f"pid:{os.getpid()}")
    assert ax.pressed == [] and posted == []
    assert d.app("Code").bundle_id == "com.microsoft.VSCode"                       # any other app is unaffected


def test_a_click_where_a_notch_window_would_take_it_is_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    """A click addressed to VS Code at a point the notch covers would land on the notch (foreground) or could: it is
    refused, by the system's hit test, before and right before the click."""
    d, ax, posted = _driver(monkeypatch)
    d._state[CODE.bundle_id] = _AppState(pid=CODE.pid, shot=Shot("s1", 4115, (0.0, 0.0, 400.0, 300.0), (400, 300),
                                                                   pid=CODE.pid))
    d.shots = SimpleNamespace(window=lambda wid: Window(4115, CODE.pid, (0.0, 0.0, 400.0, 300.0), "FERB"),
                              window_at=lambda x, y, max_layer=0: None, on_screen=lambda wid: True)
    ax.hits = {(200, 10): NOTCH.pid, (50, 200): CODE.pid}
    for mechanism in ("pointer", "ax", "foreground"):
        with pytest.raises(Refused, match=OWN):
            asyncio.run(d.click_at("Code", "s1", 200, 10, mechanism=mechanism, allow_foreground=True))
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.pointer_at("Code", "s1", 50, 200, "drag", (200, 10)))       # a drop onto the notch
    assert posted == [] and ax.pressed == []


def test_foreground_typing_or_a_key_while_the_notch_is_in_front_is_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, posted = _driver(monkeypatch, front=NOTCH)
    asyncio.run(d.snapshot("Code"))
    for action in (ComputerAction("type", "s1e1", "yes", mechanism="foreground", allow_foreground=True),
                   ComputerAction("key", None, "return", mechanism="foreground", allow_foreground=True)):
        with pytest.raises(Refused, match=OWN):
            asyncio.run(d.act("Code", action, "w1", "t1"))
    assert posted == []


def test_a_script_for_a_ferb_app_or_naming_one_to_system_events_is_refused_not_asked(monkeypatch: pytest.MonkeyPatch
                                                                                    ) -> None:
    from ferb.act.mac.tools import MacAppleScript, MacOpen
    d, ax, posted = _driver(monkeypatch)
    d.apps.apps.append(RunningApp("com.apple.systemevents", "System Events", 77, False))
    tool = MacAppleScript(d, sandbox=object())
    assert OWN in (tool.precheck(tool.Input(app="System Events", script='tell process "FERB" to click button 1'))
                   or "")
    assert OWN in (tool.precheck(tool.Input(app="System Events", script='tell application id "dev.ferb.notch"'))
                   or "")
    assert tool.precheck(tool.Input(app="System Events", script='tell process "Code" to get name')) is None
    with pytest.raises(Refused, match=OWN):
        d.app("dev.ferb.notch")                                                   # what mac.applescript resolves
    opener = MacOpen(d)
    out = asyncio.run(opener.run(opener.Input(bundle_id="dev.ferb.notch"), SimpleNamespace(task_id="t1")))
    assert out.is_error and OWN in out.content["error"]


# ---------- fix round 3, H1: the real notch is an accessory app, at window level 25 ----------

NOTCH_PANEL = Window(77, NOTCH.pid, (560.0, 0.0, 400.0, 220.0), "")             # the expanded question, level 25
APPROVE = (760, 180)                                                            # its Approve button (screen points)


def _shot_over_notch(d: MacComputerDriver, ax: Ax, layered: list[tuple[Window, int]] | None) -> None:
    """VS Code's window spans the screen's top; the notch's question covers its top centre. `layered`: what the window
    list shows at a point, front to back, with each window's level (None: the window list can't be read)."""
    d._state[CODE.bundle_id] = _AppState(pid=CODE.pid, shot=Shot("s1", 4115, (0.0, 0.0, 1512.0, 900.0), (1512, 900),
                                                                   pid=CODE.pid))
    code_win = Window(4115, CODE.pid, (0.0, 0.0, 1512.0, 900.0), "FERB")
    d.shots = SimpleNamespace(window=lambda wid: code_win, window_at=lambda x, y, max_layer=0: code_win,
                              on_screen=lambda wid: True,
                              windows_at=lambda x, y: None if layered is None else [w for w, _ in layered])


def test_a_click_on_the_real_notchs_approve_button_is_refused_by_every_route(monkeypatch: pytest.MonkeyPatch) -> None:
    """The notch isn't in `apps.running()` (accessory): its pid is found among every running app. A pointer click and a
    foreground press of an element whose centre lies under the notch's question are refused."""
    d, ax, posted = _driver(monkeypatch)
    assert NOTCH not in d.apps.running()
    _shot_over_notch(d, ax, [])
    ax.hits = {APPROVE: NOTCH.pid}
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.click_at("Code", "s1", *APPROVE, mechanism="pointer"))
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.pointer_at("Code", "s1", *APPROVE, "click"))
    ax.center = lambda n: APPROVE                                               # type: ignore[method-assign]
    asyncio.run(d.snapshot("Code"))
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.act("Code", ComputerAction("press", "s1e1", mechanism="foreground", allow_foreground=True),
                          "w1", "t1"))
    assert posted == []


def test_when_the_hit_test_cant_tell_a_ferb_window_at_the_point_fails_closed(monkeypatch: pytest.MonkeyPatch) -> None:
    """With the system hit test failing, every window level is read: a FERB window at the point (the notch's panel at
    level 25, or one FERB can't tell is click-through) refuses the click; no window list refuses it too."""
    d, ax, posted = _driver(monkeypatch)
    ax.hits = {}                                                                # the hit test answers nothing
    _shot_over_notch(d, ax, [(NOTCH_PANEL, 25), (Window(4115, CODE.pid, (0.0, 0.0, 1512.0, 900.0), "FERB"), 0)])
    with pytest.raises(Refused, match="FERB can't confirm what is under this point"):
        asyncio.run(d.click_at("Code", "s1", *APPROVE, mechanism="foreground", allow_foreground=True))
    _shot_over_notch(d, ax, None)
    with pytest.raises(Refused, match="FERB can't confirm what is under this point"):
        asyncio.run(d.click_at("Code", "s1", *APPROVE, mechanism="foreground", allow_foreground=True))
    code_win = Window(4115, CODE.pid, (0.0, 0.0, 1512.0, 900.0), "FERB")
    _shot_over_notch(d, ax, [(code_win, 0)])
    assert d._own_point(50, 600, CODE.pid) is None                              # VS Code is on top there: allowed
    _shot_over_notch(d, ax, [(code_win, 0), (NOTCH_PANEL, 0)])
    assert d._own_point(*APPROVE, CODE.pid) is None                             # the notch yielded behind VS Code
    assert posted == []


def test_the_window_list_reads_every_level_and_skips_nothing(monkeypatch: pytest.MonkeyPatch) -> None:
    """`windows_at` reads the on-screen window list on every level, front to back: the notch's level-25 panel and a
    transparent FERB window are both there (no window is assumed click-through); an unreadable list is None."""
    import ferb.act.mac.screenshot as shots

    def info(level: int, pid: int, num: int, alpha: float = 1.0) -> dict[str, Any]:
        return {"kCGWindowNumber": num, "kCGWindowOwnerPID": pid, "kCGWindowLayer": level, "kCGWindowAlpha": alpha,
                "kCGWindowBounds": {"X": 0, "Y": 0, "Width": 1512, "Height": 900}}
    monkeypatch.setattr(shots, "_info_windows", lambda everywhere=False: [info(26, NOTCH.pid, 5, 0.0),
                                                                          info(25, NOTCH.pid, 77),
                                                                          info(0, CODE.pid, 4115)])
    assert [(w.id, w.pid) for w in shots.windows_at(*APPROVE) or []] == [(5, 900), (77, 900), (4115, CODE.pid)]
    assert shots.window_at(*APPROVE).id == 4115                                 # the layer-0 lookup can't see it

    def broken(everywhere: bool = False) -> list[dict[str, Any]]:
        raise RuntimeError("window server")
    monkeypatch.setattr(shots, "_info_windows", broken)
    assert shots.windows_at(*APPROVE) is None


def test_ferbs_pids_come_from_every_running_app_whatever_its_activation_policy(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """`apps.pids_of` reads NSWorkspace's whole list (accessory and UI-element apps included), not the Regular one."""
    import AppKit

    import ferb.act.mac.apps as apps
    from ferb.sense.front_window import FERB_APPS

    def app(bundle: str | None, pid: int, policy: int) -> Any:
        return SimpleNamespace(bundleIdentifier=lambda: bundle, processIdentifier=lambda: pid,
                               activationPolicy=lambda: policy)
    listed = [app("dev.ferb.notch", 900, 1), app("com.ferb.FERB", 901, 2), app("com.microsoft.VSCode", 41154, 0),
              app(None, 5, 2)]
    workspace = SimpleNamespace(runningApplications=lambda: listed)
    monkeypatch.setattr(AppKit, "NSWorkspace", SimpleNamespace(sharedWorkspace=lambda: workspace))
    assert apps.pids_of(FERB_APPS) == {900, 901}


def test_background_input_goes_on_while_the_notch_holds_the_keyboard(monkeypatch: pytest.MonkeyPatch) -> None:
    """M1: process-directed typing and keys go to the target app's process and can't reach the notch, so they work
    while the notch is key; foreground keyboard and pointer input, which go to whatever is in front, are refused."""
    d, ax, posted = _driver(monkeypatch, front=NOTCH)
    asyncio.run(d.snapshot("TextEdit"))
    assert asyncio.run(d.act("TextEdit", ComputerAction("type", "s1e1", "hello", mechanism="pid"), "w1", "t1")).ok
    assert asyncio.run(d.act("TextEdit", ComputerAction("key", None, "return", mechanism="pid"), "w1", "t1")).ok
    assert asyncio.run(d.act("TextEdit", ComputerAction("type", "s1e1", "there", mechanism="ax_keyboard"), "w1",
                             "t1")).ok
    assert posted == ["type hello", "key return"] and ax.posted == [f"ax {TEXTEDIT.pid} there"]
    posted.clear()
    asyncio.run(d.snapshot("Code"))
    for action in (ComputerAction("type", "s1e1", "yes", mechanism="foreground", allow_foreground=True),
                   ComputerAction("key", None, "return", mechanism="foreground", allow_foreground=True),
                   ComputerAction("press", "s1e1", mechanism="foreground", allow_foreground=True)):
        with pytest.raises(Refused, match=OWN):
            asyncio.run(d.act("Code", action, "w1", "t1"))
    assert posted == []
