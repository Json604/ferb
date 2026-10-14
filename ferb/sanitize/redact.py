"""Own secret redaction before model, trace and log boundaries. Must not use secret content to decide actions."""

import bisect
import codecs
import re
from typing import Any
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
CODE_WORD = re.compile(r"(?i)(?<![A-Za-z0-9])(?:one[ _-]?time[ _-]?(?:password|passcode|code|pin)|otp|totp|2fa|"
                       r"pass[ _-]?code|pin|codes?|verification|verify)(?![A-Za-z0-9])")
# The code itself: 4-8 digits, grouped by spaces or hyphens (482-913, 48 29 13), or Google's G-482913. Never part of a
# longer token or digit group (an id, a phone number); a JSON escape before it still separates it.
CODE_VALUE = re.compile(rf"(?:(?<![0-9A-Za-z_-])|(?<=\\[ntrbf])|(?<=\\u[0-9a-fA-F]{{4}}))(?<!\d[ -])"
                        rf"(?:[A-Z]-)?\d(?:[ -]?\d){{3,7}}(?![0-9A-Za-z_]|[ -]\d)")
# A code before its word is joined to it the way providers write it: "482913 is your code", "Use 482913 to verify",
# "482913: your code", "739201 (OTP)". A bare "2024 code review" has no value: nothing joins the number to the word.
CODE_LINK = re.compile(r"(?i)\s*(?:(?:is|are|was|as|to)(?![A-Za-z0-9])|[:=(]|[-–—])")
SECRET_WORD = re.compile(r"(?i)(?<![A-Za-z0-9])(?:pass(?:word|wd|phrase|code)|pwd|pin|secret(?:[ _-]?key)?|"
                         r"api[ _-]?key|(?:(?:access|auth|api|refresh)[ _-]?)?token|private[ _-]?key)"
                         r"(?![A-Za-z0-9])")
# What follows a secret word: an optional "for gmail" / "to the wifi", a link (":", "=", "is", "was", " - ") or none.
SECRET_AFTER = re.compile(r"""(?ix)(?P<qual>\s+(?:for|to|of|on|at)\s+(?:the\s+|my\s+|your\s+)?[^\s:=]+)?
                              (?P<link>(?:\\?["'])?[ \t]*[:=][ \t]*\n?[ \t]*|\s+(?:is|was)(?![A-Za-z0-9])[ \t]*:?\s*|
                                       \s+[-–—]\s+)""")
SECRET_DIRECT = re.compile(r"\s+")
# The owner's own: "my password is sunshine" hides a plain word too (a page's "Your password is incorrect" doesn't).
MINE = re.compile(r"(?i)(?<![A-Za-z0-9])(?:my|our)\s+(?:[A-Za-z-]+\s+)?$")
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
    if not value or value == REDACTED:
        return None
    digit = any(c.isdigit() for c in value)
    symbol = any(not c.isalnum() for c in value)
    if strong or (after is not None and (mine or (len(value) >= 4 and (digit or symbol)))) or \
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
    words = [m for m in SECRET_WORD.finditer(s) if not in_url(m.span())]
    for word in words:
        value = _secret_value(s, word)
        if value is not None:
            spans.append(value)
    codes = [m for m in CODE_VALUE.finditer(s) if not in_url(m.span())]
    labels = [m.span() for m in CODE_WORD.finditer(s) if not in_url(m.span())] if codes else []
    if labels:
        starts, ends, begins = _word_starts(s), [b for _, b in labels], [a for a, _ in labels]
        for code in codes:
            before = bisect.bisect_right(ends, code.start()) - 1
            after = bisect.bisect_left(begins, code.end())
            if (before >= 0 and _words_between(starts, ends[before], code.start()) <= CODE_REACH) or (
                    after < len(labels) and CODE_LINK.match(s, code.end()) is not None
                    and _words_between(starts, code.end(), begins[after]) <= CODE_REACH):
                spans.append(code.span())
    return spans


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
        bool(CODE_WORD.search(label)) and CODE_VALUE.fullmatch(value) is not None)


def _labelled_secret(kind: str, label: str, text: str) -> bool:
    """A text field a secret word names (API key, token, PIN, secret): its value is that secret, whatever its shape
    (C1). Only text-entry fields, so a "Pin tab" checkbox keeps its state."""
    return bool(text.strip()) and kind.casefold() in TEXT_FIELD_KINDS and SECRET_WORD.search(label) is not None


# Streaming retains a possible introducer until its line is complete. This only buffers bytes: the complete
# text is still redacted by the narrow credential patterns, so 'login phone number' remains ordinary text.
OTP_CONTEXT_START = re.compile(r"(?i)\b(?:one[ _-]?time|verification|authentication|security|login|sign[ _-]?in|access|2fa|otp|totp)\b")
OTP_TEXT = re.compile(r"(?i)(\b(?:one[ -]?time(?: password| code)?|verification code|authentication code|security code|login code|sign[ -]?in code|otp)\b[^\d\n]{0,40})(\d(?:[ -]?\d){3,7})(?!\d)")
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

    def answer(self, text: str, *, question: str) -> str:
        """The owner's typed answer to an ordinary question, which is no field: a code is hidden for this answer
        when the question carries an explicit code label (rule 14); the question's other words never hide it."""
        if _labelled_code(question, text):
            return REDACTED if text else text
        return self._str(text)

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
            return [self._walk(v) for v in obj]
        if isinstance(obj, str):
            return self._str(obj)
        return obj

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
        for p in SECRET_PATTERNS:
            s = p.sub(REDACTED, s)
        ids = _url_id_spans(s)              # a card in a URL's query or fragment is still one; its path holds ids
        s = CARD.sub(lambda m: m.group() if not payment_card(m.group()) or s[m.start() - 1] in "/." and
                     any(a <= m.start() and m.end() <= b for a, b in ids) else REDACTED, s)
        s = OTP_TEXT.sub(lambda m: m.group(1) + REDACTED, s)
        s = _mask_spans(s, labelled_secret_spans(s))
        return URL.sub(lambda m: self._url(m.group(0)), s)

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
        # Contextual codes and grouped card numbers may straddle whitespace. Keep the current line from their
        # class introducer/first digit until a line ending, so a later chunk cannot reveal a partial secret.
        line_start = self.pending.rfind("\n") + 1
        tail = self.pending[line_start:]
        contextual = OTP_CONTEXT_START.search(tail)
        numeric = re.search(r"(?<!\d)\d[\d -]*$", tail)
        if contextual:
            cut = min(cut, line_start + contextual.start())
        if numeric:
            cut = min(cut, line_start + numeric.start())
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
        """C1: a code, a code word or a secret word pairs with text up to CODE_REACH words away, so the emitted part
        never ends within that reach of one: the cut moves back to the start of its run of nearby class words (each
        within reach of the next), and the rest is decided once together. A secret word's run starts at the "my"/"our"
        before it, and a trailing "my"/"our" with up to two words after it is kept, since it can still become "my wifi
        password is …"."""
        pending = self.pending
        mine = re.search(r"(?i)(?<![A-Za-z0-9])(?:my|our)(?:\s+[A-Za-z-]+){0,2}\s*$", pending)
        if mine:
            cut = min(cut, mine.start())
        found = [m.span() for m in CODE_WORD.finditer(pending)] + [m.span() for m in CODE_VALUE.finditer(pending)]
        for m in SECRET_WORD.finditer(pending):
            own = MINE.search(pending, max(0, m.start() - 40), m.start())
            found.append((own.start() if own else m.start(), m.end()))
        before = sorted(span for span in found if span[0] < cut)
        if not before:
            return cut
        starts = _word_starts(pending)
        reach = CODE_REACH + 1
        if _words_between(starts, before[-1][1], cut) > reach:
            return cut
        first = len(before) - 1
        while first > 0 and _words_between(starts, before[first - 1][1], before[first][0]) <= reach:
            first -= 1
        start = before[first][0]
        while start > 0 and not pending[start - 1].isspace():
            start -= 1
        return min(cut, start)

    def feed(self, chunk: bytes, final: bool = False) -> bytes:
        self.pending += self.decoder.decode(chunk, final=final)
        if self.suppress:
            end = (self.PRIVATE_END.search(self.pending) if self.suppress == 'private'
                   else re.search(r'\n', self.pending))
            if end is None:
                self.pending = self.pending[-128:] if self.suppress == 'private' and not final else ''
                return b''
            self.pending = self.pending[end.end():]
            self.suppress = None
        if final:
            opening = self._unfinished_key()
            if opening is not None:
                self.pending = self.pending[:opening] + REDACTED
            out, self.pending = self.redactor.text(self.pending), ''
            return out.encode('utf-8')
        cut = self._safe_cut()
        out = self.redactor.text(self.pending[:cut])
        self.pending = self.pending[cut:]
        bound = max(self.MAX_PENDING, max((len(v) for v in self.redactor.secret_values()), default=0) + 1)
        if len(self.pending) > bound:
            opening = self._unfinished_key()
            if opening is not None:
                out += self.redactor.text(self.pending[:opening]) + REDACTED
                self.pending, self.suppress = self.pending[-128:], 'private'
            else:
                out += '[redacted: oversized unfinished record]\n'
                self.pending, self.suppress = '', 'record'
        return out.encode('utf-8')
