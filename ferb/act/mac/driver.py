"""Own native target references and explicit AX/process/foreground input mechanics.
Agents choose methods and retries; this driver never substitutes targets, replays uncertain input or restores focus.
"""

import asyncio
import contextlib
import math
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
from ferb.act.evidence.mask import PageState, page_masks
from ferb.act.mac.profile import Profiles
from ferb.core.errors import NoTarget, Refused, StaleRef, Withheld
from ferb.core.types import ActionResult, ComputerAction

UNTRUSTED_APPS = {"com.apple.mail": "email", "com.apple.MobileSMS": "message", "net.whatsapp.WhatsApp": "message",
                  "com.tinyspeck.slackmacgap": "message", "com.hnc.Discord": "message",
                  "ru.keepcoder.Telegram": "message", "com.microsoft.teams2": "message"}


Rect = tuple[float, float, float, float]          # x, y, w, h

# L13: a capture is never refused for what it shows. Owner, 2026-10-14: "no screenshots should be refused; go with
# redacted stuff" (and "no exceptions"). The secret fields found are painted; where they can't be located or placed,
# the picture goes on to the F5 sanitizer's text recognition (every image path runs it) and its note says so. It is
# withheld only when the boxes found can't be painted on it at all.
OCR_ONLY = "masked by text recognition only (a secret it can't read as text could show)"
OCR_REST = "anything else is left to text recognition (a secret it can't read as text could show)"
PAGE_UNREAD = ("the page's secret fields couldn't be located (neither the browser extension nor Accessibility could "
               "read the page this window shows: {why}); " + OCR_ONLY)
WINDOW_UNREAD = ("the window's secret fields couldn't be located (Accessibility couldn't read this window when it was "
                 "captured); " + OCR_ONLY)
WINDOW_HALF_READ = ("Accessibility couldn't read this window in one look around the capture: the secret fields the "
                    "other look found are painted where it saw them; " + OCR_REST)
FIELD_UNPLACED = "a secret field Accessibility found has no place on screen to paint; " + OCR_ONLY
FIELD_MOVED = ("a secret field moved, appeared or went during the capture: painted where it was before and after; "
               + OCR_REST)
PAGE_MOVED = ("the page changed during the capture: its secret fields are painted where they were before and after; "
              + OCR_REST)
PAGE_UNPLACED = ("the page's secret fields couldn't be placed on the window (Accessibility shows no page area); "
                 + OCR_ONLY)
PAGE_PART_PLACED = ("the page changed during the capture and one look's secret fields couldn't be placed on the "
                    "window (Accessibility showed no page area): the others are painted where they were; " + OCR_REST)
ONE_LOOK = ("the browser extension read the page in one look around the capture only ({why}): its secret fields are "
            "painted where it saw them; a field only the other look could show is left to text recognition (a secret "
            "it can't read as text could show)")
UNPAINTABLE = "the capture couldn't be painted over; pixels withheld"
AX_ONLY = "masked from Accessibility only: the browser extension couldn't read this page ({why})"
EXTENSION_UNREAD = "not paired with this window's profile, an incognito window, or a page scripts can't reach"
AX_NO_AREA, AX_EMPTY = "Accessibility shows no page area", "Accessibility's page area is empty"
OTHER_PAGE = ("the paired profile's window at this place shows another page (its active tab's title isn't the "
              "captured window's)")


def same_title(a: str, b: str) -> bool:
    """Two window/tab titles as the same text, whitespace runs aside (the window server trims and folds them)."""
    return " ".join(a.split()) == " ".join(b.split())


@dataclass(frozen=True)
class WebSecrets:
    """What the browser extension says about the page a browser window shows (L13): the mask state of its active tab
    (mask.PageState: isSecret's password and code fields, keyed, in visual-viewport CSS px). None: it couldn't be read,
    and `why` says why when known (FERB's words, or the extension's own fixed reason; never page content)."""
    state: PageState | None = None
    why: str = ""
    late: bool = False             # a profile didn't answer (timed out or failed): not asked again for this capture
    tab: tuple[str, str] | None = None   # the answering window's active tab (URL, title), protocol 15

    @property
    def complete(self) -> bool:
        return self.state is not None

    @classmethod
    def from_reply(cls, r: dict[str, Any]) -> "WebSecrets":
        """A `window_secrets` answer from the profile that owns the window. Anything malformed is a page nobody could
        read, never a page without secrets."""
        state = PageState.parse(r.get("state")) if r.get("complete") is True else None
        reason = r.get("reason")
        why = f"its profile couldn't read the page: {reason[:160]}" if isinstance(reason, str) and reason else \
            "its profile couldn't read the page"
        t = r.get("tab")
        tab = (t["url"][:4096], t["title"]) if isinstance(t, dict) and isinstance(t.get("url"), str) \
            and isinstance(t.get("title"), str) else None         # the title whole: it is compared (round 3, m2)
        return cls(state, "" if state is not None else why, tab=tab)


Look = tuple[axm.AxTree | None, WebSecrets | None]      # one look at a window: its AX walk, the extension's page


def window_masks(before: Look, after: Look, browser: bool) -> tuple[list[Rect], str]:
    """Screen-point boxes to paint over on one capture of a window (L13), from the looks before and after it, and a
    note on how its secret fields were found when that is worth saying. Painted: the secret fields the AX walks found
    (AXSecureTextField, and text fields whose metadata names a password field) and, for a browser, the fields the
    extension marks secret in its page (mask.page_masks), placed on the AX web area that draws it. A field whose place
    during the capture isn't certain (it moved, appeared or went between the looks) is painted where each look saw
    it. Where they can't be located (Accessibility can't read the window, neither the extension nor Accessibility
    read the browser's page) or placed (a field with no box, a page with no area), the note says the picture is left
    to text recognition. Never refused: only `painted` withholds, when the boxes can't be painted at all."""
    (t1, w1), (t2, w2) = before, after
    if t1 is None or t2 is None or t1.secure is None or t2.secure is None:
        # One look (or both) couldn't read the window: what a readable look located is still painted, its page's
        # fields on its own page area (review I1)
        read = [(t, w) for t, w in ((t1, w1), (t2, w2)) if t is not None and t.secure is not None]
        masks = _union(*(_ax_boxes(t)[0] for t, _ in read))
        notes = [WINDOW_HALF_READ if read else WINDOW_UNREAD]
        if not all(_ax_boxes(t)[1] for t, _ in read):        # what that look found isn't all painted either (A1)
            notes.append(FIELD_UNPLACED)
        for t, w in read:
            if w is not None and w.state is not None:
                rects, ok = _placed(_look_items(w.state, None), w.state, t)
                masks += rects
                if not ok:
                    notes.append(PAGE_UNPLACED)
        if read:                  # the extension's page from a look with no AX window: on a readable look's page area
            for unread_tree, w in ((t1, w1), (t2, w2)):        # (round 3, m1; painting a little too much is harmless)
                if (unread_tree is None or unread_tree.secure is None) and w is not None and w.state is not None:
                    rects, ok = _placed(_look_items(w.state, None), w.state, read[0][0])
                    masks += rects
                    if not ok:
                        notes.append(PAGE_UNPLACED)
        return masks, "; ".join(dict.fromkeys(notes))
    (ax1, whole1), (ax2, whole2) = _ax_boxes(t1), _ax_boxes(t2)
    notes = [FIELD_UNPLACED] if not (whole1 and whole2) else [FIELD_MOVED] if sorted(ax1) != sorted(ax2) else []
    masks = _union(ax1, ax2)
    p1, p2 = (w1.state if w1 else None), (w2.state if w2 else None)
    if p1 is not None and p2 is not None:
        page, note = _page_on_screen(p1, p2, t1, t2)
        return masks + page, "; ".join(notes + [note] if note else notes)
    placed = False                                   # the extension answered one look only: its fields still count
    for p, t in ((p1, t1), (p2, t2)):
        if p is not None:
            rects, placed = _placed(_look_items(p, None), p, t)
            masks += rects
    if browser:
        why = (w2.why if w2 else "") or (w1.why if w1 else "") or EXTENSION_UNREAD
        ax_why = next((AX_EMPTY if t.has_web else AX_NO_AREA for t in (t1, t2) if not t.has_web
                       or any(a in t.empty_web_areas for a in axm.outermost(t.web_areas))), None)
        notes.append(ONE_LOOK.format(why=why) if placed else PAGE_UNREAD.format(why=f"{why}; {ax_why}") if ax_why
                     else AX_ONLY.format(why=why))
    return masks, "; ".join(notes)


def _page_on_screen(p1: PageState, p2: PageState, t1: axm.AxTree, t2: axm.AxTree) -> tuple[list[Rect], str]:
    """The page's secret fields on screen for one capture, from the extension's looks before and after it and the AX
    page areas each look saw; a note when they weren't still (painted where each look saw them, whatever each look
    can place: review I1) or can't be placed."""
    a1, a2 = axm.outermost(t1.web_areas), axm.outermost(t2.web_areas)
    still: tuple[list[Rect], Rect] | None = None
    if sorted(a1) == sorted(a2):
        with contextlib.suppress(Withheld):          # something moved, appeared or went, or the page changed
            still = page_masks(p1, p2)
    if still is not None:
        try:
            return page_on_screen(*still, a2), ""
        except Withheld:                             # fields, and no page area to place them on
            return [], PAGE_UNPLACED
    (r1, ok1), (r2, ok2) = _placed(_look_items(p1, p2), p1, t1), _placed(_look_items(p2, p1), p2, t2)
    return r1 + r2, PAGE_MOVED if ok1 and ok2 else PAGE_PART_PLACED if ok1 or ok2 else PAGE_UNPLACED


def _look_items(p: PageState, other: PageState | None) -> list[Rect]:
    """What one look of a page must paint when the looks can't be merged, as page_masks would: its secret fields and
    the frame hosts it paints whole, and a frame host the other look painted whole or whose frame changed document
    since (a reload): painted wherever either look saw it (round 2, A2). A frame clean in both looks (no secret field
    in it, `paint` False) stays visible (review M4)."""
    then = {i.key: i for i in other.items} if other is not None else {}
    return [i.rect for i in p.items
            if i.paint or (i.key in then and (then[i.key].paint or then[i.key].child != i.child))]


def _placed(rects: list[Rect], p: PageState, t: axm.AxTree) -> tuple[list[Rect], bool]:
    """A look's page rects on that look's own page area, and whether they could be placed (no rects: trivially)."""
    try:
        return page_on_screen(rects, p.viewport, axm.outermost(t.web_areas)), True
    except Withheld:
        return [], False


def _ax_boxes(tree: axm.AxTree) -> tuple[list[Rect], bool]:
    """The secret-field boxes an AX walk found that can be painted, and whether that is every one it found."""
    boxes = list(tree.secret_rects) if tree.secret_rects is not None else [n.rect for n in tree.nodes if n.secret]
    placed = [b for b in boxes if b[2] > 0 and b[3] > 0]
    return placed, not (tree.secure and not boxes) and len(placed) == len(boxes)


def page_tab(titles: tuple[str, ...], *looks: Look) -> tuple[str, str] | None:
    """The captured browser window's active tab as the extension named it (protocol 15), latest look first: only when
    its title is one the window server gave the captured window (when it was listed, or now: a page can retitle itself
    mid-capture, round 3 m2). The extension only sees its own profile's windows; another profile's window can be at the
    same place. No title from the window server: None (the AppleScript read, which sees every window, decides)."""
    known = [t for t in titles if t]
    for _, web in reversed(looks):
        if web is not None and web.tab is not None and any(same_title(web.tab[1], t) for t in known):
            return web.tab
    return None


def _union(*boxes: list[Rect]) -> list[Rect]:
    return list(dict.fromkeys(b for group in boxes for b in group))


def page_on_screen(rects: list[Rect], viewport: Rect, areas: list[Rect]) -> list[Rect]:
    """The page's secret-field rects in screen points: its visual viewport's CSS px scaled onto the AX web area that
    draws it, the outermost one whose shape fits the viewport best (a side panel or docked tools are others). Areas
    that fit equally well (a split view: which half is the active tab can't be told) all get them. Withheld when there
    are fields and no area to place them on."""
    if not rects:
        return []
    ox, oy, vw, vh = viewport
    fits = [a for a in areas if a[2] > 0 and a[3] > 0]
    if not fits:
        raise Withheld(PAGE_UNPLACED)
    misfit = {a: abs(math.log((a[2] / vw) / (a[3] / vh))) for a in fits}
    best = min(misfit.values())
    out: list[Rect] = []
    for ax, ay, aw, ah in (a for a in fits if misfit[a] == best):
        kx, ky = aw / vw, ah / vh
        out += [(ax + (x - ox) * kx, ay + (y - oy) * ky, w * kx, h * ky) for x, y, w, h in rects]
    return out


async def painted(jpeg: bytes, masks: list[Rect], bounds: Rect, size: tuple[int, int]) -> bytes:
    """The window capture with every box painted over opaque (in a worker thread); withheld when it can't be."""
    from ferb.act.evidence.mask import mask_secret_fields, to_image_boxes
    boxes = to_image_boxes(masks, bounds, size)
    if not boxes:
        return jpeg
    try:
        return await asyncio.to_thread(mask_secret_fields, jpeg, boxes)
    except (ValueError, RuntimeError):
        raise Withheld(UNPAINTABLE) from None


SETTLE_S = 0.4                # after background events, before reading what changed (one read, from the walker)
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
    masking: str = ""                              # how its secret fields were found, when that is worth saying (L13)
    tab: tuple[str, str] | None = None             # a browser window's active tab (URL, title) from the extension


@dataclass
class _AppState:
    pid: int = 0
    gen: int = 0
    nodes: dict[str, axm.AxNode] = field(default_factory=dict)
    shots: int = 0
    shot: Shot | None = None
    tree: axm.AxTree | None = None                 # the latest walk (serves snapshot, signature, secure scan)
    tree_at: float = 0.0


class MacComputerDriver:
    os: Literal["macos", "windows", "linux"] = "macos"

    def __init__(self, device_id: str, real_input: RealInput, excluded: Callable[[], Collection[str]] = frozenset,
                 ax: Any = axm, apps: Any = appsm, shots: Any = shotm,
                 web_secrets: Callable[[str, Rect, str], Awaitable[WebSecrets | None]] | None = None,
                 profiles: Profiles | None = None) -> None:
        self.device_id, self.input, self.excluded, self.ax, self.apps = device_id, real_input, excluded, ax, apps
        self.shots, self.web_secrets = shots, web_secrets       # web_secrets: the extension's page inventory (L13)
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

    async def _look(self, a: Any, w: Any, before: Look | None = None) -> Look:
        """One look at the captured window `w` now, for its capture masks (`window_masks`): a fresh Accessibility walk
        (never an earlier one: a field may have moved since) and, for a browser, the extension's page, asked while the
        walk runs (review M6: one after the other, two looks could outlast the planner's wait for the front window).
        After a look `before` whose ask nobody answered, the extension isn't asked again. The AX window is the one at
        the window's bounds (with its title, when several are there, confirmed against the window server's title now).
        None there (it moved, or can't be read): no walk (`window_masks` then uses no page from this look)."""
        def now_titled() -> str:                     # the window server's title for the captured window now (M7)
            return self._title_now(w)

        async def walk() -> axm.AxTree | None:
            win = await asyncio.to_thread(self.ax.window_with_frame, a.pid, w.bounds, w.title or None, now_titled)
            return await self._walk(a, win) if win is not None else None
        browser = BROWSER_BUNDLES.get(a.bundle_id)
        earlier = before[1] if before is not None else None
        if not (browser and self.web_secrets) or (earlier is not None and earlier.late):
            return await walk(), earlier
        ask = asyncio.ensure_future(self.web_secrets(browser, w.bounds, w.title))
        try:
            tree = await walk()
        except BaseException:                        # the walk failed or was cancelled: so is the ask beside it (A3)
            ask.cancel()
            with contextlib.suppress(asyncio.CancelledError, Exception):
                await ask                            # (a second cancel arriving during this wait is swallowed)
            raise
        web = await ask
        # The answering window must be the captured one (B5): its active tab carries the captured window's title. A
        # paired profile's window at the same bounds showing another page (an unpaired window captured over it) would
        # have its fields painted over the wrong picture. Unknown either way (no title from the window server, no tab
        # in the answer): kept, as before protocol 15.
        if web is not None and web.state is not None and w.title and web.tab is not None \
                and not same_title(web.tab[1], w.title) and not same_title(web.tab[1], now_titled()):
            web = WebSecrets(None, OTHER_PAGE)                # (the window's title now counts too: round 3, m2)
        # a look with no AX window keeps the extension's answer too: `late` must reach the look after the capture (A4),
        # and its page is placed on the readable look's page area (round 3, m1)
        return tree, web

    async def screenshot(self, name_or_bundle: str,
                         marks: list[tuple[str, tuple[float, float, float, float]]] | None = None,
                         window: str | None = None, native_resolution: bool = False
                         ) -> tuple[Any, Shot, bytes]:
        """The app's front window only, in memory, with every secret field painted over (L13): the password fields
        the Accessibility tree shows (and, for a browser, the fields the extension marks secret in the tab on screen),
        found before and after the capture (`window_masks`; where they can't be located or placed, the picture is left
        to text recognition, never refused). Withheld only when the boxes can't be painted on it. The capture becomes
        the app's latest shot (mac.click / pointer / record); its `masking` says how a browser page's fields were found
        (Accessibility alone, or not at all) and when the rest is left to text recognition."""
        a, w, jpeg, size, note, tab = await self._capture(name_or_bundle, marks, window, native_resolution)
        st = self._state.setdefault(a.bundle_id, _AppState())
        st.shots += 1
        st.shot = Shot(f"shot{st.shots}", w.id, w.bounds, size, pid=a.pid,
                       signature=st.tree.signature() if st.tree else None, masking=note, tab=tab)
        return a, st.shot, jpeg

    async def window_image(self, name_or_bundle: str, window: str | None = None) -> tuple[Any, Shot, bytes]:
        """FERB's own picture of the app's front window for the evidence store (D26, D29), or of the exact window id
        `window` (L12: the owner's front window at send): the same capture and masking as `screenshot`, but it
        never becomes the app's latest shot, so a worker's pending mac.click / pointer / record keeps its own. Its
        `tab` is the browser window's active tab as the extension named it, when its title is the window's (B5)."""
        a, w, jpeg, size, note, tab = await self._capture(name_or_bundle, None, window, False)
        return a, Shot("evidence", w.id, w.bounds, size, pid=a.pid, masking=note, tab=tab), jpeg

    async def page_secrets(self, pid: int, bounds: Rect, title: str = "") -> tuple[list[Rect], Rect, bool] | None:
        """For a browser tab capture whose page the extension couldn't read (B6): the secret fields the Accessibility
        tree of that browser process's window at `bounds` shows inside its page area, that area (screen points), and
        whether every secret field it found has a box. The window is the one showing the tab: picked by the tab's
        `title` among windows at those bounds, and its own title must be the tab's (round 3, m6: a Chromium window is
        named after the tab it shows). None: no such single AX window, the walk can't say, or not exactly one page
        area (docked tools or a split view: which one is the tab can't be told)."""
        a = self.app(f"pid:{pid}")
        win = await asyncio.to_thread(self.ax.window_with_frame, pid, bounds, title or None)
        if win is None or (title and not same_title(await asyncio.to_thread(self.ax.window_title, win), title)):
            return None
        tree = await self._walk(a, win)
        areas = axm.outermost(tree.web_areas)
        if tree.secure is None or len(areas) != 1 or areas[0][2] <= 0 or areas[0][3] <= 0:
            return None
        boxes, whole = _ax_boxes(tree)
        ax, ay, aw, ah = areas[0]
        inside = [b for b in boxes if b[0] < ax + aw and b[0] + b[2] > ax and b[1] < ay + ah and b[1] + b[3] > ay]
        return inside, areas[0], whole

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
                       window: str | None, native_resolution: bool
                       ) -> tuple[Any, Any, bytes, tuple[int, int], str, tuple[str, str] | None]:
        a = self.app(name_or_bundle)
        await self._expose(a)                         # an Electron app builds its full tree only when asked
        browser = a.bundle_id in BROWSER_BUNDLES

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
        first = await self._look(a, w)
        jpeg, width, height = await asyncio.to_thread(grab, w)
        second = await self._look(a, w, first)
        masks, note = window_masks(first, second, browser)
        return (a, w, await painted(jpeg, masks, w.bounds, (width, height)), (width, height), note,
                page_tab((w.title, self._title_now(w)) if browser else (), first, second))

    def _title_now(self, w: Any) -> str:
        """The window server's title for this captured window id now ("" when it is gone)."""
        current = self.shots.window(w.id)
        return current.title if current is not None else ""

    def recording_source(self, name: str, shot_id: str) -> tuple[dict[str, Any], Any]:
        """Pin the window the worker actually saw; repeated captures never replace its action references. Each frame is
        painted over like a screenshot (L13), its note returned with it; a frame that can't be painted is withheld
        (Withheld), never the recording."""
        a = self.app(name)
        st = self._state.get(a.bundle_id)
        shot = st.shot if st else None
        if shot is None or shot.id != shot_id or shot.pid != a.pid or time.monotonic() - shot.at > 30:
            raise Refused('take a fresh mac_screenshot to select the recording window')

        async def current() -> Any:
            w = await asyncio.to_thread(self.shots.window, shot.window)
            if w is None or w.pid != a.pid or self.app(a.bundle_id).pid != a.pid:
                raise Refused('recording window closed or changed owner')
            return w

        async def capture() -> bytes | tuple[bytes, str]:
            w = await current()
            await self._expose(a)
            browser = a.bundle_id in BROWSER_BUNDLES
            first = await self._look(a, w)
            image, width, height = await asyncio.to_thread(self.shots.capture, w)
            await current()
            masks, note = window_masks(first, await self._look(a, w, first), browser)   # moved: OCR only
            frame = await painted(image, masks, w.bounds, (width, height))
            return (frame, note) if note else frame
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

    async def _value_reads(self, n: axm.AxNode, text: str) -> str | None:
        """Poll the element's value until it reads `text` or SETTLE_S passes (cheap: one attribute read per poll). A
        web field's accessibility value follows a set asynchronously (Chrome, live 2026-10-24)."""
        deadline = time.monotonic() + SETTLE_S
        while True:
            observed = await asyncio.to_thread(self.ax.value, n.ref)
            if observed == text or time.monotonic() >= deadline:
                return observed
            await asyncio.sleep(0.05)

    async def _set_value(self, a: Any, n: axm.AxNode, text: str, holder: str, task: str | None,
                         fg: bool) -> ActionResult:
        accepted = await asyncio.to_thread(self.ax.set_value, n.ref, text)
        observed = (await self._value_reads(n, text) if accepted
                    else await asyncio.to_thread(self.ax.value, n.ref))
        ok = accepted and observed == text
        self.profiles.record(a.bundle_id, "ax_value", ok)
        facts: dict[str, Any] = {"method": "ax_value", "accepted": accepted, "value_matches": observed == text}
        if ok:
            return ActionResult(True, "value set and read back", facts)
        # Live 2026-10-24: the field had taken the value, the worker typed it again and it held it twice. What a
        # snapshot would show about the field, never its text: whether it is empty, and its length.
        facts |= {"field_empty": not observed, "field_length": len(observed)} if observed is not None else {}
        return ActionResult(False, "AXValue result not confirmed; no typing attempted. The field may already hold "
                            "text: read it in a fresh snapshot before typing again (typing adds to what is there)",
                            facts)

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
