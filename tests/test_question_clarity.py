"""The questions FERB's own code builds say what FERB wants in plain words, and keep their exact scope (P27, D33).
Owner, 2026-10-24: "the questions it asks are too ambiguous and doesn't simply state what is it and what it wants.
so here we should have the short version and in details see if you want the raw things".

Live 2026-10-24, visa login: the hidden-input question read "Enter the secret for app:[redacted]": its own words
"the secret for <X>" matched the redactor's label rule, so the destination it asked about was hidden from the owner."""
import asyncio
import time
from typing import Any

import pytest

from ferb.core.types import Identity, ToolContext
from ferb.sanitize.redact import Redactor

VISA = 'https://atlasauth.b2clogin.com'
SAVED = 'US Visa Scheduling password for ktky.attri@gmail.com'


def _ctx() -> ToolContext:
    return ToolContext('t-visa', 'w1', 'r-visa', Identity.OWNER, frozenset({'browser:act'}), 'dev', None, '/tmp', (),
                       ())


def _asked(daemon: Any, kind: str) -> Any:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        found = [ap for ap in daemon.daemon.state.approvals.pending() if ap.kind == kind]
        if found:
            return found[0]
        time.sleep(0.01)
    raise AssertionError(f'no {kind} question was asked')


def _published(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> list[dict[str, Any]]:
    """What clients receive as approval.requested: the bus event, redacted as the daemon publishes it."""
    seen: list[dict[str, Any]] = []
    bus = daemon.daemon.bus
    real = bus.publish

    def publish(topic: str, data: dict[str, Any]) -> Any:
        if topic == 'approval.requested':
            seen.append(data)
        return real(topic, data)
    monkeypatch.setattr(bus, 'publish', publish)
    return seen


def _status_of(daemon: Any, approval_id: str) -> dict[str, Any]:
    status = daemon.run(daemon.daemon._chat_status(None, {'peek': True}))      # chat.status redacts its approvals
    return next(a for a in status['approvals'] if a['approval_id'] == approval_id)


def _cancel(daemon: Any, ap: Any, waiting: Any) -> None:
    daemon.call_soon(daemon.daemon.state.approvals.resolve, ap.id, False)
    assert waiting.result(5) is None


@pytest.mark.parametrize('target', [VISA, 'app:com.apple.Notes', 'https://x.test:8443', 'https://1234.example.com',
                                    'https://password.example.com', 'https://xn--bcher-kva.example'])
def test_the_secret_question_keeps_its_destination_through_redaction(daemon: Any, monkeypatch: pytest.MonkeyPatch,
                                                                     target: str) -> None:
    published = _published(daemon, monkeypatch)
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), target, remember=SAVED),
                                               daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        assert target in Redactor().text(ap.reason)
        assert target in _status_of(daemon, ap.id)['reason']
        assert target in next(e for e in published if e['approval_id'] == ap.id)['reason']
        assert SAVED in ap.reason and 'hidden' in ap.reason
    finally:
        _cancel(daemon, ap, waiting)


@pytest.mark.parametrize('origin', ['https://accounts.google.com', 'https://x.test:8443', 'https://1234.example.com',
                                    'https://otp.example.com'])
def test_the_login_code_question_keeps_its_destination_through_redaction(daemon: Any, monkeypatch: pytest.MonkeyPatch,
                                                                         origin: str) -> None:
    published = _published(daemon, monkeypatch)
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_login_code(_ctx(), origin), daemon.loop)
    ap = _asked(daemon, 'owner_login_code')
    try:
        assert origin in Redactor().text(ap.reason)
        assert origin in _status_of(daemon, ap.id)['reason']
        assert origin in next(e for e in published if e['approval_id'] == ap.id)['reason']
    finally:
        _cancel(daemon, ap, waiting)
