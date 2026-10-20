"""L18: only values entered through the sealed secret input become hidden secrets (D28). An answer to a question with
options never is, so a 2FA method picked as "1" can't turn every "1" in later traces, model context and answers into
[redacted] (code-f5-f6 I2)."""
import pytest

from ferb.core.types import Identity
from ferb.policy.rules import ActionRequest
from ferb.sanitize.redact import REDACTED

LATER = 'freed 1.5 GB in 11 files'
TWO_FA = 'Which 2FA method should I use: 1 (SMS) or 2 (authenticator app)?'


def ask(daemon, question: str, options: list[str], kind: str = 'owner_question'):
    action = ActionRequest('owner.ask', kind, Identity.OWNER, frozenset(), 't', 'w', 'r', frozenset())
    ap = daemon.daemon.state.approvals.question(action, question, options)
    ap.kind = kind
    return ap


def answer(daemon, ap, text: str) -> None:
    daemon.run(daemon.daemon._chat_answer(None, {'approval_id': ap.id, 'text': text}))


@pytest.mark.parametrize('typed,choice', [('1', 'SMS'), ('2', 'Authenticator app'), ('sms', 'SMS')])
def test_a_2fa_choice_answer_is_kept_and_never_becomes_a_secret(daemon, typed, choice) -> None:
    """(c) The pick reaches the task as typed, and later text passes through redaction unchanged."""
    ap = ask(daemon, TWO_FA, ['SMS', 'Authenticator app'])
    answer(daemon, ap, typed)
    assert (ap.choice, ap.answer) == (choice, typed)
    assert daemon.daemon.redact.text(LATER) == LATER
    assert daemon.daemon.redact({'answer': LATER, 'note': 'step 1 of 2'}) == {'answer': LATER, 'note': 'step 1 of 2'}


def test_a_typed_answer_is_never_remembered_as_a_secret(daemon) -> None:
    """A free-text answer to a code-labelled question is hidden for that one answer only (rule 14 at capture); it is
    not entered through the sealed input, so it never becomes a hidden secret that rewrites later text."""
    ap = ask(daemon, TWO_FA, [])
    answer(daemon, ap, '1')
    assert ap.answer == REDACTED
    assert daemon.daemon.redact.text(LATER) == LATER


def test_values_entered_through_the_sealed_input_stay_redacted_in_later_text(daemon) -> None:
    """(d) The sealed paths still hide what the owner entered: a login code and a short secret, before and after the
    fill hands the secret back (gateway.release_secret), and a later choice answer leaves them hidden."""
    code = ask(daemon, 'Enter the login code for https://www.zomato.com (hidden; Enter cancels).', [],
               kind='owner_login_code')
    answer(daemon, code, '482913')
    secret = ask(daemon, 'Enter the secret for https://x.test (hidden; Enter cancels).', [], kind='owner_secret')
    answer(daemon, secret, 'pa5')
    redact = daemon.daemon.redact
    assert redact.text('the page shows 482913 and pa5') == f'the page shows {REDACTED} and {REDACTED}'
    daemon.daemon.gateway.release_secret('pa5')          # the sealed fill is done: the value stays hidden
    assert redact.text('error: pa5 rejected') == f'error: {REDACTED} rejected'

    answer(daemon, ask(daemon, TWO_FA, ['SMS', 'Authenticator app']), '1')
    assert redact.text('482913, pa5; ' + LATER) == f'{REDACTED}, {REDACTED}; {LATER}'


def test_a_typed_superscript_digit_is_no_option_number(daemon) -> None:
    """'²' is a digit but not a decimal number: it picks nothing and is kept as typed (int('²') would raise)."""
    ap = ask(daemon, TWO_FA, ['SMS', 'Authenticator app'])
    answer(daemon, ap, '²')
    assert (ap.choice, ap.answer) == (None, '²')


def test_a_chat_reply_to_an_open_code_question_reaches_the_planner_with_its_code_masked(daemon) -> None:
    """Re-review 2 (R1): the chat path masks the code in a reply ("4829, then book it") instead of passing it whole,
    and hides a one-word reply to a password question; a longer message keeps its words (it may be a steer)."""
    ask(daemon, "What's the code on the Uber screen?", [])
    assert daemon.daemon._hide_code_reply("4829, then book it") == f"{REDACTED}, then book it"
    ask(daemon, "What's your Gmail password?", [])
    assert daemon.daemon._hide_code_reply("hunter2!") == REDACTED
    assert daemon.daemon._hide_code_reply("use the work account") == "use the work account"
