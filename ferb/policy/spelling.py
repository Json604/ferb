"""Misspelled app and service names in the owner's own words (owner decision 2026-10-01).

"check my salck messages": a word one or two letters off exactly one installed app or known service — and not itself
an English word — is read as that name, and FERB says so ("Using Slack, from 'salck'"). A word close to several names
is asked about instead ("Did you mean …?"). Only the owner's own words are corrected, never pasted text, so the rule
that every target comes from the owner's words still holds.
"""

import re
from dataclasses import dataclass, field
from functools import lru_cache
from pathlib import Path

DICTIONARY = Path("/usr/share/dict/words")
MIN_LEN = 4
MODERN = frozenset({          # everyday words the Mac's (old) dictionary lacks: never typos of an app name
    "email", "emails", "inbox", "online", "offline", "website", "websites", "webpage", "login", "logout", "signup",
    "username", "password", "laptop", "desktop", "smartphone", "app", "apps", "chat", "chats", "texting", "selfie",
    "emoji", "emojis", "playlist", "podcast", "podcasts", "download", "downloads", "upload", "uploads", "update",
    "updates", "settings", "screenshot", "screenshots", "wifi", "bluetooth", "browser", "browsers", "tab", "tabs",
    "link", "links", "reels", "story", "stories", "dm", "dms", "msgs", "pls", "plz", "thx", "okay"})


@dataclass
class Fixed:
    text: str
    fixes: list[tuple[str, str]] = field(default_factory=list)            # (what the owner typed, the name)
    ambiguous: list[tuple[str, list[str]]] = field(default_factory=list)  # (what they typed, the names it could be)

    def note(self) -> str:
        return " ".join(f"Using {name}, from “{typed}”." for typed, name in self.fixes)

    def question(self) -> str:
        typed, names = self.ambiguous[0]
        options = ", ".join(names[:-1]) + f" or {names[-1]}"
        return f"Did you mean {options} (for “{typed}”)?"


@lru_cache(maxsize=1)
def _english() -> frozenset[str]:
    try:
        return frozenset(w.strip().lower() for w in DICTIONARY.read_text(errors="ignore").splitlines())
    except OSError:
        return frozenset()


def distance(a: str, b: str) -> int:
    """Edits between two words, a swap of neighbours counting as one ("salck" -> "slack" = 1)."""
    d = [[0] * (len(b) + 1) for _ in range(len(a) + 1)]
    for i in range(len(a) + 1):
        d[i][0] = i
    for j in range(len(b) + 1):
        d[0][j] = j
    for i in range(1, len(a) + 1):
        for j in range(1, len(b) + 1):
            cost = 0 if a[i - 1] == b[j - 1] else 1
            d[i][j] = min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + cost)
            if i > 1 and j > 1 and a[i - 1] == b[j - 2] and a[i - 2] == b[j - 1]:
                d[i][j] = min(d[i][j], d[i - 2][j - 2] + 1)
    return d[len(a)][len(b)]


def _allowed(word: str) -> int:
    return 1 if len(word) <= 7 else 2


def fix(own_words: str, names: list[str], english: frozenset[str] | None = None) -> Fixed:
    """Correct the owner's words against these names (installed apps, known services)."""
    english = _english() if english is None else english
    by_token: dict[str, str] = {}                               # a single-word form -> the name to show
    for n in names:
        parts = re.findall(r"[a-z0-9]+", n.lower())
        if len(parts) == 1 and len(parts[0]) >= MIN_LEN:
            by_token.setdefault(parts[0], n)
        elif parts and len(parts[0]) >= MIN_LEN:                # "Brave Browser" is also "brave"
            by_token.setdefault(parts[0], n)
    out = Fixed(own_words)
    seen: set[str] = set()
    for word in re.findall(r"[A-Za-z][A-Za-z0-9]+", own_words):
        w = word.lower()
        if w in seen or len(w) < MIN_LEN or w in by_token or w in english or w in MODERN:
            continue
        seen.add(w)
        close = sorted({name for tok, name in by_token.items() if distance(w, tok) <= _allowed(tok)})
        if len(close) == 1:
            out.fixes.append((word, close[0]))
            out.text = re.sub(rf"\b{re.escape(word)}\b", close[0], out.text)
        elif len(close) > 1:
            out.ambiguous.append((word, close))
    return out
