"""FERB's inbox (section 3 identity/mailbox.py): read one-time codes. Block 1 reads only.

Source: Gmail's Atom feed of unread inbox mail (`/mail/feed/atom`), fetched inside FERB's identity browser with the
seeded session — no extra credentials. The code goes straight from here into an origin-checked field
(`browser.fill_otp`); it's never returned to a model, logged or stored (section 21 test 5). Email is untrusted:
only a digit code is taken from it, and only when exactly one candidate is present.
"""

import asyncio
import re
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from datetime import datetime
from typing import Awaitable, Callable

# What a worker is told when no code was filled (never the code itself).
REASONS = {"no_new_message": "no new code arrived in FERB's inbox",
           "no_code": "the newest email has no code in it",
           "ambiguous": "the newest email has more than one code; refusing to guess",
           "not_signed_in": "FERB's inbox is signed out"}

FEED_URL = "https://mail.google.com/mail/feed/atom"
NS = {"a": "http://purl.org/atom/ns#"}
_CODE = re.compile(r"(?<![\w-])(?:G-)?(\d{4,8})(?![\w-])")
_YEAR = re.compile(r"^(19|20)\d{2}$")


@dataclass(frozen=True)
class Message:
    id: str
    sender: str
    subject: str
    summary: str
    issued: datetime


@dataclass(frozen=True)
class CodeResult:
    code: str | None
    message_id: str | None
    reason: str                                  # "ok" | "no_new_message" | "no_code" | "ambiguous" | ...


def parse_feed(xml_text: str) -> tuple[str, list[Message]]:
    """-> (feed title, messages). Raises ValueError when it isn't an inbox feed (e.g. a sign-in page)."""
    start = xml_text.find("<feed")
    if start < 0:
        raise ValueError("not an Atom feed (signed out?)")
    root = ET.fromstring(xml_text[start:xml_text.rfind("</feed>") + len("</feed>")])
    title = root.findtext("a:title", "", NS)
    msgs = []
    for e in root.findall("a:entry", NS):
        issued = e.findtext("a:issued", "", NS) or e.findtext("a:modified", "", NS)
        msgs.append(Message(id=e.findtext("a:id", "", NS), sender=e.findtext("a:author/a:email", "", NS).lower(),
                            subject=e.findtext("a:title", "", NS), summary=e.findtext("a:summary", "", NS),
                            issued=datetime.fromisoformat(issued.replace("Z", "+00:00"))))
    return title, msgs


def extract_code(msg: Message) -> str | None | bool:
    """One code -> str; none -> None; several different ones -> False (ambiguous)."""
    found = {m.group(1) for m in _CODE.finditer(f"{msg.subject}\n{msg.summary}") if not _YEAR.match(m.group(1))}
    if not found:
        return None
    return found.pop() if len(found) == 1 else False


class GmailFeedMailbox:
    def __init__(self, fetch: Callable[[str], Awaitable[str]], address: str, feed_url: str = FEED_URL) -> None:
        self.fetch, self.address, self.feed_url = fetch, address.lower(), feed_url
        self.used: set[str] = set()              # message ids whose code was already filled (memory only)

    async def _feed(self) -> tuple[str, list[Message]] | None:
        """None when the feed can't be read as FERB's inbox (signed out: 401, a sign-in page, bad XML)."""
        try:
            return parse_feed(await self.fetch(self.feed_url))
        except Exception:                        # load refused (e.g. 401) or not a feed: treat as signed out
            return None

    async def signed_in(self) -> bool:
        feed = await self._feed()
        return feed is not None and self.address in feed[0].lower()

    async def find_code(self, since: datetime, sender_contains: str | None = None) -> CodeResult:
        feed = await self._feed()
        if feed is None:
            return CodeResult(None, None, "not_signed_in")
        title, msgs = feed
        if self.address not in title.lower():
            return CodeResult(None, None, "not_signed_in")
        fresh = [m for m in msgs if m.issued >= since and m.id not in self.used
                 and (not sender_contains or sender_contains.lower() in m.sender)]
        if not fresh:
            return CodeResult(None, None, "no_new_message")
        newest = max(fresh, key=lambda m: m.issued)
        code = extract_code(newest)
        if code is None:
            return CodeResult(None, newest.id, "no_code")
        if code is False:
            return CodeResult(None, newest.id, "ambiguous")
        assert isinstance(code, str)
        return CodeResult(code, newest.id, "ok")


async def wait_for_code(mailbox: GmailFeedMailbox, since: datetime, sender_contains: str | None,
                        timeout: float = 120.0, poll: float = 5.0) -> CodeResult:
    """Poll the inbox until a fresh code arrives; stop early on a signed-out inbox or an ambiguous message."""
    loop = asyncio.get_running_loop()
    end = loop.time() + timeout
    while True:
        found = await mailbox.find_code(since, sender_contains)
        if found.code or found.reason in ("ambiguous", "not_signed_in") or loop.time() >= end:
            return found
        await asyncio.sleep(poll)
