"""Shared synthetic Mac controls/captures and task helpers. No native input is sent by these fakes."""

import asyncio
import dataclasses
import os
from dataclasses import dataclass, field
from datetime import timedelta
from typing import Any

import pytest
import ferb.act.mac.driver as drv
from ferb.act.gui_lock import GuiLock
from ferb.act.mac.accessibility import AxNode, AxTree
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.input import RealInput, Step
from ferb.act.mac.profile import Profiles
from ferb.act.mac.screenshot import Window
from ferb.config import DEFAULT_EXCLUDE_APPS
from ferb.core.types import Identity, Task
from ferb.orchestrator.tree import TaskRecord
from ferb.policy.targets import Request
from tests.conftest import RunningDaemon

def grant_task(d: RunningDaemon, tid: str, identity: Identity, scopes: set[str], words: Any = None) -> Any:
    """A running task with a run token; `words` (a string) are the owner's own words for its request (what counts
    as named at the gateway); anything else records a request with no words."""
    t = Task(tid, None, "g", "dev", identity, scopes, "worker", f"req-{tid}", [],
             d.daemon.clock.now() + timedelta(minutes=10))

    async def go() -> Any:
        root = d.daemon.spans.start("task", task_id=t.id, input={})
        d.daemon.tasks.add(TaskRecord(t, d.daemon.clock.now(), root.id))
        d.daemon.state.requests.record(Request(f"req-{tid}", words if isinstance(words, str) else ""))
        return d.daemon.tokens.validate(d.daemon.tokens.issue(t.id, f"w-{tid}", scopes, identity, ttl_s=600))
    return d.run(go())


def gateway_call(d: RunningDaemon, grant: Any, name: str, timeout: float = 30, **args: Any) -> dict[str, Any]:
    """One gateway call."""
    out = d.run(d.daemon.gateway.call(grant, name, args), timeout=timeout)
    return out                                                               # type: ignore[no-any-return]


@dataclass
class FakeAx:
    tree: AxTree
    press_ok: bool = True
    set_ok: bool = True

    def __post_init__(self) -> None:
        self.calls: list[str] = []

    def read_tree(self, pid: int, browser: bool = False, window: Any = None) -> AxTree:
        return self.tree

    def expose_tree(self, pid: int) -> bool:
        self.calls.append("ax.expose")
        return False

    def app_window(self, pid: int) -> Any:
        return "win"

    def has_focus(self, pid: int, el: Any) -> bool | None:
        return None

    def value(self, el: Any) -> str | None:
        return None

    def press_ancestor(self, el: Any) -> str | None:
        return None

    def open_panels(self, pid: int, win: Any) -> list[Any]:
        return []

    def open_menus(self, pid: int) -> list[Any]:
        return []

    def menu_item_for_shortcut(self, pid: int, key: str, mods: list[str]) -> Any:
        return None

    def post_text(self, pid: int, text: str) -> bool:
        return True                                              # this fake app ignores the AX keyboard

    def post_key(self, pid: int, keycode: int, mods: list[str]) -> bool:
        return True

    def window_title(self, win: Any) -> str:
        return "Window"

    def web_areas_of(self, win: Any) -> list[Any]:
        return list(self.tree.web_areas)

    def press(self, el: Any) -> bool:
        self.calls.append("ax.press")
        return self.press_ok

    def set_value(self, el: Any, text: str) -> bool:
        self.calls.append("ax.set_value")
        return self.set_ok

    def focus(self, el: Any) -> bool:
        self.calls.append("ax.focus")
        return True

    def center(self, n: AxNode) -> tuple[float, float]:
        return 10.0, 10.0

    def frame(self, el: Any) -> Any:                             # a window's frame; elements report none
        return BOUNDS if el == "win" else None

    def element_window(self, el: Any) -> Any:
        return "win"

    def raise_window(self, pid: int, bounds: Any) -> bool:
        self.calls.append("ax.raise")
        return True


class FakeApps:
    def __init__(self) -> None:
        self.apps = [RunningApp("com.apple.Notes", "Notes", 11, False), RunningApp("com.brave.Browser", "Brave", 12, True),
                     RunningApp("com.secret.Vault", "Vault", 13, False)]
        self.activated: list[str] = []

    def find(self, q: str) -> RunningApp | None:
        return next((a for a in self.apps if q.lower() in (a.bundle_id.lower(), a.name.lower(), f"pid:{a.pid}")), None)

    def running(self) -> list[RunningApp]:
        return self.apps

    def pids_of(self, bundle_ids: Any) -> set[int]:
        return {a.pid for a in self.apps if a.bundle_id.casefold() in bundle_ids}

    def frontmost(self) -> RunningApp | None:
        return next(a for a in self.apps if a.active)

    def activate(self, bid: str, pid: int | None = None) -> bool:
        self.activated.append(bid)
        self.apps = [RunningApp(a.bundle_id, a.name, a.pid, a.bundle_id == bid) for a in self.apps]
        return True


def mac(mp: pytest.MonkeyPatch, press_ok: bool = True) -> tuple[MacComputerDriver, FakeAx, FakeApps, list[str]]:
    nodes = [AxNode("btn", "button", "Save", None, (0, 0, 20, 20), False, True),
             AxNode("pw", "textbox", "Password", "filled", (0, 30, 20, 20), True, True),
             AxNode("title", "textbox", "Title", "draft", (0, 60, 20, 20), False, True)]
    ax, apps, posted = FakeAx(AxTree(nodes, "note text", "My note"), press_ok=press_ok), FakeApps(), []

    async def no_sleep(s: float) -> None:
        return None
    ri = RealInput(lambda: False, GuiLock(), sleep=no_sleep)
    d = MacComputerDriver("dev", ri, excluded=lambda: {"com.secret.Vault"}, ax=ax, apps=apps, shots=FakeShots())
    import ferb.act.mac.driver as drv
    mp.setattr(drv, "click_steps", lambda x, y, pid=None: [Step("click", lambda: posted.append(f"click {x},{y}"))])
    mp.setattr(drv, "type_steps", lambda t, pid=None: [Step("type", lambda: posted.append(f"type {t}"))])
    mp.setattr(drv, "key_steps", lambda c, pid=None: [Step(c, lambda: posted.append(f"key {c} to pid"))])
    mp.setattr(drv, "SETTLE_S", 0)
    return d, ax, apps, posted


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

    def front_window(self, pid: int, like: Any = None, title: Any = None) -> Window | None:
        return Window(pid * 10, pid, self.bounds, "w")

    def window(self, wid: int) -> Window | None:
        return Window(wid, wid // 10, self.bounds, "w")

    def on_screen(self, wid: int) -> bool:
        return True

    def window_at(self, x: float, y: float, max_layer: int = 0) -> Window | None:
        return Window(self.top if self.top is not None else 110, 0, self.bounds, "w")

    def windows_at(self, x: float, y: float) -> list[Window] | None:
        return [w for w in [self.window_at(x, y)] if w is not None]     # every level: no FERB window here

    def capture(self, w: Window, max_edge: Any = None) -> tuple[bytes, int, int]:
        self.captured.append(w.id)
        if self.on_capture:
            self.on_capture()
        return self.jpeg, *IMG


PAGE_AREA = (100.0, 130.0, 800.0, 520.0)           # where a browser window at BOUNDS draws its page (80-pt toolbar)


class VisionAx(FakeAx):
    def __init__(self) -> None:
        # The window's tree shows a page Accessibility has read, as a browser's does (L13: a browser page nobody read
        # withholds its capture); a native app ignores it.
        super().__init__(AxTree(has_web=True, web_areas=[PAGE_AREA]))
        self.secure: list[bool | None] = []        # the password-field answer of each walk, in order (then False)
        self.hit_answer: Any = None                # None: Accessibility can't hit-test (the window list decides)
        self.hit_el: Any = None                    # the app's own hit test: the element under the point
        self.clicks = 0                            # clicks posted to the app's process that it took

    def read_tree(self, pid: int, browser: bool = False, window: Any = None) -> AxTree:
        secure = self.secure.pop(0) if self.secure else False
        return dataclasses.replace(self.tree, text=f"{self.tree.text} clicks={self.clicks}", secure=secure,
                                   window="win")          # the tree's masking inventory (secret rects, web areas) too

    def app_window(self, pid: int) -> Any:
        return "win"

    def frame(self, win: Any) -> Any:
        return BOUNDS

    def window_with_frame(self, pid: int, bounds: Any, title: Any = None, current_title: Any = None) -> Any:
        return "win"

    def hit(self, x: float, y: float) -> Any:
        return self.hit_answer

    def hit_element(self, pid: int, x: float, y: float) -> Any:
        return self.hit_el

    def element_window(self, el: Any) -> Any:
        return "win"

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
    mp.setattr(drv, "load_route", lambda: None)  # opt in explicitly; unit tests never post to real process IDs

    def clicks(x: float, y: float, pid: int | None = None) -> list[Step]:
        where = " to pid" if pid else ""

        def click() -> None:
            posted.append(f"click {x},{y}{where}")
            if pid and shots.takes_pid_clicks:                 # the window is on this Space: the app took it
                ax.clicks += 1
        return [Step("move", lambda: posted.append(f"move {x},{y}{where}")), Step("click", click)]
    mp.setattr(drv, "click_steps", clicks)
    mp.setattr(drv, "pointer_steps", lambda start, end, kind, pid, **kwargs: (clicks(*start, pid), lambda: None))
    return d, ax, shots, apps, posted


def _fake_mac(d: RunningDaemon, mp: pytest.MonkeyPatch) -> tuple[FakeShots, list[str]]:
    drv, ax, shots, apps, posted = vision(mp)
    mac = d.daemon.mac
    assert mac is not None
    mac.ax, mac.apps, mac.shots, mac.input = ax, apps, shots, drv.input
    return shots, posted


CHAT = "net.whatsapp.WhatsApp"


@dataclass
class Ax:
    """A fake app window: a text box, a Send button; typing lands only if the app takes background keystrokes."""
    takes_background: bool = True
    focused: str | None = "textbox"
    box_has_focus: bool | None = True                       # False: a sheet / dialog holds the keyboard focus
    press_ok: bool = True                                   # False: the control ignores Accessibility presses
    ax_keys: bool = False                                   # True: the app takes the Accessibility keyboard (spike 20)
    send_button: bool = True
    box: str = ""
    text: str = ""
    shortcut: Any = None                                    # what menu_item_for_shortcut answers
    calls: list[str] = field(default_factory=list)
    panels: list[tuple[str, Any]] = field(default_factory=list)

    def read_tree(self, pid: int, browser: bool = False, window: Any = None) -> AxTree:
        nodes = [AxNode("box", "textbox", "Message", self.box, (0, 0, 10, 10), False, True)]
        if self.send_button:
            nodes.append(AxNode("send", "button", "Send", None, (20, 0, 10, 10), False, True))
        return AxTree(nodes, self.text, "Chat")

    def value(self, el: Any) -> str | None:
        return self.box if el == "box" else None

    def expose_tree(self, pid: int) -> bool:
        self.calls.append("expose")
        return True

    def post_text(self, pid: int, text: str) -> bool:
        self.calls.append(f"ax keys {text}")
        if self.ax_keys:
            self.box += text
        return True

    def post_key(self, pid: int, keycode: int, mods: list[str]) -> bool:
        self.calls.append(f"ax key {keycode}")
        if self.ax_keys:
            self.text += f"[{keycode}]"
        return True

    def window_title(self, win: Any) -> str:
        return "" if win is None else getattr(self, "title", "Chat")

    def app_window(self, pid: int) -> Any:
        return "win"

    def focused_role(self, pid: int) -> str | None:
        return self.focused

    def has_focus(self, pid: int, el: Any) -> bool | None:
        return self.box_has_focus

    def focus(self, el: Any) -> bool:
        self.calls.append("focus")
        return True

    def set_value(self, el: Any, text: str) -> bool:
        return False                                        # this app ignores Accessibility values

    def press(self, el: Any) -> bool:
        self.calls.append("press")
        return self.press_ok

    def press_ancestor(self, el: Any) -> str | None:
        self.calls.append("press ancestor")
        return None

    def center(self, n: AxNode) -> tuple[float, float]:
        return 5.0, 5.0

    def menu_item_for_shortcut(self, pid: int, key: str, mods: list[str]) -> Any:
        return self.shortcut

    def open_panels(self, pid: int, win: Any) -> list[tuple[str, Any]]:
        return list(self.panels)

    def open_menus(self, pid: int) -> list[Any]:
        return []

    def cancel(self, el: Any) -> bool:
        self.calls.append(f"cancel {el}")
        self.panels = []
        return True


class Apps:
    def __init__(self) -> None:
        self.apps = [RunningApp(CHAT, "WhatsApp", 21, False), RunningApp("com.apple.TextEdit", "TextEdit", 22, False),
                     RunningApp("com.microsoft.VSCode", "Code", 23, True)]
        self.activated: list[str] = []

    def find(self, q: str) -> RunningApp | None:
        return next((a for a in self.apps if q.lower() in (a.bundle_id.lower(), a.name.lower(), f"pid:{a.pid}")), None)

    def frontmost(self) -> RunningApp | None:
        return next(a for a in self.apps if a.active)

    def activate(self, bid: str, pid: int | None = None) -> bool:
        self.activated.append(bid)
        self.apps = [RunningApp(a.bundle_id, a.name, a.pid, a.bundle_id == bid) for a in self.apps]
        return True



def background_mac(mp: pytest.MonkeyPatch, ax: Ax, owner: list[bool] | None = None,
          comm: bool = False, profiles: Profiles | None = None) -> tuple[MacComputerDriver, Apps, list[str]]:
    apps, posted = Apps(), []
    owner = owner if owner is not None else []

    async def no_sleep(s: float) -> None:
        return None

    def busy() -> bool:                              # each check of the owner takes the next answer
        return owner.pop(0) if owner else False

    def typed(text: str, pid: int | None = None) -> list[Step]:
        def post() -> None:
            posted.append(f"type {text!r} {'to pid' if pid else 'foreground'}")
            if pid is None or ax.takes_background:
                ax.box += text
        return [Step("type", post)]

    def keyed(combo: str, pid: int | None = None) -> list[Step]:
        def post() -> None:
            posted.append(f"key {combo} {'to pid' if pid else 'foreground'}")
            if pid is None or ax.takes_background:
                ax.text += f"[{combo}]"
        return [Step(combo, post)]
    mp.setattr(drv, "type_steps", typed)
    mp.setattr(drv, "key_steps", keyed)
    mp.setattr(drv, "click_steps", lambda x, y, pid=None: [Step("click", lambda: posted.append("click foreground"))])
    mp.setattr(drv, "SETTLE_S", 0)
    d = MacComputerDriver("dev", RealInput(busy, GuiLock(), sleep=no_sleep, poll_s=0.1, yield_after_s=5),
                          ax=ax, apps=apps, profiles=profiles)
    d.communication = lambda b: comm and b == CHAT
    return d, apps, posted


def ids(d: MacComputerDriver, app: str = "WhatsApp") -> dict[str, str]:
    _, out = asyncio.run(d.snapshot(app))
    return {e["label"]: e["id"] for e in out["elements"]}


@dataclass
class ScrollAx(Ax):
    position: int = 0
    keys_scroll: bool = True
    bar: bool = True

    def read_tree(self, pid: int, browser: bool = False, window: Any = None) -> AxTree:
        return AxTree([AxNode("area", "textbox", "Document", "text", (0, 0, 10, 10), False, True, "AXTextArea",
                              ("menu",))], f"line {self.position}", "Untitled")

    def scroll_area_of(self, el: Any) -> Any:
        return "scroll"

    def window_named(self, pid: int, which: str) -> Any:
        return "win" if which == "Untitled" else None

    def scroll_area_in(self, win: Any) -> Any:
        return "scroll"

    def frame(self, el: Any) -> Any:
        return (0.0, 0.0, 10.0, 10.0)

    def scroll_content(self, area: Any) -> Any:
        return "area"

    def scroll_position(self, area: Any, content: Any = None) -> tuple[Any, ...]:
        return (self.position / 10, None, (self.position, 20))

    def scroll_bar(self, area: Any, vertical: bool = True) -> Any:
        return "bar" if vertical and self.bar else None

    def perform(self, el: Any, action: str) -> bool:
        self.calls.append(f"{action} {el}")
        if el == "bar":
            self.position += 1 if action == "AXIncrement" else -1
            return True
        return False
