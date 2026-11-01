"""FERB's Mac tools never operate FERB's own apps (rule 8, rule 9, D31, W2).

The send-to-developer review: `MacComputerDriver.app()` refused only the owner's exclusion list, so a worker could
press the notch's Approve button, click it, or type into it, and answer FERB's own question for the owner. Every Mac
tool now refuses FERB's own apps (`front_window.FERB_APPS`, the daemon's own pid); a pointer click on a point where a
FERB window would take it is refused, and foreground input is posted only while its target is the front app (never
into FERB's app, even right after the owner's Yes in the notch); an AppleScript for one, or a System Events script
naming one, is refused, never asked. Fakes only: no real app, window or key event."""

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
        self.comes_forward = True                       # an activated app becomes the front app (False: it doesn't)

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
        if self.comes_forward:
            self.front = next(a for a in self.apps if a.bundle_id == bid)
        return True


class Ax:
    """Two windows: the notch's question (an Approve button) and VS Code's editor. `hit` is the system's hit test."""

    def __init__(self) -> None:
        self.pressed: list[str] = []
        self.hits: dict[tuple[int, int], int] = {}
        self.posted: list[str] = []
        self.pane = (560.0, 100.0, 400.0, 160.0)                                # a scroll pane centred on (760, 180)
        self.frames: dict[int, tuple[float, float, float, float]] = {}          # the window frame `hit` names, per pid

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
        return None if pid is None else (pid, self.frames.get(pid, (0.0, 0.0, 400.0, 300.0)))

    def raise_window(self, pid: int, bounds: Any) -> bool:
        return True

    def center(self, n: AxNode) -> tuple[float, float]:
        return 5.0, 5.0

    def post_text(self, pid: int, text: str) -> bool:                     # the AX keyboard API, to the app's process
        self.posted.append(f"ax {pid} {text}")
        return True

    def hit_element(self, pid: int, x: float, y: float) -> Any:            # the app's own hit test: nothing here
        return None

    def app_window(self, pid: int) -> Any:
        return "win"

    def scroll_area_in(self, win: Any) -> Any:
        return "pane"

    def frame(self, el: Any) -> Any:
        return self.pane if el == "pane" else self.frames.get(CODE.pid) if el == "win" else None

    def element_window(self, el: Any) -> Any:
        return "win"

    def window_with_frame(self, pid: int, bounds: Any, title: Any = None, current_title: Any = None) -> Any:
        return "win"


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


# decision: HAND-238
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


# decision: HAND-238
def test_a_click_where_a_notch_window_would_take_it_is_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    """A real click (it goes to whatever window is on top there: `mac.click` foreground) addressed to VS Code at a point
    the notch covers would land on the notch: it is refused, by the system's hit test, before and right before the
    click. Input posted to VS Code's own process can't land there (round 4, N1: below)."""
    d, ax, posted = _driver(monkeypatch)
    d._state[CODE.bundle_id] = _AppState(pid=CODE.pid, shot=Shot("s1", 4115, (0.0, 0.0, 400.0, 300.0), (400, 300),
                                                                   pid=CODE.pid))
    d.shots = SimpleNamespace(window=lambda wid: Window(4115, CODE.pid, (0.0, 0.0, 400.0, 300.0), "FERB"),
                              window_at=lambda x, y, max_layer=0: None, on_screen=lambda wid: True)
    ax.hits = {(200, 10): NOTCH.pid, (50, 200): CODE.pid}
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.click_at("Code", "s1", 200, 10, mechanism="foreground", allow_foreground=True))
    assert posted == [] and ax.pressed == []


# decision: HAND-238
def test_foreground_input_right_after_a_yes_in_the_notch_goes_through(monkeypatch: pytest.MonkeyPatch) -> None:
    """Step 12e, fix 1 (live 2026-10-31, clean-up task: every foreground action after the owner's Yes was refused,
    because the notch they had just answered in was in front): the target is brought forward and the input lands
    there."""
    d, ax, posted = _driver(monkeypatch, front=NOTCH)
    asyncio.run(d.snapshot("TextEdit"))
    for action in (ComputerAction("type", "s1e1", "yes", mechanism="foreground", allow_foreground=True),
                   ComputerAction("key", None, "return", mechanism="foreground", allow_foreground=True)):
        d.apps.front = NOTCH                                                      # the owner answered in the notch
        assert asyncio.run(d.act("TextEdit", action, "w1", "t1")).ok
    assert posted == ["type yes", "key return"] and d.apps.activated == [TEXTEDIT.bundle_id, TEXTEDIT.bundle_id]


# decision: HAND-238
def test_foreground_input_never_lands_while_the_notch_stays_in_front(monkeypatch: pytest.MonkeyPatch) -> None:
    """The target didn't come forward (the notch is still in front): nothing is posted (M1)."""
    from ferb.act.mac.input import PointerInterrupted
    d, ax, posted = _driver(monkeypatch, front=NOTCH)
    d.apps.comes_forward = False
    asyncio.run(d.snapshot("TextEdit"))
    for action in (ComputerAction("type", "s1e1", "yes", mechanism="foreground", allow_foreground=True),
                   ComputerAction("key", None, "return", mechanism="foreground", allow_foreground=True)):
        with pytest.raises(PointerInterrupted):
            asyncio.run(d.act("TextEdit", action, "w1", "t1"))
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


def test_a_click_on_the_real_notchs_approve_button_is_refused_by_every_route_that_can_reach_it(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """The notch isn't in `apps.running()` (accessory): its pid is found among every running app. A real click and a
    foreground press of an element whose centre lies under the notch's question are refused (input posted to VS
    Code's process goes there, never to the notch: round 4, N1, below)."""
    d, ax, posted = _driver(monkeypatch)
    assert NOTCH not in d.apps.running()
    _shot_over_notch(d, ax, [])
    ax.hits = {APPROVE: NOTCH.pid}
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.click_at("Code", "s1", *APPROVE, mechanism="foreground", allow_foreground=True))
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
    monkeypatch.setattr(shots, "_info_windows", lambda everywhere=False, strict=False: [info(26, NOTCH.pid, 5, 0.0),
                                                                                        info(25, NOTCH.pid, 77),
                                                                                        info(0, CODE.pid, 4115)])
    assert [(w.id, w.pid, w.layer, w.alpha) for w in shots.windows_at(*APPROVE) or []] == [
        (5, 900, 26, 0.0), (77, 900, 25, 1.0), (4115, CODE.pid, 0, 1.0)]       # with each one's level and alpha
    assert shots.window_at(*APPROVE).id == 4115                                 # the layer-0 lookup can't see it

    def broken(everywhere: bool = False, strict: bool = False) -> list[dict[str, Any]]:
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
    while the notch is key, and FERB never brings anything forward for them."""
    d, ax, posted = _driver(monkeypatch, front=NOTCH)
    asyncio.run(d.snapshot("TextEdit"))
    assert asyncio.run(d.act("TextEdit", ComputerAction("type", "s1e1", "hello", mechanism="pid"), "w1", "t1")).ok
    assert asyncio.run(d.act("TextEdit", ComputerAction("key", None, "return", mechanism="pid"), "w1", "t1")).ok
    assert posted == ["type hello", "key return"] and d.apps.activated == [] and d.apps.front == NOTCH


# ---------- fix round 4: no over-refusal, one fail-open closed (N1, N2, N3, N4, U1; D31, rule 8, rule 9) ----------

CANT = "FERB can't confirm what is under this point"
FERB_APP = RunningApp("com.ferb.FERB", "FERB", 901, False)                       # FERB.app: another of FERB's processes
CODE_WIN = Window(4115, CODE.pid, (0.0, 0.0, 1512.0, 900.0), "FERB")
FAR = (100, 800)                                                                # far from the notch


def _listed(num: int, pid: int, bounds: tuple[float, float, float, float], layer: int = 0,
            alpha: float = 1.0) -> Window:
    """A window as the on-screen window list gives it: its number, owner, bounds, level and alpha."""
    return Window(num, pid, bounds, "", layer=layer, alpha=alpha)                # type: ignore[call-arg]


def _cursor() -> Window:
    return _listed(300, NOTCH.pid, (0.0, 0.0, 1512.0, 900.0), layer=24)        # FERB's pointer overlay, full screen


def _light() -> Window:
    return _listed(301, NOTCH.pid, (536.0, 0.0, 440.0, 56.0), layer=26)        # the notch's glow


def _code_shot(d: MacComputerDriver, ax: Ax, layered: list[Window] | None) -> None:
    """VS Code's window spans the screen and was just captured; `layered`: the window list at any point, front to
    back (None: the window list can't be read)."""
    signature = ax.read_tree(CODE.pid).signature()
    ax.frames[CODE.pid] = CODE_WIN.bounds
    d._state[CODE.bundle_id] = _AppState(pid=CODE.pid, shot=Shot("s1", 4115, CODE_WIN.bounds, (1512, 900),
                                                                   pid=CODE.pid, signature=signature))
    d.shots = SimpleNamespace(window=lambda wid: CODE_WIN, window_at=lambda x, y, max_layer=0: CODE_WIN,
                              on_screen=lambda wid: True, front_window=lambda pid, like=None, title=None: CODE_WIN,
                              windows_at=lambda x, y: None if layered is None else list(layered))


def _pid_routes(monkeypatch: pytest.MonkeyPatch) -> list[tuple[str, int | None]]:
    """Pointer and wheel events as (what, the pid they are posted to; None: posted globally)."""
    sent: list[tuple[str, int | None]] = []

    def pointer(start: Any, finish: Any, kind: str, pid: int, **kw: Any) -> tuple[list[Step], Any]:
        return [Step(kind, lambda: sent.append((f"pointer {kind}", pid)))], lambda: None

    def wheel(x: float, y: float, lines: int, pid: int | None = None, **kw: Any) -> list[Step]:
        return [Step("scroll", lambda: sent.append(("scroll", pid)))]
    monkeypatch.setattr(drv, "pointer_steps", pointer)
    monkeypatch.setattr(drv, "wheel_steps", wheel)
    monkeypatch.setattr(drv, "load_route", lambda: None)
    return sent


def test_a_null_window_list_is_cant_tell_never_empty(monkeypatch: pytest.MonkeyPatch) -> None:
    """N2: Quartz answering NULL for the window list is "can't tell" (None), never an empty list that would let the
    click through when the hit test failed too."""
    import Quartz

    import ferb.act.mac.screenshot as shots
    monkeypatch.setattr(Quartz, "CGWindowListCopyWindowInfo", lambda options, window: None)
    assert shots.windows_at(*FAR) is None
    d, ax, posted = _driver(monkeypatch)
    ax.hits = {}
    _code_shot(d, ax, None)
    d.shots.windows_at = shots.windows_at                                       # the real reader over the NULL list
    with pytest.raises(Refused, match=CANT):
        asyncio.run(d.click_at("Code", "s1", *FAR, mechanism="foreground", allow_foreground=True))
    assert posted == []


def test_ferbs_reported_click_through_windows_are_skipped_while_their_reporter_runs(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """H1, N1: the notch reports its click-through windows (the pointer overlay, the glow) by number; the point check
    skips exactly those, only the reporter's own windows, and only while the reporter runs. The question's panel is
    never skipped, and a process that isn't FERB's can't report."""
    d, ax, posted = _driver(monkeypatch)
    ax.hits = {}                                                                # the system hit test can't answer
    _code_shot(d, ax, [_cursor(), CODE_WIN])
    assert d._own_point(*FAR, CODE.pid, 4115) == drv.CANT_CONFIRM             # unreported: it could take the click
    assert d.report_click_through(NOTCH.pid, [300, 301])
    assert d._own_point(*FAR, CODE.pid, 4115) is None
    asyncio.run(d.click_at("Code", "s1", *FAR, mechanism="foreground", allow_foreground=True))
    assert posted == ["click"]
    _code_shot(d, ax, [_light(), _cursor(), NOTCH_PANEL, CODE_WIN])
    assert d._own_point(*APPROVE, CODE.pid, 4115) == drv.CANT_CONFIRM         # the question's panel takes clicks
    d.apps.apps.append(FERB_APP)
    assert d.report_click_through(FERB_APP.pid, [77])                         # FERB.app naming the notch's panel
    assert d._own_point(*APPROVE, CODE.pid, 4115) == drv.CANT_CONFIRM         # not its window: nothing skipped
    assert not d.report_click_through(CODE.pid, [77, 300])                    # VS Code isn't FERB
    assert not d.report_click_through(4242, [300])                            # nor is a process that isn't running
    _code_shot(d, ax, [_cursor(), CODE_WIN])
    d.apps.apps.remove(NOTCH)                                                   # the notch quit
    assert d._own_point(*FAR, CODE.pid, 4115) is None                          # (its windows went with it)
    d.apps.apps.append(NOTCH)                                                   # a notch at that pid again: unreported
    assert d._own_point(*FAR, CODE.pid, 4115) == drv.CANT_CONFIRM


def test_the_hit_test_naming_a_reported_overlay_lets_the_window_list_decide(monkeypatch: pytest.MonkeyPatch) -> None:
    """U1: if the system hit test lands on FERB's pointer overlay, FERB doesn't refuse its own ordinary click there:
    when every FERB window at the point is one the notch reported click-through, the window list decides. Where the
    question's panel is at the point too, the hit naming FERB still refuses."""
    d, ax, posted = _driver(monkeypatch)
    assert d.report_click_through(NOTCH.pid, [300, 301])
    ax.hits = {FAR: NOTCH.pid, APPROVE: NOTCH.pid}
    _code_shot(d, ax, [_cursor(), CODE_WIN])
    assert d._own_point(*FAR, CODE.pid, 4115) is None
    _code_shot(d, ax, [_cursor(), NOTCH_PANEL, CODE_WIN])
    assert d._own_point(*APPROVE, CODE.pid, 4115) == OWN
    _code_shot(d, ax, None)
    assert d._own_point(*FAR, CODE.pid, 4115) == OWN                          # no list to tell the overlay apart


def test_input_posted_to_another_apps_process_skips_the_point_check(monkeypatch: pytest.MonkeyPatch) -> None:
    """N1 (round 4, ruling 3): input posted to VS Code's process (CGEventPostToPid: `mac.pointer`, `mac.click` with
    pointer or ax) can't reach the notch, so no point check refuses it, even at the Approve point with the hit test
    failing and no window list: it goes to VS Code's pid, never the notch's. Input for a FERB process is refused."""
    d, ax, posted = _driver(monkeypatch)
    sent = _pid_routes(monkeypatch)
    ax.hits = {}
    for kind in ("click", "dblclick", "rightclick", "hover", "scroll"):
        _code_shot(d, ax, None)
        asyncio.run(d.pointer_at("Code", "s1", *APPROVE, kind))
    _code_shot(d, ax, None)
    with pytest.raises(Refused, match="couldn't read"):                        # a drop where FERB can't read what is
        asyncio.run(d.pointer_at("Code", "s1", *FAR, "drag", APPROVE))         # there, in an editor: it could be its
    _code_shot(d, ax, None)                                                    # terminal (fails closed)
    asyncio.run(d.click_at("Code", "s1", *APPROVE, mechanism="pointer"))
    _code_shot(d, ax, None)
    r = asyncio.run(d.click_at("Code", "s1", *APPROVE, mechanism="ax"))       # the app's own hit test: no element
    assert not r.ok and "AX hit test unavailable" in r.detail
    assert [w for w, _ in sent] == ["pointer click", "pointer dblclick", "pointer rightclick", "pointer hover",
                                    "scroll", "pointer click"]
    assert {pid for _, pid in sent} == {CODE.pid}
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.pointer_at("FERB", "s1", *FAR, "click"))
    assert posted == []


def test_the_window_walk_lets_the_first_window_that_takes_clicks_decide(monkeypatch: pytest.MonkeyPatch) -> None:
    """N3: front to back, reported click-through windows and windows with alpha 0 are skipped, and the first window
    that takes clicks decides: a FERB window refuses, the target's own window allows (the screenshot's window, or
    without one the target's normal window). A target window that takes no clicks (Brave's alpha-0 strip, measured on
    this Mac) never stops the walk; a window whose click handling FERB can't know (another app's menu, another of the
    target's windows) decides nothing, so a FERB window under it still refuses."""
    d, ax, posted = _driver(monkeypatch)
    assert d.report_click_through(NOTCH.pid, [300, 301])
    ax.hits = {}
    strip = _listed(9001, CODE.pid, CODE_WIN.bounds, layer=26, alpha=0.0)
    floating = _listed(9002, CODE.pid, CODE_WIN.bounds, layer=26)
    menu = _listed(9003, TEXTEDIT.pid, CODE_WIN.bounds, layer=101)
    for layered, window, expected in (
            ([strip, NOTCH_PANEL, CODE_WIN], 4115, drv.CANT_CONFIRM),
            ([strip, NOTCH_PANEL, CODE_WIN], None, drv.CANT_CONFIRM),
            ([floating, NOTCH_PANEL, CODE_WIN], 4115, drv.CANT_CONFIRM),
            ([floating, NOTCH_PANEL, CODE_WIN], None, drv.CANT_CONFIRM),
            ([menu, NOTCH_PANEL, CODE_WIN], 4115, drv.CANT_CONFIRM),
            ([_light(), _cursor(), strip, CODE_WIN, NOTCH_PANEL], 4115, None),
            ([_cursor(), menu, CODE_WIN], 4115, None),
            ([CODE_WIN, NOTCH_PANEL], None, None)):                             # the notch yielded behind VS Code
        _code_shot(d, ax, layered)
        assert d._own_point(*APPROVE, CODE.pid, window) == expected, (layered, window)


def test_a_foreground_scroll_runs_the_point_check(monkeypatch: pytest.MonkeyPatch) -> None:
    """N4: a foreground `mac.act` scroll posts a wheel at the pane's centre, globally: under the notch's question it is
    refused, before anything is posted; elsewhere it goes out."""
    d, ax, posted = _driver(monkeypatch)
    sent = _pid_routes(monkeypatch)
    _code_shot(d, ax, [NOTCH_PANEL, CODE_WIN])
    ax.hits = {APPROVE: NOTCH.pid, (300, 700): CODE.pid}
    scroll = ComputerAction("scroll", None, mechanism="foreground", allow_foreground=True, direction="down")
    with pytest.raises(Refused, match=OWN):
        asyncio.run(d.act("Code", scroll, "w1", "t1"))
    assert sent == []
    ax.pane = (200.0, 600.0, 200.0, 200.0)                                     # centred on (300, 700)
    asyncio.run(d.act("Code", scroll, "w1", "t1"))
    assert sent == [("scroll", None)]


def test_each_checked_click_records_which_check_decided(monkeypatch: pytest.MonkeyPatch) -> None:
    """Measurability (round 4): every point check notes which check decided: the system hit test, the window list, or
    neither ("refused: cant_tell"), for the call's tool span. Input posted to a process isn't checked, so notes
    nothing."""
    from ferb.core import call_facts
    d, ax, posted = _driver(monkeypatch)
    sent = _pid_routes(monkeypatch)
    with call_facts.recording() as facts:
        ax.hits = {FAR: CODE.pid}
        _code_shot(d, ax, [CODE_WIN])
        asyncio.run(d.click_at("Code", "s1", *FAR, mechanism="foreground", allow_foreground=True))
    assert facts["point_checks"] and set(facts["point_checks"]) == {"hit_test"}
    with call_facts.recording() as facts:
        ax.hits = {}
        asyncio.run(d.click_at("Code", "s1", *FAR, mechanism="foreground", allow_foreground=True))
        _code_shot(d, ax, None)
        with pytest.raises(Refused, match=CANT):
            asyncio.run(d.click_at("Code", "s1", *FAR, mechanism="foreground", allow_foreground=True))
    assert set(facts["point_checks"][:-1]) == {"window_list"} and facts["point_checks"][-1] == "refused: cant_tell"
    with call_facts.recording() as facts:
        _code_shot(d, ax, None)
        asyncio.run(d.pointer_at("Code", "s1", *FAR, "click"))
    assert facts == {} and sent == [("pointer click", CODE.pid)]


def test_the_tool_span_carries_the_point_checks_its_code_recorded(daemon: Any) -> None:
    """The gateway keeps what a call's code noted (`call_facts`) on its tool span, a refusal's too, so `ferb trace`
    shows each click's deciding check and `ferb stats` counts them."""
    import io
    import json

    from ferb.cli import admin
    from ferb.core import call_facts
    from ferb.core.types import ToolResult
    from ferb.trace import compose, render
    from tests.conftest import GatewayCap, gateway_request, gateway_setup, gateway_tool_call

    class Clicks(GatewayCap):
        async def run(self, args: Any, ctx: Any) -> ToolResult:
            call_facts.note("point_checks", "hit_test")
            await asyncio.to_thread(call_facts.note, "point_checks", "refused: cant_tell")   # from a worker thread
            return ToolResult({"error": CANT}, is_error=True)
    grant, _ = gateway_setup(daemon, {"mac:act"}, gateway_request("click it"), [Clicks("fake.click", "click",
                                                                                         {"mac:act"})])
    gateway_tool_call(daemon, grant, "fake.click")

    async def spans() -> list[Any]:
        return daemon.daemon.db.conn.execute(
            "SELECT input_json FROM spans WHERE kind = 'tool.call' AND json_extract(input_json, '$.tool') = "
            "'fake.click'").fetchall()
    rows = daemon.run(spans())
    assert [json.loads(r["input_json"])["point_checks"] for r in rows] == [["hit_test", "refused: cant_tell"]]
    out = io.StringIO()
    admin.stats(daemon.daemon.home, out, days=36500)                            # (the daemon's fake clock)
    assert "Click point checks: hit test 1, window list 0, refused (can't tell) 1" in out.getvalue()
    trace = {"task_id": "t1", "outcome": {}, "spans": [
        {"id": "s1", "kind": "tool.call", "started_at": "2026-10-25T10:00:00+00:00",
         "ended_at": "2026-10-25T10:00:01+00:00", "input": {"tool": "mac.click", "point_checks": ["hit_test"] * 3},
         "output": None, "error": None, "origin": None, "children": []},
        {"id": "s2", "kind": "tool.call", "started_at": "2026-10-25T10:00:02+00:00",
         "ended_at": "2026-10-25T10:00:03+00:00",
         "input": {"tool": "mac.act", "point_checks": ["window_list", "refused: cant_tell"]},
         "output": None, "error": CANT, "origin": None, "children": []}]}
    story = render.story(compose.compose(trace))
    assert "Click point checks: hit test 3, window list 1, refused (can't tell) 1" in story


def test_only_a_running_ferb_app_reports_its_click_through_windows(daemon: Any,
                                                                   monkeypatch: pytest.MonkeyPatch) -> None:
    """`ui.click_through` (cli role): the daemon records the windows a running FERB app reports, the caller known by
    its socket's pid; any other caller is refused and nothing is recorded; the list must be window numbers."""
    import os

    from ferb.cli.client import Client, RpcError
    driver = daemon.daemon.mac
    ferb_pids: set[int] = set()
    monkeypatch.setattr(driver, "apps", SimpleNamespace(pids_of=lambda ids: set(ferb_pids), running=lambda: []))
    client = Client(daemon.sock)
    try:
        with pytest.raises(RpcError, match="FERB's own app"):
            client.call("ui.click_through", {"windows": [300]})
        assert driver.click_through() == {}
        ferb_pids.add(os.getpid())                                              # this test's process plays the notch
        assert client.call("ui.click_through", {"windows": [300, 301]}) == {"windows": 2}
        assert driver.click_through() == {os.getpid(): frozenset({300, 301})}
        assert client.call("ui.click_through", {"windows": []}) == {"windows": 0}   # its panels closed
        assert driver.click_through() == {os.getpid(): frozenset()}
        for bad in ({"windows": "300"}, {"windows": [True]}, {"windows": [-1]}, {"windows": [1.5]}, {}):
            with pytest.raises(RpcError, match="window numbers"):
                client.call("ui.click_through", bad)
    finally:
        client.close()
