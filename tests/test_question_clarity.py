"""The questions FERB's own code builds say what FERB wants in plain words, and keep their exact scope (P27, D33).
Owner, 2026-10-24: "the questions it asks are too ambiguous and doesn't simply state what is it and what it wants.
so here we should have the short version and in details see if you want the raw things".

Live 2026-10-24, visa login: the hidden-input question read "Enter the secret for app:[redacted]": its own words
"the secret for <X>" matched the redactor's label rule, so the destination it asked about was hidden from the owner."""
import asyncio
import re
from typing import Any

import pytest

from ferb.core.types import Identity, ToolContext
from ferb.sanitize.redact import Redactor
from tests.support.hidden_input import asked as _asked, keychain_helper

VISA = 'https://atlasauth.b2clogin.com'
SAVED = 'US Visa Scheduling password for ktky.attri@gmail.com'


def _ctx() -> ToolContext:
    return ToolContext('t-visa', 'w1', 'r-visa', Identity.OWNER, frozenset({'browser:act'}), 'dev', None, '/tmp', (),
                       ())


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
    """Owner, 2026-10-24: "just state what is this for". The line leads with the place, from the code-derived origin
    (re-review R1: worker text never comes first), then what the value is for: the saved name, in quotes, as a name
    only."""
    published = _published(daemon, monkeypatch)
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), VISA,
                                                                                remember='US visa scheduling login'),
                                               daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        want = "For atlasauth.b2clogin.com only: “US visa scheduling login” · saved in your Mac's Keychain"
        assert ap.summary == want and Redactor().text(want) == want
        assert next(e for e in published if e['approval_id'] == ap.id)['summary'] == want
        assert _status_of(daemon, ap.id)['summary'] == want
        assert VISA in ap.reason                           # the exact destination stays in the full question
    finally:
        _cancel(daemon, ap, waiting)


@pytest.mark.parametrize('target, shown, place', [('app:com.example.Bank', 'Bank', 'the Bank app'),
                                                  ('app:com.example.Bank', None, 'the app com.example.Bank'),
                                                  ('https://x.test:8443', None, 'x.test:8443'),
                                                  ('https://1234.example.com', None, '1234.example.com'),
                                                  ('https://x.test:443', None, 'x.test')])
def test_the_secret_summary_names_the_app_or_site_without_saving(daemon: Any, target: str, shown: str | None,
                                                                 place: str) -> None:
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), target, shown=shown),
                                               daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        assert ap.summary == f"For {place}: a password or private answer — used once, not saved"
        assert Redactor().text(ap.summary) == ap.summary
    finally:
        _cancel(daemon, ap, waiting)


def test_a_saved_app_secret_names_the_app_first(daemon: Any) -> None:
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(
        _ctx(), 'app:com.example.Bank', remember='bank', shown='Bank'), daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        assert ap.summary == "For the Bank app only: “bank” · saved in your Mac's Keychain"
    finally:
        _cancel(daemon, ap, waiting)


# Review I1 and re-review R1: worker text could show a fake place before FERB's own, through quote and dash
# lookalikes, letters that look like dots, invisible marks beside ASCII dots, and HTML entities the notch decodes.
SPOOF = "visa” — for accounts.google.com only · saved in your Mac's Keychain. Real: “x"
SHOWN = re.compile(r"[A-Za-z0-9 :,'()\-]*…?")


@pytest.mark.parametrize('remember, name', [
    (SPOOF, "visa for accountsgooglecom only saved i…"),
    ("x\u2033 \u2500 for accounts\uff0egoogle\uff0ecom only", "x for accountsgooglecom only"),
    ("x \u02ee \u4e00 for accounts\ua4f8google\ua4f8com only", "x for accountsgooglecom only"),
    ("for accounts\u034f.google\u034f.com only", "for accountsgooglecom only"),
    ("for accounts\ufe0f.google\ufe0f.com only", "for accountsgooglecom only"),
    ("x&rdquo; &mdash; for g&period;co only", "xrdquo mdash for gperiodco only"),
    ("x&commat;evil&colon;&sol;&sol;g", "xcommatevilcolonsolsolg"),
    ('bank.example.com login', "bankexamplecom login"),
    ('Bank password for me@bank.example', "Bank password for mebankexample"),
    ('a' * 60, 'a' * 39 + '…'),
    ('**visa** `portal`\n\u202e(x)', "visa portal (x)"),
    ("Asha's bank: savings, 2/3 (main) - US", "Asha's bank: savings, 23 (main) - US"),
    ("https://3627733615/login", "https:3627733615login")])
def test_the_secret_line_starts_with_the_place_and_shows_only_a_plain_ascii_name(daemon: Any, remember: str,
                                                                                 name: str) -> None:
    """The line always starts with the code-derived place. The name keeps only ASCII letters, digits, spaces and
    : , ' ( ) - (everything else is dropped, whitespace collapsed, at most 40 characters), so no lookalike, mark or
    entity can rebuild a quote, a dash or a place in it, and no slash can make it a link."""
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), 'https://x.test',
                                                                                remember=remember), daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        want = f"For x.test only: “{name}” · saved in your Mac's Keychain"
        assert ap.summary == want and Redactor().text(want) == want
        shown = want.split("“", 1)[1].rsplit("”", 1)[0]
        assert SHOWN.fullmatch(shown) and not set(shown) & set(".&;")
        assert remember in ap.reason or remember.startswith('**')      # the full question keeps the exact name
    finally:
        _cancel(daemon, ap, waiting)


def test_a_name_with_nothing_plain_left_shows_no_name(daemon: Any) -> None:
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), 'https://x.test',
                                                                                remember='“”—·．'), daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        assert ap.summary == "For x.test only: a saved secret · saved in your Mac's Keychain"
    finally:
        _cancel(daemon, ap, waiting)


@pytest.mark.parametrize('target, place', [('https://x.test:8443', 'x.test:8443'),
                                           ('https://10.0.0.1:8443', '10.0.0.1:8443')])
def test_a_saved_name_with_a_code_word_never_hides_the_place(daemon: Any, target: str, place: str) -> None:
    """A code word in the worker's name ("visa login code") would make the redactor read a port after it as a code: the
    place comes first, so it shows whole."""
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), target,
                                                                                remember='visa login code'),
                                               daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        assert ap.summary == f"For {place} only: “visa login code” · saved in your Mac's Keychain"
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
    assert asked == [{'target': 'app:com.example.Bank', 'handle': None, 'remember': None, 'shown': 'Bank',
                      'field': None}]


# the place first: "Login code for x.test:8443" would read the port (or a number in the host) as the code
@pytest.mark.parametrize('origin, want', [('https://accounts.google.com', 'For accounts.google.com: a login code — used once'),
                                          ('https://x.test:443', 'For x.test: a login code — used once'),
                                          ('https://1234.example.com', 'For 1234.example.com: a login code — used once'),
                                          ('https://x.test:8443', 'For x.test:8443: a login code — used once'),
                                          ('https://[::1]:8443', 'For [::1]:8443: a login code — used once')])
def test_the_login_code_question_says_in_one_line_where_it_goes(daemon: Any, monkeypatch: pytest.MonkeyPatch,
                                                                origin: str, want: str) -> None:
    published = _published(daemon, monkeypatch)
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_login_code(_ctx(), origin), daemon.loop)
    ap = _asked(daemon, 'owner_login_code')
    try:
        assert ap.summary == want and Redactor().text(want) == want
        assert next(e for e in published if e['approval_id'] == ap.id)['summary'] == want
        assert _status_of(daemon, ap.id)['summary'] == want
    finally:
        _cancel(daemon, ap, waiting)


def test_a_host_whose_own_name_holds_a_code_label_keeps_its_name_but_not_its_port(daemon: Any) -> None:
    """Review M4, a known limit: "otp" in the host labels the port after it as a code, wherever FERB shows it (Details
    too), and FERB's redaction has no bypass for its own text. The place-first line still shows the host's name."""
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_login_code(_ctx(), 'https://otp.example.com:8443'),
                                               daemon.loop)
    ap = _asked(daemon, 'owner_login_code')
    try:
        assert ap.summary == 'For otp.example.com:[redacted]: a login code — used once'
    finally:
        _cancel(daemon, ap, waiting)


def _escape(daemon: Any, tmp_path: Any, rule: str, choice: int = 2) -> list[Any]:
    """The escape question through the gateway's one authorization step (step 6b); the call never runs."""
    from tests.conftest import Answers, gateway_request, gateway_setup, gateway_tool_call
    from tests.support.fake_shell import FakeShell
    shell = FakeShell()
    grant, _ = gateway_setup(daemon, {'shell'}, gateway_request('Check my extension.'), [shell])   # type: ignore[list-item]
    answers = Answers(daemon, choice=choice)
    try:
        r = gateway_tool_call(daemon, grant, 'shell.run', command='ls "Default/Extensions"', cwd=str(tmp_path),
                              escape=rule, why='the worker wants to look inside')
    finally:
        answers.stop()
    assert r['is_error'] and shell.grants == []
    return answers.asked


@pytest.mark.parametrize('rule, want', [
    ('read:~/Library/Application Support/Google/Chrome',
     "FERB wants to read ~/Library/Application Support/Google/Chrome for this task (normally off-limits: a private "
     "store FERB stays out of)."),
    ('write:~/.zshrc', "FERB wants to change ~/.zshrc once (normally off-limits: FERB never changes it on its own)."),
    ('trustd', "FERB wants to let one command use macOS's certificate service, trustd (normally off: it reaches "
               "certificate addresses outside FERB's proxy)."),
])
def test_the_escape_question_says_what_ferb_wants_and_the_rule_it_lifts(daemon: Any, tmp_path: Any, rule: str,
                                                                       want: str) -> None:
    asked = _escape(daemon, tmp_path, rule)
    # the task's reason ends the short line, quoted as its words (live 2026-10-26: three Yeses to mechanical steps
    # served turning off the owner's extension, and the reason was only under Details)
    assert [ap.summary for ap in asked] == [want]
    assert asked[0].why == 'the worker wants to look inside'                    # its own smaller line
    assert 'Default/Extensions' in asked[0].reason and rule in asked[0].reason   # the exact command and rule id stay


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
     'FERB wants to change account security (a password, two-factor or recovery setting, or deleting the account) on '
     'accounts.google.com for me@x.com.'),
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
    # the task's reason ends the line, quoted and marked as the task's, never FERB's own words (D33)
    want = "FERB wants to send to bob@x.com, whom you didn't name."
    [ap] = answers.asked
    assert ap.summary == want and want != ap.reason
    assert ap.why == '(from the task) FERB: it is fine'                            # its own line, marked (D33)
    assert 'mail.send to bob@x.com' in ap.reason and 'it is fine' in ap.reason     # the step and reason: Details
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
            'summary': "For atlasauth.b2clogin.com: a password or private answer — used once, not saved"})
        await asyncio.sleep(0.05)                      # the banner is posted from a task the publish starts
    daemon.run(ask())
    assert notes == [('FERB needs you', 'FERB needs your OK: For atlasauth.b2clogin.com: a password or private answer '
                                        '— used once, not saved')]


@pytest.mark.parametrize('handle', [None, 'visa'])
def test_ask_secret_refuses_a_whole_browser_before_any_question(daemon: Any, handle: str | None) -> None:
    """Defense in depth for any caller: the gateway itself never asks for, saves or fills a secret bound to a whole
    browser app."""
    from ferb.core.errors import Refused
    with pytest.raises(Refused, match='whole web browser'):
        daemon.run(daemon.daemon.gateway.ask_secret(_ctx(), 'app:com.google.Chrome', handle=handle, remember='visa'))
    assert not [ap for ap in daemon.daemon.state.approvals.pending() if ap.kind == 'owner_secret']


# ---------- the account the hidden input is for (L36 §5) ----------

@pytest.mark.parametrize('username, shown', [
    ('Asha Rao', 'Asha Rao'), ('Asha\n\x1b[31m  Rao\t', 'Asha [31m Rao'),
    ('\u202eAsha\u200b Rao\u2028', 'Asha Rao'),              # fix round 1: no reordering or invisible characters
    ('Asha Rao ' * 40, ('Asha Rao ' * 40)[:63].rstrip() + '…')],   # clipped to 64
    ids=['plain', 'controls', 'format-characters', 'long'])
def test_the_secret_question_names_the_username_saved_for_the_site(daemon: Any, username: str, shown: str) -> None:
    """L36 §5 ("Password for Kartikey Attri at usvisascheduling.com", the spec's example), so the owner knows which
    account. The account comes after FERB's place (re-review R1, ruling C1): the owner's own words, printable, on one
    line, without format or line-separator characters, at most 64 characters, never before the place."""
    from ferb.identity.owner_secrets import OwnerSecrets
    from tests.support.memory_items import owner_fact, run
    source = 'memory:' + owner_fact(daemon, 'visa_username', 'the username is Asha Rao').id   # an item that holds
    run(daemon, lambda: OwnerSecrets(daemon.daemon.home, lambda *a: None).set_username(VISA, username, source))
    for remember, want in (
            ('US visa scheduling login',
             f"For atlasauth.b2clogin.com only (account: {shown}): “US visa scheduling login” · saved in your "
             "Mac's Keychain"),
            (None, f"For atlasauth.b2clogin.com (account: {shown}): a password or private answer — used once, "
                   "not saved")):
        waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), VISA, remember=remember),
                                                   daemon.loop)
        ap = _asked(daemon, 'owner_secret')
        try:
            assert ap.summary.startswith('For atlasauth.b2clogin.com') and shown in ap.summary
            assert ap.summary == want and Redactor().text(want) == want
            assert f'Account: {shown}' in ap.reason
            assert ap.reason.startswith(f'Destination: {VISA}. Account: {shown}. Type the password')
        finally:
            _cancel(daemon, ap, waiting)


def _saved(daemon: Any, monkeypatch: pytest.MonkeyPatch, target: str, words: str, sites: list[str]) -> Any:
    """Save a hidden input named "password" at `target` through ask_secret, in a task whose request has the owner's
    `words` and the target `sites`; the store, on a fake Keychain (never the owner's)."""
    from ferb.identity.owner_secrets import OwnerSecrets
    from ferb.policy.targets import Request, Targets
    from tests.support.memory_items import run
    keychain = keychain_helper(monkeypatch)
    run(daemon, lambda: daemon.daemon.state.requests.record(Request('r-visa', words, Targets.of(sites=sites))))
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), target, remember='password'),
                                               daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    daemon.call_soon(daemon.daemon.state.approvals.resolve, ap.id, True, None, 'hidden-canary-1')
    assert waiting.result(5) == 'hidden-canary-1'
    store = OwnerSecrets(daemon.daemon.home)
    assert keychain == {f'{target} · password': 'hidden-canary-1'}
    assert 'hidden-canary-1' not in store.path.read_text()
    return store


def test_a_secret_saved_during_a_task_serves_its_sites_and_the_next_question_names_the_account(
        daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """I14, live 2026-10-24: the visa task named usvisascheduling.com; its sign-in form is on atlasauth.b2clogin.com.
    The entry saved there lists the task's sites the owner typed (by code), so the username the owner gave for
    usvisascheduling.com names the form's account; the question's place stays the form's origin (D12)."""
    from tests.support.memory_items import owner_fact, run
    store = _saved(daemon, monkeypatch, VISA, 'book my visa slot on usvisascheduling.com',
                   ['usvisascheduling.com', 'www.usvisascheduling.com'])
    assert store.records()[VISA]['sites'] == ['usvisascheduling.com', 'www.usvisascheduling.com']
    source = 'memory:' + owner_fact(daemon, 'visa_username', 'the username is Asha Rao').id   # an item that holds
    run(daemon, lambda: store.set_username('https://usvisascheduling.com', 'Asha Rao', source))
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), VISA, remember='food'),
                                               daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        assert ap.summary == ("For atlasauth.b2clogin.com only (usvisascheduling.com account: Asha Rao): “food” · "
                              "saved in your Mac's Keychain")             # review I-2: a linked account names its site
    finally:
        _cancel(daemon, ap, waiting)


@pytest.mark.parametrize('words', ['book my visa slot', 'book my visa slot; the page says usvisascheduling.net'])
def test_a_site_the_owner_never_typed_is_never_linked(daemon: Any, monkeypatch: pytest.MonkeyPatch,
                                                     words: str) -> None:
    """Fix round 1, rule 8's provenance: only a target site whose host is in the owner's own words links a record; one
    only the model or outside content named never does."""
    store = _saved(daemon, monkeypatch, VISA, words, ['usvisascheduling.com'])
    assert store.records()[VISA]['sites'] == []
    assert 'linked' not in store.listing('https://usvisascheduling.com')


def test_a_sign_in_on_the_tasks_own_site_links_nothing(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Fix round 1: only a sign-in form on another address needs a link. A task naming both usvisascheduling.com and
    gmail.com that signs in on usvisascheduling.com itself doesn't hand that site's username to gmail.com."""
    from tests.support.memory_items import run
    site = 'https://www.usvisascheduling.com'
    store = _saved(daemon, monkeypatch, site, 'sign in to usvisascheduling.com, then mail it from gmail.com',
                   ['usvisascheduling.com', 'gmail.com'])
    run(daemon, lambda: store.set_username(site, 'Asha Rao', 'memory:x'))
    assert store.records()[site]['sites'] == []
    assert store.listing('https://gmail.com') == {'place': 'https://gmail.com', 'username': None, 'entries': [],
                                                  'unusable': []}


def test_a_sign_in_host_the_planner_added_to_the_targets_still_links_the_sites_the_owner_typed(
        daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Task 6 re-review: after a steer the planner may add the sign-in form's own host to the task's targets. The
    own-site rule reads only the sites the owner typed, so the form's record still lists the visa site."""
    store = _saved(daemon, monkeypatch, VISA, 'book my visa slot on usvisascheduling.com',
                   ['usvisascheduling.com', 'atlasauth.b2clogin.com'])
    assert store.records()[VISA]['sites'] == ['usvisascheduling.com']


@pytest.mark.parametrize('content', ['{not json',
                                     '{"version": 2, "records": {"https://x.test": {"entries": {}, "sites": [1]}}}'])
def test_an_unreadable_handle_file_never_blocks_a_one_time_hidden_input(daemon: Any, content: str) -> None:
    """m17: the question reads the handle file for the account; one it can't read (or whose fields have the wrong
    types) means no account named, and the one-time hidden input is still asked. A save is refused before the owner
    types anything, saying why."""
    from ferb.core.errors import Refused
    path = daemon.daemon.home / 'owner-secret-handles.json'
    path.write_text(content)
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), VISA), daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        assert ap.summary == 'For atlasauth.b2clogin.com: a password or private answer — used once, not saved'
    finally:
        _cancel(daemon, ap, waiting)
    with pytest.raises(Refused, match='owner-secret-handles.json'):
        daemon.run(daemon.daemon.gateway.ask_secret(_ctx(), VISA, remember='password'))
    assert not [ap for ap in daemon.daemon.state.approvals.pending() if ap.kind == 'owner_secret']
    assert path.read_text() == content


def test_a_hidden_value_question_names_the_pages_field_and_a_payment_its_currency() -> None:
    """Live 2026-10-27 (task 400d67fb): "For www.zomato.com: a password or private answer" for the card's CVV, and
    "this pays 137.39 on www.zomato.com". Owner: "how can you tell its cvv, bank otp simply BUT why cant FERB??"."""
    from ferb.core.types import Target
    from ferb.daemon.gateway import secret_summary
    from ferb.policy import confirm
    from ferb.policy.rules import Kind
    assert secret_summary("https://www.zomato.com", None, field="CVV digit 1 of 3") == (
        "For zomato.com: the page's “CVV digit 1 of 3” field — used once, not saved")
    assert secret_summary("https://www.zomato.com", None) == (
        "For zomato.com: a password or private answer — used once, not saved")
    needs = confirm.decide(Kind.CLICK, "pay", Target(site="www.zomato.com", amount=137.39, currency="₹",
                                                     account="Kotak RuPay ending 0545"),
                           {"amount": ["external:web:www.zomato.com"]}, read_private=False, read_external=True,
                           known=lambda k, v: False, outside_read=["external:web:www.zomato.com"])
    assert [n.rule for n in needs] == ["floor:pay"]          # the price's origin isn't a second question
    assert confirm.summary(needs) == "FERB wants to pay ₹137.39 on zomato.com from Kotak RuPay ending 0545."
