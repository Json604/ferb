"""Accessibility (AX) reads and actions for Mac apps (section 3 act/mac): the tree of an app's focused window, press,
set value, focus. Blocking pyobjc calls — callers run them in a thread.

Password fields (AXSecureTextField) are reported only as filled/empty: their value is never read. The web AX tree
of a browser is built on demand; if the first read has no web content, re-read for up to ~2 s (section 9.2).
"""

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
        x, y = _point(_attr(el, "AXPosition"), AX.kAXValueCGPointType)
        w, h = _point(_attr(el, "AXSize"), AX.kAXValueCGSizeType)
        tree.nodes.append(AxNode(el, "searchbox" if subrole == "AXSearchField" else ROLES[role],
                                 _clip(label, MAX_LABEL), value, (x, y, w, h), secret,
                                 bool(_attr(el, "AXEnabled") if _attr(el, "AXEnabled") is not None else True)))
    for child in _attr(el, "AXChildren") or []:
        _walk(child, tree, texts, depth + 1)


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
