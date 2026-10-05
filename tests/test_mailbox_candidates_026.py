"""OTP discovery exposes every opaque candidate; no newest-message or year heuristic chooses a code."""
import asyncio
import json
from datetime import datetime, timedelta, timezone
import pytest
from ferb.identity.mailbox import GmailFeedMailbox
from tests.browserkit import ADDRESS, feed


def test_code_candidates_preserve_older_message_and_year_shaped_code_without_disclosing_digits():
    now=datetime.now(timezone.utc).replace(microsecond=0)
    async def fetch(url):
        return feed([('service@example.test','Your login','Code 2026',now-timedelta(minutes=1)),
                     ('service@example.test','Another login','Code 654321 or 123456',now)])
    mailbox=GmailFeedMailbox(fetch,ADDRESS)
    result=asyncio.run(mailbox.candidates())
    assert len(result['candidates']) == 3
    assert {c['issued'] for c in result['candidates']} == {now.isoformat(),(now-timedelta(minutes=1)).isoformat()}
    for candidate in result['candidates']:
        assert 'subject' not in candidate and 'context' not in candidate
    assert {mailbox.take(c['candidate']).code for c in result['candidates']} == {'2026','654321','123456'}


def test_candidate_inventory_withholds_unrecognized_secrets_in_subject_and_body():
    now = datetime(2026, 10, 5, tzinfo=timezone.utc)

    async def fetch(url):
        return feed([('service@example.test', 'Your one-time code is AB12CD',
                      'Use EF34GH to sign in. Copyright 2026.', now)])

    mailbox = GmailFeedMailbox(fetch, ADDRESS)
    result = asyncio.run(mailbox.candidates())

    assert len(result['candidates']) == 1
    visible = json.dumps(result)
    assert 'AB12CD' not in visible and 'EF34GH' not in visible
    candidate = result['candidates'][0]
    assert not {'subject', 'summary', 'body', 'context'} & candidate.keys()
    assert candidate['sender'] == 'service@example.test'
    assert candidate['issued'] == now.isoformat()
    assert candidate['account'] == ADDRESS
    assert candidate['message_id'] and candidate['source'] and candidate['marker']
    assert mailbox.take(candidate['candidate']).code == '2026'
    assert 'withheld' in result['coverage'] and 'unsupported' in result['coverage']


def test_mailbox_transport_failure_is_not_misreported_as_signed_out():
    async def fetch(url): raise ConnectionError('provider unavailable')
    with pytest.raises(ConnectionError,match='provider unavailable'):
        asyncio.run(GmailFeedMailbox(fetch,ADDRESS).candidates())
