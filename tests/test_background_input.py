"""Native input regressions: exact targets, one explicit mechanism, background delivery facts and no focus restore."""

import asyncio
from dataclasses import dataclass
from typing import Any

import pytest

import ferb.act.mac.driver as drv
from ferb.act.mac.accessibility import AxNode, AxTree
from ferb.act.mac.apps import RunningApp
from ferb.act.mac.input import Step
from ferb.act.mac.profile import Profiles
from ferb.core.errors import Refused
from ferb.core.types import ComputerAction
from tests.mackit import Ax, CHAT, ScrollAx, ids, background_mac as setup


def test_typing_and_keys_go_to_the_app_in_the_background(monkeypatch: pytest.MonkeyPatch) -> None:
    ax = Ax(focused="button")
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("type", e["Message"], "on my way")))
    assert r.ok and "background" in r.detail and ax.box == "on my way"
    r = asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "tab")))
    assert r.ok and "background" in r.detail
    assert posted == ["type 'on my way' to pid", "key tab to pid"]
    assert apps.activated == []                                    # the owner's screen never moved
    assert d.profiles.prefers(CHAT, "pid_keys") and d.profiles.prefers(CHAT, "pid_keycodes")   # learned


def test_unobserved_background_input_needs_an_explicit_next_method(monkeypatch):
    ax = Ax(takes_background=False)
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)
    async def work():
        result = await d.act('WhatsApp', ComputerAction('type', e['Message'], 'hi', allow_foreground=True))
        assert result.facts['delivered'] and not result.facts['observed_change']
        assert ax.calls.count('ax keys hi') == 0 and apps.activated == []
        result = await d.act('WhatsApp', ComputerAction('type', e['Message'], '!', mechanism='foreground', allow_foreground=True))
        assert result.ok and result.facts['method'] == 'foreground'
    asyncio.run(work())
    assert posted == ["type 'hi' to pid", "type '!' foreground"]
    assert apps.activated == [CHAT]


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
        for _ in range(3):
            await d.act("WhatsApp", ComputerAction("type", e["Message"], "hi"))
        assert d.profiles.get(CHAT).mechanisms["pid_keys"].failed == 3
        ax.takes_background = True                                            # the app takes them again (a dialog went)
        r = await d.act("WhatsApp", ComputerAction("type", e["Message"], "yo"))
        assert "in the background" in r.detail and d.profiles.status(CHAT, "pid_keys") == "landed"
    asyncio.run(work())
    again = Profiles(tmp_path / "apps")                                       # persisted per app, counts kept
    m = again.get(CHAT).mechanisms["pid_keys"]
    assert m.landed == 1 and m.failed == 3 and m.streak == 0 and m.last == "landed"
    assert (tmp_path / "apps" / "net.whatsapp.WhatsApp.json").exists()


def test_explicit_foreground_never_restores_a_saved_owner_app(monkeypatch):
    d, apps, _ = setup(monkeypatch, Ax())
    async def work():
        await d.act('WhatsApp', ComputerAction('key', text='tab', mechanism='foreground', allow_foreground=True))
        await asyncio.sleep(0.05)
    asyncio.run(work())
    assert apps.activated == [CHAT]
    assert not hasattr(d, 'restore_owner')


def test_owner_input_stops_foreground_without_reactivation(monkeypatch):
    from ferb.act.mac.input import Yielded
    d, apps, posted = setup(monkeypatch, Ax(), [False, True])
    with pytest.raises(Yielded):
        asyncio.run(d.act('WhatsApp', ComputerAction('key', text='tab', mechanism='foreground', allow_foreground=True)))
    assert apps.activated == [CHAT] and posted == []


def test_failed_ax_press_does_not_choose_an_enclosing_control_or_return(monkeypatch):
    ax = Ax(press_ok=False)
    ax.press_ancestor = lambda el: pytest.fail('ancestor is another target')
    d, apps, posted = setup(monkeypatch, ax)
    result = asyncio.run(d.act('WhatsApp', ComputerAction('press', ids(d)['Send'])))
    assert not result.ok and posted == [] and apps.activated == []


def test_arrow_keys_are_available_in_text_fields_and_lists(monkeypatch):
    for focus, key in (("textbox", "left"), ("button", "down")):
        d, _, posted = setup(monkeypatch, Ax(focused=focus))
        ids(d)
        assert asyncio.run(d.act("WhatsApp", ComputerAction("key", None, key))).ok
        assert posted


def test_return_is_not_blocked_by_a_send_label_or_messaging_app(monkeypatch):
    for has_send in (True, False):
        d, _, posted = setup(monkeypatch, Ax(focused="textbox", send_button=has_send), comm=True)
        ids(d)
        assert asyncio.run(d.act("WhatsApp", ComputerAction("key", None, "return"))).ok


def test_explicit_ax_shortcut_observes_menu_enabled_state_without_key_fallback(monkeypatch):
    ax = Ax(shortcut=('item', 'Edit > Select All', False))
    d, apps, posted = setup(monkeypatch, ax)
    result = asyncio.run(d.act('WhatsApp', ComputerAction('key', text='cmd+a', mechanism='ax')))
    assert not result.ok and 'disabled' in result.detail and posted == []
    ax.shortcut = ('item', 'Edit > Select All', True)
    assert asyncio.run(d.act('WhatsApp', ComputerAction('key', text='cmd+a', mechanism='ax'))).ok
    ax.shortcut = None
    assert not asyncio.run(d.act('WhatsApp', ComputerAction('key', text='cmd+k', mechanism='ax'))).ok
    assert posted == [] and apps.activated == []
    assert asyncio.run(d.act('WhatsApp', ComputerAction('key', text='cmd+k', mechanism='pid'))).ok
    assert posted == ['key cmd+k to pid']


def test_cancel_requires_the_exact_selected_sheet(monkeypatch):
    ax = Ax(panels=[('sheet', 'save-sheet')])
    d, apps, posted = setup(monkeypatch, ax)
    d._state[CHAT] = drv._AppState(gen=1, nodes={'s1e1': AxNode('save-sheet', 'sheet', 'Save', None, (0,0,10,10), False, True)})
    result = asyncio.run(d.act('WhatsApp', ComputerAction('cancel', 's1e1')))
    assert result.ok and 'cancel save-sheet' in ax.calls and posted == [] and apps.activated == []


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
    asyncio.run(work())
    assert not [p for p in posted if p.startswith("type")] and ax.box == ""      # nothing typed anywhere
    assert apps.frontmost().bundle_id == "com.microsoft.VSCode"                   # type: ignore[union-attr]


def test_background_delivery_does_not_add_app_activation(monkeypatch):
    from ferb.act.mac import applescript
    d, _, posted = setup(monkeypatch, Ax(focused="button"))
    ids(d)
    for combo in ("cmd+tab", "ctrl+left", "cmd+q"):
        asyncio.run(d.act("WhatsApp", ComputerAction("key", None, combo)))
    assert not d.apps.activated
    assert "activate" in applescript.wrap(CHAT, "activate")


def test_windowless_app_is_reported_without_selecting_a_reopen_menu(monkeypatch):
    ax = Ax()
    ax.app_window = lambda pid: None
    ax.open_window = lambda *args: pytest.fail('reopen menu must be an agent choice')
    d, apps, _ = setup(monkeypatch, ax)
    assert asyncio.run(d.ensure_window(CHAT, wait_s=0)) is False
    assert apps.activated == []


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


def test_ax_keyboard_is_an_explicit_background_method(monkeypatch):
    ax = Ax(takes_background=False, focused='button', box_has_focus=True, ax_keys=True)
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)
    result = asyncio.run(d.act('WhatsApp', ComputerAction('type', e['Message'], 'syllabus', mechanism='ax_keyboard')))
    assert result.ok and result.facts['method'] == 'ax_keyboard' and ax.box == 'syllabus'
    assert 'ax keys syllabus' in ax.calls and posted == [] and apps.activated == []
    assert d.profiles.prefers(CHAT, 'ax_keyboard')


def test_unknown_focus_never_clicks_a_different_control_then_types(monkeypatch):
    ax = Ax(takes_background=False, box_has_focus=None)
    d, apps, posted = setup(monkeypatch, ax)
    e = ids(d)
    with pytest.raises(Refused, match='keyboard focus'):
        asyncio.run(d.act('WhatsApp', ComputerAction('type', e['Message'], 'syllabus')))
    assert posted == [] and apps.activated == []
    result = asyncio.run(d.act('WhatsApp', ComputerAction('type', e['Message'], 'syllabus', mechanism='foreground')))
    assert result.facts['code'] == 'needs_foreground' and apps.activated == []


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
    r = asyncio.run(d.act("WhatsApp", ComputerAction("edit", e["Message"], "X", select=(0, 5), mechanism="ax_value")))
    assert r.ok and ax.box == "X bravo charlie " and "rewriting" in r.detail


@dataclass
class LaggingAx(Ax):
    """A web text field (Chrome): AXValue is set at once, but the accessibility value reads the old text for a few
    reads before it catches up, as Chrome updates its tree asynchronously."""
    lag: int = 3
    old: str = ""
    reads: int = 0

    def set_value(self, el: Any, text: str) -> bool:
        self.box, self.reads = text, 0
        return True

    def value(self, el: Any) -> str | None:
        if el != "box":
            return None
        self.reads += 1
        return self.box if self.reads > self.lag else self.old


def test_set_value_waits_for_a_web_fields_value_to_catch_up(monkeypatch: pytest.MonkeyPatch) -> None:
    """Live 2026-10-24 (usvisascheduling.com Username in Chrome): set_value said "not confirmed" though the field took
    the value; the worker typed it again and the field held it twice. The read-back waits for the value to settle."""
    ax = LaggingAx()
    d, apps, posted = setup(monkeypatch, ax)
    monkeypatch.setattr(drv, "SETTLE_S", 2.0)
    e = ids(d)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("set_value", e["Message"], "Kartikey Attri", mechanism="ax_value")))
    assert r.ok and r.detail == "value set and read back" and ax.reads == ax.lag + 1
    assert r.facts == {"method": "ax_value", "accepted": True, "value_matches": True}
    assert posted == [] and apps.activated == []


def test_set_value_that_never_reads_back_says_the_field_may_already_hold_text(monkeypatch: pytest.MonkeyPatch) -> None:
    ax = LaggingAx(lag=10_000, old="ktky")
    d, apps, posted = setup(monkeypatch, ax)
    monkeypatch.setattr(drv, "SETTLE_S", 0.1)
    e = ids(d)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("set_value", e["Message"], "Kartikey Attri", mechanism="ax_value")))
    assert not r.ok and ax.reads > 1                                    # it waited, reading again
    assert r.facts == {"method": "ax_value", "accepted": True, "value_matches": False, "field_empty": False,
                       "field_length": 4}                               # a snapshot's facts, never the text itself
    assert "no typing attempted" in r.detail and "fresh snapshot before typing again" in r.detail
    assert "typing adds to what is there" in r.detail and "ktky" not in r.detail
    assert posted == []


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
    r = asyncio.run(d.act("WhatsApp", ComputerAction("select", e["Format"], "Plain Text")))
    assert r.ok and ax.chosen == "Plain Text" and ax.calls[1:] == ["press pop", "press item:Plain Text"]
    r = asyncio.run(d.act("WhatsApp", ComputerAction("select", e["Format"], "Markdown")))
    assert not r.ok and r.facts["candidates"] == [{"label":"Plain Text"}, {"label":"Rich Text"}] and "cancel menu" in ax.calls   # closed again
    assert posted == [] and apps.activated == []


def test_scrolling_prefers_target_scroll_bar_then_text_view_page_keys(monkeypatch: pytest.MonkeyPatch) -> None:
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
    assert r.ok and "scroll bar" in r.detail and posted == [] and ax.position == 2
    assert "focus" not in ax.calls
    ax.keys_scroll = False                                                   # keys do nothing: the scroll bar
    r = asyncio.run(d.act("WhatsApp", ComputerAction("scroll", None, direction="up", window="Untitled")))
    assert r.ok and "scroll bar" in r.detail and ax.position == 1
    ax.bar = False
    ax.keys_scroll = True
    r = asyncio.run(d.act("WhatsApp", ComputerAction("scroll", e["Document"], direction="down", mechanism="pid")))
    assert r.ok and "Page Down" in r.detail and ax.position == 3 and apps.activated == []


def test_no_cleanup_activation_overrides_a_later_owner_choice(monkeypatch):
    d, apps, posted = setup(monkeypatch, Ax())
    async def work():
        await d.act('WhatsApp', ComputerAction('key', text='tab', mechanism='foreground', allow_foreground=True))
        apps.apps = [RunningApp(a.bundle_id, a.name, a.pid, False) for a in apps.apps]
        apps.apps.append(RunningApp('com.apple.Calculator', 'Calculator', 99, True))
        apps.activated.clear()
        await asyncio.sleep(0.05)
    asyncio.run(work())
    assert apps.activated == [] and apps.frontmost().bundle_id == 'com.apple.Calculator'


def test_closing_a_popup_menu_the_app_doesnt_answer_keeps_a_known_non_selection_failed(
        monkeypatch: pytest.MonkeyPatch) -> None:
    """No matching item is a known outcome; the cleanup's AXCancel timing out (NoAnswer) must not make it unknown."""
    from ferb.act.mac.accessibility import NoAnswer
    ax = PopupAx()

    def busy(el: Any) -> bool:
        ax.calls.append(f"cancel {el}")
        raise NoAnswer("AXCancel")
    ax.cancel = busy                                                 # type: ignore[method-assign]
    d, apps, posted = setup(monkeypatch, ax)
    r = asyncio.run(d.act("WhatsApp", ComputerAction("select", ids(d)["Format"], "Markdown")))
    assert not r.ok and r.facts["code"] == "target_unresolved" and "cancel menu" in ax.calls
