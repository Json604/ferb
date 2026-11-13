"""Fixtures for the Rust port of the Mac hands (rust/crates/ferb-mac-hands), made by running the Python build on
invented data (ENG-3).

    cd <repo> && uv run python rust/fixtures/mac-hands/make.py

Nothing here touches the Mac: no event is posted, no window captured, no app launched, no clipboard written, no
permission asked. Native modules are replaced by fakes (the Python tests' own where they exist: tests/mackit.py), the
clock is a counter and every id is fixed, so the same bytes come out on every run. Writes, next to this file:

- pure.json: keys and gestures (input.py; each CGEvent created, never posted, and read back), masks
  (evidence/mask.py and the driver's two-look rules), the OCR region rules (sanitize/images.py), the Accessibility
  walk over fake element trees (accessibility.py), the browser list and app lookup rules, permission rows, locks,
  the after-action read, the WAV writer's bytes, profiles' files, AppleScript's named paths and wrapper.
- driver.json and tools.json: written by make_driver.py (the driver and the tools on the tests' fakes).
"""

import asyncio
import hashlib
import json
import os
import random
import sys
import tempfile
import time
import types
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
ROOT = Path(tempfile.mkdtemp(prefix="ferbmh-"))
os.environ["FERB_HOME"] = str(ROOT / "home")
os.environ["HOME"] = str(ROOT / "user")
os.environ["TZ"] = "UTC"
time.tzset()
(ROOT / "user").mkdir()
sys.path.insert(0, str(REPO))

import Quartz  # noqa: E402

from ferb.act import aftercheck, locks  # noqa: E402
from ferb.act.evidence import mask as maskm  # noqa: E402
from ferb.act.mac import accessibility as axm  # noqa: E402
from ferb.act.mac import applescript, browsers, input as inp, profile, tools as mtools  # noqa: E402
from ferb.act.mac import apps as appsm  # noqa: E402
from ferb.act.mac import audio as audiom  # noqa: E402
from ferb.act.mac import driver as drv  # noqa: E402
from ferb.device import permissions as perms  # noqa: E402
from ferb.sanitize import images  # noqa: E402
from ferb.sanitize.redact import Redactor  # noqa: E402


def dump(name: str, value: Any) -> None:
    (HERE / name).write_text(json.dumps(value, indent=1, ensure_ascii=False, sort_keys=False) + "\n")


def sha(value: Any) -> str:
    return hashlib.sha256(json.dumps(value, ensure_ascii=True).encode()).hexdigest()


# ---------- keys and gestures ----------

COMBOS = ["cmd+s", "Cmd+Shift+S", "command+option+h", "cmd+h", "cmd+q", "ctrl+cmd+q", "shift+cmd+q", "alt+shift+cmd+q",
          "cmd+tab", "shift+cmd+tab", "cmd+space", "ctrl+space", "ctrl+left", "ctrl+up", "opt+cmd+esc", "f3", "f4",
          "cmd+f3", "return", "Enter", "esc", "tab", "space", "a", "A", "1", "+", "cmd++", "cmd+=", "cmd+v",
          "alt+shift+cmd+v", "ctrl+v", "keypad5", "keypadenter", "pgdn", "backspace", "fwddelete", "cmd+m",
          "super+meta+k", "ctl+function+f12", "hyper+x", "cmd+ctrl+alt+shift+fn+z", "", "  ", "cmd+", "+cmd",
          "cmd + shift + p", "f13", "f20", "volumeup", "cmd+ä", "é", "comma", "cmd+period", "shift+tab",
          "Arrowleft", "cmd+arrowdown", "control+arrowright", "opt+cmd+h", "cmd+alt+h", "cmd+shift+h", "cmd+H"]
SHORTCUTS = [("Q", 0), ("q", 1), ("H", 2), ("v", 0), ("V", 8), ("x", 4), ("x", 7), ("x", 15), ("Ä", 0)]


class Recorded:
    """Each CGEvent a step creates, read back through Quartz's getters; nothing is posted."""

    def __init__(self) -> None:
        self.events: list[Any] = []

    def ev(self, e: Any, route: Any) -> None:
        kind = Quartz.CGEventGetType(e)
        loc = Quartz.CGEventGetLocation(e)
        d: dict[str, Any] = {"type": int(kind), "route": route}
        if kind in (Quartz.kCGEventKeyDown, Quartz.kCGEventKeyUp):
            try:
                n, s = Quartz.CGEventKeyboardGetUnicodeString(e, 4, None, None)
                chars: Any = s[:n] if s else ""
            except UnicodeDecodeError:          # a lone UTF-16 surrogate: Python sets len(ch) code points
                chars = None
            d |= {"keycode": int(Quartz.CGEventGetIntegerValueField(e, Quartz.kCGKeyboardEventKeycode)),
                  "flags": int(Quartz.CGEventGetFlags(e)) & 0xFF0000, "chars": chars}
        elif kind == Quartz.kCGEventScrollWheel:
            pixels = bool(Quartz.CGEventGetIntegerValueField(e, Quartz.kCGScrollWheelEventIsContinuous))
            axis = ((Quartz.kCGScrollWheelEventPointDeltaAxis1, Quartz.kCGScrollWheelEventPointDeltaAxis2) if pixels
                    else (Quartz.kCGScrollWheelEventDeltaAxis1, Quartz.kCGScrollWheelEventDeltaAxis2))
            d |= {"at": [loc.x, loc.y], "v": int(Quartz.CGEventGetIntegerValueField(e, axis[0])),
                  "h": int(Quartz.CGEventGetIntegerValueField(e, axis[1])), "pixels": pixels}
        else:
            d |= {"at": [loc.x, loc.y],
                  "clicks": int(Quartz.CGEventGetIntegerValueField(e, Quartz.kCGMouseEventClickState))}
        self.events.append(d)


def run_steps(steps: list[Any], rec: Recorded) -> list[dict[str, Any]]:
    out = []
    for s in steps:
        before = len(rec.events)
        s.post()
        out.append({"what": s.what, "events": rec.events[before:]})
    return out


def gestures() -> dict[str, Any]:
    rec = Recorded()
    inp._post = lambda ev, pid=None: rec.ev(ev, {"pid": pid})            # never posted

    class Route:
        def post(self, ev: Any, pid: int, window: int, point: tuple[float, float]) -> None:
            rec.ev(ev, {"pid": pid, "window": window, "local": list(point)})
    out: dict[str, Any] = {"click": [], "wheel": [], "type": [], "key": [], "pointer": []}
    for x, y, pid in ((10.0, 20.5, None), (300.25, 40.0, 77)):
        out["click"].append({"args": [x, y, pid], "steps": run_steps(inp.click_steps(x, y, pid), rec)})
    for args in ((5.0, 6.0, -3, None, False, None, False, False, (0, 0)),
                 (5.0, 6.0, 7, 41, True, 9, True, True, (1.0, 2.0)),
                 (50.0, 60.0, -120, 41, False, 9, True, True, (10.0, 20.0)),
                 (50.0, 60.0, 4, 41, True, None, False, False, (0, 0))):
        x, y, lines, pid, horizontal, window, route, pixels, origin = args
        steps = inp.wheel_steps(x, y, lines, pid, horizontal=horizontal, window=window,
                                route=Route() if route else None, pixels=pixels, window_origin=origin)
        out["wheel"].append({"args": [x, y, lines, pid, horizontal, window, route, pixels, list(origin)],
                             "steps": run_steps(steps, rec)})
    for text, pid in (("hi there", None), ("é😀\n", 12), ("", 3)):
        out["type"].append({"args": [text, pid], "steps": run_steps(inp.type_steps(text, pid), rec)})
    for combo, pid in (("cmd+s", 5), ("Cmd+Shift+S", None), ("return", 9), ("pagedown", 9), ("alt+ctrl+fn+f5", 1)):
        out["key"].append({"args": [combo, pid], "steps": run_steps(inp.key_steps(combo, pid), rec)})
    for kind in ("click", "dblclick", "rightclick", "hover", "drag"):
        for route in (False, True):
            steps, release = inp.pointer_steps((10.0, 20.0), (110.0, 70.0), kind, 33, window=8 if route else None,
                                               route=Route() if route else None, window_origin=(5.0, 5.0))
            out["pointer"].append({"args": [kind, route], "steps": run_steps(steps, rec)})
    return out


def keys() -> dict[str, Any]:
    return {"combos": [{"combo": c, "normalize": inp.normalize(c), "valid": inp.valid_combo(c),
                        "screen_wide": inp.screen_wide(c), "quit_or_hide": inp.quit_or_hide(c),
                        "enters_text": inp.enters_text(c)} for c in COMBOS],
            "shortcuts": [{"char": c, "mods": m, "combo": inp.shortcut_combo(c, m)} for c, m in SHORTCUTS],
            "gestures": gestures()}


# ---------- masks ----------

def tree_json(t: Any) -> dict[str, Any] | None:
    if t is None:
        return None
    return {"nodes": [{"rect": list(n.rect), "secret": n.secret} for n in t.nodes], "secure": t.secure,
            "secret_rects": [list(r) for r in t.secret_rects] if t.secret_rects is not None else None,
            "web_areas": [list(r) for r in t.web_areas], "empty_web_areas": [list(r) for r in t.empty_web_areas],
            "has_web": t.has_web}


def web_json(w: Any) -> dict[str, Any] | None:
    if w is None:
        return None
    s = w.state
    return {"state": None if s is None else {"top": s.top, "viewport": list(s.viewport),
                                             "items": [{"key": i.key, "rect": list(i.rect), "paint": i.paint,
                                                        "child": i.child} for i in s.items]},
            "why": w.why, "late": w.late, "tab": list(w.tab) if w.tab else None}


RECTS = [(110.0, 200.0, 120.0, 24.0), (110.0, 260.0, 120.0, 24.0), (300.0, 300.0, 0.0, 20.0),
         (115.0, 205.0, 50.0, 10.0), (90.0, 40.0, 30.0, 30.0)]
AREAS = [(100.0, 130.0, 800.0, 520.0), (100.0, 130.0, 400.0, 520.0), (500.0, 130.0, 400.0, 520.0),
         (120.0, 150.0, 100.0, 100.0), (0.0, 0.0, 0.0, 0.0)]


def rand_state(r: random.Random) -> Any:
    keys_ = ["field|a", "field|b", "frame|c", "frame|d"]
    items = []
    for k in r.sample(keys_, r.randint(0, 4)):
        items.append(maskm.PageItem(k, r.choice([(10.0, 20.0, 100.0, 20.0), (10.0, 60.0, 100.0, 20.0),
                                                 (0.0, 0.0, 400.0, 300.0), (12.0, 22.0, 50.0, 10.0)]),
                                    r.random() < 0.7, r.choice([None, "doc1", "doc2"]) if k.startswith("frame") else None))
    return maskm.PageState(r.choice(["top1", "top1", "top2"]), r.choice([(0.0, 0.0, 800.0, 520.0),
                                                                      (0.0, 0.0, 800.0, 520.0),
                                                                      (0.0, 40.0, 1600.0, 1040.0)]), tuple(items))


def rand_tree(r: random.Random) -> Any:
    if r.random() < 0.12:
        return None
    nodes = [axm.AxNode(None, "textbox", "pw", "filled", r.choice(RECTS), True, True) for _ in range(r.randint(0, 2))]
    t = axm.AxTree(nodes, secure=r.choice([True, False, None, False]))
    if r.random() < 0.8:
        t.secret_rects = [r.choice(RECTS) for _ in range(r.randint(0, 2))]
    t.web_areas = [r.choice(AREAS) for _ in range(r.randint(0, 3))]
    t.empty_web_areas = [a for a in t.web_areas if r.random() < 0.3]
    t.has_web = bool(t.web_areas) and r.random() < 0.9
    return t


def rand_web(r: random.Random) -> Any:
    if r.random() < 0.3:
        return None
    if r.random() < 0.25:
        return drv.WebSecrets(None, r.choice(["", "its profile couldn't read the page: blocked"]),
                              late=r.random() < 0.3)
    return drv.WebSecrets(rand_state(r), "", tab=r.choice([None, ("https://example.test/a", "Inbox  (3)"),
                                                           ("https://example.test/b", "Other")]))


def masks() -> dict[str, Any]:
    r = random.Random(7)
    states = [rand_state(r) for _ in range(60)]
    page = []
    for i in range(0, 60, 2):
        a, b = states[i], (states[i + 1] if i % 6 else states[i])
        try:
            got: Any = [[list(x) for x in maskm.page_masks(a, b)[0]], list(maskm.page_masks(a, b)[1])]
        except Exception as e:
            got = {"withheld": str(e)}
        page.append({"before": web_json(drv.WebSecrets(a))["state"], "after": web_json(drv.WebSecrets(b))["state"],
                     "out": got})
    boxes = []
    for bounds, size in (((100.0, 50.0, 800.0, 600.0), (1600, 1200)), ((0.0, 0.0, 333.0, 211.0), (1000, 634)),
                         ((10.0, 10.0, 0.0, 5.0), (5, 5))):
        rects = RECTS + [(95.0, 45.0, 10.0, 10.0), (850.0, 600.0, 100.0, 100.0), (333.3, 100.7, 0.1, 0.1)]
        boxes.append({"rects": [list(x) for x in rects], "bounds": list(bounds), "size": list(size),
                      "out": [list(b) for b in maskm.to_image_boxes(rects, bounds, size)]})
    on_screen = []
    for rects, vp, areas in (([], (0.0, 0.0, 800.0, 520.0), []),
                             ([(10.0, 20.0, 100.0, 20.0)], (0.0, 0.0, 800.0, 520.0), AREAS),
                             ([(10.0, 20.0, 100.0, 20.0)], (5.0, 40.0, 400.0, 520.0), AREAS + [AREAS[1]]),
                             ([(10.0, 20.0, 100.0, 20.0)], (0.0, 0.0, 800.0, 520.0), [(0.0, 0.0, 0.0, 9.0)])):
        try:
            got = [list(x) for x in drv.page_on_screen(rects, vp, areas)]
        except Exception as e:
            got = {"withheld": str(e)}
        on_screen.append({"rects": [list(x) for x in rects], "viewport": list(vp), "areas": [list(a) for a in areas],
                          "out": got})
    windows = []
    for _ in range(400):
        before, after = (rand_tree(r), rand_web(r)), (rand_tree(r), rand_web(r))
        browser = r.random() < 0.6
        m, note = drv.window_masks(before, after, browser)
        windows.append({"before": [tree_json(before[0]), web_json(before[1])],
                        "after": [tree_json(after[0]), web_json(after[1])], "browser": browser,
                        "masks": [list(x) for x in m], "note": note})
    tabs = []
    for titles in ((), ("Inbox (3)",), ("", "Other"), ("x", "Inbox  (3) ")):
        looks = [(None, rand_web(r)) for _ in range(3)]
        got = drv.page_tab(titles, *looks)
        tabs.append({"titles": list(titles), "looks": [web_json(w) for _, w in looks],
                     "out": list(got) if got else None})
    replies = [{"complete": True, "state": {"top": "t", "viewport": [0, 0, 10, 10], "items": []}},
               {"complete": True, "state": {"top": "t", "viewport": [0, 0, 0, 10], "items": []}},
               {"complete": True, "state": {"top": "t", "viewport": [0, 0, 10, 10],
                                            "items": [{"key": "k", "rect": [1, 2, 3, 4], "paint": True}]}},
               {"complete": True, "state": {"top": "t", "viewport": [0, 0, 10, 10],
                                            "items": [{"key": "k", "rect": [1, 2, 3, True], "paint": True}]}},
               {"complete": True, "state": {"top": "t", "viewport": [0, 0, 10, 10],
                                            "items": [{"key": "k", "rect": [1, 2, 3, 4], "paint": 1}]}},
               {"complete": True, "state": {"top": "t", "viewport": [0, 0, 10, 10],
                                            "items": [{"key": "k", "rect": [1, 2, 3, 4], "paint": False,
                                                       "child": 5}]}},
               {"complete": False, "reason": "x" * 200, "tab": {"url": "u" * 5000, "title": "T"}},
               {"complete": False, "reason": "", "tab": {"url": "u", "title": None}},
               {"complete": "yes"}, {}]
    return {"page_masks": page, "to_image_boxes": boxes, "page_on_screen": on_screen, "window_masks": windows,
            "page_tab": tabs, "from_reply": [{"reply": x, "out": web_json(drv.WebSecrets.from_reply(x))}
                                             for x in replies],
            "same_title": [[a, b, drv.same_title(a, b)] for a, b in (("a  b", "a b"), ("a\tb\n", " a b"),
                                                                    ("a b", "ab"), ("", " "))],
            "outermost": [{"areas": [list(a) for a in xs], "out": [list(a) for a in axm.outermost(xs)]}
                          for xs in (AREAS, AREAS + AREAS, [AREAS[3], AREAS[0]], [])]}


# ---------- OCR region rules ----------

def ocr() -> list[dict[str, Any]]:
    frames = [
        ["Your verification code", "48 29 13", "Thanks"],
        ["Enter the code we sent you", "482913"],
        ["Password:", "sunshine2024"],
        ["password", "hunter"],
        ["482913", "is your verification code"],
        ["Card 4111 1111 1111 1111", "visa-4111111111111111"],
        ["4111-1111-1111-1112"],
        ["Login code", "Order 1234", "call 5551234"],
        ["••••••••", "* * * *", "...", "....•"],
        ["Hello there", "Meeting at 10:30", "Room 4021"],
        ["token: ghp_" + "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8", "ok"],
        ["Your OTP is 123456", "123456"],
        ["sign-in code", "12 34 56", "a"],
        [],
    ]
    out = []
    for f in frames:
        regions = [images.TextRegion(t, (i * 10, i * 20, 30 + i, 15)) for i, t in enumerate(f)]
        painted: list[Any] = []

        def mask(image: bytes, boxes: list[Any]) -> bytes:
            painted.append([list(b) for b in boxes])
            return b"masked"
        red = images.ImageRedactor(Redactor(), recognize=lambda image: regions, mask=mask)
        res = red(b"img", [(1, 2, 3, 4)] if len(f) % 2 else None)
        out.append({"lines": f, "rects": [[1, 2, 3, 4]] if len(f) % 2 else [],
                    "neighbours": sorted(images.neighbour_spans(f)),
                    "painted": painted[0] if painted else None, "result": res.decode()})
    return out


# ---------- the Accessibility walk over fake trees ----------

class FakeEl:
    def __init__(self, eid: str, attrs: dict[str, Any], actions: list[str]) -> None:
        self.eid, self.attrs, self.actions = eid, attrs, actions


class Pt(tuple):
    pass


class Clock:
    def __init__(self, step: float) -> None:
        self.t, self.step = 0.0, step

    def monotonic(self) -> float:
        self.t += self.step
        return self.t

    def sleep(self, s: float) -> None:
        self.t += s


def build(spec: dict[str, Any]) -> tuple[dict[str, FakeEl], Any]:
    """Elements from a spec: {"els": {id: {"attrs": {...}, "actions": [...]}}, "apps": {pid: id}, "cg": {pid: [...]}}.
    Attribute values: JSON scalars, {"el": id}, {"els": [ids]}, {"pt": [x, y]}, {"sz": [w, h]}, {"range": [a, b]},
    {"url": absolute, "path": p}, {"strs": [...]}."""
    els = {k: FakeEl(k, v.get("attrs", {}), v.get("actions", [])) for k, v in spec["els"].items()}

    def conv(v: Any) -> Any:
        if isinstance(v, dict):
            if "el" in v:
                return els[v["el"]]
            if "els" in v:
                return [els[x] for x in v["els"]]
            if "pt" in v or "sz" in v:
                return Pt(v.get("pt") or v.get("sz"))
            if "range" in v:
                return tuple(v["range"])
            if "url" in v:
                return types.SimpleNamespace(absoluteString=lambda: v["url"],
                                             filePathURL=lambda: types.SimpleNamespace(path=lambda: v.get("path")))
            if "strs" in v:
                return tuple(v["strs"])
        return v
    for e in els.values():
        e.attrs = {k: conv(v) for k, v in e.attrs.items()}
    return els, conv


def walk_env(spec: dict[str, Any], step: float) -> tuple[dict[str, FakeEl], Clock]:
    els, _ = build(spec)
    clock = Clock(step)
    axm._attr = lambda el, name: None if el is None else el.attrs.get(name)
    axm.actions = lambda el: list(el.actions) if el is not None else []
    axm._same = lambda a, b: a is not None and b is not None and a is b
    axm._point = lambda v, kind: (float(v[0]), float(v[1])) if isinstance(v, Pt) else (0.0, 0.0)
    axm._app = lambda pid: els[spec["apps"][str(pid)]]
    axm._system = lambda: els[spec["system"]] if "system" in spec else None
    axm.cg_windows = lambda pid: [dict(w, bounds=tuple(float(x) for x in w["bounds"]))
                                  for w in spec.get("cg", {}).get(str(pid), [])]
    axm.time = types.SimpleNamespace(monotonic=clock.monotonic, sleep=clock.sleep)
    return els, clock


def node_json(n: Any) -> dict[str, Any]:
    return {"ref": n.ref.eid if n.ref is not None else None, "role": n.role, "label": n.label, "value": n.value,
            "rect": list(n.rect), "secret": n.secret, "enabled": n.enabled, "ax_role": n.ax_role,
            "actions": list(n.actions), "selected": n.selected, "expanded": n.expanded, "container": n.container,
            "subrole": n.subrole, "path": list(n.path), "index": n.index, "identity": n.identity()}


def full_tree_json(t: Any) -> dict[str, Any]:
    return {"nodes": [node_json(n) for n in t.nodes], "text": t.text, "title": t.title, "truncated": t.truncated,
            "has_web": t.has_web, "truncated_by": t.truncated_by, "secure": t.secure,
            "secret_rects": [list(r) for r in t.secret_rects] if t.secret_rects is not None else None,
            "web_areas": [list(r) for r in t.web_areas], "empty_web_areas": [list(r) for r in t.empty_web_areas],
            "headings": t.headings, "windows": [dict(w, bounds=list(w["bounds"])) for w in t.windows],
            "containers": t.containers, "web_empty": t.web_empty,
            "window": t.window.eid if t.window is not None else None, "masks_only": t.masks_only,
            "offset": t.offset, "next_offset": t.next_offset, "repeated_nodes": t.repeated_nodes,
            "signature": json.loads(json.dumps(t.signature()))}


def el(attrs: dict[str, Any], actions: list[str] | None = None) -> dict[str, Any]:
    return {"attrs": attrs, "actions": actions or []}


def pos(x: float, y: float, w: float, h: float) -> dict[str, Any]:
    return {"AXPosition": {"pt": [x, y]}, "AXSize": {"sz": [w, h]}}


def chat_tree() -> dict[str, Any]:
    e: dict[str, Any] = {
        "app": el({"AXRole": "AXApplication", "AXChildren": {"els": ["win", "menu1", "pop", "dlg", "plainwin"]},
                   "AXFocusedWindow": {"el": "win"}, "AXWindows": {"els": ["win", "dlg", "plainwin"]},
                   "AXFocusedUIElement": {"el": "box"}, "AXMenuBar": {"el": "bar"}}),
        "win": el({"AXRole": "AXWindow", "AXTitle": "Chat with Asha‎ ", "AXChildren": {"els": ["grp", "sheet"]},
                   "AXParent": {"el": "app"}, **pos(100, 50, 800, 600)}, ["AXRaise"]),
        "grp": el({"AXRole": "AXGroup", "AXParent": {"el": "win"}, "AXChildren": {"els": ["h1", "st1", "box", "pw", "send", "row", "web",
                                                                "web2", "st1", "cb", "sl", "srch", "mystery"]}}),
        "h1": el({"AXRole": "AXHeading", "AXValue": "Messages", **pos(110, 60, 200, 20)}),
        "st1": el({"AXRole": "AXStaticText", "AXValue": "Hello Asha", **pos(110, 90, 200, 20)}),
        "box": el({"AXRole": "AXTextField", "AXValue": "draft", "AXPlaceholderValue": "Message",
                   "AXEnabled": True, **pos(110, 500, 600, 30), "AXSelectedTextRange": {"range": [2, 0]},
                   "AXWindow": {"el": "win"}, "AXParent": {"el": "grp"}},
                  ["AXConfirm", "AXShowMenu", "AXFancy"]),
        "pw": el({"AXRole": "AXSecureTextField", "AXTitle": "Password", "AXNumberOfCharacters": 8,
                  **pos(110, 540, 300, 30)}),
        "send": el({"AXRole": "AXButton", "AXTitle": "Send", "AXEnabled": False, **pos(720, 500, 60, 30),
                    "AXWindow": {"el": "win"}},
                   ["AXPress"]),
        "row": el({"AXRole": "AXRow", "AXChildren": {"els": ["rt1", "rt2"]}, "AXSelected": True,
                   "AXValue": "latest message", **pos(110, 120, 600, 40)}, ["AXPress", "AXScrollToVisible"]),
        "rt1": el({"AXRole": "AXStaticText", "AXValue": " Rahul "}),
        "rt2": el({"AXRole": "AXStaticText", "AXTitle": "online"}),
        "web": el({"AXRole": "AXWebArea", "AXChildren": {"els": ["pwd2"]}, **pos(100, 130, 800, 520),
                   "AXParent": {"el": "grp"},
                   "AXURL": {"url": "https://example.test/x", "path": None}}),
        "pwd2": el({"AXRole": "AXTextField", "AXDescription": "Enter PWD here", **pos(120, 150, 100, 20),
                    "AXParent": {"el": "web"}, "AXDOMClassList": {"strs": ["xterm", "a"]}}),
        "web2": el({"AXRole": "AXWebArea", **pos(120, 150, 100, 100)}),
        "cb": el({"AXRole": "AXCheckBox", "AXTitle": "Remember", "AXValue": 0, "AXExpanded": False},
                 ["AXPress"]),
        "sl": el({"AXRole": "AXSlider", "AXValue": 0.5, "AXHelp": "Volume"}, ["AXIncrement", "AXDecrement"]),
        "srch": el({"AXRole": "AXTextField", "AXSubrole": "AXSearchField", "AXValue": ""}),
        "mystery": el({}, []),
        "sheet": el({"AXRole": "AXSheet", "AXChildren": {"els": ["ok"]}}),
        "ok": el({"AXRole": "AXButton", "AXTitle": "OK"}, ["AXPress"]),
        "menu1": el({"AXRole": "AXMenu", "AXChildren": {"els": ["mi1"]}}),
        "mi1": el({"AXRole": "AXMenuItem", "AXTitle": "Copy", "AXMenuItemCmdChar": "C",
                   "AXMenuItemCmdModifiers": 0}, ["AXPress", "AXCancel"]),
        "pop": el({"AXRole": "AXPopover", "AXChildren": {"els": ["pb"]}}),
        "pb": el({"AXRole": "AXButton", "AXDescription": "Close"}, ["AXPress"]),
        "dlg": el({"AXRole": "AXWindow", "AXSubrole": "AXDialog", "AXTitle": "Alert", "AXChildren": {"els": ["dt"]},
                   **pos(300, 200, 300, 150)}),
        "dt": el({"AXRole": "AXStaticText", "AXValue": "Are you sure?"}),
        "plainwin": el({"AXRole": "AXWindow", "AXTitle": "Other", **pos(0, 0, 400, 300)}),
        "bar": el({"AXRole": "AXMenuBar", "AXChildren": {"els": ["apple", "filem", "editm"]}}),
        "apple": el({"AXTitle": "Apple"}),
        "filem": el({"AXRole": "AXMenuBarItem", "AXTitle": "File", "AXChildren": {"els": ["filemenu"]}}),
        "filemenu": el({"AXRole": "AXMenu", "AXChildren": {"els": ["fsearch", "fnew", "fblank", "fexp"]}}),
        "fsearch": el({"AXRole": "AXMenuItem", "AXTitle": " Search‎", "AXMenuItemCmdChar": "F",
                       "AXMenuItemCmdModifiers": 0}),
        "fnew": el({"AXRole": "AXMenuItem", "AXTitle": "New Chat", "AXMenuItemCmdChar": "n",
                    "AXMenuItemCmdModifiers": 1, "AXEnabled": False}),
        "fblank": el({"AXRole": "AXMenuItem", "AXTitle": ""}),
        "fexp": el({"AXRole": "AXMenuItem", "AXTitle": "Export", "AXChildren": {"els": ["expm"]}}),
        "expm": el({"AXRole": "AXMenu", "AXChildren": {"els": ["pdf", "txt"]}}),
        "pdf": el({"AXRole": "AXMenuItem", "AXTitle": "PDF", "AXMenuItemCmdChar": "p", "AXMenuItemCmdModifiers": 3}),
        "txt": el({"AXRole": "AXMenuItem", "AXTitle": "Text"}),
        "editm": el({"AXRole": "AXMenuBarItem", "AXTitle": "Edit", "AXChildren": {"els": ["editmenu"]}}),
        "editmenu": el({"AXRole": "AXMenu", "AXChildren": {"els": ["paste", "paste2"]}}),
        "paste": el({"AXRole": "AXMenuItem", "AXTitle": "Paste", "AXMenuItemCmdChar": "V",
                     "AXMenuItemCmdModifiers": 0}),
        "paste2": el({"AXRole": "AXMenuItem", "AXTitle": "Paste and Match Style", "AXMenuItemCmdChar": "V",
                      "AXMenuItemCmdModifiers": 3}),
    }
    return {"els": e, "apps": {"21": "app"},
            "cg": {"21": [{"id": 210, "title": "Chat with Asha", "onscreen": True, "bounds": [100, 50, 800, 600]},
                          {"id": 211, "title": "Alert", "onscreen": False, "bounds": [301, 201, 300, 150]},
                          {"id": 212, "title": "", "onscreen": True, "bounds": [0, 0, 1, 1]}]}}


def deep_tree(depth: int, wide: int) -> dict[str, Any]:
    e: dict[str, Any] = {"app": el({"AXChildren": {"els": ["win"]}, "AXWindows": {"els": ["win"]}}),
                         "win": el({"AXRole": "AXWindow", "AXTitle": "Deep", "AXChildren": {"els": ["d0", "wide"]},
                                    **pos(0, 0, 500, 500)})}
    for i in range(depth):
        kids = [f"d{i + 1}"] if i + 1 < depth else ["deep_pw"]
        e[f"d{i}"] = el({"AXRole": "AXGroup", "AXChildren": {"els": kids}}, ["AXPress"] if i % 7 == 0 else [])
    e["deep_pw"] = el({"AXRole": "AXSecureTextField", **pos(10, 10, 50, 10)})
    e["wide"] = el({"AXRole": "AXList", "AXChildren": {"els": [f"w{i}" for i in range(wide)] + ["w0"]}})
    for i in range(wide):
        e[f"w{i}"] = el({"AXRole": "AXButton", "AXTitle": f"Item {i}"}, ["AXPress"])
    return {"els": e, "apps": {"5": "app"}, "cg": {"5": [{"id": 50, "title": "Deep", "onscreen": True,
                                                         "bounds": [0, 0, 500, 500]}]}}


def browser_tree(empty: bool) -> dict[str, Any]:
    e = {"app": el({"AXChildren": {"els": ["win"]}, "AXMainWindow": {"el": "win"}}),
         "win": el({"AXRole": "AXWindow", "AXTitle": "Page", "AXChildren": {"els": ["tb", "web"]},
                    **pos(0, 0, 800, 600)}),
         "tb": el({"AXRole": "AXToolbar"}),
         "web": el({"AXRole": "AXWebArea", **pos(0, 80, 800, 520),
                    **({} if empty else {"AXChildren": {"els": ["p"]}})}),
         "p": el({"AXRole": "AXStaticText", "AXValue": "page text"})}
    return {"els": e, "apps": {"8": "app"}}


def walks() -> dict[str, Any]:
    """Specs by name; `deep` is generated from its parameters on both sides (deep_tree)."""
    specs = {"chat": chat_tree(), "browser_empty": browser_tree(True), "browser_full": browser_tree(False)}
    big = deep_tree(45, 5010)
    cases = []
    for name, spec_name, pid, kw, step in (
            ("chat", "chat", 21, {}, 0.0),
            ("chat_masks_only", "chat", 21, {"masks_only": True}, 0.0),
            ("chat_dialog_window", "chat", 21, {"window": "dlg"}, 0.0),
            ("chat_offset", "chat", 21, {"window": "win", "offset": 5}, 0.0),
            ("chat_budget", "chat", 21, {"budget_s": 0.05}, 0.004),
            ("chat_browser", "chat", 21, {"browser": True}, 0.0),
            ("deep", "deep", 5, {}, 0.0),
            ("deep_offset", "deep", 5, {"offset": 5000}, 0.0),
            ("deep_masks_only", "deep", 5, {"masks_only": True}, 0.0),
            ("browser_empty", "browser_empty", 8, {"browser": True}, 0.01),
            ("browser_full", "browser_full", 8, {"browser": True}, 0.01),
            ("browser_budget", "browser_empty", 8, {"browser": True, "budget_s": 0.6}, 0.01)):
        spec = big if spec_name == "deep" else specs[spec_name]
        els, clock = walk_env(spec, step)
        args = dict(kw)
        if "window" in args:
            args["window"] = els[args["window"]]
        tree = axm.read_tree(pid, **args)
        out = full_tree_json(tree)
        compact = len(tree.nodes) > 200
        cases.append({"name": name, "spec": spec_name, "pid": pid, "kw": kw, "step": step, "clock_after": clock.t,
                      "sha": sha(out) if compact else None, "tree": None if compact else out,
                      "counts": {"nodes": len(tree.nodes), "text": len(tree.text), "next_offset": tree.next_offset,
                                 "truncated_by": tree.truncated_by, "secure": tree.secure,
                                 "repeated": tree.repeated_nodes, "secret_rects": len(tree.secret_rects or [])}})
    return {"specs": specs, "deep": [45, 5010], "cases": cases}


def ax_helpers() -> dict[str, Any]:
    spec = chat_tree()
    els, _ = walk_env(spec, 0.0)
    from ferb.policy.confirm import browser_page
    axm.browser_page = browser_page
    out: dict[str, Any] = {}
    out["ax_windows"] = [w.eid for w in axm.ax_windows(21)]
    out["app_window"] = (axm.app_window(21) or types.SimpleNamespace(eid=None)).eid
    out["windows"] = [dict(w, bounds=list(w["bounds"])) for w in axm.windows(21, els["win"])]
    out["window_named"] = {k: getattr(axm.window_named(21, k), "eid", None)
                           for k in ("Chat with Asha", "210", " 211 ", "212", "Alert", "Other", "9", "nope")}
    out["window_with_frame"] = [
        {"bounds": list(b), "title": t, "out": getattr(axm.window_with_frame(21, b, t), "eid", None)}
        for b, t in (((100, 50, 800, 600), None), ((101, 51, 799, 601), "x"), ((300, 200, 300, 150), None),
                     ((5, 5, 5, 5), None))]
    out["menus"] = axm.menus(21)
    out["menu_item_for_shortcut"] = [
        {"key": k, "mods": m, "out": None if (f := axm.menu_item_for_shortcut(21, k, m)) is None
         else [f[0].eid, f[1], f[2]]}
        for k, m in (("f", ["cmd"]), ("N", ["cmd", "shift"]), ("p", ["cmd", "shift", "alt"]), ("v", ["cmd"]),
                     ("x", ["cmd"]), ("p", ["shift", "alt"]))]
    out["menu_item_shortcut"] = [{"path": p, "out": list(s) if (s := axm.menu_item_shortcut(21, p)) else None}
                                 for p in (["File", "Search"], ["Edit", "Paste"], ["File", "Export", "PDF"],
                                           ["File"], ["Nope"], ["Edit", "Paste and Match Style"])]
    out["surface_facts"] = {k: axm.surface_facts(els[k]) for k in ("box", "pwd2", "win")}
    out["page_url"] = {k: axm.page_url(els[k]) for k in ("pwd2", "box", "web")}
    out["place_of"] = {k: list(axm.place_of(els[k])) for k in ("box", "send")}
    out["secure_element"] = {k: axm.secure_element(els[k]) for k in ("box", "pw", "pwd2", "srch")}
    out["window_facts"] = list(axm.window_facts(els["box"]))[:6]
    out["has_focus"] = {k: axm.has_focus(21, els[k]) for k in ("box", "grp", "win", "send")}
    return out


# decision: TEST-500 (no Mac reached): the module level `_attr` etc. are left patched; nothing below needs them real.


# ---------- smaller pure rules ----------

def apps_rules() -> dict[str, Any]:
    names = ["Arc", "Firefox", "‎Safari", "Notes", "Brave Browser", "Google Chrome Beta", "com.apple.Safari",
             "com.google.Chrome.app.abc", "com.apple.Safari.WebApp.x", "com.operasoftware.OperaGX", "pid:77",
             "COM.BRAVE.BROWSER", "new browser", "com.example.newbrowser", ""]
    browsers._handlers = None
    browsers._running_bundles = lambda pid=None, name=None: ["org.mozilla.firefox"] if pid == 77 else []
    out = {"known": {n: browsers.known(n) for n in names}, "is_browser_app": {n: browsers.is_browser_app(n)
                                                                            for n in names},
           "clean": {n: browsers.clean(n) for n in names + ["‏​WhatsApp  ", "A⁠B"]},
           "pair_key": {n: browsers.pair_key(n) for n in names + ["com.google.chrome", "company.thebrowser.Browser"]},
           "data_folders": list(browsers.data_folders())}
    running = [appsm.RunningApp("net.whatsapp.WhatsApp", "‎WhatsApp", 21, False),
               appsm.RunningApp("com.apple.TextEdit", "TextEdit", 22, True),
               appsm.RunningApp("com.apple.TextEdit", "TextEdit", 23, False),
               appsm.RunningApp("com.microsoft.VSCode", "Code", 24, False)]
    appsm.running = lambda: running
    out["candidates"] = {q: [a.pid for a in appsm.candidates(q)] for q in
                         ("WhatsApp", "‏whatsapp", "com.apple.textedit", "TextEdit", "pid:24", "pid:x", "Code",
                          "code ", "nothing")}
    plists = [None, {}, {"CFBundleDocumentTypes": [{"CFBundleTypeRole": "Shell",
                                                    "LSItemContentTypes": ["public.unix-executable"]}]},
              {"CFBundleDocumentTypes": [{"CFBundleTypeRole": "Editor", "LSItemContentTypes": ["public.shell-script"],
                                          "CFBundleTypeExtensions": ["SH", "zsh"]}]},
              {"CFBundleDocumentTypes": [{"CFBundleTypeRole": "Viewer", "CFBundleTypeExtensions": ["sh"]},
                                         {"LSItemContentTypes": ["public.shell-script"]}]}]
    conforms = {("public.shell-script", "public.shell-script"), ("public.shell-script", "public.script"),
                ("com.apple.terminal.shell-script", "public.shell-script")}
    out["shell_role"] = [appsm.shell_role(p) for p in plists]
    out["editor_role"] = [[appsm.editor_role(p, k, ext, lambda a, b: (a, b) in conforms) for k, ext in
                           (("public.shell-script", ""), ("com.apple.terminal.shell-script", "x"), ("", "sh"),
                            ("public.data", "zsh"))] for p in plists]
    out["conforms"] = [list(x) for x in sorted(conforms)]
    out["plists"] = plists
    return out


def permission_rules() -> dict[str, Any]:
    grants = [{}, {"accessibility": "granted", "screen_recording": "denied", "full_disk_access": "granted",
                   "input_monitoring": "not_determined"}]
    contain = [{}, {"profile_ok": True, "shell": True, "proxy": True},
               {"profile_ok": True, "shell": True, "proxy": True, "proxy_alive": False}]
    rows = [perms.rows(g, c, frozenset({"screen_recording"}) if i else frozenset())
            for i, g in enumerate(grants) for c in contain]
    clip = [perms.clipboard_row(s) for s in ("allow", "unrestricted", "default", "ask", "deny", None, "odd")]
    fresh = [perms.fresh_answer(x) for x in (b'{"screen_recording": "granted", "x": "y", "input_monitoring": 3}',
                                             b"[1]", b"nope", b'{"input_monitoring": "denied"}')]
    restart = sorted(perms.restart_needed({"screen_recording": "denied", "input_monitoring": "granted"},
                                          {"screen_recording": "granted", "input_monitoring": "granted"}))
    report = perms.report_rows({"permissions": {"accessibility": "granted"}, "containment": {"profile_ok": True},
                                "clipboard": "ask"})
    return {"rows": rows, "clipboard_rows": clip, "fresh": fresh, "restart": restart, "report_rows": report,
            "missing": [list(x) for x in perms.missing(report)],
            "panes": {p: perms.pane_url(p) for p in perms.PANES},
            "app_panes": perms.app_panes("com.ferb.ferb"),
            "ae": {str(k): v.value for k, v in perms._AE.items()}, "hid": {str(k): v.value for k, v in perms._HID.items()},
            "fda_ready": [perms.fda_ready(c) for c in contain]}


def lock_rules() -> dict[str, Any]:
    cases = [["app:b", "account:z", "file:/x", "app:a", "tab:1", "recording:r", "app:a"], [], ["bad:x"], ["app"]]
    out = []
    for c in cases:
        try:
            out.append({"keys": c, "out": locks.ordered(c)})
        except ValueError as e:
            out.append({"keys": c, "error": str(e)})
    return {"ordered": out}


def after_rules() -> list[dict[str, Any]]:
    facts = [{"value_matches": True, "value": "x"}, {"value_matches": None, "accepted": False},
             {"method": "ax_value"}, {"method": "ax_value", "accepted": True, "note": "n"},
             {"observed_change": True}, {"observed_change": False, "observed": "the page changed"},
             {"observed_change": True, "observed": "o", "note": "kept"}, {"bytes_match": False},
             {"bytes_match": 1}, {}]
    expects = [{"value_contains": "X"}, {"changed": True}, {"changed": False}, {}]
    out = []
    for f in facts:
        after = aftercheck.after_of(aftercheck.facts_of({"facts": f}))
        out.append({"facts": f, "after": after, "compare": [aftercheck.compare(e, after) for e in expects]})
    out.append({"content": {"ok": 1, "observed_change": True},
                "facts_of": aftercheck.facts_of({"ok": 1, "observed_change": True})})
    return out


def wav_rules() -> list[dict[str, Any]]:
    import array
    out = []
    folder = ROOT / "wav"
    for i, chunks in enumerate(([], [(0.25, 16000, None), (-0.5, 16000, 0.5)], [(0.1, 48000, 0.0), (0.9, 48000, 0.0)],
                                [(0.3, 0, 1.0), (0.2, 24000, None)])):
        w = audiom.WavWriter(folder / f"{i}.wav")
        for value, rate, at in chunks:
            samples = array.array("f", [value, -value / 2, value / 4] * 3).tobytes() + b"\x01"
            w.write(samples, rate, at)
        facts = w.close()
        data = (folder / f"{i}.wav").read_bytes()
        out.append({"chunks": chunks, "facts": facts, "sha": hashlib.sha256(data).hexdigest(), "len": len(data),
                    "head": data[:44].hex()})
    return out


def profile_rules() -> dict[str, Any]:
    folder = ROOT / "profiles"
    stamps = iter(f"2026-01-0{i}T00:00:00+00:00" for i in range(1, 10))
    profile._now = lambda: next(stamps)
    p = profile.Profiles(folder)
    p.record("com.x/y z", "ax_press", True)
    p.record("com.x/y z", "ax_press", False)
    p.record("com.x/y z", "pid_keys", False)
    files = {f.name: f.read_text() for f in sorted(folder.iterdir())}
    (folder / "bad.json").write_text("{nope")
    q = profile.Profiles(folder)
    return {"files": files, "as_dict": p.as_dict("com.x/y z"), "status": [p.status("com.x/y z", "ax_press"),
                                                                        p.status("com.x/y z", "nope")],
            "reload": q.as_dict("com.x/y z"), "bad": {k: v for k, v in q.as_dict("bad").items() if k != "diagnostics"},
            "bad_diag_ops": [d["operation"] for d in q.diagnostics.get("bad", [])]}


def script_rules() -> dict[str, Any]:
    """HOME and the volumes folder are fixed invented paths, so the output is the same on every Mac."""
    scripts = ['get name of every note', 'do shell script "ls"', 'DO  SHELL\u00ac\n SCRIPT "x"',
               '\u00abevent sysoexec\u00bb "x"', '<< event sysoexec >>', 'tell app "Finder" to open "/tmp/a b"',
               'open "~/x" & "~" & "Macintosh HD:Users:me:" & "nohd:x" & "/tmp/a b" & "a/b:c"', '"a\nb"',
               'tell application "FERB" to quit', 'tell process "ferbs"', 'com.ferb.daemon', 'dev.ferb.notch',
               'DO SHELL\tSCRIPT', 'doshell script', 'tell app "Ferb"', 'COM.FERB.APP', 'x-ferb', 'ferb2']
    volumes = ROOT / "volumes"
    (volumes / "Macintosh HD").mkdir(parents=True)
    applescript.VOLUMES = str(volumes)
    home = os.environ["HOME"]
    os.environ["HOME"] = "/Users/invented"
    try:
        rows = [{"script": s, "runs_shell": mtools.runs_shell(s), "own_app": bool(mtools.OWN_APP_SCRIPT.search(s)),
                 "named_paths": [p.replace(str(volumes), "<VOLUMES>") for p in applescript.named_paths(s)]}
                for s in scripts]
    finally:
        os.environ["HOME"] = home
    return {"scripts": rows, "home": "/Users/invented", "volumes_present": ["Macintosh HD"],
            "wrap": [applescript.wrap("com.apple.Notes", "get x")]}


def main() -> None:
    dump("pure.json", {"keys": keys(), "masks": masks(), "ocr": ocr(), "apps": apps_rules(),
                       "permissions": permission_rules(), "locks": lock_rules(), "after": after_rules(),
                       "wav": wav_rules(), "profiles": profile_rules(), "scripts": script_rules(),
                       "walks": walks(), "ax_helpers": ax_helpers()})


if __name__ == "__main__":
    main()
