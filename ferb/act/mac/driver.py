"""Own native target references and explicit AX/process/foreground input mechanics.
Agents choose methods and retries; this driver never substitutes targets, replays uncertain input or restores focus.
"""

import asyncio
import contextlib
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Awaitable, Callable, Collection, ContextManager, Literal

from ferb.act.mac import accessibility as axm
from ferb.act.mac import apps as appsm
from ferb.act.mac import screenshot as shotm
from ferb.act.mac.ax_browser import BROWSER_BUNDLES, is_browser
from ferb.act.mac.window_input import load_route
from ferb.act.mac.input import (KEYCODES, PointerInterrupted, RealInput, Step, click_steps, key_steps,
                                normalize, pointer_steps, type_steps, valid_combo, wheel_steps)
from ferb.act.mac.profile import Profiles
from ferb.core.errors import NoTarget, Refused, StaleRef
from ferb.core.types import ActionResult, ComputerAction

UNTRUSTED_APPS = {"com.apple.mail": "email", "com.apple.MobileSMS": "message", "net.whatsapp.WhatsApp": "message",
                  "com.tinyspeck.slackmacgap": "message", "com.hnc.Discord": "message",
                  "ru.keepcoder.Telegram": "message", "com.microsoft.teams2": "message"}


PASSWORD_REFUSED = "that window shows a password field; FERB doesn't capture it (the frame was dropped)"


SETTLE_S = 0.4                # after background events, before reading what changed (one read, from the walker)
TREE_FRESH_S = 1.0            # a walk this recent serves a second reader (the snapshot's image check)
FOCUS_ELSEWHERE = ("something else in the app has the keyboard focus (a dialog or sheet may be open), so nothing was "
                   "typed; take a new snapshot")



@dataclass(frozen=True)
class Shot:
    id: str
    window: int                                    # CGWindowID
    bounds: tuple[float, float, float, float]      # window, screen points
    size: tuple[int, int]                          # image px
    pid: int = 0                                  # process observed with this window, not a later app instance
    at: float = field(default_factory=time.monotonic)
    signature: tuple[Any, ...] | None = None


@dataclass
class _AppState:
    pid: int = 0
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
        self.shots, self.web_password = shots, web_password     # web_password: the extension's check
        self.profiles = profiles if profiles is not None else Profiles(None)   # what landed per app, learned
        self.on_activate: Callable[[str, str | None], None] = lambda bundle, task: None   # the trail's attribution
        # The trail's attribution (P4): while a call that changes an app runs, events in that app are FERB's.
        self.acting: Callable[[str, str | None], ContextManager[None]] = lambda bundle, task: contextlib.nullcontext()
        self._state: dict[str, _AppState] = {}
        self.snapshot_images: Callable[[], bool] = lambda: True   # `[mac] snapshot_image` (owner request, 2026-10-01)
        self._exposed: set[int] = set()                 # app processes asked for their full tree (Electron)

    def app(self, name_or_bundle: str) -> Any:
        a = self.apps.find(name_or_bundle)
        if a is None:
            candidates = self.apps.candidates(name_or_bundle) if hasattr(self.apps, "candidates") else []
            if candidates:
                raise Refused("ambiguous app identity; choose pid from mac.apps: " +
                              "; ".join(f"{c.name} {c.bundle_id} pid:{c.pid}" for c in candidates))
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
        """Observe launch readiness only. Opening an app's menu/window is a separate agent choice."""
        deadline = time.monotonic() + wait_s
        while True:
            a = self.apps.find(bundle_id)
            if a is not None:
                await self._expose(a)
                if await asyncio.to_thread(self.ax.app_window, a.pid) is not None:
                    return True
            if time.monotonic() >= deadline:
                return False
            await asyncio.sleep(0.25)

    # ---------- the one walk ----------

    async def _walk(self, a: Any, window: Any = None, offset: int = 0) -> axm.AxTree:
        """One Accessibility walk of the app's window, cached per app: the snapshot, the signature before / after an
        action and the password-field scan all read from it."""
        if offset:
            tree = await asyncio.to_thread(self.ax.read_tree, a.pid, is_browser(a.bundle_id), window, offset)
        elif window is None:
            tree = await asyncio.to_thread(self.ax.read_tree, a.pid, is_browser(a.bundle_id))
        else:
            tree = await asyncio.to_thread(self.ax.read_tree, a.pid, is_browser(a.bundle_id), window)
        st = self._state.setdefault(a.bundle_id, _AppState())
        if st.pid and st.pid != a.pid:
            st = _AppState(gen=st.gen)
            self._state[a.bundle_id] = st
        st.pid = a.pid
        st.tree, st.tree_at = tree, time.monotonic()
        return tree

    async def _signature(self, a: Any) -> tuple[Any, ...]:
        """What the window shows, to tell whether a background event did anything (snapshot ids untouched)."""
        return (await self._walk(a)).signature()

    async def snapshot(self, name_or_bundle: str, window: str | None = None, subtree: str | None = None, offset: int = 0) -> tuple[Any, dict[str, Any]]:
        a = self.app(name_or_bundle)
        await self._expose(a)
        win = None
        subtree_node = None
        if window:
            win = await asyncio.to_thread(self.ax.window_named, a.pid, window)
            if win is None:
                listed = await asyncio.to_thread(self.ax.windows, a.pid)
                names = ", ".join(f"{w['title'] or '(untitled)'} [{w['id']}]" for w in listed) or "none"
                raise Refused(f"{a.name} has no window {window!r} that Accessibility can reach right now "
                              f"(its windows: {names}); no window was substituted or brought forward")
        if subtree:
            st = self._state.get(a.bundle_id)
            if st and st.pid != a.pid:
                raise StaleRef("element process changed; take a new snapshot")
            if window:
                raise Refused("choose window or subtree, not both")
            subtree_node = self.node(a.bundle_id, subtree)
            win = subtree_node.ref
        tree = await self._walk(a, win, offset)
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
                               "text": tree.text, "offset": offset, "next_offset": tree.next_offset,
                               "profile": self.profiles.as_dict(a.bundle_id), "repeated_ax_nodes": tree.repeated_nodes}
        if tree.next_offset is not None:
            continuation: dict[str, Any] = {"offset": tree.next_offset}
            if subtree_node is not None:
                root_ref = f"s{st.gen}e0"
                st.nodes[root_ref] = subtree_node
                continuation["subtree"] = root_ref
            elif window:
                continuation["window"] = window
            out["continuation"] = continuation
        if tree.windows:
            out["windows"] = tree.windows
        if tree.headings:
            out["headings"] = tree.headings
        if tree.containers:
            out["open"] = sorted(set(tree.containers))      # a sheet / menu / dialog / popover is up: its
        if tree.truncated:                                  # controls are in `elements` with "in"
            out["truncated"] = tree.truncated_by or True
            out["retrieval"] = "mac.snapshot with returned continuation arguments continues this exact page/subtree; subtree=<element id> retrieves below the depth limit; full text values are not clipped"
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
                return None if 'depth' in st.tree.truncated_by else st.tree.secure
        win = await asyncio.to_thread(self.ax.window_with_frame, a.pid, bounds)
        if win is None:
            return None
        tree = await self._walk(a, win)
        return None if 'depth' in tree.truncated_by else tree.secure

    async def screenshot(self, name_or_bundle: str,
                         marks: list[tuple[str, tuple[float, float, float, float]]] | None = None,
                         window: str | None = None, native_resolution: bool = False
                         ) -> tuple[Any, Shot, bytes]:
        """The app's front window only, in memory. A password field the Accessibility tree shows (or, for a browser,
        the extension reports in the tab on screen), before or after capture, is masked only with complete geometry.
        Unknown inventory withholds the frame. The capture becomes the app's latest shot (mac.click / pointer / record)."""
        a, w, jpeg, size = await self._capture(name_or_bundle, marks, window, native_resolution)
        st = self._state.setdefault(a.bundle_id, _AppState())
        st.shots += 1
        st.shot = Shot(f"shot{st.shots}", w.id, w.bounds, size, pid=a.pid,
                       signature=st.tree.signature() if st.tree else None)
        return a, st.shot, jpeg

    async def window_image(self, name_or_bundle: str) -> tuple[Any, Shot, bytes]:
        """FERB's own picture of the app's front window for the evidence store (D26, D29): the same capture and
        password-field rule as `screenshot`, but it never becomes the app's latest shot, so a worker's pending
        mac.click / pointer / record keeps its own."""
        a, w, jpeg, size = await self._capture(name_or_bundle, None, None, False)
        return a, Shot("evidence", w.id, w.bounds, size, pid=a.pid), jpeg

    def secret_fields(self, bundle_id: str) -> list[tuple[float, float, float, float]]:
        """Screen-point boxes of the password fields in the latest walk of this app (after a capture: of the captured
        window), for masking a stored picture."""
        st = self._state.get(bundle_id)
        if st is None or st.tree is None:
            return []
        if st.tree.secret_rects is not None:
            return list(st.tree.secret_rects)
        return [n.rect for n in st.tree.nodes if n.secret]

    async def _capture(self, name_or_bundle: str, marks: list[tuple[str, tuple[float, float, float, float]]] | None,
                       window: str | None, native_resolution: bool) -> tuple[Any, Any, bytes, tuple[int, int]]:
        a = self.app(name_or_bundle)
        browser = BROWSER_BUNDLES.get(a.bundle_id)

        def find() -> Any:
            if window and window.isdigit():
                w = self.shots.window(int(window))
                if w is not None and w.pid != a.pid:
                    w = None
            else:
                axwin = self.ax.window_named(a.pid, window) if window else self.ax.app_window(a.pid)
                if window and axwin is None:
                    raise Refused("no unique reachable window matches that exact title; choose a window id")
                w = self.shots.front_window(a.pid, self.ax.frame(axwin))
            if w is None:
                raise NoTarget(f"{a.name} has no unique capture target; choose an exact window id from mac.snapshot")
            return w

        secret_rects: list[tuple[float, float, float, float]] = []
        async def check(w: Any, fresh: bool) -> None:
            secure = await self._secure(a, w.bounds, fresh)
            boxes = self.secret_fields(a.bundle_id)
            if secure is None or (secure and (not boxes or any(w <= 0 or h <= 0 for _, _, w, h in boxes))):
                raise Refused("the window's secret-field inventory is incomplete; pixels withheld")
            secret_rects.extend(boxes)
            if browser and self.web_password is not None and await self.web_password(browser, w.bounds):
                raise Refused(PASSWORD_REFUSED)

        def grab(w: Any) -> tuple[bytes, int, int]:
            try:
                if native_resolution:
                    return self.shots.capture(w, marks=marks, max_edge=None)
                if marks:
                    return self.shots.capture(w, marks=marks)     # type: ignore[no-any-return]
                return self.shots.capture(w)                  # type: ignore[no-any-return]
            except LookupError as e:                          # the window went away: nothing was captured
                raise NoTarget(f"couldn't capture {a.name}'s window: {e}") from None
            except (RuntimeError, TimeoutError) as e:
                raise Refused(f"couldn't capture {a.name}'s window: {e}") from None
        w = await asyncio.to_thread(find)
        await check(w, fresh=False)
        jpeg, width, height = await asyncio.to_thread(grab, w)
        await check(w, fresh=True)                            # appeared while capturing: the frame is dropped
        if secret_rects:
            from ferb.act.evidence.mask import mask_secret_fields, to_image_boxes
            jpeg = await asyncio.to_thread(mask_secret_fields, jpeg, to_image_boxes(secret_rects, w.bounds, (width, height)))
        return a, w, jpeg, (width, height)

    def recording_source(self, name: str, shot_id: str) -> tuple[dict[str, Any], Any]:
        """Pin the window the worker actually saw; repeated captures never replace its action references."""
        a = self.app(name)
        st = self._state.get(a.bundle_id)
        shot = st.shot if st else None
        if shot is None or shot.id != shot_id or shot.pid != a.pid or time.monotonic() - shot.at > 30:
            raise Refused('take a fresh mac_screenshot to select the recording window')
        browser = BROWSER_BUNDLES.get(a.bundle_id)

        async def current() -> Any:
            w = await asyncio.to_thread(self.shots.window, shot.window)
            if w is None or w.pid != a.pid or self.app(a.bundle_id).pid != a.pid:
                raise Refused('recording window closed or changed owner')
            secure = await self._secure(a, w.bounds, fresh=True)
            if secure is None:
                raise Refused("the window's secret-field inventory is incomplete; recording pixels withheld")
            if secure:
                raise Refused(PASSWORD_REFUSED)
            if browser and self.web_password and await self.web_password(browser, w.bounds):
                raise Refused(PASSWORD_REFUSED)
            return w

        async def capture() -> bytes:
            w = await current()
            image, _, _ = await asyncio.to_thread(self.shots.capture, w)
            await current()
            return image
        return {'kind': 'window', 'app': a.bundle_id, 'pid': a.pid, 'window_id': shot.window}, capture

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
        steps, release = pointer_steps(start, finish, kind, a.pid, window=shot.window, route=route,
                                       window_origin=shot.bounds[:2])
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
                            f"{'changed' if changed else 'not confirmed changed'}; verify with a fresh screenshot",
                            {'method': 'window_pointer' if route else 'pid_pointer', 'delivered': True,
                             'observed_change': changed, 'outcome_verified': False, 'replayed': False})

    async def click_at(self, name_or_bundle: str, shot_id: str, x: float, y: float, holder: str = "",
                       task: str | None = None, allow_foreground: bool = False, mechanism: str = "pointer") -> ActionResult:
        """Use one explicit pointer, AX or foreground method for the exact captured window."""
        a = self.app(name_or_bundle)
        st = self._state.get(a.bundle_id)
        shot = st.shot if st else None
        if shot is None or shot.id != shot_id:
            raise StaleRef(f"{shot_id} isn't {a.name}'s latest screenshot; take a new one")
        if not (0 <= x < shot.size[0] and 0 <= y < shot.size[1]):
            raise Refused(f"({x}, {y}) is outside the {shot.size[0]}x{shot.size[1]} screenshot")
        now = await asyncio.to_thread(self.shots.window, shot.window)
        if now is None or now.pid != a.pid or not _same(now.bounds, shot.bounds):
            raise StaleRef("the window moved, resized, changed owner or closed since the screenshot; take a new one")
        bx, by, bw, bh = shot.bounds
        px, py = bx + x * bw / shot.size[0], by + y * bh / shot.size[1]

        if mechanism == "pointer":
            return await self.pointer_at(name_or_bundle, shot_id, x, y, "click", holder=holder)
        if mechanism == "ax":
            el = await asyncio.to_thread(self.ax.hit_element, a.pid, px, py)
            if el is None:
                return ActionResult(False, "AX hit test unavailable; no pointer input attempted",
                                    {"method": "ax", "performed": False})
            win_frame = await asyncio.to_thread(lambda: self.ax.frame(axm._attr(el, "AXWindow")))
            if win_frame is None or not _same(win_frame, shot.bounds):
                return ActionResult(False, "AX hit target cannot be bound to the captured window",
                                    {"method": "ax", "performed": False})
            ok = await asyncio.to_thread(self.ax.press, el)
            return ActionResult(ok, "AX press accepted; inspect the result" if ok else "AX press refused",
                                {"method": "ax", "accepted": ok, "outcome_verified": False})
        if mechanism != "foreground":
            raise Refused("click mechanism must be pointer, ax or foreground")
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

    # ---------- actions ----------

    async def act(self, app: str, action: ComputerAction, holder: str = "", task: str | None = None) -> ActionResult:
        if action.kind in {"key", "type", "set_value", "select"} and action.text is None:
            raise Refused(f"{action.kind} requires explicit text; omitted text never means clear or Return")
        a = self.app(app)
        st = self._state.get(a.bundle_id)
        if action.element and st and st.pid and st.pid != a.pid:
            raise StaleRef("element process changed; take a new snapshot")
        mechanism = action.mechanism or ("pid" if action.kind in {"type", "key"} else "ax")
        if mechanism == "foreground" and not action.allow_foreground:
            return ActionResult(False, "This explicit route needs task-wide foreground permission.",
                                {"code": "needs_foreground", "method": mechanism, "performed": False})
        if action.kind == "key":
            if not action.text or not valid_combo(action.text):
                raise Refused(f"unsupported key {action.text!r}")
            return await self._key(a, action.text, holder, task, mechanism)
        if action.kind == "scroll":
            return await self._scroll(a, action, holder, task, mechanism)
        if action.element is None:
            raise Refused("this action needs an element id from a snapshot")
        n = self.node(a.bundle_id, action.element)
        if n.secret and action.kind in ("set_value", "type", "edit"):
            raise Refused("that's a password field; FERB never types passwords for the owner")
        if mechanism == "foreground":
            if action.kind in {"press", "click"}:
                return await self._real(a, click_steps(*self.ax.center(n)), holder, "click", task)
            if action.kind == "type":
                async def focus_it() -> None:
                    await asyncio.to_thread(self.ax.focus, n.ref)
                    if await asyncio.to_thread(self.ax.has_focus, a.pid, n.ref) is not True:
                        raise Refused(FOCUS_ELSEWHERE)
                return await self._real(a, type_steps(action.text), holder, "typing", task, focus_it)
            raise Refused(f"{action.kind} has no foreground implementation; choose its documented primitive")
        if action.kind in ("press", "click"):
            if mechanism != "ax":
                raise Refused("press uses AX; use mac.pointer for process/window mouse input")
            return await self._press(a, n, holder, task, False)
        if action.kind == "cancel":
            if mechanism != "ax":
                raise Refused("cancel requires an exact AX element")
            ok = await asyncio.to_thread(self.ax.cancel, n.ref)
            return ActionResult(ok, "AXCancel accepted" if ok else "AXCancel refused",
                                {"method": "ax", "accepted": ok, "outcome_verified": False})
        if action.kind == "focus":
            ok = await asyncio.to_thread(self.ax.focus, n.ref)
            self.profiles.record(a.bundle_id, "ax_focus", ok)
            return ActionResult(ok, "AX focus request accepted" if ok else "AX focus request refused",
                                {"method": "ax", "accepted": ok, "outcome_verified": False})
        if action.kind == "set_value":
            if mechanism not in {"ax", "ax_value"}:
                raise Refused("set_value uses AXValue; choose type for keyboard delivery")
            return await self._set_value(a, n, action.text, holder, task, False)
        if action.kind == "type":
            return await self._type(a, n, action.text, holder, task, mechanism)
        if action.kind == "edit":
            return await self._edit(a, n, action.select, action.text, mechanism)
        if action.kind == "select":
            return await self._select(a, n, action.text)
        raise Refused(f"unknown action {action.kind}")

    async def _foreground(self, a: Any, steps: list[Any], holder: str, what: str, task: str | None, allow: bool,
                          why: str, prepare: Callable[[], Awaitable[None]] | None = None) -> ActionResult:
        if not allow:
            return ActionResult(False, why, {"code": "needs_foreground", "performed": False})
        return await self._real(a, steps, holder, what, task, prepare)

    async def _press(self, a: Any, n: axm.AxNode, holder: str, task: str | None, fg: bool) -> ActionResult:
        ok = await asyncio.to_thread(self.ax.press, n.ref)
        self.profiles.record(a.bundle_id, "ax_press", ok)
        return ActionResult(ok, "AX press request accepted; inspect the outcome" if ok else
                            "AX press request refused; check what happened before trying again (if it may have gone "
                            "through, a repeat needs the owner)",
                            {"method": "ax", "accepted": ok, "outcome_verified": False})

    # ---- typing

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

    async def _set_value(self, a: Any, n: axm.AxNode, text: str, holder: str, task: str | None,
                         fg: bool) -> ActionResult:
        accepted = await asyncio.to_thread(self.ax.set_value, n.ref, text)
        observed = await asyncio.to_thread(self.ax.value, n.ref)
        ok = accepted and observed == text
        self.profiles.record(a.bundle_id, "ax_value", ok)
        return ActionResult(ok, "value set and read back" if ok else "AXValue result not confirmed; no typing attempted",
                            {"method": "ax_value", "accepted": accepted, "value_matches": observed == text})

    async def _type(self, a: Any, n: axm.AxNode, text: str, holder: str, task: str | None,
                    mechanism: str) -> ActionResult:
        if mechanism not in {"pid", "ax_keyboard"}:
            raise Refused("type needs mechanism pid or ax_keyboard; set_value is a separate whole-value operation")
        await asyncio.to_thread(self.ax.focus, n.ref)
        if await asyncio.to_thread(self.ax.has_focus, a.pid, n.ref) is not True:
            raise Refused(FOCUS_ELSEWHERE)
        before = await asyncio.to_thread(self.ax.value, n.ref)
        sig = await self._signature(a)
        if mechanism == "pid":
            await self.input.to_process(type_steps(text, a.pid), holder)
            accepted = True
        else:
            accepted = await asyncio.to_thread(self.ax.post_text, a.pid, text)
        await asyncio.sleep(SETTLE_S)
        after = await asyncio.to_thread(self.ax.value, n.ref)
        changed = after != before if before is not None else await self._signature(a) != sig
        self.profiles.record(a.bundle_id, "pid_keys" if mechanism == "pid" else "ax_keyboard", changed)
        return ActionResult(accepted, "typing delivered in the background; " +
                            ("a value change was observed" if changed else "result unconfirmed; inspect before retrying"),
                            {"method": mechanism, "delivered": accepted, "observed_change": changed,
                             "outcome_verified": False, "replayed": False})

    # ---- editing text in place

    async def _edit(self, a: Any, n: axm.AxNode, select: Any, text: str | None, mechanism: str = "ax") -> ActionResult:
        """Insert or replace text at an exact range through AXSelectedTextRange + AXSelectedText (verified on this Mac
        in the background). Whole-value replacement is a separate explicit ax_value method."""
        value = await asyncio.to_thread(self.ax.value, n.ref)
        length = len(value or "")
        if select in (None, "", "caret"):
            rng = await asyncio.to_thread(self.ax.selected_range, n.ref)
            if rng is None:
                raise Refused("caret range unavailable; choose an explicit range after inspecting the value")
        elif select == "all":
            rng = (0, length)
        elif select == "end":
            rng = (length, 0)
        elif select == "start":
            rng = (0, 0)
        elif isinstance(select, (list, tuple)) and len(select) == 2:
            rng = (int(select[0]), int(select[1]))
        else:
            raise Refused(f"select must be (start, length), \"all\", \"end\" or \"start\", not {select!r}")
        start, count = rng
        if value is None or start < 0 or count < 0 or start + count > length:
            raise Refused("edit range cannot be verified against the current full value")
        if mechanism not in {"ax", "ax_value"}:
            raise Refused("edit uses ax selected text or explicit ax_value rewrite")
        ok = mechanism == "ax" and await asyncio.to_thread(self.ax.set_selected_range, n.ref, start, count)
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
        if mechanism == "ax":
            self.profiles.record(a.bundle_id, "ax_selected_text", False)
        if mechanism == "ax_value" and await asyncio.to_thread(self.ax.set_value, n.ref, expected):
            if await asyncio.to_thread(self.ax.value, n.ref) == expected:
                self.profiles.record(a.bundle_id, "ax_value", True)
                return ActionResult(True, "edited by rewriting the box's whole value (the app's undo sees one change)")
        return ActionResult(False, "the selected edit method did not confirm the requested value; inspect before retrying",
                            {"method": mechanism, "outcome_verified": False, "replayed": False})

    # ---- keys

    async def _key(self, a: Any, combo: str, holder: str, task: str | None, mechanism: str) -> ActionResult:
        parts = normalize(combo)
        key, mods = parts[-1], parts[:-1]
        if mechanism == "foreground":
            return await self._real(a, key_steps(combo), holder, combo, task)
        if mechanism == "ax":
            found = await asyncio.to_thread(self.ax.menu_item_for_shortcut, a.pid, key, mods)
            if found is None:
                return ActionResult(False, "no unique menu item exposes that key equivalent; inspect mac.menu",
                                    {"method": "ax", "performed": False})
            item, path, enabled = found
            if not enabled:
                return ActionResult(False, f"menu item {path!r} is disabled", {"method": "ax", "performed": False})
            ok = await asyncio.to_thread(self.ax.press, item)
            return ActionResult(ok, f"AX menu request {'accepted' if ok else 'refused'}: {path}",
                                {"method": "ax", "accepted": ok, "outcome_verified": False})
        before = await self._signature(a)
        if mechanism == "pid":
            await self.input.to_process(key_steps(combo, a.pid), holder)
            accepted = True
        elif mechanism == "ax_keyboard":
            accepted = await asyncio.to_thread(self.ax.post_key, a.pid, KEYCODES[key], mods)
        else:
            raise Refused("key needs pid, ax_keyboard, ax (menu equivalent) or explicit foreground")
        await asyncio.sleep(SETTLE_S)
        changed = await self._signature(a) != before
        if changed:
            self.profiles.record(a.bundle_id, "pid_keycodes" if mechanism == "pid" else "ax_keyboard", True)
        return ActionResult(accepted, f"{combo} sent in the background; " +
                            ("state changed; inspect the outcome" if changed else "nothing visibly changed; result unconfirmed"),
                            {"method": mechanism, "delivered": accepted, "observed_change": changed,
                             "outcome_verified": False, "replayed": False})

    # ---- scrolling

    async def _scroll(self, a: Any, action: ComputerAction, holder: str, task: str | None, mechanism: str) -> ActionResult:
        direction = (action.direction or "down").lower()
        if direction not in ("up", "down", "left", "right"):
            raise Refused("scroll direction must be up, down, left or right")
        amount = int(action.amount or 1)
        if not 1 <= amount <= 50:
            raise Refused("scroll amount must be 1–50 per bounded call")
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
        if mechanism == "foreground":
            rect = await asyncio.to_thread(self.ax.frame, area)
            if rect is None:
                return ActionResult(False, "scroll pane has no current bounds", {"performed": False})
            x, y, w, h = rect
            return await self._real(a, wheel_steps(x + w / 2, y + h / 2,
                                    -10 if direction in {"down", "right"} else 10,
                                    horizontal=direction in {"left", "right"}), holder, "scroll", task)
        if mechanism not in {"ax", "pid"}:
            raise Refused("scroll supports ax scrollbar, pid text page keys or explicit foreground")
        content = await asyncio.to_thread(self.ax.scroll_content, area)
        vertical = direction in ("up", "down")
        bar = await asyncio.to_thread(self.ax.scroll_bar, area, vertical)
        # Page keys are only safe for an explicitly targeted text view, never a chat row/list selection.
        text_view = n is not None and n.ax_role == "AXTextArea" and vertical
        moved, route, stop = 0, "", "batch limit reached"
        before = await asyncio.to_thread(self.ax.scroll_position, area, content)
        for _ in range(amount):
            front = self.apps.frontmost()
            if self.input.owner_active() and front and front.pid == a.pid:
                stop = "owner is using this app"
                break
            if self._scroll_boundary(before, direction):
                stop = "scroll bar boundary reached; verify content before claiming the last item"
                break
            if bar is not None and mechanism == "ax":
                act = "AXIncrement" if direction in ("down", "right") else "AXDecrement"
                if not await asyncio.to_thread(self.ax.perform, bar, act):
                    stop = "AX scrollbar increment refused; no alternate route attempted"
                    break
                route = "scroll bar"
            elif text_view and mechanism == "pid":
                if not await asyncio.to_thread(self.ax.focus, n.ref):
                    stop = "text view could not take focus"
                    break
                await self.input.to_process(key_steps("pageup" if direction == "up" else "pagedown", a.pid), holder)
                route = "Page Up" if direction == "up" else "Page Down"
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
        detail = f"scrolled {direction}: {moved}/{amount} step(s) via {route or mechanism}; {stop}"
        if moved or "boundary reached" in stop:
            return ActionResult(True, detail)
        return ActionResult(False, detail, {"method": mechanism, "observed_steps": moved, "replayed": False})

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
                                window=shot.window, route=route, pixels=True, window_origin=shot.bounds[:2])
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
            matches = [el for el, t in items if t == title]
            hit = matches[0] if len(matches) == 1 else None
            if hit is not None and await asyncio.to_thread(self.ax.press, hit):
                self.profiles.record(a.bundle_id, "ax_press", True)
                await asyncio.sleep(0.3)
                now = await asyncio.to_thread(self.ax.value, n.ref)
                return ActionResult(True, f"AX selection press accepted for {title!r}; observed value: {now!r}",
                                    {"method": "ax", "accepted": True, "observed_value": now, "outcome_verified": False})
            if menu is not None:                        # closing it is cleanup: the non-selection is already known
                try:
                    await asyncio.to_thread(self.ax.cancel, menu)
                except axm.NoAnswer:
                    pass
            if items:
                return ActionResult(False, f"no unique exact item {title!r}; inspect the returned candidates",
                                    {"code": "target_unresolved", "matches": len(matches),
                                     "candidates": [{"label": t} for _, t in items], "performed": False})
        return ActionResult(False, "no unique exact menu item; inspect candidates and choose an explicit element",
                            {"code": "target_unresolved", "performed": False})

    # ---------- the foreground, last resort, only by request ----------

    async def _bring_forward(self, a: Any, shot: Shot) -> None:
        """After activating the app: raise the captured window itself and wait (~3 s) for it to be on screen — the
        app may switch to another Space or a full-screen window first."""
        await asyncio.to_thread(self.ax.raise_window, a.pid, shot.bounds)
        for _ in range(12):
            front = self.apps.frontmost()
            if self.input.owner_active() or front is None or front.pid != a.pid:
                raise Refused("owner input or focus changed while waiting for the explicitly requested window")
            if await asyncio.to_thread(self.shots.on_screen, shot.window):
                await asyncio.sleep(0.2)
                return
            await asyncio.sleep(0.25)

    async def _real(self, a: Any, steps: list[Any], holder: str, what: str, task: str | None = None,
                    prepare: Callable[[], Awaitable[None]] | None = None) -> ActionResult:
        """An explicitly chosen foreground action. Yield permanently if the owner takes over; restore nothing."""
        if not self.input.available():
            return ActionResult(False, "FERB is paused: no foreground input", {"performed": False})
        await self.input.quiet(holder, what)
        front = self.apps.frontmost()
        if front is None or front.pid != a.pid:
            self.on_activate(a.bundle_id, task)
            self.apps.activate(a.bundle_id, a.pid)
            await asyncio.sleep(0.3)
        if prepare is not None:
            await prepare()
        def guarded(step: Step) -> Step:
            def post() -> None:
                front_now = self.apps.frontmost()
                if front_now is None or front_now.pid != a.pid:
                    raise PointerInterrupted("foreground target changed; input stopped without restoring an app")
                step.post()
            return Step(step.what, post)
        n = await self.input.run([guarded(step) for step in steps], holder, what)
        return ActionResult(True, f"{what} ({n} steps, explicit foreground input); inspect the result",
                            {"method": "foreground", "delivered_steps": n, "outcome_verified": False})


def _same(a: tuple[float, ...], b: tuple[float, ...]) -> bool:
    return all(abs(p - q) <= 1 for p, q in zip(a, b))


def profiles_dir(home: Path) -> Path:
    return home / "device" / "apps"
