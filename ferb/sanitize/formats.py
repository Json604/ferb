"""Own the secret formats a value shows by its own shape: vendor keys and tokens, private keys, signed tokens,
credential headers, URL credentials, database passwords on a command line, and payment cards (Luhn) with their
expiry and CVV. One table, each entry named, so a gate can say which format it found (`first_format`).
Must not judge intent: a format is a secret class (rule 14), never a decision about an action."""

import re
from typing import Iterator, NamedTuple

# Any whitespace but a line break: a page's no-break space reads as a space wherever a pattern takes one.
_H = r"\t\x0b\x0c\x1c-\x1f \x85\xa0  -     　"


class Hit(NamedTuple):
    start: int
    end: int
    kind: str


def _end(chars: str) -> str:
    """A token's end: no more of its own characters follow (a `\\b` misses a token that ends in "-")."""
    return rf"(?![{chars}])"


_B64 = r"A-Za-z0-9_\-"
# (name, pattern, group masked). The value alone is masked: a label or scheme word before it stays readable.
TABLE: list[tuple[str, re.Pattern[str], int]] = [
    ("AWS access key id", re.compile(rf"A(?<![A-Z0-9]A)(?:KIA|SIA|BIA|CCA)[0-9A-Z]{{16}}{_end('0-9A-Za-z')}"), 0),
    ("AWS secret access key", re.compile(r"(?i)aws_?secret_?(?:access_?)?key[\"']?[ \t]*[:=][ \t]*[\"']?"
                                         r"([A-Za-z0-9/+=]{40})(?![A-Za-z0-9/+=])"), 1),
    ("Stripe key", re.compile(rf"[sr]k_(?<![A-Za-z0-9][sr]k_)(?:live|test)_[0-9A-Za-z]{{10,}}{_end('0-9A-Za-z')}"), 0),
    ("GitHub token", re.compile(rf"gh[pousr]_(?<![A-Za-z0-9]gh[pousr]_)[A-Za-z0-9]{{20,}}{_end('A-Za-z0-9')}"), 0),
    ("GitHub fine-grained token", re.compile(rf"github_pat_(?<![A-Za-z0-9]github_pat_)[A-Za-z0-9_]{{20,}}"
                                             rf"{_end('A-Za-z0-9_')}"), 0),
    ("GitLab token", re.compile(rf"glpat-(?<![A-Za-z0-9]glpat-)[{_B64}]{{20,}}{_end(_B64)}"), 0),
    ("Slack token", re.compile(r"xox[abprs]-(?<![A-Za-z0-9]xox[abprs]-)[A-Za-z0-9-]{10,}(?![A-Za-z0-9-])"), 0),
    ("Anthropic key", re.compile(rf"sk-ant-(?<![A-Za-z0-9]sk-ant-)[{_B64}]{{16,}}{_end(_B64)}"), 0),
    ("OpenAI key", re.compile(rf"sk-(?<![A-Za-z0-9]sk-)(?:proj-|svcacct-|admin-)?[{_B64}]{{20,}}{_end(_B64)}"), 0),
    ("Google API key", re.compile(rf"AIza(?<![A-Za-z0-9]AIza)[{_B64}]{{35}}{_end(_B64)}"), 0),
    ("npm token", re.compile(r"npm_(?<![A-Za-z0-9]npm_)[A-Za-z0-9]{36}(?![A-Za-z0-9])"), 0),
    ("Hugging Face token", re.compile(r"hf_(?<![A-Za-z0-9]hf_)[A-Za-z0-9]{30,}(?![A-Za-z0-9])"), 0),
    ("SendGrid key", re.compile(rf"SG\.(?<![A-Za-z0-9]SG\.)[{_B64}]{{16,}}\.[{_B64}]{{16,}}{_end(_B64)}"), 0),
    ("Twilio key", re.compile(r"(?:SK|AC)(?<![A-Za-z0-9]SK)(?<![A-Za-z0-9]AC)[0-9a-f]{32}(?![A-Za-z0-9])"), 0),
    ("Twilio auth token", re.compile(r"AC(?<![A-Za-z0-9]AC)[0-9a-f]{32}:([0-9a-f]{32})(?![A-Za-z0-9])"), 1),
    ("Discord bot token", re.compile(rf"[MNO](?<![{_B64}.][MNO])[{_B64}]{{23,27}}\.[{_B64}]{{6,7}}\.[{_B64}]{{27,40}}"
                                     rf"{_end(_B64 + '.')}"), 0),
    ("Azure account key", re.compile(r"(?i)(?<![A-Za-z0-9])(?:AccountKey|SharedAccessKey)=([A-Za-z0-9+/=]{20,})"), 1),
    ("Sentry DSN", re.compile(r"(?i)https?://([0-9a-f]{32}(?::[0-9a-f]{32})?)@[A-Za-z0-9.-]*sentry\.io"), 1),
    ("bearer token", re.compile(rf"(?i)(?<![A-Za-z0-9])bearer[{_H}]+([A-Za-z0-9._~+/=-]{{12,}})"), 1),
    ("Authorization header", re.compile(rf"(?i)(?<![A-Za-z0-9-])(?:proxy-)?authorization[\"']?[{_H}]*[:=][{_H}]*[\"']?"
                                        rf"(?:(?:basic|bearer|token|digest|negotiate|apikey)[{_H}]+)?"
                                        r"([^\s\"',;]+)"), 1),
    ("database password", re.compile(r"(?i)(?<![A-Za-z0-9-])--password=([^\s\"']+)"), 1),
    ("CVV", re.compile(rf"(?i)(?<![A-Za-z0-9])(?:cvv2?|cvc2?|csc|card[{_H}]+verification"
                       rf"(?:[{_H}]+(?:code|value|number))?)"
                       rf"(?:[{_H}]*\([A-Za-z0-9 ]{{1,12}}\))?[{_H}]*(?:[:=#-]|is)?[{_H}]*(\d{{3,4}})(?![\d/])"), 1),
    ("card expiry", re.compile(rf"(?i)(?<![A-Za-z0-9])(?:exp(?:iry|ires|iration)?(?:[{_H}]+date)?|valid[{_H}]+"
                               rf"(?:thru|through|till|until)|good[{_H}]+thru|mm[{_H}]*/[{_H}]*yy(?:yy)?)[{_H}]*[:=.]?"
                               rf"[{_H}]*((?:0[1-9]|1[0-2])[{_H}]?[/-][{_H}]?(?:20)?\d{{2}})(?![\d/])"), 1),
]
SECRET_PATTERNS = [p for _, p, _ in TABLE]                # every entry, Discord's included
# Substrings each entry needs, read first (a text holding none skips the pattern's scan): exact, or lowercase for the
# case-insensitive entries.
NEEDLES: dict[str, tuple[tuple[str, ...], bool]] = {
    "AWS access key id": (("AKIA", "ASIA", "ABIA", "ACCA"), False), "AWS secret access key": (("secret",), True),
    "Stripe key": (("k_live_", "k_test_"), False), "GitHub token": (("ghp_", "gho_", "ghu_", "ghs_", "ghr_"), False),
    "GitHub fine-grained token": (("github_pat_",), False), "GitLab token": (("glpat-",), False),
    "Slack token": (("xox",), False), "Anthropic key": (("sk-ant-",), False), "OpenAI key": (("sk-",), False),
    "Google API key": (("AIza",), False), "npm token": (("npm_",), False), "Hugging Face token": (("hf_",), False),
    "SendGrid key": (("SG.",), False), "Twilio key": (("SK", "AC"), False), "Twilio auth token": (("AC",), False),
    "Discord bot token": (("",), False), "Azure account key": (("accesskey=", "accountkey="), True),
    "Sentry DSN": (("sentry",), True), "bearer token": (("bearer",), True),
    "Authorization header": (("authorization",), True), "database password": (("--password=",), True),
    "CVV": (("cvv", "cvc", "csc", "verification"), True), "card expiry": (("exp", "valid", "good", "mm"), True)}
_DISCORD = next(entry for entry in TABLE if entry[0] == "Discord bot token")
TABLE.remove(_DISCORD)                                # read only in a text long enough, with two dots
_NEEDLE_SETS = {kind: frozenset(needles) for kind, (needles, _) in NEEDLES.items()}
_EXACT_NEEDLES = re.compile("|".join(sorted({re.escape(n) for needles, folded in NEEDLES.values() if not folded
                                              for n in needles if n}, key=len, reverse=True)))
# A needle found implies every needle inside it ("sk-ant-" holds "sk-"; one search finds only the longer).
_IMPLIED = {n: {m for ns, _ in NEEDLES.values() for m in ns if m and m in n} for ns, _ in NEEDLES.values() for n in ns}
_FOLDED_NEEDLES = re.compile("|".join(sorted({re.escape(n) for needles, folded in NEEDLES.values() if folded
                                               for n in needles}, key=len, reverse=True)))

# A private key block (PEM, OpenSSH, PGP) runs from its BEGIN line to the first END line after it; with no END line it
# is a truncated block, masked over its body lines (base64 and armor headers), escaped or not.
PRIVATE_KEY_START = re.compile(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----")
PRIVATE_KEY_END = re.compile(r"-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----")
_KEY_BODY = re.compile(r"(?:(?:\r\n|\r|\n|\\r\\n|\\n|\\r)[ \t]*(?:[A-Za-z0-9+/=]++|(?:Proc-Type|DEK-Info|Comment|"
                       r"Version|Hash|Charset):[^\r\n\\]*+)?[ \t]*+(?=[\r\n\"]|\\[rn]|$))*+"     # whole lines, then
                       # a last line whose base64 run is followed by other text on that line (step 12d fuzz: a pasted
                       # key with words after it on its last line kept that line)
                       r"(?:(?:\r\n|\r|\n|\\r\\n|\\n|\\r)[ \t]*[A-Za-z0-9+/=]{16,}+)?")
# A JSON web token, read once (a failed start skips its run of token characters).
_JWT_START = re.compile(r"eyJ(?<![A-Za-z0-9_-]eyJ)")
JWT = re.compile(r"eyJ[A-Za-z0-9_-]{8,}+\.[A-Za-z0-9_-]{8,}+\.[A-Za-z0-9_-]{8,}+")
_TOKEN_RUN = re.compile(r"[A-Za-z0-9_-]*+")
# A cookie header's value: the rest of its line, or its quoted string.
COOKIE = re.compile(rf"(?i)(?<![A-Za-z0-9-])(?:set-)?cookie[\"']?[{_H}]*:[{_H}]*")
_QUOTED = {'"': re.compile(r'(?:[^"\\\r\n]|\\.)*+"'), "'": re.compile(r"(?:[^'\\\r\n]|\\.)*+'")}
_LINE_REST = re.compile(r"[^\r\n]*")
# A MySQL/MariaDB client's password glued to -p, on the client's own command line.
_MYSQL = re.compile(r"(?<![A-Za-z0-9_./-])(?:mysql|mysqldump|mysqladmin|mariadb|mariadb-dump)(?![A-Za-z0-9_-])")
_MYSQL_ARGS = re.compile(r"[^\r\n|;&]*")
_MYSQL_P = re.compile(r"(?<!\S)-p([^\s\"']+)")
# A URL (its scheme run read once); its userinfo and its signed or credential parameters (`url_hits`).
URL = re.compile(r"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s\"'<>]+")
_URL_START = re.compile(r"(?<![a-zA-Z0-9+.-])[a-zA-Z]")
_SCHEME_CHARS = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+.-")
_PARAM = re.compile(r"(?<![^&;?#])([^&;=#]*)=([^&;#]*)")
_TOKEN_LIKE = re.compile(r"[A-Za-z0-9._~+/=-]{16,}")
SECRET_QUERY = {"token", "code", "key", "sig", "signature", "auth", "session", "access_token", "id_token",
                "refresh_token", "password", "secret", "otp", "apikey", "api_key", "client_secret", "sessionid", "pwd",
                "pass", "passwd", "pin"}
SECRET_QUERY_WORDS = ("signature", "token", "credential", "secret", "password", "apikey", "api_key", "hmac")
CURSOR_PARAMS = {"pagetoken", "nextpagetoken", "continuationtoken", "synctoken", "cursor"}
# A payment card: digit groups joined by single spaces or hyphens, a token of its own (never inside an id); a JSON
# escape before it still separates it. Every window of whole groups with 13-19 digits is checked (`card_hits`).
_DIGIT_RUN = re.compile(rf"[0-9](?:(?<![0-9A-Za-z_][0-9])|(?<=\\[ntrbf][0-9])|(?<=\\u[0-9a-fA-F]{{4}}[0-9]))"
                        rf"(?<![0-9A-Za-z_]-[0-9])[0-9]*(?:[{_H}-][0-9]+)*(?![0-9A-Za-z_]|-[0-9A-Za-z_])")
_GROUP = re.compile(r"[0-9]+")
_AFTER_CARD = re.compile(rf"[{_H},;:|()-]{{0,3}}(?:(?i:exp(?:iry|ires|iration)?|valid[{_H}]+(?:thru|till))[{_H}.:]*)?"
                         rf"((?:0[1-9]|1[0-2])[{_H}]?[/-][{_H}]?(?:20)?\d{{2}})(?![\d/])"
                         rf"(?:[{_H},;:|]{{1,3}}(?:(?i:cvv2?|cvc2?|csc)[{_H}.:]*)?(\d{{3,4}})(?![\d/]))?")


def secret_param(name: str) -> bool:
    """A URL query or fragment parameter that carries a credential (pagination cursors aside)."""
    k = name.lower()
    flat = "".join(ch for ch in k if ch.isalnum())
    if flat in CURSOR_PARAMS:
        return False
    return (k in SECRET_QUERY or any(w in k or w.replace("_", "") in flat for w in SECRET_QUERY_WORDS)
            or flat.endswith("sig") or flat in {"xamzcredential", "xgoogcredential"})


def payment_card(value: str) -> bool:
    digits = [int(c) for c in value if c.isdigit() and c.isascii()]
    if not 13 <= len(digits) <= 19 or len(set(digits)) == 1:
        return False
    return sum((d * 2 - 9 if d * 2 > 9 else d * 2) if i % 2 else d
               for i, d in enumerate(reversed(digits))) % 10 == 0


def urls(s: str) -> list[re.Match[str]]:
    """Every URL in `s`, reading the text once: only the first start of a scheme run can match."""
    found, pos = [], 0
    while (sep := s.find("://", pos)) >= 0:
        run = sep
        while run > pos and s[run - 1] in _SCHEME_CHARS:
            run -= 1
        start = _URL_START.search(s, run, sep)
        url = URL.match(s, start.start()) if start is not None else None
        if url is not None:
            found.append(url)
        pos = url.end() if url is not None else sep + 1
    return found


def url_hits(s: str, url: re.Match[str]) -> list[Hit]:
    """A URL's credentials: its userinfo password (or a token alone as userinfo), and the values of its credential
    query and fragment parameters. The rest of the URL keeps its exact text."""
    start, text = url.start(), url.group()
    out: list[Hit] = []
    rest = text.index("://") + 3
    stop = min((i for i in (text.find(c, rest) for c in "/?#") if i >= 0), default=len(text))
    at = text.rfind("@", rest, stop)
    if at > rest:
        info = text[rest:at]
        colon = info.find(":")
        if colon >= 0 and colon + 1 < len(info):
            out.append(Hit(start + rest + colon + 1, start + at, "URL password"))
        elif colon < 0 and _TOKEN_LIKE.fullmatch(info) and any(c.isdigit() for c in info):
            out.append(Hit(start + rest, start + at, "URL token"))
    for mark in "?#":
        i = text.find(mark, stop)
        if i < 0:
            continue
        end = text.find("#", i + 1) if mark == "?" else len(text)
        end = len(text) if end < 0 else end
        for m in _PARAM.finditer(text, i + 1, end):
            if m.group(2) and secret_param(m.group(1)):
                out.append(Hit(start + m.start(2), start + m.end(2), "URL credential"))
    return out


def url_paths(found: list[re.Match[str]]) -> list[tuple[int, int]]:
    """Where each URL's scheme, host and path lie (a card-like number there is an id, never a card)."""
    out = []
    for m in found:
        tail = re.search(r"[?#]", m.group())
        out.append((m.start(), m.start() + tail.start() if tail else m.end()))
    return out


def private_key_hits(s: str) -> list[Hit]:
    out, pos, endless = [], 0, len(s) + 1           # past `endless` no END line follows: never searched again
    while (begin := PRIVATE_KEY_START.search(s, pos)) is not None:
        end = PRIVATE_KEY_END.search(s, begin.end()) if begin.end() < endless else None
        if end is None:
            endless = min(endless, begin.end())
        if end is not None:
            out.append(Hit(begin.start(), end.end(), "private key"))
            pos = end.end()
            continue
        body = _KEY_BODY.match(s, begin.end())
        stop = body.end() if body is not None else begin.end()
        while stop > begin.end() and s[stop - 1] in " \t\r\n":
            stop -= 1
        out.append(Hit(begin.start(), stop, "private key"))
        pos = max(stop, begin.end())
    return out


def jwt_hits(s: str) -> list[Hit]:
    out, pos = [], 0
    while (start := _JWT_START.search(s, pos)) is not None:
        if (token := JWT.match(s, start.start())) is not None:
            out.append(Hit(*token.span(), "JSON web token"))
            pos = token.end()
        else:
            pos = _TOKEN_RUN.match(s, start.start()).end()        # type: ignore[union-attr]
    return out


def cookie_hits(s: str) -> list[Hit]:
    out = []
    for m in COOKIE.finditer(s):
        quote = s[m.end():m.end() + 1]
        if quote in _QUOTED and (close := _QUOTED[quote].match(s, m.end() + 1)) is not None:
            a, b = m.end() + 1, close.end() - 1
        else:
            a, b = m.end(), _LINE_REST.match(s, m.end()).end()     # type: ignore[union-attr]
            while b > a and s[b - 1].isspace():
                b -= 1
        if b > a:
            out.append(Hit(a, b, "cookie"))
    return out


def mysql_hits(s: str) -> list[Hit]:
    out = []
    for m in _MYSQL.finditer(s):
        args = _MYSQL_ARGS.match(s, m.end())
        assert args is not None
        out += [Hit(*p.span(1), "database password") for p in _MYSQL_P.finditer(s, m.end(), args.end())]
    return out


def card_hits(s: str, ids: list[tuple[int, int]] | None = None) -> list[Hit]:
    """Payment cards (Luhn) over every window of whole digit groups (so "qty 2 4111 1111 1111 1111" masks the card),
    and the expiry and CVV written right after one. A number that starts a URL path segment or host label is an id."""
    out = []
    ids = ids if ids is not None else url_paths(urls(s))
    for run in _DIGIT_RUN.finditer(s):
        if run.end() - run.start() < 13:
            continue
        groups = [g.span() for g in _GROUP.finditer(s, run.start(), run.end())]
        if sum(b - a for a, b in groups) < 13:
            continue
        last = None
        for i, (a, _) in enumerate(groups):
            # Luhn over the window as it grows to the right: `plain` is the sum with its last digit undoubled,
            # `doubled` with it doubled; a new digit on the right swaps them (O(1) per digit).
            plain = doubled = count = 0
            first, same = s[a], True
            for j in range(i, len(groups)):
                for c in s[groups[j][0]:groups[j][1]]:
                    d = ord(c) - 48
                    plain, doubled = doubled + d, plain + (d * 2 - 9 if d > 4 else d * 2)
                    count += 1
                    same = same and c == first
                if count > 19:
                    break
                if count >= 13 and plain % 10 == 0 and not same and not (
                        a > 0 and s[a - 1] in "/." and any(x <= a and groups[j][1] <= y for x, y in ids)):
                    out.append(Hit(a, groups[j][1], "payment card"))
                    last = max(last or 0, groups[j][1])
        if last is not None and (after := _AFTER_CARD.match(s, last)) is not None:
            out.append(Hit(*after.span(1), "card expiry"))
            if after.group(2):
                out.append(Hit(*after.span(2), "CVV"))
    return out


def format_hits(s: str, found_urls: list[re.Match[str]] | None = None) -> Iterator[Hit]:
    """Every format in `s`. `found_urls`: `urls(s)`, when the caller already read them."""
    if len(s) < 7:                                  # shorter than any format
        return
    lower = s.lower()
    if len(s) < 2048:                               # a short text: which needles it holds, in two searches
        held = {m for n in set(_EXACT_NEEDLES.findall(s)) | set(_FOLDED_NEEDLES.findall(lower)) for m in _IMPLIED[n]}
        kinds = [entry for entry in TABLE if held & _NEEDLE_SETS[entry[0]]]
    else:
        kinds = [entry for entry in TABLE
                 if any(map((lower if NEEDLES[entry[0]][1] else s).__contains__, NEEDLES[entry[0]][0]))]
    if len(s) >= 59 and s.count(".") >= 2:
        kinds.append(_DISCORD)
    for kind, pattern, group in kinds:
        for m in pattern.finditer(s):
            yield Hit(m.start(group), m.end(group), kind)
    if "-----BEGIN" in s:
        yield from private_key_hits(s)
    if "eyJ" in s:
        yield from jwt_hits(s)
    if "cookie" in lower:
        yield from cookie_hits(s)
    if "mysql" in lower or "mariadb" in lower:
        yield from mysql_hits(s)
    found = urls(s) if found_urls is None else found_urls
    for url in found:
        yield from url_hits(s, url)
    yield from card_hits(s, url_paths(found))


def first_format(s: str) -> str | None:
    """The name of the first format `s` holds, or None (a gate's specific hit). A hit that is only a mask (a URL's
    "code=[redacted]") is none."""
    for h in format_hits(s):
        if any(c.isalnum() for c in s[h.start:h.end].replace("[redacted]", "").replace("[excluded]", "")):
            return h.kind
    return None
