"""Real mouse + keyboard events (section 10.5), always marked as FERB's and always under the GUI lock.

An action is a list of single events (one mouse event, one keystroke). Before each one the owner is checked: if
they're using the mouse or keyboard, FERB waits until they've been quiet for the quiet period, then continues; if
that takes longer than `yield_after_s`, the action stops and says where (`Yielded`: the gateway treats the partial
effect as outcome-unknown). Long text is typed per keystroke, never pasted in one event, so it can stop mid-way.

Events can also go straight to one app's process (`pid`, `CGEventPostToPid`), with the app in the background
(PLATFORM_MACOS.md §4): keys reach the app's first responder wherever its window is; mouse and scroll events reach
a window on the current Space in our earlier probes. Exact pointer input can alternatively use a window-addressed
SkyLight route (window_input.py); app acceptance and cross-Space behavior require owner verification.
"""

import asyncio
from dataclasses import dataclass
from typing import Any, Awaitable, Callable

from ferb.act.gui_lock import GuiLock
from ferb.core.errors import FerbError, Refused
from ferb.sense.watchers.owner_input import FERB_EVENT_MARK

STEP_S = 0.02


class Yielded(FerbError):
    def __init__(self, done: int, total: int, what: str) -> None:
        super().__init__(f"stopped for the owner after {done} of {total} steps ({what}); the owner kept using the "
                         "mouse/keyboard")
        self.done, self.total = done, total


class PointerInterrupted(FerbError):
    """Some pointer input may have landed; the gateway must keep its outcome unknown."""


@dataclass
class Step:
    what: str
    post: Callable[[], None]


class RealInput:
    def __init__(self, owner_active: Callable[[], bool], gui: GuiLock, yield_after_s: float = 30.0,
                 sleep: Callable[[float], Awaitable[None]] = asyncio.sleep, poll_s: float = 0.05,
                 on_wait: Callable[[str, bool], None] = lambda holder, on: None,
                 available: Callable[[], bool] = lambda: True) -> None:
        self.owner_active, self.gui, self.yield_after_s = owner_active, gui, yield_after_s
        self.sleep, self.poll_s, self.on_wait = sleep, poll_s, on_wait     # on_wait: the owner's time isn't a stall
        self.available = available                                         # False while FERB is paused

    async def run(self, steps: list[Step], holder: str, what: str,
                  aside: Callable[[], Awaitable[None]] | None = None,
                  back: Callable[[], Awaitable[None]] | None = None) -> int:
        """Foreground input. When the owner touches the mouse or keyboard: `aside` (put their app back), wait until
        they've been quiet, `back` (bring FERB's app forward again), carry on (10.5)."""
        if not self.available():
            raise Refused("FERB is paused (`ferb resume`): it can't tell when you're using the mouse or keyboard, "
                          "so it doesn't use real input")
        async with self.gui.hold(holder):
            for i, step in enumerate(steps):
                waited = 0.0
                if self.owner_active():
                    self.on_wait(holder, True)
                    if aside is not None:
                        await aside()
                    try:
                        while self.owner_active():           # the owner wins: wait for them to be quiet
                            if waited >= self.yield_after_s:
                                raise Yielded(i, len(steps), what)
                            await self.sleep(self.poll_s)
                            waited += self.poll_s
                    finally:
                        self.on_wait(holder, False)
                    if back is not None:
                        await back()
                step.post()
                await self.sleep(STEP_S)
        return len(steps)

    async def quiet(self, holder: str, what: str) -> None:
        """Before bringing an app forward: if the owner is using the Mac, wait until they've been quiet."""
        waited = 0.0
        if not self.owner_active():
            return
        self.on_wait(holder, True)
        try:
            while self.owner_active():
                if waited >= self.yield_after_s:
                    raise Yielded(0, 1, what)
                await self.sleep(self.poll_s)
                waited += self.poll_s
        finally:
            self.on_wait(holder, False)

    async def to_process(self, steps: list[Step], holder: str, cleanup: Callable[[], None] | None = None) -> int:
        """Events posted to one app's process in the background: no owner wait (they can't collide), still under
        the GUI lock so two workers' keystrokes don't interleave."""
        async with self.gui.hold(holder):
            try:
                for step in steps:
                    step.post()
                    await self.sleep(STEP_S)
            finally:
                if cleanup is not None:
                    cleanup()
        return len(steps)


# ---------- the real events (Quartz), each marked ----------

def pointer_steps(start: tuple[float, float], end: tuple[float, float], kind: str,
                  pid: int, *, window: int | None = None, route: Any = None,
                  window_origin: tuple[float, float] = (0, 0)) -> tuple[list[Step], Callable[[], None]]:
    """A bounded process-directed gesture; cleanup releases before the GUI lock passes to another task."""
    import Quartz as q
    if kind not in {"click", "dblclick", "rightclick", "hover", "drag"}:
        raise Refused("unsupported pointer action")
    source = q.CGEventSourceCreate(q.kCGEventSourceStatePrivate)
    button = q.kCGMouseButtonRight if kind == "rightclick" else q.kCGMouseButtonLeft
    down = q.kCGEventRightMouseDown if kind == "rightclick" else q.kCGEventLeftMouseDown
    up = q.kCGEventRightMouseUp if kind == "rightclick" else q.kCGEventLeftMouseUp
    held, position = False, start

    def post(event: int, pt: tuple[float, float], count: int = 1) -> None:
        nonlocal held, position
        position = pt
        if event == down:
            held = True
        ev = q.CGEventCreateMouseEvent(source, event, pt, button)
        q.CGEventSetIntegerValueField(ev, q.kCGMouseEventClickState, count)
        if route is not None and window is not None:
            route.post(ev, pid, window, (pt[0] - window_origin[0], pt[1] - window_origin[1]))
        else:
            _post(ev, pid)
        if event == up:
            held = False

    def release() -> None:
        if held:
            post(up, position)

    steps = [Step("pointer move", lambda: post(q.kCGEventMouseMoved, start))]
    if kind != "hover":
        for count in range(1, 3 if kind == "dblclick" else 2):
            steps.append(Step("pointer down", lambda n=count: post(down, start, n)))
            if kind == "drag":
                for i in range(1, 21):
                    pt = (start[0] + (end[0] - start[0]) * i / 20, start[1] + (end[1] - start[1]) * i / 20)
                    steps.append(Step("pointer drag", lambda pt=pt: post(q.kCGEventLeftMouseDragged, pt)))
            steps.append(Step("pointer up", lambda n=count: post(up, end if kind == "drag" else start, n)))
    return steps, release

def _post(ev: Any, pid: int | None = None) -> None:
    import Quartz
    Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGEventSourceUserData, FERB_EVENT_MARK)
    if pid is None:
        Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)
    else:
        Quartz.CGEventPostToPid(pid, ev)                     # to that app only, wherever it is (spike 18)


def click_steps(x: float, y: float, pid: int | None = None) -> list[Step]:
    """A move then a click at a screen point: real input, or posted to the app's process (reaches a window on the
    current Space only — PLATFORM_MACOS.md §11)."""
    import Quartz
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)
    pt = (x, y)

    def ev(*kinds: int) -> Callable[[], None]:
        def post() -> None:
            for kind in kinds:
                e = Quartz.CGEventCreateMouseEvent(src, kind, pt, Quartz.kCGMouseButtonLeft)
                Quartz.CGEventSetIntegerValueField(e, Quartz.kCGMouseEventClickState, 1)
                _post(e, pid)
        return post
    # down + up are one step: a yield or a failed check never leaves the button held down
    return [Step("move", ev(Quartz.kCGEventMouseMoved)),
            Step("click", ev(Quartz.kCGEventLeftMouseDown, Quartz.kCGEventLeftMouseUp))]


def wheel_steps(x: float, y: float, lines: int, pid: int | None = None, *,
                horizontal: bool = False, window: int | None = None, route: Any = None,
                pixels: bool = False, window_origin: tuple[float, float] = (0, 0)) -> list[Step]:
    """Scroll-wheel events at a point (negative lines = down), real or posted to the app's process."""
    import Quartz
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)

    def post() -> None:
        ev = Quartz.CGEventCreateScrollWheelEvent(src, Quartz.kCGScrollEventUnitPixel if pixels else Quartz.kCGScrollEventUnitLine, 2, 0 if horizontal else lines, lines if horizontal else 0)
        Quartz.CGEventSetLocation(ev, (x, y))
        if route is not None and pid is not None and window is not None:
            route.post(ev, pid, window, (x - window_origin[0], y - window_origin[1]))
        else:
            _post(ev, pid)
    return [Step(f"scroll {lines:+d}", post)]


def type_steps(text: str, pid: int | None = None) -> list[Step]:
    import Quartz
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)
    steps: list[Step] = []
    for ch in text:
        def key(ch: str = ch) -> None:
            for down in (True, False):
                ev = Quartz.CGEventCreateKeyboardEvent(src, 0, down)
                Quartz.CGEventKeyboardSetUnicodeString(ev, len(ch), ch)
                _post(ev, pid)
        steps.append(Step(f"type {len(steps) + 1}/{len(text)}", key))
    return steps


# Virtual key codes (Carbon Events.h, US layout) — platform facts.
KEYCODES: dict[str, int] = {
    **dict(zip("asdfhgzxcvbqweryt", [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16, 17])),
    **dict(zip("123465=97-80]ou[ip", [18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35])),
    **dict(zip("lj'k;\\,/nm.`", [37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 50])),
    "return": 36, "tab": 48, "space": 49, "delete": 51, "escape": 53, "forwarddelete": 117,
    "home": 115, "end": 119, "pageup": 116, "pagedown": 121, "help": 114,
    "left": 123, "right": 124, "down": 125, "up": 126,
    "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97, "f7": 98, "f8": 100, "f9": 101, "f10": 109,
    "f11": 103, "f12": 111, "f13": 105, "f14": 107, "f15": 113, "f16": 106, "f17": 64, "f18": 79, "f19": 80,
    "capslock": 57, "fn": 63, "volumeup": 72, "volumedown": 73, "mute": 74,
    "keypad0": 82, "keypad1": 83, "keypad2": 84, "keypad3": 85, "keypad4": 86, "keypad5": 87, "keypad6": 88,
    "keypad7": 89, "keypad8": 91, "keypad9": 92, "keypadenter": 76, "keypadplus": 69, "keypadminus": 78,
    "keypadmultiply": 67, "keypaddivide": 75, "keypaddecimal": 65, "keypadequals": 81, "keypadclear": 71,
}
KEY_ALIASES = {"enter": "return", "esc": "escape", "backspace": "delete", "del": "forwarddelete",
               "fwddelete": "forwarddelete", "pgup": "pageup", "pgdn": "pagedown", "pgdown": "pagedown",
               "spacebar": "space", "comma": ",", "period": ".", "slash": "/", "semicolon": ";", "quote": "'",
               "minus": "-", "equals": "=", "equal": "=", "plus": "=", "backslash": "\\", "grave": "`",
               "leftbracket": "[", "rightbracket": "]", "arrowleft": "left", "arrowright": "right", "arrowup": "up",
               "arrowdown": "down"}
MODIFIERS = {"cmd": 1 << 20, "shift": 1 << 17, "alt": 1 << 19, "ctrl": 1 << 18, "fn": 1 << 23}
MODIFIER_ALIASES = {"command": "cmd", "meta": "cmd", "super": "cmd", "option": "alt", "opt": "alt",
                    "control": "ctrl", "ctl": "ctrl", "function": "fn"}


def normalize(combo: str) -> list[str]:
    """"Cmd+Shift+S" -> ["cmd", "shift", "s"]; aliases resolved, modifiers in a fixed order, the key last."""
    parts = [p.strip().lower() for p in combo.replace(" ", "").split("+") if p.strip()]
    if combo.strip() == "+":
        parts = ["="]
    if not parts:
        return []
    mods = sorted({MODIFIER_ALIASES.get(p, p) for p in parts[:-1]}, key=list(MODIFIERS).index) \
        if all(MODIFIER_ALIASES.get(p, p) in MODIFIERS for p in parts[:-1]) else [MODIFIER_ALIASES.get(p, p)
                                                                                   for p in parts[:-1]]
    key = KEY_ALIASES.get(parts[-1], parts[-1])
    return mods + [key]


def key_steps(combo: str, pid: int | None = None) -> list[Step]:
    """e.g. "cmd+s", "return", "pagedown". Unknown keys are refused by the caller (valid_combo)."""
    import Quartz
    parts = normalize(combo)
    flags = 0
    for m in parts[:-1]:
        flags |= MODIFIERS[m]
    code = KEYCODES[parts[-1]]
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)

    def press() -> None:
        for down in (True, False):
            ev = Quartz.CGEventCreateKeyboardEvent(src, code, down)
            Quartz.CGEventSetFlags(ev, flags)
            _post(ev, pid)
    return [Step(combo, press)]


ARROWS = {"left", "right", "up", "down"}


def switches_apps(combo: str) -> bool:
    """⌘Tab and Control+arrows act on the owner's whole Mac (switch apps / desktops), not on one app; so do the
    Mission Control / Spotlight keys."""
    parts = normalize(combo)
    if not parts:
        return False
    key, mods = parts[-1], set(parts[:-1])
    return (key == "tab" and "cmd" in mods) or (key in ARROWS and "ctrl" in mods) \
        or (key == "space" and "cmd" in mods) or (key in ("f3", "f4") and not mods)


def quits_or_hides(combo: str) -> bool:
    """⌘Q quits the app (never: a background app with unsaved work wedges on its save sheet — PLATFORM_MACOS.md §3),
    ⌘H / ⌥⌘H hide it and ⌘M minimises its window: the owner's screen, not FERB's."""
    parts = normalize(combo)
    if not parts:
        return False
    key, mods = parts[-1], set(parts[:-1])
    return (key == "q" and mods == {"cmd"}) or (key == "h" and "cmd" in mods) or (key == "m" and mods == {"cmd"})


def is_arrow(combo: str) -> bool:
    parts = normalize(combo)
    return bool(parts) and parts[-1] in ARROWS


def is_enter(combo: str) -> bool:
    parts = normalize(combo)
    return bool(parts) and parts[-1] == "return" and not parts[:-1]


def is_shortcut(combo: str) -> bool:
    """A key equivalent (⌘ with a key): in a background app it goes through the menu item it maps to."""
    parts = normalize(combo)
    return bool(parts) and "cmd" in parts[:-1]


def valid_combo(combo: str) -> bool:
    parts = normalize(combo)
    return bool(parts) and parts[-1] in KEYCODES and all(m in MODIFIERS for m in parts[:-1])
