"""Drive the real typed-text watcher with synthetic CGEvents handed straight to its callback. No event tap is ever
installed against the session and no event is posted; the focused app and its AX element are fakes."""
import asyncio
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.sanitize.redact import Redactor
from ferb.sense.watchers import text_input
from ferb.sense.watchers.base import Focused
from ferb.sense.watchers.text_input import TextInputWatcher


def key_event(chars: str) -> Any:
    """A synthetic key-down carrying `chars`, as the listen-only tap would deliver it (never posted)."""
    import Quartz
    event = Quartz.CGEventCreateKeyboardEvent(None, 0, True)
    Quartz.CGEventSetFlags(event, 0)        # no modifiers: a new event otherwise copies the live session's (Cmd held)
    Quartz.CGEventKeyboardSetUnicodeString(event, len(chars.encode('utf-16-le')) // 2, chars)
    return event


class Tap:
    """The part of the listen-only input tap the watcher uses."""
    running = True
    on_text: Any = None


class Screen:
    """The focused app and its focused element as Accessibility would report them."""

    def __init__(self, bundle_id: str = 'com.apple.Safari', role: str = 'AXTextField', subrole: str = '',
                 label: str = 'Search', **attrs: Any) -> None:
        self.app = Focused(4242, bundle_id, bundle_id.rsplit('.', 1)[-1])
        self.element, self.window = object(), object()
        self.attrs = {'AXRole': role, 'AXSubrole': subrole, 'AXTitle': label, 'AXIdentifier': 'field-1',
                      'AXValue': '', 'AXWindow': self.window} | attrs
        self.queried: list[str] = []

    def attr(self, element: Any, name: str) -> Any:
        self.queried.append(name)
        if name == 'AXFocusedUIElement':
            return self.element
        if element is self.window:
            return 7 if name == 'AXWindowNumber' else None
        return self.attrs.get(name)


def capture(monkeypatch: pytest.MonkeyPatch, screen: Screen, events: list[Any], *, secure: bool = False) -> list[Any]:
    """Run the real watcher over `events` and return what it recorded once the hold window has passed."""
    clock, checks = SimpleNamespace(now=0.0), SimpleNamespace(n=0)

    def secure_input() -> bool:               # every key and every flush asks this first: it counts the polls
        checks.n += 1
        return secure
    monkeypatch.setattr(text_input, 'time', SimpleNamespace(monotonic=lambda: clock.now))
    monkeypatch.setattr(text_input, 'secure_input', secure_input)
    monkeypatch.setattr(text_input, 'focused_app', lambda: screen.app)
    monkeypatch.setattr('ferb.act.mac.accessibility._attr', screen.attr)

    async def run() -> list[Any]:
        tap, out, stop = Tap(), [], asyncio.Event()
        task = asyncio.create_task(TextInputWatcher(tap, Redactor(), lambda bundle, name: False).run(out.append, stop))
        while tap.on_text is None:
            await asyncio.sleep(0)
        for event in events:
            tap.on_text(event, False)
        clock.now = 10.0                       # past the hold window: the next poll flushes the field
        seen, deadline = checks.n, asyncio.get_running_loop().time() + 10
        while checks.n < seen + 4 and asyncio.get_running_loop().time() < deadline:
            await asyncio.sleep(0.01)          # at least one whole flush after the hold window, however slow
        await asyncio.sleep(0)                 # its emit, handed to this loop, runs
        stop.set()
        await task
        return out
    return asyncio.run(run())


def texts(observations: list[Any]) -> list[str]:
    return [o.data['text'] for o in observations if o.source == 'typed_text']
