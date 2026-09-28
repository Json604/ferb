"""FERB's inbox (section 10.1): one fresh code, only when it's unambiguous, used once; a signed-out inbox is a
reported state, not an exception."""

import asyncio
from datetime import datetime, timedelta, timezone

import pytest

from ferb.identity.mailbox import GmailFeedMailbox, Message, extract_code, parse_feed, wait_for_code
from tests.browserkit import ADDRESS, feed


def mailbox(xml: str | Exception) -> GmailFeedMailbox:
    async def fetch(url: str) -> str:
        if isinstance(xml, Exception):
            raise xml
        return xml
    return GmailFeedMailbox(fetch, ADDRESS)


def test_code_extraction_and_feed_parsing() -> None:
    now = datetime.now(timezone.utc)
    msg = lambda s: Message("i", "a@b", "subj", s, now)             # noqa: E731
    assert extract_code(msg("Your code is 482913")) == "482913"
    assert extract_code(msg("G-482913 is your Google verification code")) == "482913"
    assert extract_code(msg("Welcome! Copyright 2026")) is None     # years aren't codes
    assert extract_code(msg("123456 or 654321")) is False
    title, msgs = parse_feed(feed([("A@B.example", "t", "s", now)]))
    assert ADDRESS in title and msgs[0].sender == "a@b.example"
    with pytest.raises(ValueError):
        parse_feed("<html>Sign in</html>")


def test_newest_fresh_code_once_and_ambiguity_refused() -> None:
    now = datetime.now(timezone.utc)
    m = mailbox(feed([("noreply@site.example", "Your code", "Code 482913, valid until 2026", now),
                      ("old@site.example", "Old", "Code 111111", now - timedelta(hours=2))]))
    found = asyncio.run(m.find_code(now - timedelta(minutes=1), "site.example"))
    assert found.code == "482913" and found.reason == "ok"
    m.used.add(found.message_id or "")
    assert asyncio.run(m.find_code(now - timedelta(minutes=1), None)).reason == "no_new_message"
    amb = mailbox(feed([("x@site.example", "Codes", "Use 123456 or 654321", now)]))
    assert asyncio.run(wait_for_code(amb, now - timedelta(minutes=1), None, timeout=5, poll=1)).reason == "ambiguous"


def test_signed_out_inbox_is_reported_not_raised() -> None:
    now = datetime.now(timezone.utc)
    for bad in (ConnectionError("401"), "<html>Sign in</html>", feed([], address="someone@else.example")):
        m = mailbox(bad)
        assert asyncio.run(m.signed_in()) is False
        assert asyncio.run(wait_for_code(m, now, None, timeout=5, poll=1)).reason == "not_signed_in"
