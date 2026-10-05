"""Measure desktop/window observations for explicit diagnostics. Must not move Spaces or restore the owner desktop.

`ferb watch`: the owner-verified background test (owner's request, 2026-10-02).

Runs every kind of interaction FERB uses, one at a time, with the target app or browser in the background, and after
each one asks the owner what they saw. The owner's answer is the verdict; FERB's own detector (Space changes, app
activations, the frontmost app polled through Accessibility) is recorded next to it so stale assumptions can't fool
either side. Results: ~/.ferb/watch/<timestamp>.json.

Mac steps act only on TextEdit and Calculator (documents the test creates; nothing is ever saved, sent or deleted;
apps are never quit). Browser steps use a throwaway Chrome with its own user-data-dir under the FERB home — never the
owner's profiles, never FERB's identity session. No GUI: the terminal asks the questions."""

from __future__ import annotations

import functools
import http.server
import itertools
import json
import os
import queue
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, TextIO

TEXTEDIT, CALC = "com.apple.TextEdit", "com.apple.calculator"
CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
HERE = Path(__file__).parent
PAGE_FILE, EXT_DIR = HERE / "watch_page.html", HERE / "watch_ext"
ANSWERS = {"": "nothing", "n": "nothing", "s": "desktop switched", "a": "app came forward", "f": "a flash",
           "m": "mouse moved", "w": "a window appeared"}
PROMPT = ("   What did you see?  [Enter]=nothing  s=desktop/Space switched  a=an app or window came forward  "
          "f=a flash  m=my mouse moved  w=a window appeared  o=other (describe) > ")


# ---------- the detector: what macOS itself says happened ----------

DETECTOR_SRC = r'''
import json, sys, time, objc
from AppKit import NSObject, NSWorkspace, NSWorkspaceActiveSpaceDidChangeNotification, NSWorkspaceDidActivateApplicationNotification
from PyObjCTools import AppHelper
out = open(sys.argv[1], "a", buffering=1)
def write(kind, app=None): out.write(json.dumps({"t": round(time.time(), 3), "event": kind, "app": app}) + "\n")
class W(NSObject):
    def space_(self, n):
        f = NSWorkspace.sharedWorkspace().frontmostApplication()
        write("desktop_changed", str(f.bundleIdentifier()) if f else None)
    def act_(self, n): write("app_activated", str(n.userInfo()["NSWorkspaceApplicationKey"].bundleIdentifier()))
w = W.new(); nc = NSWorkspace.sharedWorkspace().notificationCenter()
nc.addObserver_selector_name_object_(w, objc.selector(w.space_, signature=b"v@:@"), NSWorkspaceActiveSpaceDidChangeNotification, None)
nc.addObserver_selector_name_object_(w, objc.selector(w.act_, signature=b"v@:@"), NSWorkspaceDidActivateApplicationNotification, None)
write("started"); AppHelper.runConsoleEventLoop(installInterrupt=True)
'''


def _front() -> str | None:
    """The owner's app: Accessibility's focused application first (NSWorkspace goes stale in a long process)."""
    try:
        from ferb.sense.watchers.base import focused_app
        f = focused_app()
        if f is not None:
            return f.bundle_id
    except Exception:
        pass
    try:
        from AppKit import NSWorkspace
        a = NSWorkspace.sharedWorkspace().frontmostApplication()
        return str(a.bundleIdentifier()) if a else None
    except Exception:
        return None


def _range(v: Any) -> tuple[int, int] | None:
    """An AXValue CFRange as (location, length); pyobjc hands it back as a plain tuple, not a struct."""
    import ApplicationServices as AX
    if v is None:
        return None
    ok, rng = AX.AXValueGetValue(v, AX.kAXValueCFRangeType, None)
    if not ok:
        return None
    if hasattr(rng, "location"):
        return int(rng.location), int(rng.length)
    return int(rng[0]), int(rng[1])


def _mouse() -> tuple[float, float]:
    import Quartz
    p = Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))
    return round(p.x, 1), round(p.y, 1)


class Detector:
    def __init__(self, log: Path) -> None:
        self.log = log
        self.proc = subprocess.Popen([sys.executable, "-c", DETECTOR_SRC, str(log)], stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL)
        self.samples: list[tuple[float, str | None]] = []
        self._stop = threading.Event()
        threading.Thread(target=self._poll, daemon=True).start()

    def _poll(self) -> None:
        last: str | None = "?"
        while not self._stop.is_set():
            f = _front()
            if f != last:
                self.samples.append((time.time(), f))
                last = f
            time.sleep(0.05)

    def since(self, t0: float) -> dict[str, Any]:
        events = []
        try:
            for line in self.log.read_text().splitlines():
                e = json.loads(line)
                if e["t"] >= t0 and e["event"] != "started":
                    events.append(e)
        except OSError:
            pass
        return {"notifications": events, "frontmost_changes": [(round(t - t0, 2), f) for t, f in self.samples if t >= t0]}

    def stop(self) -> None:
        self._stop.set()
        self.proc.terminate()


# ---------- the run ----------

@dataclass
class Step:
    id: str
    title: str
    run: Callable[[], dict[str, Any]]
    experimental: bool = False          # unverified mechanism: may do nothing, must still not move the owner


@dataclass
class Session:
    out: TextIO
    ask: Callable[[str], str]
    detector: Detector
    owner_app: str | None
    results: list[dict[str, Any]] = field(default_factory=list)

    def do(self, step: Step) -> dict[str, Any]:
        print(f"\n{step.id}. {step.title}" + ("   [experimental]" if step.experimental else ""), file=self.out)
        print("   Watch your screen. Starting in 2 s …", file=self.out)
        time.sleep(2)
        t0, m0 = time.time(), _mouse()
        rec: dict[str, Any] = {"id": step.id, "title": step.title, "experimental": step.experimental}
        try:
            rec["result"] = step.run()
        except Exception as e:                                   # a failing step is a finding, not a crash
            rec["error"] = f"{type(e).__name__}: {str(e)[:300]}"
        time.sleep(1.2)
        rec["detector"] = self.detector.since(t0)
        rec["mouse_moved"] = _mouse() != m0
        rec["front_at_end"] = _front()
        print(f"   FERB's result: {json.dumps(rec.get('result', rec.get('error')), default=str)[:300]}", file=self.out)
        d = rec["detector"]
        if d["notifications"] or d["frontmost_changes"]:
            print(f"   Detector saw: {json.dumps(d, default=str)[:300]}", file=self.out)
        else:
            print("   Detector saw: nothing", file=self.out)
        rec["owner_saw"] = "pending"
        if step.id == "M14":
            rec["owner_action_worked"] = "u"
            rec["owner_typing_undisturbed"] = "u"
        self.results.append(rec)
        raw = self.ask(PROMPT).strip()
        rec["owner_saw"] = ANSWERS.get(raw.lower(), raw if raw.lower().startswith("o") is False else
                                       self.ask("   describe: ").strip())
        if step.id == "M14":
            rec["owner_action_worked"] = self.ask(
                "   Inspect the test document: did it scroll down? y=yes n=no u=unsure > ").strip().lower()
            rec["owner_typing_undisturbed"] = self.ask(
                "   Did keyboard focus stay with your app? y=yes n=no u=unsure > ").strip().lower()
        return rec


# ---------- Mac steps (TextEdit, Calculator) ----------

def _ax() -> Any:
    from ferb.act.mac import accessibility
    return accessibility


def _attr(el: Any, name: str) -> Any:
    return _ax()._attr(el, name)


def _set(el: Any, name: str, value: Any) -> int:
    import ApplicationServices as AX
    return int(AX.AXUIElementSetAttributeValue(el, name, value))


def _pids(bundle: str) -> list[int]:
    from AppKit import NSRunningApplication
    return [int(a.processIdentifier()) for a in NSRunningApplication.runningApplicationsWithBundleIdentifier_(bundle)
            if not a.isTerminated()]


def _launch_bg(bundle: str) -> dict[str, Any]:
    before = _pids(bundle)
    if before:
        return {"already_running": before, "launched": False}
    subprocess.run(["open", "-g", "-b", bundle], capture_output=True, check=False)
    for _ in range(40):
        time.sleep(0.25)
        if _pids(bundle):
            return {"launched": True, "pids": _pids(bundle)}
    return {"launched": False, "error": "did not start"}


def _find(el: Any, pred: Callable[[Any], bool], depth: int = 0) -> Any:
    if depth > 30 or el is None:
        return None
    if pred(el):
        return el
    for c in _attr(el, "AXChildren") or []:
        hit = _find(c, pred, depth + 1)
        if hit is not None:
            return hit
    return None


def _run_steps(steps: list[Any], pause: float = 0.02) -> int:
    for s in steps:
        s.post()
        time.sleep(pause)
    return len(steps)


class MacTest:
    def __init__(self) -> None:
        self.pid: int | None = None
        self.win: Any = None
        self.area: Any = None
        self.created: list[Any] = []
        self.calc_pid: int | None = None

    def _app_el(self) -> Any:
        import ApplicationServices as AX
        return AX.AXUIElementCreateApplication(self.pid)

    def open_textedit(self) -> dict[str, Any]:
        r = _launch_bg(TEXTEDIT)
        pids = _pids(TEXTEDIT)
        self.pid = max(pids) if pids else None          # several instances: the newest (the probe left one stuck)
        r["pid"], r["instances"] = self.pid, len(pids)
        return r

    def new_document(self) -> dict[str, Any]:
        ax = _ax()
        before = _attr(ax.app_window(self.pid), "AXTitle") if ax.app_window(self.pid) is not None else None
        seen = {w["id"] for w in ax.cg_windows(self.pid)}
        ok = ax.press_menu(self.pid, ["File", "New"])
        time.sleep(1.0)
        candidate = ax.app_window(self.pid)
        title = _attr(candidate, "AXTitle") if candidate is not None else None
        if not ok or not any(w["id"] not in seen and w["title"] == title for w in ax.cg_windows(self.pid)):
            raise RuntimeError("could not establish a new document; existing documents are untouched")
        self.win = candidate
        self.area = _find(self.win, lambda e: _attr(e, "AXRole") == "AXTextArea")
        if self.win is not None:
            self.created.append(self.win)
        return {"menu_pressed": ok, "window": _attr(self.win, "AXTitle") if self.win is not None else None,
                "previous_window": before, "text_area_found": self.area is not None}

    def set_value(self) -> dict[str, Any]:
        err = _set(self.area, "AXValue", "FERB watch test — line 1 set through Accessibility.\n")
        time.sleep(0.5)
        return {"ax_error": err, "value_now": str(_attr(self.area, "AXValue"))[:80]}

    def type_pid(self) -> dict[str, Any]:
        from ferb.act.mac.input import type_steps
        _set(self.area, "AXFocused", True)
        before = str(_attr(self.area, "AXValue") or "")
        n = _run_steps(type_steps("typed by keystrokes posted to the process. ", self.pid))
        time.sleep(0.6)
        after = str(_attr(self.area, "AXValue") or "")
        return {"keystrokes": n, "landed": after != before and "posted to the process" in after}

    def selected_text(self) -> dict[str, Any]:
        before = str(_attr(self.area, "AXValue") or "")
        err = _set(self.area, "AXSelectedText", "[inserted through AXSelectedText] ")
        time.sleep(0.5)
        after = str(_attr(self.area, "AXValue") or "")
        return {"ax_error": err, "landed": "[inserted through AXSelectedText]" in after and after != before}

    def _menu_item_for(self, cmd_char: str, modifiers: int = 0) -> Any:
        """The menu item a shortcut maps to (AXMenuItemCmdChar / AXMenuItemCmdModifiers), or None."""
        import ApplicationServices as AX
        bar = _attr(AX.AXUIElementCreateApplication(self.pid), "AXMenuBar")
        for top in _attr(bar, "AXChildren") or []:
            for menu in _attr(top, "AXChildren") or []:
                for item in _attr(menu, "AXChildren") or []:
                    if str(_attr(item, "AXMenuItemCmdChar") or "") == cmd_char and \
                            int(_attr(item, "AXMenuItemCmdModifiers") or 0) == modifiers:
                        return item
        return None

    def key_combo(self) -> dict[str, Any]:
        """A shortcut in the background: only when the app validates its menu item (a background app often has no
        first responder, so Edit items are disabled and the keystroke just beeps — the owner heard it, 2026-10-02).
        Otherwise the same edit through Accessibility attributes."""
        from ferb.act.mac.input import key_steps
        import ApplicationServices as AX
        if not str(_attr(self.area, "AXValue") or ""):
            _set(self.area, "AXValue", "FERB watch test: some text to select.\n")
            time.sleep(0.3)
        total = len(str(_attr(self.area, "AXValue") or ""))
        item = self._menu_item_for("A", 0)
        enabled = bool(_attr(item, "AXEnabled")) if item is not None else None
        out: dict[str, Any] = {"menu_item": str(_attr(item, "AXTitle")) if item is not None else None,
                               "menu_item_enabled_in_background": enabled, "document_chars": total}
        if enabled:
            _set(self.area, "AXFocused", True)
            _run_steps(key_steps("cmd+a", self.pid))
            time.sleep(0.5)
            r = _range(_attr(self.area, "AXSelectedTextRange"))
            out["via"], out["selected_chars"] = "keystroke cmd+A to the process", r[1] if r else None
        else:
            err = _set(self.area, "AXSelectedTextRange", AX.AXValueCreate(AX.kAXValueCFRangeType, (0, total)))
            time.sleep(0.3)
            r = _range(_attr(self.area, "AXSelectedTextRange"))
            out["via"], out["ax_error"], out["selected_chars"] = "AXSelectedTextRange (shortcut unavailable)", err, \
                r[1] if r else None
        out["landed"] = bool(out["selected_chars"]) and out["selected_chars"] == total
        _set(self.area, "AXSelectedTextRange", AX.AXValueCreate(AX.kAXValueCFRangeType, (total, 0)))
        return out

    def _fill_long(self) -> None:
        _set(self.area, "AXValue", "".join(f"line {i}\n" for i in range(1, 400)))
        time.sleep(0.5)

    def _visible(self) -> Any:
        r = _range(_attr(self.area, "AXVisibleCharacterRange"))
        return list(r) if r else None

    def scroll_keys(self) -> dict[str, Any]:
        from ferb.act.mac.input import KEYCODES, key_steps
        self._fill_long()
        _set(self.area, "AXFocused", True)
        before = self._visible()
        KEYCODES.setdefault("pagedown", 121)
        _run_steps(key_steps("pagedown", self.pid) * 3, pause=0.15)
        time.sleep(0.6)
        after = self._visible()
        return {"visible_before": before, "visible_after": after, "landed": before != after}

    def scroll_wheel_pid(self) -> dict[str, Any]:
        import Quartz
        from ferb.sense.watchers.owner_input import FERB_EVENT_MARK
        _set(self.area, "AXFocused", True)
        _run_steps(__import__("ferb.act.mac.input", fromlist=["key_steps"]).key_steps("cmd+up", self.pid))
        time.sleep(0.4)
        before = self._visible()
        for _ in range(10):
            ev = Quartz.CGEventCreateScrollWheelEvent(None, Quartz.kCGScrollEventUnitLine, 1, -10)
            Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGEventSourceUserData, FERB_EVENT_MARK)
            Quartz.CGEventPostToPid(self.pid, ev)
            time.sleep(0.03)
        time.sleep(0.6)
        after = self._visible()
        return {"visible_before": before, "visible_after": after, "landed": before != after}

    def scroll_window_targeted(self) -> dict[str, Any]:
        """Exercise the real driver on our disposable document; the owner's answers decide acceptance."""
        import asyncio
        from ferb.act.gui_lock import GuiLock
        from ferb.act.mac import apps, screenshot
        from ferb.act.mac.apps import RunningApp
        from ferb.act.mac.driver import MacComputerDriver
        from ferb.act.mac.input import RealInput
        pid = self.pid
        if pid is None or self.win is None or self.area is None or self.win not in self.created:
            raise RuntimeError("no test-owned TextEdit document; refusing input")
        self._fill_long()
        # Pin lookup to the instance that owns the document, including when several TextEdits are running.
        class PinnedApps:
            def find(self, name: str) -> Any:
                return RunningApp(TEXTEDIT, "TextEdit", pid, False)
            def frontmost(self) -> Any:
                return apps.frontmost()
            def activate(self, bundle: str) -> None:
                raise RuntimeError("the check must never activate an app")
        d = MacComputerDriver("watch", RealInput(lambda: True, GuiLock()), apps=PinnedApps())
        async def check() -> dict[str, Any]:
            before = self._visible()
            _, shot, _ = await d.screenshot(TEXTEDIT)
            expected = screenshot.front_window(pid, _ax().frame(self.win))
            if expected is None or expected.id != shot.window or any(
                    abs(v - e) > 2 for v, e in zip(expected.bounds, _ax().frame(self.win))):
                raise RuntimeError("capture is not the test document")
            x, y, w, h = _ax().frame(self.area)
            bx, by, bw, bh = shot.bounds
            # Intersect the often very tall AXTextArea with its captured viewport.
            left, top = max(x, bx), max(y, by)
            right, bottom = min(x + w, bx + bw), min(y + h, by + bh)
            if right <= left or bottom <= top:
                raise RuntimeError("test text area is outside the captured window")
            px = ((left + right) / 2 - bx) * shot.size[0] / bw
            py = ((top + bottom) / 2 - by) * shot.size[1] / bh
            result = await d.pointer_at(TEXTEDIT, shot.id, px, py, "scroll", amount=1)
            after = self._visible()
            return {"visible_before": before, "visible_after": after, "landed": before != after,
                    "same_space": screenshot.on_screen(shot.window), "driver": result.detail}
        return asyncio.run(check())

    def click_pid(self) -> dict[str, Any]:
        import ApplicationServices as AX
        import Quartz
        from ferb.sense.watchers.owner_input import FERB_EVENT_MARK
        ax = _ax()
        x, y = ax._point(_attr(self.area, "AXPosition"), AX.kAXValueCGPointType)
        w, h = ax._point(_attr(self.area, "AXSize"), AX.kAXValueCGSizeType)
        pt = (x + 40, y + min(h - 10, 30))
        _set(self.area, "AXFocused", True)
        _run_steps(__import__("ferb.act.mac.input", fromlist=["key_steps"]).key_steps("cmd+up", self.pid))
        time.sleep(0.3)

        def caret() -> Any:
            r = _range(_attr(self.area, "AXSelectedTextRange"))
            return r[0] if r else None
        before = caret()
        src = Quartz.CGEventSourceCreate(Quartz.kCGEventSourceStatePrivate)
        for kind in (Quartz.kCGEventMouseMoved, Quartz.kCGEventLeftMouseDown, Quartz.kCGEventLeftMouseUp):
            ev = Quartz.CGEventCreateMouseEvent(src, kind, pt, Quartz.kCGMouseButtonLeft)
            Quartz.CGEventSetIntegerValueField(ev, Quartz.kCGEventSourceUserData, FERB_EVENT_MARK)
            Quartz.CGEventPostToPid(self.pid, ev)
            time.sleep(0.03)
        time.sleep(0.5)
        after = caret()
        return {"point": pt, "caret_before": before, "caret_after": after, "landed": before != after}

    def open_calc(self) -> dict[str, Any]:
        r = _launch_bg(CALC)
        pids = _pids(CALC)
        self.calc_pid = max(pids) if pids else None
        r["pid"] = self.calc_pid
        return r

    def _calc_window(self) -> Any:
        if self.calc_pid is None:                                 # `--only M11`: open it first
            self.open_calc()
            time.sleep(1.0)
        return _ax().app_window(self.calc_pid)

    def _calc_display(self) -> str | None:
        """Everything Calculator shows as text (the expression line and the result), direction marks stripped."""
        texts: list[str] = []

        def walk(e: Any, d: int) -> None:
            if e is None or d > 25:
                return
            if _attr(e, "AXRole") in ("AXStaticText", "AXTextField"):
                v = _attr(e, "AXValue")
                if v not in (None, ""):
                    texts.append(str(v).replace("\u200e", "").replace("\u200f", ""))
            for c in _attr(e, "AXChildren") or []:
                walk(c, d + 1)
        walk(self._calc_window(), 0)
        return " | ".join(texts) if texts else None

    def _calc_button(self, label: str) -> Any:
        win = self._calc_window()
        return _find(win, lambda e: _attr(e, "AXRole") == "AXButton" and
                     (str(_attr(e, "AXDescription") or "").strip().lower() == label.lower() or
                      str(_attr(e, "AXTitle") or "").strip() == label))

    def calc_press(self) -> dict[str, Any]:
        import ApplicationServices as AX
        pressed = []
        for label in ("clear", "7", "multiply", "6", "equals"):
            alts = {"clear": ("all clear", "AC", "C", "Clear"), "multiply": ("×", "*", "times"), "equals": ("=",)}
            b = self._calc_button(label)
            for alt in alts.get(label, ()):
                b = b or self._calc_button(alt)
            if b is None:
                pressed.append(f"{label}: not found")
                continue
            pressed.append(f"{label}: {AX.AXUIElementPerformAction(b, 'AXPress')}")
            time.sleep(0.25)
        time.sleep(0.5)
        return {"pressed": pressed, "display": self._calc_display(), "landed": "42" in (self._calc_display() or "")}

    def calc_type(self) -> dict[str, Any]:
        from ferb.act.mac.input import KEYCODES, key_steps, type_steps
        self._calc_window()
        b = self._calc_button("clear") or self._calc_button("all clear") or self._calc_button("AC")
        if b is not None:
            import ApplicationServices as AX
            AX.AXUIElementPerformAction(b, "AXPress")
            time.sleep(0.3)
        _run_steps(type_steps("12+3", self.calc_pid), pause=0.08)
        _run_steps(key_steps("return", self.calc_pid))
        time.sleep(0.6)
        shown = self._calc_display() or ""
        if "15" not in shown:                                   # Return ignored? try "=" as a typed character
            _run_steps(type_steps("=", self.calc_pid), pause=0.08)
            time.sleep(0.6)
            shown = self._calc_display() or ""
        return {"display": shown, "landed": "15" in shown}

    def cleanup(self) -> dict[str, Any]:
        """Close the documents this test created: the window's close button, then the sheet's Delete / Don't Save."""
        import ApplicationServices as AX
        closed = []
        for win in self.created:
            btn = _attr(win, "AXCloseButton")
            if btn is None:
                closed.append("no close button")
                continue
            AX.AXUIElementPerformAction(btn, "AXPress")
            time.sleep(0.8)
            sheet = _find(win, lambda e: _attr(e, "AXRole") == "AXSheet")
            if sheet is not None:
                b = _find(sheet, lambda e: _attr(e, "AXRole") == "AXButton" and
                          str(_attr(e, "AXTitle") or "") in ("Delete", "Don't Save"))
                closed.append("closed, sheet: " + ("discarded" if b is not None and
                                                      AX.AXUIElementPerformAction(b, "AXPress") == 0 else "left open"))
            else:
                closed.append("closed")
        return {"documents": closed, "note": "apps were left running; nothing saved"}


def mac_steps(t: MacTest) -> list[Step]:
    return [
        Step("M1", "Open TextEdit in the background (open -g)", t.open_textedit),
        Step("M2", "TextEdit: new document through its menu bar (AXPress on File > New)", t.new_document),
        Step("M3", "TextEdit: set the document's text through Accessibility (AXValue)", t.set_value),
        Step("M4", "TextEdit: type with keystrokes posted to its process (CGEventPostToPid)", t.type_pid),
        Step("M5", "TextEdit: insert text at the caret (AXSelectedText)", t.selected_text),
        Step("M6", "TextEdit: select all — shortcut to its process if its menu item is enabled, else Accessibility", t.key_combo),
        Step("M7", "TextEdit: scroll with Page Down keys posted to its process", t.scroll_keys),
        Step("M8", "TextEdit: scroll-wheel events posted to its process", t.scroll_wheel_pid, experimental=True),
        Step("M9", "TextEdit: mouse click posted to its process (caret should move)", t.click_pid, experimental=True),
        Step("M10", "Open Calculator in the background", t.open_calc),
        Step("M11", "Calculator: press 7 × 6 = through Accessibility (AXPress)", t.calc_press),
        Step("M12", "Calculator: type 12+3 and Return as keystrokes to its process", t.calc_type),
        Step("M14", "TextEdit: one window-targeted wheel tick (owner verifies scrolling and focus)",
             t.scroll_window_targeted, experimental=True),
        Step("M13", "Close the TextEdit documents this test made (nothing saved)", t.cleanup),
    ]


# ---------- browser steps (throwaway Chrome, own user-data-dir, never a real profile) ----------

class _Pipe:
    """Chrome over --remote-debugging-pipe (fds 3/4), as spike 3 and the platform probes did."""

    def __init__(self, args: list[str]) -> None:
        to_r, self.to_w = os.pipe()
        self.from_r, from_w = os.pipe()

        def fds() -> None:
            os.dup2(to_r, 3)
            os.dup2(from_w, 4)
        self.proc = subprocess.Popen(args, preexec_fn=fds, pass_fds=(3, 4), stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL)
        os.close(to_r)
        os.close(from_w)
        self.ids = itertools.count(1)
        self.pending: dict[int, queue.Queue[Any]] = {}
        self.lock = threading.Lock()
        threading.Thread(target=self._reader, daemon=True).start()

    def _reader(self) -> None:
        buf = b""
        try:
            while chunk := os.read(self.from_r, 65536):
                buf += chunk
                while b"\0" in buf:
                    raw, buf = buf.split(b"\0", 1)
                    m = json.loads(raw)
                    if "id" in m:
                        with self.lock:
                            q = self.pending.pop(m["id"], None)
                        if q:
                            q.put(m)
        except OSError:
            pass

    def send(self, method: str, params: dict[str, Any] | None = None, session: str | None = None,
             timeout: float = 20) -> dict[str, Any]:
        mid = next(self.ids)
        q: queue.Queue[Any] = queue.Queue()
        with self.lock:
            self.pending[mid] = q
        msg: dict[str, Any] = {"id": mid, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        os.write(self.to_w, json.dumps(msg).encode() + b"\0")
        r = q.get(timeout=timeout)
        if "error" in r:
            raise RuntimeError(f"{method}: {r['error']}")
        return dict(r.get("result", {}))


class BrowserTest:
    def __init__(self, home: Path) -> None:
        self.udd = Path(tempfile.mkdtemp(prefix="watch-chrome-", dir=str(home)))
        self.pipe: _Pipe | None = None
        self.sw: str | None = None
        self.win: int | None = None
        self.tab: int | None = None
        self.srv: socketserver.TCPServer | None = None
        self.port = 0

    def _serve(self) -> str:
        class Quiet(http.server.SimpleHTTPRequestHandler):
            def log_message(self, *a: Any, **k: Any) -> None:
                return None
        handler = functools.partial(Quiet, directory=str(HERE))
        self.srv = socketserver.TCPServer(("127.0.0.1", 0), handler)
        self.port = self.srv.server_address[1]
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        return f"http://127.0.0.1:{self.port}/{PAGE_FILE.name}"

    def x(self, expr: str) -> Any:
        assert self.pipe is not None
        r = self.pipe.send("Runtime.evaluate", {"expression": expr, "returnByValue": True, "awaitPromise": True},
                           session=self.sw)
        if "exceptionDetails" in r:
            raise RuntimeError(json.dumps(r["exceptionDetails"])[:300])
        return r.get("result", {}).get("value")

    def state(self) -> Any:
        return self.x(f"chrome.scripting.executeScript({{target:{{tabId:{self.tab}}}, world:'MAIN', "
                      f"func:()=>__state()}}).then(r=>r[0].result)")

    def cdp(self, method: str, params: dict[str, Any] | None = None) -> Any:
        return self.x(f"chrome.debugger.sendCommand({{tabId:{self.tab}}}, {json.dumps(method)}, "
                      f"{json.dumps(params or {})})")

    def launch(self) -> dict[str, Any]:
        if not Path(CHROME).exists():
            raise RuntimeError("Google Chrome isn't installed at /Applications; browser steps need it")
        self.page = self._serve()
        self.pipe = _Pipe([CHROME, f"--user-data-dir={self.udd}", "--remote-debugging-pipe",
                           "--enable-unsafe-extension-debugging", "--no-first-run", "--no-default-browser-check",
                           "--no-startup-window", "--disable-sync", "--disable-background-networking"])
        time.sleep(1.5)
        ext = self.pipe.send("Extensions.loadUnpacked", {"path": str(EXT_DIR)})
        time.sleep(1.0)
        targets = self.pipe.send("Target.getTargets")["targetInfos"]
        sw = [t for t in targets if t["type"] == "service_worker" and ext["id"] in t["url"]]
        if not sw:
            raise RuntimeError("the probe extension's service worker didn't start")
        self.sw = self.pipe.send("Target.attachToTarget", {"targetId": sw[0]["targetId"], "flatten": True})["sessionId"]
        self.pipe.send("Runtime.enable", session=self.sw)
        return {"chrome_pid": self.pipe.proc.pid, "user_data_dir": str(self.udd), "windows": "none (no startup window)"}

    def create_window(self) -> dict[str, Any]:
        w = self.x(f"chrome.windows.create({{url:'about:blank', state:'minimized'}})")
        self.win = w["id"]
        time.sleep(0.8)
        w2 = self.x(f"chrome.windows.get({self.win})")
        return {"window": self.win, "state": w2["state"], "focused": w2["focused"]}

    def open_tab(self) -> dict[str, Any]:
        t = self.x(f"chrome.tabs.create({{windowId:{self.win}, url:'{self.page}', active:false}})")
        self.tab = t["id"]
        for _ in range(40):
            time.sleep(0.25)
            if self.x(f"chrome.tabs.get({self.tab})")["status"] == "complete":
                break
        w = self.x(f"chrome.windows.get({self.win})")
        return {"tab": self.tab, "loaded": True, "window_state": w["state"], "window_focused": w["focused"]}

    def attach(self) -> dict[str, Any]:
        self.x(f"chrome.debugger.attach({{tabId:{self.tab}}}, '1.3')")
        self.cdp("Emulation.setFocusEmulationEnabled", {"enabled": True})
        self.cdp("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": 2, "y": 2})
        time.sleep(0.4)
        st = self.state()
        return {"attached": True, "visibilityState": st["visibilityState"], "hasFocus": st["hasFocus"]}

    def _center(self, rect: list[float]) -> tuple[float, float]:
        return rect[0] + rect[2] / 2, rect[1] + rect[3] / 2

    def click(self) -> dict[str, Any]:
        st = self.state()
        x, y = self._center(st["btn"])
        for kind in ("mouseMoved", "mousePressed", "mouseReleased"):
            self.cdp("Input.dispatchMouseEvent", {"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1})
        time.sleep(0.4)
        st2 = self.state()
        return {"clicks": st2["clicks"], "trusted_clicks": st2["trusted_clicks"], "landed": st2["trusted_clicks"] > st["trusted_clicks"]}

    def type_text(self) -> dict[str, Any]:
        st = self.state()
        x, y = self._center(st["inp"])
        for kind in ("mouseMoved", "mousePressed", "mouseReleased"):
            self.cdp("Input.dispatchMouseEvent", {"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1})
        time.sleep(0.2)
        self.cdp("Input.insertText", {"text": "hello from ferb"})
        time.sleep(0.3)
        st2 = self.state()
        return {"active": st2["activeElement"], "value": st2["inpValue"], "landed": st2["inpValue"] == "hello from ferb"}

    def press_enter(self) -> dict[str, Any]:
        self.cdp("Input.dispatchKeyEvent", {"type": "keyDown", "key": "Enter", "code": "Enter",
                                            "windowsVirtualKeyCode": 13, "text": "\r"})
        self.cdp("Input.dispatchKeyEvent", {"type": "keyUp", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13})
        time.sleep(0.3)
        taken = self.x(f"chrome.scripting.executeScript({{target:{{tabId:{self.tab}}}, world:'MAIN', "
                       f"func:()=>__take()}}).then(r=>r[0].result)")
        keys = [k for k in taken["keys"] if k[0] in ("keydown", "keyup")]
        return {"key_events": keys[:4], "landed": any(k[2] == "Enter" and k[1] for k in keys)}

    def scroll(self) -> dict[str, Any]:
        before = self.state()["scrollY"]
        self.cdp("Input.dispatchMouseEvent", {"type": "mouseWheel", "x": 300, "y": 300, "deltaX": 0, "deltaY": 600})
        time.sleep(0.5)
        after = self.state()["scrollY"]
        return {"scrollY_before": before, "scrollY_after": after, "landed": after > before}

    def screenshot(self) -> dict[str, Any]:
        import base64
        r = self.cdp("Page.captureScreenshot", {"format": "png"})
        p = self.udd / "screenshot.png"
        p.write_bytes(base64.b64decode(r["data"]))
        return {"saved": str(p), "bytes": p.stat().st_size}

    def navigate(self) -> dict[str, Any]:
        self.x(f"chrome.tabs.update({self.tab}, {{url:'{self.page}#second'}})")
        time.sleep(1.0)
        t = self.x(f"chrome.tabs.get({self.tab})")
        return {"url": t["url"], "window_state": self.x(f"chrome.windows.get({self.win})")["state"]}

    def second_window(self) -> dict[str, Any]:
        w = self.x("chrome.windows.create({url:'about:blank', focused:false})")
        time.sleep(1.0)
        w2 = self.x(f"chrome.windows.get({w['id']})")
        self.x(f"chrome.windows.remove({w['id']})")
        return {"state": w2["state"], "focused": w2["focused"], "note": "a normal (not minimized) window, then removed"}

    def close(self) -> dict[str, Any]:
        try:
            if self.tab is not None:
                self.x(f"chrome.debugger.detach({{tabId:{self.tab}}})")
        except Exception:
            pass
        try:
            assert self.pipe is not None
            self.pipe.send("Browser.close", timeout=5)
        except Exception:
            pass
        if self.pipe is not None:
            try:
                self.pipe.proc.wait(5)
            except subprocess.TimeoutExpired:
                self.pipe.proc.kill()
        if self.srv is not None:
            self.srv.shutdown()
        return {"chrome_closed": True, "kept": str(self.udd) + " (screenshot inside; delete when done)"}


def browser_steps(b: BrowserTest) -> list[Step]:
    return [
        Step("B1", "Launch a throwaway Chrome with no window (its own user-data-dir, probe extension loaded)", b.launch),
        Step("B2", "Create FERB's window minimized (chrome.windows.create state:'minimized')", b.create_window),
        Step("B3", "Open the test page in a background tab of that window (tabs.create active:false)", b.open_tab),
        Step("B4", "Attach the debugger to that tab with focus emulation (the infobar lives in the minimized window)", b.attach),
        Step("B5", "Click a button in the page with trusted input (Input.dispatchMouseEvent)", b.click),
        Step("B6", "Type into a field (trusted click, then Input.insertText)", b.type_text),
        Step("B7", "Press Enter (Input.dispatchKeyEvent)", b.press_enter),
        Step("B8", "Scroll the page (trusted mouseWheel)", b.scroll),
        Step("B9", "Screenshot the background tab (Page.captureScreenshot)", b.screenshot),
        Step("B10", "Navigate the tab to another URL", b.navigate),
        Step("B11", "Create a second, NON-minimized window with focused:false, then remove it", b.second_window,
             experimental=True),
        Step("B12", "Close the throwaway Chrome", b.close),
    ]


# ---------- entry ----------

def run(home: Path, out: TextIO, ask: Callable[[str], str] = input, parts: tuple[str, ...] = ("mac", "browser"),
        only: str | None = None, dry_run: bool = False) -> int:
    wdir = home / "watch"
    wdir.mkdir(parents=True, exist_ok=True)
    mac, browser = MacTest(), BrowserTest(wdir)
    steps = (mac_steps(mac) if "mac" in parts else []) + (browser_steps(browser) if "browser" in parts else [])
    if only:
        wanted = {s.strip().upper() for s in only.split(",")}
        keep = [s for s in steps if s.id in wanted]
        # setup / teardown steps every selection needs
        need = {"M": ["M1", "M2", "M13"], "B": ["B1", "B2", "B3", "B4", "B12"]}
        for s in steps:
            if s.id[0] in need and s.id in need[s.id[0]] and any(k.id[0] == s.id[0] for k in keep) and s not in keep:
                keep.append(s)
        steps = [s for s in steps if s in keep]  # preserve setup, action, cleanup order
    print("ferb watch — the owner-verified background test", file=out)
    print(f"{len(steps)} steps. Keep doing what you were doing in another app; after each step, say what you saw.",
          file=out)
    if dry_run:
        for s in steps:
            print(f"  {s.id:<4} {s.title}" + ("   [experimental]" if s.experimental else ""), file=out)
        return 0
    stamp = datetime.now().strftime("%Y-%m-%d-%H%M%S")
    det = Detector(wdir / f"{stamp}.events.jsonl")
    time.sleep(0.8)
    owner_app = _front()
    print(f"Your app now: {owner_app}. Results go to {wdir / (stamp + '.json')}", file=out)
    sess = Session(out, ask, det, owner_app)
    interrupted = False
    try:
        for s in steps:
            sess.do(s)
    except KeyboardInterrupt:
        interrupted = True
        print("\nstopped by you; verification is incomplete", file=out)
    finally:
        det.stop()
        if "browser" in parts and browser.pipe is not None and browser.pipe.proc.poll() is None:
            try:
                browser.close()
            except Exception:
                pass
        report = {"at": stamp, "owner_app": owner_app, "macos": subprocess.run(["sw_vers", "-productVersion"],
                  capture_output=True, text=True).stdout.strip(), "steps": sess.results, "interrupted": interrupted}
        (wdir / f"{stamp}.json").write_text(json.dumps(report, indent=1, default=str))
    print("\nSummary (you | detector | FERB's own check):", file=out)
    bad = 0
    for r in sess.results:
        d = r["detector"]
        det_s = "quiet" if not d["notifications"] and not d["frontmost_changes"] else "MOVED"
        res = r.get("result") or {}
        own = "ok" if not r.get("error") and res.get("landed", True) else "did nothing" if not r.get("error") else "error"
        flag = "" if r["owner_saw"] == "nothing" else "  <-- you saw something"
        bad += (r["owner_saw"] != "nothing" or
                (r["id"] == "M14" and (r.get("owner_action_worked") != "y" or
                                       r.get("owner_typing_undisturbed") != "y")))
        print(f"  {r['id']:<4} {r['owner_saw']:<18} {det_s:<6} {own:<12} {r['title'][:60]}{flag}", file=out)
    print(f"\n{bad} step(s) disturbed you or lack owner acceptance. Full record: {wdir / (stamp + '.json')}", file=out)
    return 130 if interrupted else (0 if bad == 0 else 1)


def main(argv: list[str] | None = None) -> int:
    import argparse
    from ferb.paths import ferb_home
    p = argparse.ArgumentParser(prog="ferb watch")
    p.add_argument("--mac", action="store_true", help="only the Mac steps")
    p.add_argument("--browser", action="store_true", help="only the browser steps")
    p.add_argument("--only", help="comma-separated step ids, e.g. M4,M9 (setup / teardown steps are added)")
    p.add_argument("--dry-run", action="store_true", help="list the steps and exit")
    a = p.parse_args(argv)
    parts = tuple(x for x, on in (("mac", a.mac), ("browser", a.browser)) if on) or ("mac", "browser")
    return run(ferb_home(), sys.stdout, parts=parts, only=a.only, dry_run=a.dry_run)
