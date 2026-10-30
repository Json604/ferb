"""Rework phase 3, real Mac (pytest -m real): the driver acts on TextEdit and Calculator in the background — a document
it creates, typed into, edited in place, scrolled by keys; Calculator buttons pressed — while a thread polls the
owner's frontmost app through Accessibility every 50 ms, and the frontmost app never changes (what `ferb watch`
proved with the owner watching, 2026-10-02). Apps are launched with `open -g`, never activated, never quit; the
documents are closed through the window's close button and the save sheet's Delete, both found in a snapshot.

Needs Accessibility for the interpreter. Skips when the owner is in TextEdit or Calculator.
"""

import asyncio
import subprocess
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.act.gui_lock import GuiLock
from ferb.act.mac import accessibility as ax
from ferb.act.mac import apps as appsm
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.input import RealInput
from ferb.act.mac.profile import Profiles
from ferb.act.mac.window_input import load_route
from ferb.core.types import ComputerAction

pytestmark = pytest.mark.real

TEXTEDIT, CALC = "com.apple.TextEdit", "com.apple.calculator"


def _front() -> str | None:
    """Accessibility's focused application first; NSWorkspace when that read fails (-25204 while the owner is in a
    full-screen app, seen in every probe run)."""
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


class _FrontWatch:
    """The owner's frontmost app, polled every 50 ms on a thread; every change is recorded (None readings — the
    intermittent -25204 while the owner is in a full-screen app — don't count)."""

    def __init__(self, owner: str) -> None:
        self.owner, self.changes, self._stop = owner, [], threading.Event()
        self.thread = threading.Thread(target=self._poll, daemon=True)
        self.thread.start()

    def _poll(self) -> None:
        last = self.owner
        while not self._stop.is_set():
            f = _front()
            if f is not None and f != last:
                self.changes.append((round(time.time(), 2), f))
                last = f
            time.sleep(0.05)

    def stop(self) -> list[tuple[float, str]]:
        self._stop.set()
        self.thread.join(2)
        return self.changes


def _instances(bundle: str) -> list[RunningApp]:
    """Running instances, newest first (the probe left a TextEdit wedged on a save sheet; later runs left clean,
    document-less ones: the newest is tried first, the wedged one last)."""
    from AppKit import NSRunningApplication
    out = [RunningApp(bundle, str(a.localizedName() or ""), int(a.processIdentifier()), bool(a.isActive()))
           for a in NSRunningApplication.runningApplicationsWithBundleIdentifier_(bundle) if not a.isTerminated()]
    return sorted(out, key=lambda a: -a.pid)


def _launch(bundle: str, new_instance: bool) -> RunningApp:
    """`open -g` (never activates); `-n` for another instance of an app whose running one can't be used."""
    before = {a.pid for a in _instances(bundle)}
    subprocess.run(["open", "-g"] + (["-n"] if new_instance else []) + ["-b", bundle], check=True, capture_output=True)
    for _ in range(60):
        time.sleep(0.25)
        new = [a for a in _instances(bundle) if a.pid not in before]
        if new:
            time.sleep(2.0)
            return new[0]
    raise AssertionError(f"{bundle} did not start")


def _new_document(d: MacComputerDriver, te: RunningApp) -> str | None:
    """File > New through the driver (AXPress on the menu item); the new window's title once Accessibility reaches
    it, None if this instance gives no window (wedged)."""
    seen = {w["id"] for w in ax.cg_windows(te.pid)}
    if not asyncio.run(d.press_menu("TextEdit", ["File", "New"])):
        return None

    def reached() -> str | None:
        w = ax.app_window(te.pid)
        t = ax.window_title(w) if w is not None else ""
        if t.startswith("Untitled") and any(c["title"] == t and c["id"] not in seen for c in ax.cg_windows(te.pid)):
            return t
        return None
    _wait(lambda: reached() is not None, 4.0)
    return reached()


class _Apps:
    """The driver's app lookup pinned to the instances this test chose (two TextEdit instances may be running)."""

    def __init__(self, chosen: dict[str, RunningApp]) -> None:
        self.chosen = chosen

    def find(self, q: str) -> RunningApp | None:
        return next((a for a in self.chosen.values() if q.lower() in (a.bundle_id.lower(), a.name.lower())), None)

    def frontmost(self) -> RunningApp | None:
        return appsm.frontmost()

    def activate(self, bundle_id: str) -> bool:                    # the test must never get here
        raise AssertionError(f"the driver tried to activate {bundle_id}")


def _elements(snap: dict[str, Any], role: str | None = None, label: str | None = None,
              where: str | None = None) -> list[dict[str, Any]]:
    out = []
    for e in snap["elements"]:
        if role and e["role"] != role:
            continue
        if label is not None and e["label"].strip().lower() != label.lower():
            continue
        if where is not None and e.get("in") != where:
            continue
        out.append(e)
    return out


def _wait(cond: Any, timeout: float = 6.0, step: float = 0.25) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if cond():
            return True
        time.sleep(step)
    return bool(cond())


def test_textedit_and_calculator_are_driven_in_the_background_and_the_front_app_never_changes(tmp_path: Path) -> None:
    owner = _front()
    if owner is None:
        pytest.skip("can't read the frontmost app (Accessibility)")
    if owner in (TEXTEDIT, CALC):
        pytest.skip("the owner is in the target app")
    calc = next((a for a in _instances(CALC) if ax.app_window(a.pid) is not None), None) or _launch(CALC, False)
    chosen: dict[str, RunningApp] = {CALC: calc}
    d = MacComputerDriver("dev", RealInput(lambda: False, GuiLock()), apps=_Apps(chosen),
                          profiles=Profiles(tmp_path / "apps"))
    watch = _FrontWatch(owner)
    created: list[str] = []
    te: RunningApp | None = None
    try:
        run = asyncio.run
        # --- TextEdit: a new document through the menu bar (the first instance that gives one), addressed by title
        title = None
        for te in _instances(TEXTEDIT) or [_launch(TEXTEDIT, False)]:
            chosen[TEXTEDIT] = te
            title = _new_document(d, te)
            if title is not None:
                break
        if title is None:
            te = chosen[TEXTEDIT] = _launch(TEXTEDIT, True)
            title = _new_document(d, te)
        assert te is not None and title is not None, "no TextEdit instance gave a new document"
        created.append(title)
        new = {"title": title}
        _, snap = run(d.snapshot("TextEdit", window=new["title"]))
        assert snap["window"] == new["title"]
        assert any(w["title"] == new["title"] and w["reachable"] for w in snap["windows"])
        boxes = _elements(snap, role="textbox")
        assert boxes and "menu" in boxes[0].get("actions", []), snap["elements"][:5]     # TextEdit's AXTextArea
        area = boxes[0]["id"]
        # type: unicode keys to the process, read back
        r = run(d.act("TextEdit", ComputerAction("type", area, "hello from ferb ")))
        assert r.ok and "background" in r.detail, r
        # edit in place: replace the first word, append at the end — AXSelectedTextRange + AXSelectedText
        r = run(d.act("TextEdit", ComputerAction("edit", area, "HELLO", select=(0, 5))))
        assert r.ok and "replaced" in r.detail, r
        r = run(d.act("TextEdit", ComputerAction("edit", area, "(edited)", select="end")))
        assert r.ok and "inserted" in r.detail, r
        _, snap = run(d.snapshot("TextEdit", window=new["title"]))
        value = _elements(snap, role="textbox")[0].get("value", "")
        assert value.startswith("HELLO from ferb ") and value.endswith("(edited)"), value
        # Exact native pointer: select part of this throwaway document without moving the owner's cursor.
        import Quartz
        cursor_before = Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))
        _, pointer_shot, _ = run(d.screenshot("TextEdit"))
        node = d.node(TEXTEDIT, _elements(snap, role="textbox")[0]["id"])
        rx, ry, rw, rh = node.rect
        bx, by, bw, bh = pointer_shot.bounds
        kx, ky = pointer_shot.size[0] / bw, pointer_shot.size[1] / bh
        if d.shots.on_screen(pointer_shot.window):
            r = run(d.pointer_at("TextEdit", pointer_shot.id, (rx + 8 - bx) * kx, (ry + 10 - by) * ky,
                                 "drag", ((rx + min(rw - 8, 100) - bx) * kx, (ry + 10 - by) * ky)))
            assert r.ok, r.detail
            selected = str(ax._attr(node.ref, "AXSelectedText") or "")
            assert selected, f"pointer drag did not select text: {r.detail}"
        elif load_route() is None:
            from ferb.core.errors import Refused
            with pytest.raises(Refused, match="current Space"):
                run(d.pointer_at("TextEdit", pointer_shot.id, 10, 10, "hover"))
            print("Legacy pointer correctly refused: the throwaway window is on another Space; selection probe pending.")
        else:
            print("Window-targeted transport available; native gesture acceptance uses owner-supervised watch checks.")
        cursor_after = Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))
        assert (cursor_after.x, cursor_after.y) == (cursor_before.x, cursor_before.y)
        # a disabled shortcut is refused with the alternative named (Select All has no key window in the background)
        found = ax.menu_item_for_shortcut(te.pid, "a", ["cmd"])
        assert found is not None and "Select All" in found[1]
        if not found[2]:
            with pytest.raises(Exception, match="disabled"):
                run(d.act("TextEdit", ComputerAction("key", None, "cmd+a")))
        # scroll by keys: fill the document, Page Down to the process with the text view focused
        area = _elements(snap, role="textbox")[0]["id"]
        r = run(d.act("TextEdit", ComputerAction("set_value", area, "".join(f"line {i}\n" for i in range(400)))))
        assert r.ok, r
        r = run(d.act("TextEdit", ComputerAction("scroll", area, direction="down", amount=2)))
        assert r.ok and "Page Down" in r.detail, r
        # --- Calculator: 7 × 6 = through AXPress on its buttons
        _, csnap = run(d.snapshot("Calculator"))
        buttons = {e["label"].strip().lower(): e["id"] for e in _elements(csnap, role="button")}
        clear = next((buttons[k] for k in ("all clear", "clear", "ac", "c") if k in buttons), None)
        seven, six = buttons.get("7"), buttons.get("6")
        times = next((buttons[k] for k in ("multiply", "×", "*", "times") if k in buttons), None)
        equals = next((buttons[k] for k in ("equals", "=") if k in buttons), None)
        assert all(x for x in (clear, seven, six, times, equals)), sorted(buttons)
        for b in (clear, seven, times, six, equals):
            r = run(d.act("Calculator", ComputerAction("press", b)))
            assert r.ok and r.detail == "pressed", r
            time.sleep(0.2)
        assert _wait(lambda: "42" in run(d.snapshot("Calculator"))[1]["text"].replace("‎", ""), 3.0)
        _, csnap = run(d.snapshot("Calculator"))                      # new ids after the checks' snapshots
        clear = next(e["id"] for e in _elements(csnap, role="button")
                     if e["label"].strip().lower() in ("all clear", "clear", "ac", "c"))
        assert run(d.act("Calculator", ComputerAction("press", clear))).ok
        # the profile learned what landed
        assert d.profiles.status(TEXTEDIT, "pid_keys") == "landed" and d.profiles.status(TEXTEDIT, "ax_selected_text") == "landed"
        assert d.profiles.status(TEXTEDIT, "pid_keycodes") == "landed" and d.profiles.status(CALC, "ax_press") == "landed"
        assert (tmp_path / "apps" / "com.apple.TextEdit.json").exists()
    finally:
        # close the documents this test made: the window's close button, then the sheet's Delete (both from snapshots)
        closed = []
        for title in created:
            try:
                _, snap = asyncio.run(d.snapshot("TextEdit", window=title))
                close = next((e for e in snap["elements"] if e["label"].lower().startswith("close")), None)
                if close is not None:
                    asyncio.run(d.act("TextEdit", ComputerAction("press", close["id"])))
                else:
                    win = ax.window_named(te.pid, title)
                    ax.press(ax._attr(win, "AXCloseButton"))
                delete = None
                for _ in range(24):
                    time.sleep(0.25)
                    _, snap = asyncio.run(d.snapshot("TextEdit", window=title))
                    delete = next((e for e in _elements(snap, role="button", where="sheet")
                                   if e["label"] in ("Delete", "Don't Save")), None)
                    if delete is not None:
                        break
                if delete is not None:
                    asyncio.run(d.act("TextEdit", ComputerAction("press", delete["id"])))
                closed.append((title, _wait(lambda: ax.window_named(te.pid, title) is None)))   # type: ignore[union-attr]
            except Exception as e:                                         # the owner can discard it by hand
                closed.append((title, f"left open: {e}"))
        changes = watch.stop()
    print(f"\nowner app: {owner}; TextEdit pid {te.pid if te else None}; Calculator pid {calc.pid}; "
          f"frontmost changes during the run: {changes}; closed: {closed}")
    assert changes == [], changes
    assert all(ok is True for _, ok in closed), closed
