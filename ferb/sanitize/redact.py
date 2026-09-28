"""The one redaction pipeline (section 10.6). Runs before anything reaches a model, trace, log, exception message,
journal, memory or export. Returns new objects; never mutates its input. Imports core only.
"""

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
    re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}"),
    re.compile(r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{12,}"),
]
URL = re.compile(r"\b[a-zA-Z][a-zA-Z0-9+.-]*://[^\s\"'<>]+")
SECRET_QUERY = {"token", "code", "key", "sig", "signature", "auth", "session", "access_token", "id_token",
                "refresh_token", "password", "secret", "otp", "apikey", "api_key", "client_secret", "sessionid"}
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
        self.exclude_domains = {d.lower().lstrip(".") for d in (exclude_domains or set())}
        self.exclude_apps = {a.lower() for a in (exclude_apps or set())}
        self._values: set[str] = set()      # exact secret values; memory only, never persisted

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
        secret_field = str(d.get("type", "")).lower() == "password" or \
            "one-time-code" in str(d.get("autocomplete", "")).lower()
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
                out[k] = self._walk(v, ks)
        return out

    def _str(self, s: str) -> str:
        if self.exclude_apps and s.lower() in self.exclude_apps:
            return EXCLUDED
        for v in self._values:
            if v in s:
                s = s.replace(v, REDACTED)
        for p in SECRET_PATTERNS:
            s = p.sub(REDACTED, s)
        return URL.sub(lambda m: self._url(m.group(0)), s)

    def _url(self, url: str) -> str:
        try:
            parts = urlsplit(url)
        except ValueError:
            return url
        host = (parts.hostname or "").lower()
        if any(host == d or host.endswith("." + d) for d in self.exclude_domains):
            return EXCLUDED
        netloc = parts.netloc
        if parts.password is not None:
            netloc = netloc.replace(f":{parts.password}@", f":{REDACTED}@", 1)
        query = parts.query
        if query:
            pairs = parse_qsl(query, keep_blank_values=True)
            if any(k.lower() in SECRET_QUERY for k, _ in pairs):
                query = urlencode([(k, REDACTED if k.lower() in SECRET_QUERY else v) for k, v in pairs], safe="[]")
        fragment = parts.fragment
        if fragment and "=" in fragment:
            pairs = parse_qsl(fragment, keep_blank_values=True)
            if any(k.lower() in SECRET_QUERY for k, _ in pairs):
                fragment = urlencode([(k, REDACTED if k.lower() in SECRET_QUERY else v) for k, v in pairs], safe="[]")
        return urlunsplit((parts.scheme, netloc, parts.path, query, fragment))
