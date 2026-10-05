"""Owns sealed unread-mail candidate inventory; must never choose an account, message or code for an agent.

Gmail Atom exposes unread inbox entries only. Numeric strings are opaque candidates, not inferred OTPs; the agent
judges sender, time and message identity. Subject/body stay sealed because unsupported secret formats cannot be
safely masked by the numeric candidate grammar. Alphanumeric formats need an explicitly chosen alternative route.
Transport failures propagate distinctly from an unavailable or wrong-account feed.
"""
import re
import secrets
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from datetime import datetime
from typing import Any, Awaitable, Callable

FEED_URL = "https://mail.google.com/mail/feed/atom"
NS = {"a": "http://purl.org/atom/ns#"}
_CODE = re.compile(r"(?<![\w-])(?:G-)?(\d{4,})(?![\w-])")
_EMAIL = re.compile(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+")

@dataclass(frozen=True)
class Message:
    id: str
    sender: str
    subject: str
    summary: str
    issued: datetime

@dataclass(frozen=True)
class CodeCandidate:
    code: str
    message_id: str

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



class GmailFeedMailbox:
    def __init__(self, fetch: Callable[[str], Awaitable[str]], address: str, feed_url: str = FEED_URL) -> None:
        self.fetch, self.address, self.feed_url = fetch, address.lower(), feed_url
        self.used: set[str] = set()
        self._candidates: dict[str, CodeCandidate] = {}

    async def _feed(self) -> tuple[str, list[Message]] | None:
        text = await self.fetch(self.feed_url)
        try:
            return parse_feed(text)
        except (ValueError, ET.ParseError):
            return None

    def _account_matches(self, title: str) -> bool:
        # Exact address equality avoids accepting another inbox merely containing FERB's address.
        return set(e.lower() for e in _EMAIL.findall(title)) == {self.address}

    async def signed_in(self) -> bool:
        feed = await self._feed()
        return feed is not None and self._account_matches(feed[0])

    async def candidates(self) -> dict[str, Any]:
        feed = await self._feed()
        self._candidates.clear()
        if feed is None:
            return {"available":False,"reason":"feed_unavailable","candidates":[]}
        title, messages = feed
        if not self._account_matches(title):
            return {"available":False,"reason":"account_mismatch","expected_account":self.address,
                    "observed_accounts":sorted(set(_EMAIL.findall(title))),"candidates":[]}
        out = []
        for msg in messages:
            if msg.id in self.used:
                continue
            content = f"{msg.subject}\n{msg.summary}"
            codes = list(dict.fromkeys(m.group(1) for m in _CODE.finditer(content)))
            labels = {code:f"[candidate {i+1}: {len(code)} digits]" for i,code in enumerate(codes)}
            for code in codes:
                token = secrets.token_urlsafe(18)
                self._candidates[token] = CodeCandidate(code,msg.id)
                out.append({"candidate":token,"message_id":msg.id,"source":self.feed_url,"account":self.address,
                            "sender":msg.sender,"issued":msg.issued.isoformat(),"marker":labels[code]})
        return {"available":True,"candidates":out,"coverage":
                "Unread inbox entries only; numeric candidates of at least four digits, not verified OTPs. "
                "Subject and body are withheld to keep unsupported secret formats sealed. Alphanumeric and shorter "
                "codes are unsupported by this inventory. No candidate is selected automatically; insufficient "
                "identity evidence requires asking the owner or choosing another explicit route."}

    def take(self, token: str) -> CodeCandidate:
        candidate = self._candidates.pop(token, None)
        if candidate is None or candidate.message_id in self.used:
            raise ValueError("OTP candidate is unavailable, consumed or belongs to another inventory")
        return candidate
