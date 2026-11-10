"""L18: an answer to a question with options never becomes a hidden secret, so a 2FA method picked as "1" can't turn
every "1" in later traces, model context and answers into [redacted] (code-f5-f6 I2). Since GATE-572 (owner,
2026-11-10) no value the owner enters, through the hidden input or not, is hidden by its value in later text."""
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


# decision: GATE-572
def test_values_entered_through_the_hidden_input_are_not_hidden_by_value_in_later_text(daemon) -> None:
    """(d) Owner 2026-11-10: a login code and a one-time secret typed into the hidden input reach their fill only, and
    later text that happens to hold the same characters is left as it is; only FERB's own keys are known by value."""
    code = ask(daemon, 'Enter the login code for https://www.orderly.example (hidden; Enter cancels).', [],
               kind='owner_login_code')
    answer(daemon, code, '482913')
    secret = ask(daemon, 'Enter the secret for https://x.test (hidden; Enter cancels).', [], kind='owner_secret')
    answer(daemon, secret, 'booked')
    assert (code.answer, secret.answer) == ('482913', 'booked')       # the fills get them as typed
    redact = daemon.daemon.redact
    assert redact.text('I booked the flight') == 'I booked the flight'
    assert redact.text('order 482913 shipped') == 'order 482913 shipped'
    assert redact.secret_values() == frozenset()


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


def test_a_reply_to_the_planners_own_password_question_is_hidden(daemon) -> None:
    """Re-review 3 (R1-b): the planner's clarifying question (the conversation's `asked`) gets the same rule as a
    worker's: a password typed in reply never reaches the planner; a code in reply to a code question is masked."""
    d = daemon.daemon

    async def reply(asked: str, text: str) -> str:
        d.supervisor.continuity.ensure("chat-r1b")
        d.supervisor.continuity.save_pending("chat-r1b", "connect the TV to the wifi", asked)
        return d._hide_code_reply(text, "chat-r1b")
    assert daemon.run(reply("What's the Wi-Fi password for the router?", "hunter2!")) == REDACTED
    assert daemon.run(reply("What's the code on the TV screen?", "4829, then pair it")) == f"{REDACTED}, then pair it"
    assert daemon.run(reply("Which room is the TV in?", "the bedroom")) == "the bedroom"


def test_stop_and_cancel_work_while_a_password_question_is_open(daemon) -> None:
    """Re-review 3 (R1-c): a typed one-word reply to a password question is hidden from the model, so stopping is the
    code's: Esc / Ctrl-C / the notch's stop (task.stop) stops the task, and the question's Cancel declines it."""
    from ferb.core.types import Task
    from ferb.orchestrator.schemas import TaskSpec
    from ferb.orchestrator.supervisor import _Tree
    d = daemon.daemon
    ap = ask(daemon, "What's the password for the router?", [])
    assert d._hide_code_reply("stop") == REDACTED                     # never shown to the planner
    worker = Task("tree-r1c.worker", "tree-r1c", "Connect the TV", "device", Identity.OWNER, set(), "worker",
                  "req-r1c", [], None)
    tree = _Tree("tree-r1c", TaskSpec(goal="Connect the TV", identity="owner"), worker)

    async def stop() -> dict:
        d.supervisor._tree_of[tree.id] = tree
        try:
            return await d._task_stop(None, {"task_ids": [tree.id]})
        finally:
            d.supervisor._tree_of.pop(tree.id, None)
    assert daemon.run(stop())["stopped"] == ["tree-r1c"] and tree.stop
    daemon.run(d._chat_approve(None, {"approval_id": ap.id, "granted": False}))
    assert ap.state != "pending"
