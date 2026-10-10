"""P5 secret classes at capture; ordinary private text remains usable."""
from ferb.sanitize.redact import REDACTED, Redactor


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


def test_ordinary_owner_answer_to_otp_question_is_scrubbed_before_resolution(daemon) -> None:
    from ferb.core.types import Identity
    from ferb.policy.rules import ActionRequest
    async def answer():
        action = ActionRequest('owner.ask', 'owner_question', Identity.OWNER, frozenset(), 't', 'w', 'r', frozenset())
        ap = daemon.daemon.state.approvals.question(action, 'Please enter the OTP on the Zomato page.', ['Done', 'Cancel'])
        await daemon.daemon._chat_answer(None, {'approval_id': ap.id, 'text': '314159'})
        return ap.answer, daemon.daemon.redact.text('page echoed 314159')
    answer, echo = daemon.run(answer())
    assert answer == REDACTED and '314159' not in echo


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
    assert value == ' x ' and echo == 'input:[redacted]'
