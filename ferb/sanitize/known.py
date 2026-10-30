"""Own the known-value registry: the exact secret values FERB holds (Keychain handles preloaded at start, sealed fills,
login codes, API keys, proxy tokens, hidden answers), found wherever they stand in a text. Memory only, never
persisted. Must not decide anything from a value beyond where it stands.

A value is kept as registered (L36 §7): an ordinary word of the system word list (`SealedWord`) is found only as a
whole word, case aside; every other value exactly, wherever it appears, glued to other text too (rule 14). Two
exceptions keep FERB's own data intact: a hit strictly inside one of FERB's generated id bodies is no hit (an altered
id breaks lineage), and a short value (`SHORT`, e.g. a 6-digit OTP) is no hit inside a longer number, so it never
breaks a JSON number or a phone number. Each value is also found in its escaped forms (JSON, repr, URL, HTML)."""

import bisect
import functools
import html
import json
import re
import threading
import unicodedata
from typing import Iterable, Iterator
from urllib.parse import quote, quote_plus

from ferb.sanitize.dictionary import ordinary_word

REDACTED = "[redacted]"
SHORT = 6                       # a value this long or shorter never matches inside a longer number

# The generated part of a FERB id: a uuid4 hex (task, request, journal, trail, memory ids), a sha256 hex (learning
# nodes, m-/key-) or a 6-byte token hex (chat sessions, chat-).
_ID_HEX = r"[0-9a-f]{64}|[0-9a-f]{32}|[0-9a-f]{12}"
ID_BODY = re.compile(rf"(?<![0-9A-Za-z])({_ID_HEX})(?![0-9A-Za-z])")
_HEX_LETTER = re.compile(r"[a-f]")
# Scripts written without spaces between words (L36 §7 review 1): a word there is hidden wherever it appears.
_SPACELESS_SCRIPTS = ("CJK ", "HIRAGANA ", "KATAKANA ", "KATAKANA-HIRAGANA ", "HALFWIDTH KATAKANA ", "HANGUL ",
                      "HALFWIDTH HANGUL ", "THAI ", "LAO ", "KHMER ", "MYANMAR ")
# A JSON or string escape (\n \t \r \b \f, \uXXXX) or a URL escape (%XX) just before a word ends the word before it.
_ESCAPE_BEFORE = re.compile(r"(?:\\[ntrbf]|\\u[0-9A-Fa-f]{4}|%[0-9A-Fa-f]{2})\Z")


def word_char(ch: str) -> bool:
    """A character that is part of a word: a letter, number or mark, or a private-use character (the FTS tokenizer's
    word characters, migration 0014)."""
    cat = unicodedata.category(ch)
    return cat[0] in "LNM" or cat == "Co"


def _continues(ch: str) -> bool:
    """Whether `ch` continues a word answer: a letter or a mark only; a digit or "_" ends it."""
    return unicodedata.category(ch)[0] in "LM"


class SealedWord(str):
    """A sealed value that is an ordinary word: found only as a whole word, case aside (L36 §7 rulings 4, 5). `found`
    tells whether it occurs at all; `starts` finds every place it starts, overlapping ones too."""
    found: re.Pattern[str]
    starts: re.Pattern[str]

    def __new__(cls, value: str) -> "SealedWord":
        word = super().__new__(cls, value)
        word.found = re.compile(re.escape(value), re.IGNORECASE)
        word.starts = re.compile(f"(?=({re.escape(value)}))", re.IGNORECASE)
        return word


def sealed(value: str) -> str:
    """How a value is kept, decided once when it is registered: a `SealedWord` when the system word list holds it and
    it has no letter of a script written without spaces, else the value itself."""
    if isinstance(value, SealedWord) or any(unicodedata.name(c, "").startswith(_SPACELESS_SCRIPTS)
                                            for c in value if not c.isascii()) or not ordinary_word(value):
        return value
    return SealedWord(value)


@functools.lru_cache(maxsize=1024)
def _escaped(value: str, word: bool) -> tuple[str, ...]:
    forms = dict.fromkeys((json.dumps(value, ensure_ascii=False)[1:-1], json.dumps(value)[1:-1],
                           value.encode("unicode_escape").decode(), quote(value, safe=""), quote_plus(value, safe=""),
                           html.escape(value)))
    forms.pop(value, None)
    return tuple(SealedWord(f) if word else f for f in forms)


def escaped_forms(value: str) -> tuple[str, ...]:
    """How a value reads once escaped, when that differs from it: inside JSON (both `ensure_ascii` ways), in a repr,
    URL-encoded (path and form) and HTML-escaped. A `SealedWord`'s forms keep its whole-word rule."""
    return _escaped(str(value), isinstance(value, SealedWord))


def id_bodies(s: str, *, keyed: bool = False) -> list[tuple[int, int]]:
    """Where FERB's generated id bodies lie in `s`. In free text a body has an a-f letter (twelve digits are a phone,
    UPI or Aadhaar number); under FERB's own id keys (`keyed`) any body counts, so generated ids keep their form."""
    return [m.span(1) for m in ID_BODY.finditer(s) if keyed or _HEX_LETTER.search(m.group(1))]


def _occurrences(s: str, value: str) -> Iterator[int]:
    start = s.find(value)
    while start != -1:
        yield start
        start = s.find(value, start + 1)


def _occurs(s: str, value: str) -> bool:
    return bool(value) and (value.found.search(s) is not None if isinstance(value, SealedWord) else value in s)


def sealed_spans(s: str, value: str, bodies: list[tuple[int, int]] | None = None) -> list[tuple[int, int]]:
    """Where a known value stands in `s`: never strictly inside an id body (a hit equal to a body, or reaching past
    one, is the secret itself); a `SealedWord` only as a whole word (only a letter or mark continues it, and an escape
    before it ends the word before); a short value never inside a longer number; any other value exactly."""
    if not value:
        return []
    if bodies is None:
        bodies = id_bodies(s)
    word = isinstance(value, SealedWord)
    short = len(value) <= SHORT
    starts = (m.start(1) for m in value.starts.finditer(s)) if isinstance(value, SealedWord) else _occurrences(s, value)
    out = []
    for start in starts:
        end = start + len(value)
        i = bisect.bisect_right(bodies, (start, len(s) + 1)) - 1
        if i >= 0 and end <= bodies[i][1] and (start, end) != bodies[i]:
            continue
        if short and ((value[0].isdigit() and start and s[start - 1].isdigit())
                      or (value[-1].isdigit() and end < len(s) and s[end].isdigit())):
            continue
        if word and not ((start == 0 or not _continues(value[0]) or not _continues(s[start - 1])
                          or _ESCAPE_BEFORE.search(s, max(0, start - 6), start) is not None)
                         and (end == len(s) or not _continues(value[-1]) or not _continues(s[end]))):
            continue
        out.append((start, end))
    return out


def known_spans(s: str, values: Iterable[str], *, keyed: bool = False) -> list[tuple[int, int]]:
    """Every place one of `values` stands in `s` (`sealed_spans`), the ids read once for all of them. A `KnownValues`
    is read with one search first."""
    if isinstance(values, KnownValues) and not values.anywhere(s):
        return []
    present = [v for v in values if _occurs(s, v)]
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
        plain = sorted((re.escape(v) for v in self if v and not isinstance(v, SealedWord)), key=len, reverse=True)
        words = sorted((re.escape(v) for v in self if v and isinstance(v, SealedWord)), key=len, reverse=True)
        parts = ([f"(?:{'|'.join(plain)})"] if plain else []) + ([f"(?i:{'|'.join(words)})"] if words else [])
        self._finder = re.compile("|".join(parts)) if parts else None
        return self

    def anywhere(self, s: str) -> bool:
        return self._finder is not None and self._finder.search(s) is not None


class Registry:
    """The known values: lasting ones (`remember`, `add`) and leased ones (`retain`/`release`, reference counted, for
    an answer only while its question waits). `version` changes with every change, so a mark made under an older set
    of values is never trusted (`Redactor`)."""

    def __init__(self) -> None:
        self._values: set[str] = set()
        self._leased: dict[str, int] = {}
        self._forms: KnownValues | None = None
        self.version = 0
        self._lock = threading.RLock()          # worker threads read the values while the loop's thread adds some

    def _changed(self) -> None:
        self._forms = None
        self.version += 1

    def _kept(self, value: str) -> str:
        held = next((v for v in (*self._values, *self._leased) if v == value), None)
        return held if held is not None else sealed(value)

    def remember(self, value: str) -> None:
        """A value known to be secret whatever its length (a hidden input, a stored secret revealed for a fill, a
        preloaded Keychain value). A typed answer to an ordinary question never comes here (L18)."""
        with self._lock:
            if value and value not in self._values:
                self._values.add(self._kept(value))
                self._changed()

    def lasting(self) -> int:
        """How many lasting values are known (never which)."""
        return len(self._values)

    def add(self, value: str) -> None:
        """A value FERB itself handled (a fill, a login code, a proxy token): known when it has 4 characters or more."""
        if len(value) >= 4:
            self.remember(value)

    def forget(self, value: str) -> None:
        with self._lock:
            if value in self._values:
                self._values.discard(value)
                self._changed()

    def retain(self, value: str) -> None:
        with self._lock:
            if value:
                key = self._kept(value)
                self._leased[key] = self._leased.get(key, 0) + 1
                self._changed()

    def release(self, value: str) -> None:
        with self._lock:
            count = self._leased.get(value, 0)
            if count <= 1:
                self._leased.pop(value, None)
            else:
                self._leased[value] = count - 1
            self._changed()

    def values(self) -> "KnownValues":
        """Every known value as kept, and each one's escaped forms."""
        with self._lock:
            if self._forms is None:
                held = self._values | self._leased.keys()
                self._forms = KnownValues(held | {form for value in held for form in escaped_forms(value)})
            return self._forms
