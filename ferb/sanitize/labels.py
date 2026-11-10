"""Own the labelled secret classes (C1, owner decision 2026-10-15; rule 14): a value a label names as a secret, and a
one-time code with its code word near. One label vocabulary (`label_class`) serves dict keys, `key=value` and
`"key": value` text, URL parameters and field metadata; prose adds the owner's secret words ("my password is …",
"PIN 4821") and code words ("482913 is your code"). Labelled by words, never by digit shape alone: a number with no
code or secret word near it stays visible. Must not judge intent: these are secret classes, never decisions.

Owner-decided details kept: address and coupon codes are ordinary data ("PIN code 560001"); a code reaches its word
across at most `CODE_REACH` words."""

import bisect
import functools
import math
import re
from collections import Counter
from typing import Any

from ferb.sanitize.formats import _H, urls
from ferb.sanitize.known import REDACTED

# ---------- the one vocabulary ----------

# A label's words ("session_id" -> session, id; "apiKey" -> api, key). FERB's own id keys never label a secret.
INTERNAL_ID_KEYS = {"task_id", "span_id", "session_id", "run_token_id", "request_id", "covered", "source_refs",
                    "procedure_id", "approval_id", "timer_id"}
# Words a label of any form names a secret with (prose too).
PROSE_SECRET = {"password", "passwords", "passwd", "pwd", "passphrase", "passcode", "secret", "token", "otp", "totp",
                "pin", "cvv", "cvc", "cvv2", "cvc2", "csc", "apikey"}
# Words only an identifier-shaped label names a secret with ({"session": …}, `cookie=…`, `"auth": …`).
KEY_SECRET = {"cookie", "session", "credential", "credentials", "authorization", "auth", "bearer", "sessionid"}
SECRET_COMPOUNDS = {"apikey", "accesstoken", "refreshtoken", "idtoken", "clientsecret", "privatekey", "setcookie",
                    "onetimecode", "onetimepassword", "xapikey", "cardnumber", "ccnumber", "ccnum", "cardnum",
                    "securitycode", "accountkey", "sharedaccesskey", "secretkey", "accesskey", "signingkey",
                    "encryptionkey", "masterkey"}
# Words that name a one-time code: its value is masked when it is code-shaped.
CODE_KEY_WORDS = {"code", "codes", "verification", "2fa"}
# The last word of a credential-like key: its value is masked when it looks random (`high_entropy`).
CREDENTIAL_LAST = {"key", "sig", "signature", "dsn", "salt", "hmac"}
# Words after the naming word that still name its value ("session_id", "password_value", "token_b64").
QUALIFIERS = {"id", "value", "string", "str", "b64", "base64", "hex", "raw", "plain", "plaintext", "text", "data",
              "enc", "encrypted", "bytes"}
# Address and coupon codes are ordinary data (owner decision 2026-10-15).
PLACE_WORDS = {"pin", "zip", "postal", "post", "area", "coupon", "promo", "promotional", "discount", "voucher"}


def words(key: str) -> list[str]:
    spaced = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", key)
    return [w for w in re.split(r"[^A-Za-z0-9]+", spaced.lower()) if w]


@functools.lru_cache(maxsize=8192)
def label_class(label: str) -> str | None:
    """What a label names: "secret" (any value), "code" (a code-shaped value), "credential" (a random-looking value)
    or None. The naming word is the label's last (after qualifiers like "_id", "_value"): "session_token" and
    "DB_PASSWORD" name secrets, "token_limit" and "pin_message" don't. An identifier-shaped label (no whitespace: a
    dict key, `KEY=`, a field's name) reads the whole vocabulary; a label with spaces only the prose words."""
    if label in INTERNAL_ID_KEYS:
        return None
    ws = words(label)
    if not ws:
        return None
    if any(a in PLACE_WORDS and b in ("code", "codes") for a, b in zip(ws, ws[1:])) or \
            any(w.endswith("code") and w[:-4] in PLACE_WORDS for w in ws):
        return None
    while len(ws) > 1 and ws[-1] in QUALIFIERS and not (ws[-1] == "id" and ws[-2] == "key"):
        ws = ws[:-1]                                # "session_id" names the session; "kms_key_id" no key
    prose = any(c.isspace() for c in label.strip())
    last = ws[-1]
    if last in PROSE_SECRET or (not prose and last in KEY_SECRET) or \
            any("".join(ws[-n:]) in SECRET_COMPOUNDS for n in (1, 2, 3, 4)):
        return "secret"
    if last in CODE_KEY_WORDS:
        return "code"
    if not prose and len(ws) >= 2 and last in CREDENTIAL_LAST:
        return "credential"
    return None


def secret_key(key: str) -> bool:
    """A dict key whose value is a secret whatever it is."""
    return label_class(key) == "secret"


_TOKEN_CHARS = re.compile(r"[A-Za-z0-9+/=_.~-]+")


def high_entropy(value: str) -> bool:
    """A value that looks random: 16+ token characters, letters and digits mixed, about 3+ bits per character."""
    if len(value) < 16 or not _TOKEN_CHARS.fullmatch(value) or not any(c.isdigit() for c in value) or \
            not any(c.isalpha() for c in value):
        return False
    n = len(value)
    return -sum(c / n * math.log2(c / n) for c in Counter(value).values()) >= 3.0


def labelled_value(cls: str | None, value: str) -> bool:
    """Whether a label of class `cls` hides this value."""
    v = value.strip()
    if not v or cls is None or v.lower() in ("null", "none", "true", "false", "undefined"):
        return False
    if cls == "secret":
        return True
    if cls == "code":
        return CODE_VALUE.fullmatch(v) is not None
    return high_entropy(v)


# ---------- key-form labels in text: `KEY=value`, `"key": value`, `--flag=value` ----------

KEY_LABEL = re.compile(rf"""(?<![A-Za-z0-9_$.\-])(?P<q>["'`]?)
                           (?P<key>--?[A-Za-z][A-Za-z0-9_-]*+|[A-Za-z_$][A-Za-z0-9_.$-]*+)
                           (?P=q)[{_H}]*(?P<op>=|:(?!//))[{_H}]*""", re.VERBOSE)
_STOP = re.compile(r"[\s\"'<>`]")
_SPACE = re.compile(r"\s")
_DOUBLE_QUOTE = re.compile(r'"')


class Ends:
    """Where the run of value characters from a position ends, for one text, read once (a long line of labels with
    no space would otherwise be read again from every label: quadratic)."""

    def __init__(self, s: str) -> None:
        self.s = s
        self._found: dict[str, list[int]] = {}

    def _next(self, kind: str, pattern: re.Pattern[str], pos: int) -> int:
        if (found := self._found.get(kind)) is None:
            found = self._found[kind] = [m.start() for m in pattern.finditer(self.s)]
        i = bisect.bisect_left(found, pos)
        return found[i] if i < len(found) else len(self.s)

    def stop(self, pos: int) -> int:
        """Before whitespace, a quote, "<" or ">"."""
        return self._next("stop", _STOP, pos)

    def space(self, pos: int) -> int:
        """Before whitespace."""
        return self._next("space", _SPACE, pos)

    def quote(self, pos: int) -> int:
        """Before a double quote."""
        return self._next("quote", _DOUBLE_QUOTE, pos)
_VALUE_QUOTES = {q: re.compile(rf"(?:[^{q}\\\r\n]|\\.)*+{q}") for q in "\"'`"}


def key_label_spans(s: str, ends: "Ends | None" = None) -> list[tuple[int, int]]:
    """Values after identifier-shaped labels the vocabulary names. A plain single word before ":" unquoted is prose
    ("Password: x"), read by `secret_value`; quoted keys (JSON) stop an unquoted value at "," "}" "]", and a query's
    at "&"."""
    out, ends = [], ends or Ends(s)
    for m in KEY_LABEL.finditer(s):
        key, op, quoted = m.group("key"), m.group("op"), m.group("q")
        if op == ":" and not quoted and re.fullmatch(r"[A-Za-z]+", key) and not re.search(r"[a-z][A-Z]", key):
            continue
        cls = label_class(key.lstrip("-"))
        if cls is None:
            continue
        start = m.end()
        if start < len(s) and s[start] in _VALUE_QUOTES:
            close = _VALUE_QUOTES[s[start]].match(s, start + 1)
            if close is None:
                continue
            a, b = start + 1, close.end() - 1
        else:
            a, b = start, ends.stop(start)
            stops = ",}]" if quoted else ""
            stops += "&" if m.start() and s[m.start() - 1] in "?&" else ""
            if stops:
                b = next((i for i in range(a, min(b, a + 4096)) if s[i] in stops), b)
            while b > a and s[b - 1] in ".,;:!?)]}":
                b -= 1
        if b > a and labelled_value(cls, s[a:b]) and REDACTED != s[a:b]:
            out.append((a, b))
    return out


# ---------- one-time codes ----------

OTP_LABEL = re.compile(rf"(?i)\b(?:otp|totp|2fa|one[{_H}_-]?time(?:[{_H}_-]+(?:code|password|pin))?|"
                       rf"(?:verification|authentication|security|login|sign[{_H}_-]?in|access)[{_H}_-]+"
                       r"(?:code|pin))\b")
# The owner's code words (code, OTP, passcode, PIN, one-time password, verification); never part of a file name.
CODE_WORD = re.compile(rf"(?i)(?<![A-Za-z0-9])(?:one[{_H}_-]?time[{_H}_-]?(?:password|passcode|code|pin)|otp|totp|"
                       rf"2fa|pass[{_H}_-]?code|pin|codes?|verification)(?![A-Za-z0-9]|\.[A-Za-z0-9])")
# Simple inline HTML tags, and the marks that may sit between a code and its label on one line.
_TAG = r"</?[A-Za-z][A-Za-z0-9]*(?:[ \t][^<>\r\n]*|/)?>"
_TAG_RE = re.compile(_TAG)
_MARKUP = rf"(?:[{_H}*_`'\"]|{_TAG})"
# The code itself: 4-8 digits, grouped by spaces or hyphens, or Google's G-482913; never part of a longer token or
# digit group. A masked code counts as the digits it replaced.
CODE_VALUE = re.compile(rf"(?:(?<![0-9A-Za-z_-])|(?<=(?<![0-9A-Za-z])_)|(?<=\\[ntrbf])|(?<=\\u[0-9a-fA-F]{{4}}))"
                        rf"(?<!\d[{_H}-])(?<!\[redacted\][{_H}-])"
                        rf"(?:[A-Z]-)?\d(?:[{_H}-]?\d){{3,7}}"
                        rf"(?![0-9A-Za-z]|_[0-9A-Za-z]|[{_H}-]\d|[{_H}-]\[redacted\])")
# A code before its word is joined to it: "482913 is your code", "739201 as your PIN", "739201 (OTP)".
CODE_LINK = re.compile(rf"(?i){_MARKUP}*+(?:(?:is|are|was|as)(?![A-Za-z0-9])|\()")
_PLACE = r"(?:pin|zip|postal|post|area|coupon|promo(?:tional)?|discount|voucher)"
_PLACES = re.compile(rf"(?i)(?<![A-Za-z0-9]){_PLACE}(?:[{_H}]*+/[{_H}]*+{_PLACE})*+")
_ADDRESS_TAIL = re.compile(rf"(?i)[{_H}_-]?codes?(?![A-Za-z0-9])")
CODE_REACH = 8                      # a code word reaches a code across at most this many words (owner's corpus)
LINE_BREAK = re.compile(r"\r\n|\r|\n")
LABEL_DIRECT = re.compile(rf"(?:{_MARKUP}|[:=#\-–—])*+")
LABEL_LINK = re.compile(rf"(?i)(?:(?<![A-Za-z0-9])(?:is|are|was)|[:=#\-–—]){_MARKUP}*+$")
_HARD_BREAK = re.compile(r"[;!?](?=\s|$)|,(?=\s)")
_SENTENCE_DOT = re.compile(r"\.(?=\s+[A-Z]|\s*$)")
_ABBREVIATION = re.compile(r"[A-Za-z]{1,3}|[A-Za-z]/[A-Za-z]")
# What every code word holds (a text without any has no code word, so its code rules are skipped).
_CODE_STEMS = ("code", "otp", "2fa", "pin", "pass", "verification")
# An explicit OTP label reaches the first code within 40 non-digit characters ("verification code to sign in 482913").
OTP_TEXT = re.compile(rf"(?i)\b(?:one[{_H}-]?time(?:[{_H}](?:password|code))?|"
                      rf"(?:verification|authentication|security|login|sign[{_H}-]?in)[{_H}]code|otp)"
                      r"\b(?:(?!\[redacted\])[^\d\r\n]){0,40}"
                      rf"(?<![+\d])(?<!\d[{_H}-])(?<![A-Za-z]-)(?<!\[redacted\][{_H}-])"
                      rf"(\d(?:[{_H}-]?\d){{3,7}})(?![{_H}-]?\d|[{_H}-]\[redacted\])")


def _inside(spans: list[tuple[int, int]], pos: int) -> bool:
    i = bisect.bisect_right(spans, (pos, float("inf"))) - 1
    return i >= 0 and pos < spans[i][1]


def _address_codes(s: str) -> list[tuple[int, int]]:
    return [(chain.start(), tail.end()) for chain in _PLACES.finditer(s)
            if (tail := _ADDRESS_TAIL.match(s, chain.end())) is not None]


def _not_address(s: str, found: Any) -> list[re.Match[str]]:
    matches: list[re.Match[str]] = list(found)
    if not matches:
        return matches
    spans = _address_codes(s)
    return [m for m in matches if not _inside(spans, m.start())] if spans else matches


def code_words(s: str) -> list[re.Match[str]]:
    """The code words of `s`, outside address codes and HTML tags ("<code>" names no code)."""
    found = _not_address(s, CODE_WORD.finditer(s))
    if not found or "<" not in s:
        return found
    tags = [m.span() for m in _TAG_RE.finditer(s)]
    return [m for m in found if not _inside(tags, m.start())] if tags else found


def _clause_break(s: str, a: int, b: int) -> bool:
    """Whether a sentence or clause ends between a and b: ";" "!" "?" and ", " always; a dot only before a new
    sentence and after a word that isn't abbreviation-shaped ("Rs. 2,000", "a/c no. XX1234" keep a bank's OTP)."""
    if _HARD_BREAK.search(s, a, b):
        return True
    for dot in _SENTENCE_DOT.finditer(s, a, b):
        chunk = s[max(0, dot.start() - 24):dot.start()]
        if not chunk or chunk[-1].isspace() or not _ABBREVIATION.fullmatch(chunk.split()[-1]):
            return True
    return False


_GAP_CHARS = 512                     # a gap longer than this is beyond any code word's reach


def _words_between(s: str, a: int, b: int) -> int:
    """How many words start between a and b (a word that started before a doesn't count)."""
    if b - a > _GAP_CHARS:
        return CODE_REACH + 1
    n = len(s[a:b].split())
    return n - 1 if n and 0 < a < len(s) and not s[a - 1].isspace() and not s[a].isspace() else n


def _label_joined(s: str, label_end: int, code: re.Match[str]) -> bool:
    gap = s[label_end:code.start()]
    if LABEL_DIRECT.fullmatch(gap):
        return True
    return LINE_BREAK.search(gap) is None and (LABEL_LINK.search(gap) is not None or (
        gap.rstrip().endswith("(") and s.startswith(")", code.end())))


def inline_code_spans(s: str, in_url: Any) -> list[tuple[int, int]]:
    """Codes with their code word on the same line within reach, joined to it: after it directly or after a link
    ("Code: 482913", "… is 482913", "code (482913)"), or before it with a link ("482913 is your code")."""
    lower = s.lower()
    if not any(map(lower.__contains__, _CODE_STEMS)):
        return []
    codes = [m for m in CODE_VALUE.finditer(s) if not in_url(m.span())]
    labels = [m.span() for m in code_words(s) if not in_url(m.span())] if codes else []
    spans = [m.span(1) for m in OTP_TEXT.finditer(s) if not in_url(m.span(1))]
    if not labels:
        return spans
    ends, begins = [b for _, b in labels], [a for a, _ in labels]
    for code in codes:
        before = bisect.bisect_right(ends, code.start()) - 1
        after = bisect.bisect_left(begins, code.end())
        if (before >= 0 and _words_between(s, ends[before], code.start()) <= CODE_REACH
                and LINE_BREAK.search(s, ends[before], code.start()) is None
                and not _clause_break(s, ends[before], code.start())
                and _label_joined(s, ends[before], code)) or (
                after < len(labels) and CODE_LINK.match(s, code.end()) is not None
                and _words_between(s, code.end(), begins[after]) <= CODE_REACH
                and LINE_BREAK.search(s, code.end(), begins[after]) is None
                and not _clause_break(s, code.end(), begins[after])):
            spans.append(code.span())
    return spans


# ---------- a code alone on its line next to its label line (an email's layout) ----------

_CODE_LEAD = re.compile(r"(?:>[\s>]*)?(?:#+(?=\s)\s*)?(?:[-*•·–—](?=\s))?")
_MARK_CHARS = frozenset(".,;:()[]'\"*_`")


def marks_from(s: str, a: int, b: int) -> int:
    while a < b:
        if s[a].isspace() or s[a] in _MARK_CHARS:
            a += 1
        elif s[a] == "<" and (tag := _TAG_RE.match(s, a, b)) is not None:
            a = tag.end()
        else:
            break
    return a


def _marks_to(s: str, a: int, b: int) -> int:
    while b > a:
        if s[b - 1].isspace() or s[b - 1] in _MARK_CHARS:
            b -= 1
        elif s[b - 1] == ">" and (lt := s.rfind("<", a, b)) >= 0 and _TAG_RE.fullmatch(s, lt, b) is not None:
            b = lt
        else:
            break
    return b


def code_alone_span(line: str) -> tuple[int, int] | None:
    """Where the code lies in a line that is only a code with its marks (brackets, markup, a quote or list lead)."""
    a = _CODE_LEAD.match(line, len(line) - len(line.lstrip())).end()   # type: ignore[union-attr]
    a = marks_from(line, a, len(line))
    b = _marks_to(line, a, len(line))
    return (a, b) if a < b and CODE_VALUE.fullmatch(line[a:b]) is not None else None


def code_alone(line: str) -> bool:
    return code_alone_span(line) is not None


def quote_only(line: str) -> bool:
    return not line.replace(">", "").strip()


def label_line(line: str) -> bool:
    """A line that labels the code under it: a code word within reach of its end, no sentence ending after it."""
    found = code_words(line)
    if not found:
        return False
    text = line.rstrip().rstrip(".:=#-–—").rstrip()
    end = found[-1].end()
    return len(line[end:].split()) <= CODE_REACH and (end >= len(text) or not _clause_break(text, end, len(text)))


def link_line(line: str) -> bool:
    """A line that names the code above it: it starts with a link and holds a code word ("is your code")."""
    found = code_words(line)
    word = found[0] if found else None
    return (CODE_LINK.match(line) is not None and word is not None and not _clause_break(line, 0, word.start())
            and len(line[:word.start()].split()) <= CODE_REACH)


def content_lines(s: str) -> list[tuple[int, int]]:
    """The lines of `s` that aren't blank, as (start, end)."""
    lines, begin = [], 0
    for m in [*LINE_BREAK.finditer(s), None]:
        end = m.start() if m is not None else len(s)
        if s[begin:end].strip():
            lines.append((begin, end))
        begin = m.end() if m is not None else len(s)
    return lines


def line_code_spans(s: str, lines: list[tuple[int, int]]) -> list[tuple[int, int]]:
    spans = []
    for i, (a, b) in enumerate(lines):
        code = code_alone_span(s[a:b])
        if code is None:
            continue
        j = i - 1
        while j >= 0 and quote_only(s[lines[j][0]:lines[j][1]]):
            j -= 1
        above = s[lines[j][0]:lines[j][1]] if j >= 0 else ""
        below = s[lines[i + 1][0]:lines[i + 1][1]] if i + 1 < len(lines) else ""
        if label_line(above) or link_line(below):
            spans.append((a + code[0], a + code[1]))
    return spans


# ---------- backup and recovery code lists ----------

CODE_LIST = re.compile(rf"(?i)(?<![A-Za-z0-9])(?:backup|recovery|2fa|two[{_H}-]?factor|one[{_H}-]?time|verification|"
                       rf"security|login)(?:[{_H}]+backup)?[{_H}]+codes[{_H}]*:[{_H}]*")
LIST_TOKEN = re.compile(r"(?=[A-Za-z0-9-]*\d)[A-Za-z0-9]{3,10}(?:-[A-Za-z0-9]{3,10})?(?![A-Za-z0-9-])")
_LIST_GAP = re.compile(rf"[{_H},]*(?:(?:\r\n|\r|\n)[{_H}]*)*+")
_LIST_LEAD = re.compile(rf"[{_H},]*")
LIST_MARKER = re.compile(rf"(?:\d+[.)]|[-*•])[{_H}]+")
_LIST_SPLIT = re.compile(r"[\s,]+")


def list_line(line: str) -> bool:
    """A line of only list codes, after an item marker ("1. a1b2c-3d4e5", ", 482-913")."""
    body = line.lstrip()
    if (marker := LIST_MARKER.match(body)) is not None:
        body = body[marker.end():]
    tokens = [t for t in _LIST_SPLIT.split(body) if t]
    return bool(tokens) and all(LIST_TOKEN.fullmatch(t) for t in tokens)


def list_label_line(line: str) -> bool:
    """A line that starts a list: its label, then only codes ("Backup codes: 1234 5678")."""
    found = _not_address(line, CODE_LIST.finditer(line))
    rest = line[found[-1].end():] if found else ""
    return bool(found) and (not rest.replace(",", " ").strip() or list_line(rest))


def code_list_spans(s: str) -> list[tuple[int, int]]:
    """Each code after a backup/recovery/2FA list label; across line breaks only onto lines made only of codes."""
    spans = []
    for label in _not_address(s, CODE_LIST.finditer(s)):
        pos = label.end()
        while True:
            gap = _LIST_GAP.match(s, pos)
            assert gap is not None
            start = gap.end()
            breaks = list(LINE_BREAK.finditer(gap.group()))
            if breaks:
                line_end = LINE_BREAK.search(s, start)
                if not list_line(s[pos + breaks[-1].end():line_end.start() if line_end else len(s)]):
                    break
                start = _LIST_LEAD.match(s, start).end()               # type: ignore[union-attr]
            if (breaks or pos == label.end()) and (marker := LIST_MARKER.match(s, start)) is not None:
                start = marker.end()
            token = LIST_TOKEN.match(s, start)
            if token is None:
                break
            spans.append(token.span())
            pos = token.end()
    return spans


# ---------- prose secret words: "password: x", "my password is x", "PIN 4821" ----------

SECRET_WORD = re.compile(rf"(?i)(?<![A-Za-z0-9])(?:pass(?:word|wd|phrase|code)|pwd|pin|secret(?:[{_H}_-]?key)?|"
                         rf"api[{_H}_-]?key|(?:(?:access|auth|api|refresh)[{_H}_-]?)?token|private[{_H}_-]?key)"
                         r"(?![A-Za-z0-9])")
SECRET_AFTER = re.compile(rf"""(?ix)(?P<qual>[{_H}]+(?:for|to|of|on|at)[{_H}]+(?:(?:the|my|your)[{_H}]+)?[^\s:=]+)?
                              (?P<link>(?:\\?["'])?[{_H}]*[:=][{_H}]*(?:(?:\r\n|\r|\n)[{_H}]*)*+|
                                       [{_H}]+(?:is|was)(?![A-Za-z0-9])[{_H}]*+:?[{_H}]*+|[{_H}]+[-–—][{_H}]+)""")
SECRET_DIRECT = re.compile(rf"[{_H}]+")
MINE = re.compile(rf"(?i)(?<![A-Za-z0-9])(?:my|our)[{_H}]+(?:[A-Za-z-]+[{_H}]+)?$")
_TRAILING = ".,;:!?)]}\"'`"
_ALNUM = re.compile(r"[^\W_]")
_NOT_ALNUM = re.compile(r"[\W_]")
_QUOTE_CLOSE = {quote: re.compile(re.escape(quote) + r"|\r|\n") for quote in ('"', "'", '\\"', "\\'")}
_ENDS_CLAUSE = re.compile(rf"(?i)[{_H}]*(?:$|[\r\n.,;!?)]|(?:and|but)(?![A-Za-z0-9]))")


_SECRET_STEMS = ("pass", "pwd", "pin", "secret", "key", "token")


def secret_words(s: str) -> list[re.Match[str]]:
    return _not_address(s, SECRET_WORD.finditer(s))


def secret_value(s: str, word: re.Match[str], ends: "Ends | None" = None) -> tuple[int, int] | None:
    """Where the value after a secret word lies: after ":"/"=" any value; after "is"/"was"/" - " one with a digit or a
    symbol (any, when it is the owner's own and ends its clause); right after the word one with a digit. A value on a
    later line than its label is that whole line, unless quoted; a quote that doesn't close on its line quotes
    nothing."""
    end, strong, mine = word.end(), False, MINE.search(s, max(0, word.start() - 40), word.start()) is not None
    after = SECRET_AFTER.match(s, end)
    next_line = keyed = False
    if after is not None:
        end, strong = after.end(), ":" in after.group("link") or "=" in after.group("link")
        next_line = LINE_BREAK.search(after.group("link")) is not None
        keyed = after.group("link").lstrip("\\")[:1] in ("\"", "'")      # a quoted key: JSON's "password": …
    else:
        direct = SECRET_DIRECT.match(s, end)
        if direct is None:
            return None
        end = direct.end()
    if end >= len(s) or s[end].isspace():
        return None
    quote = s[end] if s[end] in "\"'" else ("\\" + s[end + 1] if s.startswith(("\\\"", "\\'"), end) else "")
    if quote:
        found = _QUOTE_CLOSE[quote].search(s, end + len(quote))
        close = found.start() if found is not None and found.group() == quote else -1
        quote = quote if close >= 0 else ""
    if quote:
        start, stop = end + len(quote), close
    elif next_line:
        line_end = LINE_BREAK.search(s, end)
        start, stop = end, line_end.start() if line_end else len(s)
        while s[stop - 1].isspace():
            stop -= 1
    else:
        start = end
        stop = (ends or Ends(s)).space(end)
        # an unquoted value never runs past a later double quote (JSON text); an unclosed one it starts with stays
        stop = min(stop, (ends or Ends(s)).quote(start + 1))
        if keyed:                                   # a JSON value ends at its delimiter
            stop = next((i for i in range(start, min(stop, start + 4096)) if s[i] in ",}]"), stop)
            if s[start:stop].strip().lower() in ("true", "false", "null"):
                return None
        while stop > start and s[stop - 1] in _TRAILING:
            stop -= 1
        if s.startswith("\\", stop - 1):
            stop -= 1
    length = max(0, stop - start)
    if next_line and not quote:
        if not any(c.isalnum() for c in s[start:stop].replace(REDACTED, "")):
            return None
    elif _ALNUM.search(s, start, stop) is None or s[start:stop] == REDACTED:
        return None
    digit = any(c.isdigit() for c in s[start:stop])
    symbol = _NOT_ALNUM.search(s, start, stop) is not None
    ends_clause = _ENDS_CLAUSE.match(s, stop + len(quote)) is not None
    if strong or (after is not None and ((mine and (digit or symbol or ends_clause))
                                         or (length >= 4 and (digit or symbol)))) or \
            (after is None and length >= 4 and digit):
        return start, stop
    return None


# ---------- all labelled spans of a text ----------

def labelled_spans(s: str, found_urls: list[re.Match[str]] | None = None) -> list[tuple[int, int]]:
    """Where the labelled classes lie in `s`: values after key-form labels and prose secret words, codes with their
    code word, codes alone next to their label line, and backup code lists. A URL's own words never count (its
    credentials are the URL rule's)."""
    url_spans = [m.span() for m in (urls(s) if found_urls is None else found_urls)]

    def in_url(span: tuple[int, int]) -> bool:
        i = bisect.bisect_right(url_spans, (span[0], float("inf"))) - 1
        return i >= 0 and span[1] <= url_spans[i][1]
    ends, lower = Ends(s), s.lower()
    spans = [span for span in key_label_spans(s, ends) if not in_url(span)] if ("=" in s or ":" in s) else []
    if any(map(lower.__contains__, _SECRET_STEMS)):
        for word in secret_words(s):
            if not in_url(word.span()) and (value := secret_value(s, word, ends)) is not None:
                spans.append(value)
    spans += inline_code_spans(s, in_url)
    if ("\n" in s or "\r" in s) and any(map(lower.__contains__, _CODE_STEMS)):
        spans += line_code_spans(s, content_lines(s))
    return spans + (code_list_spans(s) if "codes" in lower else [])


# ---------- fields, answers ----------

PASSWORD_FIELD = re.compile(r"(?i)passw|pwd")
PASSWORD_AUTOCOMPLETE = re.compile(r"(?i)\b(?:current|new)-password\b")
CARD_AUTOCOMPLETE = re.compile(r"(?i)\bcc-(?:number|csc|exp(?:-month|-year)?)\b")
TEXT_FIELD_KINDS = frozenset({"", "text", "password", "textbox", "combobox", "axtextfield", "axtextarea",
                              "axcombobox"})
FIELD_METADATA = ("autocomplete", "name", "id", "label", "placeholder", "title")
INPUT_TYPES = frozenset({"text", "password", "email", "search", "tel", "url", "number", "checkbox", "radio", "button",
                         "submit", "reset", "file", "hidden", "image", "date", "datetime-local", "month", "week",
                         "time", "color", "range", "textarea"})
SIGN_IN_SECRET = re.compile(r"(?i)(?<![A-Za-z0-9])(?:pass(?:word|wd|code|phrase)s?|pwds?|pins?)(?![A-Za-z0-9])")
_DIGITS_ONLY = re.compile(r"[\d\s-]+")


def password_field(kind: str, *metadata: Any) -> bool:
    """A text-entry field of this kind that its metadata names a password field ("Show password" checkboxes stay)."""
    return kind.casefold() in TEXT_FIELD_KINDS and any(
        isinstance(m, str) and PASSWORD_FIELD.search(m) for m in metadata)


def labelled_code(label: str, text: str) -> bool:
    """A code typed or given for a code label: an explicit OTP label hides any digit answer, any code word a
    code-shaped one."""
    value = text.strip()
    return (bool(OTP_LABEL.search(label)) and _DIGITS_ONLY.fullmatch(value) is not None) or (
        bool(code_words(label)) and CODE_VALUE.fullmatch(value) is not None)


def labelled_secret(kind: str, label: str, text: str) -> bool:
    """A text field a secret word (or a secret label of the vocabulary) names: its value is that secret, whatever its
    shape. Only a record that names a text-entry kind, so a "Pin tab" checkbox keeps its state."""
    return (bool(text.strip()) and bool(kind) and kind.casefold() in TEXT_FIELD_KINDS
            and (bool(secret_words(label)) or label_class(label) == "secret"))


def hidden_answer(question: str, text: str, *, reply: bool) -> bool:
    """An answer hidden whole: to a question with a password, passcode or passphrase word or a sign-in PIN, whatever
    its shape; to an explicit one-time code question, any digits. A chat reply while the question is open may be a
    steer (L3): hidden whole for a password question only when it is one word."""
    value = text.strip()
    if bool(OTP_LABEL.search(question)) and _DIGITS_ONLY.fullmatch(value) is not None:
        return True
    asked = bool(_not_address(question, SIGN_IN_SECRET.finditer(question)))
    return asked and (not reply or len(value.split()) == 1)
