"""L17: typed text is captured per field through the native adapter (D7), never from a secure field, while Secure
Input is on, or in a terminal (a terminal app, or a terminal inside another app by D31's facts). Key events here are
synthetic CGEvents handed straight to the watcher's callback: no event tap is ever installed against the session."""
from typing import Any

import pytest

from ferb.sense.watchers.text_input import key_text
from tests.support.terminal_ax import (CLAUDE_TERMINAL, EDITOR, JETBRAINS_EDITOR, JETBRAINS_TERMINAL, PAGE_SHELL_BOX,
                                       PAGE_XTERM, TERMINAL, XCODE_CONSOLE)
from tests.support.typed_text import ChainScreen, Screen, capture, key_event, texts

# The terminal apps the brief names (L17): Terminal, iTerm2, Warp, Ghostty, kitty, Alacritty, WezTerm, Hyper.
TERMINALS = ['com.apple.Terminal', 'com.googlecode.iterm2', 'dev.warp.Warp-Stable', 'com.mitchellh.ghostty',
             'net.kovidgoyal.kitty', 'org.alacritty', 'com.github.wez.wezterm', 'co.zeit.hyper']


def test_native_adapter_feeds_the_typed_character(monkeypatch) -> None:
    """(a) PyObjC gives (0, '') for a zero-length buffer: the adapter must ask with a real length (C1)."""
    import Quartz
    assert Quartz.CGEventKeyboardGetUnicodeString(key_event('a'), 0, None, None) == (0, '')
    assert Quartz.CGEventKeyboardGetUnicodeString(key_event('a'), 4, None, None) == (1, 'a')

    recorded = capture(monkeypatch, Screen(), [key_event('a')])
    assert texts(recorded) == ['a']
    assert recorded[0].data['bundle_id'] == 'com.apple.Safari' and recorded[0].hints['actor'] == 'owner'


def test_native_adapter_reads_the_whole_string_of_a_key_event(monkeypatch) -> None:
    """A key event can carry more than one UTF-16 unit (an emoji, a composed sequence); none of it is cut off."""
    long = 'compose: ünïcødé and 😀 together, longer than any first guess at a buffer'
    assert texts(capture(monkeypatch, Screen(), [key_event('h'), key_event('😀'), key_event(long)])) == \
        ['h😀' + long]


def test_typed_digits_in_an_unlabelled_field_are_kept_as_typed(monkeypatch) -> None:
    """§8.5: no digit-shape rule guesses codes in typed text; only explicit code labels hide a code."""
    assert texts(capture(monkeypatch, Screen(label='Quantity'), [key_event('482913')])) == ['482913']
    assert texts(capture(monkeypatch, Screen(label='Verification code'), [key_event('482913')])) == ['[redacted]']


@pytest.mark.parametrize('case', ['secure field', 'secure input', *TERMINALS, 'com.googlecode.iTerm2'])
def test_secure_fields_secure_input_and_terminal_apps_record_nothing(monkeypatch, case) -> None:
    """(b) The same keystroke in an ordinary field is recorded; in each guarded place nothing is."""
    assert texts(capture(monkeypatch, Screen(), [key_event('s')])) == ['s']

    screen = Screen(subrole='AXSecureTextField') if case == 'secure field' else \
        Screen() if case == 'secure input' else Screen(bundle_id=case, role='AXTextArea')
    assert capture(monkeypatch, screen, [key_event('hunter2')], secure=case == 'secure input') == []
    if case not in ('secure field', 'secure input'):
        assert screen.queried == []            # a terminal's focused element is never even read (where-boundary)


def test_a_key_event_with_a_lone_surrogate_gives_no_text_and_never_loops(monkeypatch) -> None:
    """A split pair is completed by one more unit; a lone surrogate can never decode, so that event's text is dropped
    (it used to grow the buffer forever)."""
    import Quartz
    units, calls = ['a', '\ud800'], []

    def unicode_string(event: Any, size: int, _a: Any, _b: Any) -> tuple[int, str]:
        calls.append(size)
        if len(calls) > 64:
            raise RuntimeError('the adapter keeps asking')
        if '\ud800' in units[:size]:
            raise UnicodeDecodeError('utf-16-le', b'', 0, 1, 'unexpected end of data')
        return len(units[:size]), ''.join(units[:size])
    monkeypatch.setattr(Quartz, 'CGEventKeyboardGetUnicodeString', unicode_string)
    assert key_text(object()) == '' and calls == [4, 5]


def test_polls_read_no_accessibility_while_nothing_is_typed(monkeypatch) -> None:
    """The 100 ms flush asks for the focused field only while typed text is held."""
    screen = Screen()
    assert capture(monkeypatch, screen, []) == []
    assert screen.queried == []


# ---------- D28, D31, rule 14: no terminal surface is captured, by FERB's own terminal facts ----------


@pytest.mark.parametrize('bundle,chain', [
    ('com.microsoft.VSCode', TERMINAL), ('com.microsoft.VSCode', CLAUDE_TERMINAL),
    ('com.todesktop.230313mzl4w4u92', TERMINAL), ('com.google.Chrome', PAGE_XTERM),
    ('com.jetbrains.pycharm', JETBRAINS_TERMINAL), ('com.apple.dt.Xcode', XCODE_CONSOLE)])
def test_owner_keystrokes_in_a_terminal_inside_another_app_are_never_captured(monkeypatch, bundle, chain) -> None:
    """A `sudo` password the owner types into VS Code's terminal (its xterm.js input, as the live trace showed it) has
    no secure field and Secure Input may be off: like a terminal app's, it is never captured. The same terminal facts
    FERB's own typing floor uses (policy/terminal.py) decide it, read from the focus and its ancestors."""
    assert capture(monkeypatch, ChainScreen(bundle, chain), [key_event('hunter2')]) == []


@pytest.mark.parametrize('bundle,chain', [('com.microsoft.VSCode', EDITOR), ('com.jetbrains.pycharm', JETBRAINS_EDITOR),
                                          ('com.google.Chrome', PAGE_SHELL_BOX)])
def test_an_editor_or_a_page_box_beside_a_terminal_is_still_captured(monkeypatch, bundle, chain) -> None:
    """VS Code's editor, a JetBrains editor in a window whose title names a terminal file, and a page box whose label
    says "Shell command" (web content: only a terminal library's classes decide) are ordinary fields."""
    assert texts(capture(monkeypatch, ChainScreen(bundle, chain), [key_event('print(1)')])) == ['print(1)']


@pytest.mark.parametrize('bundle', ['dev.zed.Zed', 'org.tabby', 'com.example.Shelly'])
def test_an_app_that_is_or_hosts_an_unseen_terminal_is_never_read(monkeypatch, bundle) -> None:
    """Zed's terminal panel shows Accessibility nothing, Tabby is a terminal app, and an app whose Info.plist declares
    the Shell role is a terminal: none is captured, and its focused element is never read (a where-boundary)."""
    monkeypatch.setattr('ferb.act.mac.apps.declares_shell', lambda b: b == 'com.example.Shelly')
    screen = ChainScreen(bundle, EDITOR)
    assert capture(monkeypatch, screen, [key_event('hunter2')]) == [] and screen.queried == []


@pytest.mark.parametrize('bundle', ['dev.ferb.notch', 'DEV.FERB.NOTCH', 'com.ferb.ferb', 'com.ferb.app', 'com.ferb.daemon'])
def test_ferb_own_apps_record_nothing_and_are_never_read(monkeypatch, bundle) -> None:
    """Rule 17 (owner, 2026-10-25): what the owner types in FERB's own apps arrives as their message over RPC; reading
    its field per key lagged the notch."""
    screen = Screen(bundle_id=bundle)
    assert capture(monkeypatch, screen, [key_event('hi')]) == []
    assert screen.queried == []


def test_an_app_on_the_owners_terminal_list_is_never_read(monkeypatch) -> None:
    """D31 round 2, N5: the typing floor counts the owner's [protection] terminal_apps; so does capture."""
    from ferb.sense.watchers import text_input
    screen = ChainScreen('com.example.myterm', EDITOR)
    monkeypatch.setattr(text_input, 'owner_terminal_apps', lambda: frozenset({'com.example.myterm'}))
    assert capture(monkeypatch, screen, [key_event('hunter2')]) == [] and screen.queried == []
