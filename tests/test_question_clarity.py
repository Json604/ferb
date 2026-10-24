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


# ---------- a short line FERB's code writes, the exact question under Details ----------

def test_the_secret_question_says_in_one_line_where_it_goes_and_what_ferb_does(daemon: Any,
                                                                                 monkeypatch: pytest.MonkeyPatch) -> None:
    """The site comes from the code-derived origin; the worker's proposed name appears only as the saved name."""
    published = _published(daemon, monkeypatch)
    saved = 'bank.example.com login'                      # a worker's name that reads like a place: never the place
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), VISA, remember=saved),
                                               daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        want = ("atlasauth.b2clogin.com needs your password or another private answer. FERB saves it in your Mac's "
                "Keychain as 'bank.example.com login' and fills it only there.")
        assert ap.summary == want and Redactor().text(want) == want
        assert next(e for e in published if e['approval_id'] == ap.id)['summary'] == want
        assert _status_of(daemon, ap.id)['summary'] == want
        assert VISA in ap.reason                           # the exact destination stays in the full question
    finally:
        _cancel(daemon, ap, waiting)


@pytest.mark.parametrize('target, shown, place', [('app:com.example.Bank', 'Bank', 'The Bank app'),
                                                  ('app:com.example.Bank', None, 'The app com.example.Bank'),
                                                  ('https://x.test:8443', None, 'x.test')])
def test_the_secret_summary_names_the_app_or_site_without_saving(daemon: Any, target: str, shown: str | None,
                                                                 place: str) -> None:
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), target, shown=shown),
                                               daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        assert ap.summary == (f"{place} needs your password or another private answer. FERB fills it only there and "
                              "doesn't save it.")
        assert Redactor().text(ap.summary) == ap.summary
    finally:
        _cancel(daemon, ap, waiting)


def test_mac_secret_tells_the_question_the_apps_name() -> None:
    from types import SimpleNamespace
    from ferb.act.mac.tools import MacSecret
    from tests.support.mac_secret import secret_driver
    asked: list[dict[str, Any]] = []
    cap = MacSecret(secret_driver('com.example.Bank', 'Bank', asked))
    assert not asyncio.run(cap.run(cap.Input(app='Bank', element='e1'), SimpleNamespace(task_id='t'))).is_error
    assert asked == [{'target': 'app:com.example.Bank', 'handle': None, 'remember': None, 'shown': 'Bank'}]


@pytest.mark.parametrize('origin, host', [('https://accounts.google.com', 'accounts.google.com'),
                                          ('https://1234.example.com', '1234.example.com'),
                                          ('https://x.test:8443', 'x.test')])
def test_the_login_code_question_says_in_one_line_where_it_goes(daemon: Any, monkeypatch: pytest.MonkeyPatch,
                                                                origin: str, host: str) -> None:
    published = _published(daemon, monkeypatch)
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_login_code(_ctx(), origin), daemon.loop)
    ap = _asked(daemon, 'owner_login_code')
    try:
        want = f"{host} asks for a login code. FERB fills it only there and doesn't save it."
        assert ap.summary == want and Redactor().text(want) == want
        assert next(e for e in published if e['approval_id'] == ap.id)['summary'] == want
        assert _status_of(daemon, ap.id)['summary'] == want
    finally:
        _cancel(daemon, ap, waiting)


def _escape(daemon: Any, tmp_path: Any, rule: str, choice: int = 2) -> list[Any]:
    from types import SimpleNamespace
    from ferb.core.errors import Refused
    from tests.conftest import Answers, gateway_request, gateway_setup
    grant, _ = gateway_setup(daemon, {'shell:run'}, gateway_request('Check my extension.'), [])
    rec = daemon.daemon.tasks.get(grant.task_id)
    args = SimpleNamespace(escape=rule, command='ls "Default/Extensions"', cwd=str(tmp_path))
    answers = Answers(daemon, choice=choice)
    try:
        with pytest.raises(Refused):
            daemon.run(daemon.daemon.gateway._escape(grant, rec, args, 'the worker wants to look inside'))
    finally:
        answers.stop()
    return answers.asked


@pytest.mark.parametrize('rule, want', [
    ('read:~/Library/Application Support/Google/Chrome',
     "FERB wants to read ~/Library/Application Support/Google/Chrome once, with one command (normally off-limits: a "
     "private store FERB's shell never reads)."),
    ('write:~/.zshrc', "FERB wants to change ~/.zshrc once, with one command (normally off-limits: FERB's shell never "
                       "writes there)."),
    ('trustd', "FERB wants to let one command use macOS's certificate service, trustd (normally off: it reaches "
               "certificate addresses outside FERB's proxy)."),
])
def test_the_escape_question_says_what_ferb_wants_and_the_rule_it_lifts(daemon: Any, tmp_path: Any, rule: str,
                                                                       want: str) -> None:
    asked = _escape(daemon, tmp_path, rule)
    assert [ap.summary for ap in asked] == [want]
    assert 'Default/Extensions' in asked[0].reason and rule in asked[0].reason   # the exact command and rule id stay
    assert 'the worker wants to look inside' not in asked[0].summary


def test_the_admin_escape_and_its_password_question_each_say_what_they_are(daemon: Any, tmp_path: Any) -> None:
    asked = _escape(daemon, tmp_path, 'admin', choice=1)                    # Yes, then no password: cancelled
    assert [ap.summary for ap in asked] == [
        "FERB wants to run one command as administrator (sudo), with your Mac password (normally off-limits: FERB's "
        "shell never uses admin rights).",
        "Your Mac password, for the one command you just approved. FERB passes it only to that command and doesn't "
        "save it."]
    assert asked[1].kind == 'owner_admin_password'


def _need(rule: str, **values: str) -> Any:
    from ferb.policy import confirm
    return confirm.Need(rule, confirm.key(rule, values), values)


@pytest.mark.parametrize('needs, want', [
    ([_need('floor:pay', amount='20', site='amazon.com', account='', recipients='')],
     'FERB wants to pay 20 on amazon.com.'),
    ([_need('floor:account_security', site='accounts.google.com', account='me@x.com')],
     'FERB wants to change a password, sign-in or recovery setting on accounts.google.com for me@x.com.'),
    ([_need('floor:delete_permanent', path='/Users/k/Desktop/old.txt')],
     'FERB wants to permanently delete /Users/k/Desktop/old.txt, skipping the Trash.'),
    ([_need('floor:delete_permanent', path='"Sprint notes"', place='notion.so')],
     'FERB wants to permanently delete "Sprint notes" on notion.so.'),
    ([_need('floor:new_recipient', recipients='bob@x.com', verb='sends to')],
     "FERB wants to send to bob@x.com, whom you didn't name."),
    ([_need('floor:new_recipient', recipients='', verb='sends to', place='WhatsApp', account='')],
     "FERB wants to send something in WhatsApp, to someone the step doesn't name."),
    ([_need('floor:new_recipient', recipients='', verb='calls', place='FaceTime', account='')],
     "FERB wants to start a call in FaceTime, with someone the step doesn't name."),
    ([_need('sink:new_destination', destination='paste.example', read='the web page on kartikey.xyz')],
     "FERB wants to send to paste.example, a place you haven't named, after reading the web page on kartikey.xyz."),
    ([_need('sink:private_url', destination='evil.example', value='kartikey@x.com', read='an email')],
     'FERB wants to open evil.example with your own data in the address (kartikey@x.com).'),
    ([_need('sink:outside_value', what='typed', value='+1 555 0100', origin='an email')],
     'FERB wants to type +1 555 0100, which came from an email, not from you.'),
    ([_need('floor:new_recipient', recipients='a@x.com, b@x.com', verb='sends to'),
      _need('sink:irreversible_bulk', what='send', count='2', bytes='12 bytes', sample='"a@x.com", "b@x.com"')],
     "FERB wants to send to a@x.com, b@x.com, whom you didn't name."),
    ([_need('sink:irreversible_bulk', what='send', count='3', bytes='12 bytes', sample='"a", "b", "c"')],
     'FERB wants to send to 3 recipients at once: "a", "b", "c".'),
    ([_need('floor:new_recipient', recipients='bob@x.com', verb='sends to'),
      _need('sink:outside_value', what='recipient', value='bob@x.com', origin='the web page on x.com')],
     "FERB wants to send to bob@x.com, whom you didn't name; use the recipient bob@x.com, which came from the web page "
     "on x.com, not from you."),
])
def test_a_code_confirmation_says_the_concrete_action_in_one_line(needs: list[Any], want: str) -> None:
    from ferb.policy import confirm
    assert confirm.summary(needs) == want


def test_a_bulk_delete_summary_counts_what_goes() -> None:
    from ferb.policy import confirm
    need = confirm.bulk_delete('/Users/k/old', {'path': '/Users/k/old', 'files': 3, 'directories': 1, 'bytes': 2048,
                                                'paths': ['/Users/k/old/', '/Users/k/old/a', '/Users/k/old/b']})
    assert confirm.summary([need]) == ('FERB wants to permanently delete /Users/k/old: 3 files in 1 folder, '
                                       '2048 bytes.')


def test_a_code_confirmation_carries_its_summary_to_the_app_and_keeps_the_step_in_details(
        daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.policy import confirm
    from tests.conftest import Answers, gateway_request, gateway_setup
    published = _published(daemon, monkeypatch)
    grant, _ = gateway_setup(daemon, {'mail:send'}, gateway_request('Send the report.'), [])
    rec = daemon.daemon.tasks.get(grant.task_id)
    needs = [confirm.Need('floor:new_recipient', confirm.key('floor:new_recipient', ['bob@x.com']),
                          {'recipients': 'bob@x.com', 'verb': 'sends to'})]
    answers = Answers(daemon, choice=2)
    try:
        answer = daemon.run(daemon.daemon.gateway.ask_confirmation(grant, rec, needs, None,
                                                                   'mail.send to bob@x.com', 'FERB: it is fine'))
    finally:
        answers.stop()
    assert not answer
    want = "FERB wants to send to bob@x.com, whom you didn't name."
    [ap] = answers.asked
    assert ap.summary == want and want != ap.reason
    assert 'mail.send to bob@x.com' in ap.reason and 'it is fine' in ap.reason     # the step and reason: Details
    assert 'it is fine' not in ap.summary                                          # the task's words never the line
    assert next(e for e in published if e['approval_id'] == ap.id)['summary'] == want


def test_chat_status_lists_a_code_questions_summary(daemon: Any) -> None:
    from ferb.policy.rules import ActionRequest
    action = ActionRequest('ferb.confirm', 'owner_question', Identity.OWNER, frozenset(), 't', 'w', 'r', frozenset())
    ap = daemon.daemon.state.approvals.question(action, 'FERB: this sends to bob@x.com … Go ahead?', ['Yes', 'No'])
    ap.summary = "FERB wants to send to bob@x.com, whom you didn't name."
    assert _status_of(daemon, ap.id)['summary'] == ap.summary
    plain = daemon.daemon.state.approvals.question(action, 'Which file?', [])
    assert 'summary' not in _status_of(daemon, plain.id)


def test_a_waiting_code_questions_banner_carries_its_short_line(daemon: Any) -> None:
    notes: list[tuple[str, str]] = []

    async def note(title: str, text: str) -> None:
        notes.append((title, text))
    daemon.daemon.notify = note

    async def ask() -> None:
        daemon.daemon._publish('approval.requested', {
            'approval_id': 'ap-banner', 'capability': 'secret.fill', 'kind': 'owner_secret', 'task_id': 't-away',
            'reason': f'Destination: {VISA}. Type the password or secret for it here (hidden; Enter cancels).',
            'summary': "atlasauth.b2clogin.com needs your password or another private answer."})
        await asyncio.sleep(0.05)                      # the banner is posted from a task the publish starts
    daemon.run(ask())
    assert notes == [('FERB needs you',
                      'FERB needs your OK: atlasauth.b2clogin.com needs your password or another private answer.')]


@pytest.mark.parametrize('handle', [None, 'visa'])
def test_ask_secret_refuses_a_whole_browser_before_any_question(daemon: Any, handle: str | None) -> None:
    """Defense in depth for any caller: the gateway itself never asks for, saves or fills a secret bound to a whole
    browser app."""
    from ferb.core.errors import Refused
    with pytest.raises(Refused, match='whole web browser'):
        daemon.run(daemon.daemon.gateway.ask_secret(_ctx(), 'app:com.google.Chrome', handle=handle, remember='visa'))
    assert not [ap for ap in daemon.daemon.state.approvals.pending() if ap.kind == 'owner_secret']
