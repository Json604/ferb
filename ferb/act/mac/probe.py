"""Background-control probe for one running Mac app (rework phase 1, docs/PLATFORM_MACOS.md).

Given a bundle id, tries every background mechanism FERB might use (Accessibility actions and attributes, the
Accessibility keyboard, keyboard / mouse / scroll events posted to the app's process) on HARMLESS targets, and prints
a JSON capability profile. The owner's frontmost app (Accessibility's focused application) and mouse position are
recorded before and after every step; any change is reported. The app is never activated.

Only TextEdit, Calculator and Finder are allowed as targets, and only on things the probe creates or that merely
change a view:
  TextEdit    File > New through the menu bar (AX press), edits in that untitled document, closed afterwards with
              `close document "<title>" saving no` through AppleScript (only the document the probe created).
  Calculator  digit / clear keys (display cleared afterwards), the View menu's mode (restored afterwards).
  Finder      read-only tree, hit test, menu list, View > as List / as Icons on the front window (restored).
The probe never quits an app, not even one it launched (`open -g -b`): quitting can raise save sheets. It reports
`launched_by_probe` so the caller can decide.

Run:  .venv/bin/python -m ferb.act.mac.probe com.apple.TextEdit [--pid=N] [--json-only] [--seed]
      --seed writes the result into the app's learned profile (~/.ferb/device/apps/<bundle>.json), the one the
      driver consults and updates with every action's outcome (ferb.act.mac.profile).
"""

from __future__ import annotations

import json
import subprocess
import sys
import time
from typing import Any, Callable

ALLOWED = {"com.apple.TextEdit", "com.apple.calculator", "com.apple.finder"}
WALK_CAP = 4000
SETTLE = 0.4

# ---------------------------------------------------------------- low-level helpers

def _ax():
    import ApplicationServices as AX
    return AX


def attr(el: Any, name: str) -> Any:
    AX = _ax()
    if el is None:
        return None
    err, v = AX.AXUIElementCopyAttributeValue(el, name, None)
    return v if err == 0 else None


def set_attr(el: Any, name: str, value: Any) -> int:
    return int(_ax().AXUIElementSetAttributeValue(el, name, value))


def actions(el: Any) -> list[str]:
    err, names = _ax().AXUIElementCopyActionNames(el, None)
    return [str(n) for n in (names or [])] if err == 0 else []


def perform(el: Any, action: str) -> int:
    return int(_ax().AXUIElementPerformAction(el, action))


def rect(el: Any) -> tuple[float, float, float, float] | None:
    AX = _ax()
    p, s = attr(el, "AXPosition"), attr(el, "AXSize")
    if p is None or s is None:
        return None
    ok1, pt = AX.AXValueGetValue(p, AX.kAXValueCGPointType, None)
    ok2, sz = AX.AXValueGetValue(s, AX.kAXValueCGSizeType, None)
    if not (ok1 and ok2):
        return None
    return float(pt.x), float(pt.y), float(sz.width), float(sz.height)


def center(el: Any) -> tuple[float, float] | None:
    r = rect(el)
    return None if r is None else (r[0] + r[2] / 2, r[1] + r[3] / 2)


def cfrange(location: int, length: int) -> Any:
    AX = _ax()
    import Foundation
    return AX.AXValueCreate(AX.kAXValueCFRangeType, Foundation.NSRange(location, length))


def read_range(v: Any) -> tuple[int, int] | None:
    AX = _ax()
    if v is None:
        return None
    ok, r = AX.AXValueGetValue(v, AX.kAXValueCFRangeType, None)
    if not ok:
        return None
    return (int(r[0]), int(r[1])) if isinstance(r, tuple) else (int(r.location), int(r.length))


AX_FRONT_ERRORS: list[int] = []


def front() -> str | None:
    """The owner's app (bundle id): Accessibility's focused application, retried briefly; when that read fails
    (seen on this Mac: kAXErrorCannotComplete -25204 while the owner was in a full-screen app), NSWorkspace's
    frontmostApplication. Returns "ws:<bundle>" for the fallback so the two sources stay distinguishable."""
    AX = _ax()
    from AppKit import NSRunningApplication, NSWorkspace
    for _ in range(3):
        err, app = AX.AXUIElementCopyAttributeValue(AX.AXUIElementCreateSystemWide(), "AXFocusedApplication", None)
        if err == 0 and app is not None:
            err, pid = AX.AXUIElementGetPid(app, None)
            ra = NSRunningApplication.runningApplicationWithProcessIdentifier_(pid) if err == 0 else None
            if ra is not None and ra.bundleIdentifier():
                return str(ra.bundleIdentifier())
        AX_FRONT_ERRORS.append(int(err))
        time.sleep(0.05)
    fm = NSWorkspace.sharedWorkspace().frontmostApplication()
    return f"ws:{fm.bundleIdentifier()}" if fm is not None and fm.bundleIdentifier() else None


def app_windows(pid: int) -> list[dict[str, Any]]:
    """The app's real windows from the window server (AXWindows can be empty when they sit on another Space)."""
    import Quartz
    on = {w.get("kCGWindowNumber") for w in
          Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID) or []}
    out = []
    for w in Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionAll, Quartz.kCGNullWindowID) or []:
        if int(w.get("kCGWindowOwnerPID", 0)) == pid and int(w.get("kCGWindowLayer", 0)) == 0 and w.get("kCGWindowName"):
            out.append({"name": str(w.get("kCGWindowName")), "onscreen": w.get("kCGWindowNumber") in on,
                        "bounds": dict(w.get("kCGWindowBounds") or {})})
    return out


def mouse() -> tuple[float, float]:
    import Quartz
    ev = Quartz.CGEventCreate(None)
    p = Quartz.CGEventGetLocation(ev)
    return round(float(p.x), 1), round(float(p.y), 1)


def pid_of(bundle: str, want: int | None = None) -> int | None:
    from AppKit import NSRunningApplication
    for a in NSRunningApplication.runningApplicationsWithBundleIdentifier_(bundle):
        if not a.isTerminated() and (want is None or int(a.processIdentifier()) == want):
            return int(a.processIdentifier())
    return None


def instances(bundle: str) -> int:
    from AppKit import NSRunningApplication
    return sum(1 for a in NSRunningApplication.runningApplicationsWithBundleIdentifier_(bundle) if not a.isTerminated())


def walk(root: Any, cap: int = WALK_CAP) -> tuple[int, int, bool]:
    """(elements seen, AXGroup elements that expose actions, truncated)."""
    seen, groups_with_actions, stack = 0, 0, [root]
    while stack:
        el = stack.pop()
        seen += 1
        if seen > cap:
            return seen - 1, groups_with_actions, True
        if str(attr(el, "AXRole") or "") == "AXGroup" and actions(el):
            groups_with_actions += 1
        stack.extend(attr(el, "AXChildren") or [])
    return seen, groups_with_actions, False


def find(root: Any, pred: Callable[[Any], bool], cap: int = WALK_CAP) -> Any:
    seen, stack = 0, [root]
    while stack and seen < cap:
        el = stack.pop(0)
        seen += 1
        if pred(el):
            return el
        stack.extend(attr(el, "AXChildren") or [])
    return None


def menu_item(app: Any, path: list[str]) -> Any:
    level = attr(app, "AXMenuBar")
    el = None
    for i, name in enumerate(path):
        kids = attr(level, "AXChildren") or []
        if i > 0:
            kids = [x for m in kids for x in (attr(m, "AXChildren") or [])]
        el = next((k for k in kids if str(attr(k, "AXTitle") or "").strip().lower() == name.lower()), None)
        if el is None:
            return None
        level = el
    return el


def top_window_at(x: float, y: float) -> dict[str, Any] | None:
    """Front-most on-screen layer-0 window containing the point, from CGWindowList (front to back)."""
    import Quartz
    for w in Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionOnScreenOnly, Quartz.kCGNullWindowID) or []:
        if int(w.get("kCGWindowLayer", 0)) != 0 or float(w.get("kCGWindowAlpha", 1)) == 0:
            continue
        b = w.get("kCGWindowBounds") or {}
        if b.get("X", 0) <= x <= b.get("X", 0) + b.get("Width", 0) and b.get("Y", 0) <= y <= b.get("Y", 0) + b.get("Height", 0):
            return {"owner": str(w.get("kCGWindowOwnerName")), "pid": int(w.get("kCGWindowOwnerPID", 0)),
                    "name": str(w.get("kCGWindowName") or "")}
    return None


# ---------------------------------------------------------------- event posting (to the pid only)

def _src():
    import Quartz
    return Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)


def post_unicode(pid: int, text: str) -> None:
    import Quartz
    src = _src()
    for ch in text:
        for down in (True, False):
            ev = Quartz.CGEventCreateKeyboardEvent(src, 0, down)
            Quartz.CGEventKeyboardSetUnicodeString(ev, len(ch), ch)
            Quartz.CGEventPostToPid(pid, ev)
        time.sleep(0.02)


def post_keycode(pid: int, keycode: int, flags: int = 0) -> None:
    import Quartz
    src = _src()
    for down in (True, False):
        ev = Quartz.CGEventCreateKeyboardEvent(src, keycode, down)
        if flags:
            Quartz.CGEventSetFlags(ev, flags)
        Quartz.CGEventPostToPid(pid, ev)


WINDOW_UNDER_POINTER, WINDOW_THAT_CAN_HANDLE = 91, 92   # kCGMouseEventWindowUnderMousePointer[ThatCanHandleThisEvent]


def _target(ev: Any, window_number: int | None) -> None:
    """Stamp the window-number fields AppKit reads to route a mouse event to a window (CGEventTypes.h)."""
    import Quartz
    if window_number:
        Quartz.CGEventSetIntegerValueField(ev, WINDOW_UNDER_POINTER, window_number)
        Quartz.CGEventSetIntegerValueField(ev, WINDOW_THAT_CAN_HANDLE, window_number)


def post_click(pid: int, x: float, y: float, clicks: int = 1, window_number: int | None = None) -> None:
    import Quartz
    src = _src()
    pt = (x, y)
    mv = Quartz.CGEventCreateMouseEvent(src, Quartz.kCGEventMouseMoved, pt, Quartz.kCGMouseButtonLeft)
    _target(mv, window_number)
    Quartz.CGEventPostToPid(pid, mv)
    for n in range(1, clicks + 1):
        for kind in (Quartz.kCGEventLeftMouseDown, Quartz.kCGEventLeftMouseUp):
            ev = Quartz.CGEventCreateMouseEvent(src, kind, pt, Quartz.kCGMouseButtonLeft)
            Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGMouseEventClickState, n)
            _target(ev, window_number)
            Quartz.CGEventPostToPid(pid, ev)
        time.sleep(0.05)


def post_drag(pid: int, x0: float, y0: float, x1: float, y1: float, window_number: int | None = None) -> None:
    import Quartz
    src = _src()
    down = Quartz.CGEventCreateMouseEvent(src, Quartz.kCGEventLeftMouseDown, (x0, y0), Quartz.kCGMouseButtonLeft)
    _target(down, window_number)
    Quartz.CGEventPostToPid(pid, down)
    steps = 6
    for i in range(1, steps + 1):
        p = (x0 + (x1 - x0) * i / steps, y0 + (y1 - y0) * i / steps)
        ev = Quartz.CGEventCreateMouseEvent(src, Quartz.kCGEventLeftMouseDragged, p, Quartz.kCGMouseButtonLeft)
        _target(ev, window_number)
        Quartz.CGEventPostToPid(pid, ev)
        time.sleep(0.02)
    up = Quartz.CGEventCreateMouseEvent(src, Quartz.kCGEventLeftMouseUp, (x1, y1), Quartz.kCGMouseButtonLeft)
    _target(up, window_number)
    Quartz.CGEventPostToPid(pid, up)


def post_scroll(pid: int, x: float, y: float, lines: int, window_number: int | None = None) -> None:
    import Quartz
    ev = Quartz.CGEventCreateScrollWheelEvent(_src(), Quartz.kCGScrollEventUnitLine, 1, lines)
    Quartz.CGEventSetLocation(ev, (x, y))
    _target(ev, window_number)
    Quartz.CGEventPostToPid(pid, ev)


def window_number(pid: int, name: str) -> int | None:
    import Quartz
    for w in Quartz.CGWindowListCopyWindowInfo(Quartz.kCGWindowListOptionAll, Quartz.kCGNullWindowID) or []:
        if int(w.get("kCGWindowOwnerPID", 0)) == pid and str(w.get("kCGWindowName") or "") == name \
                and int(w.get("kCGWindowLayer", 0)) == 0 and (w.get("kCGWindowBounds") or {}).get("Width", 0) > 0:
            return int(w["kCGWindowNumber"])
    return None


def ax_post_text(app: Any, text: str) -> None:
    """Accessibility keyboard API (AXUIElementPostKeyboardEvent): needs real virtual keycodes, US layout."""
    AX = _ax()
    layout = {c: k for c, k in zip("asdfhgzxcvbqweryt123465=97-80]ou[ip", range(35))}
    layout.update(dict(zip("lj'k;\\,/nm.` ", [37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 50, 49])))
    for ch in text:
        kc = layout[ch]
        AX.AXUIElementPostKeyboardEvent(app, ord(ch), kc, True)
        AX.AXUIElementPostKeyboardEvent(app, ord(ch), kc, False)
        time.sleep(0.02)


def osascript(bundle: str, body: str) -> tuple[bool, str]:
    script = f'tell application id "{bundle}"\n{body}\nend tell'
    p = subprocess.run(["osascript", "-e", script], capture_output=True, text=True, timeout=20)
    return p.returncode == 0, (p.stdout or p.stderr).strip()


# ---------------------------------------------------------------- the probe

class Probe:
    def __init__(self, bundle: str, pid: int | None = None) -> None:
        if bundle not in ALLOWED:
            raise SystemExit(f"refused: {bundle} is not a probe target (allowed: {sorted(ALLOWED)})")
        self.bundle, self.want_pid = bundle, pid
        self.profile: dict[str, Any] = {
            "bundle": bundle, "ax_tree_size": None, "has_actions_on_groups": None, "ax_press_ok": None,
            "ax_set_value_ok": None, "ax_selected_text_ok": None, "ax_post_key_ok": None, "pid_keys_ok": None,
            "pid_keycode_ok": None, "pid_mouse_click_ok": None, "pid_mouse_drag_ok": None, "pid_scroll_ok": None,
            "menu_press_ok": None, "app_hit_test_ok": None, "ax_focus_ok": None, "front_app_unchanged": None,
            "mouse_unchanged": None, "notes": []}
        self.steps: list[dict[str, Any]] = []
        self.owner_front = front()
        self.mouse0 = mouse()
        self.front_changes: list[str] = []
        self.launched = False
        self.pid: int | None = None
        self.app: Any = None

    def note(self, s: str) -> None:
        self.profile["notes"].append(s)

    def step(self, name: str, fn: Callable[[], Any]) -> Any:
        before = front()
        t = time.monotonic()
        try:
            result = fn()
            rec: dict[str, Any] = {"step": name, "result": result}
        except Exception as e:                                   # never let one mechanism stop the rest
            result = None
            rec = {"step": name, "error": f"{type(e).__name__}: {e}"}
        after = front()
        rec["front_before"], rec["front_after"], rec["ms"] = before, after, int((time.monotonic() - t) * 1000)
        same = lambda a, b: (a or "").removeprefix("ws:") == (b or "").removeprefix("ws:")
        if not same(before, self.owner_front) or not same(after, self.owner_front):
            self.front_changes.append(f"{name}: {before} -> {after} (owner app was {self.owner_front})")
        self.steps.append(rec)
        return result

    # ---- setup

    def ensure_running(self) -> None:
        self.pid = self.want_pid or pid_of(self.bundle)
        if self.want_pid is not None and self.want_pid != pid_of(self.bundle, self.want_pid):
            raise SystemExit(f"pid {self.want_pid} is not a running {self.bundle}")
        if self.pid is None:
            subprocess.run(["open", "-g", "-b", self.bundle], check=False)
            self.launched = True
            for _ in range(60):
                time.sleep(0.25)
                self.pid = pid_of(self.bundle)
                if self.pid is not None:
                    break
            time.sleep(2.0)
        if self.pid is None:
            raise SystemExit(f"{self.bundle} is not running and could not be launched")
        self.app = _ax().AXUIElementCreateApplication(self.pid)
        self.profile["pid"], self.profile["launched_by_probe"] = self.pid, self.launched
        if front() == self.bundle:
            self.note("the target app was the owner's frontmost app when the probe started: acting steps skipped")
            self.profile["target_was_frontmost"] = True

    def generic(self) -> None:
        self.step("set_AXEnhancedUserInterface", lambda: set_attr(self.app, "AXEnhancedUserInterface", True))
        self.step("set_AXManualAccessibility", lambda: set_attr(self.app, "AXManualAccessibility", True))
        wins = attr(self.app, "AXWindows") or []
        self.profile["ax_windows"] = [str(attr(w, "AXTitle") or "") for w in wins]
        self.profile["cg_windows"] = app_windows(self.pid)
        root = attr(self.app, "AXFocusedWindow") or attr(self.app, "AXMainWindow") or (wins[0] if wins else None)
        if root is None:                                         # seen: AXWindows empty, focused element still live
            root = attr(attr(self.app, "AXFocusedUIElement"), "AXWindow")
            self.profile["root_window_via_focused_element"] = root is not None
        if root is not None:
            self.profile["root_window"] = {"title": str(attr(root, "AXTitle") or ""), "rect": rect(root)}
        if root is None:
            self.note("no window to walk")
            return
        n, g, trunc = self.step("walk_tree", lambda: walk(root))
        self.profile["ax_tree_size"], self.profile["has_actions_on_groups"] = n, g > 0
        self.profile["groups_with_actions"], self.profile["tree_truncated"] = g, trunc
        c = center(root)
        if c is not None:
            self.hit_test(c, "window centre")

    def hit_test(self, c: tuple[float, float], what: str) -> None:
        AX = _ax()

        def run() -> dict[str, Any]:
            err, el = AX.AXUIElementCopyElementAtPosition(self.app, c[0], c[1], None)
            app_hit = {"err": int(err), "role": str(attr(el, "AXRole")) if el is not None else None,
                       "pid": int(AX.AXUIElementGetPid(el, None)[1]) if el is not None else None,
                       "window": str(attr(attr(el, "AXWindow"), "AXTitle") or "") if el is not None else None}
            err2, el2 = AX.AXUIElementCopyElementAtPosition(AX.AXUIElementCreateSystemWide(), c[0], c[1], None)
            sys_hit = {"err": int(err2), "role": str(attr(el2, "AXRole")) if el2 is not None else None,
                       "pid": int(AX.AXUIElementGetPid(el2, None)[1]) if el2 is not None else None}
            top = top_window_at(*c)
            covered = top is not None and top["pid"] != self.pid
            return {"point": c, "what": what, "app_element_hit": app_hit, "system_wide_hit": sys_hit,
                    "top_window_at_point": top, "covered_by_other_app": covered}
        r = self.step(f"hit_test:{what}", run)
        if r:
            ok = (r["app_element_hit"]["err"] == 0 and r["app_element_hit"]["pid"] == self.pid
                  and r["app_element_hit"]["role"] not in (None, "AXMenuBar", "AXApplication"))
            r["target_window_onscreen"] = any(w["onscreen"] for w in app_windows(self.pid)
                                              if w["bounds"].get("X", 0) <= c[0] <= w["bounds"].get("X", 0) + w["bounds"].get("Width", 0)
                                              and w["bounds"].get("Y", 0) <= c[1] <= w["bounds"].get("Y", 0) + w["bounds"].get("Height", 0))
            self.profile["app_hit_test_ok"] = ok if self.profile["app_hit_test_ok"] is None else \
                (self.profile["app_hit_test_ok"] and ok)
            if r["covered_by_other_app"]:
                self.profile["app_hit_test_covered_ok"] = ok
                self.profile["system_wide_hit_returns_target"] = r["system_wide_hit"]["pid"] == self.pid

    # ---- TextEdit

    def textedit(self) -> None:
        AX = _ax()
        keys_before = {(w["name"], tuple(sorted(w["bounds"].items()))) for w in app_windows(self.pid)}
        item = menu_item(self.app, ["File", "New"])
        self.profile["file_new_enabled"] = bool(attr(item, "AXEnabled")) if item is not None else None
        r = self.step("menu_press File>New", lambda: perform(item, "AXPress") if item is not None else "no item")
        new, new_cg = None, None
        for _ in range(12):
            time.sleep(0.25)
            new_cg = next((w for w in app_windows(self.pid)
                           if (w["name"], tuple(sorted(w["bounds"].items()))) not in keys_before
                           and w["name"].startswith("Untitled")), None)
            if new_cg is None:
                continue
            w = attr(self.app, "AXFocusedWindow") or attr(self.app, "AXMainWindow") \
                or attr(attr(self.app, "AXFocusedUIElement"), "AXWindow")
            if w is not None and str(attr(w, "AXTitle") or "") == new_cg["name"]:
                new = w
                break
        self.profile["menu_press_ok"] = r == 0 and new_cg is not None
        self.profile["new_window_cg"] = new_cg
        if new is None and new_cg is not None:
            self.note("File > New made a window but Accessibility exposes no window object for it")
        if new is None:
            self.note("File > New made no new Untitled window; TextEdit edits skipped")
            return
        self.doc_title = str(attr(new, "AXTitle"))
        self.profile["created_document"] = self.doc_title
        area = find(new, lambda e: str(attr(e, "AXRole") or "") == "AXTextArea")
        if area is None and str(attr(attr(self.app, "AXFocusedUIElement"), "AXRole") or "") == "AXTextArea":
            area = attr(self.app, "AXFocusedUIElement")
            self.note("text area taken from AXFocusedUIElement (not found under the window)")
        if area is None:
            self.note("no AXTextArea in the new document")
            return
        self.profile["text_area_actions"] = actions(area)
        self.profile["text_area_settable"] = {
            a: bool(AX.AXUIElementIsAttributeSettable(area, a, None)[1])
            for a in ("AXValue", "AXSelectedText", "AXSelectedTextRange", "AXFocused")}
        val = lambda: str(attr(area, "AXValue") or "")

        # (1) AXValue whole-value set
        r = self.step("ax_set_value", lambda: set_attr(area, "AXValue", "alpha "))
        time.sleep(SETTLE)
        self.profile["ax_set_value_ok"] = r == 0 and val().startswith("alpha")
        # (2) AXSelectedTextRange + AXSelectedText insertion at the end
        def sel_insert() -> dict[str, Any]:
            n = len(val())
            e1 = set_attr(area, "AXSelectedTextRange", cfrange(n, 0))
            rng = read_range(attr(area, "AXSelectedTextRange"))
            e2 = set_attr(area, "AXSelectedText", "bravo ")
            return {"set_range_err": e1, "range_after_set": rng, "set_text_err": e2}
        r = self.step("ax_selected_text_insert", sel_insert)
        time.sleep(SETTLE)
        self.profile["ax_selected_text_ok"] = bool(r) and r["set_text_err"] == 0 and "bravo" in val()
        # (2b) replace a range (edit in the middle)
        def sel_replace() -> dict[str, Any]:
            v = val()
            i = v.find("alpha")
            e1 = set_attr(area, "AXSelectedTextRange", cfrange(i, len("alpha")))
            e2 = set_attr(area, "AXSelectedText", "ALPHA")
            return {"set_range_err": e1, "set_text_err": e2, "value": val()}
        r = self.step("ax_selected_text_replace", sel_replace)
        self.profile["ax_selected_text_replace_ok"] = bool(r) and "ALPHA" in r["value"]
        # (3) focus + Accessibility keyboard
        rf = self.step("ax_focus", lambda: set_attr(area, "AXFocused", True))
        focused = attr(self.app, "AXFocusedUIElement")
        import CoreFoundation as CF
        self.profile["ax_focus_ok"] = rf == 0 and focused is not None and bool(CF.CFEqual(focused, area))
        set_attr(area, "AXSelectedTextRange", cfrange(len(val()), 0))
        before = val()
        self.step("ax_post_keyboard", lambda: ax_post_text(self.app, "charlie "))
        time.sleep(SETTLE)
        self.profile["ax_post_key_ok"] = "charlie" in val().lower() and val() != before
        # (4) CGEventPostToPid, unicode events
        before = val()
        self.step("pid_keys_unicode", lambda: post_unicode(self.pid, "delta "))
        time.sleep(SETTLE)
        self.profile["pid_keys_ok"] = "delta" in val().lower() and val() != before
        # (4b) CGEventPostToPid, keycode-only events (kVK_ANSI_E = 14, kVK_Space = 49)
        before = val()
        self.step("pid_keys_keycode", lambda: [post_keycode(self.pid, k) for k in (14, 49)])
        time.sleep(SETTLE)
        self.profile["pid_keycode_ok"] = val() == before + "e " or val().lower().endswith("e ")
        # (5) mouse click to pid: caret should move to the start of the text
        r0 = rect(area)
        if r0 is not None:
            x0, y0 = r0[0] + 4, r0[1] + 10
            self.hit_test((x0, y0), "text area")
            set_attr(area, "AXSelectedTextRange", cfrange(len(val()), 0))
            self.step("pid_mouse_click", lambda: post_click(self.pid, x0, y0))
            time.sleep(SETTLE)
            after = read_range(attr(area, "AXSelectedTextRange"))
            self.profile["pid_mouse_click_ok"] = after is not None and after[0] <= 1
            self.profile["caret_after_pid_click"] = after
            wn = window_number(self.pid, self.doc_title)
            self.profile["window_number"] = wn
            set_attr(area, "AXSelectedTextRange", cfrange(len(val()), 0))
            self.step("pid_mouse_click+window_number", lambda: post_click(self.pid, x0, y0, window_number=wn))
            time.sleep(SETTLE)
            after = read_range(attr(area, "AXSelectedTextRange"))
            self.profile["pid_mouse_click_winnum_ok"] = after is not None and after[0] <= 1
            self.profile["caret_after_pid_click_winnum"] = after
            # (5b) double-click selects a word
            self.step("pid_mouse_double_click", lambda: post_click(self.pid, x0, y0, clicks=2))
            time.sleep(SETTLE)
            self.profile["selected_after_pid_double_click"] = str(attr(area, "AXSelectedText") or "")
            # (5c) drag selects a range
            set_attr(area, "AXSelectedTextRange", cfrange(0, 0))
            self.step("pid_mouse_drag", lambda: post_drag(self.pid, x0, y0, x0 + 60, y0))
            time.sleep(SETTLE)
            sel = str(attr(area, "AXSelectedText") or "")
            self.profile["pid_mouse_drag_ok"] = len(sel) > 0
            self.profile["selected_after_pid_drag"] = sel
            set_attr(area, "AXSelectedTextRange", cfrange(0, 0))
            self.step("pid_mouse_drag+window_number", lambda: post_drag(self.pid, x0, y0, x0 + 60, y0, window_number=wn))
            time.sleep(SETTLE)
            sel = str(attr(area, "AXSelectedText") or "")
            self.profile["pid_mouse_drag_winnum_ok"] = len(sel) > 0
            # (6) scroll to pid: fill the document, then scroll the text area
            set_attr(area, "AXValue", "\n".join(f"line {i}" for i in range(400)))
            set_attr(area, "AXSelectedTextRange", cfrange(0, 0))
            time.sleep(SETTLE)
            scroll_area = attr(area, "AXParent")
            bar = attr(scroll_area, "AXVerticalScrollBar")
            def pos() -> dict[str, Any]:
                return {"scrollbar": attr(bar, "AXValue"), "visible": read_range(attr(area, "AXVisibleCharacterRange"))}
            p0 = pos()
            cx, cy = r0[0] + r0[2] / 2, r0[1] + r0[3] / 2
            self.step("pid_scroll", lambda: [post_scroll(self.pid, cx, cy, -8) for _ in range(5)])
            time.sleep(SETTLE)
            p1 = pos()
            self.profile["pid_scroll_ok"] = p0 != p1 and (p1["scrollbar"] or 0) > (p0["scrollbar"] or 0)
            self.profile["scroll_before_after"] = [p0, p1]
            self.step("pid_scroll+window_number", lambda: [post_scroll(self.pid, cx, cy, -8, window_number=wn) for _ in range(5)])
            time.sleep(SETTLE)
            p2 = pos()
            self.profile["pid_scroll_winnum_ok"] = p2 != p1 and (p2["scrollbar"] or 0) > (p1["scrollbar"] or 0)
            self.profile["scroll_after_winnum"] = p2
            # (6b) AXScrollToVisible on the first line, as the AX alternative
            if "AXScrollToVisible" in actions(area):
                self.step("ax_scroll_to_visible", lambda: perform(area, "AXScrollToVisible"))
        # (7) AXPress on a toggle that only changes the view: Format > Font > Show Fonts is a panel; use the ruler
        item = menu_item(self.app, ["Format", "Text", "Show Ruler"]) or menu_item(self.app, ["Format", "Text", "Hide Ruler"])
        if item is not None:
            t0 = str(attr(item, "AXTitle"))
            r = self.step(f"menu_press Format>Text>{t0}", lambda: perform(item, "AXPress"))
            time.sleep(SETTLE)
            back = menu_item(self.app, ["Format", "Text", "Hide Ruler" if t0 == "Show Ruler" else "Show Ruler"])
            self.profile["ax_press_ok"] = r == 0 and back is not None
            if back is not None:
                self.step("menu_press restore ruler", lambda: perform(back, "AXPress"))
        self.profile["final_value_head"] = val()[:80]

    def textedit_cleanup(self) -> None:
        title = getattr(self, "doc_title", None)
        if not title:
            return
        if instances(self.bundle) == 1:
            ok, out = osascript(self.bundle, f'close document "{title}" saving no')
            self.profile["cleanup"] = {"close_document": ok, "detail": out[:200]}
        else:                                                    # AppleScript can't pick an instance: close through AX
            ok, out = self.ax_close_untitled(title)
            self.profile["cleanup"] = {"ax_close": ok, "detail": out}
        if not ok:
            self.note(f"could not close {title!r} through AppleScript (Automation permission?); it is left open "
                      "unsaved so the owner can discard it")

    def ax_close_untitled(self, title: str) -> tuple[bool, str]:
        """Close the probe's own untitled document: the window's close button, then "Delete" on the save sheet."""
        win = next((w for w in attr(self.app, "AXWindows") or [] if str(attr(w, "AXTitle") or "") == title), None) \
            or attr(self.app, "AXFocusedWindow") or attr(self.app, "AXMainWindow")
        if win is None or str(attr(win, "AXTitle") or "") != title:
            return False, "window not found through Accessibility"
        if perform(attr(win, "AXCloseButton"), "AXPress") != 0:
            return False, "close button press failed"
        for _ in range(24):                                      # the sheet shows up under AXChildren (role AXSheet)
            time.sleep(0.25)                                     # before/instead of AXSheets (probe, TextEdit 26.6)
            sheet = next(iter(attr(win, "AXSheets") or []), None) or next(
                (c for c in attr(win, "AXChildren") or [] if str(attr(c, "AXRole") or "") == "AXSheet"), None)
            if sheet is not None:
                btn = find(sheet, lambda e: str(attr(e, "AXRole") or "") == "AXButton"
                           and str(attr(e, "AXTitle") or "") == "Delete")
                if btn is None:
                    return False, "save sheet without a Delete button; left open"
                r = perform(btn, "AXPress")
                time.sleep(0.5)
                return r == 0 and window_number(self.pid, title) is None, f"Delete pressed (err {r})"
        return window_number(self.pid, title) is None, "no sheet appeared"

    # ---- Calculator

    def calculator(self) -> None:
        win = attr(self.app, "AXFocusedWindow") or attr(self.app, "AXMainWindow")
        if win is None:
            self.note("Calculator has no window")
            return
        def button(desc: str) -> Any:
            return find(win, lambda e: str(attr(e, "AXRole") or "") == "AXButton"
                        and str(attr(e, "AXDescription") or attr(e, "AXTitle") or "").strip().lower() == desc.lower())
        def display() -> str:
            el = find(win, lambda e: str(attr(e, "AXRole") or "") == "AXStaticText"
                      and str(attr(e, "AXDescription") or "").lower().startswith("main display"))
            if el is None:
                el = find(win, lambda e: str(attr(e, "AXRole") or "") in ("AXStaticText", "AXTextField")
                          and str(attr(e, "AXValue") or "").strip() != "")
            return str(attr(el, "AXValue") or "") if el is not None else ""
        clear = button("All Clear") or button("Clear") or button("AC") or button("C")
        self.profile["calc_buttons_found"] = {"1": button("1") is not None, "2": button("2") is not None,
                                              "clear": clear is not None, "display": display()}
        if clear is not None:
            self.step("ax_press clear", lambda: perform(clear, "AXPress"))
        # (1) AXPress digit buttons
        b1 = button("1")
        r = self.step("ax_press 1", lambda: perform(b1, "AXPress") if b1 is not None else "no button")
        time.sleep(SETTLE)
        d = display()
        self.profile["ax_press_ok"] = r == 0 and d.strip().endswith("1")
        self.profile["display_after_ax_press"] = d
        digits = lambda s: "".join(ch for ch in s if ch.isdigit())   # each check: the display just before + the digit
        # (2) AX keyboard
        before = digits(display())
        self.step("ax_post_keyboard 2", lambda: ax_post_text(self.app, "2"))
        time.sleep(SETTLE * 2)
        d = display()
        self.profile["ax_post_key_ok"] = digits(d) == before + "2"
        self.profile["display_after_ax_keyboard"] = d
        # (3) pid keys
        before = digits(display())
        self.step("pid_keys_unicode 3", lambda: post_unicode(self.pid, "3"))
        time.sleep(SETTLE)
        d = display()
        self.profile["pid_keys_ok"] = digits(d) == before + "3"
        self.profile["display_after_pid_keys"] = d
        before = digits(display())
        self.step("pid_keys_keycode 4 (kVK_ANSI_4=21)", lambda: post_keycode(self.pid, 21))
        time.sleep(SETTLE * 2)
        d = display()
        self.profile["pid_keycode_ok"] = digits(d) == before + "4"
        self.profile["display_after_pid_keycode"] = d
        # (4) pid mouse click on the "5" button
        b5 = button("5")
        c = center(b5) if b5 is not None else None
        if c is not None:
            self.hit_test(c, "button 5")
            before = digits(display())
            self.step("pid_mouse_click 5", lambda: post_click(self.pid, *c))
            time.sleep(SETTLE)
            d = display()
            self.profile["pid_mouse_click_ok"] = digits(d) == before + "5"
            self.profile["display_after_pid_click"] = d
        if clear is not None:
            self.step("ax_press clear (restore)", lambda: perform(clear, "AXPress"))
        # (5) View menu: switch mode and restore
        view = menu_item(self.app, ["View"])
        items = [x for m in (attr(view, "AXChildren") or []) for x in (attr(m, "AXChildren") or [])]
        checked = next((x for x in items if attr(x, "AXMenuItemMarkChar")), None)
        other = next((x for x in items if x is not checked and str(attr(x, "AXTitle") or "") in ("Basic", "Scientific", "Programmer")), None)
        if checked is not None and other is not None:
            t_checked, t_other = str(attr(checked, "AXTitle")), str(attr(other, "AXTitle"))
            w0 = rect(win)
            r = self.step(f"menu_press View>{t_other}", lambda: perform(other, "AXPress"))
            time.sleep(1.5)
            fresh = [x for m in (attr(menu_item(self.app, ["View"]), "AXChildren") or [])
                     for x in (attr(m, "AXChildren") or [])]
            now = next((str(attr(x, "AXTitle")) for x in fresh if attr(x, "AXMenuItemMarkChar")), None)
            w1 = rect(attr(self.app, "AXFocusedWindow") or attr(self.app, "AXMainWindow"))
            self.profile["menu_press_ok"] = r == 0 and (now == t_other or w0 != w1)
            self.profile["calc_mode_switch"] = {"checked_after": now, "window_before": w0, "window_after": w1}
            self.step(f"menu_press View>{t_checked} (restore)", lambda: perform(checked, "AXPress"))
            time.sleep(1.5)
        self.profile["pid_scroll_ok"] = None
        self.note("Calculator: no scrollable or text-editing target; scroll / set-value / selected-text not applicable")

    # ---- Finder (read-only, plus a view toggle on the front window that is restored)

    def finder(self) -> None:
        view = menu_item(self.app, ["View"])
        items = [x for m in (attr(view, "AXChildren") or []) for x in (attr(m, "AXChildren") or [])]
        modes = {str(attr(x, "AXTitle") or ""): x for x in items
                 if str(attr(x, "AXTitle") or "") in ("as Icons", "as List", "as Columns", "as Gallery")}
        checked = next((t for t, x in modes.items() if attr(x, "AXMenuItemMarkChar")), None)
        enabled = {t: bool(attr(x, "AXEnabled")) for t, x in modes.items()}
        self.profile["finder_view_items"] = {"checked": checked, "enabled": enabled}
        if checked and enabled.get("as List") and enabled.get("as Icons"):
            target = "as List" if checked != "as List" else "as Icons"
            r = self.step(f"menu_press View>{target}", lambda: perform(modes[target], "AXPress"))
            time.sleep(0.8)
            now = next((t for t, x in modes.items() if attr(x, "AXMenuItemMarkChar")), None)
            self.profile["menu_press_ok"] = r == 0 and now == target
            self.step(f"menu_press View>{checked} (restore)", lambda: perform(modes[checked], "AXPress"))
        else:
            self.note("Finder: no front window with view modes enabled; menu press skipped")
        self.note("Finder: acting steps limited to a view-mode toggle (restored); no typing, clicks or scrolls")

    # ---- run

    def run(self) -> dict[str, Any]:
        self.ensure_running()
        try:
            self.generic()
            if not self.profile.get("target_was_frontmost"):
                if self.bundle == "com.apple.TextEdit":
                    self.textedit()
                elif self.bundle == "com.apple.calculator":
                    self.calculator()
                elif self.bundle == "com.apple.finder":
                    self.finder()
        finally:
            if self.bundle == "com.apple.TextEdit":
                self.textedit_cleanup()
        self.profile["cg_windows_at_end"] = app_windows(self.pid)
        self.profile["ax_focused_app_errors"] = sorted(set(AX_FRONT_ERRORS))
        self.profile["front_app_unchanged"] = not self.front_changes
        self.profile["front_changes"] = self.front_changes
        self.profile["owner_front_app"] = self.owner_front
        self.profile["mouse_unchanged"] = mouse() == self.mouse0
        self.profile["mouse_before_after"] = [self.mouse0, mouse()]
        self.profile["steps"] = self.steps
        return self.profile


def main(argv: list[str]) -> int:
    if not argv or argv[0].startswith("-"):
        print(__doc__)
        return 2
    pid = next((int(a.split("=", 1)[1]) for a in argv if a.startswith("--pid=")), None)
    profile = Probe(argv[0], pid=pid).run()
    if "--seed" in argv:
        from ferb.act.mac.driver import profiles_dir
        from ferb.act.mac.profile import Profiles
        from ferb.paths import ferb_home
        written = Profiles(profiles_dir(ferb_home())).seed(argv[0], profile)
        profile["seeded"] = written
    if "--json-only" in argv:
        profile = {k: v for k, v in profile.items() if k != "steps"}
    print(json.dumps(profile, indent=1, default=str, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
