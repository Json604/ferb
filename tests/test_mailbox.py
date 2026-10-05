"""The sealed mailbox parser preserves message evidence and distinguishes wrong-account feeds."""
import asyncio
from datetime import datetime, timezone
import pytest
from ferb.identity.mailbox import GmailFeedMailbox, parse_feed
from tests.browserkit import ADDRESS, feed


def test_feed_parses_every_message_and_preserves_sender():
    now=datetime.now(timezone.utc)
    title,msgs=parse_feed(feed([("A@B.example","t","s",now)]))
    assert ADDRESS in title and msgs[0].sender == "a@b.example"
    with pytest.raises(ValueError): parse_feed("<html>Sign in</html>")


@pytest.mark.parametrize("xml",["<html>Sign in</html>",feed([],address="someone@else.example")])
def test_unavailable_or_other_account_is_not_accepted(xml):
    async def fetch(url): return xml
    mailbox=GmailFeedMailbox(fetch,ADDRESS)
    assert not asyncio.run(mailbox.signed_in())
    assert not asyncio.run(mailbox.candidates())["available"]
