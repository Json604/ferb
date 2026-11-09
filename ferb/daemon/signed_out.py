"""Own the signed-out view of a page (GATE-568, GATE-569): what a page FERB read in the owner's signed-in browser shows
to anyone, without the owner's sign-in. Must not judge what a page is about: it only fetches the same address with no
cookies or credentials and returns its text, or None when it can't tell.

Asked only by the URL sink, only when it would otherwise ask the owner, only for the page a value of the owner's was
read on. The address goes to the host that already served it to the owner's browser, so nothing new leaves. Fails
closed: anything other than one plain public HTTPS answer of text (a redirect, a sign-in wall, an error, a private or
local address, a page too big or too slow) returns None, and the value stays the owner's."""

import asyncio
import ipaddress
import socket

import httpx

from ferb.policy import urls

TIMEOUT_S = 5.0                     # one plain GET; a slower page stays the owner's
MAX_BYTES = 2_000_000               # a page's text past this isn't read; the value stays the owner's
_TEXT = ("text/", "application/xhtml", "application/json", "application/xml")
_AGENT = ("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 "
          "Safari/605.1.15")


def _public(host: str, port: int) -> bool:
    """Whether every address the host resolves to is a public one (never this Mac, its network or a link-local one)."""
    try:
        found = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM)
    except (OSError, UnicodeError):
        return False
    addresses = {str(info[4][0]) for info in found}
    return bool(addresses) and all(ipaddress.ip_address(a.split("%", 1)[0]).is_global for a in addresses)


async def fetch(url: str) -> str | None:
    """The page at `url` as anyone gets it (no cookies, no credentials, no redirect followed), its text; None when
    FERB can't tell (not HTTPS, not a public host, not a 200 answer of text, too slow, too big)."""
    parsed = urls.parse(url)
    if parsed is None or parsed.scheme != "https" or not parsed.host or parsed.username or parsed.password:
        return None
    if not await asyncio.to_thread(_public, parsed.host, int(parsed.port or 443)):
        return None
    address = url.split("#", 1)[0]                     # a fragment never leaves a browser either
    try:
        async with httpx.AsyncClient(follow_redirects=False, timeout=TIMEOUT_S, trust_env=False,
                                     headers={"User-Agent": _AGENT, "Accept": "text/html,*/*;q=0.5"}) as client:
            async with client.stream("GET", address) as answer:
                if answer.status_code != 200 or not answer.headers.get("content-type", "").startswith(_TEXT):
                    return None
                body = bytearray()
                async for chunk in answer.aiter_bytes():
                    body += chunk
                    if len(body) > MAX_BYTES:
                        return None
                return body.decode(answer.encoding or "utf-8", "replace")
    except (httpx.HTTPError, OSError, UnicodeError, ValueError, LookupError):
        return None
