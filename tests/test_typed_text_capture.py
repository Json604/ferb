"""L17: typed text is captured per field through the native adapter (D7), never from a secure field, while Secure
Input is on, or in a terminal app. Key events here are synthetic CGEvents handed straight to the watcher's callback:
no event tap is ever installed against the session."""
from typing import Any

import pytest

from ferb.sense.watchers.text_input import key_text
from tests.support.typed_text import Screen, capture, key_event, texts

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
