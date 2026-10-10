"""Owns parsing a URL the way a browser does for special schemes (the WHATWG URL rules Chrome follows), so the host FERB
checks is the host the browser opens (S4). Must not judge content or intent: shapes only.

For a special scheme (http, https, ws, wss, ftp, file): leading and trailing C0 controls and spaces are stripped, tabs
and newlines are removed anywhere, `\\` is `/`, every leading `/` or `\\` after the scheme is skipped, userinfo ends
at the last `@`, and the host is percent-decoded, IDNA-normalized and lowercased. A host with a forbidden code point,
or an invalid port, makes the URL unparseable (None). Other schemes fall back to `urlsplit`."""

import ipaddress
import re
from dataclasses import dataclass
from urllib.parse import unquote, urlsplit

SPECIAL = frozenset({"http", "https", "ws", "wss", "ftp", "file"})
_SCHEME = re.compile(r"[A-Za-z][A-Za-z0-9+.-]*")
_C0_SPACE = "".join(chr(i) for i in range(0x21))         # C0 controls and space
_FORBIDDEN_HOST = frozenset(" #%/:<>?@[\\]^|")           # forbidden host code points (after percent-decoding)


@dataclass(frozen=True)
class URL:
    scheme: str
    username: str
    password: str
    host: str                  # "" when the URL names no host
    port: str
    path: str
    query: str
    fragment: str


def _host(raw: str) -> str | None:
    """A host as a browser normalizes it: an IPv6 literal (brackets dropped), else percent-decoded, IDNA-encoded and
    lowercased; None when it has a forbidden code point or can't be encoded."""
    if raw.startswith("["):
        try:
            return str(ipaddress.IPv6Address(raw[1:-1])) if raw.endswith("]") else None
        except ValueError:
            return None
    host = unquote(raw)
    if any(c in _FORBIDDEN_HOST or ord(c) < 0x20 for c in host):
        return None
    try:
        return host.encode("idna").decode("ascii").lower()
    except UnicodeError:
        return host.lower() if host.isascii() else None


def parse(url: str) -> URL | None:
    """The URL's parts as a browser reads them, or None when a browser couldn't parse it."""
    text = "".join(c for c in url if c not in "\t\n\r").strip(_C0_SPACE)
    scheme, sep, rest = text.partition(":")
    if not sep or not _SCHEME.fullmatch(scheme):
        return None
    scheme = scheme.lower()
    if scheme not in SPECIAL:
        try:
            p = urlsplit(text)
            return URL(scheme, p.username or "", p.password or "", (p.hostname or "").lower(),
                       str(p.port or ""), p.path, p.query, p.fragment)
        except ValueError:
            return None
    rest = rest.replace("\\", "/").lstrip("/")            # skip every leading / or \ after the scheme
    cut = re.search(r"[/?#]", rest)
    authority, tail = (rest[:cut.start()], rest[cut.start():]) if cut else (rest, "")
    userinfo, at, hostport = authority.rpartition("@")    # userinfo ends at the last @
    if not at:
        userinfo, hostport = "", authority
    user, _, password = userinfo.partition(":")
    if hostport.startswith("["):
        end = hostport.find("]")
        raw_host, after = (hostport[:end + 1], hostport[end + 1:]) if end != -1 else (hostport, "")
        if after and not after.startswith(":"):
            return None
        port = after[1:]
    else:
        raw_host, _, port = hostport.partition(":")
    if port and not port.isdigit():
        return None
    host = _host(raw_host) if raw_host else ""
    if host is None:
        return None
    path, _, fragment = tail.partition("#")
    path, _, query = path.partition("?")
    return URL(scheme, user, password, host, port, path, query, fragment)
