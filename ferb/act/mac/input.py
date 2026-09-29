"""Real mouse + keyboard events (section 10.5), always marked as FERB's and always under the GUI lock.

An action is a list of single events (one mouse event, one keystroke). Before each one the owner is checked: if
they're using the mouse or keyboard, FERB waits until they've been quiet for the quiet period, then continues; if
that takes longer than `yield_after_s`, the action stops and says where (`Yielded`: the gateway treats the partial
effect as outcome-unknown). Long text is typed per keystroke, never pasted in one event, so it can stop mid-way.
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

    async def run(self, steps: list[Step], holder: str, what: str) -> int:
        if not self.available():
            raise Refused("FERB is paused (`ferb resume`): it can't tell when you're using the mouse or keyboard, "
                          "so it doesn't use real input")
        async with self.gui.hold(holder):
            for i, step in enumerate(steps):
                waited = 0.0
                if self.owner_active():
                    self.on_wait(holder, True)
                    try:
                        while self.owner_active():           # the owner wins: wait for them to be quiet
                            if waited >= self.yield_after_s:
                                raise Yielded(i, len(steps), what)
                            await self.sleep(self.poll_s)
                            waited += self.poll_s
                    finally:
                        self.on_wait(holder, False)
                step.post()
                await self.sleep(STEP_S)
        return len(steps)


# ---------- the real events (Quartz), each marked ----------

def _post(ev: Any) -> None:
    import Quartz
    Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGEventSourceUserData, FERB_EVENT_MARK)
    Quartz.CGEventPost(Quartz.kCGHIDEventTap, ev)


def click_steps(x: float, y: float) -> list[Step]:
    import Quartz
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)
    pt = (x, y)

    def ev(*kinds: int) -> Callable[[], None]:
        def post() -> None:
            for kind in kinds:
                _post(Quartz.CGEventCreateMouseEvent(src, kind, pt, Quartz.kCGMouseButtonLeft))
        return post
    # down + up are one step: a yield or a failed check never leaves the button held down
    return [Step("move", ev(Quartz.kCGEventMouseMoved)),
            Step("click", ev(Quartz.kCGEventLeftMouseDown, Quartz.kCGEventLeftMouseUp))]


def type_steps(text: str) -> list[Step]:
    import Quartz
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)
    steps: list[Step] = []
    for ch in text:
        def key(ch: str = ch) -> None:
            for down in (True, False):
                ev = Quartz.CGEventCreateKeyboardEvent(src, 0, down)
                Quartz.CGEventKeyboardSetUnicodeString(ev, len(ch), ch)
                _post(ev)
        steps.append(Step(f"type {len(steps) + 1}/{len(text)}", key))
    return steps


KEYCODES = {"return": 36, "tab": 48, "space": 49, "delete": 51, "escape": 53, "left": 123, "right": 124,
            "down": 125, "up": 126, "a": 0, "s": 1, "c": 8, "v": 9, "x": 7, "z": 6, "w": 13, "n": 45, "f": 3}
MODIFIERS = {"cmd": 1 << 20, "shift": 1 << 17, "alt": 1 << 19, "ctrl": 1 << 18}


def key_steps(combo: str) -> list[Step]:
    """e.g. "cmd+s", "return". Unknown keys are refused by the caller."""
    import Quartz
    parts = combo.lower().split("+")
    flags = 0
    for m in parts[:-1]:
        flags |= MODIFIERS[m]
    code = KEYCODES[parts[-1]]
    src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)

    def press() -> None:
        for down in (True, False):
            ev = Quartz.CGEventCreateKeyboardEvent(src, code, down)
            Quartz.CGEventSetFlags(ev, flags)
            _post(ev)
    return [Step(combo, press)]


def valid_combo(combo: str) -> bool:
    parts = combo.lower().split("+")
    return parts[-1] in KEYCODES and all(m in MODIFIERS for m in parts[:-1])
