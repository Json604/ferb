"""MacComputerDriver (section 5 ComputerDriver): takes an action on an app, resolves the method, calls the parts.

Input order, every app (9.4, spike 18): Accessibility with the app in the background (press / set value / focus);
then keystrokes posted to the app's own process, still in the background (checked by what changed; an app that
ignores them is remembered); only then the foreground — bring the app forward once per stretch of work, one marked
event at a time under the GUI lock, the owner's input held back by the guard and winning (10.5), and the owner's app
put back once the task moves on or has been idle there for STRETCH_IDLE_S. Arrow keys in a text box and Return where
a Send button belongs are refused. Element ids come from the latest snapshot of that app and go stale with a newer
one. Excluded apps are never read or acted on; browsers are read-only here (ax_browser).
"""

import asyncio
from dataclasses import dataclass, field
from typing import Any, Awaitable, Callable, Collection, Literal

from ferb.act.mac import accessibility as axm
from ferb.act.mac import apps as appsm
from ferb.act.mac import screenshot as shotm
from ferb.act.mac.ax_browser import ACT_REFUSED, BROWSER_BUNDLES, is_browser
from ferb.act.mac.input import (RealInput, Step, click_steps, is_arrow, is_enter, key_steps, switches_apps,
                                type_steps, valid_combo)
from ferb.core.errors import Refused, StaleRef
from ferb.core.types import ActionResult, ComputerAction

UNTRUSTED_APPS = {"com.apple.mail": "email", "com.apple.MobileSMS": "message", "net.whatsapp.WhatsApp": "message",
                  "com.tinyspeck.slackmacgap": "message", "com.hnc.Discord": "message",
                  "ru.keepcoder.Telegram": "message", "com.microsoft.teams2": "message"}


PASSWORD_REFUSED = "that window shows a password field; FERB doesn't capture it (the frame was dropped)"


STRETCH_IDLE_S = 3.0          # the owner's app comes back after this long without foreground work in FERB's app
SETTLE_S = 0.4                # after background keystrokes, before checking what changed
FOCUS_ELSEWHERE = ("something else in the app has the keyboard focus (a dialog or sheet may be open), so nothing was "
                   "typed; take a new snapshot")
ARROW_REFUSED = ("arrow keys in a text box aren't used: set the box's whole value again (action set_value) instead "
                 "of moving the cursor to edit it")


class _NoGuard:
    def engage(self) -> bool:
        return False

    def release(self) -> None:
        return None


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
        self.on_activate: Callable[[str, str | None], None] = lambda bundle, task: None   # the trail's attribution
        self._state: dict[str, _AppState] = {}
        self.snapshot_images: Callable[[], bool] = lambda: True   # `[mac] snapshot_image` (owner request, 2026-10-01)
        self.communication: Callable[[str], bool] = lambda bundle: False  # messaging / calling apps (9.3)
        self.guard: Any = _NoGuard()                    # holds back the owner's input while FERB's app is forward
        self._background_ok: dict[str, bool] = {}       # does this app take keystrokes in the background?
        self._forward_app: str | None = None            # the app FERB brought forward, while a stretch lasts
        self._owner_app: Any = None                     # the app the owner was in, to put back
        self._restore_at: asyncio.TimerHandle | None = None
        self._exposed: set[int] = set()                 # app processes asked for their full tree (Electron)

    def app(self, name_or_bundle: str) -> Any:
        a = self.apps.find(name_or_bundle)
        if a is None:
            raise Refused(f"{name_or_bundle} isn't running (open it first)")
        ex = {e.lower() for e in self.excluded()}
        if a.bundle_id.lower() in ex or a.name.lower() in ex:
            raise Refused(f"{a.name} is on the owner's exclusion list")
        return a

    async def _expose(self, a: Any) -> None:
        if a.pid not in self._exposed:
            self._exposed.add(a.pid)
            await asyncio.to_thread(self.ax.expose_tree, a.pid)

    async def menus(self, name_or_bundle: str) -> list[dict[str, Any]]:
        a = self.app(name_or_bundle)
        result: list[dict[str, Any]] = await asyncio.to_thread(self.ax.menus, a.pid)
        return result

    async def press_menu(self, name_or_bundle: str, path: list[str]) -> bool:
        a = self.app(name_or_bundle)
        return bool(await asyncio.to_thread(self.ax.press_menu, a.pid, path))

    async def ensure_window(self, bundle_id: str, wait_s: float = 10.0) -> bool:
        """After opening an app: wait for its main window — one with a title; slow starters (Electron apps) show an
        untitled panel first — reopening it from the app's own menu if there's none (in the background). True once
        there's a window (titled, or untitled when that's all it shows by the end)."""
        reopened, untitled = False, False
        for i in range(int(wait_s / 0.25)):
            a = self.apps.find(bundle_id)
            if a is not None:
                await self._expose(a)
                win = await asyncio.to_thread(self.ax.app_window, a.pid)
                if win is not None and self.ax.window_title(win):
                    return True
                untitled = untitled or win is not None
                if win is None and not reopened and i >= 4:        # running ~1 s and still no window
                    reopened = await asyncio.to_thread(self.ax.open_window, a.pid, a.name)
            await asyncio.sleep(0.25)
        return untitled

    async def snapshot(self, name_or_bundle: str) -> tuple[Any, dict[str, Any]]:
        a = self.app(name_or_bundle)
        await self._expose(a)
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

    def marks(self, bundle_id: str) -> list[tuple[str, tuple[float, float, float, float]]]:
        """The latest snapshot's controls as numbered boxes for the image: "12" = element s<gen>e12."""
        st = self._state.get(bundle_id)
        if st is None:
            return []
        return [(eid.split("e", 1)[1], n.rect) for eid, n in st.nodes.items() if n.rect[2] > 0 and n.rect[3] > 0]

    async def screenshot(self, name_or_bundle: str,
                         marks: list[tuple[str, tuple[float, float, float, float]]] | None = None
                         ) -> tuple[Any, Shot, bytes]:
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
                if marks:
                    return self.shots.capture(w, marks=marks)     # type: ignore[no-any-return]
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

    async def click_at(self, name_or_bundle: str, shot_id: str, x: float, y: float, holder: str = "",
                       task: str | None = None) -> ActionResult:
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
        r = await self._real(a, [guarded(s) for s in click_steps(px, py)], holder, "click", task,
                             prepare=lambda: self._bring_forward(a, shot))
        return ActionResult(True, f"clicked ({x:.0f}, {y:.0f}) in {shot.id}; {r.detail}")

    def label_at(self, name_or_bundle: str, shot_id: str, x: float, y: float) -> str | None:
        """What control is at (x, y) of the app's latest screenshot, by Accessibility (None: it can't tell)."""
        a = self.app(name_or_bundle)
        st = self._state.get(a.bundle_id)
        shot = st.shot if st else None
        if shot is None or shot.id != shot_id:
            raise StaleRef(f"{shot_id} isn't {a.name}'s latest screenshot; take a new one")
        bx, by, bw, bh = shot.bounds
        label = self.ax.label_at(bx + x * bw / shot.size[0], by + y * bh / shot.size[1])
        return str(label) if label else None

    async def screen_text(self, name_or_bundle: str) -> dict[str, Any]:
        """The window's Accessibility text, for FERB's own checks only (who was on screen): it doesn't change the
        element ids of the latest snapshot and never reaches the model."""
        a = self.app(name_or_bundle)
        tree = await asyncio.to_thread(self.ax.read_tree, a.pid, is_browser(a.bundle_id))
        return {"bundle_id": a.bundle_id, "text": tree.text,
                "elements": [{"role": n.role, "label": n.label, "value": None if n.secret else n.value}
                             for n in tree.nodes]}

    async def act(self, app: str, action: ComputerAction, holder: str = "", task: str | None = None) -> ActionResult:
        a = self.app(app)
        if is_browser(a.bundle_id):
            raise Refused(ACT_REFUSED)
        if action.kind == "key":
            if not action.text or not valid_combo(action.text):
                raise Refused(f"unsupported key {action.text!r} (e.g. return, tab, escape, cmd+s)")
            if switches_apps(action.text):
                raise Refused(f"{action.text} switches the owner's apps or desktops; FERB works with {a.name} where it "
                              "is — no need to bring it forward")
            await self._key_rules(a, action.text)
            return await self._key(a, action.text, holder, task)
        if action.element is None:
            raise Refused("this action needs an element id from a snapshot")
        n = self.node(a.bundle_id, action.element)
        if n.secret and action.kind in ("set_value", "type"):
            raise Refused("that's a password field; FERB never types passwords for the owner")
        if action.kind == "press":
            if await asyncio.to_thread(self.ax.press, n.ref):
                return ActionResult(True, "pressed")
            return await self._real(a, click_steps(*self.ax.center(n)), holder, "click", task)
        if action.kind == "click":                                # a control: pressed in the background if it can be
            if await asyncio.to_thread(self.ax.press, n.ref):
                return ActionResult(True, "pressed")
            return await self._real(a, click_steps(*self.ax.center(n)), holder, "click", task)
        if action.kind == "focus":
            return ActionResult(await asyncio.to_thread(self.ax.focus, n.ref), "focused")
        if action.kind == "set_value":
            # only a real text field takes a value that the app then acts on (WhatsApp's search "button" accepts a
            # set value but never searches on it — Sept 30 trail); anything else is clicked and typed into
            if n.role in ("textbox", "searchbox", "combobox") and \
                    await asyncio.to_thread(self.ax.set_value, n.ref, action.text or ""):
                return ActionResult(True, "value set")
            return await self._type(a, n, action.text or "", holder, task)
        if action.kind == "type":
            return await self._type(a, n, action.text or "", holder, task)
        raise Refused(f"unknown action {action.kind}")

    # ---------- keys and typing: in the background first (9.4 step 2) ----------

    async def _key_rules(self, a: Any, combo: str) -> None:
        """Buttons over keystrokes, whole values over cursor editing (owner decision 2026-10-01, every app)."""
        if not (is_arrow(combo) or is_enter(combo)):
            return
        if await asyncio.to_thread(self.ax.focused_role, a.pid) not in ("textbox", "searchbox"):
            return
        if is_arrow(combo):
            raise Refused(ARROW_REFUSED)
        send = self._send_button(a.bundle_id)
        if send is not None:
            raise Refused(f"don't press Return in a text box: press the Send button ({send}) instead")
        if self.communication(a.bundle_id):
            raise Refused("in a messaging app Return would send without FERB's checks: press the Send button instead "
                          "(take a new snapshot to find it — it may appear only once there's text)")

    def _send_button(self, bundle_id: str) -> str | None:
        from ferb.act.mac.tools import SEND_LABEL
        st = self._state.get(bundle_id)
        for eid, n in (st.nodes.items() if st else ()):
            if n.role == "button" and n.enabled and SEND_LABEL.search(n.label or ""):
                return eid
        return None

    async def _signature(self, a: Any) -> tuple[Any, ...]:
        """What the window shows, to tell whether a background keystroke did anything (snapshot ids untouched)."""
        tree = await asyncio.to_thread(self.ax.read_tree, a.pid, False)
        return tree.title, tree.text, tuple((n.role, n.label, n.value) for n in tree.nodes)

    async def _key(self, a: Any, combo: str, holder: str, task: str | None) -> ActionResult:
        if self._background_ok.get(a.bundle_id) is not False:
            before = await self._signature(a)
            await self.input.to_process(key_steps(combo, a.pid), holder)
            await asyncio.sleep(SETTLE_S)
            if await self._signature(a) != before:
                self._background_ok[a.bundle_id] = True
                return ActionResult(True, f"{combo} (in the background)")
            if self._background_ok.get(a.bundle_id):            # the app takes them; this key just showed nothing
                return ActionResult(True, f"{combo} sent in the background; nothing visibly changed")
            self._background_ok[a.bundle_id] = False
            return ActionResult(False, f"{combo} went to {a.name} in the background but nothing changed; if it "
                                       "should have, do it again (FERB will bring the app forward this time)")
        return await self._real(a, key_steps(combo), holder, combo, task)

    async def _type(self, a: Any, n: Any, text: str, holder: str, task: str | None) -> ActionResult:
        """Keystrokes go to the app's keyboard focus: typed only once that is the box FERB means (a Save sheet or a
        dialog can hold it — then nothing is typed, here or in the foreground)."""
        await asyncio.to_thread(self.ax.focus, n.ref)
        focused = await asyncio.to_thread(self.ax.has_focus, a.pid, n.ref)
        if focused and self._background_ok.get(a.bundle_id) is not False:
            before = await asyncio.to_thread(self.ax.value, n.ref)
            sig = await self._signature(a) if before is None else None
            await self.input.to_process(type_steps(text, a.pid), holder)
            await asyncio.sleep(SETTLE_S)
            after = await asyncio.to_thread(self.ax.value, n.ref)
            landed = (after != before) if before is not None or after is not None else await self._signature(a) != sig
            if landed:
                self._background_ok[a.bundle_id] = True
                return ActionResult(True, "typed (in the background)")
            self._background_ok[a.bundle_id] = False          # nothing arrived: the foreground, once

        if n.role not in ("textbox", "searchbox"):
            # Not a real text field (WhatsApp's search box reports itself as an unlabeled button and only searches on
            # real typing — Sept 30 trail): click it in front, then type, as a person would. Accessibility can't tell
            # where its focus lands, so the focus check (for real text fields) can't apply here.
            return await self._real(a, click_steps(*self.ax.center(n)) + type_steps(text), holder, "click + typing",
                                    task)

        async def focus_it() -> None:                          # in front now: focus the box, and check it took
            await asyncio.to_thread(self.ax.focus, n.ref)
            if await asyncio.to_thread(self.ax.has_focus, a.pid, n.ref) is False:
                raise Refused(FOCUS_ELSEWHERE)
        return await self._real(a, type_steps(text), holder, "typing", task, prepare=focus_it)

    async def _bring_forward(self, a: Any, shot: Shot) -> None:
        """After activating the app: raise the captured window itself and wait (~3 s) for it to be on screen — the
        app may switch to another Space or a full-screen window first."""
        for _ in range(12):
            await asyncio.to_thread(self.ax.raise_window, a.pid, shot.bounds)
            if await asyncio.to_thread(self.shots.on_screen, shot.window):
                await asyncio.sleep(0.2)
                return
            await asyncio.sleep(0.25)

    # ---------- the foreground, last resort (9.4 step 3) ----------

    async def _real(self, a: Any, steps: list[Any], holder: str, what: str, task: str | None = None,
                    prepare: Callable[[], Awaitable[None]] | None = None) -> ActionResult:
        """Real input goes to the frontmost app: bring this one forward (once per stretch of work), act, and leave it
        there a moment for the next step; the owner's app comes back when the stretch ends. Every switch is FERB's
        (the trail attributes it to the task)."""
        if not self.input.available():
            return ActionResult(False, "FERB is paused (`ferb resume`): no real mouse / keyboard input")
        await self.input.quiet(holder, what)                      # the owner is busy: don't pull them away now
        await self._forward(a, task)

        async def aside() -> None:                                # the owner touched the mouse / keyboard
            self.restore_owner(task)

        async def back() -> None:
            await self._forward(a, task)
            if prepare is not None:
                await prepare()
        try:
            if prepare is not None:
                await prepare()
            n = await self.input.run(steps, holder, what, aside=aside, back=back)
        finally:
            self._restore_later(task)
        return ActionResult(True, f"{what} ({n} steps, real input)")

    async def _forward(self, a: Any, task: str | None) -> None:
        if self._restore_at is not None:
            self._restore_at.cancel()
            self._restore_at = None
        front = self.apps.frontmost()
        if self._forward_app is None and front is not None and front.bundle_id != a.bundle_id:
            self._owner_app = front                               # where the owner was: put back at the end
        if front is None or front.bundle_id != a.bundle_id:
            if self._forward_app is None:
                self.guard.engage()                               # before the switch: none of their input lands
            self.on_activate(a.bundle_id, task)
            self.apps.activate(a.bundle_id)
            await asyncio.sleep(0.3)
        self._forward_app = a.bundle_id

    def restore_owner(self, task: str | None = None) -> None:
        """End of a stretch, or the owner stepped in: their app in front again, the guard off."""
        if self._restore_at is not None:
            self._restore_at.cancel()
            self._restore_at = None
        if self._forward_app is None:
            return
        owner, self._owner_app, self._forward_app = self._owner_app, None, None
        if owner is not None:
            self.on_activate(owner.bundle_id, task)
            self.apps.activate(owner.bundle_id)
        self.guard.release()

    def _restore_later(self, task: str | None) -> None:
        if self._forward_app is None:
            return
        if self._restore_at is not None:
            self._restore_at.cancel()
        self._restore_at = asyncio.get_running_loop().call_later(STRETCH_IDLE_S, self.restore_owner, task)


def _same(a: tuple[float, ...], b: tuple[float, ...]) -> bool:
    return all(abs(p - q) <= 1 for p, q in zip(a, b))
