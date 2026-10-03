"""MacComputerDriver (section 5 ComputerDriver): takes an action on an app, background first, foreground by request.

Order per primitive (docs/PLATFORM_MACOS.md §12 and the owner-verified rules), each rung tried only after the one
before it verifiably did nothing, with the per-app learned profile (ferb.act.mac.profile) recording what landed:
  press     AXPress → AXPress on the nearest ancestor that lists it → AXFocused + Return / Space to the process
  type      AXFocused + unicode keys to the process (read back) → the Accessibility keyboard (apps the profile
            marks) → AXValue → foreground
  edit      AXSelectedTextRange + AXSelectedText → AXValue rewrite with the edit applied (no arrow keys, ever)
  key       a ⌘ shortcut goes through the menu item it maps to, pressed only when that item is enabled (a disabled
            key equivalent would only beep); Escape dismisses a sheet / dialog / menu with AXCancel; other keys go
            to the process and are checked by what changed
  scroll    AXScrollToVisible → slider / stepper AXIncrement → Page Up / Down keys with the scroll area focused →
            the scroll bar's AXValue / AXIncrement → foreground
  select    AXPress the pop-up button, then AXPress the menu item by title (AXValue where settable)
  click at  the app's own hit test → AXPress; pixel click to the process only for a window on the current Space,
            verified; foreground
The foreground (real input, the app brought forward once per stretch, the owner's input held back and winning,
their app put back) runs only when the call carries allow_foreground=true. Excluded apps are never read or acted
on; browsers are read-only here (ax_browser). Element ids come from the latest snapshot of that app.
"""

import asyncio
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Awaitable, Callable, Collection, Literal

from ferb.act.mac import accessibility as axm
from ferb.act.mac import apps as appsm
from ferb.act.mac import screenshot as shotm
from ferb.act.mac.ax_browser import ACT_REFUSED, BROWSER_BUNDLES, is_browser
from ferb.act.mac.window_input import load_route
from ferb.act.mac.input import (KEYCODES, PointerInterrupted, RealInput, Step, click_steps, is_arrow, is_enter, is_shortcut, key_steps,
                                normalize, pointer_steps, quits_or_hides, switches_apps, type_steps, valid_combo, wheel_steps)
from ferb.act.mac.profile import Profiles
from ferb.core.errors import Refused, StaleRef
from ferb.core.types import ActionResult, ComputerAction

UNTRUSTED_APPS = {"com.apple.mail": "email", "com.apple.MobileSMS": "message", "net.whatsapp.WhatsApp": "message",
                  "com.tinyspeck.slackmacgap": "message", "com.hnc.Discord": "message",
                  "ru.keepcoder.Telegram": "message", "com.microsoft.teams2": "message"}


PASSWORD_REFUSED = "that window shows a password field; FERB doesn't capture it (the frame was dropped)"


STRETCH_IDLE_S = 3.0          # the owner's app comes back after this long without foreground work in FERB's app
SETTLE_S = 0.4                # after background events, before reading what changed (one read, from the walker)
TREE_FRESH_S = 1.0            # a walk this recent serves a second reader (the snapshot's image check)
FOCUS_ELSEWHERE = ("something else in the app has the keyboard focus (a dialog or sheet may be open), so nothing was "
                   "typed; take a new snapshot")
ARROW_REFUSED = ("arrow keys in a text box aren't used (a background text view beeps at them): use action edit "
                 "with select (start, length) / \"all\" / \"end\" and text, or set_value for the whole text")
FOREGROUND_HINT = ("nothing more can be done in the background; call again with allow_foreground=true to do it "
                   "with the app in front (the owner's screen will move to it)")
TEXT_KINDS = axm.TEXT_KINDS


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
    at: float = field(default_factory=time.monotonic)
    signature: tuple[Any, ...] | None = None


@dataclass
class _AppState:
    gen: int = 0
    nodes: dict[str, axm.AxNode] = field(default_factory=dict)
    shots: int = 0
    shot: Shot | None = None
    tree: axm.AxTree | None = None                 # the latest walk (serves snapshot, signature, secure scan)
    tree_at: float = 0.0
    tree_unread: bool = False                      # a snapshot's walk the image's password check may still use


class MacComputerDriver:
    os: Literal["macos", "windows", "linux"] = "macos"

    def __init__(self, device_id: str, real_input: RealInput, excluded: Callable[[], Collection[str]] = frozenset,
                 ax: Any = axm, apps: Any = appsm, shots: Any = shotm,
                 web_password: Callable[[str, tuple[float, float, float, float]], Awaitable[bool | None]] | None = None,
                 profiles: Profiles | None = None) -> None:
        self.device_id, self.input, self.excluded, self.ax, self.apps = device_id, real_input, excluded, ax, apps
        self.shots, self.web_password = shots, web_password     # web_password: the extension's check (9.4)
        self.profiles = profiles if profiles is not None else Profiles(None)   # what landed per app, learned
        self.on_activate: Callable[[str, str | None], None] = lambda bundle, task: None   # the trail's attribution
        self._state: dict[str, _AppState] = {}
        self.snapshot_images: Callable[[], bool] = lambda: True   # `[mac] snapshot_image` (owner request, 2026-10-01)
        self.communication: Callable[[str], bool] = lambda bundle: False  # messaging / calling apps (9.3)
        self.guard: Any = _NoGuard()                    # holds back the owner's input while FERB's app is forward
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
        ok = bool(await asyncio.to_thread(self.ax.press_menu, a.pid, path))
        self.profiles.record(a.bundle_id, "menu_press", ok)
        return ok

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

    # ---------- the one walk ----------

    async def _walk(self, a: Any, window: Any = None) -> axm.AxTree:
        """One Accessibility walk of the app's window, cached per app: the snapshot, the signature before / after an
        action and the password-field scan all read from it."""
        if window is None:
            tree = await asyncio.to_thread(self.ax.read_tree, a.pid, is_browser(a.bundle_id))
        else:
            tree = await asyncio.to_thread(self.ax.read_tree, a.pid, is_browser(a.bundle_id), window)
        st = self._state.setdefault(a.bundle_id, _AppState())
        st.tree, st.tree_at = tree, time.monotonic()
        return tree

    async def _signature(self, a: Any) -> tuple[Any, ...]:
        """What the window shows, to tell whether a background event did anything (snapshot ids untouched)."""
        return (await self._walk(a)).signature()

    async def snapshot(self, name_or_bundle: str, window: str | None = None) -> tuple[Any, dict[str, Any]]:
        a = self.app(name_or_bundle)
        await self._expose(a)
        win = None
        if window:
            win = await asyncio.to_thread(self.ax.window_named, a.pid, window)
            if win is None:
                listed = await asyncio.to_thread(self.ax.windows, a.pid)
                names = ", ".join(f"{w['title'] or '(untitled)'} [{w['id']}]" for w in listed) or "none"
                raise Refused(f"{a.name} has no window {window!r} that Accessibility can reach right now "
                              f"(its windows: {names}); a window on another Space is reachable only while it is "
                              "the app's focused or main window")
        tree = await self._walk(a, win)
        st = self._state.setdefault(a.bundle_id, _AppState())
        st.gen += 1
        st.tree_unread = True
        st.nodes = {f"s{st.gen}e{i + 1}": n for i, n in enumerate(tree.nodes)}
        els = []
        for eid, n in st.nodes.items():
            d: dict[str, Any] = {"id": eid, "role": n.role, "label": n.label}
            if n.actions:
                d["actions"] = list(n.actions)
            if n.secret:
                d["sensitive"], d["filled"] = True, n.value == "filled"
            elif n.value not in (None, ""):
                d["value"] = n.value
            if not n.enabled:
                d["disabled"] = True
            if n.selected:
                d["selected"] = True
            if n.expanded is not None:
                d["expanded"] = n.expanded
            if n.container:
                d["in"] = n.container
            els.append(d)
        out: dict[str, Any] = {"app": a.name, "bundle_id": a.bundle_id, "window": tree.title, "elements": els,
                               "text": tree.text}
        if tree.windows:
            out["windows"] = tree.windows
        if tree.headings:
            out["headings"] = tree.headings
        if tree.containers:
            out["open"] = sorted(set(tree.containers))      # a sheet / menu / dialog / popover is up: its
        if tree.truncated:                                  # controls are in `elements` with "in"
            out["truncated"] = tree.truncated_by or True
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

    # ---------- screenshots ----------

    async def _secure(self, a: Any, bounds: tuple[float, float, float, float], fresh: bool) -> bool | None:
        """Is there a password field in the captured window? From the walk just made for the snapshot when it's
        recent and of that window (one walk serves both), else a new walk."""
        st = self._state.get(a.bundle_id)
        if not fresh and st is not None and st.tree is not None and st.tree_unread \
                and time.monotonic() - st.tree_at <= TREE_FRESH_S:
            st.tree_unread = False
            f = await asyncio.to_thread(self.ax.frame, st.tree.window)
            if f is not None and _same(f, bounds):
                return st.tree.secure
        win = await asyncio.to_thread(self.ax.window_with_frame, a.pid, bounds)
        if win is None:
            return None
        tree = await self._walk(a, win)
        return tree.secure

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

        async def check(w: Any, fresh: bool) -> None:
            if await self._secure(a, w.bounds, fresh):
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
        await check(w, fresh=False)
        jpeg, width, height = await asyncio.to_thread(grab, w)
        await check(w, fresh=True)                            # appeared while capturing: the frame is dropped
        st = self._state.setdefault(a.bundle_id, _AppState())
        st.shots += 1
        st.shot = Shot(f"shot{st.shots}", w.id, w.bounds, (width, height),
                       signature=st.tree.signature() if st.tree else None)
        return a, st.shot, jpeg

    async def pointer_at(self, name: str, shot_id: str, x: float, y: float, kind: str,
                         end: tuple[float, float] | None = None, holder: str = "", *,
                         direction: str = "down", amount: int = 1, distance: float = 0.8) -> ActionResult:
        """Exact visual input to an app process. Never activate or take over the global mouse."""
        import math
        a = self.app(name)
        st = self._state.get(a.bundle_id)
        shot = st.shot if st else None
        if shot is None or shot.id != shot_id or time.monotonic() - shot.at > 30:
            raise StaleRef("take a fresh screenshot before pointer input")
        if kind not in {"click", "dblclick", "rightclick", "hover", "drag", "scroll"}:
            raise Refused("unsupported pointer action")
        if direction not in {"up", "down", "left", "right"} or not 1 <= amount <= 50:
            raise Refused("scroll needs a direction and amount between 1 and 50")
        if not math.isfinite(distance) or not 0.01 <= distance <= 1.0:
            raise Refused("scroll distance must be a viewport fraction from 0.01 to 1.0")
        route = load_route()
        if kind == "drag" and end is None:
            raise Refused("drag needs an end point")
        points = [(x, y), end or (x, y)]
        if any(not all(math.isfinite(v) for v in pt) or not (0 <= pt[0] < shot.size[0] and 0 <= pt[1] < shot.size[1]) for pt in points):
            raise Refused("pointer point outside the screenshot")
        bx, by, bw, bh = shot.bounds
        start, finish = [(bx + px * bw / shot.size[0], by + py * bh / shot.size[1]) for px, py in points]

        def window_ok() -> None:
            w = self.shots.window(shot.window)
            if w is None or w.pid != a.pid or not _same(w.bounds, shot.bounds):
                raise StaleRef("the window moved or closed; take another screenshot")
            if route is None and not self.shots.on_screen(shot.window):
                raise Refused("window-targeted backend unavailable; the legacy pointer route is limited to the current "
                              "Space on this Mac. No foreground switch was made")
            front = self.apps.frontmost()
            if self.input.owner_active() and front and front.pid == a.pid:
                raise Refused("the owner is using this app; pointer input stopped")

        window_ok()
        if shot.signature is None or await self._signature(a) != shot.signature:
            raise StaleRef("the app changed since the screenshot; take another screenshot")
        if kind == "scroll":
            return await self._pointer_scroll(a, st, shot, start, direction, amount, holder, route, window_ok, distance)
        steps, release = pointer_steps(start, finish, kind, a.pid, window=shot.window, route=route)
        attempted = False
        def guarded(step: Step) -> Step:
            def post() -> None:
                nonlocal attempted
                window_ok()
                attempted = True
                step.post()
            return Step(step.what, post)
        st.shot = None
        try:
            await self.input.to_process([guarded(step) for step in steps], holder, cleanup=release)
        except (Refused, StaleRef) as error:
            if attempted:
                raise PointerInterrupted(f"pointer input interrupted; outcome unknown: {error}") from error
            raise
        await asyncio.sleep(SETTLE_S)
        changed = await self._signature(a) != shot.signature
        if kind != "hover":
            self.profiles.record(a.bundle_id, "pid_pointer_" + kind, changed)
        return ActionResult(True, f"{kind} input delivered to the app process; visible state "
                            f"{'changed' if changed else 'not confirmed changed'}; verify with a fresh screenshot")

    async def click_at(self, name_or_bundle: str, shot_id: str, x: float, y: float, holder: str = "",
                       task: str | None = None, allow_foreground: bool = False) -> ActionResult:
        """A click at (x, y) in the app's latest screenshot (image px). First the app's own hit test turns the point
        into an element and presses it through Accessibility; then, for a window on the current Space, a click posted
        to the app's process, kept only if something changed; the real mouse (the app brought forward, yielding to
        the owner) only with allow_foreground."""
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

        # 1. the app's own hit test: the element under the point, pressed through Accessibility
        el = await asyncio.to_thread(self.ax.hit_element, a.pid, px, py)
        if el is not None:
            win_frame = await asyncio.to_thread(lambda: self.ax.frame(axm._attr(el, "AXWindow")))
            if win_frame is None or _same(win_frame, shot.bounds):
                self.profiles.record(a.bundle_id, "hit_test", True)
                if await asyncio.to_thread(self.ax.press, el):
                    self.profiles.record(a.bundle_id, "ax_press", True)
                    what = await asyncio.to_thread(self.ax.describe, el)
                    return ActionResult(True, f"pressed the {what} under ({x:.0f}, {y:.0f}) through Accessibility")
                pressed = await asyncio.to_thread(self.ax.press_ancestor, el)
                if pressed:
                    return ActionResult(True, f"pressed the {pressed} around ({x:.0f}, {y:.0f}) through Accessibility")
            else:
                self.profiles.record(a.bundle_id, "hit_test", False)
        # 2. a click posted to the app's process: only reaches a window on the current Space; kept if it changed
        #    something
        if await asyncio.to_thread(self.shots.on_screen, shot.window) and not self.profiles.skips(a.bundle_id, "pid_mouse"):
            before = await self._signature(a)
            await self.input.to_process(click_steps(px, py, a.pid), holder)
            await asyncio.sleep(SETTLE_S)
            landed = await self._signature(a) != before
            self.profiles.record(a.bundle_id, "pid_mouse", landed)
            if landed:
                return ActionResult(True, f"clicked ({x:.0f}, {y:.0f}) in {shot.id} (posted to the app in the background)")
        # 3. the real mouse, only by request

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
        r = await self._foreground(a, [guarded(s) for s in click_steps(px, py)], holder, "click", task,
                                   allow_foreground, f"no control under ({x:.0f}, {y:.0f}) could be pressed",
                                   prepare=lambda: self._bring_forward(a, shot))
        if r.ok:
            return ActionResult(True, f"clicked ({x:.0f}, {y:.0f}) in {shot.id}; {r.detail}")
        return r

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
        tree = await self._walk(a)
        return {"bundle_id": a.bundle_id, "text": tree.text,
                "elements": [{"role": n.role, "label": n.label, "value": None if n.secret else n.value}
                             for n in tree.nodes]}

    # ---------- actions ----------

    async def act(self, app: str, action: ComputerAction, holder: str = "", task: str | None = None) -> ActionResult:
        a = self.app(app)
        if is_browser(a.bundle_id):
            raise Refused(ACT_REFUSED)
        fg = bool(action.allow_foreground)
        if action.kind == "key":
            if not action.text or not valid_combo(action.text):
                raise Refused(f"unsupported key {action.text!r} (e.g. return, tab, escape, pagedown, cmd+s)")
            if switches_apps(action.text):
                raise Refused(f"{action.text} switches the owner's apps or desktops; FERB works with {a.name} where it "
                              "is — no need to bring it forward")
            if quits_or_hides(action.text):
                raise Refused(f"{action.text} quits, hides or minimises {a.name}: FERB never does that (close what "
                              "you opened with its close button instead)")
            await self._key_rules(a, action.text)
            return await self._key(a, action.text, holder, task, fg)
        if action.kind == "scroll":
            return await self._scroll(a, action, holder, task, fg)
        if action.element is None:
            raise Refused("this action needs an element id from a snapshot")
        n = self.node(a.bundle_id, action.element)
        if n.secret and action.kind in ("set_value", "type", "edit"):
            raise Refused("that's a password field; FERB never types passwords for the owner")
        if action.kind in ("press", "click"):
            return await self._press(a, n, holder, task, fg)
        if action.kind == "focus":
            ok = await asyncio.to_thread(self.ax.focus, n.ref)
            self.profiles.record(a.bundle_id, "ax_focus", ok)
            return ActionResult(ok, "focused" if ok else "the element refused the focus")
        if action.kind == "set_value":
            return await self._set_value(a, n, action.text or "", holder, task, fg)
        if action.kind == "type":
            return await self._type(a, n, action.text or "", holder, task, fg)
        if action.kind == "edit":
            return await self._edit(a, n, action.select, action.text)
        if action.kind == "select":
            return await self._select(a, n, action.text or "")
        raise Refused(f"unknown action {action.kind}")

    async def _foreground(self, a: Any, steps: list[Any], holder: str, what: str, task: str | None, allow: bool,
                          why: str, prepare: Callable[[], Awaitable[None]] | None = None) -> ActionResult:
        """The last rung: only when the call allows the owner's screen to move; otherwise say what was tried."""
        if not allow:
            return ActionResult(False, f"{why}; {FOREGROUND_HINT}")
        return await self._real(a, steps, holder, what, task, prepare)

    # ---- press

    async def _press(self, a: Any, n: axm.AxNode, holder: str, task: str | None, fg: bool) -> ActionResult:
        if await asyncio.to_thread(self.ax.press, n.ref):
            self.profiles.record(a.bundle_id, "ax_press", True)
            return ActionResult(True, "pressed")
        self.profiles.record(a.bundle_id, "ax_press", False)
        pressed = await asyncio.to_thread(self.ax.press_ancestor, n.ref)
        if pressed:
            self.profiles.record(a.bundle_id, "ax_press", True)
            return ActionResult(True, f"pressed (its enclosing {pressed} took the press)")
        if not self.profiles.skips(a.bundle_id, "pid_keycodes"):      # keyboard: focus it, Return / Space
            before = await self._signature(a)
            if await asyncio.to_thread(self.ax.focus, n.ref):
                key = "space" if n.role in ("checkbox", "radio") else "return"
                await self.input.to_process(key_steps(key, a.pid), holder)
                await asyncio.sleep(SETTLE_S)
                landed = await self._signature(a) != before
                self.profiles.record(a.bundle_id, "pid_keycodes", landed)
                if landed:
                    return ActionResult(True, f"pressed (focused, then {key} in the background)")
        return await self._foreground(a, click_steps(*self.ax.center(n)), holder, "click", task, fg,
                                      "it ignores Accessibility presses and the keyboard in the background")

    # ---- typing

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
        from ferb.policy.effects import label_kind
        from ferb.policy.rules import Kind
        st = self._state.get(bundle_id)
        for eid, n in (st.nodes.items() if st else ()):
            if n.role == "button" and n.enabled and label_kind(n.label) == Kind.SEND:
                return eid
        return None

    async def _value_changed(self, n: axm.AxNode, before: str | None) -> tuple[bool, str | None]:
        """Poll the element's value until it changes or SETTLE_S passes (cheap: one attribute read per poll)."""
        deadline = time.monotonic() + SETTLE_S
        while True:
            after = await asyncio.to_thread(self.ax.value, n.ref)
            if after != before:
                return True, after
            if time.monotonic() >= deadline:
                return False, after
            await asyncio.sleep(0.05)

    @staticmethod
    def _text_field(n: axm.AxNode) -> bool:
        return n.ax_role in {"AXTextField", "AXTextArea", "AXComboBox"} if n.ax_role else n.role in TEXT_KINDS

    async def _set_value(self, a: Any, n: axm.AxNode, text: str, holder: str, task: str | None,
                         fg: bool) -> ActionResult:
        # only a real text field takes a value that the app then acts on (WhatsApp's search "button" accepts a
        # set value but never searches on it — Sept 30 trail); anything else is focused / pressed and typed into
        if self._text_field(n):
            ok = await asyncio.to_thread(self.ax.set_value, n.ref, text)
            ok = ok and await asyncio.to_thread(self.ax.value, n.ref) == text
            self.profiles.record(a.bundle_id, "ax_value", ok)
            if ok:
                return ActionResult(True, "value set")
        return await self._type(a, n, text, holder, task, fg)

    async def _type(self, a: Any, n: axm.AxNode, text: str, holder: str, task: str | None, fg: bool) -> ActionResult:
        """Keystrokes go to the app's keyboard focus: typed only once that is the box FERB means (a Save sheet or a
        dialog can hold it — then nothing is typed, here or in the foreground)."""
        if not self._text_field(n):
            await asyncio.to_thread(self.ax.press, n.ref)
        await asyncio.to_thread(self.ax.focus, n.ref)
        focused = await asyncio.to_thread(self.ax.has_focus, a.pid, n.ref)
        if focused is False:          # a different control or dialog holds the keyboard
            raise Refused(FOCUS_ELSEWHERE)
        before = await asyncio.to_thread(self.ax.value, n.ref)
        if focused and not self.profiles.skips(a.bundle_id, "pid_keys"):
            sig = await self._signature(a) if before is None else None
            await self.input.to_process(type_steps(text, a.pid), holder)
            if before is not None:
                landed, _ = await self._value_changed(n, before)
            else:
                await asyncio.sleep(SETTLE_S)
                landed = await self._signature(a) != sig
            self.profiles.record(a.bundle_id, "pid_keys", landed)
            if landed:
                return ActionResult(True, "typed (in the background)")
        if not self.profiles.skips(a.bundle_id, "ax_keyboard"):
            landed = await self._ax_type(a, n, text)
            self.profiles.record(a.bundle_id, "ax_keyboard", landed)
            if landed:
                return ActionResult(True, "typed (in the background, Accessibility keyboard)")
        if self._text_field(n):                                      # replace the whole value, the edit applied
            new = (before or "") + text
            ok = await asyncio.to_thread(self.ax.set_value, n.ref, new)
            ok = ok and await asyncio.to_thread(self.ax.value, n.ref) == new
            self.profiles.record(a.bundle_id, "ax_value", ok)
            if ok:
                return ActionResult(True, "typed by setting the box's value (Accessibility)")
        if not self._text_field(n):
            # Not a real text field (WhatsApp's search box reports itself as an unlabeled button and only searches on
            # real typing — Sept 30 trail): click it in front, then type, as a person would.
            return await self._foreground(a, click_steps(*self.ax.center(n)) + type_steps(text), holder,
                                          "click + typing", task, fg, "it takes no typing in the background")

        async def focus_it() -> None:                          # in front now: focus the box, and check it took
            await asyncio.to_thread(self.ax.focus, n.ref)
            if await asyncio.to_thread(self.ax.has_focus, a.pid, n.ref) is False:
                raise Refused(FOCUS_ELSEWHERE)
        return await self._foreground(a, type_steps(text), holder, "typing", task, fg,
                                      "it takes no typing in the background", prepare=focus_it)

    async def _ax_type(self, a: Any, n: axm.AxNode, text: str) -> bool:
        """Spike 20: press (a box that isn't a text field) or focus it through Accessibility, then type through the
        Accessibility keyboard API — still in the background. True once the window shows the change."""
        if self._text_field(n):
            await asyncio.to_thread(self.ax.focus, n.ref)
        else:
            await asyncio.to_thread(self.ax.press, n.ref)                # WhatsApp's search "button" focuses its box
        await asyncio.sleep(0.3)
        if await asyncio.to_thread(self.ax.has_focus, a.pid, n.ref) is not True:
            return False
        before = await self._signature(a)              # focus changes are not evidence of text arriving
        if not await asyncio.to_thread(self.ax.post_text, a.pid, text):
            return False
        await asyncio.sleep(SETTLE_S + 0.6)                              # the app filters / updates
        return await self._signature(a) != before

    # ---- editing text in place

    async def _edit(self, a: Any, n: axm.AxNode, select: Any, text: str | None) -> ActionResult:
        """Insert or replace text at an exact range through AXSelectedTextRange + AXSelectedText (verified on this Mac
        in the background), else rewrite the whole value with the edit applied. Never arrow keys."""
        value = await asyncio.to_thread(self.ax.value, n.ref)
        length = len(value or "")
        if select in (None, "", "caret"):
            rng = await asyncio.to_thread(self.ax.selected_range, n.ref) or (length, 0)
        elif select == "all":
            rng = (0, length)
        elif select == "end":
            rng = (length, 0)
        elif select == "start":
            rng = (0, 0)
        elif isinstance(select, (list, tuple)) and len(select) == 2:
            rng = (max(0, int(select[0])), max(0, int(select[1])))
        else:
            raise Refused(f"select must be (start, length), \"all\", \"end\" or \"start\", not {select!r}")
        start, count = min(rng[0], length), min(rng[1], max(0, length - min(rng[0], length)))
        ok = await asyncio.to_thread(self.ax.set_selected_range, n.ref, start, count)
        if ok and text is None:
            got = await asyncio.to_thread(self.ax.selected_range, n.ref)
            self.profiles.record(a.bundle_id, "ax_selected_text", got == (start, count))
            if got == (start, count):
                return ActionResult(True, f"selected {count} characters from {start}")
            return ActionResult(False, f"the selection didn't take (the box reports {got})")
        if text is None:
            return ActionResult(False, "that box doesn't let its selection be set")
        expected = (value or "")[:start] + text + (value or "")[start + count:]
        if ok and await asyncio.to_thread(self.ax.set_selected_text, n.ref, text):
            landed, after = await self._value_changed(n, value)
            if after == expected or (value is None and landed):
                self.profiles.record(a.bundle_id, "ax_selected_text", True)
                return ActionResult(True, f"edited: {count} characters at {start} replaced with the text "
                                          f"({len(text)} characters)" if count else
                                          f"inserted {len(text)} characters at {start}")
        self.profiles.record(a.bundle_id, "ax_selected_text", False)
        if value is not None and await asyncio.to_thread(self.ax.set_value, n.ref, expected):
            if await asyncio.to_thread(self.ax.value, n.ref) == expected:
                self.profiles.record(a.bundle_id, "ax_value", True)
                return ActionResult(True, "edited by rewriting the box's whole value (the app's undo sees one change)")
        return ActionResult(False, "that box takes neither a selection edit nor a new value through Accessibility; "
                                   "set_value or type may still work")

    # ---- keys

    async def _key(self, a: Any, combo: str, holder: str, task: str | None, fg: bool) -> ActionResult:
        parts = normalize(combo)
        key, mods = parts[-1], parts[:-1]
        if is_shortcut(combo):                                   # a key equivalent: the menu item it maps to
            found = await asyncio.to_thread(self.ax.menu_item_for_shortcut, a.pid, key, mods)
            if found is not None:
                item, path, enabled = found
                if not enabled:
                    raise Refused(f"{combo} is the menu item “{path}”, which {a.name} reports disabled right now (an "
                                  f"app in the background often has no key window, so the key would only beep). "
                                  f"Instead: {_alternative(key, mods)}")
                if await asyncio.to_thread(self.ax.press, item):
                    self.profiles.record(a.bundle_id, "menu_press", True)
                    return ActionResult(True, f"{combo}: pressed its menu item “{path}” (Accessibility, in the "
                                              "background)")
        if key == "escape":                                      # dismiss what's open, never a blind Escape
            win = await asyncio.to_thread(self.ax.app_window, a.pid)
            panels = await asyncio.to_thread(self.ax.open_panels, a.pid, win)
            panels += [("menu", m) for m in await asyncio.to_thread(self.ax.open_menus, a.pid)]
            for kind, el in panels:
                if await asyncio.to_thread(self.ax.cancel, el):
                    return ActionResult(True, f"escape: dismissed the {kind} (AXCancel)")
            if panels:
                return ActionResult(False, f"a {panels[0][0]} is open but didn't take AXCancel: press its Cancel / "
                                           "Close button (take a snapshot: its buttons are listed with \"in\")")
        changed = False
        if not self.profiles.skips(a.bundle_id, "pid_keycodes"):
            before = await self._signature(a)
            await self.input.to_process(key_steps(combo, a.pid), holder)
            await asyncio.sleep(SETTLE_S)
            changed = await self._signature(a) != before
            if changed:
                self.profiles.record(a.bundle_id, "pid_keycodes", True)
                return ActionResult(True, f"{combo} (in the background)")
            if self.profiles.prefers(a.bundle_id, "pid_keycodes"):      # the app takes them; this key showed nothing
                return ActionResult(True, f"{combo} sent in the background; nothing visibly changed")
            # A key with nothing to act on (escape with no dialog, cmd+s on a saved document) changes nothing and
            # proves nothing: it never marks the app as ignoring background input (audit 2026-10-02).
        if not self.profiles.skips(a.bundle_id, "ax_keyboard"):
            before = await self._signature(a)
            if await asyncio.to_thread(self.ax.post_key, a.pid, KEYCODES[key], mods):
                await asyncio.sleep(SETTLE_S + 0.4)
                if await self._signature(a) != before:
                    self.profiles.record(a.bundle_id, "ax_keyboard", True)
                    return ActionResult(True, f"{combo} (in the background, Accessibility keyboard)")
        if fg:
            return await self._real(a, key_steps(combo), holder, combo, task)
        return ActionResult(True, f"{combo} sent to {a.name} in the background; nothing visibly changed (take a "
                                  "snapshot to check)")

    # ---- scrolling

    async def _scroll(self, a: Any, action: ComputerAction, holder: str, task: str | None, fg: bool) -> ActionResult:
        direction = (action.direction or "down").lower()
        if direction not in ("up", "down", "left", "right"):
            raise Refused("scroll direction must be up, down, left or right")
        amount = max(1, min(50, int(action.amount or 1)))
        n = self.node(a.bundle_id, action.element) if action.element else None
        # AXScrollToVisible reveals a row; it says nothing about directional scrolling.
        if n is not None and n.ax_role in {"AXSlider", "AXStepper"}:
            act = "AXIncrement" if direction in ("up", "right") else "AXDecrement"
            done = 0
            for _ in range(amount):
                if not await asyncio.to_thread(self.ax.perform, n.ref, act):
                    break
                done += 1
            return ActionResult(bool(done), f"slider: {done} step(s); value now {self.ax.value(n.ref)}")
        if n is not None:
            area = await asyncio.to_thread(self.ax.scroll_area_of, n.ref)
        else:
            win = await asyncio.to_thread(self.ax.window_named, a.pid, action.window) if action.window else \
                await asyncio.to_thread(self.ax.app_window, a.pid)
            area = await asyncio.to_thread(self.ax.scroll_area_in, win) if win is not None else None
        if area is None:
            return ActionResult(False, "no unique scroll container: choose an element inside the intended pane, "
                                "or use mac.pointer action=scroll at a fresh screenshot point")
        content = await asyncio.to_thread(self.ax.scroll_content, area)
        vertical = direction in ("up", "down")
        bar = await asyncio.to_thread(self.ax.scroll_bar, area, vertical)
        # Page keys are only safe for an explicitly targeted text view, never a chat row/list selection.
        text_view = n is not None and n.ax_role == "AXTextArea" and vertical
        moved, mechanism, stop = 0, "", "batch limit reached"
        before = await asyncio.to_thread(self.ax.scroll_position, area, content)
        for _ in range(amount):
            front = self.apps.frontmost()
            if self.input.owner_active() and front and front.pid == a.pid:
                stop = "owner is using this app"
                break
            if self._scroll_boundary(before, direction):
                stop = "scroll bar boundary reached; verify content before claiming the last item"
                break
            if bar is not None:
                act = "AXIncrement" if direction in ("down", "right") else "AXDecrement"
                if not await asyncio.to_thread(self.ax.perform, bar, act):
                    cur = await asyncio.to_thread(self.ax.value, bar)
                    if cur is not None:
                        await asyncio.to_thread(self.ax.set_scroll_bar, bar,
                                                float(cur) + (0.2 if act == "AXIncrement" else -0.2))
                mechanism = "scroll bar"
            elif text_view and not self.profiles.skips(a.bundle_id, "pid_keycodes"):
                if not await asyncio.to_thread(self.ax.focus, n.ref):
                    stop = "text view could not take focus"
                    break
                await self.input.to_process(key_steps("pageup" if direction == "up" else "pagedown", a.pid), holder)
                mechanism = "Page Up" if direction == "up" else "Page Down"
            else:
                stop = "no directional AX route; use mac.pointer action=scroll with a fresh screenshot"
                break
            await asyncio.sleep(SETTLE_S)
            after = await asyncio.to_thread(self.ax.scroll_position, area, content)
            if after == before:
                stop = "movement not confirmed; this does not establish the end"
                break
            moved += 1
            before = after
        detail = f"scrolled {direction}: {moved}/{amount} step(s) via {mechanism or 'AX'}; {stop}"
        if moved or "boundary reached" in stop:
            return ActionResult(True, detail)
        if fg:
            rect = self.ax.frame(area)
            if rect:
                x, y, w, h = rect
                return await self._foreground(a, wheel_steps(x + w / 2, y + h / 2,
                    -10 if direction in ("down", "right") else 10, horizontal=not vertical),
                    holder, "scroll", task, True, detail)
        return ActionResult(False, detail + "; use pointer scroll or explicitly allow_foreground=true")

    @staticmethod
    def _scroll_boundary(position: tuple[Any, ...], direction: str) -> bool:
        value = position[0 if direction in ("up", "down") else 1]
        return value is not None and (value == 1.0 if direction in ("down", "right") else value == 0.0)

    async def _pointer_scroll(self, a: Any, st: _AppState, shot: Shot, point: tuple[float, float],
                              direction: str, amount: int, holder: str, route: Any,
                              guard: Callable[[], None], distance: float = 0.8) -> ActionResult:
        # Resolve only a container under this captured point, never the first pane of the window.
        area = None
        if st.tree:
            for node in sorted(st.tree.nodes, key=lambda n: n.rect[2] * n.rect[3]):
                x, y, w, h = node.rect
                if x <= point[0] < x + w and y <= point[1] < y + h:
                    area = await asyncio.to_thread(self.ax.scroll_area_of, node.ref)
                    if area is not None:
                        break
        content = await asyncio.to_thread(self.ax.scroll_content, area) if area is not None else None
        async def observe() -> tuple[Any, ...]:
            return await asyncio.to_thread(self.ax.scroll_position, area, content) if area is not None else ()
        before = await observe()
        st.shot = None
        moved, sent, stop = 0, 0, "batch limit reached"
        viewport = self.ax.frame(area) if area is not None else shot.bounds
        # A viewport-sized step, not a tiny line notch; keep overlap for visual continuity.
        span = viewport[3 if direction in ("up", "down") else 2] if viewport else shot.bounds[3]
        pixels = max(1, min(800, round(span * distance)))
        lines = -pixels if direction in ("down", "right") else pixels
        for _ in range(amount):
            if before and self._scroll_boundary(before, direction):
                stop = "scroll bar boundary reached; verify content"
                break
            steps = wheel_steps(*point, lines, a.pid, horizontal=direction in ("left", "right"),
                                window=shot.window, route=route, pixels=True)
            def guarded(step: Step) -> Step:
                def post() -> None:
                    guard()
                    step.post()
                return Step(step.what, post)
            try:
                await self.input.to_process([guarded(step) for step in steps], holder)
            except (Refused, StaleRef) as error:
                if sent:
                    raise PointerInterrupted(f"scroll interrupted after {sent} ticks; inspect the result: {error}") from error
                raise
            sent += 1
            await asyncio.sleep(SETTLE_S)
            after = await observe()
            if not after or after == before:
                stop = "movement not confirmed; take a fresh screenshot; end not established"
                break
            moved += 1
            before = after
        return ActionResult(bool(moved) or "boundary reached" in stop,
                            f"scroll {direction} ({pixels}px/step): sent {sent}/{amount} viewport-sized steps, {moved} observed changes; {stop}")

    # ---- pop-up buttons

    async def _select(self, a: Any, n: axm.AxNode, title: str) -> ActionResult:
        """A pop-up button's item by title: open the menu with AXPress, press the AXMenuItem; AXValue where settable."""
        want = title.strip().lower()
        if not want:
            raise Refused("select needs the item's title in text")
        if await asyncio.to_thread(self.ax.press, n.ref):
            menu, items = None, []
            for _ in range(8):
                await asyncio.sleep(0.2)
                menu = await asyncio.to_thread(self.ax.popup_menu, n.ref, a.pid)
                if menu is not None:
                    items = await asyncio.to_thread(self.ax.menu_items, menu)
                    if items:
                        break
            hit = next((el for el, t in items if t.lower() == want), None) or \
                next((el for el, t in items if t.lower().startswith(want)), None)
            if hit is not None and await asyncio.to_thread(self.ax.press, hit):
                self.profiles.record(a.bundle_id, "ax_press", True)
                await asyncio.sleep(0.3)
                now = await asyncio.to_thread(self.ax.value, n.ref)
                return ActionResult(True, f"selected “{title}”" + (f" (now shows {now!r})" if now else ""))
            if menu is not None:
                await asyncio.to_thread(self.ax.cancel, menu)
            if items:
                return ActionResult(False, f"no item {title!r} in that menu; it has: " +
                                           ", ".join(t for _, t in items[:40]))
        if await asyncio.to_thread(self.ax.settable, n.ref, "AXValue") and \
                await asyncio.to_thread(self.ax.set_value, n.ref, title):
            return ActionResult(True, f"set to {title!r} (AXValue)")
        return ActionResult(False, "that control opened no menu through Accessibility and takes no value")

    # ---------- the foreground, last resort, only by request ----------

    async def _bring_forward(self, a: Any, shot: Shot) -> None:
        """After activating the app: raise the captured window itself and wait (~3 s) for it to be on screen — the
        app may switch to another Space or a full-screen window first."""
        for _ in range(12):
            await asyncio.to_thread(self.ax.raise_window, a.pid, shot.bounds)
            if await asyncio.to_thread(self.shots.on_screen, shot.window):
                await asyncio.sleep(0.2)
                return
            await asyncio.sleep(0.25)

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


def _alternative(key: str, mods: list[str]) -> str:
    """What does the same thing through Accessibility attributes when a key equivalent's menu item is disabled."""
    if key in ("a", "c", "x", "v") and mods == ["cmd"]:
        return ("action edit on the text box (select \"all\" / \"end\" / (start, length), with text to replace or "
                "insert) works through AXSelectedTextRange and AXSelectedText without the key")
    if key == "z":
        return "there is no background undo; set_value or edit the text back instead"
    return ("press the control or the menu item (mac_menu) once a snapshot shows it enabled, or use edit / set_value "
            "on the element directly")


def _same(a: tuple[float, ...], b: tuple[float, ...]) -> bool:
    return all(abs(p - q) <= 1 for p, q in zip(a, b))


def profiles_dir(home: Path) -> Path:
    return home / "device" / "apps"
