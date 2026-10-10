"""Owns parsing a URL the way a browser does for special schemes (the WHATWG URL rules Chrome follows), so the host FERB
checks is the host the browser opens (S4). Must not judge content or intent: shapes only.

For a special scheme (http, https, ws, wss, ftp, file): leading and trailing C0 controls and spaces are stripped, tabs
and newlines are removed anywhere, `\\` is `/` in the authority and the path (it stays literal in the query and the
fragment), every leading `/` or `\\` after the scheme is skipped (`file:` keeps a host only after exactly `//`),
userinfo ends at the last `@`, and the host is percent-decoded, mapped and encoded as UTS46 non-transitional IDNA
(ß and ς stay themselves, as in Chrome; a joiner out of context or a fullwidth slash makes it unparseable), lowercased,
and a host that ends in a number is an IPv4 address in every form a browser reads (hex, octal, one integer, short
forms) and comes out dotted. A host with a forbidden code point (DEL included) or an invalid port (above 65535) makes
the URL unparseable (None). Other schemes fall back to `urlsplit`."""

import ipaddress
import re
from dataclasses import dataclass
from urllib.parse import unquote, urlsplit

import idna

SPECIAL = frozenset({"http", "https", "ws", "wss", "ftp", "file"})
MAX_PORT = 65535
_SCHEME = re.compile(r"[A-Za-z][A-Za-z0-9+.-]*")
_C0_SPACE = "".join(chr(i) for i in range(0x21))         # C0 controls and space
_FORBIDDEN_HOST = frozenset(" #%/:<>?@[\\]^|\x7f")       # forbidden host code points (after percent-decoding and mapping)
_DIGITS = re.compile(r"[0-9]+")
_HEX_PART = re.compile(r"0[xX][0-9a-fA-F]*")             # "0x" alone is 0, as in a browser
_OCTAL = re.compile(r"0[0-7]+")


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


def _ipv4_part(part: str) -> int | None:
    """One part of a numeric IPv4 host as a browser reads it: hex after 0x, octal after a leading 0, else decimal."""
    if _HEX_PART.fullmatch(part):
        return int(part[2:], 16) if len(part) > 2 else 0
    if len(part) > 1 and part.startswith("0"):
        return int(part, 8) if _OCTAL.fullmatch(part) else None
    return int(part) if _DIGITS.fullmatch(part) else None


def _ipv4(host: str) -> str | None:
    """`host` when it doesn't end in a number; its dotted IPv4 form when a browser reads it as an address (hex, octal,
    a single integer, short forms); None when it ends in a number but isn't a valid address (1.2.3.4.5, 256.0.0.1)."""
    parts = host.split(".")
    if len(parts) > 1 and parts[-1] == "":              # one trailing dot is allowed
        parts.pop()
    if not parts or not (_DIGITS.fullmatch(parts[-1]) or _HEX_PART.fullmatch(parts[-1])):
        return host
    if len(parts) > 4:
        return None
    numbers = [_ipv4_part(p) for p in parts]
    if any(n is None for n in numbers):
        return None
    values = [n for n in numbers if n is not None]
    if any(n > 255 for n in values[:-1]) or values[-1] >= 256 ** (5 - len(values)):
        return None
    address = values[-1]
    for i, n in enumerate(values[:-1]):
        address += n * 256 ** (3 - i)
    return str(ipaddress.IPv4Address(address))


def _host(raw: str) -> str | None:
    """A host as a browser normalizes it: an IPv6 literal (brackets dropped); else percent-decoded, UTS46-mapped
    (non-transitional, no STD3 rules: `_` stays), each non-ASCII label IDNA-encoded, lowercased, and a numeric host
    made dotted IPv4; None when it has a forbidden code point, a label IDNA rejects, or a bad numeric form."""
    if raw.startswith("["):
        try:
            return str(ipaddress.IPv6Address(raw[1:-1])) if raw.endswith("]") else None
        except ValueError:
            return None
    try:
        host = idna.uts46_remap(unquote(raw), std3_rules=False, transitional=False)
    except idna.IDNAError:
        return None
    if any(c in _FORBIDDEN_HOST or ord(c) < 0x20 for c in host):
        return None
    labels: list[str] = []
    for label in host.split("."):
        if label.isascii():
            labels.append(label.lower())                  # browsers run no IDNA2008 checks on an ASCII label
            continue
        try:
            labels.append(idna.alabel(label).decode("ascii"))
        except idna.IDNAError:
            return None
    return _ipv4(".".join(labels))


def _authority_and_path(scheme: str, rest: str) -> tuple[str, str]:
    """The authority and the path of what follows `scheme:` up to the query, with `\\` as `/`. Every leading slash is
    skipped (`https:attacker.example` and `https://\\attacker.example` name attacker.example); `file:` has a host only
    after exactly two slashes (`file:///Users/me` names no site)."""
    text = rest.replace("\\", "/")
    if scheme == "file":
        if not text.startswith("//"):
            return "", text
        text = text[2:]
    else:
        text = text.lstrip("/")
    cut = text.find("/")
    return (text[:cut], text[cut:]) if cut != -1 else (text, "")


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
    head, _, fragment = rest.partition("#")               # `\` stays literal from the query on
    head, _, query = head.partition("?")
    authority, path = _authority_and_path(scheme, head)
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
    if port and (not _DIGITS.fullmatch(port) or int(port) > MAX_PORT):
        return None
    host = _host(raw_host) if raw_host else ""
    if host is None:
        return None
    return URL(scheme, user, password, host, port, path, query, fragment)
