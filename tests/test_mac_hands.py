"""Step 5b (kb area hands, Mac): one browser list, structural facts, secure fields on key presses, terminal detection
that fails closed, the shortcut guard (HAND-200), the multi-window pointer check, responsiveness, the one
foreground path, honest mechanisms, stable P19 identity for Mac actions, and the capture retry. Offline, on fakes:
nothing here drives the Mac."""

import asyncio
import dataclasses
import threading
import time
from types import SimpleNamespace
from typing import Any

import pytest

import ferb.act.mac.driver as drv
from ferb.act.gui_lock import GuiLock
from ferb.act.mac import accessibility as axm
from ferb.act.mac import browsers
from ferb.act.mac.accessibility import AxNode, AxTree
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.input import RealInput, Step, enters_text, quit_or_hide, screen_wide, shortcut_combo
from ferb.act.mac.launched import Launched, LaunchedApps
from ferb.core.errors import Refused, StaleRef
from ferb.core.types import ActionFacts, ComputerAction
from ferb.policy import confirm, terminal
from tests.mackit import BOUNDS, CHAT, Ax, background_mac, ids, vision
from tests.support.terminal_ax import terminal_mac, vscode_ax

ARC, FIREFOX, EDGE = "company.thebrowser.Browser", "org.mozilla.firefox", "com.microsoft.edgemac"


@pytest.fixture(autouse=True)
def no_native_route(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(drv, "load_route", lambda: None)
    monkeypatch.setattr(browsers, "_handlers", None)          # no Launch Services list carried between tests


# ---------- 1. one browser list ----------

def test_one_list_knows_every_browser_by_bundle_id_name_or_launch_services(monkeypatch: pytest.MonkeyPatch) -> None:
    for bundle in (ARC, FIREFOX, EDGE, "com.vivaldi.Vivaldi", "com.google.Chrome.canary", "com.apple.Safari"):
        assert browsers.known(bundle) and browsers.is_browser(bundle), bundle
    for name in ("Arc", "Firefox", "Microsoft Edge", "Google Chrome Beta", "\u200eSafari", "Brave Browser"):
        assert browsers.is_browser_app(name), name
    assert not browsers.is_browser_app("com.apple.Notes") and not browsers.is_browser("com.apple.mail")
    monkeypatch.setattr(browsers, "_enumerate", lambda: (frozenset({"com.example.newbrowser"}),
                                                         frozenset({"new browser"})))
    assert not browsers.is_browser("com.example.NewBrowser")           # nothing enumerated yet: never enumerates
    assert browsers.is_browser_now("com.example.NewBrowser")           # installed apps that open web links
    assert browsers.is_browser("com.example.NewBrowser") and browsers.is_browser_app("\u200fNew Browser")
    monkeypatch.setattr(browsers, "_running_bundles", lambda pid=None, name=None: [FIREFOX] if pid == 77 else [])
    assert browsers.is_browser_app("pid:77") and not browsers.is_browser_app("pid:78")


def test_invisible_marks_are_stripped_before_names_compare(monkeypatch: pytest.MonkeyPatch) -> None:
    assert browsers.clean("\u200eWhatsApp ") == "whatsapp" == browsers.clean("\u200f\u200bWhatsApp")
    from ferb.act.mac import apps
    chat = RunningApp(CHAT, "\u200eWhatsApp", 21, False)
    monkeypatch.setattr(apps, "running", lambda: [chat])
    assert apps.candidates("WhatsApp") == [chat] == apps.candidates("\u200fwhatsapp")


def test_a_mac_read_of_any_browser_is_outside_web_content() -> None:
    """kb area hands: Mac snapshots/screenshots of Arc, Edge, Firefox were labelled the owner's own app data."""
    from ferb.act.mac.tools import _sources
    for bundle in (ARC, FIREFOX, EDGE, "com.google.Chrome.beta"):
        assert _sources(bundle) == (("web", bundle),), bundle
    assert _sources("com.apple.Notes") == (("app", "com.apple.Notes"),)


# decision: GATE-201
def test_foreground_rule_covers_every_browser_and_one_key_per_app() -> None:
    for app in ("Arc", "Firefox", ARC, "Vivaldi", "Google Chrome Canary", "\u200eOrion"):
        need = confirm.foreground_need("mac.click", {"app": app, "mechanism": "pointer"})
        assert need is not None and need.rule == confirm.FOREGROUND, app
    a = confirm.foreground_need("mac.act", {"app": "\u200eWhatsApp", "mechanism": "foreground",
                                            "allow_foreground": True})
    b = confirm.foreground_need("mac.act", {"app": "WhatsApp", "mechanism": "foreground", "allow_foreground": True})
    assert a is not None and b is not None and a.key == b.key          # one Yes covers the app however it's spelt


def test_browser_pages_include_extension_and_about_pages_but_not_a_blank_one() -> None:
    for page in ("chrome-extension://abc/options.html", "about:config", "about:preferences", "moz-extension://x/",
                 "chrome://settings"):
        assert confirm.browser_page(page), page
    for page in ("about:blank", "about:srcdoc", "https://example.com", None, ""):
        assert not confirm.browser_page(page), page


def test_the_device_scan_lists_every_installed_browser(tmp_path: Any) -> None:
    from ferb.device.scan import scan
    from tests.conftest import make_app
    apps = tmp_path / "Applications"
    make_app(apps, "Firefox", FIREFOX)
    make_app(apps, "Arc", ARC)
    make_app(apps, "Notes", "com.apple.Notes")
    make_app(apps, "Odd", "com.example.odd", CFBundleURLTypes=[{"CFBundleURLSchemes": ["https", "odd"]}])
    found = {b.bundle_id: b for b in scan("dev", [apps]).browsers}
    assert set(found) == {FIREFOX, ARC, "com.example.odd"}
    assert found[ARC].name == "arc" and "ext:arc" in found[ARC].drivers
    assert "ext:firefox" not in found[FIREFOX].drivers and found[FIREFOX].drivers


# ---------- 2. structural facts ----------

class FactsAx(Ax):
    """The chat fake with the facts the driver reads: roles, subroles, the focus and its window."""
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


def _facts_mac(monkeypatch: pytest.MonkeyPatch) -> tuple[Any, FactsAx, list[str]]:
    ax = FactsAx(focused="textbox")
    d, _, posted = background_mac(monkeypatch, ax)
    return d, ax, posted


def test_an_input_reports_its_target_and_what_received_it(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, posted = _facts_mac(monkeypatch)
    e = ids(d)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("press", e["Send"])))
    assert r.observed is not None and r.observed.target_role == "AXButton" and r.observed.secure_field is False
    assert r.observed.received_by is not None and r.observed.received_by.bundle_id == CHAT
    assert r.observed.received_by.window == "Chat with Asha" and r.observed.received_by.role == "AXButton"
    r = asyncio.run(d.act("WhatsApp", ComputerAction("type", e["Message"], "hi")))
    assert r.observed is not None and r.observed.terminal_target is False
    assert r.observed.received_by is not None and r.observed.received_by.role == "AXTextField"   # read after input
    recorded = r.observed.recorded()
    assert recorded["target_role"] == "AXTextField" and recorded["received_by"]["window"] == "Chat with Asha"


def test_mac_act_hands_its_facts_to_the_gateway(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac.tools import MacAct
    d, ax, posted = _facts_mac(monkeypatch)
    e = ids(d)
    cap = MacAct(d)
    ctx = SimpleNamespace(terminal=None, worker_id="w1", task_id="t1")
    out = asyncio.run(cap._run(cap.Input(app="WhatsApp", action="press", element=e["Send"]), ctx))  # type: ignore[arg-type]
    assert isinstance(out.facts, ActionFacts) and out.facts.target_role == "AXButton"


def test_terminal_input_the_owner_approved_is_reported_as_terminal_target(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.core.types import TerminalGrant
    ax = vscode_ax(focus="term108")
    d = terminal_mac(monkeypatch, ax)
    _, out = asyncio.run(d.snapshot("Code"))
    term = next(x["id"] for x in out["elements"] if x["label"].startswith("Terminal 108"))
    inp = d.terminal_input("Code", ComputerAction("type", term, "ls"), "t1")
    r = asyncio.run(d.act("Code", ComputerAction("type", term, "ls"), "w1", "t1", TerminalGrant(inp, "c1")))
    assert r.observed is not None and r.observed.terminal_target is True


def test_pointer_input_reports_the_element_under_the_point(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, apps, posted = vision(monkeypatch)
    ax.role_of = lambda el: "AXButton" if el == "Export" else ""        # type: ignore[attr-defined]
    _, shot, _ = asyncio.run(d.screenshot("Notes"))
    ax.hit_el = "Export"
    r = asyncio.run(d.click_at("Notes", shot.id, 400, 300, mechanism="ax"))
    assert r.observed is not None and r.observed.target_role == "AXButton"
    assert r.observed.received_by is not None and r.observed.received_by.bundle_id == "com.apple.Notes"


# ---------- 3. secure fields refuse key presses that enter text ----------

# decision: HAND-218
def test_text_entering_keys_into_a_password_field_are_refused_on_every_route(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, posted = _facts_mac(monkeypatch)
    ax.focus_el = "pw"
    for combo, mechanism in (("a", "pid"), ("shift+a", "pid"), ("space", "pid"), ("cmd+v", "ax"), ("7", "foreground")):
        with pytest.raises(Refused, match="password field"):
            asyncio.run(d.act("WhatsApp", ComputerAction("key", None, combo, mechanism=mechanism,  # type: ignore[arg-type]
                                                         allow_foreground=True)))
    assert posted == []
    assert asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "return"))).ok    # submitting isn't typing
    assert asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "tab"))).ok
    assert posted == ["key return to pid", "key tab to pid"]


# decision: HAND-218
def test_which_keys_enter_text() -> None:
    assert all(enters_text(k) for k in ("a", "shift+a", "1", "space", "/", "cmd+v", "alt+cmd+shift+v", "keypad5"))
    assert not any(enters_text(k) for k in ("return", "tab", "escape", "left", "delete", "cmd+a", "ctrl+c", "f5",
                                            "keypadenter"))


# decision: HAND-218
def test_a_paste_item_into_a_password_field_is_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, posted = _facts_mac(monkeypatch)
    ax.focus_el = "pw"
    ax.menu_item_shortcut = lambda pid, path: ("V", 0)                  # type: ignore[attr-defined]
    with pytest.raises(Refused, match="password field"):
        asyncio.run(d.press_menu("WhatsApp", ["Edit", "Paste"]))


# ---------- 4. terminal detection fails closed ----------

def test_unreadable_focus_in_an_editor_counts_as_a_terminal() -> None:
    why = terminal.surface("com.microsoft.VSCode", [], unread=True)
    assert why is not None and "couldn't read" in why
    assert terminal.surface("com.microsoft.VSCode", []) is None             # the app alone (a file's opener) is not
    assert terminal.surface("com.apple.TextEdit", [], unread=True) is None  # an app that can't host one: unchanged
    assert terminal.can_host("com.jetbrains.pycharm") and terminal.can_host("dev.zed.Zed")
    assert terminal.can_host("com.apple.Terminal") and not terminal.can_host("com.apple.TextEdit")


def test_keys_and_typing_ask_when_the_editors_focus_cant_be_read(monkeypatch: pytest.MonkeyPatch) -> None:
    ax = vscode_ax(focus=None)                                          # VS Code says nothing has the focus
    d = terminal_mac(monkeypatch, ax)
    inp = d.terminal_input("Code", ComputerAction("key", None, "return"), "t1")
    assert inp is not None and "couldn't read" in inp.surface
    with pytest.raises(Refused, match="couldn't read"):
        asyncio.run(d.act("Code", ComputerAction("key", None, "return"), "w1", "t1"))
    assert ax.landed == []


def test_typing_in_an_editor_stops_when_the_focus_becomes_unreadable(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac.input import PointerInterrupted
    ax = vscode_ax(focus="editor")
    d = terminal_mac(monkeypatch, ax)
    monkeypatch.setattr(drv, "TYPE_CHUNK", 2)
    ax.on_char = lambda a: setattr(a, "focused", None) if len(a.landed[-1][1]) == len("type ab") else None
    _, out = asyncio.run(d.snapshot("Code"))
    editor = next(x["id"] for x in out["elements"] if x["label"] == "Editor content")
    with pytest.raises(PointerInterrupted, match="couldn't read where the keyboard focus is"):
        asyncio.run(d.act("Code", ComputerAction("type", editor, "abcdef"), "w1", "t1"))
    assert ax.landed == [("editor", "type ab")]


# ---------- 5. the shortcut guard (HAND-200) ----------

# decision: HAND-200, HAND-219
def test_screen_wide_combinations() -> None:
    for combo in ("cmd+tab", "cmd+shift+tab", "cmd+space", "ctrl+space", "ctrl+left", "ctrl+up", "cmd+alt+escape",
                  "f3", "f4", "ctrl+cmd+q", "shift+cmd+q"):
        assert screen_wide(combo), combo
    for combo in ("cmd+s", "cmd+a", "tab", "left", "cmd+`", "f5", "cmd+q", "cmd+w"):
        assert not screen_wide(combo), combo
    assert quit_or_hide("cmd+q") == "quit" and quit_or_hide("Command+H") == "hide"
    assert quit_or_hide("cmd+alt+h") == "hide_others" and quit_or_hide("cmd+shift+q") is None
    assert shortcut_combo("Q", 0) == "cmd+q" and shortcut_combo("H", 2) == "cmd+alt+h" and shortcut_combo("x", 8) == "x"


def _launched(task: str, app: RunningApp, unsaved: bool | None = False) -> tuple[LaunchedApps, list[str]]:
    done: list[str] = []
    la = LaunchedApps(lambda: [app], lambda t, a: False, unsaved=lambda pid: unsaved,
                      quit=lambda pid, b: done.append(f"quit {b}") or True,
                      hide=lambda pid, b: done.append(f"hide {b}") or True, now=lambda: "now")
    la.add(task, Launched(app.bundle_id, app.name, app.pid, "now"))
    return la, done


# decision: HAND-200
def test_screen_wide_keys_are_refused_on_every_route(monkeypatch: pytest.MonkeyPatch) -> None:
    d, apps, posted = background_mac(monkeypatch, Ax(focused="button"))
    ids(d)
    for mechanism in ("pid", "ax", "foreground"):
        for combo in ("cmd+tab", "cmd+space", "ctrl+right", "cmd+alt+esc"):
            with pytest.raises(Refused, match="whole Mac"):
                asyncio.run(d.act("WhatsApp", ComputerAction("key", None, combo, mechanism=mechanism,  # type: ignore[arg-type]
                                                             allow_foreground=True), task="t1"))
    assert posted == [] and apps.activated == []


# decision: HAND-200
def test_cmd_q_quits_only_an_app_ferb_opened_without_unsaved_work(monkeypatch: pytest.MonkeyPatch) -> None:
    d, apps, posted = background_mac(monkeypatch, Ax(focused="button"))
    chat = apps.find("WhatsApp")
    assert chat is not None
    d.launched, done = _launched("t1.worker", chat)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+q"), task="t1.step2"))
    assert r.ok and "asked WhatsApp to quit" in r.detail and done == [f"quit {CHAT}"] and posted == []
    assert d.launched.of("t1") == []                                       # no longer the task's
    with pytest.raises(Refused, match="isn't an app FERB opened"):          # quit already: not FERB's to quit again
        asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+q"), task="t1"))
    d.launched, done = _launched("t1", chat, unsaved=True)
    with pytest.raises(Refused, match="unsaved changes"):
        asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+q", mechanism="ax"), task="t1"))
    d.launched, done = _launched("t1", chat, unsaved=None)
    with pytest.raises(Refused, match="couldn't tell"):
        asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+q"), task="t1"))
    with pytest.raises(Refused, match="isn't an app FERB opened"):          # another task's app
        asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+q"), task="t2"))
    assert done == [] and posted == []


# decision: HAND-200, HAND-219
def test_cmd_h_hides_a_launched_app_and_hide_others_is_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    d, apps, posted = background_mac(monkeypatch, Ax(focused="button"))
    chat = apps.find("WhatsApp")
    assert chat is not None
    d.launched, done = _launched("t1", chat)
    assert asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+h"), task="t1")).ok
    assert done == [f"hide {CHAT}"]
    with pytest.raises(Refused, match="every other app"):
        asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+alt+h"), task="t1"))
    d.launched = None
    with pytest.raises(Refused, match="opened for the task"):
        asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+h"), task="t1"))
    assert posted == []


# decision: HAND-219
def test_the_quit_menu_item_goes_through_the_same_guard(monkeypatch: pytest.MonkeyPatch) -> None:
    """By any mechanism: mac.menu's path, a press on the item, a click on it all carry ⌘Q."""
    d, apps, posted = background_mac(monkeypatch, Ax(focused="button"))
    d.launched = LaunchedApps(lambda: [], lambda t, a: False)              # FERB opened nothing for this task
    pressed: list[Any] = []
    d.ax.menu_item_shortcut = lambda pid, path: ("Q", 0) if path[-1].startswith("Quit") else None  # type: ignore[attr-defined]
    d.ax.press_menu = lambda pid, path: pressed.append(path) or True      # type: ignore[attr-defined]
    with pytest.raises(Refused, match="isn't an app FERB opened"):
        asyncio.run(d.press_menu("WhatsApp", ["WhatsApp", "Quit WhatsApp"], task="t1"))
    assert asyncio.run(d.press_menu("WhatsApp", ["File", "Search"], task="t1")) and pressed == [["File", "Search"]]
    chat = apps.find("WhatsApp")
    assert chat is not None
    d.launched, done = _launched("t1", chat)
    assert asyncio.run(d.press_menu("WhatsApp", ["WhatsApp", "Quit WhatsApp"], task="t1"))
    assert done == [f"quit {CHAT}"] and pressed == [["File", "Search"]]   # FERB's safe quit, not the item


# decision: HAND-219
def test_a_press_on_a_quit_menu_item_is_guarded(monkeypatch: pytest.MonkeyPatch) -> None:
    ax = Ax(focused="button")
    ax.read_tree = lambda *a: AxTree([AxNode("quit", "menuitem", "Quit WhatsApp", None, (0, 0, 9, 9), False,  # type: ignore[method-assign]
                                             True, "AXMenuItem")], "", "Chat")
    ax.role_of = lambda el: "AXMenuItem"                                 # type: ignore[attr-defined]
    ax.item_shortcut = lambda el: ("Q", 0)                               # type: ignore[attr-defined]
    d, apps, posted = background_mac(monkeypatch, ax)
    d.launched = LaunchedApps(lambda: [], lambda t, a: False)              # FERB opened nothing for this task
    with pytest.raises(Refused, match="isn't an app FERB opened"):
        asyncio.run(d.act("WhatsApp", ComputerAction("press", ids(d)["Quit WhatsApp"]), task="t1"))
    assert "press" not in ax.calls


# ---------- 6. the pointer check works for apps with two windows ----------

class TwoWindowAx:
    """An app with two windows: its sole-window lookup answers None (as accessibility.app_window does); each window
    is read by its own walk. `hit_el` is the app's hit test."""

    def __init__(self) -> None:
        self.clicks = 0
        self.hit_el: Any = "send"
        self.button = AxNode("send", "button", "Send", None, (400.0, 190.0, 40.0, 20.0), False, True, "AXButton",
                             path=("AXWindow", "AXGroup"), index=3)

    def read_tree(self, pid: int, browser: bool = False, window: Any = None, offset: int = 0) -> AxTree:
        if window is None:
            return AxTree(title="")                                       # no window to read without one named
        return AxTree([self.button], f"clicks={self.clicks}", "Chat A", secure=False, secret_rects=[], window=window)

    def app_window(self, pid: int) -> Any:
        return None

    def ax_windows(self, pid: int) -> list[Any]:
        return ["A", "B"]

    def expose_tree(self, pid: int) -> bool:
        return True

    def window_with_frame(self, pid: int, bounds: Any, title: Any = None, current_title: Any = None) -> Any:
        return "A"

    def hit_element(self, pid: int, x: float, y: float) -> Any:
        return self.hit_el

    def frame(self, el: Any) -> Any:
        return BOUNDS if el in ("A", "B") else None

    def element_window(self, el: Any) -> Any:
        return "A"

    def window_title(self, win: Any) -> str:
        return "Chat A"


def _two_windows(monkeypatch: pytest.MonkeyPatch) -> tuple[Any, TwoWindowAx, list[str]]:
    d, _, shots, apps, posted = vision(monkeypatch)
    ax = TwoWindowAx()
    d.ax = ax

    def clicks(start: Any, end: Any, kind: str, pid: int, **kw: Any) -> tuple[list[Step], Any]:
        def click() -> None:
            posted.append(f"{kind} {start}")
            ax.clicks += 1
        return [Step(kind, click)], lambda: None
    monkeypatch.setattr(drv, "pointer_steps", clicks)
    return d, ax, posted


def test_a_click_in_an_app_with_two_windows_goes_through_and_sees_its_change(monkeypatch: pytest.MonkeyPatch) -> None:
    """Live: mac.click failed 43% of the time; with 2+ windows the staleness gate walked no window at all."""
    d, ax, posted = _two_windows(monkeypatch)
    _, shot, _ = asyncio.run(d.screenshot("Notes", window="110"))
    assert shot.nodes and shot.signature is not None                      # the captured window's own walk
    r = asyncio.run(d.pointer_at("Notes", shot.id, 640, 300, "click"))   # (420, 200) on screen: the Send button
    assert r.ok and posted == ["click (420.0, 200.0)"] and r.facts["observed_change"] is True


def test_another_control_under_the_point_is_stale(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, posted = _two_windows(monkeypatch)
    _, shot, _ = asyncio.run(d.screenshot("Notes", window="110"))
    ax.hit_el = "delete"                                                  # the list scrolled: another control there
    with pytest.raises(StaleRef, match="another control"):
        asyncio.run(d.pointer_at("Notes", shot.id, 640, 300, "click"))
    ax.hit_el = None                                                      # no hit test (another Space): the walk
    ax.clicks = 5                                                         # the window changed since
    with pytest.raises(StaleRef, match="changed since the screenshot"):
        asyncio.run(d.pointer_at("Notes", shot.id, 640, 300, "click"))
    assert posted == []


def test_ensure_window_is_ready_with_two_windows(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, posted = _two_windows(monkeypatch)
    assert asyncio.run(d.ensure_window("com.apple.Notes", wait_s=0)) is True


def test_window_by_id_breaks_a_frame_tie_by_title(monkeypatch: pytest.MonkeyPatch) -> None:
    """Two maximized windows share a frame: the id's window is the one whose title the window server gives it."""
    monkeypatch.setattr(axm, "ax_windows", lambda pid: ["w1", "w2"])
    monkeypatch.setattr(axm, "cg_windows", lambda pid: [{"id": 7, "title": "Report.pages", "bounds": BOUNDS},
                                                        {"id": 8, "title": "Notes.pages", "bounds": BOUNDS}])
    monkeypatch.setattr(axm, "frame", lambda w: BOUNDS)
    monkeypatch.setattr(axm, "window_title", lambda w: {"w1": "Notes.pages", "w2": "Report.pages"}[w])
    assert axm.window_named(1, "7") == "w2" and axm.window_named(1, "8") == "w1"


# ---------- 7. responsiveness ----------

# decision: HAND-221
def test_ax_elements_get_a_messaging_timeout(monkeypatch: pytest.MonkeyPatch) -> None:
    import sys
    calls: list[tuple[Any, float]] = []
    fake = SimpleNamespace(AXUIElementCreateSystemWide=lambda: "system", AXUIElementCreateApplication=lambda p: f"app{p}",
                           AXUIElementSetMessagingTimeout=lambda el, s: calls.append((el, s)))
    monkeypatch.setitem(sys.modules, "ApplicationServices", fake)
    monkeypatch.setattr(axm, "_timeout_set", False)
    assert axm._app(42) == "app42"
    assert calls == [("system", axm.AX_MESSAGE_TIMEOUT_S), ("app42", axm.AX_MESSAGE_TIMEOUT_S)]
    axm._app(43)
    assert calls[-1] == ("app43", axm.AX_MESSAGE_TIMEOUT_S) and len(calls) == 3     # the global one is set once


# decision: HAND-221
def test_a_walk_stops_at_its_budget_and_says_it_cant_tell_about_passwords(monkeypatch: pytest.MonkeyPatch) -> None:
    from tests.test_mac_snapshot import El, fake_ax
    fake_ax(monkeypatch)
    monkeypatch.setattr(axm, "WALK_MAX_NODES", 5)
    win = El("AXWindow", children=[El("AXButton", {"AXTitle": f"b{i}"}, acts=["AXPress"]) for i in range(20)])
    tree = axm.AxTree(_deadline=time.monotonic() + 60)
    axm._walk(win, tree, [], 0, None, [0])
    assert "budget" in tree.truncated_by and len(tree.nodes) < 20


def test_pid_typing_gives_the_gui_lock_up_between_chunks_and_posts_off_the_loop() -> None:
    gui = GuiLock()
    seen: list[tuple[int, str | None, bool]] = []

    async def sleep(s: float) -> None:
        await asyncio.sleep(0)
    ri = RealInput(lambda: False, gui, sleep=sleep)

    async def go() -> None:
        loop_thread = threading.get_ident()
        steps = [Step(f"type {i}", lambda i=i: seen.append((i, gui.holder, threading.get_ident() != loop_thread)))
                 for i in range(5)]
        other: list[str] = []

        async def rival() -> None:
            await asyncio.sleep(0)
            async with gui.hold("w2"):
                other.append("w2 ran")
        task = asyncio.ensure_future(rival())
        await ri.to_process(steps, "w1", release_every=2)
        await task
        assert other == ["w2 ran"]
    asyncio.run(go())
    assert [i for i, _, _ in seen] == [0, 1, 2, 3, 4] and all(off for _, _, off in seen)
    assert all(holder == "w1" for _, holder, _ in seen)


def test_cleanup_runs_once_at_the_end_of_chunked_steps() -> None:
    cleaned: list[str] = []

    async def sleep(s: float) -> None:
        return None
    ri = RealInput(lambda: False, GuiLock(), sleep=sleep)
    asyncio.run(ri.to_process([Step("s", lambda: None)] * 5, "w1", cleanup=lambda: cleaned.append("c"),
                              release_every=2))
    assert cleaned == ["c"]


# ---------- 8. the one foreground path ----------

def test_a_foreground_press_raises_its_window_and_checks_what_is_under_the_point(
        monkeypatch: pytest.MonkeyPatch) -> None:
    from tests.mackit import mac
    d, ax, apps, posted = mac(monkeypatch)
    _, out = asyncio.run(d.snapshot("Notes"))
    save = next(x["id"] for x in out["elements"] if x["label"] == "Save")
    r = asyncio.run(d.act("Notes", ComputerAction("press", save, mechanism="foreground", allow_foreground=True)))
    assert r.ok and "ax.raise" in ax.calls and posted == ["click 10.0,10.0"]
    posted.clear()
    d.shots.top = 999                                                     # another window over the point now
    with pytest.raises(StaleRef, match="another window"):
        asyncio.run(d.act("Notes", ComputerAction("press", save, mechanism="foreground", allow_foreground=True)))
    assert posted == []


def test_a_foreground_scroll_raises_and_checks_too(monkeypatch: pytest.MonkeyPatch) -> None:
    from tests.mackit import mac
    d, ax, apps, posted = mac(monkeypatch)
    ax.scroll_area_in = lambda win: "pane"                                # type: ignore[attr-defined]
    ax.frame = lambda el: (110.0, 60.0, 100.0, 100.0) if el == "pane" else BOUNDS if el == "win" else None  # type: ignore[method-assign]
    ax.window_named = lambda pid, w: "win"                               # type: ignore[attr-defined]
    monkeypatch.setattr(drv, "wheel_steps", lambda x, y, lines, pid=None, **kw: [
        Step("wheel", lambda: posted.append(f"wheel {x},{y}"))])
    asyncio.run(d.snapshot("Notes"))
    scroll = ComputerAction("scroll", None, mechanism="foreground", allow_foreground=True, direction="down",
                            window="My note")
    r = asyncio.run(d.act("Notes", scroll))
    assert r.ok and "ax.raise" in ax.calls and posted == ["wheel 160.0,110.0"]
    posted.clear()
    d.shots.top = 999
    with pytest.raises(StaleRef):
        asyncio.run(d.act("Notes", scroll))
    assert posted == []


def test_click_at_honours_the_shot_age_on_every_mechanism(monkeypatch: pytest.MonkeyPatch) -> None:
    d, ax, shots, apps, posted = vision(monkeypatch)
    _, shot, _ = asyncio.run(d.screenshot("Notes"))
    d._state["com.apple.Notes"].shot = dataclasses.replace(shot, at=time.monotonic() - 31)
    ax.hit_el = "Export"
    for mechanism in ("ax", "foreground", "pointer"):
        with pytest.raises(StaleRef):
            asyncio.run(d.click_at("Notes", shot.id, 400, 300, mechanism=mechanism, allow_foreground=True))
    assert posted == [] and "ax.press" not in ax.calls


# ---------- 9. mechanisms honoured or refused ----------

def test_focus_and_select_refuse_routes_they_dont_take(monkeypatch: pytest.MonkeyPatch) -> None:
    d, apps, posted = background_mac(monkeypatch, Ax())
    e = ids(d)
    for kind, text in (("focus", None), ("select", "Large")):
        for mechanism in ("pid", "ax_value"):
            with pytest.raises(Refused, match=f"not {mechanism}"):
                asyncio.run(d.act("WhatsApp", ComputerAction(kind, e["Message"], text, mechanism=mechanism)))  # type: ignore[arg-type]
    assert "focus" not in d.ax.calls


# ---------- 10. stable P19 identity ----------

# decision: HAND-220
def test_a_new_snapshot_keeps_a_press_the_same_action(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac.tools import MacAct
    from ferb.daemon.gateway import hashed_args
    d, apps, posted = background_mac(monkeypatch, Ax())
    cap = MacAct(d)
    first = hashed_args(cap, cap.Input(app="WhatsApp", action="press", element=ids(d)["Send"]))
    again = ids(d)["Send"]                                                 # a new snapshot: a new id
    second = hashed_args(cap, cap.Input(app="WhatsApp", action="press", element=again))
    assert again != "s1e2" and first == second and first["element"]["label"] == "Send"
    other = hashed_args(cap, cap.Input(app="WhatsApp", action="press", element=ids(d)["Message"]))
    assert other != second


# decision: HAND-220
def test_a_new_screenshot_keeps_a_click_on_the_same_control_the_same_action(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac.tools import MacClick, MacPointer
    from ferb.daemon.gateway import hashed_args
    d, ax, posted = _two_windows(monkeypatch)
    cap = MacClick(d)
    _, shot, _ = asyncio.run(d.screenshot("Notes", window="110"))
    first = hashed_args(cap, cap.Input(app="Notes", shot=shot.id, x=640, y=300))
    _, shot2, _ = asyncio.run(d.screenshot("Notes", window="110"))
    second = hashed_args(cap, cap.Input(app="Notes", shot=shot2.id, x=650, y=310))   # a few pixels off, same button
    assert shot2.id != shot.id and first == second and first["at"]["label"] == "Send"
    elsewhere = hashed_args(cap, cap.Input(app="Notes", shot=shot2.id, x=100, y=100))
    assert elsewhere != second and "point" in elsewhere["at"]
    pointer = MacPointer(d)
    spec = pointer.describe(pointer.Input(app="Notes", shot=shot2.id, x=650, y=310, action="click"))
    assert "650" not in str(spec.content) and shot2.id not in str(spec.content)


# decision: HAND-220
def test_a_snapshot_elements_identity_holds_no_snapshot_id_or_value() -> None:
    n = AxNode("ref", "textbox", "", "secret draft", (0, 0, 1, 1), False, True, "AXTextField", subrole="AXSearchField",
               path=("AXWindow", "AXGroup"), index=4)
    assert n.identity() == {"path": "AXWindow/AXGroup", "role": "AXTextField", "subrole": "AXSearchField",
                            "label": "", "index": 4}


# ---------- 11. the marks path is gone ----------


# ---------- 12. ScreenCaptureKit "Failed to start stream" ----------

# decision: HAND-405
def test_a_capture_that_fails_to_start_is_tried_once_more(monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.mac import screenshot
    tries: list[int] = []

    def once(w: Any, max_edge: Any) -> tuple[bytes, int, int]:
        tries.append(1)
        if len(tries) == 1:
            raise screenshot.CaptureError("Failed to start stream", "com.apple.ScreenCaptureKit.SCStreamErrorDomain",
                                          screenshot.START_FAILED)
        return b"jpeg", 10, 10
    monkeypatch.setattr(screenshot, "_capture_once", once)
    monkeypatch.setattr(screenshot.time, "sleep", lambda s: None)
    monkeypatch.setattr(axm, "window_minimized", lambda pid, bounds: False)
    w = screenshot.Window(1, 2, (0.0, 0.0, 10.0, 10.0), "")
    assert screenshot.capture(w) == (b"jpeg", 10, 10) and len(tries) == 2
    tries.clear()

    def other(w: Any, max_edge: Any) -> tuple[bytes, int, int]:
        tries.append(1)
        raise screenshot.CaptureError("denied", "x", -3801)
    monkeypatch.setattr(screenshot, "_capture_once", other)
    with pytest.raises(screenshot.CaptureError):
        screenshot.capture(w)
    assert len(tries) == 1                                                # any other failure: not retried


def test_after_an_unknown_press_a_fresh_snapshot_doesnt_escape_the_repeat_check(
        daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """kb area gateway, P19 bypass by re-observing: after an unknown-outcome "Send", a new snapshot gave the button a
    new id and the identical press ran again unasked. The press is identified by what the button is."""
    from ferb.act.mac.tools import MacAct, MacSnapshot
    from tests.conftest import gateway_request, gateway_setup, gateway_tool_call
    d, apps, posted = background_mac(monkeypatch, Ax())
    act, snap = MacAct(d), MacSnapshot(d)
    ran: list[str] = []

    async def lost(args: Any, ctx: Any) -> Any:
        ran.append(str(args.element))
        raise ConnectionError("lost the acknowledgement")
    act._run = lost                                                       # type: ignore[method-assign]
    grant, _ = gateway_setup(daemon, {"mac:read", "mac:act"}, gateway_request("message Asha on WhatsApp"),
                             [act, snap])

    def send_id() -> str:
        out = gateway_tool_call(daemon, grant, "mac.snapshot", app="WhatsApp")
        return next(e["id"] for e in out["content"]["elements"] if e["label"] == "Send")
    first = send_id()
    out = gateway_tool_call(daemon, grant, "mac.act", app="WhatsApp", action="press", element=first)
    assert "outcome unknown" in out["content"]["error"] and ran == [first]
    again = send_id()
    assert again != first
    out = gateway_tool_call(daemon, grant, "mac.act", app="WhatsApp", action="press", element=again)
    assert out["content"].get("retry_action") and ran == [first]          # refused: the same press, not run again


# decision: HAND-409
def test_foreground_input_refused_while_the_owner_cant_be_watched_says_so_not_paused(daemon: Any) -> None:
    """Fix batch 2026-11-04: with the owner-input tap down the daemon counts the owner as using the Mac (SENSE-406),
    and the refusal said "FERB is paused", which wasn't true. The daemon's own input says which it is."""
    from ferb.act.mac.input import PAUSED, UNMONITORED
    inp = daemon.daemon.mac.input
    inp.available = lambda: False                                    # as while the tap is down
    with pytest.raises(Refused) as caught:
        asyncio.run(inp.run([], "w", "type"))
    assert str(caught.value) == UNMONITORED and "Input Monitoring" in UNMONITORED
    sense, daemon.daemon.sense = daemon.daemon.sense, SimpleNamespace(paused=True)
    try:
        assert inp.why_unavailable() == PAUSED                       # the real pause keeps its words
    finally:
        daemon.daemon.sense = sense
