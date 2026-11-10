"""P5 secret classes at capture; ordinary private text remains usable."""
from ferb.sanitize.redact import REDACTED, Redactor
import pytest


def test_otp_fields_secure_input_and_concealed_clipboard() -> None:
    r = Redactor()
    assert r.field('314159', label='verification code') == REDACTED
    assert r.field('private', role='AXSecureTextField') == REDACTED
    assert r.field('private', secure_input=True) == REDACTED
    assert r.field('private', concealed=True) == REDACTED
    assert r.field('314159', label='invoice number') == '314159'
    assert r.text('Your verification code is 314159.') == 'Your verification code is [redacted].'
    assert r.text('The year is 2026 and invoice 314159 is paid.') == 'The year is 2026 and invoice 314159 is paid.'


def test_payment_card_checksum_and_contextual_credentials() -> None:
    r = Redactor()
    assert r.text('Card 4111 1111 1111 1111') == 'Card [redacted]'
    assert r.text('4111 1111 1111 1112') == '4111 1111 1111 1112'
    assert r({'role': 'AXSecureTextField', 'value': 'short'})['value'] == REDACTED
    assert r({'label': 'one-time code', 'value': '123456'})['value'] == REDACTED


def test_split_stream_card_and_otp_remain_secret() -> None:
    r = Redactor()
    s = r.stream()
    out = s.feed(b'Your verification code is 123') + s.feed(b'456.\nCard 4111 1111 ') + s.feed(b'1111 1111\n', final=True)
    assert b'123456' not in out and b'4111 1111' not in out


@pytest.mark.parametrize('label,value', [
    ('Which phone number should I use for login?', '2025550143'),
    ('Which accessibility option should I use?', '1'),
    ('Which security setting should I select?', '2'),
    ('Which account should get access?', '3'),
])
def test_nonsecret_numeric_answers_remain_available(label, value):
    assert Redactor().field(value, label=label) == value


@pytest.mark.parametrize('label', ['OTP', 'TOTP', '2FA', 'verification code', 'authentication code',
                                  'security code', 'login code', 'sign-in code', 'access code',
                                  'one-time-code', 'one time password', 'verification_code'])
def test_explicit_code_labels_still_hide_codes(label):
    assert Redactor().field('314159', label=label) == REDACTED


def test_stream_keeps_code_context_across_the_label_boundary():
    stream = Redactor().stream()
    out = stream.feed(b'Your verification ') + stream.feed(b'code is 314') + stream.feed(b'159.\n', final=True)
    assert b'314159' not in out and b'[redacted]' in out


def test_ordinary_login_phone_answer_is_not_registered_as_secret(daemon):
    from ferb.core.types import Identity
    from ferb.policy.rules import ActionRequest
    async def answer():
        action = ActionRequest('owner.ask', 'owner_question', Identity.OWNER, frozenset(), 't', 'w', 'r', frozenset())
        ap = daemon.daemon.state.approvals.question(action, 'Which phone number should I use for login?', [])
        await daemon.daemon._chat_answer(None, {'approval_id': ap.id, 'text': '2025550143'})
        return ap.answer, daemon.daemon.redact.text('phone 2025550143')
    answer, echo = daemon.run(answer())
    assert answer == '2025550143' and echo == 'phone 2025550143'


def test_ordinary_owner_answer_to_otp_question_is_scrubbed_before_resolution(daemon) -> None:
    from ferb.core.types import Identity
    from ferb.policy.rules import ActionRequest
    async def answer():
        action = ActionRequest('owner.ask', 'owner_question', Identity.OWNER, frozenset(), 't', 'w', 'r', frozenset())
        ap = daemon.daemon.state.approvals.question(action, 'Please enter the OTP on the Orderly page.', ['Done', 'Cancel'])
        await daemon.daemon._chat_answer(None, {'approval_id': ap.id, 'text': '314159'})
        return ap.answer
    assert daemon.run(answer()) == REDACTED      # hidden for this answer, never remembered (L18)


def test_admin_answer_preserves_spaces_and_rejects_newline_privately(daemon) -> None:
    import pytest
    from ferb.core.errors import FerbError
    from ferb.core.types import Identity
    from ferb.policy.rules import ActionRequest
    async def answer():
        action = ActionRequest('shell.run', 'owner_admin_password', Identity.OWNER, frozenset(), 't', 'w', 'r', frozenset())
        ap = daemon.daemon.state.approvals.question(action, 'Administrator password', [])
        ap.kind = 'owner_admin_password'
        with pytest.raises(FerbError, match='one line'):
            await daemon.daemon._chat_answer(None, {'approval_id': ap.id, 'text': 'do-not-echo\n'})
        await daemon.daemon._chat_answer(None, {'approval_id': ap.id, 'text': ' x '})
        return ap.answer, daemon.daemon.redact.text('input: x ')
    value, echo = daemon.run(answer())
    assert value == ' x ' and echo == 'input: x '        # never known by value (owner, GATE-572)
