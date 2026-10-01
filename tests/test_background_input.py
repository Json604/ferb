"""Mac input order, every app (section 9.4, spike 18; owner decisions 2026-10-01): Accessibility and per-process
keystrokes with the app in the background first; the foreground only as a last resort, once per stretch of work, with
the owner's input held back and winning; arrow-key editing and Return-instead-of-Send refused."""

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
    send_button: bool = True
    box: str = ""
    text: str = ""
    calls: list[str] = field(default_factory=list)

    def read_tree(self, pid: int, browser: bool = False) -> AxTree:
        nodes = [AxNode("box", "textbox", "Message", self.box, (0, 0, 10, 10), False, True)]
        if self.send_button:
            nodes.append(AxNode("send", "button", "Send", None, (20, 0, 10, 10), False, True))
        return AxTree(nodes, self.text, "Chat")

    def value(self, el: Any) -> str | None:
        return self.box

    def expose_tree(self, pid: int) -> bool:
        self.calls.append("expose")
        return True

    def window_title(self, win: Any) -> str:
        return "" if win is None else getattr(self, "title", "Chat")

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

    def center(self, n: AxNode) -> tuple[float, float]:
        return 5.0, 5.0


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
          comm: bool = False) -> tuple[MacComputerDriver, Apps, list[str]]:
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
    mp.setattr(drv, "click_steps", lambda x, y: [Step("click", lambda: posted.append("click foreground"))])
    mp.setattr(drv, "SETTLE_S", 0)
    d = MacComputerDriver("dev", RealInput(busy, GuiLock(), sleep=no_sleep, poll_s=0.1, yield_after_s=5),
                          ax=ax, apps=apps)
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
    r = asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "escape")))
    assert r.ok and "background" in r.detail
    assert posted == ["type 'on my way' to pid", "key escape to pid"]
    assert apps.activated == []                                    # the owner's screen never moved


def test_an_app_that_ignores_background_keys_is_brought_forward_once_per_stretch(
        monkeypatch: pytest.MonkeyPatch) -> None:
    ax = Ax(takes_background=False, focused="button")
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)

    async def work() -> None:
        r = await d.act("WhatsApp", ComputerAction("type", e["Message"], "hi"))
        assert r.ok and "real input" in r.detail and ax.box == "hi"     # nothing arrived in the background: forward
        await d.act("WhatsApp", ComputerAction("key", None, "tab"))      # remembered: straight to the foreground
        await d.act("WhatsApp", ComputerAction("key", None, "tab"))
        assert apps.activated == [CHAT]                                  # one switch for the whole stretch
        d.restore_owner()
    asyncio.run(work())
    assert apps.activated == [CHAT, "com.microsoft.VSCode"]              # and one switch back
    assert posted == ["type 'hi' to pid", "guard on", "type 'hi' foreground", "key tab foreground",
                      "key tab foreground", "guard off"]                 # the guard covers the whole window


def test_the_owners_app_comes_back_after_the_stretch_goes_idle(monkeypatch: pytest.MonkeyPatch) -> None:
    ax = Ax(takes_background=False, focused="button")
    d, apps, _ = setup(monkeypatch, ax)
    e = ids(d)
    monkeypatch.setattr(drv, "STRETCH_IDLE_S", 0.05)

    async def work() -> None:
        await d.act("WhatsApp", ComputerAction("type", e["Message"], "hi"))
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
        await d.act("WhatsApp", ComputerAction("click", e["Send"]))
        d.restore_owner()
    asyncio.run(work())
    assert apps.activated[:3] == [CHAT, "com.microsoft.VSCode", CHAT]      # forward, aside for the owner, back
    assert "click foreground" in posted


def test_arrow_keys_in_a_text_box_are_refused(monkeypatch: pytest.MonkeyPatch) -> None:
    d, _, posted = setup(monkeypatch, Ax(focused="textbox"))
    ids(d)
    with pytest.raises(Refused, match="whole value"):
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


def test_nothing_is_typed_while_a_dialog_holds_the_keyboard_focus(monkeypatch: pytest.MonkeyPatch) -> None:
    """Found on the real Mac (2026-10-01): TextEdit's Save sheet held the focus, so keystrokes meant for the document
    would have gone into the file-name box. FERB types only once the box it means has the focus — in the background
    or the foreground — and otherwise says so."""
    ax = Ax(box_has_focus=False, focused="textbox")
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)

    async def work() -> None:
        with pytest.raises(Refused, match="keyboard focus"):
            await d.act("WhatsApp", ComputerAction("type", e["Message"], "hello"))
        d.restore_owner()
    asyncio.run(work())
    assert not [p for p in posted if p.startswith("type")] and ax.box == ""      # nothing typed anywhere
    assert apps.frontmost().bundle_id == "com.microsoft.VSCode"                   # type: ignore[union-attr]


def test_ferb_never_switches_the_owners_apps_or_desktops(monkeypatch: pytest.MonkeyPatch) -> None:
    """Owner's report (2026-10-01): told to "bring WhatsApp to the front", a worker pressed cmd+tab. FERB works with
    apps where they are; keys that switch apps or desktops, and AppleScript activate, are refused."""
    from ferb.act.mac import applescript
    d, _, posted = setup(monkeypatch, Ax(focused="button"))
    ids(d)
    for combo in ("cmd+tab", "cmd+shift+tab", "ctrl+left", "ctrl+up"):
        with pytest.raises(Refused, match="switches the owner's apps"):
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
    ax.app_window = lambda pid: object() if windows[0] else None           # type: ignore[attr-defined]

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
    ax.app_window = app_window                                       # type: ignore[attr-defined]
    ax.window_title = lambda win: current[0] or ""                   # type: ignore[method-assign]
    ax.open_window = lambda pid, name: False                         # type: ignore[attr-defined]
    d, apps, _ = setup(monkeypatch, ax)
    monkeypatch.setattr(drv.asyncio, "sleep", _no_sleep)
    assert asyncio.run(d.ensure_window(CHAT)) is True
    assert ax.calls.count("expose") == 1                             # asked once per app process
    asyncio.run(d.snapshot("WhatsApp"))
    assert ax.calls.count("expose") == 1 and apps.activated == []
