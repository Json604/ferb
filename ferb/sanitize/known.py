"""Own the known-value registry: the exact values of FERB's own credentials (the model providers' API keys, the shell
proxy's tokens, FERB's sign-in code), found wherever they stand in a text. Only those are known by value (owner,
GATE-572): no saved secret and no value the owner types into a hidden input. Memory only, never persisted. Must not
decide anything from a value beyond where it stands.

A value is found exactly, wherever it appears, glued to other text too (rule 14). Two exceptions keep FERB's own data
intact: a hit strictly inside one of FERB's generated id bodies is no hit (an altered id breaks lineage), and a short
value (`SHORT`) is no hit inside a longer number, so it never breaks a JSON number or a phone number. Each value is
also found in its escaped forms (JSON, repr, URL, HTML)."""

import bisect
import functools
import html
import json
import re
import threading
import unicodedata
from typing import Iterable, Iterator
from urllib.parse import quote, quote_plus

REDACTED = "[redacted]"
SHORT = 6                       # a value this long or shorter never matches inside a longer number

# The generated part of a FERB id: a uuid4 hex (task, request, journal, trail, memory ids), a sha256 hex (learning
# nodes, m-/key-) or a 6-byte token hex (chat sessions, chat-).
_ID_HEX = r"[0-9a-f]{64}|[0-9a-f]{32}|[0-9a-f]{12}"
ID_BODY = re.compile(rf"(?<![0-9A-Za-z])({_ID_HEX})(?![0-9A-Za-z])")
_HEX_LETTER = re.compile(r"[a-f]")


def word_char(ch: str) -> bool:
    """A character that is part of a word: a letter, number or mark, or a private-use character (the FTS tokenizer's
    word characters, migration 0014)."""
    cat = unicodedata.category(ch)
    return cat[0] in "LNM" or cat == "Co"


@functools.lru_cache(maxsize=1024)
def escaped_forms(value: str) -> tuple[str, ...]:
    """How a value reads once escaped, when that differs from it: inside JSON (both `ensure_ascii` ways), in a repr,
    URL-encoded (path and form) and HTML-escaped."""
    forms = dict.fromkeys((json.dumps(value, ensure_ascii=False)[1:-1], json.dumps(value)[1:-1],
                           value.encode("unicode_escape").decode(), quote(value, safe=""), quote_plus(value, safe=""),
                           html.escape(value)))
    forms.pop(value, None)
    return tuple(forms)


def id_bodies(s: str, *, keyed: bool = False) -> list[tuple[int, int]]:
    """Where FERB's generated id bodies lie in `s`. In free text a body has an a-f letter (twelve digits are a phone,
    UPI or Aadhaar number); under FERB's own id keys (`keyed`) any body counts, so generated ids keep their form."""
    return [m.span(1) for m in ID_BODY.finditer(s) if keyed or _HEX_LETTER.search(m.group(1))]


def _occurrences(s: str, value: str) -> Iterator[int]:
    start = s.find(value)
    while start != -1:
        yield start
        start = s.find(value, start + 1)


def sealed_spans(s: str, value: str, bodies: list[tuple[int, int]] | None = None) -> list[tuple[int, int]]:
    """Where a known value stands in `s`: never strictly inside an id body (a hit equal to a body, or reaching past
    one, is the secret itself); a short value never inside a longer number; otherwise exactly."""
    if not value:
        return []
    if bodies is None:
        bodies = id_bodies(s)
    short = len(value) <= SHORT
    out = []
    for start in _occurrences(s, value):
        end = start + len(value)
        i = bisect.bisect_right(bodies, (start, len(s) + 1)) - 1
        if i >= 0 and end <= bodies[i][1] and (start, end) != bodies[i]:
            continue
        if short and ((value[0].isdigit() and start and s[start - 1].isdigit())
                      or (value[-1].isdigit() and end < len(s) and s[end].isdigit())):
            continue
        out.append((start, end))
    return out


def known_spans(s: str, values: Iterable[str], *, keyed: bool = False) -> list[tuple[int, int]]:
    """Every place one of `values` stands in `s` (`sealed_spans`), the ids read once for all of them. A `KnownValues`
    is read with one search first."""
    if isinstance(values, KnownValues) and not values.anywhere(s):
        return []
    present = [v for v in values if v and v in s]
    if not present:
        return []
    bodies = id_bodies(s, keyed=keyed)
    return [span for v in present for span in sealed_spans(s, v, bodies)]


def holds_sealed(s: str, values: Iterable[str]) -> bool:
    """Whether `s` still shows one of the known `values` where the redactor would hide it."""
    return bool(known_spans(s, values))


class KnownValues(frozenset[str]):
    """The known values and their forms, with one compiled search for whether any occurs at all (`anywhere`)."""
    _finder: re.Pattern[str] | None

    def __new__(cls, values: Iterable[str]) -> "KnownValues":
        self = super().__new__(cls, values)
        plain = sorted((re.escape(v) for v in self if v), key=len, reverse=True)
        self._finder = re.compile("|".join(plain)) if plain else None
        return self

    def anywhere(self, s: str) -> bool:
        return self._finder is not None and self._finder.search(s) is not None


class Registry:
    """The known values (`add`, `forget`). `version` changes with every change, so a mark made under an older set of
    values is never trusted (`Redactor`)."""

    def __init__(self) -> None:
        self._values: set[str] = set()
        self._forms: KnownValues | None = None
        self.version = 0
        self._lock = threading.RLock()          # worker threads read the values while the loop's thread adds some

    def _changed(self) -> None:
        self._forms = None
        self.version += 1

    def add(self, value: str) -> None:
        """One of FERB's own credentials (an API key, a proxy token, a sign-in code): known when it has 4 characters
        or more."""
        with self._lock:
            if len(value) >= 4 and value not in self._values:
                self._values.add(value)
                self._changed()

    def lasting(self) -> int:
        """How many values are known (never which)."""
        return len(self._values)

    def forget(self, value: str) -> None:
        with self._lock:
            if value in self._values:
                self._values.discard(value)
                self._changed()

    def values(self) -> "KnownValues":
        """Every known value and each one's escaped forms."""
        with self._lock:
            if self._forms is None:
                held = self._values
                self._forms = KnownValues(held | {form for value in held for form in escaped_forms(value)})
            return self._forms
