"""Mac input order, every app (PLATFORM_MACOS.md §12 and the owner-verified rules; rework phase 3): Accessibility and
per-process events with the app in the background first, in the ranked order per primitive, each rung only after the
one before verifiably did nothing; the per-app learned profile remembers what landed; the foreground only when the
call allows it (`allow_foreground`), once per stretch of work, with the owner's input held back and winning;
arrow-key editing, Return-instead-of-Send, disabled shortcuts and app switching refused."""

import asyncio
from dataclasses import dataclass, field
from typing import Any

import pytest

import ferb.act.mac.driver as drv
from ferb.act.gui_lock import GuiLock
from ferb.act.mac.accessibility import AxNode, AxTree
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.input import RealInput, Step
from ferb.act.mac.profile import MISS_LIMIT, Profiles
from ferb.core.errors import Refused
from ferb.core.types import ComputerAction

CHAT = "net.whatsapp.WhatsApp"


@dataclass
class Ax:
    """A fake app window: a text box, a Send button; typing lands only if the app takes background keystrokes."""
    takes_background: bool = True
    focused: str | None = "textbox"
    box_has_focus: bool | None = True                       # False: a sheet / dialog holds the keyboard focus
    press_ok: bool = True                                   # False: the control ignores Accessibility presses
    ax_keys: bool = False                                   # True: the app takes the Accessibility keyboard (spike 20)
    send_button: bool = True
    box: str = ""
    text: str = ""
    shortcut: Any = None                                    # what menu_item_for_shortcut answers
    calls: list[str] = field(default_factory=list)
    panels: list[tuple[str, Any]] = field(default_factory=list)

    def read_tree(self, pid: int, browser: bool = False, window: Any = None) -> AxTree:
        nodes = [AxNode("box", "textbox", "Message", self.box, (0, 0, 10, 10), False, True)]
        if self.send_button:
            nodes.append(AxNode("send", "button", "Send", None, (20, 0, 10, 10), False, True))
        return AxTree(nodes, self.text, "Chat")

    def value(self, el: Any) -> str | None:
        return self.box if el == "box" else None

    def expose_tree(self, pid: int) -> bool:
        self.calls.append("expose")
        return True

    def post_text(self, pid: int, text: str) -> bool:
        self.calls.append(f"ax keys {text}")
        if self.ax_keys:
            self.box += text
        return True

    def post_key(self, pid: int, keycode: int, mods: list[str]) -> bool:
        self.calls.append(f"ax key {keycode}")
        if self.ax_keys:
            self.text += f"[{keycode}]"
        return True

    def window_title(self, win: Any) -> str:
        return "" if win is None else getattr(self, "title", "Chat")

    def app_window(self, pid: int) -> Any:
        return "win"

    def focused_role(self, pid: int) -> str | None:
        return self.focused

    def has_focus(self, pid: int, el: Any) -> bool | None:
        return self.box_has_focus

    def focus(self, el: Any) -> bool:
        self.calls.append("focus")
        return True

    def set_value(self, el: Any, text: str) -> bool:
        return False                                        # this app ignores Accessibility values

    def press(self, el: Any) -> bool:
        self.calls.append("press")
        return self.press_ok

    def press_ancestor(self, el: Any) -> str | None:
        self.calls.append("press ancestor")
        return None

    def center(self, n: AxNode) -> tuple[float, float]:
        return 5.0, 5.0

    def menu_item_for_shortcut(self, pid: int, key: str, mods: list[str]) -> Any:
        return self.shortcut

    def open_panels(self, pid: int, win: Any) -> list[tuple[str, Any]]:
        return list(self.panels)

    def open_menus(self, pid: int) -> list[Any]:
        return []

    def cancel(self, el: Any) -> bool:
        self.calls.append(f"cancel {el}")
        self.panels = []
        return True


class Apps:
    def __init__(self) -> None:
        self.apps = [RunningApp(CHAT, "WhatsApp", 21, False), RunningApp("com.apple.TextEdit", "TextEdit", 22, False),
                     RunningApp("com.microsoft.VSCode", "Code", 23, True)]
        self.activated: list[str] = []

    def find(self, q: str) -> RunningApp | None:
        return next((a for a in self.apps if q.lower() in (a.bundle_id.lower(), a.name.lower())), None)

    def frontmost(self) -> RunningApp | None:
        return next(a for a in self.apps if a.active)

    def activate(self, bid: str) -> bool:
        self.activated.append(bid)
        self.apps = [RunningApp(a.bundle_id, a.name, a.pid, a.bundle_id == bid) for a in self.apps]
        return True


class Guard:
    def __init__(self, log: list[str]) -> None:
        self.log = log

    def engage(self) -> bool:
        self.log.append("guard on")
        return True

    def release(self) -> None:
        self.log.append("guard off")


def setup(mp: pytest.MonkeyPatch, ax: Ax, owner: list[bool] | None = None,
          comm: bool = False, profiles: Profiles | None = None) -> tuple[MacComputerDriver, Apps, list[str]]:
    apps, posted = Apps(), []
    owner = owner if owner is not None else []

    async def no_sleep(s: float) -> None:
        return None

    def busy() -> bool:                              # each check of the owner takes the next answer
        return owner.pop(0) if owner else False

    def typed(text: str, pid: int | None = None) -> list[Step]:
        def post() -> None:
            posted.append(f"type {text!r} {'to pid' if pid else 'foreground'}")
            if pid is None or ax.takes_background:
                ax.box += text
        return [Step("type", post)]

    def keyed(combo: str, pid: int | None = None) -> list[Step]:
        def post() -> None:
            posted.append(f"key {combo} {'to pid' if pid else 'foreground'}")
            if pid is None or ax.takes_background:
                ax.text += f"[{combo}]"
        return [Step(combo, post)]
    mp.setattr(drv, "type_steps", typed)
    mp.setattr(drv, "key_steps", keyed)
    mp.setattr(drv, "click_steps", lambda x, y, pid=None: [Step("click", lambda: posted.append("click foreground"))])
    mp.setattr(drv, "SETTLE_S", 0)
    d = MacComputerDriver("dev", RealInput(busy, GuiLock(), sleep=no_sleep, poll_s=0.1, yield_after_s=5),
                          ax=ax, apps=apps, profiles=profiles)
    d.communication = lambda b: comm and b == CHAT
    d.guard = Guard(posted)
    return d, apps, posted


def ids(d: MacComputerDriver, app: str = "WhatsApp") -> dict[str, str]:
    _, out = asyncio.run(d.snapshot(app))
    return {e["label"]: e["id"] for e in out["elements"]}


def test_typing_and_keys_go_to_the_app_in_the_background(monkeypatch: pytest.MonkeyPatch) -> None:
    ax = Ax(focused="button")
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("set_value", e["Message"], "on my way")))
    assert r.ok and "background" in r.detail and ax.box == "on my way"
    r = asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "tab")))
    assert r.ok and "background" in r.detail
    assert posted == ["type 'on my way' to pid", "key tab to pid"]
    assert apps.activated == []                                    # the owner's screen never moved
    assert d.profiles.prefers(CHAT, "pid_keys") and d.profiles.prefers(CHAT, "pid_keycodes")   # learned


def test_an_app_that_ignores_background_keys_is_never_brought_forward_without_leave(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """Every background rung is tried (keys to the process, the Accessibility keyboard once, AXValue); when none
    lands the call fails and says how to allow the foreground. Only after MISS_LIMIT misses in a row is the mechanism
    skipped for that app (a later success forgets it). With allow_foreground the app comes forward once per stretch."""
    ax = Ax(takes_background=False, focused="button")
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)

    async def work() -> None:
        for _ in range(MISS_LIMIT):
            r = await d.act("WhatsApp", ComputerAction("type", e["Message"], "hi"))
            assert not r.ok and "allow_foreground=true" in r.detail            # background only, and it says so
        assert apps.activated == [] and d.profiles.skips(CHAT, "pid_keys")    # remembered now
        assert ax.calls.count("ax keys hi") == 1                              # the AX keyboard was tried once
        r = await d.act("WhatsApp", ComputerAction("type", e["Message"], "!", allow_foreground=True))
        assert r.ok and "real input" in r.detail
        assert posted[-1] == "type '!' foreground" and "type '!' to pid" not in posted   # straight to the front
        await d.act("WhatsApp", ComputerAction("key", None, "tab", allow_foreground=True))
        assert apps.activated == [CHAT]                                      # one switch for the whole stretch
        d.restore_owner()
    asyncio.run(work())
    assert apps.activated == [CHAT, "com.microsoft.VSCode"]                  # and one switch back
    assert posted[0] == "type 'hi' to pid" and "guard on" in posted and posted[-1] == "guard off"


def test_a_key_with_no_visible_effect_never_brands_the_app_foreground_only(monkeypatch: pytest.MonkeyPatch) -> None:
    ax = Ax(takes_background=False, focused="button")                        # tab changes nothing here
    d, apps, posted = setup(monkeypatch, ax)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "tab")))
    assert r.ok and "nothing visibly changed" in r.detail
    assert d.profiles.status(CHAT, "pid_keycodes") is None and apps.activated == []   # not remembered, not forward


def test_a_later_background_success_forgets_earlier_misses(monkeypatch: pytest.MonkeyPatch, tmp_path: Any) -> None:
    ax = Ax(takes_background=False, focused="button")
    profiles = Profiles(tmp_path / "apps")
    d, apps, posted = setup(monkeypatch, ax, profiles=profiles)
    e = ids(d)

    async def work() -> None:
        for _ in range(MISS_LIMIT):
            await d.act("WhatsApp", ComputerAction("type", e["Message"], "hi"))
        assert d.profiles.skips(CHAT, "pid_keys")
        ax.takes_background = True                                            # the app takes them again (a dialog went)
        d.profiles.record(CHAT, "pid_keys", True)                             # e.g. a probe or a later success
        r = await d.act("WhatsApp", ComputerAction("type", e["Message"], "yo"))
        assert "in the background" in r.detail and not d.profiles.skips(CHAT, "pid_keys")
    asyncio.run(work())
    again = Profiles(tmp_path / "apps")                                       # persisted per app, counts kept
    m = again.get(CHAT).mechanisms["pid_keys"]
    assert m.landed == 2 and m.failed == MISS_LIMIT and m.streak == 0 and m.last == "landed"
    assert (tmp_path / "apps" / "net.whatsapp.WhatsApp.json").exists()


def test_the_owners_app_comes_back_after_the_stretch_goes_idle(monkeypatch: pytest.MonkeyPatch) -> None:
    ax = Ax(takes_background=False, focused="button")
    d, apps, _ = setup(monkeypatch, ax)
    e = ids(d)
    monkeypatch.setattr(drv, "STRETCH_IDLE_S", 0.05)

    async def work() -> None:
        await d.act("WhatsApp", ComputerAction("type", e["Message"], "hi", allow_foreground=True))
        assert apps.frontmost().bundle_id == CHAT                     # type: ignore[union-attr]
        await asyncio.sleep(0.1)
    asyncio.run(work())
    assert apps.frontmost().bundle_id == "com.microsoft.VSCode"       # type: ignore[union-attr]


def test_the_owner_touching_the_mac_puts_their_app_back_until_they_stop(monkeypatch: pytest.MonkeyPatch) -> None:
    ax = Ax(takes_background=False, focused="button")
    owner = [False, True, True, False]               # quiet when FERB comes forward; then types; then stops
    d, apps, posted = setup(monkeypatch, ax, owner)
    e = ids(d)

    async def work() -> None:
        await d.act("WhatsApp", ComputerAction("click", e["Send"]))        # a control: pressed in the background
        assert apps.activated == []
        ax.press_ok = False                                                 # this one ignores presses: a real click
        r = await d.act("WhatsApp", ComputerAction("click", e["Send"]))
        assert not r.ok and apps.activated == []                            # ...only with leave
        await d.act("WhatsApp", ComputerAction("click", e["Send"], allow_foreground=True))
        d.restore_owner()
    asyncio.run(work())
    assert apps.activated[:3] == [CHAT, "com.microsoft.VSCode", CHAT]      # forward, aside for the owner, back
    assert "click foreground" in posted


def test_a_press_falls_back_to_the_enclosing_control_then_the_keyboard(monkeypatch: pytest.MonkeyPatch) -> None:
    """Electron wraps the pressable node in groups: the ancestor that lists AXPress takes it; failing that, the
    element is focused and Return goes to the app's process (checked by what changed)."""
    ax = Ax(press_ok=False, focused="button")
    ax.press_ancestor = lambda el: "group “Chats”"                          # type: ignore[method-assign]
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("press", e["Send"])))
    assert r.ok and "enclosing group" in r.detail and posted == []
    ax.press_ancestor = lambda el: None                                      # type: ignore[method-assign]
    r = asyncio.run(d.act("WhatsApp", ComputerAction("press", e["Send"])))
    assert r.ok and "return" in r.detail and posted == ["key return to pid"] and apps.activated == []


def test_arrow_keys_in_a_text_box_are_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    d, _, posted = setup(monkeypatch, Ax(focused="textbox"))
    ids(d)
    with pytest.raises(Refused, match="action edit"):
        asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "left")))
    d2, _, _ = setup(monkeypatch, Ax(focused="button"))             # in a list or menu arrows are fine
    ids(d2)
    assert asyncio.run(d2.act("WhatsApp", ComputerAction("key", None, "down"))).ok
    assert posted == []


def test_return_where_a_send_button_belongs_is_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    d, _, _ = setup(monkeypatch, Ax(focused="textbox", send_button=True))
    e = ids(d)
    with pytest.raises(Refused, match=f"Send button \\({e['Send']}\\)"):
        asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "return")))
    d2, _, _ = setup(monkeypatch, Ax(focused="textbox", send_button=False), comm=True)   # Send not shown yet
    ids(d2)
    with pytest.raises(Refused, match="messaging app"):
        asyncio.run(d2.act("WhatsApp", ComputerAction("key", None, "return")))
    d3, _, _ = setup(monkeypatch, Ax(focused="textbox", send_button=False))              # an ordinary form
    ids(d3)
    assert asyncio.run(d3.act("WhatsApp", ComputerAction("key", None, "return"))).ok


def test_a_shortcut_runs_its_menu_item_and_is_refused_while_that_item_is_disabled(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """Owner-verified (2026-10-02): ⌘A posted to a background TextEdit did nothing and beeped — Edit > Select All
    was disabled with no key window. A shortcut goes through its menu item, only when enabled; otherwise the refusal
    names the Accessibility way to do the same."""
    ax = Ax(focused="button", shortcut=("item", "Edit > Select All", False))
    d, apps, posted = setup(monkeypatch, ax)
    ids(d)
    with pytest.raises(Refused, match="Edit > Select All.*disabled.*action edit"):
        asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+a")))
    assert posted == [] and apps.activated == []                              # no key equivalent was posted
    ax.shortcut = ("item", "Edit > Select All", True)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+a")))
    assert r.ok and "Edit > Select All" in r.detail and "press" in ax.calls and posted == []
    ax.shortcut = None                                                        # no menu item: the key itself goes
    r = asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "cmd+k")))
    assert r.ok and posted == ["key cmd+k to pid"]


def test_escape_dismisses_an_open_sheet_with_axcancel_never_blindly(monkeypatch: pytest.MonkeyPatch) -> None:
    """Escape posted to a process didn't dismiss TextEdit's save sheet (probe); AXCancel on the sheet does."""
    ax = Ax(focused="button", panels=[("sheet", "save-sheet")])
    d, _, posted = setup(monkeypatch, ax)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "escape")))
    assert r.ok and "dismissed the sheet" in r.detail and "cancel save-sheet" in ax.calls and posted == []
    r = asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "escape")))     # nothing open: a plain key
    assert posted == ["key escape to pid"]


def test_nothing_is_typed_while_a_dialog_holds_the_keyboard_focus(monkeypatch: pytest.MonkeyPatch) -> None:
    """Found on the real Mac (2026-10-01): TextEdit's Save sheet held the focus, so keystrokes meant for the document
    would have gone into the file-name box. FERB types only once the box it means has the focus — in the background
    or the foreground — and otherwise says so."""
    ax = Ax(box_has_focus=False, focused="textbox")
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)

    async def work() -> None:
        with pytest.raises(Refused, match="keyboard focus"):
            await d.act("WhatsApp", ComputerAction("type", e["Message"], "hello", allow_foreground=True))
        d.restore_owner()
    asyncio.run(work())
    assert not [p for p in posted if p.startswith("type")] and ax.box == ""      # nothing typed anywhere
    assert apps.frontmost().bundle_id == "com.microsoft.VSCode"                   # type: ignore[union-attr]


def test_ferb_never_switches_the_owners_apps_or_desktops_nor_quits_an_app(monkeypatch: pytest.MonkeyPatch) -> None:
    """Owner's report (2026-10-01): told to "bring WhatsApp to the front", a worker pressed cmd+tab. FERB works with
    apps where they are; keys that switch apps or desktops, and AppleScript activate, are refused. ⌘Q is refused
    too: a background app with unsaved work wedges on its save sheet (PLATFORM_MACOS.md §3)."""
    from ferb.act.mac import applescript
    d, _, posted = setup(monkeypatch, Ax(focused="button"))
    ids(d)
    for combo in ("cmd+tab", "cmd+shift+tab", "ctrl+left", "ctrl+up", "cmd+space"):
        with pytest.raises(Refused, match="switches the owner's apps"):
            asyncio.run(d.act("WhatsApp", ComputerAction("key", None, combo)))
    for combo in ("cmd+q", "cmd+h", "cmd+m"):
        with pytest.raises(Refused, match="never does that"):
            asyncio.run(d.act("WhatsApp", ComputerAction("key", None, combo)))
    assert posted == []
    for script in ("activate", "reopen", "activate\nget name of window 1"):
        with pytest.raises(Refused, match="front"):
            applescript.wrap(CHAT, script)


def test_an_app_running_without_a_window_gets_it_back_in_the_background(monkeypatch: pytest.MonkeyPatch) -> None:
    """Spike 18: WhatsApp can run with no window; its own "Open main window" menu item brings it back, pressed
    through Accessibility — the app stays in the background."""
    ax = Ax()
    windows: list[bool] = [False]
    opened: list[str] = []
    ax.app_window = lambda pid: object() if windows[0] else None           # type: ignore[method-assign]

    def open_window(pid: int, name: str) -> bool:
        opened.append(name)
        windows[0] = True
        return True
    ax.open_window = open_window                                          # type: ignore[attr-defined]
    d, apps, _ = setup(monkeypatch, ax)
    monkeypatch.setattr(drv.asyncio, "sleep", _no_sleep)
    assert asyncio.run(d.ensure_window(CHAT)) is True
    assert opened == ["WhatsApp"] and apps.activated == []


async def _no_sleep(s: float) -> None:
    return None


def test_electron_apps_are_asked_for_their_tree_and_slow_windows_are_waited_for(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """Slack (Electron) hid its controls, so a background press failed and FERB clicked in the foreground. FERB now asks
    each app for its full Accessibility tree (AXManualAccessibility) and, after opening an app, waits for its titled
    main window — Slack first showed an untitled panel."""
    ax = Ax()
    steps = iter([None, "", "", "Slack | general"])                 # nothing yet, an untitled panel, then the window
    current: list[Any] = [None]

    def app_window(pid: int) -> Any:
        current[0] = next(steps, "Slack | general")
        return None if current[0] is None else object()
    ax.app_window = app_window                                       # type: ignore[method-assign]
    ax.window_title = lambda win: current[0] or ""                   # type: ignore[method-assign]
    ax.open_window = lambda pid, name: False                         # type: ignore[attr-defined]
    d, apps, _ = setup(monkeypatch, ax)
    monkeypatch.setattr(drv.asyncio, "sleep", _no_sleep)
    assert asyncio.run(d.ensure_window(CHAT)) is True
    assert ax.calls.count("expose") == 1                             # asked once per app process
    asyncio.run(d.snapshot("WhatsApp"))
    assert ax.calls.count("expose") == 1 and apps.activated == []


def test_a_search_box_that_isnt_a_text_field_is_typed_in_the_background_with_the_ax_keyboard(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """Spike 20: WhatsApp's search box (an unlabeled button) ignores a set value and keystrokes posted to its process,
    but pressed through Accessibility and typed into with the Accessibility keyboard, it searches in the background.
    The profile learns it, and the next time that rung comes first after the process keys."""
    ax = Ax(takes_background=False, focused="button", box_has_focus=False, ax_keys=True)
    d, apps, posted = setup(monkeypatch, ax)
    d._state[CHAT] = drv._AppState(gen=1, nodes={"s1e1": AxNode("btn", "button", "", None, (0, 0, 10, 10), False, True)})
    r = asyncio.run(d.act("WhatsApp", ComputerAction("set_value", "s1e1", "syllabus")))
    assert r.ok and "Accessibility keyboard" in r.detail and ax.box == "syllabus"
    assert "press" in ax.calls and "ax keys syllabus" in ax.calls                # pressed to focus, then typed
    assert apps.activated == [] and posted == []                                 # never came forward
    assert d.profiles.prefers(CHAT, "ax_keyboard")


def test_a_box_that_takes_no_background_input_is_clicked_and_typed_in_front_only_with_leave(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """The last resort for such a box: click it and type with the app in front, then put the owner back — and only
    when the call allows the foreground."""
    ax = Ax(takes_background=False, focused="button", box_has_focus=False)
    d, apps, posted = setup(monkeypatch, ax)
    d._state[CHAT] = drv._AppState(gen=1, nodes={"s1e1": AxNode("btn", "button", "", None, (0, 0, 10, 10), False, True)})

    async def work() -> None:
        r = await d.act("WhatsApp", ComputerAction("set_value", "s1e1", "syllabus"))
        assert not r.ok and "allow_foreground" in r.detail and apps.activated == []
        r = await d.act("WhatsApp", ComputerAction("set_value", "s1e1", "syllabus", allow_foreground=True))
        assert r.ok and "click + typing" in r.detail                          # not "value set"
        d.restore_owner()
    asyncio.run(work())
    assert posted[-4:] == ["guard on", "click foreground", "type 'syllabus' foreground", "guard off"]
    assert apps.activated == [CHAT, "com.microsoft.VSCode"]                  # forward once, then back


# ---------- the new primitives: edit, select, scroll ----------

@dataclass
class TextAx(Ax):
    """A text area that takes AXSelectedTextRange + AXSelectedText (as TextEdit's does, probed 2026-10-02)."""
    selection: tuple[int, int] = (0, 0)
    range_ok: bool = True

    def settable(self, el: Any, attr: str) -> bool:
        return attr in ("AXSelectedTextRange", "AXSelectedText", "AXValue")

    def selected_range(self, el: Any) -> tuple[int, int] | None:
        return self.selection

    def set_selected_range(self, el: Any, start: int, length: int) -> bool:
        if not self.range_ok:
            return False
        self.selection = (start, length)
        return True

    def set_selected_text(self, el: Any, text: str) -> bool:
        if not self.range_ok:
            return False
        s, n = self.selection
        self.box = self.box[:s] + text + self.box[s + n:]
        self.selection = (s + len(text), 0)
        return True

    def set_value(self, el: Any, text: str) -> bool:
        self.box = text
        return True


def test_edit_replaces_or_inserts_exactly_the_range_through_selected_text(monkeypatch: pytest.MonkeyPatch) -> None:
    ax = TextAx(box="alpha bravo ")
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("edit", e["Message"], "ALPHA", select=(0, 5))))
    assert r.ok and ax.box == "ALPHA bravo " and "replaced" in r.detail
    r = asyncio.run(d.act("WhatsApp", ComputerAction("edit", e["Message"], "charlie ", select="end")))
    assert r.ok and ax.box == "ALPHA bravo charlie " and "inserted" in r.detail
    r = asyncio.run(d.act("WhatsApp", ComputerAction("edit", e["Message"], None, select="all")))
    assert r.ok and ax.selection == (0, len(ax.box)) and "selected" in r.detail
    assert posted == [] and apps.activated == [] and d.profiles.prefers(CHAT, "ax_selected_text")
    ax.range_ok = False                                                        # no selection API: value rewritten
    r = asyncio.run(d.act("WhatsApp", ComputerAction("edit", e["Message"], "X", select=(0, 5))))
    assert r.ok and ax.box == "X bravo charlie " and "rewriting" in r.detail


@dataclass
class PopupAx(Ax):
    items: tuple[str, ...] = ("Plain Text", "Rich Text")
    chosen: str = "Rich Text"
    opened: bool = False

    def read_tree(self, pid: int, browser: bool = False, window: Any = None) -> AxTree:
        return AxTree([AxNode("pop", "popup", "Format", self.chosen, (0, 0, 10, 10), False, True, "AXPopUpButton",
                              ("press", "menu"))], "", "Prefs")

    def press(self, el: Any) -> bool:
        self.calls.append(f"press {el}")
        if el == "pop":
            self.opened = True
        elif el.startswith("item:"):
            self.chosen, self.opened = el[5:], False
        return True

    def popup_menu(self, el: Any, pid: int) -> Any:
        return "menu" if self.opened else None

    def menu_items(self, menu: Any) -> list[tuple[Any, str]]:
        return [(f"item:{t}", t) for t in self.items]

    def value(self, el: Any) -> str | None:
        return self.chosen if el == "pop" else None

    def settable(self, el: Any, attr: str) -> bool:
        return False


def test_a_popup_button_item_is_chosen_by_pressing_the_button_then_the_menu_item(
        monkeypatch: pytest.MonkeyPatch) -> None:
    ax = PopupAx()
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("select", e["Format"], "plain text")))
    assert r.ok and ax.chosen == "Plain Text" and ax.calls[1:] == ["press pop", "press item:Plain Text"]
    r = asyncio.run(d.act("WhatsApp", ComputerAction("select", e["Format"], "Markdown")))
    assert not r.ok and "Plain Text, Rich Text" in r.detail and "cancel menu" in ax.calls   # closed again
    assert posted == [] and apps.activated == []


@dataclass
class ScrollAx(Ax):
    position: int = 0
    keys_scroll: bool = True
    bar: bool = True

    def read_tree(self, pid: int, browser: bool = False, window: Any = None) -> AxTree:
        return AxTree([AxNode("area", "textbox", "Document", "text", (0, 0, 10, 10), False, True, "AXTextArea",
                              ("menu",))], f"line {self.position}", "Untitled")

    def scroll_area_of(self, el: Any) -> Any:
        return "scroll"

    def window_named(self, pid: int, which: str) -> Any:
        return "win" if which == "Untitled" else None

    def scroll_area_in(self, win: Any) -> Any:
        return "scroll"

    def frame(self, el: Any) -> Any:
        return (0.0, 0.0, 10.0, 10.0)

    def scroll_content(self, area: Any) -> Any:
        return "area"

    def scroll_position(self, area: Any, content: Any = None) -> tuple[Any, ...]:
        return (self.position / 10, None, (self.position, 20))

    def scroll_bar(self, area: Any, vertical: bool = True) -> Any:
        return "bar" if vertical and self.bar else None

    def perform(self, el: Any, action: str) -> bool:
        self.calls.append(f"{action} {el}")
        if el == "bar":
            self.position += 1 if action == "AXIncrement" else -1
            return True
        return False


def test_scrolling_goes_by_page_keys_with_the_area_focused_then_the_scroll_bar(monkeypatch: pytest.MonkeyPatch) -> None:
    """Owner-verified: Page Down keycodes to the process scroll a background text view; scroll-wheel events don't."""
    ax = ScrollAx()
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)

    def keyed(combo: str, pid: int | None = None) -> list[Step]:
        def post() -> None:
            posted.append(f"key {combo} to pid")
            if ax.keys_scroll:
                ax.position += 2 if combo == "pagedown" else -2
        return [Step(combo, post)]
    monkeypatch.setattr(drv, "key_steps", keyed)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("scroll", e["Document"], direction="down", amount=2)))
    assert r.ok and "Page Down" in r.detail and posted == ["key pagedown to pid"] * 2 and ax.position == 4
    assert "focus" in ax.calls
    ax.keys_scroll = False                                                   # keys do nothing: the scroll bar
    r = asyncio.run(d.act("WhatsApp", ComputerAction("scroll", None, direction="up", window="Untitled")))
    assert r.ok and "scroll bar" in r.detail and ax.position == 3
    ax.bar = False
    r = asyncio.run(d.act("WhatsApp", ComputerAction("scroll", e["Document"], direction="down")))
    assert not r.ok and "allow_foreground" in r.detail and apps.activated == []
