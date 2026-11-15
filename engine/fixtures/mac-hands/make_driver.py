"""Driver and tool fixtures for the Rust port of the Mac hands (ENG-3): the real `MacComputerDriver` and the Mac tools
driven through scripted scenarios on the Python tests' own fakes (tests/mackit.py: the chat app `Ax`/`Apps`, the note
app `FakeAx`/`FakeApps`, the capture fakes `VisionAx`/`FakeShots`; the gestures patched to record what would have
been posted, as the tests patch them). Nothing reaches the Mac.

    cd <repo> && uv run python rust/fixtures/mac-hands/make_driver.py

Writes driver.json: per scenario, the fake's starting state, each step's call and what it returned or raised, what
the gestures would have posted, the fake's own call log, and the answers the policy module gave (terminal.py,
confirm.browser_page, provenance.typed_destination), so the Rust fakes replay them. Clocks and ids are fixed; the
profiles' timestamps are a fixed string.
"""

import asyncio
import dataclasses
import json
import os
import sys
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
ROOT = Path(tempfile.mkdtemp(prefix="ferbmd-"))
os.environ["FERB_HOME"] = str(ROOT / "home")
os.environ["HOME"] = str(ROOT / "user")
(ROOT / "user").mkdir()
sys.path.insert(0, str(REPO))

import pytest  # noqa: E402

import ferb.act.mac.driver as drv  # noqa: E402
import ferb.act.mac.tools as mtools  # noqa: E402
from ferb.act.gui_lock import GuiLock  # noqa: E402
from ferb.act.mac import profile as profm  # noqa: E402
from ferb.act.mac.accessibility import AxNode, AxTree  # noqa: E402
from ferb.act.mac.apps import RunningApp  # noqa: E402
from ferb.act.mac.input import RealInput, Step  # noqa: E402
from ferb.act.mac.launched import LaunchedApps  # noqa: E402
from ferb.core.types import ActionResult, ComputerAction, Identity, TerminalGrant, TerminalInput, ToolContext  # noqa: E402
from ferb.policy import confirm, provenance, terminal  # noqa: E402
from tests import mackit  # noqa: E402

profm._now = lambda: "2026-01-01T00:00:00+00:00"
OWN_PID = os.getpid()


# ---------- the policy answers, recorded ----------

ORACLE: dict[str, Any] = {}


def key(name: str, *args: Any) -> str:
    return json.dumps([name, *[sorted(a) if isinstance(a, (set, frozenset)) else a for a in args]],
                      default=lambda o: sorted(o) if isinstance(o, (set, frozenset)) else list(o))


def rec(name: str, fn: Any) -> Any:
    def wrapped(*args: Any, **kw: Any) -> Any:
        out = fn(*args, **kw)
        ORACLE[key(name, *args, *kw.values())] = out
        return out
    return wrapped


class _Menus:
    def match(self, text: str) -> Any:
        out = terminal.TERMINAL_MENUS.match(text)
        ORACLE[key("terminal_menu", text)] = out is not None
        return out


class _Unasked:
    def format(self, why: str, app: str) -> str:
        out = terminal.UNASKED.format(why=why, app=app)
        ORACLE[key("unasked", why, app)] = out
        return out


class _Runners:
    def get(self, bundle: str, default: Any = None) -> Any:
        out = terminal.CODE_RUNNERS.get(bundle, default)
        ORACLE[key("code_runner", bundle)] = out
        return out


class _Opens:
    def __contains__(self, bundle: str) -> bool:
        out = bundle in terminal.OPENS_FILES
        ORACLE[key("opens_files", bundle)] = out
        return out


drv.terminals = SimpleNamespace(
    surface=rec("surface", terminal.surface), can_host=rec("can_host", terminal.can_host),
    run_menu=rec("run_menu", lambda path: terminal.run_menu(list(path))), run_control=rec("run_control", terminal.run_control),
    editor_app=rec("editor_app", terminal.editor_app), structural=rec("structural", terminal.structural),
    newline=rec("newline", terminal.newline), TERMINAL_MENUS=_Menus(), UNASKED=_Unasked(), CODE_RUNNERS=_Runners(),
    OPENS_FILES=_Opens())
drv.floors = SimpleNamespace(browser_page=rec("browser_page", confirm.browser_page))
mtools.floors = drv.floors
mtools.typed_destination = rec("typed_destination", provenance.typed_destination)


# ---------- what comes out ----------

def facts_json(f: Any) -> Any:
    return None if f is None else f.recorded()


def result_json(r: Any) -> Any:
    if isinstance(r, ActionResult):
        return {"ok": r.ok, "detail": r.detail, "facts": r.facts, "observed": facts_json(r.observed)}
    if hasattr(r, "content") and hasattr(r, "is_error"):
        return {"content": r.content, "is_error": r.is_error, "read_sources": [list(s) for s in r.read_sources],
                "facts": facts_json(r.facts), "image": r.image is not None}
    if isinstance(r, TerminalInput):
        return {k: (list(v) if isinstance(v, tuple) else v) for k, v in dataclasses.asdict(r).items()}
    return r


def run_step(fn: Any) -> dict[str, Any]:
    try:
        out = fn()
        if asyncio.iscoroutine(out):
            out = asyncio.run(out)
        return {"ok": result_json(out)}
    except Exception as e:
        return {"error": type(e).__name__, "message": str(e)}


def jsonable(v: Any) -> Any:
    return json.loads(json.dumps(v, default=lambda o: list(o) if isinstance(o, (tuple, set, frozenset)) else str(o)))


# ---------- the fakes, as the tests build them ----------

def chat(mp: Any, ax_kind: str = "chat", owner: list[bool] | None = None, **cfg: Any) -> tuple[Any, Any, Any, list[str]]:
    ax = (mackit.Ax if ax_kind == "chat" else FactsAx)(**cfg)
    d, apps, posted = mackit.background_mac(mp, ax, owner=owner)
    d.profiles = profm.Profiles(None)
    return d, ax, apps, posted


class FactsAx(mackit.Ax):
    """tests/test_mac_hands.py FactsAx: roles, subroles, the focus and its window."""
    focus_el: Any = "box"

    def role_of(self, el: Any) -> str:
        return {"box": "AXTextField", "send": "AXButton", "pw": "AXSecureTextField"}.get(el, "")

    def subrole_of(self, el: Any) -> str:
        return "AXSecureTextField" if el == "pw" else ""

    def secure_element(self, el: Any) -> bool:
        return el == "pw"

    def focused_element(self, pid: int) -> Any:
        return self.focus_el

    def place_of(self, el: Any) -> tuple[str, str]:
        return ("Chat with Asha", str(el))


class TermAx(FactsAx):
    """FactsAx whose elements carry the live trees' facts (tests/support/terminal_ax.py's chains): the message box is
    VS Code's xterm input, the send button an editor's Run control; ⌘V's item is Edit > Paste."""
    chains: dict[str, Any] = {}

    def surface_facts(self, el: Any) -> list[dict[str, Any]]:
        return [dict(f) for f in self.chains.get(el, [])]

    def menu_item_shortcut(self, pid: int, path: list[str]) -> Any:
        return ("V", 0) if list(path) == ["Edit", "Paste"] else None

    def item_shortcut(self, el: Any) -> Any:
        return None


def ctx(scopes: set[str], terminal: Any = None) -> ToolContext:
    return ToolContext("t1.worker", "w1", "r1", Identity.FERB, frozenset(scopes), "dev", None, str(ROOT / "ws"), (), (),
                       terminal=terminal)


SCENARIOS: list[dict[str, Any]] = []


def scenario(name: str, fake: dict[str, Any], steps: list[tuple[str, Any, Any]], d: Any, ax: Any, posted: list[str]) -> None:
    out = []
    for op, args, fn in steps:
        calls_before = len(getattr(ax, "calls", []))
        posted_before = len(posted)
        got = run_step(fn)
        out.append({"op": op, "args": jsonable(args), "out": jsonable(got), "posted": posted[posted_before:],
                    "calls": list(getattr(ax, "calls", []))[calls_before:]})
    SCENARIOS.append({"name": name, "fake": fake, "steps": out})


def CA(kind: str, element: Any = None, text: Any = None, **kw: Any) -> ComputerAction:
    return ComputerAction(kind, element, text, **kw)


def ca_json(a: ComputerAction) -> dict[str, Any]:
    return {k: v for k, v in dataclasses.asdict(a).items()}


def build_chat(mp: Any) -> None:
    def act(d: Any, a: ComputerAction, **kw: Any) -> tuple[str, Any, Any]:
        return ("act", {"app": "WhatsApp", "action": ca_json(a), **kw}, lambda: d.act("WhatsApp", a, holder="w1", task="t1.worker", **kw))

    def ids(d: Any) -> dict[str, str]:
        _, out = asyncio.run(d.snapshot("WhatsApp"))
        return {e["label"]: e["id"] for e in out["elements"]}

    # 1. the chat app, background typing that lands
    d, ax, apps, posted = chat(mp)
    steps = [("snapshot", {"app": "WhatsApp"}, lambda: d.snapshot("WhatsApp")[1] if False else asyncio.run(d.snapshot("WhatsApp"))[1])]
    e = {"Message": "s1e1", "Send": "s1e2"}
    steps += [act(d, CA("type", e["Message"], "hi there")), act(d, CA("press", e["Send"])),
              act(d, CA("set_value", e["Message"], "x")), act(d, CA("key", None, "return")),
              act(d, CA("key", None, "cmd+tab")), act(d, CA("key", None, "cmd+q")), act(d, CA("key", None, "opt+cmd+h")),
              act(d, CA("key", None, "nope+x")), act(d, CA("key", None, None)),
              act(d, CA("focus", e["Message"], mechanism="pid")), act(d, CA("select", e["Message"], "a", mechanism="pid")),
              act(d, CA("press", "s0e1")), act(d, CA("press", None)),
              act(d, CA("key", None, "cmd+s", mechanism="ax")),
              act(d, CA("type", e["Message"], "fg", mechanism="foreground")),
              act(d, CA("cancel", e["Send"])), act(d, CA("edit", e["Message"], "Z", select="end")),
              act(d, CA("edit", e["Message"], "Q", select=[0, 2])), act(d, CA("edit", e["Message"], None, select="all")),
              act(d, CA("edit", e["Message"], "Q", select="bogus")),
              ("element_identity", {"element": e["Send"]}, lambda: d.element_identity(mackit.CHAT, e["Send"])),
              ("element_identity", {"element": "s9e1"}, lambda: d.element_identity(mackit.CHAT, "s9e1")),
              ("snapshot", {"app": "WhatsApp"}, lambda: asyncio.run(d.snapshot("WhatsApp"))[1]),
              act(d, CA("type", "s1e1", "late")),
              ("app", {"q": "Nope"}, lambda: d.app("Nope")),
              ("app", {"q": "pid:23"}, lambda: d.app("pid:23"))]
    scenario("chat_background", {"kind": "chat", "cfg": {}}, steps, d, ax, posted)

    # 2. an app that takes no background keystrokes; a press it ignores; a sheet holding the focus
    d, ax, apps, posted = chat(mp, takes_background=False, press_ok=False)
    steps = [("snapshot", {"app": "WhatsApp"}, lambda: asyncio.run(d.snapshot("WhatsApp"))[1]),
             act(d, CA("type", "s1e1", "hello")), act(d, CA("press", "s1e2"))]
    scenario("chat_deaf", {"kind": "chat", "cfg": {"takes_background": False, "press_ok": False}}, steps, d, ax, posted)
    d, ax, apps, posted = chat(mp, box_has_focus=False, send_button=False)
    steps = [("snapshot", {"app": "WhatsApp"}, lambda: asyncio.run(d.snapshot("WhatsApp"))[1]),
             act(d, CA("type", "s1e1", "hello")), act(d, CA("type", "s1e1", "hello", mechanism="foreground",
                                                            allow_foreground=True))]
    scenario("chat_sheet", {"kind": "chat", "cfg": {"box_has_focus": False, "send_button": False}}, steps, d, ax, posted)

    # 3. foreground typing and keys, the owner busy then quiet; and the owner taking over mid-way
    d, ax, apps, posted = chat(mp, owner=[True, True, False])
    steps = [("snapshot", {"app": "WhatsApp"}, lambda: asyncio.run(d.snapshot("WhatsApp"))[1]),
             act(d, CA("key", None, "cmd+s", mechanism="foreground", allow_foreground=True)),
             act(d, CA("type", "s1e1", "abc", mechanism="foreground", allow_foreground=True))]
    scenario("chat_foreground", {"kind": "chat", "cfg": {}, "owner": [True, True, False]}, steps, d, ax, posted)

    # 4. structural facts, a password field, keys into it
    d, ax, apps, posted = chat(mp, ax_kind="facts", focused="textbox")
    steps = [("snapshot", {"app": "WhatsApp"}, lambda: asyncio.run(d.snapshot("WhatsApp"))[1]),
             act(d, CA("press", "s1e2")), act(d, CA("type", "s1e1", "yo")), act(d, CA("key", None, "a")),
             ("set_focus", "pw", lambda: setattr(ax, "focus_el", "pw")),
             act(d, CA("key", None, "a")), act(d, CA("key", None, "cmd+v")), act(d, CA("key", None, "return")),
             act(d, CA("type", None, "typed", mechanism="foreground", allow_foreground=True)),
             ("set_focus", "box", lambda: setattr(ax, "focus_el", "box")),
             act(d, CA("type", None, "typed", mechanism="foreground", allow_foreground=True)),
             ("terminal_input", {"action": ca_json(CA("key", None, "return"))},
              lambda: d.terminal_input("WhatsApp", CA("key", None, "return"), "t1.worker")),
             ("terminal_input", {"action": ca_json(CA("type", "s1e1", "ls\n"))},
              lambda: d.terminal_input("WhatsApp", CA("type", "s1e1", "ls\n"), "t1.worker")),
             ("browser_page", {"action": ca_json(CA("type", "s1e1", "chrome://settings"))},
              lambda: d.browser_page("WhatsApp", CA("type", "s1e1", "chrome://settings")))]
    scenario("chat_facts", {"kind": "facts", "cfg": {"focused": "textbox"}}, steps, d, ax, posted)


def build_vision(mp: Any) -> None:
    d, ax, shots, apps, posted = mackit.vision(mp)
    d.profiles = profm.Profiles(None)
    shots.jpeg = b"\xff\xd8\xffJPEG"
    painted: list[Any] = []

    async def fake_paint(jpeg: bytes, masks: list[Any], bounds: Any, size: Any) -> bytes:
        from ferb.act.evidence.mask import to_image_boxes
        boxes = to_image_boxes(masks, bounds, size)
        painted.append([list(b) for b in boxes])
        return jpeg if not boxes else b"painted"
    mp.setattr(drv, "painted", fake_paint)
    # never a real event: the wheel is recorded as the tests' other gestures are
    mp.setattr(drv, "wheel_steps", lambda x, y, lines, pid=None, **kw: [
        Step("scroll", lambda: posted.append(f"scroll {lines} at {x},{y}{' to pid' if pid else ''}"))])
    real_mono = time.monotonic
    steps: list[tuple[str, Any, Any]] = []

    def shot(window: Any = None) -> Any:
        a, s, jpeg = asyncio.run(d.screenshot("Notes", window=window))
        return {"app": a.name, "shot": s.id, "window": s.window, "bounds": list(s.bounds), "size": list(s.size),
                "pid": s.pid, "masking": s.masking, "masks": [list(m) for m in s.masks], "jpeg": jpeg.decode("latin-1"),
                "painted": painted[-1] if painted else None, "captured": list(shots.captured)}

    def click(mech: str, x: float, y: float, allow: bool = False, sid: str | None = None) -> tuple[str, Any, Any]:
        return ("click_at", {"shot": sid, "x": x, "y": y, "mechanism": mech, "allow_foreground": allow},
                lambda: d.click_at("Notes", sid or d._state["com.apple.Notes"].shot.id, x, y, holder="w1",
                                   task="t1.worker", allow_foreground=allow, mechanism=mech))

    ax.tree.secret_rects = [(120.0, 160.0, 200.0, 24.0)]
    steps += [("screenshot", {}, shot),
              ("click_identity", {"x": 50, "y": 60}, lambda: d.click_identity("com.apple.Notes", "shot1", 50.0, 60.0)),
              ("shot_point", {"x": 50, "y": 60}, lambda: d.shot_point("Notes", "shot1", 50.0, 60.0)),
              click("pointer", 50.0, 60.0),
              ("screenshot", {}, shot), click("pointer", 5000.0, 60.0), click("ax", 50.0, 60.0),
              click("foreground", 50.0, 60.0), click("foreground", 50.0, 60.0, True), click("bogus", 50.0, 60.0),
              ("screenshot", {}, shot), click("pointer", 50.0, 60.0, sid="shot1"),
              ("pointer", {"kind": "hover"}, lambda: d.pointer_at("Notes", d._state["com.apple.Notes"].shot.id,
                                                                    10.0, 10.0, "hover", None, "w1", task="t1.worker")),
              ("screenshot", {}, shot),
              ("pointer", {"kind": "drag"}, lambda: d.pointer_at("Notes", d._state["com.apple.Notes"].shot.id,
                                                                   10.0, 10.0, "drag", (100.0, 120.0), "w1")),
              ("screenshot", {}, shot),
              ("pointer", {"kind": "scroll"}, lambda: d.pointer_at("Notes", d._state["com.apple.Notes"].shot.id,
                                                                     10.0, 10.0, "scroll", None, "w1",
                                                                     direction="down", amount=3, distance=0.5)),
              ("pointer", {"kind": "zoom"}, lambda: d.pointer_at("Notes", "shot9", 1.0, 1.0, "zoom")),
              ("screenshot", {"secure": [None, False]}, lambda: (ax.secure.extend([None, False]), shot())[1]),
              ("screenshot", {"browser": True}, lambda: asyncio.run(d.screenshot("Brave"))[1].masking),
              ("app", {"q": "Passwords"}, lambda: d.app("Passwords").name),
              ("own_point", {"x": 10, "y": 10}, lambda: d._own_point(10.0, 10.0, 11, None))]
    scenario("vision", {"kind": "vision", "cfg": {"secret_rects": [[120.0, 160.0, 200.0, 24.0]]}}, steps, d, ax, posted)
    del real_mono


def build_tools(mp: Any) -> None:
    d, ax, apps, posted = chat(mp, ax_kind="facts", focused="textbox")
    painted: list[Any] = []
    act = mtools.MacAct(d)
    snap = mtools.MacSnapshot(d)
    c = ctx({"mac:act", "mac:read"})

    def tool(t: Any, args: dict[str, Any]) -> tuple[str, Any, Any]:
        return (f"run {t.name}", args, lambda: t.run(t.Input(**args), c))

    def describe(t: Any, args: dict[str, Any]) -> tuple[str, Any, Any]:
        def go() -> Any:
            s = t.describe(t.Input(**args))
            return {"kind": s.kind, "site": s.target.site, "recipients": list(s.target.recipients), "path": s.target.path,
                    "contact": s.target.contact, "content": s.content, "declared": s.declared, "place": s.place}
        return (f"describe {t.name}", args, go)
    menu, click, pointer = mtools.MacMenu(d), mtools.MacClick(d), mtools.MacPointer(d)
    script, secret, apps_t = mtools.MacAppleScript(d), mtools.MacSecret(d), mtools.MacApps(d)
    steps = [tool(snap, {"app": "WhatsApp"}),
             tool(act, {"app": "WhatsApp", "action": "press", "element": "s1e2"}),
             tool(act, {"app": "WhatsApp", "action": "type", "element": "s1e1", "text": "hey"}),
             tool(act, {"app": "WhatsApp", "action": "press", "element": "s0e2"}),
             tool(apps_t, {}),
             describe(act, {"app": "WhatsApp", "action": "type", "element": "s1e1", "text": " <a@example.com>, "}),
             describe(act, {"app": "WhatsApp", "action": "focus", "element": "s1e1"}),
             describe(act, {"app": "WhatsApp", "action": "type", "element": "s1e1", "text": "+91 98765 43210;"}),
             describe(click, {"app": "WhatsApp", "shot": "shot1", "x": 1, "y": 2, "purpose": "call",
                              "contact": {"name": "Asha", "selected": "Asha K", "detail": "+1 555"}}),
             describe(pointer, {"app": "WhatsApp", "shot": "shot1", "x": 1, "y": 2, "action": "hover", "purpose": "send"}),
             describe(pointer, {"app": "WhatsApp", "shot": "shot1", "x": 1, "y": 2, "action": "scroll", "amount": 3,
                                "distance": 0.25, "direction": "up"}),
             describe(menu, {"app": "WhatsApp", "path": "File>Search"}), describe(menu, {"app": "WhatsApp"}),
             describe(script, {"app": "WhatsApp", "script": "get x"}),
             ("precheck", {"tool": "mac.applescript"}, lambda: script.precheck(script.Input(app="com.apple.systemevents",
                                                                                          script='tell process "FERB"'))),
             ("precheck", {"tool": "mac.applescript", "shell": True},
              lambda: script.precheck(script.Input(app="WhatsApp", script='do shell script "x"'))),
             ("precheck", {"tool": "mac.secret"}, lambda: secret.precheck(secret.Input(app="Arc", element="s1e1"))),
             ("precheck", {"tool": "mac.pointer"}, lambda: pointer.precheck(pointer.Input(app="WhatsApp", shot="s",
                                                                                        x=1, y=1, action="drag"))),
             ("locks", {"tool": "mac.act"}, lambda: act.locks(act.Input(app="WhatsApp", action="press"))),
             ("ledger_args", {"tool": "mac.act"},
              lambda: act.ledger_args(act.Input(app="WhatsApp", action="press", element="s1e2"),
                                      {"app": "net.whatsapp.WhatsApp", "action": "press", "element": "s1e2"})),
             ("menu_path", {"path": " File >Search > "}, lambda: menu.Input(app="x", path=" File >Search > ").path),
             ("menu_path", {"path": " > "}, lambda: menu.Input(app="x", path=" > ").path),
             ("preview_target", {"tool": "mac.act", "action": "key"},
              lambda: (lambda p: [p.scope, p.label, p.resolve is not None])(
                  act.preview_target(act.Input(app="WhatsApp", action="key", element="s1e1", text="a"), "o"))),
             ("preview_target", {"tool": "mac.act", "action": "press"},
              lambda: (lambda p: [p.scope, p.label, p.resolve is not None])(
                  act.preview_target(act.Input(app="WhatsApp", action="press", element="s1e1"), "o"))),
             ("preview_target", {"tool": "mac.snapshot", "observed": True},
              lambda: act.preview_target(snap.Input(app="WhatsApp"), "o", {"windows": [{"id": 1}]}))]
    scenario("tools_chat", {"kind": "facts", "cfg": {"focused": "textbox"}}, steps, d, ax, posted)
    del painted


def build_terminal(mp: Any) -> None:
    from tests.support import terminal_ax as T
    chains = {"box": [dict(f, dom_classes=list(f["dom_classes"])) if "dom_classes" in f else dict(f) for f in T.TERMINAL],
              "send": [{"role": "AXButton", "title": "Run Python File"}] + [
                  dict(f, dom_classes=list(f["dom_classes"])) if "dom_classes" in f else dict(f) for f in T.WORKBENCH]}
    ax = TermAx(focused="textbox")
    ax.chains = chains
    d, apps, posted = mackit.background_mac(mp, ax)
    d.profiles = profm.Profiles(None)
    d.clipboard = lambda: (7, "codex", False)

    def act(a: ComputerAction, grant: Any = None, app: str = "Code") -> tuple[str, Any, Any]:
        g = None if grant is None else {"input": result_json(grant.input), "confirmation_id": grant.confirmation_id}
        return ("act", {"app": app, "action": ca_json(a), "grant": g},
                lambda: d.act(app, a, holder="w1", task="t1.worker", **({"terminal": grant} if grant else {})))
    typed = TerminalInput("Code", "com.microsoft.VSCode", "x", text="ls -la", enter=True)
    steps = [("snapshot", {"app": "Code"}, lambda: asyncio.run(d.snapshot("Code"))[1])]
    found: list[Any] = []

    def ti(a: ComputerAction) -> tuple[str, Any, Any]:
        def go() -> Any:
            got = d.terminal_input("Code", a, "t1.worker")
            found.append(got)
            return got
        return ("terminal_input", {"app": "Code", "action": ca_json(a)}, go)
    steps += [ti(CA("type", "s1e1", "ls -la")), act(CA("type", "s1e1", "ls -la"))]
    grant = TerminalGrant(TerminalInput("Code", "com.microsoft.VSCode", "", text="ls -la", enter=True), "c1")
    steps += [act(CA("type", "s1e1", "ls -la"), grant), ti(CA("key", None, "return")),
              act(CA("key", None, "return"), TerminalGrant(TerminalInput("Code", "com.microsoft.VSCode", "", keys="return",
                                                                       covered_by="c1"), "c1")),
              ti(CA("key", None, "cmd+v")), act(CA("key", None, "cmd+v")), ti(CA("key", None, "ctrl+c")),
              act(CA("key", None, "ctrl+c")), ti(CA("press", "s1e2")), act(CA("press", "s1e2")),
              ("secret_terminal_input", {"app": "Code", "element": "s1e1"},
               lambda: d.secret_terminal_input("Code", "s1e1", "the server password")),
              ("menu_terminal_input", {"app": "Code", "path": ["Edit", "Paste"]},
               lambda: d.menu_terminal_input("Code", ["Edit", "Paste"])),
              ("press_menu", {"app": "Code", "path": ["Edit", "Paste"]},
               lambda: d.press_menu("Code", ["Edit", "Paste"], task="t1.worker"))]
    scenario("terminal", {"kind": "facts", "cfg": {"focused": "textbox"}, "chains": chains}, steps, d, ax, posted)
    del typed, found


def main() -> None:
    mp = pytest.MonkeyPatch()
    try:
        mp.setattr(drv, "load_route", lambda: None)
        build_chat(mp)
        build_vision(mp)
        build_tools(mp)
        build_terminal(mp)
    finally:
        mp.undo()
    (HERE / "driver.json").write_text(json.dumps({"oracle": ORACLE,
                                                   "scenarios": SCENARIOS}, indent=1, ensure_ascii=False,
                                                  default=str).replace(str(OWN_PID), "<OWN_PID>") + "\n")


if __name__ == "__main__":
    main()
