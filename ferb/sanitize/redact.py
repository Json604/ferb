"""Own secret redaction before model, trace and log boundaries. Must not use secret content to decide actions."""

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
# FERB's own internal id fields: ids, never token values (owner decision). Values are still pattern-scanned.
INTERNAL_ID_KEYS = {"task_id", "span_id", "session_id", "run_token_id"}

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
# "passw" or "pwd"). A secret class at the capture boundary like OTP_LABEL, never a judgement of intent. It covers
# text-entry fields (their AX role, ARIA role or input type; "" when a record names none): a checkbox, a button or a
# menu item ("Show password", "Passwords") is no text field, so its state stays readable.
PASSWORD_FIELD = re.compile(r"(?i)passw|pwd")
TEXT_FIELD_KINDS = frozenset({"", "text", "password", "email", "search", "tel", "url", "number", "textarea",
                              "textbox", "searchbox", "combobox", "axtextfield", "axtextarea", "axcombobox",
                              "axsearchfield"})
FIELD_METADATA = ("autocomplete", "name", "id", "label", "placeholder", "title")


def password_field(kind: str, *metadata: Any) -> bool:
    """A text-entry field of this kind (its role or input type) that its metadata names a password field."""
    return kind.casefold() in TEXT_FIELD_KINDS and any(isinstance(m, str) and PASSWORD_FIELD.search(m)
                                                        for m in metadata)


def _labelled_code(label: str, text: str) -> bool:
    return bool(OTP_LABEL.search(label)) and re.fullmatch(r"[\d -]+", text.strip()) is not None


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
_REF_DIGITS = re.compile(r"[0-9]+(?:-[0-9]+)*")


def _url_id_spans(s: str) -> list[tuple[int, int]]:
    """Where each URL's scheme, host and path lie in `s`. Digits there are ids (a post, a message, a snowflake), not
    a credential; a URL's query and fragment are not part of these spans."""
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

    def field(self, text: str, *, role: str = "", label: str = "", secure_input: bool = False,
              concealed: bool = False, metadata: tuple[str, ...] = ()) -> str:
        """Capture boundary for AX/DOM/clipboard fields; metadata never grants action permission."""
        if (secure_input or concealed or "secure" in role.casefold() or role.casefold() == "password"
                or password_field(role, label, *metadata) or _labelled_code(label, text)):
            return REDACTED if text else text
        return self._str(text)

    def answer(self, text: str, *, question: str) -> str:
        """The owner's typed answer to an ordinary question, which is no field: a code is hidden for this answer
        when the question carries an explicit code label (rule 14); the question's other words never hide it."""
        if _labelled_code(question, text):
            return REDACTED if text else text
        return self._str(text)

    def id_ref(self, ref: str) -> str:
        """One of FERB's own record ids (m-<hex>, trail:<id>, …) as a span input's source_refs hold it. Text
        redaction's substring and card scans never run inside it (L16: an altered ref breaks lineage), but a ref that
        is itself a secret is still redacted: a credential pattern, or an exact secret value or card number as the
        whole ref or as the id after its family prefix."""
        if self.exclude_apps and ref.lower() in self.exclude_apps:
            return EXCLUDED
        ident = ref.rpartition(":")[2]
        secrets = self.secret_values()
        if ref in secrets or ident in secrets or (_REF_DIGITS.fullmatch(ident) and payment_card(ident)):
            return REDACTED
        for p in SECRET_PATTERNS:
            ref = p.sub(REDACTED, ref)
        return ref

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
        secret_field = (str(d.get("type", "")).lower() == "password"
                        or "one-time-code" in str(d.get("autocomplete", "")).lower()
                        or password_field(str(d.get("input_type") or d.get("type") or d.get("role") or ""),
                                          *(d.get(k) for k in FIELD_METADATA)))
        input_event = isinstance(d.get("method"), str) and d["method"].startswith("Input.")
        out: dict[Any, Any] = {}
        for k, v in d.items():
            ks = str(k)
            if secret_key(ks) and v not in (None, "", [], {}):
                out[k] = REDACTED
            elif secret_field and ks == "value" and v not in (None, ""):
                out[k] = REDACTED
            elif input_event and ks == "params" and isinstance(v, dict):
                out[k] = {pk: (REDACTED if pk in INPUT_TEXT_FIELDS else self._walk(pv)) for pk, pv in v.items()}
            else:
                out[k] = (self.field(v, role=str(d.get("role", d.get("type", ""))),
                                     label=str(d.get("label", d.get("name", d.get("autocomplete", "")))))
                          if ks in {"value", "text", "content"} and isinstance(v, str) else self._walk(v, ks))
        return out

    def _str(self, s: str) -> str:
        if self.exclude_apps and s.lower() in self.exclude_apps:
            return EXCLUDED
        for v in sorted(self.secret_values(), key=len, reverse=True):
            if v in s:
                s = s.replace(v, REDACTED)
        for p in SECRET_PATTERNS:
            s = p.sub(REDACTED, s)
        ids = _url_id_spans(s)              # a card in a URL's query or fragment is still one; its path holds ids
        s = CARD.sub(lambda m: m.group() if any(a <= m.start() < b for a, b in ids) or not payment_card(m.group())
                     else REDACTED, s)
        s = OTP_TEXT.sub(lambda m: m.group(1) + REDACTED, s)
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
        return cut

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
