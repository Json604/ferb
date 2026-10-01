"""Accessibility (AX) reads and actions for Mac apps (section 3 act/mac): the tree of an app's focused window, press,
set value, focus. Blocking pyobjc calls — callers run them in a thread.

Password fields (AXSecureTextField) are reported only as filled/empty: their value is never read. The web AX tree
of a browser is built on demand; if the first read has no web content, re-read for up to ~2 s (section 9.2).
"""

import re
import time
from dataclasses import dataclass, field
from typing import Any

MAX_ELEMENTS, MAX_TEXT, MAX_LABEL, MAX_VALUE, MAX_DEPTH = 400, 8000, 120, 200, 40

ROLES = {"AXButton": "button", "AXLink": "link", "AXTextField": "textbox", "AXTextArea": "textbox",
         "AXSecureTextField": "textbox", "AXCheckBox": "checkbox", "AXRadioButton": "radio",
         "AXPopUpButton": "combobox", "AXComboBox": "combobox", "AXMenuItem": "menuitem",
         "AXMenuButton": "button", "AXSlider": "slider", "AXIncrementor": "button", "AXDisclosureTriangle": "button",
         "AXTab": "tab", "AXSearchField": "searchbox"}
TEXT_ROLES = {"AXStaticText", "AXHeading"}


@dataclass
class AxNode:
    ref: Any                           # the AXUIElement: kept in the daemon only
    role: str
    label: str
    value: str | None
    rect: tuple[float, float, float, float]
    secret: bool
    enabled: bool


@dataclass
class AxTree:
    nodes: list[AxNode] = field(default_factory=list)
    text: str = ""
    title: str = ""
    truncated: bool = False
    has_web: bool = False


def _attr(el: Any, name: str) -> Any:
    import ApplicationServices as AX
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


def _clip(s: Any, n: int) -> str:
    return " ".join(str(s or "").split())[:n]


def app_window(pid: int) -> Any:
    import ApplicationServices as AX
    app = AX.AXUIElementCreateApplication(pid)
    return _attr(app, "AXFocusedWindow") or _attr(app, "AXMainWindow") or next(iter(_attr(app, "AXWindows") or []), None)


def frame(win: Any) -> tuple[float, float, float, float] | None:
    import ApplicationServices as AX
    if win is None:
        return None
    x, y = _point(_attr(win, "AXPosition"), AX.kAXValueCGPointType)
    w, h = _point(_attr(win, "AXSize"), AX.kAXValueCGSizeType)
    return (x, y, w, h) if w and h else None


def window_with_frame(pid: int, bounds: tuple[float, float, float, float]) -> Any:
    """The app's Accessibility window at these screen bounds (the captured window), or None."""
    import ApplicationServices as AX
    for win in _attr(AX.AXUIElementCreateApplication(pid), "AXWindows") or []:
        f = frame(win)
        if f is not None and all(abs(a - b) <= 2 for a, b in zip(f, bounds)):
            return win
    return None


def raise_window(pid: int, bounds: tuple[float, float, float, float]) -> bool:
    """Bring this exact window to the front of its app (activating the app alone may leave another window on top)."""
    import ApplicationServices as AX
    win = window_with_frame(pid, bounds)
    if win is None:
        return False
    AX.AXUIElementSetAttributeValue(win, "AXMain", True)
    return bool(AX.AXUIElementPerformAction(win, "AXRaise") == 0)


def hit(x: float, y: float) -> tuple[int, tuple[float, float, float, float] | None] | None:
    """What a click at this screen point would reach: (pid, frame of its window), or None if Accessibility can't
    tell. Uses the system's own hit test, so click-through overlays (screen tools, the Dock's layer) don't count."""
    import ApplicationServices as AX
    err, el = AX.AXUIElementCopyElementAtPosition(AX.AXUIElementCreateSystemWide(), x, y, None)
    if err != 0 or el is None:
        return None
    err, pid = AX.AXUIElementGetPid(el, None)
    if err != 0:
        return None
    win = el if _attr(el, "AXRole") == "AXWindow" else _attr(el, "AXWindow")
    return int(pid), frame(win)


def label_at(x: float, y: float) -> str | None:
    """The label of the control at a screen point (the element itself or, for its text, its parent button), or None
    when Accessibility can't tell. Used to tell a call / send button from any other click (build step 16a)."""
    import ApplicationServices as AX
    err, el = AX.AXUIElementCopyElementAtPosition(AX.AXUIElementCreateSystemWide(), x, y, None)
    if err != 0 or el is None:
        return None
    parts = []
    for node in (el, _attr(el, "AXParent")):
        if node is None:
            continue
        parts += [str(v) for v in (_attr(node, "AXTitle"), _attr(node, "AXDescription"), _attr(node, "AXHelp"))
                  if v]
    return " ".join(parts) or None


SECURE_SCAN_MAX = 5000


def has_secure_field(win: Any, browser: bool = False) -> bool | None:
    """True if the window holds a password field (AXSecureTextField role or subrole), False if a full walk found
    none, None if it couldn't tell (no window, or too large to walk). A browser builds its web AX tree on demand: until
    a web area shows up, walk again for up to ~2 s (9.2); still none, it can't tell."""
    deadline = time.monotonic() + (2.0 if browser else 0.0)
    while True:
        found, web = _scan_secure(win)
        if found is not False or not browser or web:
            return found
        if time.monotonic() >= deadline:
            return None
        time.sleep(0.25)


def _scan_secure(win: Any) -> tuple[bool | None, bool]:
    if win is None:
        return None, False
    seen, web, stack = 0, False, [(win, 0)]
    while stack:
        el, depth = stack.pop()
        seen += 1
        if seen > SECURE_SCAN_MAX:
            return None, web
        role = str(_attr(el, "AXRole") or "")
        web = web or role == "AXWebArea"
        if "AXSecureTextField" in (role, str(_attr(el, "AXSubrole") or "")):
            return True, web
        if depth < MAX_DEPTH:
            stack.extend((c, depth + 1) for c in (_attr(el, "AXChildren") or []))
        elif _attr(el, "AXChildren"):
            return None, web
    return False, web


def read_tree(pid: int, browser: bool = False) -> AxTree:
    tree = AxTree()
    deadline = time.monotonic() + (2.0 if browser else 0.0)
    while True:
        win = app_window(pid)
        tree = AxTree(title=_clip(_attr(win, "AXTitle"), MAX_LABEL) if win is not None else "")
        texts: list[str] = []
        if win is not None:
            _walk(win, tree, texts, 0)
        tree.text = _clip(" ".join(texts), MAX_TEXT)
        if not browser or tree.has_web or time.monotonic() >= deadline:
            return tree
        time.sleep(0.25)                                   # the web AX tree is built on demand: ask again


def _walk(el: Any, tree: AxTree, texts: list[str], depth: int) -> None:
    if depth > MAX_DEPTH or len(tree.nodes) >= MAX_ELEMENTS:
        tree.truncated = tree.truncated or len(tree.nodes) >= MAX_ELEMENTS
        return
    import ApplicationServices as AX
    role = str(_attr(el, "AXRole") or "")
    if role == "AXWebArea":
        tree.has_web = True
    subrole = str(_attr(el, "AXSubrole") or "")
    if role in TEXT_ROLES:
        v = _attr(el, "AXValue") or _attr(el, "AXTitle")
        if v:
            texts.append(str(v))
    if role in ROLES:
        secret = role == "AXSecureTextField"
        label = (_attr(el, "AXTitle") or _attr(el, "AXDescription") or _attr(el, "AXPlaceholderValue")
                 or _attr(el, "AXHelp") or "")
        value = None
        if role in ("AXTextField", "AXTextArea", "AXComboBox", "AXCheckBox", "AXRadioButton", "AXPopUpButton",
                    "AXSecureTextField", "AXSlider"):
            v = None if secret else _attr(el, "AXValue")
            value = ("filled" if _attr(el, "AXNumberOfCharacters") else "") if secret else _clip(v, MAX_VALUE)
        elif role == "AXButton" and isinstance(_attr(el, "AXValue"), str):   # e.g. a chat row's latest message
            value = _clip(_attr(el, "AXValue"), MAX_VALUE)
        x, y = _point(_attr(el, "AXPosition"), AX.kAXValueCGPointType)
        w, h = _point(_attr(el, "AXSize"), AX.kAXValueCGSizeType)
        tree.nodes.append(AxNode(el, "searchbox" if subrole == "AXSearchField" else ROLES[role],
                                 _clip(label, MAX_LABEL), value, (x, y, w, h), secret,
                                 bool(_attr(el, "AXEnabled") if _attr(el, "AXEnabled") is not None else True)))
    for child in _attr(el, "AXChildren") or []:
        _walk(child, tree, texts, depth + 1)


def value(el: Any) -> str | None:
    """A text box's current value (to check that typing arrived), or None if it doesn't expose one."""
    v = _attr(el, "AXValue")
    return None if v is None else str(v)


REOPEN = re.compile(r"^(open|show) (the )?main window$", re.I)


def open_window(pid: int, app_name: str) -> bool:
    """A running app with no window: press its own menu item that opens it ("Open main window", or the app's entry in
    the Window menu) — through Accessibility, so the app stays in the background (spike 18)."""
    import ApplicationServices as AX
    bar = _attr(AX.AXUIElementCreateApplication(pid), "AXMenuBar")
    name = app_name.strip("\u200e").lower()
    candidates = []
    for top in _attr(bar, "AXChildren") or []:
        top_title = str(_attr(top, "AXTitle") or "").strip("\u200e")
        for menu in _attr(top, "AXChildren") or []:
            for item in _attr(menu, "AXChildren") or []:
                t = str(_attr(item, "AXTitle") or "").strip("\u200e")
                if REOPEN.match(t):
                    candidates.insert(0, item)
                elif top_title == "Window" and t.lower() == name:
                    candidates.append(item)
    return any(AX.AXUIElementPerformAction(c, "AXPress") == 0 for c in candidates[:1])


def has_focus(pid: int, el: Any) -> bool | None:
    """Is this element the app's keyboard focus? (Keystrokes go to the focus, not to the element FERB means: a sheet
    or dialog can hold it.) None when the app doesn't say."""
    import ApplicationServices as AX
    import CoreFoundation as CF
    focused = _attr(AX.AXUIElementCreateApplication(pid), "AXFocusedUIElement")
    return None if focused is None else bool(CF.CFEqual(focused, el))


def focused_role(pid: int) -> str | None:
    """The role (as in snapshots) of the app's focused element, read in the background; None when unknown."""
    import ApplicationServices as AX
    el = _attr(AX.AXUIElementCreateApplication(pid), "AXFocusedUIElement")
    if el is None:
        return None
    role, subrole = str(_attr(el, "AXRole") or ""), str(_attr(el, "AXSubrole") or "")
    return "searchbox" if subrole == "AXSearchField" else ROLES.get(role)


def press(el: Any) -> bool:
    import ApplicationServices as AX
    return bool(AX.AXUIElementPerformAction(el, "AXPress") == 0)


def set_value(el: Any, text: str) -> bool:
    import ApplicationServices as AX
    return bool(AX.AXUIElementSetAttributeValue(el, "AXValue", text) == 0)


def focus(el: Any) -> bool:
    import ApplicationServices as AX
    return bool(AX.AXUIElementSetAttributeValue(el, "AXFocused", True) == 0)


def center(node: AxNode) -> tuple[float, float]:
    x, y, w, h = node.rect
    return x + w / 2, y + h / 2
