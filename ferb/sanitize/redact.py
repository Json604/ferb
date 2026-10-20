"""Own secret redaction before model, trace and log boundaries. Must not use secret content to decide actions."""

import bisect
import codecs
import re
from typing import Any, Callable
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

REDACTED = "[redacted]"
EXCLUDED = "[excluded]"

# Key names whose values are secrets. Matched per word ("session_id" -> session, id; "input_tokens" -> input, tokens).
SECRET_WORDS = {"password", "passwd", "pwd", "passphrase", "secret", "token", "cookie", "otp", "session",
                "credential", "credentials", "authorization", "auth", "apikey", "bearer"}
SECRET_KEYS = {"apikey", "accesstoken", "refreshtoken", "idtoken", "clientsecret", "privatekey", "setcookie",
               "onetimecode", "xapikey"}
# FERB's own id keys: their values are ids FERB built, never token values (owner decision; L16, L28). Under these keys a
# registered value is replaced wherever it occurs in the whole value, except where it lies strictly inside one generated
# id body (ID_BODY): a short registered value can occur inside an id by chance, and an altered id breaks lineage. A hit
# equal to a body or reaching past one is redacted, and the secret patterns run over the whole value. Accepted residual:
# a registered value that is a strict substring of a longer generated body is not replaced there.
# approval_id and timer_id (owner questions and reminders) are FERB ids that clients answer by and code reads back.
INTERNAL_ID_KEYS = {"task_id", "span_id", "session_id", "run_token_id", "request_id", "covered", "source_refs",
                    "procedure_id", "approval_id", "timer_id"}
# The generated part of a FERB id: a uuid4 hex (RandomIds: task, request, journal, trail, memory ids), a sha256 hex
# (learning nodes, m-/key-) or a 6-byte token hex (chat sessions, chat-).
ID_BODY = re.compile(r"(?<![0-9A-Za-z])([0-9a-f]{64}|[0-9a-f]{32}|[0-9a-f]{12})(?![0-9A-Za-z])")

SECRET_PATTERNS = [
    re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----"),
    re.compile(r"\bsk-ant-[A-Za-z0-9_-]{16,}"),
    re.compile(r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}"),
    re.compile(r"\bgh[pousr]_[A-Za-z0-9]{20,}"),
    re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}"),
    re.compile(r"\bxox[abprs]-[A-Za-z0-9-]{10,}"),
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(r"\bAIza[0-9A-Za-z_-]{35}\b"),
    re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}"),
    re.compile(r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{12,}"),
]
URL = re.compile(r"\b[a-zA-Z][a-zA-Z0-9+.-]*://[^\s\"'<>]+")
SECRET_QUERY = {"token", "code", "key", "sig", "signature", "auth", "session", "access_token", "id_token",
                "refresh_token", "password", "secret", "otp", "apikey", "api_key", "client_secret", "sessionid"}
# Signed / pre-authorized URL credentials (S3 X-Amz-*, GCS X-Goog-*, Azure SAS `sig`, CDN `token`/`hmac` params): a
# parameter whose name holds one of these words is a secret class (rule 14), whatever the service prefixes it with.
SECRET_QUERY_WORDS = ("signature", "token", "credential", "secret", "password", "apikey", "api_key", "hmac")


# Pagination cursors name "token" but are no credential (rule 14 covers credentials): kept readable.
CURSOR_PARAMS = {"pagetoken", "nextpagetoken", "continuationtoken", "synctoken", "cursor"}


def secret_param(name: str) -> bool:
    k = name.lower()
    if "".join(ch for ch in k if ch.isalnum()) in CURSOR_PARAMS:
        return False
    flat = "".join(ch for ch in k if ch.isalnum())
    return (k in SECRET_QUERY or any(w in k or w.replace("_", "") in flat for w in SECRET_QUERY_WORDS)
            or flat.endswith("sig") or flat in {"xamzcredential", "xgoogcredential"})
# Explicit credential-field metadata, never broad login/security/access language in an ordinary owner question.
OTP_LABEL = re.compile(r"(?i)\b(?:otp|totp|2fa|one[ _-]?time(?:[ _-]+(?:code|password|pin))?|"
                       r"(?:verification|authentication|security|login|sign[ _-]?in|access)[ _-]+(?:code|pin))\b")
# A password field, also while a site's "Show password" makes it plain text (type=text, AXTextField): its own
# metadata says so (autocomplete current-password/new-password, or a name, id, label, placeholder or title holding
# "passw" or "pwd"). A secret class at the capture boundary like OTP_LABEL, never a judgement of intent. The name and
# label match covers text fields only: an input of type text, the AX text roles and their mac snapshot kinds
# (textbox, combobox), or "" when a record names no kind (then only its value). A checkbox, a menu item or an email
# field ("Show password", "Passwords", "…reset your password") stays readable. A password autocomplete marks an
# input of any type (a tel/number PIN).
PASSWORD_FIELD = re.compile(r"(?i)passw|pwd")
PASSWORD_AUTOCOMPLETE = re.compile(r"(?i)\b(?:current|new)-password\b")
TEXT_FIELD_KINDS = frozenset({"", "text", "password", "textbox", "combobox", "axtextfield", "axtextarea",
                              "axcombobox"})
FIELD_METADATA = ("autocomplete", "name", "id", "label", "placeholder", "title")
# HTML input types (and textarea): a record's `type` names its field kind only when it is one of these.
INPUT_TYPES = frozenset({"text", "password", "email", "search", "tel", "url", "number", "checkbox", "radio", "button",
                         "submit", "reset", "file", "hidden", "image", "date", "datetime-local", "month", "week",
                         "time", "color", "range", "textarea"})


def password_field(kind: str, *metadata: Any) -> bool:
    """A text-entry field of this kind (its role or input type) that its metadata names a password field."""
    return kind.casefold() in TEXT_FIELD_KINDS and any(isinstance(m, str) and PASSWORD_FIELD.search(m)
                                                        for m in metadata)


# C1 (owner decision 2026-10-15, rule 14): a one-time code is a 4-8 digit run with a code word a few words away on
# either side, and a value next to a secret word is masked whatever its shape. Labelled by the words providers and
# people write, never by digit shape alone: a number with no code or secret word near it stays visible.
# The owner's code words (code, OTP, passcode, PIN, one-time password, verification); never part of a file name
# ("tests/test_otp.py").
CODE_WORD = re.compile(r"(?i)(?<![A-Za-z0-9])(?:one[ _-]?time[ _-]?(?:password|passcode|code|pin)|otp|totp|2fa|"
                       r"pass[ _-]?code|pin|codes?|verification)(?![A-Za-z0-9]|\.[A-Za-z0-9])")
# The code itself: 4-8 digits, grouped by spaces or hyphens (482-913, 48 29 13), or Google's G-482913. Never part of a
# longer token or digit group (an id, a phone number); a JSON escape before it still separates it.
# A masked code counts as the digits it replaced, so masking twice changes nothing.
CODE_VALUE = re.compile(rf"(?:(?<![0-9A-Za-z_-])|(?<=\\[ntrbf])|(?<=\\u[0-9a-fA-F]{{4}}))(?<!\d[ -])(?<!\[redacted\][ -])"
                        rf"(?:[A-Z]-)?\d(?:[ -]?\d){{3,7}}(?![0-9A-Za-z_]|[ -]\d|[ -]\[redacted\])")
# Each relation lies on one line, except a label or a secret word ending its line with its value on the next one.
# A code before its word is joined to it the way providers write it: "482913 is your code", "739201 as your PIN",
# "739201 (OTP)". A bare "2024 code review" has no value: nothing joins the number to the word; "1500 to check the
# OTP", "issue 4821 - code freeze" and "Line 1234: code smell" don't join either.
CODE_LINK = re.compile(r"(?i)[ \t]*(?:(?:is|are|was|as)(?![A-Za-z0-9])|\()")
# Owner decision 2026-10-15: address and coupon codes are ordinary data. A code word or "PIN" inside one of these is
# no code or secret word: "PIN code 560001", "pincode", "ZIP code", "postal_code", "coupon code", "promo code".
_PLACE = r"(?:pin|zip|postal|post|area|coupon|promo(?:tional)?|discount|voucher)"
# "Pin/Zip code" names the address code once for both words (re-review 2, R3)
ADDRESS_CODE = re.compile(rf"(?i)(?<![A-Za-z0-9])(?:{_PLACE}(?:[ \t]*/[ \t]*{_PLACE})*[ _-]?codes?|pincodes?|postcodes?|"
                          r"zipcodes?)(?![A-Za-z0-9])")


def _not_address(s: str, found: Any) -> list[re.Match[str]]:
    """The matches of a word class that don't start inside an address or coupon code."""
    spans = [m.span() for m in ADDRESS_CODE.finditer(s)]
    return [m for m in found if not any(a <= m.start() < b for a, b in spans)]


def code_words(s: str) -> list[re.Match[str]]:
    return _not_address(s, CODE_WORD.finditer(s))


def secret_words(s: str) -> list[re.Match[str]]:
    return _not_address(s, SECRET_WORD.finditer(s))


# A sentence or clause break: a code never reaches its word across one ("Order 4821 is out. Share OTP 7731").
# ";", "!", "?" and a comma before a space always break; a dot only when a new sentence follows (a space and a capital,
# or the end) and the word before it isn't abbreviation-shaped (1-3 letters, or "a/c"): "Rs. 2,000", "a/c no. XX1234",
# "Ref. No. 4417" keep the bank's OTP in reach (re-review 2, C2).
_HARD_BREAK = re.compile(r"[;!?](?=\s|$)|,(?=\s)")
_SENTENCE_DOT = re.compile(r"\.(?=\s+[A-Z]|\s*$)")
_ABBREVIATION = re.compile(r"[A-Za-z]{1,3}|[A-Za-z]/[A-Za-z]")


def _clause_break(s: str, a: int, b: int) -> bool:
    """Whether a sentence or clause ends between positions a and b of s."""
    if _HARD_BREAK.search(s, a, b):
        return True
    for dot in _SENTENCE_DOT.finditer(s, a, b):
        before = re.search(r"(\S+)$", s[max(0, dot.start() - 24):dot.start()])
        if before is None or not _ABBREVIATION.fullmatch(before.group(1)):
            return True
    return False
# A code after its word comes right after it ("OTP 482913", "Code: 482913"), after a link on the same line ("… is
# 482913", "for login: 482913") or in parentheses ("code (482913)"): never after another word, so "code review on 12
# Oct 2024" and "Code (3733 recorded uses)" keep their numbers. Across lines, only a code alone on its line relates to
# the label line next to it (`_line_codes`).
LABEL_DIRECT = re.compile(r"[ \t:=#\-–—]*")
LABEL_LINK = re.compile(r"(?i)(?:(?<![A-Za-z0-9])(?:is|are|was)|[:=#\-–—])[ \t]*$")
LINE_BREAK = re.compile(r"\r\n|\r|\n")
SECRET_WORD = re.compile(r"(?i)(?<![A-Za-z0-9])(?:pass(?:word|wd|phrase|code)|pwd|pin|secret(?:[ _-]?key)?|"
                         r"api[ _-]?key|(?:(?:access|auth|api|refresh)[ _-]?)?token|private[ _-]?key)"
                         r"(?![A-Za-z0-9])")
# What follows a secret word: an optional "for gmail" / "to the wifi", a link (":", "=", "is", "was", " - ") or none.
SECRET_AFTER = re.compile(r"""(?ix)(?P<qual>[ \t]+(?:for|to|of|on|at)[ \t]+(?:the[ \t]+|my[ \t]+|your[ \t]+)?[^\s:=]+)?
                              (?P<link>(?:\\?["'])?[ \t]*[:=][ \t]*(?:\r\n|\r|\n)?[ \t]*|
                                       [ \t]+(?:is|was)(?![A-Za-z0-9])[ \t]*:?[ \t]*|[ \t]+[-–—][ \t]+)""")
SECRET_DIRECT = re.compile(r"[ \t]+")
# The owner's own: "my password is sunshine" hides a plain word too (a page's "Your password is incorrect" doesn't).
MINE = re.compile(r"(?i)(?<![A-Za-z0-9])(?:my|our)[ \t]+(?:[A-Za-z-]+[ \t]+)?$")
# "A few words": how many words may lie between a code word and its code. Eight reaches the farthest code in the
# owner's provider corpus ("Your OTP for HDFC Bank txn of Rs 2,000 is 482913").
CODE_REACH = 8
_TRAILING = ".,;:!?)]}\"'`"


def _word_starts(s: str) -> list[int]:
    return [m.start() for m in re.finditer(r"\S+", s)]


def _words_between(starts: list[int], a: int, b: int) -> int:
    return bisect.bisect_left(starts, b) - bisect.bisect_left(starts, a)


def _secret_value(s: str, word: re.Match[str]) -> tuple[int, int] | None:
    """Where the value after a secret word lies, when one is there: after ":"/"=" any value, after "is"/"was"/" - "
    one with a digit or a symbol (or any, when it is the owner's own), and right after the word one with a digit."""
    end, strong, mine = word.end(), False, MINE.search(s, max(0, word.start() - 40), word.start()) is not None
    after = SECRET_AFTER.match(s, end)
    if after is not None:
        end, strong = after.end(), ":" in after.group("link") or "=" in after.group("link")
    else:
        direct = SECRET_DIRECT.match(s, end)
        if direct is None:
            return None
        end = direct.end()
    if end >= len(s) or s[end].isspace():
        return None
    quote = s[end] if s[end] in "\"'" else ("\\" + s[end + 1] if s.startswith(("\\\"", "\\'"), end) else "")
    if quote:
        close = s.find(quote, end + len(quote))
        if close < 0 or "\n" in s[end:close]:
            return None
        start, stop = end + len(quote), close
    else:
        stop = end
        while stop < len(s) and not s[stop].isspace():
            stop += 1
        start = end
        while stop > start and s[stop - 1] in _TRAILING:
            stop -= 1
        if s.startswith("\\", stop - 1):
            stop -= 1
    value = s[start:stop]
    if not any(c.isalnum() for c in value) or REDACTED[:-1] in s[end:stop + 1]:   # no value; already masked
        return None
    digit = any(c.isdigit() for c in value)
    symbol = any(not c.isalnum() for c in value)
    # "my password is sunshine" hides a plain word that ends its clause; "my PIN is on the card" is no value
    ends_clause = re.match(r"(?i)[ \t]*(?:$|[\r\n.,;!?)]|(?:and|but)(?![A-Za-z0-9]))", s[stop:]) is not None
    if strong or (after is not None and ((mine and (digit or symbol or ends_clause))
                                         or (len(value) >= 4 and (digit or symbol)))) or \
            (after is None and len(value) >= 4 and digit):
        return start, stop
    return None


def labelled_secret_spans(s: str) -> list[tuple[int, int]]:
    """Where the C1 classes lie in `s`: codes with their code word near, and values next to a secret word. A URL's
    own words never count (its query credentials are the URL rule's)."""
    urls = [m.span() for m in URL.finditer(s)]

    def in_url(span: tuple[int, int]) -> bool:
        return any(a <= span[0] and span[1] <= b for a, b in urls)
    spans: list[tuple[int, int]] = []
    words = [m for m in secret_words(s) if not in_url(m.span())]
    for word in words:
        value = _secret_value(s, word)
        if value is not None:
            spans.append(value)
    codes = [m for m in CODE_VALUE.finditer(s) if not in_url(m.span())]
    labels = [m.span() for m in code_words(s) if not in_url(m.span())] if codes else []
    if labels:
        starts, ends, begins = _word_starts(s), [b for _, b in labels], [a for a, _ in labels]
        for code in codes:
            before = bisect.bisect_right(ends, code.start()) - 1
            after = bisect.bisect_left(begins, code.end())
            if (before >= 0 and _words_between(starts, ends[before], code.start()) <= CODE_REACH
                    and LINE_BREAK.search(s, ends[before], code.start()) is None
                    and not _clause_break(s, ends[before], code.start())
                    and _label_joined(s, ends[before], code)) or (
                    after < len(labels) and CODE_LINK.match(s, code.end()) is not None
                    and LINE_BREAK.search(s, code.end(), begins[after]) is None
                    and not _clause_break(s, code.end(), begins[after])
                    and _words_between(starts, code.end(), begins[after]) <= CODE_REACH):
                spans.append(code.span())
    return spans + _line_codes(s) + _code_list(s)


# A list of backup or recovery codes ("Backup codes: 1234 5678 9012 3456", or one per line): every code in it.
# Only a backup, recovery, 2FA or one-time code list (re-review 2, I5): "HTTP status codes: 200, 301" is ordinary data.
CODE_LIST = re.compile(r"(?i)(?<![A-Za-z0-9])(?:backup|recovery|2fa|two[ -]?factor|one[ -]?time|verification|security|"
                       r"login)(?:[ \t]+backup)?[ \t]+codes[ \t]*:[ \t]*")
# A list label's qualifier with at most two words after it ending what has been read: "codes" can still follow it
# ("Two ", "Two factor ", "security backup co"); once "codes" is there, the code word holds the line (re-review 3,
# NEW-I1). Past two words no list label can form from it.
_LIST_QUALIFIER_END = re.compile(r"(?i)(?<![A-Za-z0-9])(?:backup|recovery|2fa|two|one|verification|security|login)"
                                 r"[A-Za-z0-9]*(?:[ \t_-]+[A-Za-z0-9]*){0,2}[ \t]*$")
LIST_TOKEN = re.compile(r"(?=[A-Za-z0-9-]*\d)[A-Za-z0-9]{3,10}(?:-[A-Za-z0-9]{3,10})?(?![A-Za-z0-9-])")
_LIST_GAP = re.compile(r"[ \t,]*(?:(?:\r\n|\r|\n)[ \t]*)?")


def _code_list(s: str) -> list[tuple[int, int]]:
    """Each code after a list label; across a line break only onto a line made only of codes."""
    spans = []
    for label in _not_address(s, CODE_LIST.finditer(s)):
        pos = label.end()
        while True:
            gap = _LIST_GAP.match(s, pos)
            assert gap is not None                  # it matches the empty string
            token = LIST_TOKEN.match(s, gap.end())
            if token is None:
                break
            if LINE_BREAK.search(gap.group()):
                line_end = LINE_BREAK.search(s, token.start())
                if not _list_line(s[token.start():line_end.start() if line_end else len(s)]):
                    break
            spans.append(token.span())
            pos = token.end()
    return spans


def _list_label_line(line: str) -> bool:
    """A line that starts a list: its label, then only codes ("Backup codes: 1234 5678")."""
    labels = _not_address(line, CODE_LIST.finditer(line))
    return bool(labels) and (not line[labels[-1].end():].strip(" \t,") or _list_line(line[labels[-1].end():]))


def _list_line(line: str) -> bool:
    """A line of only list codes ("4821-9921", "a1b2c-3d4e5", ", 482-913"), as in a recovery-code list."""
    tokens = [t for t in re.split(r"[\s,]+", line) if t]
    return bool(tokens) and all(LIST_TOKEN.fullmatch(t) for t in tokens)


def _line_codes(s: str) -> list[tuple[int, int]]:
    """A code alone on its line with a label line just above it ("Your verification code is:", blank lines between
    allowed) or a line just below that names it ("482913" over "is your verification code"): an email's layout."""
    lines, begin = [], 0
    for m in [*LINE_BREAK.finditer(s), None]:
        end = m.start() if m is not None else len(s)
        if s[begin:end].strip():
            lines.append((begin, end))
        begin = m.end() if m is not None else len(s)
    spans = []
    for i, (a, b) in enumerate(lines):
        line = s[a:b]
        if not _code_alone(line):
            continue
        above = s[lines[i - 1][0]:lines[i - 1][1]] if i > 0 else ""
        below = s[lines[i + 1][0]:lines[i + 1][1]] if i + 1 < len(lines) else ""
        if _label_line(above) or _link_line(below):
            code = CODE_VALUE.search(line)
            if code is not None:
                spans.append((a + code.start(), a + code.end()))
    return spans


def _label_joined(s: str, label_end: int, code: re.Match[str]) -> bool:
    gap = s[label_end:code.start()]
    if LABEL_DIRECT.fullmatch(gap):
        return True
    return LINE_BREAK.search(gap) is None and (LABEL_LINK.search(gap) is not None or (
        gap.rstrip(" \t").endswith("(") and s.startswith(")", code.end())))


def _mask_spans(s: str, spans: list[tuple[int, int]]) -> str:
    out, kept = [], 0
    for start, end in sorted(spans):
        if start < kept:
            continue
        out.append(s[kept:start] + REDACTED)
        kept = end
    return "".join(out) + s[kept:]


def _labelled_code(label: str, text: str) -> bool:
    """A code typed or given for a code label: an explicit OTP label hides any digit answer (as before), and any
    code word hides a code-shaped one (C1)."""
    value = text.strip()
    return (bool(OTP_LABEL.search(label)) and re.fullmatch(r"[\d -]+", value) is not None) or (
        bool(code_words(label)) and CODE_VALUE.fullmatch(value) is not None)


# A password, passcode or PIN a question asks for (signing in, paying); never an address PIN code.
SIGN_IN_SECRET = re.compile(r"(?i)(?<![A-Za-z0-9])(?:password|passcode|pin)(?![A-Za-z0-9])")
# A question asks for the secret itself when its word ends the phrase: then punctuation, the end, or "for/to/of/on/at"
# (the qualifiers SECRET_AFTER takes) or "is/was" ("What's your Gmail password?", "the passcode for the door"); in
# "Which password manager do you use?" or "the password reset page" the word names something else.
SECRET_ASKED = re.compile(r"(?i)[ \t]*(?:[?.!:,;)\]]|$)|[ \t]+(?:for|to|of|on|at|is|was)(?![A-Za-z0-9])")


def _hidden_answer(question: str, text: str, *, reply: bool) -> bool:
    """An answer hidden whole (rule 14, re-review 2 R1): to a question asking for a sign-in password, passcode or PIN
    (never an address PIN code; `SECRET_ASKED`), whatever its shape; to an explicit one-time code question, any digits. A chat reply while
    the question is open may be a steer (L3): it is hidden whole for a password question only with a password's shape,
    one word."""
    value = text.strip()
    if bool(OTP_LABEL.search(question)) and re.fullmatch(r"[\d -]+", value) is not None:
        return True
    asked = any(SECRET_ASKED.match(question, word.end()) for word in _not_address(question, SIGN_IN_SECRET.finditer(question)))
    return asked and (not reply or len(value.split()) == 1)


def _labelled_secret(kind: str, label: str, text: str) -> bool:
    """A text field a secret word names (API key, token, PIN, secret): its value is that secret, whatever its shape
    (C1). Only a record that names a text-entry kind, so a "Pin tab" checkbox keeps its state and a record naming no
    kind (a chat channel called "secret-santa") keeps its text, as with the password rule."""
    return (bool(text.strip()) and bool(kind) and kind.casefold() in TEXT_FIELD_KINDS
            and bool(secret_words(label)))


def _code_alone(line: str) -> bool:
    """A line that is only a code (an email's code on its own line), with its punctuation."""
    return CODE_VALUE.fullmatch(line.strip().strip(".,;:()[]")) is not None


def _label_line(line: str) -> bool:
    """A line that labels the code under it: a code word within a few words of its end, with no sentence ending in
    between ("Your verification code is:", "Use this code to sign in", "Sign-in code"). A line break is whitespace, so
    the owner's "code word within a few words" reaches the next line (re-review 2, R2)."""
    words = code_words(line)
    if not words:
        return False
    text = line.rstrip().rstrip(".:=#-–—").rstrip()
    end = words[-1].end()
    return len(line[end:].split()) <= CODE_REACH and (end >= len(text) or not _clause_break(text, end, len(text)))


def _link_line(line: str) -> bool:
    """A line that names the code above it: it starts with a link and holds a code word ("is your verification
    code")."""
    found = code_words(line)
    word = found[0] if found else None
    return (CODE_LINK.match(line) is not None and word is not None and not _clause_break(line, 0, word.start())
            and len(line[:word.start()].split()) <= CODE_REACH)


def _secret_colon_end(line: str) -> bool:
    """A line ending in a secret word and its ":" or "=" ("Password:", "password for gmail:"), whose value may be on
    the next line."""
    for word in secret_words(line):
        after = SECRET_AFTER.match(line, word.end())
        if after is not None and after.end() >= len(line.rstrip()) and re.search("[:=]", after.group("link")):
            return True
    return False


# The first word of a two-word class word, or "my"/"our", ending what has been read of a line: it can still become one
# ("api " then "key 482-913", "my wifi " then "password is …").
_PREFIX_END = re.compile(r"(?i)(?:(?<![A-Za-z0-9])(?:my|our)(?:[ \t]+[A-Za-z-]+){0,2}|(?<![A-Za-z0-9])(?:api|private|"
                         r"access|auth|refresh|one|time|pass|backup|recovery|secret|sign|login|security|authentication|"
                         r"coupon|zip|postal|post|promo|promotional|discount|voucher|area)[ \t_/-]*\S*)[ \t]*$")


def _line_starts(pending: str) -> tuple[list[int], int]:
    """The ends of the complete lines of what has been read, and where the line still being read starts. A "\r" that
    ends the text may be the first half of "\r\n" (PTY output), so its line isn't complete yet (re-review 2, C1)."""
    breaks = [m.end() for m in LINE_BREAK.finditer(pending)]
    if breaks and breaks[-1] == len(pending) and pending.endswith("\r"):
        breaks.pop()
    return breaks, breaks[-1] if breaks else 0


# Streaming retains a possible introducer until its line is complete. This only buffers bytes: the complete
# text is still redacted by the narrow credential patterns, so 'login phone number' remains ordinary text.
OTP_CONTEXT_START = re.compile(r"(?i)\b(?:one[ _-]?time|verification|authentication|security|login|sign[ _-]?in|access|2fa|otp|totp)\b")
# Its code is never part of a longer digit group (a phone number: "a verification code to +91 98765 43210").
# Its gap never reaches over a code already masked, and a "G-" prefixed code is C1's whole.
OTP_TEXT = re.compile(r"(?i)(\b(?:one[ -]?time(?: password| code)?|verification code|authentication code|security code|login code|sign[ -]?in code|otp)\b(?:(?!\[redacted\])[^\d\r\n]){0,40})(?<![+\d])(?<!\d[ -])(?<![A-Za-z]-)(?<!\[redacted\][ -])(\d(?:[ -]?\d){3,7})(?![ -]?\d|[ -]\[redacted\])")
# A card number is a token of its own, never a digit run inside a longer hex/alphanumeric token such as an internal
# id (m-8e953d2b8ca8f…, L16); a hyphen joining it to such a token doesn't free it. A JSON escape before it (\n, \t,
# \r, \b, \f or \uXXXX in a redacted JSON text) still separates it from the word before.
_ID_CHAR = "[0-9A-Za-z_]"
CARD = re.compile(rf"(?:(?<!{_ID_CHAR})|(?<=\\[ntrbf])|(?<=\\u[0-9a-fA-F]{{4}}))(?<!{_ID_CHAR}-)"
                  rf"(?:\d[ -]?){{12,18}}\d(?!{_ID_CHAR}|-{_ID_CHAR})")


def _url_id_spans(s: str) -> list[tuple[int, int]]:
    """Where each URL's scheme, host and path lie in `s` (its query and fragment are not part of these spans). The
    URL pattern also runs on through , | ) or a JSON \\n, so only digits that start a path segment or host label
    there are an id (a post, a message, a snowflake), never a credential."""
    spans = []
    for m in URL.finditer(s):
        tail = re.search(r"[?#]", m.group())
        spans.append((m.start(), m.start() + tail.start() if tail else m.end()))
    return spans


def payment_card(value: str) -> bool:
    digits = [int(c) for c in value if c.isdigit() and c.isascii()]
    if not 13 <= len(digits) <= 19 or len(set(digits)) == 1:
        return False
    return sum((d * 2 - 9 if d * 2 > 9 else d * 2) if i % 2 else d
               for i, d in enumerate(reversed(digits))) % 10 == 0


INPUT_TEXT_FIELDS = {"text", "unmodifiedText", "key", "code"}
# Where a page or accessibility node holds its visible text, in order of preference (sibling codes, C1).
NODE_TEXT = ("value", "text", "name", "label", "title", "content")


def _words(key: str) -> list[str]:
    spaced = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", key)
    return [w for w in re.split(r"[^A-Za-z0-9]+", spaced.lower()) if w]


def secret_key(key: str) -> bool:
    if key in INTERNAL_ID_KEYS:
        return False
    words = _words(key)
    return "".join(words) in SECRET_KEYS or any(w in SECRET_WORDS for w in words)


class Redactor:
    def __init__(self, exclude_domains: set[str] | None = None, exclude_apps: set[str] | None = None) -> None:
        self.configure(exclude_domains, exclude_apps)
        self._values: set[str] = set()      # exact secret values; memory only, never persisted
        self._temporary: dict[str, int] = {}  # short/space-sensitive hidden inputs, reference-counted leases

    def retain_secret(self, value: str) -> None:
        if value:
            self._temporary[value] = self._temporary.get(value, 0) + 1

    def remember_hidden_secret(self, value: str) -> None:
        """A sealed secret (entered through the hidden secret input, or a stored one revealed for a sealed fill) is
        a secret even when shorter than pattern-derived credentials. Only those call this (L18): a typed or picked
        answer to an ordinary question never does, so a "1" can't turn every later 1 into [redacted]."""
        if value:
            self._values.add(value)

    def release_secret(self, value: str) -> None:
        count = self._temporary.get(value, 0)
        if count <= 1:
            self._temporary.pop(value, None)
        else:
            self._temporary[value] = count - 1

    def secret_values(self) -> set[str]:
        return self._values | self._temporary.keys()

    def configure(self, exclude_domains: set[str] | None = None, exclude_apps: set[str] | None = None) -> None:
        """Change the exclusions in place: the journal, spans and gateway hold this instance, so it must stay one."""
        self.exclude_domains = {d.lower().lstrip(".") for d in (exclude_domains or set())}
        self.exclude_apps = {a.lower() for a in (exclude_apps or set())}

    def add_secret_value(self, value: str) -> None:
        """E.g. a password fill_secret just typed: scrub it if a page or error echoes it back."""
        if len(value) >= 4:
            self._values.add(value)

    def forget_secret_value(self, value: str) -> None:
        self._values.discard(value)

    def __call__(self, obj: Any) -> Any:
        return self._walk(obj)

    def text(self, s: str) -> str:
        """For exception messages and any free text."""
        return self._str(s)

    def ident(self, s: str) -> str:
        """A value under one of FERB's own id keys (INTERNAL_ID_KEYS). An excluded app's name stays excluded."""
        if self.exclude_apps and s.lower() in self.exclude_apps:
            return EXCLUDED
        bodies = [m.span(1) for m in ID_BODY.finditer(s)]
        hits = []
        for value in self.secret_values():
            start = s.find(value)
            while value and start >= 0:
                end = start + len(value)
                if not any(b0 <= start and end <= b1 and (start, end) != (b0, b1) for b0, b1 in bodies):
                    hits.append((start, end))
                start = s.find(value, start + 1)
        out, kept = [], 0
        for start, end in sorted(hits):
            if start > kept or not out:
                out.append(s[kept:start] + REDACTED)
            kept = max(kept, end)
        return self._patterns("".join(out) + s[kept:])

    def field(self, text: str, *, role: str = "", label: str = "", secure_input: bool = False,
              concealed: bool = False, metadata: tuple[str, ...] = (), kind: str | None = None) -> str:
        """Capture boundary for AX/DOM/clipboard fields; metadata never grants action permission. `kind` is the
        field's input type or role for the password rule (its role when not given); none named, no password rule."""
        kind = role if kind is None else kind
        if (secure_input or concealed or "secure" in role.casefold() or role.casefold() == "password"
                or bool(kind) and password_field(kind, label, *metadata) or _labelled_code(label, text)
                or _labelled_secret(kind, label, text)):
            return REDACTED if text else text
        return self._str(text)

    def answer(self, text: str, *, question: str, reply: bool = False) -> str:
        """The owner's typed answer to a question, which is no field (rule 14, re-review 2 R1): to a question about a
        sign-in password, passcode or PIN it is hidden whole ("hunter2!"); to a question with a code word (code, OTP,
        passcode, PIN, one-time password, verification; never an address or coupon code) its 4-8 digit codes are
        masked. `reply`: a chat message while the question is open (`_hidden_answer`)."""
        if not text.strip():
            return text
        if _hidden_answer(question, text, reply=reply):
            return REDACTED
        out = self._str(text)
        return CODE_VALUE.sub(REDACTED, out) if code_words(question) else out

    def excluded_host(self, host: str) -> bool:
        """A host on the owner's exclusion list (a domain covers its subdomains)."""
        host = host.lower()
        return any(host == d or host.endswith("." + d) for d in self.exclude_domains)

    def stream(self) -> "RedactionStream":
        """A separate decoder/buffer for each process; raw partial secrets never leave it."""
        return RedactionStream(self)

    # ---------- internals ----------

    def _walk(self, obj: Any, key: str | None = None) -> Any:
        if isinstance(obj, dict):
            return self._dict(obj)
        if isinstance(obj, (list, tuple)):
            return self._siblings(list(obj), [self._walk(v) for v in obj])
        if isinstance(obj, str):
            return self._str(obj)
        return obj

    def _siblings(self, raw: list[Any], out: list[Any]) -> list[Any]:
        """A code split from its label across sibling nodes (a page's "Your verification code" heading and the code in
        the next node, C1): the siblings' texts read as lines, as an email's are (`_line_codes`)."""
        keys: list[str | None] = []
        lines: list[str] = []
        for item in raw:
            key = next((k for k in NODE_TEXT if isinstance(item, dict) and isinstance(item.get(k), str)
                        and item[k].strip()), None)
            keys.append(key)
            lines.append(item if isinstance(item, str) else str(item[key]) if key else "")
        if len(raw) < 2 or not any(_code_alone(line) for line in lines if line):
            return out
        joined, starts = "", []
        for line in lines:
            starts.append(len(joined))
            joined += " ".join(line.split()) + "\n"
        for a, _ in _line_codes(joined):
            i = bisect.bisect_right(starts, a) - 1
            if isinstance(out[i], str):
                out[i] = REDACTED
            elif keys[i] is not None and isinstance(out[i], dict):
                out[i] = {**out[i], keys[i]: REDACTED}
        return out

    def _dict(self, d: dict[Any, Any]) -> dict[Any, Any]:
        kind_type = str(d.get("type") or "")
        kind = str(d.get("input_type") or (kind_type if kind_type.lower() in INPUT_TYPES else "")   # an input type
                   or d.get("role") or "")                                                          # before a role
        autocomplete = str(d.get("autocomplete", ""))
        secret_field = (str(d.get("type", "")).lower() == "password" or "one-time-code" in autocomplete.lower()
                        or PASSWORD_AUTOCOMPLETE.search(autocomplete) is not None
                        or password_field(kind, *(d.get(k) for k in FIELD_METADATA)))
        input_event = isinstance(d.get("method"), str) and d["method"].startswith("Input.")
        out: dict[Any, Any] = {}
        for k, v in d.items():
            ks = str(k)
            if secret_key(ks) and v not in (None, "", [], {}):
                out[k] = REDACTED
            elif ks in INTERNAL_ID_KEYS and isinstance(v, (str, list, tuple)):
                out[k] = self.ident(v) if isinstance(v, str) else [
                    self.ident(item) if isinstance(item, str) else self._walk(item) for item in v]
            elif secret_field and ks == "value" and v not in (None, ""):
                out[k] = REDACTED
            elif input_event and ks == "params" and isinstance(v, dict):
                out[k] = {pk: (REDACTED if pk in INPUT_TEXT_FIELDS else self._walk(pv)) for pk, pv in v.items()}
            else:
                out[k] = (self.field(v, role=str(d.get("role", d.get("type", ""))), kind=kind,
                                     label=str(d.get("label", d.get("name", d.get("autocomplete", "")))))
                          if ks in {"value", "text", "content"} and isinstance(v, str) else self._walk(v, ks))
        return out

    def _str(self, s: str) -> str:
        if self.exclude_apps and s.lower() in self.exclude_apps:
            return EXCLUDED
        return self._patterns(self._registered(s))

    def _registered(self, s: str) -> str:
        """Every registered secret value replaced (longest first)."""
        for v in sorted(self.secret_values(), key=len, reverse=True):
            if v in s:
                s = s.replace(v, REDACTED)
        return s

    def _patterns(self, s: str) -> str:
        """The secret classes: key and token patterns, payment cards, one-time codes, URL credentials."""
        s = _before_labels(s)
        s = _mask_spans(s, labelled_secret_spans(s))
        return URL.sub(lambda m: self._url(m.group(0)), s)

    def before_labels(self, s: str) -> str:
        """`s` as the labelled classes read it, after the masks applied before them (a stream line's relations)."""
        return _before_labels(self._registered(s))

    def _url(self, url: str) -> str:
        try:
            parts = urlsplit(url)
        except ValueError:
            return url
        if self.excluded_host(parts.hostname or ""):
            return EXCLUDED
        netloc = parts.netloc
        if parts.password is not None:
            netloc = netloc.replace(f":{parts.password}@", f":{REDACTED}@", 1)
        query = parts.query
        if query:
            pairs = parse_qsl(query, keep_blank_values=True)
            if any(secret_param(k) for k, _ in pairs):
                query = urlencode([(k, REDACTED if secret_param(k) else v) for k, v in pairs], safe="[]")
        fragment = parts.fragment
        if fragment and "=" in fragment:
            pairs = parse_qsl(fragment, keep_blank_values=True)
            if any(secret_param(k) for k, _ in pairs):
                fragment = urlencode([(k, REDACTED if secret_param(k) else v) for k, v in pairs], safe="[]")
        return urlunsplit((parts.scheme, netloc, parts.path, query, fragment))


def _before_labels(s: str) -> str:
    """The masks applied before the labelled classes: key and token patterns, payment cards, contextual OTPs."""
    for p in SECRET_PATTERNS:
        s = p.sub(REDACTED, s)
    ids = _url_id_spans(s)              # a card in a URL's query or fragment is still one; its path holds ids
    s = CARD.sub(lambda m: m.group() if not payment_card(m.group()) or s[m.start() - 1] in "/." and
                 any(a <= m.start() and m.end() <= b for a, b in ids) else REDACTED, s)
    return OTP_TEXT.sub(lambda m: m.group(1) + REDACTED, s)


def _forward_context(s: str, seen: Callable[[str], str] = lambda line: line) -> str:
    """What of the released text `s` a later line can still relate to (`_forward_relations`)."""
    return _forward_relations(s, seen)[0]


def _forward_relations(s: str, seen: Callable[[str], str] = lambda line: line) -> tuple[str, set[str]]:
    """What of the text `s` a later line can still relate to, and how: its last line when that is a label line
    ("label": a code alone can follow), a "Password:" line ("secret": its value starts the next line) or a backup
    list's label (with blank lines after it and the start of the next line while that is only spaces or brackets), or
    a list line under its label ("list": kept as the label and that line). Empty otherwise. Each is a line or two, so
    the work per read stays the read's own."""
    none: tuple[str, set[str]] = ('', set())
    breaks = [m for m in LINE_BREAK.finditer(s)]
    if not breaks:
        return none
    partial = s[breaks[-1].end():]
    if not re.fullmatch(r"[ \t(\[]*", partial):
        return none
    lines, begin = [], 0
    for m in breaks:
        if s[begin:m.start()].strip():
            lines.append((begin, m.start()))
        begin = m.end()
    if not lines:
        return none
    a, b = lines[-1]
    last = s[a:b]
    # the line breaks after it as read (a "\r" may still become "\r\n"); a run of blank lines kept as one
    gaps = [m for m in breaks if m.start() >= b]
    blank = len(gaps) > 1
    after = s[b:] if not blank else gaps[0].group() + s[gaps[-1].start():]
    # a line's relations as the text redactor decides them: after the masks applied before the labelled classes too
    def either(test: Callable[[str], bool], line: str) -> bool:
        return test(line) or test(seen(line))
    kinds = ({"label"} if either(_label_line, last) else set()) | (
        {"secret"} if either(_secret_colon_end, last) else set()) | ({"list"} if either(_list_label_line, last) else set())
    if kinds:
        return last + after, kinds
    if either(_list_line, last) and not blank:
        def joined(i: int) -> bool:              # line i and the one before it are adjacent (no blank line between)
            return LINE_BREAK.fullmatch(s[lines[i - 1][1]:lines[i][0]].strip(" \t") or "-") is not None
        at = len(lines) - 1
        while (at > 0 and joined(at) and either(_list_line, s[lines[at - 1][0]:lines[at - 1][1]])
               and not either(_list_label_line, s[lines[at - 1][0]:lines[at - 1][1]])):
            at -= 1
        if at > 0 and joined(at) and either(_list_label_line, label := s[lines[at - 1][0]:lines[at - 1][1]]):
            return label + "\n" + last + after, {"list"}
    return none


# What the line being read can still become, under a line that relates to it: a code alone ("  482913", "(G-48"), or
# a list line (codes, commas and spaces only).
_CODE_ALONE_START = re.compile(r"[ \t.,;:()\[\]]*(?:[A-Z]-?)?[\d -]*[ \t.,;:()\[\]]*")
_LIST_LINE_START = re.compile(r"[A-Za-z0-9, \t-]*")


class RedactionStream:
    """Incremental UTF-8 redaction. Emit complete words, retaining possible secrets across chunks.

    An oversized unfinished record is replaced by an explicit marker rather than spilling raw bytes to disk.
    Private-key blocks are suppressed until their end marker, even when they exceed the buffer bound.
    """
    MAX_PENDING = 65536
    PRIVATE_START = re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")
    PRIVATE_END = re.compile(r"-----END [A-Z ]*PRIVATE KEY-----")
    BEARER_TAIL = re.compile(r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]*$")
    PRIVATE_HEADER_TAIL = re.compile(r"-----BEGIN [A-Z -]*$")

    def __init__(self, redactor: Redactor) -> None:
        self.redactor = redactor
        self.decoder = codecs.getincrementaldecoder('utf-8')(errors='replace')
        self.pending = ''
        self.suppress: str | None = None
        self.context = ''         # released text a later line can still relate to (`_forward_context`)

    def _unfinished_key(self) -> int | None:
        for opening in self.PRIVATE_START.finditer(self.pending):
            if self.PRIVATE_END.search(self.pending, opening.end()) is None:
                return opening.start()
        return None

    def _safe_cut(self) -> int:
        # A space-terminated prompt must be visible before a child process asks for input.
        # Keep the final word: it may still grow into a token or a URL with credentials.
        boundaries = list(re.finditer(r'\s', self.pending))
        cut = boundaries[-1].end() if boundaries else 0
        header = self.PRIVATE_HEADER_TAIL.search(self.pending)
        if header:
            cut = min(cut, header.start())
        opening = self._unfinished_key()
        if opening is not None:
            cut = min(cut, opening)
        bearer = self.BEARER_TAIL.search(self.pending)
        if bearer:
            cut = min(cut, bearer.start())
        # Grouped card numbers may straddle whitespace: the line still being read keeps its trailing digits (an OTP
        # introducer holds that whole line: `_labelled_cut`).
        _, line_start = _line_starts(self.pending)
        tail = self.pending[line_start:]
        numeric = re.search(r"(?<!\d)\d[\d -]*$", tail)
        if numeric:                         # from its token's start: a card's digits are never split from it
            start = line_start + numeric.start()
            while start > line_start and not self.pending[start - 1].isspace():
                start -= 1
            cut = min(cut, start)
        # Each guard only moves the cut back, and one can land inside another's region (a class run reaching into a
        # private key block): apply them until none moves it.
        while True:
            moved = self._guarded_cut(cut)
            if moved == cut:
                return cut
            cut = moved

    def _guarded_cut(self, cut: int) -> int:
        for value in self.redactor.secret_values():
            start = self.pending.find(value)
            while start >= 0:
                if start < cut < start + len(value):
                    cut = start
                start = self.pending.find(value, start + 1)
            for length in range(1, min(len(value), len(self.pending) + 1)):
                if self.pending.endswith(value[:length]):
                    cut = min(cut, len(self.pending) - length)
        for pattern in SECRET_PATTERNS:
            for match in pattern.finditer(self.pending):
                if match.start() < cut < match.end():
                    cut = match.start()
        return self._labelled_cut(cut)

    def _labelled_cut(self, cut: int) -> int:
        """C1, decided by whole lines, so what is emitted is masked as `Redactor.text` masks the whole (review of fix
        wave 2: cutting inside a line at its code word left the code before it visible):
        - the line still being read is held whole while it holds a class word, a code, an OTP introducer, or a word that
          can still become one ("api ", "my wifi ");
        - the last complete line is held while what comes next can still change it: a line that is only a code (its
          naming line can follow: "482913" then "is your verification code"), with the lines above that relate to it.
          A label line is released and kept as context for the lines under it (`_masked`).
        A line too long to hold (MAX_PENDING) is held only from the run of class items still within reach of its end."""
        pending = self.pending
        breaks, tail_start = _line_starts(pending)
        tail = pending[tail_start:]
        hold = cut
        # A number alone holds the line only while its link can still follow it ("482913 is your …"): an interactive
        # prompt with a number ("Delete 1234 files? [y/N] ") appears at once (re-review 2, I3).
        urls = [u.span() for u in URL.finditer(tail)]          # a URL's own digits are never a code
        joinable = any((CODE_LINK.match(tail, m.end()) is not None or re.fullmatch(r"\s*\S*", tail[m.end():]))
                       and not any(a <= m.start() < b for a, b in urls) for m in CODE_VALUE.finditer(tail))
        # The line under a label, "Password:" or list line is decided with it, so a read ending inside it never
        # releases part of it without that context (re-review 3, NEW-C1): under "Password:" the whole value line,
        # under a list while it can still be a list line, under a label while it can still be a code alone (a
        # progress line under "Code server ready (3733 files)" shows at once).
        _, kinds = _forward_relations(self.context + pending[:tail_start], self.redactor.before_labels)
        under = bool(tail.strip()) and (
            "secret" in kinds or ("list" in kinds and _LIST_LINE_START.fullmatch(tail) is not None)
            or ("label" in kinds and _CODE_ALONE_START.fullmatch(tail) is not None))
        if (CODE_WORD.search(tail) or SECRET_WORD.search(tail) or joinable or _code_alone(tail) or under
                or OTP_CONTEXT_START.search(tail) or _PREFIX_END.search(tail) or _LIST_QUALIFIER_END.search(tail)):
            hold = min(hold, tail_start if len(tail) <= self.MAX_PENDING // 2 else tail_start + self._open_run(tail))
        lines, begin = [], 0
        for end in breaks:
            text = LINE_BREAK.sub("", pending[begin:end])
            if text.strip():
                lines.append((begin, text))
            begin = end
        if lines and (at := self._held_from(lines)) is not None:
            # a line above that relates to the first held one is decided with it (never code-only lines with each
            # other: number-only output stays one line behind)
            while at > 0 and self._related(lines[at - 1][1], lines[at][1]):
                at -= 1
            hold = min(hold, lines[at][0])
        return hold

    @staticmethod
    def _related(above: str, below: str) -> bool:
        """Two neighbouring lines the text rule relates: a label over a code-only line, a code-only line over its
        naming line, "Password:" over its value, a list label over its first code."""
        return ((_code_alone(below) and _label_line(above)) or (_code_alone(above) and _link_line(below))
                or _secret_colon_end(above) or (_list_line(below) and _list_label_line(above)))

    @staticmethod
    def _held_from(lines: list[tuple[int, str]]) -> int | None:
        """Which complete line (by index) the stream must hold from (re-review 2, I2): only a last line that is only a
        code, whose naming line can still follow ("482913" then "is your verification code"). A label, "Password:"
        or list line never changes with what follows it, so it is released and kept as context (`_masked`); code-only
        lines never hold each other, so number-only output passes one line behind."""
        last = len(lines) - 1
        return last if _code_alone(lines[last][1]) else None

    @staticmethod
    def _open_run(line: str) -> int:
        """Where a very long line still being read must be held from: the start of the run of class items (each within
        reach of the next) that ends within reach of what has been read; the line's end when there is none."""
        found = ([m.span() for m in CODE_WORD.finditer(line)] + [m.span() for m in CODE_VALUE.finditer(line)]
                 + [m.span() for m in OTP_CONTEXT_START.finditer(line)])
        for m in SECRET_WORD.finditer(line):
            own = MINE.search(line, max(0, m.start() - 40), m.start())
            found.append((own.start() if own else m.start(), m.end()))
        if (prefix := _PREFIX_END.search(line)) is not None:
            found.append(prefix.span())
        found.sort()
        starts, reach = _word_starts(line), CODE_REACH + 1
        if not found or _words_between(starts, found[-1][1], len(line)) > reach:
            return max(0, line.rfind(" ") + 1)
        first = len(found) - 1
        while first > 0 and _words_between(starts, found[first - 1][1], found[first][0]) <= reach:
            first -= 1
        start = found[first][0]
        while start > 0 and not line[start - 1].isspace():
            start -= 1
        return start

    def _masked(self, released: str) -> str:
        """`released` masked as `Redactor.text` masks it after the released lines it can relate to (a label line above
        its code, "Password:" above its value, a backup list's label above its codes): those lines never change with
        what follows them, so they are released at once and kept only as context (re-review 2, R2)."""
        if not released:
            return ''
        if self.context:
            before, whole = self.redactor.text(self.context), self.redactor.text(self.context + released)
            out = whole[len(before):] if whole.startswith(before) else self.redactor.text(released)
        else:
            out = self.redactor.text(released)
        self.context = _forward_context(self.context + released, self.redactor.before_labels)
        return out

    def feed(self, chunk: bytes, final: bool = False) -> bytes:
        self.pending += self.decoder.decode(chunk, final=final)
        if self.suppress:
            end = (self.PRIVATE_END.search(self.pending) if self.suppress == 'private'
                   else re.search(r'\n', self.pending))
            if end is None:
                self.pending = self.pending[-128:] if self.suppress == 'private' and not final else ''
                return b''
            self.pending = self.pending[end.end():]
            self.suppress, self.context = None, ''
        if final:
            opening = self._unfinished_key()
            if opening is not None:
                self.pending = self.pending[:opening] + REDACTED
            out, self.pending = self._masked(self.pending), ''
            return out.encode('utf-8')
        cut = self._safe_cut()
        out = self._masked(self.pending[:cut])
        self.pending = self.pending[cut:]
        bound = max(self.MAX_PENDING, max((len(v) for v in self.redactor.secret_values()), default=0) + 1)
        if len(self.pending) > bound:
            opening = self._unfinished_key()
            if opening is not None:
                out += self._masked(self.pending[:opening]) + REDACTED
                self.pending, self.suppress, self.context = self.pending[-128:], 'private', ''
            else:
                out += '[redacted: oversized unfinished record]\n'
                self.pending, self.suppress, self.context = '', 'record', ''
        return out.encode('utf-8')
