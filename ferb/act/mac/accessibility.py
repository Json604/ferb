"""Read AX trees and perform exact element/window operations. Must not interpret labels or select among plausible targets.

Accessibility (AX) reads and actions for Mac apps.
Blocking pyobjc calls — callers run them in a thread.

One walk per snapshot: `read_tree` visits the window (plus any sheet in it, the app's open menus, popovers and
dialogs) once and records everything the driver needs from it — the controls (anything that exposes an action, whatever
its role), the text, the headings, whether a password field is anywhere in it, and a signature to compare before and
after an action. Elements are controls by their available actions (AXUIElementCopyActionNames), not by a role list:
an AXGroup with AXPress is a control; roles are reported as facts.

Password fields (AXSecureTextField, role or subrole) are reported only as filled/empty: their value is never read.
A browser's web area is built on demand: when the window shows a web area that is still empty, re-read for ~2 s."""

import re
import time
from dataclasses import dataclass, field
from typing import Any

from ferb.core.errors import FerbError

MAX_DEPTH = 40
MAX_ELEMENTS = 5000  # response page; every later node is retrievable with offset

ROLES = {"AXButton": "button", "AXLink": "link", "AXTextField": "textbox", "AXTextArea": "textbox",
         "AXSecureTextField": "textbox", "AXCheckBox": "checkbox", "AXRadioButton": "radio",
         "AXPopUpButton": "popup", "AXComboBox": "combobox", "AXMenuItem": "menuitem",
         "AXMenuButton": "button", "AXSlider": "slider", "AXIncrementor": "stepper",
         "AXDisclosureTriangle": "button", "AXTab": "tab", "AXMenuBarItem": "menuitem"}
TEXT_ROLES = {"AXStaticText", "AXHeading"}
VALUE_ROLES = {"AXTextField", "AXTextArea", "AXComboBox", "AXCheckBox", "AXRadioButton", "AXPopUpButton",
               "AXSecureTextField", "AXSlider", "AXIncrementor"}
CONTROL_ACTIONS = {"AXPress", "AXShowMenu", "AXIncrement", "AXDecrement", "AXConfirm", "AXCancel", "AXPick"}
ACTION_NAMES = {"AXPress": "press", "AXShowMenu": "menu", "AXIncrement": "increment", "AXDecrement": "decrement",
                "AXConfirm": "confirm", "AXCancel": "cancel", "AXPick": "pick", "AXScrollToVisible": "scroll_to_visible",
                "AXRaise": "raise"}
TEXT_KINDS = ("textbox", "searchbox", "combobox")
DIALOG_SUBROLES = {"AXDialog", "AXSystemDialog"}
# AXMenuItemCmdModifiers (AXMenuItem.h): 0 = ⌘ alone; bits: shift 1, option 2, control 4, 8 = no ⌘
MENU_MODIFIER_BITS = {"shift": 1, "alt": 2, "ctrl": 4}


@dataclass
class AxNode:
    ref: Any                           # the AXUIElement: kept in the daemon only
    role: str                          # the kind: button / textbox / popup / group / row ...
    label: str
    value: str | None
    rect: tuple[float, float, float, float]
    secret: bool
    enabled: bool
    ax_role: str = ""                  # the raw AX role, a fact
    actions: tuple[str, ...] = ()      # available actions, as the snapshot names them (press, menu, increment ...)
    selected: bool | None = None
    expanded: bool | None = None
    container: str | None = None       # "sheet" / "menu" / "dialog" / "popover" when it sits in one


@dataclass
class AxTree:
    nodes: list[AxNode] = field(default_factory=list)
    text: str = ""
    title: str = ""
    truncated: bool = False
    has_web: bool = False
    truncated_by: str = ""             # "elements", "depth", "elements, depth"
    secure: bool | None = False        # a password field somewhere in it; None: couldn't tell (walk cut short)
    headings: list[str] = field(default_factory=list)
    windows: list[dict[str, Any]] = field(default_factory=list)
    containers: list[str] = field(default_factory=list)     # what else was included: sheet / menu / dialog / popover
    web_empty: bool = False
    window: Any = None                 # the AX window walked

    offset: int = 0
    next_offset: int | None = None
    repeated_nodes: int = 0
    _visited: dict[int, list[Any]] = field(default_factory=dict, repr=False)

    def signature(self) -> tuple[Any, ...]:
        """What the window shows, to tell whether an action did anything (element ids untouched)."""
        return (self.title, self.text, tuple(self.containers),
                tuple((n.role, n.label, n.value, n.selected, n.expanded, n.enabled) for n in self.nodes))


def _attr(el: Any, name: str) -> Any:
    import ApplicationServices as AX
    if el is None:
        return None
    err, v = AX.AXUIElementCopyAttributeValue(el, name, None)
    return v if err == 0 else None


def _point(v: Any, kind: int) -> tuple[float, float]:
    import ApplicationServices as AX
    if v is None:
        return 0.0, 0.0
    ok, p = AX.AXValueGetValue(v, kind, None)
    if not ok:
        return 0.0, 0.0
    return (float(p.x), float(p.y)) if hasattr(p, "x") else (float(p.width), float(p.height))


def _text(s: Any) -> str:
    """Source text stays lossless; traversal coverage is reported separately."""
    return str(s or "")


def _same(a: Any, b: Any) -> bool:
    import CoreFoundation as CF
    return a is not None and b is not None and bool(CF.CFEqual(a, b))


def actions(el: Any) -> list[str]:
    import ApplicationServices as AX
    if el is None:
        return []
    err, names = AX.AXUIElementCopyActionNames(el, None)
    return [str(n) for n in (names or [])] if err == 0 else []


AX_CANNOT_COMPLETE = -25204        # kAXErrorCannotComplete: the app was busy or didn't answer in time


class NoAnswer(FerbError):
    """An AX action the app didn't answer (kAXErrorCannotComplete): it may still have been delivered, so it is neither
    done nor failed. Raised, never reduced to False, so the gateway records the outcome as unknown and a repeat needs
    the owner (P19). Every other AX error is a definite failure (False)."""

    def __init__(self, action: str, code: int = AX_CANNOT_COMPLETE) -> None:
        super().__init__(f"{action}: the app didn't answer in time (AX error {code}); the action may have happened")
        self.action, self.code = action, code


def perform(el: Any, action: str) -> bool:
    """One AX action on an exact element: True when the app accepted it, False on a definite AX error; raises
    NoAnswer when the app didn't answer (the action may have happened)."""
    import ApplicationServices as AX
    if el is None:
        return False
    code = int(AX.AXUIElementPerformAction(el, action))
    if code == AX_CANNOT_COMPLETE:
        raise NoAnswer(action, code)
    return code == 0


def role_of(el: Any) -> str:
    return str(_attr(el, "AXRole") or "")


# ---------- windows ----------

def app_window(pid: int) -> Any:
    """Return the sole reachable window. Multiple windows require explicit model selection."""
    wins = ax_windows(pid)
    return wins[0] if len(wins) == 1 else None


def ax_windows(pid: int) -> list[Any]:
    """Every window object Accessibility will hand over, deduplicated: AXWindows plus the focused / main window and
    the focused element's window (the only routes to a window on another Space)."""
    import ApplicationServices as AX
    app = AX.AXUIElementCreateApplication(pid)
    out: list[Any] = []
    for w in [_attr(app, "AXFocusedWindow"), _attr(app, "AXMainWindow"),
              _attr(_attr(app, "AXFocusedUIElement"), "AXWindow")] + list(_attr(app, "AXWindows") or []):
        if w is not None and role_of(w) in {"AXWindow", "AXSheet"} and not any(_same(w, x) for x in out):
            out.append(w)
    return out


def cg_windows(pid: int) -> list[dict[str, Any]]:
    """The app's real windows from the window server, every Space: id, title, bounds, on the current Space or not."""
    import Quartz
    on = {int(w["kCGWindowNumber"]) for w in
          Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID) or []}
    out = []
    for w in Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionAll, Quartz.kCGNullWindowID) or []:
        b = w.get("kCGWindowBounds") or {}
        if int(w.get("kCGWindowOwnerPID", 0)) != pid or int(w.get("kCGWindowLayer", 0)) != 0 \
                or float(b.get("Width", 0)) <= 0 or float(b.get("Height", 0)) <= 0:
            continue
        out.append({"id": int(w["kCGWindowNumber"]), "title": str(w.get("kCGWindowName") or ""),
                    "onscreen": int(w["kCGWindowNumber"]) in on,
                    "bounds": (float(b.get("X", 0)), float(b.get("Y", 0)), float(b.get("Width", 0)),
                               float(b.get("Height", 0)))})
    return out


def windows(pid: int, current: Any = None) -> list[dict[str, Any]]:
    """The app's windows as the snapshot lists them: window-server facts (title, id, on this Space) plus whether
    Accessibility can reach the window right now and whether it's the one read."""
    reach = [(w, frame(w), window_title(w)) for w in ax_windows(pid)]
    out = []
    for cg in cg_windows(pid):
        matches = [w for w, f, t in reach if f is not None and
                   all(abs(a - b) <= 2 for a, b in zip(f, cg["bounds"]))]
        ax = matches[0] if len(matches) == 1 else None
        d = {"id": cg["id"], "title": cg["title"], "onscreen": cg["onscreen"], "reachable": ax is not None,
             "bounds": cg["bounds"], "matching_ax_windows": len(matches)}
        if ax is not None and current is not None and _same(ax, current):
            d["current"] = True
        out.append(d)
    return out


def window_named(pid: int, which: str) -> Any:
    """Resolve an exact unique title or exact observed id. Ambiguity is never settled by order/prefix."""
    want = which.strip()
    reach = ax_windows(pid)
    if want.isdigit():
        cg = [w for w in cg_windows(pid) if w["id"] == int(want)]
        if len(cg) != 1:
            return None
        matches = [w for w in reach if (f := frame(w)) is not None and
                   all(abs(a - b) <= 2 for a, b in zip(f, cg[0]["bounds"]))]
        return matches[0] if len(matches) == 1 else None
    matches = [w for w in reach if window_title(w) == want]
    return matches[0] if len(matches) == 1 else None


def frame(win: Any) -> tuple[float, float, float, float] | None:
    import ApplicationServices as AX
    if win is None:
        return None
    x, y = _point(_attr(win, "AXPosition"), AX.kAXValueCGPointType)
    w, h = _point(_attr(win, "AXSize"), AX.kAXValueCGSizeType)
    return (x, y, w, h) if w and h else None


def window_with_frame(pid: int, bounds: tuple[float, float, float, float]) -> Any:
    """The app's Accessibility window at these screen bounds (the captured window), or None."""
    matches = [win for win in ax_windows(pid) if (f := frame(win)) is not None and
               all(abs(a - b) <= 2 for a, b in zip(f, bounds))]
    return matches[0] if len(matches) == 1 else None



def raise_window(pid: int, bounds: tuple[float, float, float, float]) -> bool:
    """Bring this exact window to the front of its app (activating the app alone may leave another window on top).
    Foreground path only."""
    import ApplicationServices as AX
    win = window_with_frame(pid, bounds)
    if win is None:
        return False
    AX.AXUIElementSetAttributeValue(win, "AXMain", True)
    return bool(AX.AXUIElementPerformAction(win, "AXRaise") == 0)


def window_title(win: Any) -> str:
    return str(_attr(win, "AXTitle") or "").strip("‎ ") if win is not None else ""


def is_dialog(win: Any) -> bool:
    return str(_attr(win, "AXSubrole") or "") in DIALOG_SUBROLES


# ---------- hit tests ----------

def hit(x: float, y: float) -> tuple[int, tuple[float, float, float, float] | None] | None:
    """What a real click at this screen point would reach: (pid, frame of its window), or None if Accessibility can't
    tell. The system's own hit test, so click-through overlays (screen tools, the Dock's layer) don't count."""
    import ApplicationServices as AX
    err, el = AX.AXUIElementCopyElementAtPosition(AX.AXUIElementCreateSystemWide(), x, y, None)
    if err != 0 or el is None:
        return None
    err, pid = AX.AXUIElementGetPid(el, None)
    if err != 0:
        return None
    win = el if _attr(el, "AXRole") == "AXWindow" else _attr(el, "AXWindow")
    return int(pid), frame(win)


def hit_element(pid: int, x: float, y: float) -> Any:
    """The app's own element under a screen point (AXUIElementCopyElementAtPosition on the app element). Works for
    windows on the current Space; for a window on another Space it answers the menu bar, which doesn't count."""
    import ApplicationServices as AX
    err, el = AX.AXUIElementCopyElementAtPosition(AX.AXUIElementCreateApplication(pid), x, y, None)
    if err != 0 or el is None or role_of(el) in ("AXMenuBar", "AXApplication", "AXMenuBarItem", ""):
        return None
    return el


def describe(el: Any) -> str:
    """A short description of an element for action results: role and label."""
    role = role_of(el)
    label = _attr(el, "AXTitle") or _attr(el, "AXDescription") or _attr(el, "AXValue") or ""
    kind = ROLES.get(role) or (role[2:].lower() if role.startswith("AX") else role) or "element"
    return f"{kind} “{_text(label)}”" if label else kind


# ---------- the walk ----------

def read_tree(pid: int, browser: bool = False, window: Any = None, offset: int = 0) -> AxTree:
    """One walk of the exact window (or sole reachable window), its sheet, the app's open menus,
    popovers and dialogs. For a browser whose web area is still empty, re-read for ~2 s."""
    import ApplicationServices as AX
    deadline = time.monotonic() + (2.0 if browser else 0.0)
    while True:
        app = AX.AXUIElementCreateApplication(pid)
        win = window if window is not None else app_window(pid)
        tree = AxTree(title=_text(_attr(win, "AXTitle")) if win is not None else "", window=win, offset=offset)
        texts: list[str] = []
        seen = [0]
        if win is not None:
            _walk(win, tree, texts, 0, "dialog" if is_dialog(win) else None, seen)
        for child in _attr(app, "AXChildren") or []:          # menus, popovers, dialogs live beside the window
            if win is not None and _same(child, win):
                continue
            role = role_of(child)
            kind = {"AXMenu": "menu", "AXPopover": "popover", "AXSheet": "sheet"}.get(role)
            if kind is None and role == "AXWindow" and is_dialog(child):
                kind = "dialog"
            if kind is None:
                continue
            tree.containers.append(kind)
            _walk(child, tree, texts, 1, kind, seen)
        tree.text = _text(" ".join(texts))
        if tree.secure is False and "depth" in tree.truncated_by:
            tree.secure = None                                 # not every corner was looked at
        tree.truncated = bool(tree.truncated_by)
        tree.windows = windows(pid, win)
        if not browser or not (tree.has_web and tree.web_empty) or time.monotonic() >= deadline:
            return tree
        time.sleep(0.25)                                       # the web AX tree is built on demand: ask again


def _mark(tree: AxTree, why: str) -> None:
    if why not in tree.truncated_by:
        tree.truncated_by = ", ".join(x for x in (tree.truncated_by, why) if x)


def _walk(el: Any, tree: AxTree, texts: list[str], depth: int, container: str | None, seen: list[int]) -> None:
    import ApplicationServices as AX
    import CoreFoundation as CF
    key = int(CF.CFHash(el)) if hasattr(el, "__c_void_p__") else id(el)
    bucket = tree._visited.setdefault(key, [])
    if any(prior is el or _same(prior, el) for prior in bucket):
        tree.repeated_nodes += 1
        return
    bucket.append(el)
    role, subrole = role_of(el), str(_attr(el, "AXSubrole") or "")
    secret = "AXSecureTextField" in (role, subrole)
    if secret:
        tree.secure = True
    children = _attr(el, "AXChildren") or []
    if role == "AXWebArea":
        tree.has_web = True
        tree.web_empty = tree.web_empty or not children
    if role == "AXSheet" and container is None:
        container = "sheet"
        tree.containers.append("sheet")
    if depth > MAX_DEPTH:
        _mark(tree, "depth")
        return
    index = seen[0]
    seen[0] += 1
    capped = len(tree.nodes) >= MAX_ELEMENTS
    if index >= tree.offset and not capped:
        if role in TEXT_ROLES:
            v = _attr(el, "AXValue") or _attr(el, "AXTitle")
            if v:
                texts.append(str(v))
                if role == "AXHeading":
                    tree.headings.append(_text(v))
        acts = actions(el)
        if role or subrole or acts:
            tree.nodes.append(_node(el, role, subrole, secret, acts, container, children))
    elif index >= tree.offset and capped:
        _mark(tree, "elements")
        tree.next_offset = tree.offset + len(tree.nodes)
    for child in children:
        _walk(child, tree, texts, depth + 1, container, seen)


def _node(el: Any, role: str, subrole: str, secret: bool, acts: list[str], container: str | None,
          children: list[Any]) -> AxNode:
    import ApplicationServices as AX
    if subrole == "AXSearchField":
        kind = "searchbox"
    elif role in ROLES:
        kind = ROLES[role]
    else:
        kind = role[2:].lower() if role.startswith("AX") else (role or "element")
    label = (_attr(el, "AXTitle") or _attr(el, "AXDescription") or _attr(el, "AXPlaceholderValue")
             or _attr(el, "AXHelp") or "")
    if not label and role not in ROLES:                         # a pressable row / cell: its own text names it
        label = " ".join(str(_attr(c, "AXValue") or _attr(c, "AXTitle") or "") for c in children
                         if role_of(c) in TEXT_ROLES).strip()
    value: str | None = None
    if secret:
        value = "filled" if _attr(el, "AXNumberOfCharacters") else ""
    elif role in VALUE_ROLES:
        value = _text(_attr(el, "AXValue"))
    else:
        v = _attr(el, "AXValue")
        if isinstance(v, str) and v:                            # e.g. a chat row's latest message
            value = _text(v)
    x, y = _point(_attr(el, "AXPosition"), AX.kAXValueCGPointType)
    w, h = _point(_attr(el, "AXSize"), AX.kAXValueCGSizeType)
    enabled = _attr(el, "AXEnabled")
    selected, expanded = _attr(el, "AXSelected"), _attr(el, "AXExpanded")
    return AxNode(el, kind, _text(label), value, (x, y, w, h), secret,
                  bool(enabled) if enabled is not None else True, role,
                  tuple(ACTION_NAMES.get(a, a) for a in acts if a in ACTION_NAMES),
                  None if selected is None else bool(selected), None if expanded is None else bool(expanded),
                  container)


def has_secure_field(win: Any, browser: bool = False) -> bool | None:
    """True if the window holds a password field, False if a full walk found none, None if it couldn't tell."""
    if win is None:
        return None
    tree = AxTree()
    _walk(win, tree, [], 0, None, [0])
    if tree.secure is False and "depth" in tree.truncated_by:
        return None
    return tree.secure


def value(el: Any) -> str | None:
    """A text box's current value (to check that typing arrived), or None if it doesn't expose one."""
    v = _attr(el, "AXValue")
    return None if v is None else str(v)


def expose_tree(pid: int) -> bool:
    """Electron apps (Slack, Discord, VS Code…) build their full Accessibility tree only when an assistive app asks
    for it with AXManualAccessibility; set once per process. Other apps refuse the attribute (-25205), harmlessly.
    AXEnhancedUserInterface is NOT set: AppKit answers -25208 and already reports it on."""
    import ApplicationServices as AX
    return bool(AX.AXUIElementSetAttributeValue(AX.AXUIElementCreateApplication(pid), "AXManualAccessibility",
                                                True) == 0)


def _title(el: Any) -> str:
    return str(_attr(el, "AXTitle") or "").strip("‎ ").strip()


# ---------- menus ----------

def menus(pid: int) -> list[dict[str, Any]]:
    """The app's menu bar, read in the background: each menu and its items (submenus one level deep, "A > B")."""
    import ApplicationServices as AX
    bar = _attr(AX.AXUIElementCreateApplication(pid), "AXMenuBar")
    out = []
    for top in (_attr(bar, "AXChildren") or [])[1:]:             # skip the Apple menu
        items: list[str] = []
        for menu in _attr(top, "AXChildren") or []:
            for item in _attr(menu, "AXChildren") or []:
                t = _title(item)
                if not t:
                    continue
                subs = [_title(x) for m in (_attr(item, "AXChildren") or []) for x in (_attr(m, "AXChildren") or [])]
                items += [f"{t} > {s}" for s in subs if s] if any(subs) else [t]
        out.append({"menu": _title(top), "items": items})
    return out


def press_menu(pid: int, path: list[str]) -> bool:
    """Press a menu item by its path ("File", "Search"), through Accessibility: the app stays in the background."""
    import ApplicationServices as AX
    level = _attr(AX.AXUIElementCreateApplication(pid), "AXMenuBar")
    el = None
    for i, name in enumerate(path):
        kids = _attr(level, "AXChildren") or []
        if i > 0:                                                  # menu items live under the menu (one child)
            kids = [x for m in kids for x in (_attr(m, "AXChildren") or [])]
        matches = [k for k in kids if _title(k) == name.strip()]
        el = matches[0] if len(matches) == 1 else None
        if el is None:
            return False
        level = el
    return perform(el, "AXPress")


def menu_item_for_shortcut(pid: int, key: str, modifiers: list[str]) -> tuple[Any, str, bool] | None:
    """The menu item a ⌘ shortcut maps to (AXMenuItemCmdChar + AXMenuItemCmdModifiers), as (item, "Edit > Select
    All", enabled). None when no item carries that key equivalent."""
    import ApplicationServices as AX
    want_mods = sum(MENU_MODIFIER_BITS[m] for m in modifiers if m in MENU_MODIFIER_BITS)
    if "cmd" not in modifiers:
        want_mods |= 8
    bar = _attr(AX.AXUIElementCreateApplication(pid), "AXMenuBar")
    stack = [(m, [_title(top)]) for top in (_attr(bar, "AXChildren") or []) for m in (_attr(top, "AXChildren") or [])]
    matches = []
    while stack:
        menu, path = stack.pop()
        for item in _attr(menu, "AXChildren") or []:
            t = _title(item)
            ch = str(_attr(item, "AXMenuItemCmdChar") or "")
            mods = _attr(item, "AXMenuItemCmdModifiers")
            if ch and ch.lower() == key.lower() and mods is not None and int(mods) == want_mods:
                enabled = _attr(item, "AXEnabled")
                matches.append((item, " > ".join(path + [t]), bool(enabled) if enabled is not None else True))
            for sub in _attr(item, "AXChildren") or []:
                stack.append((sub, path + [t]))
    return matches[0] if len(matches) == 1 else None


def open_menus(pid: int) -> list[Any]:
    """Menus open right now (an AXMenu under the app element: a pop-up button's list, a contextual menu)."""
    import ApplicationServices as AX
    return [c for c in _attr(AX.AXUIElementCreateApplication(pid), "AXChildren") or [] if role_of(c) == "AXMenu"]


def popup_menu(el: Any, pid: int) -> Any:
    """After pressing a pop-up button: its menu — under the button itself, or open beside the window."""
    for c in _attr(el, "AXChildren") or []:
        if role_of(c) == "AXMenu":
            return c
    menus = open_menus(pid)
    return menus[0] if len(menus) == 1 else None


def menu_items(menu: Any) -> list[tuple[Any, str]]:
    return [(i, _title(i)) for i in (_attr(menu, "AXChildren") or []) if role_of(i) == "AXMenuItem"]


def focused_element(pid: int) -> Any:
    import ApplicationServices as AX
    return _attr(AX.AXUIElementCreateApplication(pid), "AXFocusedUIElement")


def has_focus(pid: int, el: Any) -> bool | None:
    """Is this element the app's keyboard focus? (Keystrokes go to the focus, not to the element FERB means: a sheet
    or dialog can hold it.) None when the app doesn't say."""
    focused = focused_element(pid)
    if focused is None:
        return None
    for _ in range(30):
        if _same(focused, el):
            return True
        focused = _attr(focused, "AXParent")
        if focused is None:
            break
    return False


def press(el: Any) -> bool:
    return perform(el, "AXPress")



def cancel(el: Any) -> bool:
    return perform(el, "AXCancel")


def set_value(el: Any, text: str) -> bool:
    import ApplicationServices as AX
    return bool(AX.AXUIElementSetAttributeValue(el, "AXValue", text) == 0)


def settable(el: Any, attr: str) -> bool:
    import ApplicationServices as AX
    if el is None:
        return False
    err, ok = AX.AXUIElementIsAttributeSettable(el, attr, None)
    return err == 0 and bool(ok)


def _range(v: Any) -> tuple[int, int] | None:
    import ApplicationServices as AX
    if v is None:
        return None
    ok, r = AX.AXValueGetValue(v, AX.kAXValueCFRangeType, None)
    if not ok:
        return None
    return (int(r.location), int(r.length)) if hasattr(r, "location") else (int(r[0]), int(r[1]))


def selected_range(el: Any) -> tuple[int, int] | None:
    return _range(_attr(el, "AXSelectedTextRange"))


def set_selected_range(el: Any, start: int, length: int) -> bool:
    import ApplicationServices as AX
    v = AX.AXValueCreate(AX.kAXValueCFRangeType, (int(start), int(length)))
    return bool(AX.AXUIElementSetAttributeValue(el, "AXSelectedTextRange", v) == 0)


def set_selected_text(el: Any, text: str) -> bool:
    import ApplicationServices as AX
    return bool(AX.AXUIElementSetAttributeValue(el, "AXSelectedText", text) == 0)


def visible_range(el: Any) -> tuple[int, int] | None:
    return _range(_attr(el, "AXVisibleCharacterRange"))


# ---------- scrolling ----------

def _scroll_container(el: Any) -> bool:
    return role_of(el) in {"AXScrollArea", "AXList", "AXTable", "AXOutline"} or any(
        _attr(el, name) is not None for name in ("AXVerticalScrollBar", "AXHorizontalScrollBar"))


def scroll_area_of(el: Any, levels: int = 16) -> Any:
    """The nearest structural scroll container (itself included), or None."""
    node = el
    for _ in range(levels):
        if node is None:
            return None
        if _scroll_container(node):
            return node
        node = _attr(node, "AXParent")
    return None


def scroll_area_in(root: Any, cap: int = 2000) -> Any:
    """A unique scroll container under a window. Multiple panes require an explicit element."""
    seen, queue, found = 0, [root], []
    while queue and seen < cap:
        el = queue.pop(0)
        seen += 1
        if _scroll_container(el):
            found.append(el)
            if len(found) > 1:
                return None
            continue
        queue.extend(_attr(el, "AXChildren") or [])
    return found[0] if len(found) == 1 and not queue else None


def scroll_content(area: Any) -> Any:
    """The unique non-scrollbar child inside this exact area, or None when ambiguous."""
    children = [c for c in (_attr(area, "AXChildren") or []) if role_of(c) != "AXScrollBar"]
    return children[0] if len(children) == 1 else None


def scroll_bar(area: Any, vertical: bool = True) -> Any:
    return _attr(area, "AXVerticalScrollBar" if vertical else "AXHorizontalScrollBar")


def _visible_scroll_items(area: Any) -> tuple[Any, ...]:
    """Bounded viewport fingerprint for apps whose scrollbars/AXVisibleRows carry no usable position.

    Geometry detects actual offsets; hashed semantic identity detects recycled rows at the same coordinates.
    No message text or control values leave this helper. Offscreen content and scrollbar thumbs don't count.
    """
    import hashlib
    bounds = frame(area)
    if bounds is None:
        return ()
    x, y, w, h = bounds
    rows = list(_attr(area, "AXVisibleRows") or _attr(area, "AXChildren") or [])
    queue = [(row, 0) for row in rows[:100]]
    found, seen = [], 0
    while queue and seen < 100:
        row, depth = queue.pop(0)
        seen += 1
        rect, role = frame(row), role_of(row)
        if role == "AXScrollBar":
            continue
        if rect is not None:
            rx, ry, rw, rh = rect
            if rw <= 0 or rh <= 0 or rx + rw <= x or ry + rh <= y or rx >= x + w or ry >= y + h:
                continue
            identity = tuple(str(_attr(row, key) or "") for key in ("AXIdentifier", "AXTitle", "AXDescription"))
            digest = hashlib.blake2b(repr(identity).encode(), digest_size=12).hexdigest()
            found.append((role, rect, digest))
        if depth < 2:
            queue.extend((child, depth + 1) for child in list(_attr(row, "AXChildren") or [])[:100])
    return tuple(found)


def _scroll_fraction(value: Any) -> float | None:
    if value is None or isinstance(value, bool):
        return None
    try:
        fraction = float(value)
    except (TypeError, ValueError):
        return None
    return fraction if 0 <= fraction <= 1 else None


def scroll_position(area: Any, content: Any = None) -> tuple[Any, ...]:
    """Scrollbar values, visible text range and viewport content; unknown values never establish the end."""
    v, h = _attr(scroll_bar(area, True), "AXValue"), _attr(scroll_bar(area, False), "AXValue")
    return (_scroll_fraction(v), _scroll_fraction(h),
            visible_range(content) if content is not None else None, _visible_scroll_items(area))


# ---------- the Accessibility keyboard (deprecated API, app-dependent) ----------

# US keyboard: character -> (virtual key code, shift). For the Accessibility keyboard API, which needs real key codes.
_US = {**{c: (k, False) for c, k in zip("asdfhgzxcvbqweryt123465=97-80]ou[ip", [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12,
       13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35])},
       **{c: (k, False) for c, k in zip("lj'k;\\,/nm.` ", [37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 50, 49])}}
_SHIFTED = dict(zip("!@#$%^&*()_+{}|:\"<>?~", "1234567890-=[]\\;',./`"))
_MODS = {"cmd": 55, "shift": 56, "alt": 58, "ctrl": 59}


def _keycode(ch: str) -> tuple[int, bool] | None:
    if ch in _US:
        return _US[ch]
    if ch.isupper() and ch.lower() in _US:
        return _US[ch.lower()][0], True
    if ch in _SHIFTED and _SHIFTED[ch] in _US:
        return _US[_SHIFTED[ch]][0], True
    return None


def post_text(pid: int, text: str) -> bool:
    """Type into the app's focused control through the Accessibility keyboard API — the app stays in the background
    (spike 20: WhatsApp's search box took these, while keystrokes posted to its process never arrived; TextEdit and
    Calculator ignore them). False if a character has no key on the US layout (nothing is typed then)."""
    import ApplicationServices as AX
    import time as _t
    codes = [_keycode(c) for c in text]
    if any(c is None for c in codes):
        return False
    app = AX.AXUIElementCreateApplication(pid)
    for ch, code in zip(text, codes):
        kc, shift = code                                         # type: ignore[misc]
        if shift:
            AX.AXUIElementPostKeyboardEvent(app, 0, _MODS["shift"], True)
        AX.AXUIElementPostKeyboardEvent(app, ord(ch), kc, True)
        AX.AXUIElementPostKeyboardEvent(app, ord(ch), kc, False)
        if shift:
            AX.AXUIElementPostKeyboardEvent(app, 0, _MODS["shift"], False)
        _t.sleep(0.02)
    return True


def post_key(pid: int, keycode: int, modifiers: list[str]) -> bool:
    """One key (with modifiers) to the app's focused control through the Accessibility keyboard API."""
    import ApplicationServices as AX
    app = AX.AXUIElementCreateApplication(pid)
    mods = [_MODS[m] for m in modifiers if m in _MODS]
    for m in mods:
        AX.AXUIElementPostKeyboardEvent(app, 0, m, True)
    AX.AXUIElementPostKeyboardEvent(app, 0, keycode, True)
    AX.AXUIElementPostKeyboardEvent(app, 0, keycode, False)
    for m in reversed(mods):
        AX.AXUIElementPostKeyboardEvent(app, 0, m, False)
    return True


def focus(el: Any) -> bool:
    import ApplicationServices as AX
    return bool(AX.AXUIElementSetAttributeValue(el, "AXFocused", True) == 0)


def center(node: AxNode) -> tuple[float, float]:
    x, y, w, h = node.rect
    return x + w / 2, y + h / 2
