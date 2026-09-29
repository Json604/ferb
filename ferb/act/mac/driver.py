"""MacComputerDriver (section 5 ComputerDriver): takes an action on an app, resolves the method, calls the parts.

Element actions try Accessibility first (press / set value / focus: no focus change, nothing for the owner to
notice); only if the app ignores that does FERB fall back to real input — activating the app, one marked event at a
time under the GUI lock, yielding to the owner — and then puts the previously frontmost app back. Element ids come
from the latest snapshot of that app and go stale with a newer one. Excluded apps are never read or acted on;
browsers are read-only here (ax_browser).
"""

import asyncio
from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable, Collection, Literal

from ferb.act.mac import accessibility as axm
from ferb.act.mac import apps as appsm
from ferb.act.mac import screenshot as shotm
from ferb.act.mac.ax_browser import ACT_REFUSED, BROWSER_BUNDLES, is_browser
from ferb.act.mac.input import RealInput, Step, click_steps, key_steps, type_steps, valid_combo
from ferb.core.errors import Refused, StaleRef
from ferb.core.types import ActionResult, ComputerAction

UNTRUSTED_APPS = {"com.apple.mail": "email", "com.apple.MobileSMS": "message", "net.whatsapp.WhatsApp": "message",
                  "com.tinyspeck.slackmacgap": "message", "com.hnc.Discord": "message",
                  "ru.keepcoder.Telegram": "message", "com.microsoft.teams2": "message"}


PASSWORD_REFUSED = "that window shows a password field; FERB doesn't capture it (the frame was dropped)"


@dataclass(frozen=True)
class Shot:
    id: str
    window: int                                    # CGWindowID
    bounds: tuple[float, float, float, float]      # window, screen points
    size: tuple[int, int]                          # image px


@dataclass
class _AppState:
    gen: int = 0
    nodes: dict[str, axm.AxNode] = field(default_factory=dict)
    shots: int = 0
    shot: Shot | None = None


class MacComputerDriver:
    os: Literal["macos", "windows", "linux"] = "macos"

    def __init__(self, device_id: str, real_input: RealInput, excluded: Callable[[], Collection[str]] = frozenset,
                 ax: Any = axm, apps: Any = appsm, shots: Any = shotm,
                 web_password: Callable[[str, tuple[float, float, float, float]], Awaitable[bool | None]] | None = None,
                 ) -> None:
        self.device_id, self.input, self.excluded, self.ax, self.apps = device_id, real_input, excluded, ax, apps
        self.shots, self.web_password = shots, web_password     # web_password: the extension's check (9.4)
        self._state: dict[str, _AppState] = {}

    def app(self, name_or_bundle: str) -> Any:
        a = self.apps.find(name_or_bundle)
        if a is None:
            raise Refused(f"{name_or_bundle} isn't running (open it first)")
        ex = {e.lower() for e in self.excluded()}
        if a.bundle_id.lower() in ex or a.name.lower() in ex:
            raise Refused(f"{a.name} is on the owner's exclusion list")
        return a

    async def snapshot(self, name_or_bundle: str) -> tuple[Any, dict[str, Any]]:
        a = self.app(name_or_bundle)
        tree = await asyncio.to_thread(self.ax.read_tree, a.pid, is_browser(a.bundle_id))
        st = self._state.setdefault(a.bundle_id, _AppState())
        st.gen += 1
        st.nodes = {f"s{st.gen}e{i + 1}": n for i, n in enumerate(tree.nodes)}
        els = []
        for eid, n in st.nodes.items():
            d: dict[str, Any] = {"id": eid, "role": n.role, "label": n.label}
            if n.secret:
                d["sensitive"], d["filled"] = True, n.value == "filled"
            elif n.value not in (None, ""):
                d["value"] = n.value
            if not n.enabled:
                d["disabled"] = True
            els.append(d)
        out = {"app": a.name, "bundle_id": a.bundle_id, "window": tree.title, "elements": els, "text": tree.text}
        if tree.truncated:
            out["truncated"] = True
        return a, out

    def node(self, bundle_id: str, element: str) -> axm.AxNode:
        st = self._state.get(bundle_id)
        n = st.nodes.get(element) if st else None
        if n is None or not element.startswith(f"s{st.gen}e" if st else "-"):
            raise StaleRef(f"element {element} is from an older snapshot of this app; take a new one")
        return n

    async def screenshot(self, name_or_bundle: str) -> tuple[Any, Shot, bytes]:
        """The app's front window only, in memory. A password field the Accessibility tree shows (or, for a browser,
        the extension reports in the tab on screen), before or after the capture, drops the frame; when neither can
        tell, the capture goes ahead."""
        a = self.app(name_or_bundle)
        browser = BROWSER_BUNDLES.get(a.bundle_id)

        def find() -> Any:
            w = self.shots.front_window(a.pid, self.ax.frame(self.ax.app_window(a.pid)))
            if w is None:
                raise Refused(f"{a.name} has no window on screen")
            return w

        async def check(w: Any) -> None:
            win = await asyncio.to_thread(self.ax.window_with_frame, a.pid, w.bounds)
            if await asyncio.to_thread(self.ax.has_secure_field, win, browser is not None):
                raise Refused(PASSWORD_REFUSED)
            if browser and self.web_password is not None and await self.web_password(browser, w.bounds):
                raise Refused(PASSWORD_REFUSED)

        def grab(w: Any) -> tuple[bytes, int, int]:
            try:
                return self.shots.capture(w)                  # type: ignore[no-any-return]
            except (LookupError, RuntimeError, TimeoutError) as e:
                raise Refused(f"couldn't capture {a.name}'s window: {e}") from None
        w = await asyncio.to_thread(find)
        await check(w)
        jpeg, width, height = await asyncio.to_thread(grab, w)
        await check(w)                                        # appeared while capturing: the frame is dropped
        st = self._state.setdefault(a.bundle_id, _AppState())
        st.shots += 1
        st.shot = Shot(f"shot{st.shots}", w.id, w.bounds, (width, height))
        return a, st.shot, jpeg

    async def click_at(self, name_or_bundle: str, shot_id: str, x: float, y: float, holder: str = "") -> ActionResult:
        """A click at (x, y) in the app's latest screenshot (image px), as marked real input that yields to the owner.
        Before each event: the window is where it was when captured and a click at that point would reach it."""
        a = self.app(name_or_bundle)
        st = self._state.get(a.bundle_id)
        shot = st.shot if st else None
        if shot is None or shot.id != shot_id:
            raise StaleRef(f"{shot_id} isn't {a.name}'s latest screenshot; take a new one")
        if not (0 <= x < shot.size[0] and 0 <= y < shot.size[1]):
            raise Refused(f"({x}, {y}) is outside the {shot.size[0]}x{shot.size[1]} screenshot")
        now = await asyncio.to_thread(self.shots.window, shot.window)
        if now is None or not _same(now.bounds, shot.bounds):
            raise StaleRef("the window moved, resized or closed since the screenshot; take a new one")
        bx, by, bw, bh = shot.bounds
        px, py = bx + x * bw / shot.size[0], by + y * bh / shot.size[1]

        def reaches_the_window() -> bool:
            h = self.ax.hit(px, py)                       # the system's hit test: overlays that ignore clicks don't count
            if h is not None:
                return h[0] == a.pid and h[1] is not None and _same(h[1], shot.bounds)
            top = self.shots.window_at(px, py)
            return top is not None and top.id == shot.window

        def guarded(step: Step) -> Step:
            def post() -> None:
                w = self.shots.window(shot.window)
                if w is None or not _same(w.bounds, shot.bounds) or not reaches_the_window():
                    raise StaleRef("another window is over that point now, or the window moved; take a new "
                                   "screenshot")
                step.post()
            return Step(step.what, post)
        r = await self._real(a, [guarded(s) for s in click_steps(px, py)], holder, "click")
        return ActionResult(True, f"clicked ({x:.0f}, {y:.0f}) in {shot.id}; {r.detail}")

    async def act(self, app: str, action: ComputerAction, holder: str = "") -> ActionResult:
        a = self.app(app)
        if is_browser(a.bundle_id):
            raise Refused(ACT_REFUSED)
        if action.kind == "key":
            if not action.text or not valid_combo(action.text):
                raise Refused(f"unsupported key {action.text!r} (e.g. return, tab, escape, cmd+s)")
            return await self._real(a, key_steps(action.text), holder, action.text)
        if action.element is None:
            raise Refused("this action needs an element id from a snapshot")
        n = self.node(a.bundle_id, action.element)
        if n.secret and action.kind in ("set_value", "type"):
            raise Refused("that's a password field; FERB never types passwords for the owner")
        if action.kind == "press":
            if await asyncio.to_thread(self.ax.press, n.ref):
                return ActionResult(True, "pressed")
            return await self._real(a, click_steps(*self.ax.center(n)), holder, "click")
        if action.kind == "click":
            return await self._real(a, click_steps(*self.ax.center(n)), holder, "click")
        if action.kind == "focus":
            return ActionResult(await asyncio.to_thread(self.ax.focus, n.ref), "focused")
        if action.kind == "set_value":
            if await asyncio.to_thread(self.ax.set_value, n.ref, action.text or ""):
                return ActionResult(True, "value set")
            await asyncio.to_thread(self.ax.focus, n.ref)
            return await self._real(a, type_steps(action.text or ""), holder, "typing")
        if action.kind == "type":
            await asyncio.to_thread(self.ax.focus, n.ref)
            return await self._real(a, type_steps(action.text or ""), holder, "typing")
        raise Refused(f"unknown action {action.kind}")

    async def _real(self, a: Any, steps: list[Any], holder: str, what: str) -> ActionResult:
        """Real input goes to the frontmost app: bring this one forward, act, then put the owner's app back."""
        before = self.apps.frontmost()
        if before is None or before.bundle_id != a.bundle_id:
            self.apps.activate(a.bundle_id)
            await asyncio.sleep(0.3)
        try:
            n = await self.input.run(steps, holder, what)
        finally:
            if before is not None and before.bundle_id != a.bundle_id:
                self.apps.activate(before.bundle_id)
        return ActionResult(True, f"{what} ({n} steps, real input)")


def _same(a: tuple[float, ...], b: tuple[float, ...]) -> bool:
    return all(abs(p - q) <= 1 for p, q in zip(a, b))
