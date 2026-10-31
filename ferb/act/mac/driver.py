"""Own native target references and explicit AX/process/foreground input mechanics.
Agents choose methods and retries; this driver never substitutes targets, replays uncertain input or restores focus.
"""

import asyncio
import contextlib
import dataclasses
import hashlib
import math
import os
import re
import threading
import time
from dataclasses import dataclass, field
from pathlib import Path
from types import SimpleNamespace
from typing import Any, Awaitable, Callable, Collection, ContextManager, Literal

from ferb.act.mac import accessibility as axm
from ferb.act.mac import apps as appsm
from ferb.act.mac import browsers
from ferb.act.mac import screenshot as shotm
from ferb.act.mac.clipboard import read_general
from ferb.act.mac.window_input import load_route
from ferb.act.mac.input import (SCREEN_WIDE, PointerInterrupted, RealInput, Step, click_steps, enters_text,
                                key_steps, normalize, pointer_steps, quit_or_hide, screen_wide, shortcut_combo,
                                type_steps, valid_combo, wheel_steps)
from ferb.act.evidence.mask import PageState, page_masks
from ferb.act.mac.profile import Profiles
from ferb.core import call_facts
from ferb.core.errors import NoTarget, Refused, StaleRef, Withheld
from ferb.core.types import ActionFacts, ActionResult, ComputerAction, ReceivedBy, TerminalGrant, TerminalInput
from ferb.policy import terminal as terminals
from ferb.policy import confirm as floors

# Rule 8, rule 9, D31, W2: FERB never operates its own apps (the notch shows FERB's own questions; pressing, clicking or
# typing into it would answer them for the owner): `front_window.FERB_APPS` and the daemon's own process
OWN_APP = "FERB never operates its own app; questions are answered by the owner"
# Fix round 3, H1: fail closed when FERB can't tell whose window would take a click
CANT_CONFIRM = ("FERB can't confirm what is under this point, so it didn't click: one of its own windows could be "
                "there (rule 8, rule 9)")
# Fix round 4, N6: the Run… label rule's walk of a window for a terminal; past either, FERB can't tell, and asks
WINDOW_WALK_NODES, WINDOW_WALK_S = 2000, 0.2
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
TYPE_CHUNK = 8                # D31: typed text goes in chunks this long, the keyboard focus checked before each
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
    nodes: tuple[axm.AxNode, ...] = ()             # what the window's own walk at the capture found (staleness, P19)
    title: str = ""                                # the window server's title for it at the capture


@dataclass
class _AppState:
    pid: int = 0
    gen: int = 0
    nodes: dict[str, axm.AxNode] = field(default_factory=dict)
    shots: int = 0
    shot: Shot | None = None
    tree: axm.AxTree | None = None                 # the latest walk (serves snapshot, signature, secure scan)
    tree_at: float = 0.0
    window_id: int | None = None                   # the window-server id of the window the latest snapshot read


CLICK_GRID = 24.0             # P19: a click on no small element is identified by its point on a grid this fine (points)
CLICK_ELEMENT_MAX = 400.0     # P19: an element larger than this (a canvas, a page) doesn't identify a click by itself


def _node_at(nodes: Collection[axm.AxNode], x: float, y: float) -> axm.AxNode | None:
    """The smallest element of a walk whose frame holds this screen point (the deepest, as a hit test finds), or
    None."""
    holding = [n for n in nodes if n.rect[2] > 0 and n.rect[3] > 0 and n.rect[0] <= x < n.rect[0] + n.rect[2]
               and n.rect[1] <= y < n.rect[1] + n.rect[3]]
    return min(holding, key=lambda n: n.rect[2] * n.rect[3]) if holding else None


def shot_to_screen(shot: Any, x: float, y: float) -> tuple[float, float]:
    """Screenshot pixels to global screen points: the captured window's bounds scaled by the image size."""
    bx, by, bw, bh = shot.bounds
    return bx + x * bw / shot.size[0], by + y * bh / shot.size[1]


# The input routes `act` takes for each action (its own checks); it refuses any other before input (item 9: a route
# an action doesn't honour is refused, never ignored). A slider's scroll steps take any route (`_scroll`), but which
# element is a slider isn't checked here: it gets no point.
ELEMENT_ROUTES: dict[str, frozenset[str]] = {
    "press": frozenset({"ax", "foreground"}), "click": frozenset({"ax", "foreground"}),
    "type": frozenset({"pid", "foreground"}), "set_value": frozenset({"ax", "ax_value"}),
    "edit": frozenset({"ax", "ax_value"}), "cancel": frozenset({"ax"}),
    "scroll": frozenset({"ax", "pid", "foreground"}), "focus": frozenset({"ax"}), "select": frozenset({"ax"})}
KEY_ROUTES = frozenset({"pid", "ax", "foreground"})
SECURE_KEYS = ("that's a password field, and this key would put text into it; FERB never types passwords for the "
               "owner (the owner fills it, or mac.secret does from the Keychain)")
WINDOW_GONE = "the window moved or closed since the screenshot; take a new one"
SHOT_AGE_S = 30.0             # a screenshot's points are trusted this long (mac.click / mac.pointer)


def _pastes(parts: list[str]) -> bool:
    """A paste key: ⌘V, with or without other modifiers (Paste and Match Style is ⌥⇧⌘V)."""
    return bool(parts) and parts[-1] == "v" and "cmd" in parts[:-1]


def _pastes_item(shortcut: tuple[str, int] | None) -> bool:
    """A menu item whose key equivalent is ⌘V (AXMenuItemCmdModifiers bit 8 set means no ⌘): pressing it pastes."""
    return shortcut is not None and shortcut[0].lower() == "v" and not shortcut[1] & 8


@dataclass
class _Armed:
    """The terminal element FERB just typed approved text into: the owner's Yes covers one Return there (D31)."""
    el: Any
    confirmation_id: str
    task: str | None


def acts_on(action: ComputerAction, n: axm.AxNode) -> bool:
    """L33: whether `act` would deliver this action to the element `n`, so FERB's cursor points only there. False when
    `act` refuses it before any input, by its own checks (no text or item title; a foreground route without the
    call's permission; a route that action doesn't take; typing into a password field), and for keys, which go to the
    app's keyboard focus, not to the element. Synchronous, no I/O."""
    if action.kind == "key" or action.kind in {"type", "set_value", "select"} and action.text is None \
            or action.kind == "select" and not (action.text or "").strip():
        return False
    mechanism = action.mechanism or ("pid" if action.kind == "type" else "ax")
    if mechanism == "foreground" and not action.allow_foreground:
        return False
    if n.secret and action.kind in ("set_value", "type", "edit"):
        return False
    return mechanism in ELEMENT_ROUTES.get(action.kind, frozenset())


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
        # D31: the owner's [protection] terminal_apps (policy/terminal.py adds the built-in list), the clipboard's
        # (change count, text, concealed) for a paste's question, and the Return each process may still take
        self.terminal_apps: Callable[[], Collection[str]] = lambda: ()
        self.clipboard: Callable[[], tuple[int, str | None, bool]] = read_general
        self._armed: dict[int, _Armed] = {}
        # Fix round 4, H1 and N1: each FERB app's click-through windows (its pid: their window numbers), as it reported
        # them (`ui.click_through`); the point check skips exactly those while that app runs
        self._click_through: dict[int, frozenset[int]] = {}
        self._click_lock = threading.Lock()
        # L39 / owner decision 2: the apps each task launched (launched.LaunchedApps, set by the daemon): ⌘Q / ⌘H
        # reach only those, through its safe quit/hide
        self.launched: Any = None

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
        if self._own_app(a.bundle_id, a.pid):            # FERB's own app: every Mac tool resolves through here
            raise Refused(OWN_APP)
        return a

    async def _resolve(self, name_or_bundle: str) -> Any:
        """`app`, off the event loop (it reads the running apps through NSWorkspace)."""
        return await asyncio.to_thread(self.app, name_or_bundle)

    def is_browser(self, bundle_id: str) -> bool:
        """A web browser by the one list (browsers.py): known bundle ids, or an app Launch Services last listed as
        opening web links (through `apps`, so tests can fake it). Never blocks."""
        cached = getattr(self.apps, "known_opens_web_links", None)
        return browsers.known(bundle_id) or (bool(cached(bundle_id)) if callable(cached) else False)

    def browser_now(self, bundle_id: str) -> bool:
        """Blocking: `is_browser`, with Launch Services asked afresh when its list is stale."""
        fresh = getattr(self.apps, "opens_web_links", None)
        return browsers.known(bundle_id) or (bool(fresh(bundle_id)) if callable(fresh) else False)

    @staticmethod
    def _own_app(bundle_id: str, pid: int) -> bool:
        """One of FERB's own apps (the notch, FERB.app, its helpers: `front_window.FERB_APPS`), or the daemon itself."""
        import os
        from ferb.sense.front_window import FERB_APPS
        return bundle_id.casefold() in FERB_APPS or pid == os.getpid()

    def _own_pid(self, pid: int) -> bool:
        """Whether this process is one of FERB's own apps or the daemon, among every running app whatever its
        activation policy (fix round 3, H1: the notch is an accessory app, absent from the Regular apps list)."""
        import os
        from ferb.sense.front_window import FERB_APPS
        if pid == os.getpid():
            return True
        return pid in self.apps.pids_of(FERB_APPS)

    def _own_pids(self) -> set[int]:
        """Blocking: FERB's processes now: every running app of `FERB_APPS` whatever its activation policy, and the
        daemon (fix round 3, H1)."""
        from ferb.sense.front_window import FERB_APPS
        return set(self.apps.pids_of(FERB_APPS)) | {os.getpid()}

    def report_click_through(self, pid: int, windows: Collection[int]) -> bool:
        """Blocking (fix round 4, H1 and N1): the click-through windows a FERB app reports by number (the notch: FERB's
        pointer overlay on each display and its glow), replacing its last report, so the point check skips exactly
        those, only that app's own, while it runs. False, and nothing recorded, unless `pid` is one of FERB's running
        apps."""
        from ferb.sense.front_window import FERB_APPS
        running = set(self.apps.pids_of(FERB_APPS))
        if pid not in running:
            return False
        with self._click_lock:
            self._click_through = {p: w for p, w in self._click_through.items() if p in running} | {
                pid: frozenset(windows)}
        return True

    def click_through(self) -> dict[int, frozenset[int]]:
        """Each FERB app's reported click-through windows (its pid: their numbers), as last told."""
        with self._click_lock:
            return dict(self._click_through)

    def _skipped(self, own: Collection[int]) -> dict[int, frozenset[int]]:
        """The reported click-through windows whose reporter still runs (`own`: FERB's processes now). A report from a
        process that ended is dropped, so a later process at that pid starts unreported."""
        with self._click_lock:
            if any(p not in own for p in self._click_through):
                self._click_through = {p: w for p, w in self._click_through.items() if p in own}
            return dict(self._click_through)

    @staticmethod
    def _decided(check: str, why: str | None) -> str | None:
        """A point check's answer, noted for the call's tool span with the check that decided it (round 4)."""
        call_facts.note("point_checks", check)
        return why

    def _own_point(self, x: float, y: float, pid: int | None = None, window: int | None = None) -> str | None:
        """Blocking: why a real click posted at this screen point, which goes to whatever window takes it there (the
        HID tap: a foreground click, press or scroll), is refused, else None. Input posted to another app's process
        (CGEventPostToPid) can't reach FERB's windows and isn't checked here (round 4, N1). `pid`: the target app;
        `window`: the target window's number when the click is aimed at a screenshot's window.

        The system's hit test first: another app's element there allows it, FERB's refuses it, unless every FERB window
        at the point is one the notch reported click-through (U1: then the window list decides). When the hit test
        can't tell, the on-screen window list at the point decides (`_list_decides`); no list at all, a NULL one
        included, refuses: FERB can't tell (fail closed, rounds 3 and 4: H1, N2). Each check notes which check decided
        ("hit_test", "window_list" or "refused: cant_tell") for the call's span."""
        own = self._own_pids()
        skipped = self._skipped(own)
        hit = getattr(self.ax, "hit", None)
        found = hit(x, y) if callable(hit) else None
        if found is not None and int(found[0]) not in own:
            return self._decided("hit_test", None)
        layered = getattr(self.shots, "windows_at", None)
        windows = layered(x, y) if callable(layered) else None
        if found is not None:
            mine = [w for w in windows or [] if w.pid in own]
            if not mine or any(w.id not in skipped.get(w.pid, frozenset()) for w in mine):
                return self._decided("hit_test", OWN_APP)
        if windows is None:
            return self._decided("refused: cant_tell", CANT_CONFIRM)
        return self._decided("window_list", self._list_decides(windows, pid, window, own, skipped))

    @staticmethod
    def _list_decides(windows: list[Any], pid: int | None, window: int | None, own: Collection[int],
                      skipped: dict[int, frozenset[int]]) -> str | None:
        """The on-screen window list at a point, front to back (round 4, N3): FERB's reported click-through windows and
        windows with alpha 0 take no click and are skipped, and the first window that takes clicks decides. Any other
        FERB window refuses: it may take the click (whatever its alpha: only the notch's report makes one of FERB's
        windows click-through). The target's own window allows: the screenshot's window when the click is aimed at
        one, else the target's normal (level 0) window. A window whose click handling FERB can't know (another app's,
        or another of the target's) decides nothing, so a FERB window under it still refuses. No FERB window: None."""
        for w in windows:
            if w.id in skipped.get(w.pid, frozenset()):
                continue
            if w.pid in own:
                return CANT_CONFIRM
            if w.alpha <= 0:
                continue
            if pid is not None and w.pid == pid and (w.id == window if window is not None else w.layer == 0):
                return None
        return None

    async def _expose(self, a: Any) -> None:
        if a.pid not in self._exposed:
            self._exposed.add(a.pid)
            await asyncio.to_thread(self.ax.expose_tree, a.pid)

    async def menus(self, name_or_bundle: str) -> list[dict[str, Any]]:
        a = await self._resolve(name_or_bundle)
        result: list[dict[str, Any]] = await asyncio.to_thread(self.ax.menus, a.pid)
        return result

    async def press_menu(self, name_or_bundle: str, path: list[str], terminal: TerminalGrant | None = None,
                         task: str | None = None) -> bool:
        a = await self._resolve(name_or_bundle)
        self._armed.pop(a.pid, None)                     # D31: any other input ends a covered Return
        combo = await asyncio.to_thread(self._menu_combo, a, path)
        if combo and (handled := await self._guarded_combo(a, combo, task)) is not None:
            return handled.ok                            # ⌘Q / ⌘H's item: FERB's safe quit/hide (decision 2)
        if terminals.run_menu(path):                     # a menu that runs things in its terminal (finding 2)
            if refusal := await asyncio.to_thread(self._run_refusal, a, terminal, " > ".join(path)):
                raise Refused(refusal)
        elif refusal := await asyncio.to_thread(lambda: self._files_refusal(a, self._selected_files(a), terminal)):
            raise Refused(refusal)                       # a menu command with a .command selected (O1)
        elif await asyncio.to_thread(self._menu_pastes, a, path):        # ⌘V's item pastes into the app's focus (D31)
            focus = await asyncio.to_thread(self._focus, a)
            if await asyncio.to_thread(self._secure, focus):
                raise Refused(SECURE_KEYS)
            if refusal := await asyncio.to_thread(self._terminal_refusal, a, focus, terminal, task, None, "cmd+v"):
                raise Refused(refusal)
        elif await asyncio.to_thread(self._terminal_app, a):          # any command of a terminal app (finding 10)
            focus = await asyncio.to_thread(self._focus, a)
            if refusal := await asyncio.to_thread(self._terminal_refusal, a, focus, terminal, task, None, None,
                                                  " > ".join(path)):
                raise Refused(refusal)
        ok = bool(await asyncio.to_thread(self.ax.press_menu, a.pid, path))
        self.profiles.record(a.bundle_id, "menu_press", ok)
        return ok

    async def ensure_window(self, bundle_id: str, wait_s: float = 10.0) -> bool:
        """Observe launch readiness only: the app shows at least one window Accessibility can reach (any number:
        an app with two windows is ready too). Opening an app's menu/window is a separate agent choice."""
        deadline = time.monotonic() + wait_s
        listed = getattr(self.ax, "ax_windows", None)
        while True:
            a = await asyncio.to_thread(self.apps.find, bundle_id)
            if a is not None:
                await self._expose(a)
                if callable(listed):
                    if await asyncio.to_thread(listed, a.pid):
                        return True
                elif await asyncio.to_thread(self.ax.app_window, a.pid) is not None:
                    return True
            if time.monotonic() >= deadline:
                return False
            await asyncio.sleep(0.25)

    # ---------- the one walk ----------

    async def _walk(self, a: Any, window: Any = None, offset: int = 0) -> axm.AxTree:
        """One Accessibility walk of the app's window, cached per app: the snapshot, the signature before / after an
        action and the password-field scan all read from it."""
        browser = self.is_browser(a.bundle_id)
        if offset:
            tree = await asyncio.to_thread(self.ax.read_tree, a.pid, browser, window, offset)
        elif window is None:
            tree = await asyncio.to_thread(self.ax.read_tree, a.pid, browser)
        else:
            tree = await asyncio.to_thread(self.ax.read_tree, a.pid, browser, window)
        st = self._state.setdefault(a.bundle_id, _AppState())
        if st.pid and st.pid != a.pid:
            st = _AppState(gen=st.gen)
            self._state[a.bundle_id] = st
        st.pid = a.pid
        st.tree, st.tree_at = tree, time.monotonic()
        return tree

    async def _signature(self, a: Any, el: Any = None) -> tuple[Any, ...]:
        """What the window holding `el` shows (with no element, the app's sole window), to tell whether a background
        event did anything (snapshot ids untouched). The element's own window, so an app with two windows compares
        the one the input went to (a walk with no window reads none there)."""
        win = None
        if el is not None:
            holder = getattr(self.ax, "element_window", None)
            win = await asyncio.to_thread(holder, el) if callable(holder) else None
        return (await self._walk(a, win)).signature()

    def _window_signature(self, a: Any, shot: "Shot") -> tuple[Any, ...] | None:
        """Blocking: what the screenshot's own window shows now (found by its bounds and title), or None when that
        window can't be found there (it moved, closed or isn't readable)."""
        find = getattr(self.ax, "window_with_frame", None)
        win = find(a.pid, shot.bounds, shot.title or None) if callable(find) else None
        if win is None:
            return None
        tree = self.ax.read_tree(a.pid, self.is_browser(a.bundle_id), win)
        return tree.signature()  # type: ignore[no-any-return]

    async def snapshot(self, name_or_bundle: str, window: str | None = None, subtree: str | None = None, offset: int = 0) -> tuple[Any, dict[str, Any]]:
        a = await self._resolve(name_or_bundle)
        await asyncio.to_thread(self.browser_now, a.bundle_id)       # the browser list, fresh (Launch Services)
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
        nodes = tree.nodes
        if subtree_node is not None:                     # the paths a whole-window walk would give them (P19)
            base = subtree_node.path
            nodes = [dataclasses.replace(n, path=base + n.path, index=subtree_node.index if not n.path else n.index)
                     for n in nodes]                 # (its window stays the one the last snapshot read)
        else:
            st.window_id = next((w.get("id") for w in tree.windows if w.get("current")), None)
        st.nodes = {f"s{st.gen}e{i + 1}": n for i, n in enumerate(nodes)}
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

    def element_identity(self, bundle_id: str, element: str) -> dict[str, Any] | None:
        """P19's stable identity of a snapshot element (notes/gateway.md target 3): the window the snapshot read (its
        window-server id) and the element's role path, role, subrole and label (`AxNode.identity`), never the
        snapshot-scoped id; None when the id isn't from this app's latest snapshot. In memory, no I/O."""
        st = self._state.get(bundle_id)
        n = st.nodes.get(element) if st else None
        if st is None or n is None or not element.startswith(f"s{st.gen}e"):
            return None
        return {"window": st.window_id, **n.identity()}

    def click_identity(self, bundle_id: str, shot_id: str, x: float, y: float) -> dict[str, Any] | None:
        """P19's stable identity of a click at screenshot pixels: the captured window (its window-server id) and the
        element the screenshot's own walk found under the point (`AxNode.identity`), never the shot id or pixels.
        Where no element of a modest size is under it (a canvas, a web area FERB can't see into), the point in the
        window on a CLICK_GRID-point grid. None when the shot isn't this app's latest. In memory, no I/O."""
        st = self._state.get(bundle_id)
        shot = st.shot if st else None
        if shot is None or shot.id != shot_id or not (0 <= x < shot.size[0] and 0 <= y < shot.size[1]):
            return None
        px, py = shot_to_screen(shot, x, y)
        node = _node_at(shot.nodes, px, py)
        if node is not None and node.rect[2] <= CLICK_ELEMENT_MAX and node.rect[3] <= CLICK_ELEMENT_MAX:
            return {"window": shot.window, **node.identity()}
        return {"window": shot.window, "point": [round((px - shot.bounds[0]) / CLICK_GRID),
                                                 round((py - shot.bounds[1]) / CLICK_GRID)]}

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
        browser = browsers.pair_key(a.bundle_id) if self.is_browser(a.bundle_id) else None
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

    async def screenshot(self, name_or_bundle: str, window: str | None = None, native_resolution: bool = False
                         ) -> tuple[Any, Shot, bytes]:
        """The app's front window only, in memory, with every secret field painted over (L13): the password fields
        the Accessibility tree shows (and, for a browser, the fields the extension marks secret in the tab on screen),
        found before and after the capture (`window_masks`; where they can't be located or placed, the picture is left
        to text recognition, never refused). Withheld only when the boxes can't be painted on it. The capture becomes
        the app's latest shot (mac.click / pointer / record); its `masking` says how a browser page's fields were found
        (Accessibility alone, or not at all) and when the rest is left to text recognition."""
        a, w, jpeg, size, note, tab, tree = await self._capture(name_or_bundle, window, native_resolution)
        st = self._state.setdefault(a.bundle_id, _AppState())
        st.shots += 1
        # The captured window's own walk (never another window's, nor an earlier walk): what the staleness check and
        # P19's identity compare a later click with
        st.shot = Shot(f"shot{st.shots}", w.id, w.bounds, size, pid=a.pid,
                       signature=tree.signature() if tree is not None else None, masking=note, tab=tab,
                       nodes=tuple(tree.nodes) if tree is not None else (), title=w.title)
        return a, st.shot, jpeg

    async def window_image(self, name_or_bundle: str, window: str | None = None) -> tuple[Any, Shot, bytes]:
        """FERB's own picture of the app's front window for the evidence store (D26, D29), or of the exact window id
        `window` (L12: the owner's front window at send): the same capture and masking as `screenshot`, but it
        never becomes the app's latest shot, so a worker's pending mac.click / pointer / record keeps its own. Its
        `tab` is the browser window's active tab as the extension named it, when its title is the window's (B5)."""
        a, w, jpeg, size, note, tab, _ = await self._capture(name_or_bundle, window, False)
        return a, Shot("evidence", w.id, w.bounds, size, pid=a.pid, masking=note, tab=tab, title=w.title), jpeg

    async def page_secrets(self, pid: int, bounds: Rect, title: str = "") -> tuple[list[Rect], Rect, bool] | None:
        """For a browser tab capture whose page the extension couldn't read (B6): the secret fields the Accessibility
        tree of that browser process's window at `bounds` shows inside its page area, that area (screen points), and
        whether every secret field it found has a box. The window is the one showing the tab: picked by the tab's
        `title` among windows at those bounds, and its own title must be the tab's (round 3, m6: a Chromium window is
        named after the tab it shows). None: no such single AX window, the walk can't say, or not exactly one page
        area (docked tools or a split view: which one is the tab can't be told)."""
        a = await self._resolve(f"pid:{pid}")
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

    async def place_on_web_area(self, rect: Rect, viewport: Rect, pid: int, bounds: Rect,
                                title: str = "") -> Rect | None:
        """L33, FERB's cursor: a page element's rect (CSS px, `viewport` the page's visual viewport) in global screen
        points, on the page area of that browser process's window at `bounds` showing the tab titled `title`
        (`page_on_screen`, as captures place a page's secret fields). None whenever unsure: the app is excluded or
        gone, no single window there shows that tab (`window_with_frame`, then its title), its page areas can't be
        read quickly, the page fits none or several equally (a split view), or the spot isn't in that very window on
        screen (minimized, another Space, moved). Accessibility and Quartz work runs off the event loop."""
        def place() -> Rect | None:
            try:
                self.app(f"pid:{pid}")
            except Refused:
                return None
            win = self.ax.window_with_frame(pid, bounds, title or None)
            if win is None or (title and not same_title(self.ax.window_title(win), title)):
                return None
            try:
                placed = page_on_screen([rect], viewport, axm.outermost(self.ax.web_areas_of(win) or []))
            except Withheld:
                return None
            return placed[0] if len(placed) == 1 and shotm.window_on_screen(pid, placed[0], bounds, title or None) \
                else None
        return await asyncio.to_thread(place)

    def secret_fields(self, bundle_id: str) -> list[tuple[float, float, float, float]]:
        """Screen-point boxes of the password fields in the latest walk of this app (after a capture: of the captured
        window), for masking a stored picture."""
        st = self._state.get(bundle_id)
        if st is None or st.tree is None:
            return []
        if st.tree.secret_rects is not None:
            return list(st.tree.secret_rects)
        return [n.rect for n in st.tree.nodes if n.secret]

    async def _capture(self, name_or_bundle: str, window: str | None, native_resolution: bool
                       ) -> tuple[Any, Any, bytes, tuple[int, int], str, tuple[str, str] | None, axm.AxTree | None]:
        a = await self._resolve(name_or_bundle)
        await self._expose(a)                         # an Electron app builds its full tree only when asked
        browser = await asyncio.to_thread(self.browser_now, a.bundle_id)

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
                    return self.shots.capture(w, max_edge=None)  # type: ignore[no-any-return]
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
                page_tab((w.title, await asyncio.to_thread(self._title_now, w)) if browser else (), first, second),
                second[0] if second[0] is not None else first[0])

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

        return self._window_source(a, shot.window)

    def preview_source(self, name: str, *, window: str | None = None,
                       element: str | None = None) -> tuple[dict[str, Any], Any]:
        """Resolve only an explicit window or an observed element's exact containing window."""
        a = self.app(name)
        if window and window.isdigit():
            w = self.shots.window(int(window))
        else:
            if element:
                win = self.ax.element_window(self.node(a.bundle_id, element).ref)
            elif window:
                win = self.ax.window_named(a.pid, window)
            else:
                win = None
            bounds = self.ax.frame(win) if win is not None else None
            if bounds is None:
                raise Refused('preview needs an exact observed window')
            w = self.shots.front_window(a.pid, bounds)
        if w is None or w.pid != a.pid:
            raise Refused('preview window closed or ambiguous')
        return self._window_source(a, w.id)

    def _window_source(self, a: Any, window_id: int) -> tuple[dict[str, Any], Any]:
        async def current() -> Any:
            w = await asyncio.to_thread(self.shots.window, window_id)
            if w is None or w.pid != a.pid or (await self._resolve(a.bundle_id)).pid != a.pid:
                raise Refused('recording window closed or changed owner')
            return w

        async def capture() -> bytes | tuple[bytes, str]:
            w = await current()
            await self._expose(a)
            browser = self.is_browser(a.bundle_id)
            first = await self._look(a, w)
            image, width, height = await asyncio.to_thread(self.shots.capture, w)
            await current()
            masks, note = window_masks(first, await self._look(a, w, first), browser)   # moved: OCR only
            frame = await painted(image, masks, w.bounds, (width, height))
            return (frame, note) if note else frame
        return {'kind': 'window', 'app': a.bundle_id, 'pid': a.pid, 'window_id': window_id}, capture

    async def pointer_at(self, name: str, shot_id: str, x: float, y: float, kind: str,
                         end: tuple[float, float] | None = None, holder: str = "", *,
                         direction: str = "down", amount: int = 1, distance: float = 0.8,
                         terminal: TerminalGrant | None = None, task: str | None = None) -> ActionResult:
        """Exact visual input to an app process. Never activate or take over the global mouse."""
        import math
        a = await self._resolve(name)
        if kind != "hover":
            self._armed.pop(a.pid, None)                 # D31: any other input ends a covered Return
        if refusal := await asyncio.to_thread(self._pointer_refusal, a, shot_id, x, y, kind, end, terminal, task):
            raise Refused(refusal)                       # D31: a paste or a drop into a terminal, unasked
        st = self._state.get(a.bundle_id)
        shot = st.shot if st else None
        if st is None or shot is None or shot.id != shot_id or time.monotonic() - shot.at > SHOT_AGE_S:
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
        if kind in ("click", "dblclick") and \
                (combo := await asyncio.to_thread(lambda: self._item_combo(self._hit(a, shot_id, x, y)))) and \
                (handled := await self._guarded_combo(a, combo, task)) is not None:
            return handled                               # a ⌘Q / ⌘H menu item: FERB's safe quit/hide (decision 2)
        points = [(x, y), end or (x, y)]
        if any(not all(math.isfinite(v) for v in pt) or not (0 <= pt[0] < shot.size[0] and 0 <= pt[1] < shot.size[1]) for pt in points):
            raise Refused("pointer point outside the screenshot")
        bx, by, bw, bh = shot.bounds
        start, finish = [(bx + px * bw / shot.size[0], by + py * bh / shot.size[1]) for px, py in points]
        # Every event goes to this app's process (CGEventPostToPid, or its window through SkyLight), so FERB's own
        # windows can't take it: no point check (round 4, N1). Input for a FERB process is refused (`app()` first).
        if await asyncio.to_thread(self._own_pid, a.pid):
            raise Refused(OWN_APP)

        def window_ok() -> None:
            """Blocking: the screenshot's window is still this app's, where it was, and the owner isn't using it."""
            w = self.shots.window(shot.window)
            if w is None or w.pid != a.pid or not _same(w.bounds, shot.bounds):
                raise StaleRef(WINDOW_GONE)
            if route is None and not self.shots.on_screen(shot.window):
                raise Refused("window-targeted backend unavailable; the legacy pointer route is limited to the current "
                              "Space on this Mac. No foreground switch was made")
            front = self.apps.frontmost()
            if self.input.owner_active() and front and front.pid == a.pid:
                raise Refused("the owner is using this app; pointer input stopped")

        def checked() -> ActionFacts:
            window_ok()
            if why := self._changed_at(a, shot, *start):
                raise StaleRef(why)
            return self._point_facts(a, shot, *start)
        facts = await asyncio.to_thread(checked)
        if kind == "scroll":
            r = await self._pointer_scroll(a, st, shot, start, direction, amount, holder, route, window_ok, distance)
            return dataclasses.replace(r, observed=facts)
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
        now = await asyncio.to_thread(self._window_signature, a, shot)
        changed = now != shot.signature                  # the window gone from where it was is a change too
        if kind != "hover":
            self.profiles.record(a.bundle_id, "pid_pointer_" + kind, changed)
        return ActionResult(True, f"{kind} input delivered to the app process; visible state "
                            f"{'changed' if changed else 'not confirmed changed'}; verify with a fresh screenshot",
                            {'method': 'window_pointer' if route else 'pid_pointer', 'delivered': True,
                             'observed_change': changed, 'outcome_verified': False, 'replayed': False},
                            observed=facts)

    def _changed_at(self, a: Any, shot: Shot, px: float, py: float) -> str | None:
        """Blocking (the staleness check, notes/hands.md: works for apps with any number of windows): why the
        screenshot's window no longer shows what the worker saw at this screen point, else None. The element the
        app's own hit test finds there now is compared with the one the capture's walk of that window found there
        (the same element, one inside it, or one with its role and frame). When either is unknown (a window on
        another Space answers no hit test; nothing was found there), the window's own walk is compared with the
        capture's (`_window_signature`). The window's id and bounds are checked by the caller."""
        then = _node_at(shot.nodes, px, py)
        hit = getattr(self.ax, "hit_element", None)
        now = hit(a.pid, px, py) if callable(hit) and then is not None else None
        if then is not None and now is not None:
            if self._holds(then, now):
                return None
            return ("what is under that point changed since the screenshot (another control is there now); take "
                    "another screenshot")
        if shot.signature is None or self._window_signature(a, shot) != shot.signature:
            return "the app changed since the screenshot; take another screenshot"
        return None

    def _holds(self, then: axm.AxNode, now: Any) -> bool:
        """Blocking: the element a hit test found now is the walk's element, one inside it (a few levels), or one
        with its role at its frame (an app may hand out a new reference for the same control)."""
        parent = getattr(self.ax, "parent_of", None)
        el = now
        for _ in range(8):
            if el is None:
                break
            if self._same_el(el, then.ref):
                return True
            el = parent(el) if callable(parent) else None
        frame = getattr(self.ax, "frame", None)
        rect = frame(now) if callable(frame) else None
        return bool(then.ax_role) and self._role(now) == then.ax_role and rect is not None and _same(rect, then.rect)

    def _point_facts(self, a: Any, shot: Shot, px: float, py: float) -> ActionFacts:
        """Blocking, never raising: the structural facts of pointer input at a screen point of the shot's window (the
        element there now: role, subrole, secure field, its page's address in a browser; the window that takes it)."""
        hit = getattr(self.ax, "hit_element", None)
        try:
            el = hit(a.pid, px, py) if callable(hit) else None
        except Exception:
            el = None
        found = self._element_facts(a, el)
        return ActionFacts(received_by=ReceivedBy(a.bundle_id, shot.title or str(shot.window),
                                                  found.get("target_role")), **found)

    def shot_point(self, name_or_bundle: str, shot_id: str, x: float,
                   y: float) -> tuple[int, int, tuple[float, float]] | None:
        """L33: the app's pid, the shot's window and the global screen point a click at (x, y) of its latest
        screenshot lands on, or None when that shot is unknown or expired, (x, y) is outside it, or the window moved,
        resized, changed owner or closed since (`click_at`'s own check: it would refuse). Synchronous Quartz work."""
        a = self.app(name_or_bundle)
        st = self._state.get(a.bundle_id)
        shot = st.shot if st else None
        if shot is None or shot.id != shot_id or not (0 <= x < shot.size[0] and 0 <= y < shot.size[1]) \
                or time.monotonic() - shot.at > SHOT_AGE_S:
            return None
        now = self.shots.window(shot.window)
        if now is None or now.pid != a.pid or not _same(now.bounds, shot.bounds):
            return None
        return a.pid, shot.window, shot_to_screen(shot, x, y)

    async def click_at(self, name_or_bundle: str, shot_id: str, x: float, y: float, holder: str = "",
                       task: str | None = None, allow_foreground: bool = False, mechanism: str = "pointer",
                       terminal: TerminalGrant | None = None) -> ActionResult:
        """Use one explicit pointer, AX or foreground method for the exact captured window."""
        a = await self._resolve(name_or_bundle)
        self._armed.pop(a.pid, None)                     # D31: any other input ends a covered Return
        if refusal := await asyncio.to_thread(self._pointer_refusal, a, shot_id, x, y, "click", None, terminal, task):
            raise Refused(refusal)                       # D31: a paste into a terminal, unasked
        st = self._state.get(a.bundle_id)
        shot = st.shot if st else None
        if shot is None or shot.id != shot_id:
            raise StaleRef(f"{shot_id} isn't {a.name}'s latest screenshot; take a new one")
        if time.monotonic() - shot.at > SHOT_AGE_S:
            raise StaleRef(f"{shot_id} is more than {SHOT_AGE_S:.0f} s old; take a new screenshot")
        if not (0 <= x < shot.size[0] and 0 <= y < shot.size[1]):
            raise Refused(f"({x}, {y}) is outside the {shot.size[0]}x{shot.size[1]} screenshot")
        if (combo := await asyncio.to_thread(lambda: self._item_combo(self._hit(a, shot_id, x, y)))) and \
                (handled := await self._guarded_combo(a, combo, task)) is not None:
            return handled                               # a ⌘Q / ⌘H menu item: FERB's safe quit/hide (decision 2)
        if mechanism == "pointer":
            return await self.pointer_at(name_or_bundle, shot_id, x, y, "click", holder=holder, terminal=terminal,
                                         task=task)
        if mechanism not in ("ax", "foreground"):
            raise Refused("click mechanism must be pointer, ax or foreground")
        now = await asyncio.to_thread(self.shots.window, shot.window)
        if now is None or now.pid != a.pid or not _same(now.bounds, shot.bounds):
            raise StaleRef("the window moved, resized, changed owner or closed since the screenshot; take a new one")
        px, py = shot_to_screen(shot, x, y)
        # (ax and foreground check what is there right before acting: the hit element bound to the captured window,
        # or the point reaching that window; the content gate of `_changed_at` is the pointer route's)
        if mechanism == "foreground":                    # a real click: whatever window takes it there gets it
            if why := await asyncio.to_thread(self._own_point, px, py, a.pid, shot.window):
                raise Refused(why)                       # a FERB window may take it (rule 8, rule 9)
            # FERB's own app in front (the notch, where the owner just said Yes) is no refusal: `_real` brings the
            # target forward and checks before each event that the target is in front (step 12e, fix 1)
        facts = await asyncio.to_thread(self._point_facts, a, shot, px, py)
        if mechanism == "foreground":
            if not allow_foreground:
                return ActionResult(False, f"no control under ({x:.0f}, {y:.0f}) could be pressed",
                                    {"code": "needs_foreground", "performed": False})
            r = await self._foreground_at(a, px, py, click_steps(px, py), shot.bounds, shot.window, holder, "click",
                                          task)
            return ActionResult(True, f"clicked ({x:.0f}, {y:.0f}) in {shot.id}; {r.detail}", r.facts,
                                observed=facts)
        # ax goes to this app's own element: FERB's windows can't take it (round 4, N1)
        el = await asyncio.to_thread(self.ax.hit_element, a.pid, px, py)
        if el is None:
            return ActionResult(False, "AX hit test unavailable; no pointer input attempted",
                                {"method": "ax", "performed": False}, observed=facts)
        holder_of = getattr(self.ax, "element_window", None)
        win_frame = await asyncio.to_thread(
            lambda: self.ax.frame(holder_of(el)) if callable(holder_of) else None)
        if win_frame is None or not _same(win_frame, shot.bounds):
            return ActionResult(False, "AX hit target cannot be bound to the captured window",
                                {"method": "ax", "performed": False}, observed=facts)
        ok = await asyncio.to_thread(self.ax.press, el)
        return ActionResult(ok, "AX press accepted; inspect the result" if ok else "AX press refused",
                            {"method": "ax", "accepted": ok, "outcome_verified": False}, observed=facts)

    # ---------- the foreground, last resort, only by request: one path ----------

    async def _foreground_at(self, a: Any, x: float, y: float, steps: list[Step], bounds: Rect, window: int,
                             holder: str, what: str, task: str | None) -> ActionResult:
        """Real input at a screen point (a click, a wheel), the one foreground path (notes/hands.md): the app is
        brought forward and its window raised (`_bring_forward`), and right before each event FERB checks that no
        FERB window would take it (`_own_point`), that the window is still where it was, and that the point reaches
        that window, not another one over it. Any of those failing: nothing more is posted."""
        def reaches() -> bool:
            h = getattr(self.ax, "hit", None)
            found = h(x, y) if callable(h) else None      # the system's hit test: click-through overlays don't count
            if found is not None:
                return bool(found[0] == a.pid and found[1] is not None and _same(found[1], bounds))
            top = self.shots.window_at(x, y)
            return top is not None and top.id == window

        def guarded(step: Step) -> Step:
            def post() -> None:
                if why := self._own_point(x, y, a.pid, window):
                    raise Refused(why)
                w = self.shots.window(window)
                if w is None or not _same(w.bounds, bounds) or not reaches():
                    raise StaleRef("another window is over that point now, or the window moved; take a new look")
                step.post()
            return Step(step.what, post)
        if why := await asyncio.to_thread(self._own_point, x, y, a.pid, window):
            raise Refused(why)
        return await self._real(a, [guarded(s) for s in steps], holder, what, task,
                                lambda: self._bring_forward(a, bounds, window))

    def _element_center(self, el: Any, fallback: tuple[float, float]) -> tuple[float, float]:
        """Blocking: the centre of the element's frame now (else `fallback`, from the snapshot), where a real event
        for it goes."""
        frame = getattr(self.ax, "frame", None)
        now = frame(el) if callable(frame) else None
        if now is None:
            return fallback
        x, y, w, h = now
        return x + w / 2, y + h / 2

    def _element_window(self, a: Any, el: Any) -> tuple[Rect, int]:
        """Blocking: the frame and window-server id of the window holding the element. Refused when FERB can't tell
        (no window, or several at one place with the same title): nothing is brought forward then."""
        frame = getattr(self.ax, "frame", None)
        holder = getattr(self.ax, "element_window", None)
        win = holder(el) if callable(holder) else None
        bounds = frame(win) if callable(frame) and win is not None else None
        title = getattr(self.ax, "window_title", lambda w: "")(win) if win is not None else ""
        found = self.shots.front_window(a.pid, bounds, title or None) if bounds is not None else None
        if bounds is None or found is None:
            raise Refused("FERB can't tell which window holds that control, so it brought nothing forward; take a new "
                          "snapshot, or use mac.screenshot and mac.click")
        return bounds, int(found.id)

    async def _foreground_element(self, a: Any, el: Any, fallback: tuple[float, float],
                                  steps: Callable[[float, float], list[Step]], holder: str, what: str,
                                  task: str | None) -> ActionResult:
        """A real event at an element's centre (a click, a wheel), on the one foreground path (`_foreground_at`):
        refused first when a FERB window would take it there (fix round 3, H1), then its window found and raised."""
        x, y = await asyncio.to_thread(self._element_center, el, fallback)
        if why := await asyncio.to_thread(self._own_point, x, y, a.pid):
            raise Refused(why)
        bounds, window = await asyncio.to_thread(self._element_window, a, el)
        return await self._foreground_at(a, x, y, steps(x, y), bounds, window, holder, what, task)

    # ---------- terminals (D31, rule 10) ----------

    def _focus(self, a: Any) -> Any:
        """Blocking: the app's keyboard focus element now, or None when Accessibility can't say."""
        focused = getattr(self.ax, "focused_element", None)
        return focused(a.pid) if callable(focused) else None

    def _same_el(self, x: Any, y: Any) -> bool:
        same = getattr(self.ax, "same", None)
        return bool(same(x, y)) if callable(same) else x is not None and x == y

    def _terminal_why(self, a: Any, el: Any) -> str | None:
        """Blocking: why code counts this element, the one input would reach, a terminal (policy/terminal.py), else
        None. No element (the focus can't be read) or no facts for it, in an app that can host a terminal, counts as
        one: FERB asks rather than type unchecked (fails closed)."""
        facts = getattr(self.ax, "surface_facts", None)
        chain = facts(el) if el is not None and callable(facts) else []
        shell = getattr(self.apps, "declares_shell", None)
        return terminals.surface(a.bundle_id, chain, self.terminal_apps(),
                                 bool(shell(a.bundle_id)) if callable(shell) else False, unread=not chain)

    def _app_terminal_why(self, a: Any) -> str | None:
        """Blocking: why code counts the app itself a terminal (its bundle id, its Info.plist, Zed), else None."""
        shell = getattr(self.apps, "declares_shell", None)
        return terminals.surface(a.bundle_id, [], self.terminal_apps(),
                                 bool(shell(a.bundle_id)) if callable(shell) else False)

    def _hosts(self, a: Any) -> bool:
        """Blocking: the app can hold a terminal somewhere (policy/terminal.py `can_host`)."""
        shell = getattr(self.apps, "declares_shell", None)
        return terminals.can_host(a.bundle_id, self.terminal_apps(),
                                  bool(shell(a.bundle_id)) if callable(shell) else False)

    def _run_facts(self, a: Any) -> tuple[str | None, str | None]:
        """Blocking (review round 2, N2): what a run press would run, as the editor holds it now: the active editor's
        selected text (its text element, not whatever has the focus; VS Code marks its active editor group) and its
        file. Without an active editor FERB can read, the focused element if it is a text field that isn't a
        terminal. (None, file) when FERB can't read the selection: the question says so."""
        active = getattr(self.ax, "active_editor", None)
        editor = active(a.pid) if callable(active) else None
        if editor is None:
            focus = self._focus(a)
            editor = focus if focus is not None and self._terminal_why(a, focus) is None and \
                self._role(focus) in ("AXTextArea", "AXTextField") else None
        selected = getattr(self.ax, "selected_text", None)
        document = getattr(self.ax, "document_of", None)
        return (selected(editor) if callable(selected) and editor is not None else None,
                document(editor) if callable(document) and editor is not None else None)

    def _run_input(self, a: Any, menu: str | None = None, control: str | None = None,
                   why: str = "", keys: str | None = None) -> TerminalInput:
        """Blocking (review rounds 1 and 2, finding 2): a press that runs code, with what it would run (`_run_facts`)
        and where: the app's terminal for its Terminal (or Console, Shell) menu, else the app. `keys`: the key press
        that triggers it, which the Yes is bound to (round 3, L1: it goes on to the focus, a terminal or not)."""
        selection, file = self._run_facts(a)
        where = f"{a.name}'s terminal" if menu and terminals.TERMINAL_MENUS.match(menu.split(" > ")[0]) else a.name
        return TerminalInput(a.name, a.bundle_id, why or f"a menu or control that runs code in {where}",
                             keys=keys, menu=menu, control=control, runs=True, selection=selection, file=file,
                             runs_where=where)

    def _run_refusal(self, a: Any, grant: TerminalGrant | None, menu: str | None = None,
                     control: str | None = None) -> str | None:
        """Blocking, right before a run press: None when the owner's Yes names exactly this menu item or control and
        the editor still holds what the question showed (N3: the selection and file re-read now)."""
        g = grant.input if grant is not None and grant.input.bundle_id == a.bundle_id else None
        if g is not None and g.runs and ((menu is not None and g.menu == menu) or
                                         (control is not None and g.control == control)):
            if self._run_facts(a) == (g.selection, g.file):
                return None
            return ("what this would run changed since the owner's Yes (the editor's selection or file), so nothing "
                    "was pressed; call it again and FERB asks with what it would run now (D31)")
        return terminals.UNASKED.format(why=f"a menu or control that runs code ({menu or control})", app=a.name)

    def _runs(self, a: Any, el: Any) -> str | None:
        """Blocking: why pressing this element runs code (policy/terminal.py `run_control`; a label counts only in
        an editor app, or in a window that hosts a terminal by structure: N6, round 3), else None."""
        facts = getattr(self.ax, "surface_facts", None)
        if el is None or not callable(facts):
            return None
        chain = facts(el)
        if why := terminals.run_control(chain, terminals.editor_app(a.bundle_id)):
            return why
        labelled = terminals.run_control(chain, True)
        return labelled if labelled and self._hosts_terminal(el) else None

    def _hosts_terminal(self, el: Any) -> bool:
        """Blocking: the element's window holds an element that is a terminal by its structure (round 3, N6). The walk
        has a budget (round 4, N6: `WINDOW_WALK_NODES` elements and `WINDOW_WALK_S`); past it, or cut short by the
        reader (None), FERB can't tell, which counts as hosting one: the press asks, it never goes unasked."""
        walk = getattr(self.ax, "window_facts", None)
        if not callable(walk):
            return False
        deadline = time.monotonic() + WINDOW_WALK_S
        for read, facts in enumerate(walk(el), 1):
            if facts is None or read > WINDOW_WALK_NODES or time.monotonic() > deadline:
                return True
            if terminals.structural(facts):
                return True
        return False

    @staticmethod
    def _shortcut(parts: list[str]) -> bool:
        """A key an app's menus may carry as a command's key equivalent: one with a modifier, or a function key."""
        return len(parts) > 1 or bool(re.fullmatch(r"f\d{1,2}", parts[-1] if parts else ""))

    def _shortcut_menu(self, a: Any, parts: list[str]) -> str | None:
        """Blocking (round 2, 2a): the menu path a key press triggers (the app's own key equivalent), else None."""
        lookup = getattr(self.ax, "menu_item_for_shortcut", None)
        if not parts or not self._shortcut(parts) or not callable(lookup):
            return None
        found = lookup(a.pid, parts[-1], parts[:-1])
        return str(found[1]) if found else None

    def _key_run(self, a: Any, combo: str) -> tuple[str | None, str | None]:
        """Blocking: the run command a key would trigger: (the menu path its key equivalent presses, None), or (None,
        the focused run control's label) for Space or Return on one (2c); else (None, None)."""
        parts = normalize(combo)
        path = self._shortcut_menu(a, parts)
        if path and terminals.run_menu(path.split(" > ")):
            return path, None
        if parts in (["space"], ["return"]):
            focus = self._focus(a)
            if focus is not None and self._runs(a, focus):
                return None, str(getattr(self.ax, "place_of", lambda el: ("", ""))(focus)[1] or "a run control")
        return None, None

    def _terminal_app(self, a: Any) -> bool:
        """Blocking: a terminal by the app alone (its bundle id, its Info.plist, Zed): there every menu command can
        reach its terminal (review round 1, finding 10)."""
        return self._app_terminal_why(a) is not None

    def _menu_pastes(self, a: Any, path: list[str]) -> bool:
        shortcut = getattr(self.ax, "menu_item_shortcut", None)
        return _pastes_item(shortcut(a.pid, path)) if callable(shortcut) else False

    def _item_pastes(self, el: Any) -> bool:
        shortcut = getattr(self.ax, "item_shortcut", None)
        return _pastes_item(shortcut(el)) if callable(shortcut) else False

    def _paste_input(self, a: Any, why: str) -> TerminalInput:
        count, text, concealed = self.clipboard()
        return TerminalInput(a.name, a.bundle_id, why, text=text, paste=True, concealed=concealed, clipboard=count,
                             enter=text is not None and not terminals.newline(text))

    def _placed(self, inp: TerminalInput | None, el: Any) -> TerminalInput | None:
        """The input with where it goes, for the owner's question: the element's window title and its own label."""
        place = getattr(self.ax, "place_of", None)
        if inp is None or el is None or not callable(place):
            return inp
        window, label = place(el)
        return dataclasses.replace(inp, window=str(window or ""), label=str(label or ""))

    def terminal_input(self, app: str, action: ComputerAction, task: str | None = None) -> TerminalInput | None:
        """Blocking (the gateway runs it in a thread, before it asks the owner): the input this action would put into
        a terminal, by code's facts now, else None. Typing, a value or an edit: the element it names; a key: the app's
        keyboard focus (⌘V: the clipboard's text goes in); pressing a menu item whose key equivalent is ⌘V: the focus.
        A Return into the terminal element FERB just typed approved text into is covered by that Yes (`covered_by`)."""
        a = self.app(app)
        if action.kind in ("type", "set_value", "edit") and action.text is not None and action.element:
            ref = self.node(a.bundle_id, action.element).ref
            why = self._terminal_why(a, ref)
            return None if why is None else self._placed(TerminalInput(
                a.name, a.bundle_id, why, text=action.text, enter=not terminals.newline(action.text)), ref)
        pastes, item = False, None
        if action.kind in ("press", "click") and action.element:
            n = self.node(a.bundle_id, action.element)
            if opened := self._files_input(a, [self._element_file(n.ref)]):   # a .command item (round 2, O1)
                return opened
            if why_run := self._runs(a, n.ref):           # a run control (rounds 1 and 2, finding 2)
                return self._run_input(a, control=n.label, why=why_run)
            pastes = n.ax_role == "AXMenuItem" and self._item_pastes(n.ref)
            item = n.label if n.ax_role == "AXMenuItem" and not pastes and self._terminal_app(a) else None
            if not pastes and item is None:
                return None
        elif action.kind != "key" or not action.text or not valid_combo(action.text):
            return None
        elif opened := self._files_input(a, self._selected_files(a)):        # a key with a .command selected (O1)
            return opened
        if action.kind == "key":                           # a key that presses a run command (round 2, 2a, 2c)
            path, control = self._key_run(a, action.text or "")
            if path or control:
                return self._run_input(a, menu=path, control=control, keys="+".join(normalize(action.text or "")))
        focus = self._focus(a)
        why = self._terminal_why(a, focus)
        if why is None:
            return None
        if item is not None:
            return self._placed(TerminalInput(a.name, a.bundle_id, why, menu=item), focus)
        parts = normalize(action.text or "")
        if pastes or _pastes(parts):
            return self._placed(self._paste_input(a, why), focus)
        armed = self._armed.get(a.pid)
        if parts == ["return"] and armed is not None and armed.task == task and self._same_el(armed.el, focus):
            return self._placed(TerminalInput(a.name, a.bundle_id, why, keys="return",
                                              covered_by=armed.confirmation_id), focus)
        return self._placed(TerminalInput(a.name, a.bundle_id, why, keys="+".join(parts)), focus)

    def _script_why(self, a: Any) -> str | None:
        """Blocking: why a script for this app needs the owner's Yes: a terminal app, or an app that runs code when
        told to (System Events, Finder, Script Editor, Automator, Shortcuts: policy/terminal.py `CODE_RUNNERS`)."""
        return terminals.CODE_RUNNERS.get(a.bundle_id.casefold()) or self._app_terminal_why(a)

    @staticmethod
    def _script_paths(script: str) -> list[str]:
        """The existing files a script names in quotes (a POSIX path or one under ~): their facts go in its question."""
        found = []
        for literal in re.findall(r'"([^"\n]+)"', script):
            path = os.path.expanduser(literal)
            if path.startswith("/") and os.path.isfile(path):
                found.append(path)
        return found

    def script_terminal_input(self, app: str, script: str) -> TerminalInput | None:
        """Blocking: an AppleScript for a terminal app or an app that runs code (D31 review rounds 1 and 2; the
        sandbox lets a script reach only the app it names): the script, with each file it names that opens in a
        terminal app (path, hash, text), else None."""
        a = self.app(app)
        why = self._script_why(a)
        if why is None:
            return None
        paths = self._script_paths(script)
        files = self._open_files(paths)
        opened = {p for p, _, _ in files}
        return TerminalInput(a.name, a.bundle_id, why, script=script, files=files,
                             named=tuple(p for p in dict.fromkeys(paths) if p not in opened),   # said plainly (N7)
                             opens=a.bundle_id.casefold() in terminals.OPENS_FILES)

    def script_refusal(self, a: Any, script: str, grant: TerminalGrant | None) -> str | None:
        """Blocking, right before the script runs: None unless its app needs the owner's Yes to exactly it (and to the
        exact contents of each file it names that opens in a terminal)."""
        why = self._script_why(a)
        if why is None:
            return None
        if grant is not None and grant.input.bundle_id == a.bundle_id and grant.input.script == script and \
                self._files_covered(self._script_paths(script), grant):
            return None
        return terminals.UNASKED.format(why=why, app=a.name)

    # ---- files that open in a terminal app (review round 2, O1)

    def _opens_in_terminal(self, path: str) -> bool:
        """Blocking: Launch Services would open this file with a terminal app (a .command in Terminal)."""
        default = getattr(self.apps, "default_app_for", None)
        bundle = default(path) if callable(default) else None
        if not bundle:
            return False
        shell = getattr(self.apps, "declares_shell", None)
        return terminals.surface(bundle, [], self.terminal_apps(),
                                 bool(shell(bundle)) if callable(shell) else False) is not None

    @staticmethod
    def _file_facts(path: str) -> tuple[str, str, str]:
        """(path, sha256 of its contents, its text as the owner should read it; a binary file says so)."""
        try:
            data = open(path, "rb").read()
        except OSError:
            return path, "", "(a file FERB can't read)"
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            text = f"(a binary file of {len(data)} bytes)"
        return path, hashlib.sha256(data).hexdigest(), text

    def _open_files(self, paths: Collection[str | None]) -> tuple[tuple[str, str, str], ...]:
        """Blocking: the facts of each file among `paths` that opens in a terminal app."""
        return tuple(self._file_facts(p) for p in dict.fromkeys(p for p in paths if p) if self._opens_in_terminal(p))

    def _files_covered(self, paths: Collection[str | None], grant: TerminalGrant | None) -> bool:
        """Blocking: every file among `paths` that opens in a terminal app is in the owner's Yes, with these exact
        contents (re-hashed now)."""
        need = self._open_files(paths)
        approved = {(p, h) for p, h, _ in grant.input.files} if grant is not None else set()
        return all((p, h) in approved for p, h, _ in need)

    def _element_file(self, el: Any) -> str | None:
        file_of = getattr(self.ax, "file_of", None)
        return file_of(el) if callable(file_of) and el is not None else None

    def _selected_files(self, a: Any) -> list[str | None]:
        """Blocking: the files selected in the app's focused container (Finder's selection), for a key or menu."""
        items = getattr(self.ax, "selected_items", None)
        return [self._element_file(el) for el in (items(a.pid) if callable(items) else [])]

    def _files_input(self, a: Any, paths: Collection[str | None]) -> TerminalInput | None:
        files = self._open_files(paths)
        return None if not files else TerminalInput(a.name, a.bundle_id, "a file that opens in a terminal app",
                                                    files=files)

    def _files_refusal(self, a: Any, paths: Collection[str | None], grant: TerminalGrant | None) -> str | None:
        """Blocking, right before input that can open these files: None unless one opens in a terminal app and the
        owner's Yes doesn't name it with these exact contents."""
        if not self._open_files(paths):
            return None
        if grant is not None and grant.input.bundle_id == a.bundle_id and self._files_covered(paths, grant):
            return None
        return terminals.UNASKED.format(why="a file that opens in a terminal app and runs", app=a.name)

    def _opens_to_run(self, bundle_id: str, path: str) -> tuple[Any, tuple[tuple[str, str, str], ...]]:
        """Blocking (step 12e fix 5, `mac.open` with a file): the app as the question names it, and the file's facts
        when opening it there can run it: the app itself is a terminal (a script opened in Terminal runs), or it opens
        files with their default apps (Finder: a .command runs in Terminal) and that default is a terminal app. Else
        no files."""
        found = self.apps.find(bundle_id)
        a = found if found is not None else SimpleNamespace(bundle_id=bundle_id, name=bundle_id)
        runs = self._app_terminal_why(a) is not None or (bundle_id.casefold() in terminals.OPENS_FILES and
                                                         self._opens_in_terminal(path))
        return a, ((self._file_facts(path),) if runs else ())

    def open_terminal_input(self, bundle_id: str, path: str) -> TerminalInput | None:
        """Blocking: opening this file in this app runs it in a terminal (`_opens_to_run`): the file, with its exact
        contents, for the owner's question; else None."""
        a, files = self._opens_to_run(bundle_id, path)
        return TerminalInput(a.name, a.bundle_id, "a file that opens in a terminal app", files=files) if files else None

    def open_refusal(self, bundle_id: str, path: str, grant: TerminalGrant | None) -> str | None:
        """Blocking, right before the open: None unless the file would run in a terminal and the owner's Yes doesn't
        name it with these exact contents (re-hashed now)."""
        a, files = self._opens_to_run(bundle_id, path)
        if not files:
            return None
        approved = {(p, h) for p, h, _ in grant.input.files} if grant is not None and \
            grant.input.bundle_id == a.bundle_id else set()
        if all((p, h) in approved for p, h, _ in files):
            return None
        return terminals.UNASKED.format(why="a file that opens in a terminal app and runs", app=a.name)

    def _hit(self, a: Any, shot_id: str, x: float, y: float) -> Any:
        """Blocking: the element under (x, y) of the app's latest screenshot, or None."""
        st = self._state.get(a.bundle_id)
        shot = st.shot if st else None
        hit = getattr(self.ax, "hit_element", None)
        if shot is None or shot.id != shot_id or not callable(hit):
            return None
        return hit(a.pid, *shot_to_screen(shot, x, y))

    def _role(self, el: Any) -> str:
        role = getattr(self.ax, "role_of", None)
        return str(role(el)) if el is not None and callable(role) else ""

    def browser_page(self, app: str, action: ComputerAction) -> str | None:
        """Blocking (live 2026-10-26: a worker switched off the owner's extension on chrome://extensions): the
        browser's own page this input would act on (policy/confirm.py `browser_page`), else None. An address typed in;
        else the element's page, or the keyboard focus's for input without an element, by the address the browser
        reports. The gateway asks the owner about every such input."""
        a = self.app(app)
        if not self.browser_now(a.bundle_id):
            return None
        if action.kind in ("type", "set_value", "edit") and floors.browser_page(action.text):
            return str(action.text).strip()
        if action.element:
            el = self.node(a.bundle_id, action.element).ref
        elif action.kind in ("type", "key", "edit"):
            el = self._focus(a)
        else:
            return None
        return self._browser_page_at(el)

    def pointer_browser_page(self, app: str, shot_id: str, x: float, y: float) -> str | None:
        """Blocking: the browser's own page under a screenshot point, else None (see `browser_page`)."""
        a = self.app(app)
        if not self.browser_now(a.bundle_id):
            return None
        return self._browser_page_at(self._hit(a, shot_id, x, y))

    def _browser_page_at(self, el: Any) -> str | None:
        find = getattr(self.ax, "page_url", None)
        url = find(el) if el is not None and callable(find) else None
        return str(url) if floors.browser_page(url) else None

    def pointer_terminal_input(self, app: str, shot_id: str, x: float, y: float, kind: str,
                               end: tuple[float, float] | None = None) -> TerminalInput | None:
        """Blocking (review round 1, finding 4): what pointer input would put into a terminal: a click on a menu item
        that pastes (⌘V) while a terminal has the focus (a context menu opened over it), or any menu item of a
        terminal app; a drag whose drop point is a terminal, with the text selected where it begins. Else None."""
        a = self.app(app)
        if kind == "dblclick" and (opened := self._files_input(a, [self._element_file(self._hit(a, shot_id, x, y))])):
            return opened                                # a double-click opens the file (round 2, O1)
        if kind in ("click", "dblclick"):
            item = self._hit(a, shot_id, x, y)
            if item is not None and (why_run := self._runs(a, item)):       # a run control (finding 2)
                title = getattr(self.ax, "place_of", lambda el: ("", ""))(item)[1]
                return self._run_input(a, control=str(title or "a run button"), why=why_run)
            if item is None or self._role(item) != "AXMenuItem":
                return None
            pastes = self._item_pastes(item)
            if not pastes and not self._terminal_app(a):
                return None
            focus = self._focus(a)
            why = self._terminal_why(a, focus)
            if why is None:
                return None
            if pastes:
                return self._placed(self._paste_input(a, why), focus)
            title = getattr(self.ax, "place_of", lambda el: ("", ""))(item)[1]
            return self._placed(TerminalInput(a.name, a.bundle_id, why, menu=str(title or "a menu item")), focus)
        if kind == "drag" and end is not None:
            target = self._hit(a, shot_id, *end)
            why = self._terminal_why(a, target)
            if why is None:
                return None
            selected = getattr(self.ax, "selected_text", None)
            source = self._hit(a, shot_id, x, y)
            text = selected(source) if callable(selected) and source is not None else None
            return self._placed(TerminalInput(a.name, a.bundle_id, why, text=text, drag=True), target)
        return None

    def _pointer_refusal(self, a: Any, shot_id: str, x: float, y: float, kind: str,
                         end: tuple[float, float] | None, grant: TerminalGrant | None, task: str | None) -> str | None:
        """Blocking, right before pointer input goes out: None unless it would put something into a terminal that
        the owner's Yes doesn't name (`pointer_terminal_input`'s cases)."""
        if kind == "dblclick" and (refusal := self._files_refusal(a, [self._element_file(self._hit(a, shot_id, x, y))],
                                                                  grant)):
            return refusal                               # a double-click opens the file (round 2, O1)
        if kind in ("click", "dblclick"):
            item = self._hit(a, shot_id, x, y)
            if item is not None and self._runs(a, item):
                title = getattr(self.ax, "place_of", lambda el: ("", ""))(item)[1]
                return self._run_refusal(a, grant, control=str(title or "a run button"))
            if item is None or self._role(item) != "AXMenuItem":
                return None
            focus = self._focus(a)
            if self._item_pastes(item):
                return self._terminal_refusal(a, focus, grant, task, keys="cmd+v")
            if self._terminal_app(a):
                title = getattr(self.ax, "place_of", lambda el: ("", ""))(item)[1]
                return self._terminal_refusal(a, focus, grant, task, menu=str(title or "a menu item"))
            return None
        if kind == "drag" and end is not None:
            target = self._hit(a, shot_id, *end)
            why = self._terminal_why(a, target)
            if why is None or (grant is not None and grant.input.bundle_id == a.bundle_id and grant.input.drag):
                return None
            return terminals.UNASKED.format(why=why, app=a.name)
        return None

    def secret_terminal_input(self, app: str, element: str, name: str) -> TerminalInput | None:
        """Blocking: filling a hidden secret into this element puts it into a terminal (D31, review round 1 finding
        7: by the element's own facts, not the app's bundle id alone): the fill, named, else None."""
        a = self.app(app)
        ref = self.node(a.bundle_id, element).ref
        why = self._terminal_why(a, ref)
        return None if why is None else self._placed(TerminalInput(a.name, a.bundle_id, why, secret=name), ref)

    def secret_refusal(self, a: Any, el: Any, grant: TerminalGrant | None) -> str | None:
        """Blocking, before and again right before a secret fill: None unless the element is a terminal the owner's
        Yes to this fill doesn't cover."""
        why = self._terminal_why(a, el)
        if why is None or (grant is not None and grant.input.bundle_id == a.bundle_id and grant.input.secret):
            return None
        return terminals.UNASKED.format(why=why, app=a.name)

    def menu_terminal_input(self, app: str, path: list[str]) -> TerminalInput | None:
        """Blocking: pressing this menu item pastes into a terminal (its key equivalent is ⌘V and the app's keyboard
        focus is a terminal): the paste, else None."""
        a = self.app(app)
        if terminals.run_menu(path):                     # a menu that runs things in its terminal (finding 2)
            return self._run_input(a, menu=" > ".join(path),
                                   why=f"a menu that runs things in {a.name}'s terminal ({path[0]})")
        if opened := self._files_input(a, self._selected_files(a)):        # a menu command with a .command selected
            return opened
        pastes = self._menu_pastes(a, path)
        if not pastes and not self._terminal_app(a):
            return None
        focus = self._focus(a)
        why = self._terminal_why(a, focus)
        if why is None:
            return None
        if pastes:
            return self._placed(self._paste_input(a, why), focus)
        return self._placed(TerminalInput(a.name, a.bundle_id, why, menu=" > ".join(path)), focus)

    def _terminal_refusal(self, a: Any, el: Any, grant: TerminalGrant | None, task: str | None,
                          text: str | None = None, keys: str | None = None, menu: str | None = None) -> str | None:
        """Blocking, right before input is delivered: None when it may go to `el` (no terminal there, or the owner's
        Yes names exactly this input), else the refusal. A Return is covered only in the element the approved text
        went into, by the same task; a paste only while the clipboard holds what the owner was shown."""
        why = self._terminal_why(a, el)
        if why is None:
            return None
        if grant is not None and grant.input.bundle_id == a.bundle_id:
            g, parts = grant.input, normalize(keys) if keys else []
            if menu is not None:
                if g.menu == menu:
                    return None
            elif parts and _pastes(parts):
                if g.paste and self.clipboard()[0] == g.clipboard:
                    return None
            elif parts == ["return"] and g.covered_by:
                armed = self._armed.get(a.pid)
                if (armed is not None and armed.confirmation_id == g.covered_by == grant.confirmation_id
                        and armed.task == task and el is not None and self._same_el(armed.el, el)):
                    return None
            elif parts:
                if not g.paste and not g.covered_by and g.keys is not None and normalize(g.keys) == parts:
                    return None
            elif text is not None and not g.paste and g.text == text:
                return None
        return terminals.UNASKED.format(why=why, app=a.name)

    def _terminal_typed(self, a: Any, el: Any, grant: TerminalGrant | None, task: str | None, delivered: bool) -> None:
        """After input reached a terminal: a delivered approved text arms its one Return there; anything else (a key,
        the Return itself) disarms the process."""
        self._armed.pop(a.pid, None)
        if delivered and grant is not None and grant.input.enter and el is not None:
            self._armed[a.pid] = _Armed(el, grant.confirmation_id, task)

    async def _terminal_check(self, a: Any, el: Any, terminal: TerminalGrant | None, task: str | None,
                              text: str | None = None, keys: str | None = None, menu: str | None = None) -> bool:
        """Refuses input that would reach a terminal without the owner's Yes to it; True when `el` is a terminal."""
        refusal = await asyncio.to_thread(self._terminal_refusal, a, el, terminal, task, text, keys, menu)
        if refusal is not None:
            raise Refused(refusal)
        return terminal is not None and await asyncio.to_thread(self._terminal_why, a, el) is not None

    # ---------- facts (gateway decision 1) and the key-combination guard (decision 2) ----------

    def _secure(self, el: Any) -> bool:
        """Blocking: the element is a secure (password-class) text field by its own facts (never its value)."""
        check = getattr(self.ax, "secure_element", None)
        if el is None or not callable(check):
            return False
        try:
            return bool(check(el))
        except Exception:
            return False

    def _element_facts(self, a: Any, el: Any) -> dict[str, Any]:
        """Blocking, never raising: an element's structural facts for ActionFacts: its role and subrole, whether it is
        a secure field, and in a browser the address of the page it is in. Empty for no element."""
        out: dict[str, Any] = {}
        if el is None:
            return out
        try:
            role = self._role(el)
            sub = getattr(self.ax, "subrole_of", None)
            subrole = str(sub(el) or "") if callable(sub) else ""
            out = {"target_role": role or None, "target_subrole": subrole or None, "secure_field": self._secure(el)}
            find = getattr(self.ax, "page_url", None)
            if self.is_browser(a.bundle_id) and callable(find):
                url = find(el)
                out["page_url"] = str(url) if url else None
        except Exception:
            pass
        return out

    def _received(self, a: Any, el: Any) -> ReceivedBy:
        """Blocking, never raising: what received input: the app, the element's window title and its role."""
        try:
            window, _ = getattr(self.ax, "place_of", lambda e: ("", ""))(el) if el is not None else ("", "")
            return ReceivedBy(a.bundle_id, str(window or "") or None, self._role(el) or None)
        except Exception:
            return ReceivedBy(a.bundle_id)

    async def _facts(self, a: Any, target: Any, received: Any = None, terminal: bool | None = None,
                     after: bool = False) -> ActionFacts:
        """The structural facts of one input (gateway decision 1): the target element's (`_element_facts`), what
        received it (`received`; with `after`, the app's keyboard focus read after the input, where keystrokes went)
        and whether it reached a terminal. Read off the event loop; never fails the action."""
        def read() -> ActionFacts:
            found = self._element_facts(a, target)
            try:
                got = self._focus(a) if after else received
            except Exception:
                got = None
            return ActionFacts(received_by=self._received(a, got if got is not None else target),
                               terminal_target=terminal, **found)
        return await asyncio.to_thread(read)

    @staticmethod
    def _with(r: ActionResult, facts: ActionFacts | None) -> ActionResult:
        return dataclasses.replace(r, observed=facts) if facts is not None and r.observed is None else r

    def _menu_combo(self, a: Any, path: list[str]) -> str | None:
        """Blocking: the key equivalent of the menu item at `path`, as a combo ("cmd+q"), else None."""
        shortcut = getattr(self.ax, "menu_item_shortcut", None)
        found = shortcut(a.pid, path) if callable(shortcut) else None
        return shortcut_combo(*found) if found else None

    def _item_combo(self, el: Any) -> str | None:
        """Blocking: a menu item element's key equivalent, as a combo, else None (not a menu item, or none)."""
        if el is None or self._role(el) != "AXMenuItem":
            return None
        shortcut = getattr(self.ax, "item_shortcut", None)
        found = shortcut(el) if callable(shortcut) else None
        return shortcut_combo(*found) if found else None

    async def _guarded_combo(self, a: Any, combo: str, task: str | None) -> ActionResult | None:
        """Owner decision 2 (minimal shortcut guard), whatever delivers the combination (a key, its menu item by AX or
        a click): a screen-wide one is refused; ⌘Q and ⌘H go through FERB's safe quit / hide (launched.py: only an
        app FERB opened for this task, and quit only without unsaved work), which is the result; ⌥⌘H (hide others,
        the owner's apps) is refused. Any other combination: None, delivered as asked (in-app shortcuts unchanged)."""
        if screen_wide(combo):
            raise Refused(SCREEN_WIDE)
        what = quit_or_hide(combo)
        if what is None:
            return None
        if what == "hide_others":
            raise Refused("⌥⌘H hides every other app, the owner's included, so FERB doesn't press it (notes/hands.md "
                          "decision 2)")
        if self.launched is None or task is None:
            raise Refused(f"FERB quits or hides only apps it opened for the task, and it can't tell that here, so it "
                          f"didn't send {combo} to {a.name}")
        if what == "quit":
            detail = await self.launched.quit_now(task, a)
        else:
            detail = await self.launched.hide_now(task, a)
        return ActionResult(True, detail, {"method": "ferb_" + what, "delivered": False, "outcome_verified": False},
                            observed=ActionFacts(received_by=ReceivedBy(a.bundle_id)))

    # ---------- acting ----------

    async def act(self, app: str, action: ComputerAction, holder: str = "", task: str | None = None,
                  terminal: TerminalGrant | None = None) -> ActionResult:
        if action.kind in {"key", "type", "set_value", "select"} and action.text is None:
            raise Refused(f"{action.kind} requires explicit text; omitted text never means clear or Return")
        a = await self._resolve(app)
        st = self._state.get(a.bundle_id)
        if action.element and st and st.pid and st.pid != a.pid:
            raise StaleRef("element process changed; take a new snapshot")
        mechanism = action.mechanism or ("pid" if action.kind in {"type", "key"} else "ax")
        if mechanism == "foreground" and not action.allow_foreground:
            return ActionResult(False, "This explicit route needs task-wide foreground permission.",
                                {"code": "needs_foreground", "method": mechanism, "performed": False})
        routes = KEY_ROUTES if action.kind == "key" else ELEMENT_ROUTES.get(action.kind)
        if routes is not None and mechanism not in routes:
            raise Refused(f"{action.kind} takes mechanism {' or '.join(sorted(routes))}, not {mechanism}; nothing "
                          "was sent")
        if action.kind == "key":
            return await self._act_key(a, action.text or "", holder, task, terminal, mechanism)
        if action.kind == "scroll":
            n = self.node(a.bundle_id, action.element) if action.element else None
            r = await self._scroll(a, action, holder, task, mechanism)
            return self._with(r, await self._facts(a, n.ref if n else None))
        if action.element is None:
            raise Refused("this action needs an element id from a snapshot")
        n = self.node(a.bundle_id, action.element)
        if action.kind not in ("type", "set_value", "edit"):
            self._armed.pop(a.pid, None)                 # D31: a press, focus or pick ends a covered Return
        if action.kind in ("press", "click"):            # pressing a .command item can open it (round 2, O1)
            if refusal := await asyncio.to_thread(lambda: self._files_refusal(a, [self._element_file(n.ref)],
                                                                              terminal)):
                raise Refused(refusal)
            if n.ax_role == "AXMenuItem" and (combo := await asyncio.to_thread(self._item_combo, n.ref)) and \
                    (handled := await self._guarded_combo(a, combo, task)) is not None:
                return handled                           # ⌘Q / ⌘H's item: FERB's safe quit/hide (decision 2)
        if n.secret and action.kind in ("set_value", "type", "edit"):
            raise Refused("that's a password field; FERB never types passwords for the owner")
        r, into, received, after = await self._act_element(a, n, action, holder, task, terminal, mechanism)
        return self._with(r, await self._facts(a, n.ref, received, into, after))

    async def _act_key(self, a: Any, combo: str, holder: str, task: str | None, terminal: TerminalGrant | None,
                       mechanism: str) -> ActionResult:
        """A key press, which goes to the app's keyboard focus (D31), on any route: the shortcut guard first
        (decision 2), then the run-command and terminal checks, and no text-entering key into a password field."""
        if not combo or not valid_combo(combo):
            raise Refused(f"unsupported key {combo!r}")
        if (handled := await self._guarded_combo(a, combo, task)) is not None:
            return handled
        if refusal := await asyncio.to_thread(lambda: self._files_refusal(a, self._selected_files(a), terminal)):
            raise Refused(refusal)                       # a key with a .command selected can open it (O1)
        path, control = await asyncio.to_thread(self._key_run, a, combo)
        if (path or control) and (refusal := await asyncio.to_thread(self._run_refusal, a, terminal, path, control)):
            raise Refused(refusal)                       # a key that presses a run command (round 2, 2a, 2c)
        focus = await asyncio.to_thread(self._focus, a)              # keys go to the app's focus (D31)
        if enters_text(combo) and await asyncio.to_thread(self._secure, focus):
            raise Refused(SECURE_KEYS)
        into = await self._terminal_check(a, focus, terminal, task, keys=combo)
        r = await self._key(a, combo, holder, task, mechanism, focus)
        if into or a.pid in self._armed:
            self._terminal_typed(a, focus, None, task, False)       # a key into it: no Return stays covered
        return self._with(r, await self._facts(a, focus, terminal=into, after=True))

    async def _act_element(self, a: Any, n: axm.AxNode, action: ComputerAction, holder: str, task: str | None,
                           terminal: TerminalGrant | None, mechanism: str
                           ) -> tuple[ActionResult, bool, Any, bool]:
        """An element action on its chosen route: (result, whether it reached a terminal, what received it, whether
        to read the keyboard focus after it as the receiver)."""
        if action.kind in ("press", "click"):
            if await asyncio.to_thread(self._runs, a, n.ref):            # a run control (finding 2)
                if refusal := await asyncio.to_thread(self._run_refusal, a, terminal, None, n.label):
                    raise Refused(refusal)
            focus, into = None, False
            if n.ax_role == "AXMenuItem" and await asyncio.to_thread(self._item_pastes, n.ref):
                focus = await asyncio.to_thread(self._focus, a)          # ⌘V's item pastes into the focus (D31)
                if await asyncio.to_thread(self._secure, focus):
                    raise Refused(SECURE_KEYS)
                into = await self._terminal_check(a, focus, terminal, task, keys="cmd+v")
            elif n.ax_role == "AXMenuItem" and await asyncio.to_thread(self._terminal_app, a):
                focus = await asyncio.to_thread(self._focus, a)          # a terminal app's command (finding 10)
                await self._terminal_check(a, focus, terminal, task, menu=n.label)
            if mechanism == "foreground":
                r = await self._foreground_element(a, n.ref, self.ax.center(n), lambda x, y: click_steps(x, y),
                                                   holder, "click", task)
            else:
                r = await self._press(a, n, holder, task, False)
            if into:
                self._terminal_typed(a, focus, terminal, task, r.ok)
            return r, into, n.ref, False
        if action.kind == "type" and mechanism == "foreground":
            r, into = await self._type_foreground(a, n, action.text or "", holder, task, terminal)
            return r, into, None, True
        if action.kind == "cancel":
            ok = await asyncio.to_thread(self.ax.cancel, n.ref)
            return ActionResult(ok, "AXCancel accepted" if ok else "AXCancel refused",
                                {"method": "ax", "accepted": ok, "outcome_verified": False}), False, n.ref, False
        if action.kind == "focus":
            ok = await asyncio.to_thread(self.ax.focus, n.ref)
            self.profiles.record(a.bundle_id, "ax_focus", ok)
            return ActionResult(ok, "AX focus request accepted" if ok else "AX focus request refused",
                                {"method": "ax", "accepted": ok, "outcome_verified": False}), False, None, True
        if action.kind == "set_value":
            into = await self._terminal_check(a, n.ref, terminal, task, text=action.text)
            r = await self._set_value(a, n, action.text or "", holder, task, False)
            if into:
                self._terminal_typed(a, n.ref, terminal, task, r.ok)
            return r, into, n.ref, False
        if action.kind == "type":
            r, into = await self._type(a, n, action.text or "", holder, task, terminal)
            return r, into, None, True
        if action.kind == "edit":
            into = action.text is not None and await self._terminal_check(a, n.ref, terminal, task, text=action.text)
            r = await self._edit(a, n, action.select, action.text, mechanism)
            if into:
                self._terminal_typed(a, n.ref, terminal, task, r.ok)
            return r, into, n.ref, False
        if action.kind == "select":
            return await self._select(a, n, action.text or ""), False, n.ref, False
        raise Refused(f"unknown action {action.kind}")

    async def _type_foreground(self, a: Any, n: axm.AxNode, text: str, holder: str, task: str | None,
                               terminal: TerminalGrant | None) -> tuple[ActionResult, bool]:
        """Typing as real keystrokes with the app in front: the element focused first, the keyboard focus checked
        between chunks (a terminal it lands in asks, D31)."""
        landed: list[Any] = []

        async def focus_it() -> None:
            await asyncio.to_thread(self.ax.focus, n.ref)
            if await asyncio.to_thread(self.ax.has_focus, a.pid, n.ref) is not True:
                raise Refused(FOCUS_ELSEWHERE)
            focus = await asyncio.to_thread(self._focus, a)          # where the keystrokes land (D31)
            target = await asyncio.to_thread(self._landing, a, focus, n.ref)
            landed.append(focus if focus is not None else n.ref)
            if await self._terminal_check(a, target, terminal, task, text=text):
                landed.append(True)
        steps = type_steps(text)

        def chunk_check(i: int, step: Step) -> Step:
            def post() -> None:
                if i and i % TYPE_CHUNK == 0 and landed:
                    self._focus_moved(a, landed[0], i, len(steps), len(landed) > 1)
                step.post()
            return Step(step.what, post)
        r = await self._real(a, [chunk_check(i, step) for i, step in enumerate(steps)], holder, "typing", task,
                             focus_it)
        if len(landed) > 1:
            self._terminal_typed(a, landed[0], terminal, task, r.ok)
        return r, len(landed) > 1

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

    def _landing(self, a: Any, focus: Any, el: Any) -> Any:
        """Blocking: the element typed keystrokes are checked against (D31): the keyboard focus; when it can't be
        read, in an app that can host a terminal nothing (which counts as a terminal: fails closed), elsewhere the
        element FERB focused."""
        if focus is not None:
            return focus
        return None if self._hosts(a) else el

    async def _type(self, a: Any, n: axm.AxNode, text: str, holder: str, task: str | None,
                    terminal: TerminalGrant | None = None) -> tuple[ActionResult, bool]:
        """Background typing (process-directed keystrokes) into the element, focused first: (result, whether it
        reached a terminal)."""
        await asyncio.to_thread(self.ax.focus, n.ref)
        if await asyncio.to_thread(self.ax.has_focus, a.pid, n.ref) is not True:
            raise Refused(FOCUS_ELSEWHERE)
        focus = await asyncio.to_thread(self._focus, a)                  # where the keystrokes land (D31)
        landed = await asyncio.to_thread(self._landing, a, focus, n.ref)
        into = await self._terminal_check(a, landed, terminal, task, text=text)
        r = await self._typed(a, n, text, holder, focus, into)
        if into:
            self._terminal_typed(a, landed, terminal, task, r.ok)
        return r, into

    def _focus_moved(self, a: Any, landed: Any, done: int, total: int, strict: bool = False) -> None:
        """Blocking, between chunks of typed text (D31): approved terminal typing (`strict`) stops when the keyboard
        focus is anywhere but where it began; other typing (round 2, N4: an autocomplete may move the reported focus)
        stops only when the focus moved into a terminal. Either way the rest isn't typed."""
        now = self._focus(a)
        moved = (now is not None) if landed is None else (now is None or not self._same_el(now, landed))
        if strict and moved:
            raise PointerInterrupted(f"the keyboard focus moved after {done} of {total} characters, so the rest "
                                     "wasn't typed; check the field and the app before trying again")
        if not strict and now is None and landed is not None and self._hosts(a):
            raise PointerInterrupted(f"FERB couldn't read where the keyboard focus is after {done} of {total} "  # fails
                                     "characters (it could be a terminal), so the rest wasn't typed; check the "  # closed
                                     "field and the app before trying again")
        if not strict and now is not None and not self._same_el(now, landed) and self._terminal_why(a, now):
            raise PointerInterrupted(f"the keyboard focus moved into a terminal after {done} of {total} characters, "
                                     "so the rest wasn't typed; check the field and the app before trying again")

    def _chunked(self, a: Any, steps: list[Step], landed: Any, strict: bool = False) -> list[Step]:
        """Typing steps (one per character) with the focus checked before each chunk after the first (also when the
        focus couldn't be read at the start: a terminal it moves into still stops the rest)."""
        def guarded(i: int, step: Step) -> Step:
            def post() -> None:
                if i and i % TYPE_CHUNK == 0:
                    self._focus_moved(a, landed, i, len(steps), strict)
                step.post()
            return Step(step.what, post)
        return [guarded(i, step) for i, step in enumerate(steps)]

    async def _typed(self, a: Any, n: axm.AxNode, text: str, holder: str, landed: Any = None,
                     strict: bool = False) -> ActionResult:
        """Process-directed keystrokes, the keyboard focus checked between chunks; the GUI lock is given up between
        chunks (one long text doesn't hold every other worker's input)."""
        before = await asyncio.to_thread(self.ax.value, n.ref)
        sig = await self._signature(a, n.ref)
        await self.input.to_process(self._chunked(a, type_steps(text, a.pid), landed, strict), holder,
                                    release_every=TYPE_CHUNK)
        await asyncio.sleep(SETTLE_S)
        after = await asyncio.to_thread(self.ax.value, n.ref)
        changed = after != before if before is not None else await self._signature(a, n.ref) != sig
        self.profiles.record(a.bundle_id, "pid_keys", changed)
        return ActionResult(True, "typing delivered in the background; " +
                            ("a value change was observed" if changed else "result unconfirmed; inspect before retrying"),
                            {"method": "pid", "delivered": True, "observed_change": changed,
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

    async def _key(self, a: Any, combo: str, holder: str, task: str | None, mechanism: str,
                   focus: Any = None) -> ActionResult:
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
        if mechanism != "pid":
            raise Refused("key takes mechanism pid, ax (its menu item) or foreground")
        before = await self._signature(a, focus)         # the window the key goes to (the focus's)
        await self.input.to_process(key_steps(combo, a.pid), holder)
        await asyncio.sleep(SETTLE_S)
        changed = await self._signature(a, focus) != before
        if changed:
            self.profiles.record(a.bundle_id, "pid_keycodes", True)
        return ActionResult(True, f"{combo} sent in the background; " +
                            ("state changed; inspect the outcome" if changed else "nothing visibly changed; result unconfirmed"),
                            {"method": mechanism, "delivered": True, "observed_change": changed,
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
            now = await asyncio.to_thread(self.ax.value, n.ref)
            return ActionResult(bool(done), f"slider: {done} step(s); value now {now}")
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
            # a real wheel at the pane's centre, on the one foreground path: its window raised, the point checked to
            # reach that window (and no FERB window) right before it goes (round 4, N4)
            lines = -10 if direction in {"down", "right"} else 10
            return await self._foreground_element(
                a, area, (rect[0] + rect[2] / 2, rect[1] + rect[3] / 2),
                lambda x, y: wheel_steps(x, y, lines, horizontal=direction in {"left", "right"}), holder, "scroll", task)
        if mechanism not in {"ax", "pid"}:
            raise Refused("scroll supports ax scrollbar, pid text page keys or explicit foreground")
        content = await asyncio.to_thread(self.ax.scroll_content, area)
        vertical = direction in ("up", "down")
        bar = await asyncio.to_thread(self.ax.scroll_bar, area, vertical)
        # Page keys are only safe for an explicitly targeted text view, never a chat row/list selection, and never go
        # into a terminal unasked (D31): there the wheel scrolls (mac.pointer)
        text_view = n is not None and n.ax_role == "AXTextArea" and vertical
        if text_view and mechanism == "pid" and n is not None and \
                await asyncio.to_thread(self._terminal_why, a, n.ref) is not None:
            return ActionResult(False, "that's a terminal: FERB sends it no Page keys unasked (D31); scroll it with "
                                "mac.pointer action=scroll at a fresh screenshot point", {"performed": False})
        moved, route, stop = 0, "", "batch limit reached"
        before = await asyncio.to_thread(self.ax.scroll_position, area, content)
        for _ in range(amount):
            front = await asyncio.to_thread(self.apps.frontmost)
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
        viewport = await asyncio.to_thread(self.ax.frame, area) if area is not None else shot.bounds
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

    async def _bring_forward(self, a: Any, bounds: Rect, window: int) -> None:
        """After activating the app: raise the target window itself and wait (~3 s) for it to be on screen — the
        app may switch to another Space or a full-screen window first."""
        await asyncio.to_thread(self.ax.raise_window, a.pid, bounds)
        for _ in range(12):
            front = await asyncio.to_thread(self.apps.frontmost)
            if self.input.owner_active() or front is None or front.pid != a.pid:
                raise Refused("owner input or focus changed while waiting for the explicitly requested window")
            if await asyncio.to_thread(self.shots.on_screen, window):
                await asyncio.sleep(0.2)
                return
            await asyncio.sleep(0.25)

    async def _real(self, a: Any, steps: list[Any], holder: str, what: str, task: str | None = None,
                    prepare: Callable[[], Awaitable[None]] | None = None) -> ActionResult:
        """An explicitly chosen foreground action. Yield permanently if the owner takes over; restore nothing. Whatever
        is in front first (FERB's notch too, right after the owner's Yes there) the target is brought forward, and each
        event is posted only while the target is the front app, so none reaches FERB's own app (M1)."""
        if not self.input.available():
            return ActionResult(False, "FERB is paused: no foreground input", {"performed": False})
        await self.input.quiet(holder, what)
        front = await asyncio.to_thread(self.apps.frontmost)
        if front is None or front.pid != a.pid:
            self.on_activate(a.bundle_id, task)
            await asyncio.to_thread(self.apps.activate, a.bundle_id, a.pid)
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
