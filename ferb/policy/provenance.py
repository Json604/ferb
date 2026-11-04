"""Owns matching a consequential call's targets against where they could have come from (S3). Must not decide or block.

Structured values (account, site, path, recipient addresses) match only as a whole whitespace-delimited token of the
owner's words or of outside content (case-insensitive; a site also matches the host of a URL token; a path also
matches its ~ form). A contact or recipient without @ is, by shape: a phone number (digits, spaces and + - ( ) .,
at least 6 digits), matched when its digits equal the digits of a phone-like run in the text, exactly; a value with
no whitespace, matched as a whole token; or a multi-word name, matched as a whole phrase with word boundaries by
Unicode category so Hindi words stay whole, or when every word the text uses for it is a whole word of it (OPEN
QUESTION 4, owner 2026-10-09: "Rahul" names "Rahul Verma"; "Rahul Verma" doesn't name "Rahul Attacker"). Where the
text's name for it ends is told by shape only (`_named_by_words`). Amounts match a token that is wholly a number (one
leading currency symbol allowed), by value.
`destination_shape` tells a typed value that is wholly an email address, a phone number or a URL (a shape, never a
meaning); the browser and Mac typing tools record a typed email address or phone number as the call's recipient
(`typed_destination`), never a URL: a URL addresses no one (GATE-509).
A value in the request's words (what the owner typed) is "owner_words"; one in the owner's owner.ask answers is
"owner_answer" (a chosen option is model-written text the owner picked, so it never counts as their typed words); a
value in both is both.
The result is a fact the trace shows and F5's confirmations use; nothing here judges intent. Values under 3
characters, and CJK names inside unspaced CJK text, come out "unknown", which errs toward asking. After a restart the
request's words are gone (the owner's owner.ask answers and outside content are still matched), so a target only
in the request's words comes out "unknown"."""

import re
import unicodedata
from dataclasses import dataclass
from pathlib import Path

from ferb.core.types import Target
from ferb.policy import urls

MIN_LEN = 3
MIN_PHONE_DIGITS = 6
DESTINATION_PHONE_DIGITS = 8      # a phone number as a destination's shape (typed, or in the owner's words): shorter
                                  # runs are codes, and dates are left out
_LEAD, _TRAIL = "\"'([{<", "\"')]}>,;:!?."
_AMOUNT = re.compile(r"(\d+(?:\.\d+)?|\d{1,3}(?:,\d{3})+(?:\.\d+)?)")
_PHONE_VALUE = re.compile(r"[\d\s\-+().]+")
_PHONE_RUN = re.compile(r"[+\d][\d\s\-().]{4,}\d")
_DATE = re.compile(r"\d{4}[-./]\d{1,2}[-./]\d{1,2}|\d{1,2}[-./]\d{1,2}[-./]\d{2,4}")
_STRUCTURED = ("account", "site", "path")


def _word(ch: str) -> bool:
    cat = unicodedata.category(ch)
    return cat[0] in "LNM" or cat == "Co"


def _strip(raw: str) -> tuple[bool, str, bool]:
    """A whitespace-delimited token without its leading and trailing punctuation: (had leading, token, had trailing)."""
    tok, lead, trail = raw, False, False
    while tok and (tok[0] in _LEAD or tok[-1] in _TRAIL):
        if tok[0] in _LEAD:
            tok, lead = tok[1:], True
        else:
            tok, trail = tok[:-1], True
    return lead, tok, trail


def raw_tokens(text: str) -> list[str]:
    """Whole tokens without framing punctuation, preserving case so URL hosts undergo UTS46 before case mapping."""
    out = []
    for raw in text.split():
        tok = _strip(raw)[1]
        if tok:
            out.append(tok)
    return out


def tokens(text: str) -> list[str]:
    """The text's whole tokens, lowercased, without leading and trailing punctuation."""
    return [tok.lower() for tok in raw_tokens(text)]     # lower(), not casefold(): strasse.de is not straße.de


def phone_runs(text: str) -> list[str]:
    """The digits of each run in the text that has a phone number's destination shape (`_phone_shape`), in order."""
    return [_digits(m.group(0)) for m in _PHONE_RUN.finditer(text) if _phone_shape(m.group(0))]


def _phone_shape(value: str) -> bool:
    """Digits, spaces and + - ( ) . only, at least DESTINATION_PHONE_DIGITS digits, and not a date's shape."""
    v = value.strip()
    return bool(_PHONE_VALUE.fullmatch(v)) and len(_digits(v)) >= DESTINATION_PHONE_DIGITS and not _DATE.fullmatch(v)


def host_of(token: str) -> str:
    """The host of a token that is a URL (has ://), as a browser would open it (ferb/policy/urls.py: IDNA-encoded,
    lowercased, numeric IPv4 made dotted), so a host the owner wrote as `bücher.example` matches the sink's
    `xn--bcher-kva.example` (N4); "" when the token names none."""
    if "://" not in token:
        return ""
    parsed = urls.parse(token)
    return parsed.host if parsed is not None else ""


def _token_amount(token: str) -> float | None:
    if token and unicodedata.category(token[0]) == "Sc":     # one leading currency symbol, nothing else
        token = token[1:]
    if not _AMOUNT.fullmatch(token):
        return None
    return float(token.replace(",", ""))


def _norm_path(token: str) -> str:
    return token[:-1] if len(token) > 1 and token.endswith("/") else token


def _digits(text: str) -> str:
    return "".join(c for c in text if c.isdigit())


@dataclass(frozen=True)
class Word:
    """One word of a text, for telling where a name in it ends (`_named_by_words`): shapes only."""
    folded: str
    capital: bool             # typed with a capital first letter
    sentence_start: bool      # the first word of the text or of a line, or after . ! or ?
    tight: bool               # joined to the word before it with nothing but spaces (no punctuation, same line)


def _words(text: str) -> tuple[Word, ...]:
    out: list[Word] = []
    for line in text.splitlines():
        prev = ""                                      # the raw token before, "" at a line's start
        joined = False                                 # the token before ended without punctuation
        for raw in line.split():
            lead, tok, trail = _strip(raw)
            if tok:
                out.append(Word(tok.casefold(), tok[0].isupper(), not prev or prev[-1] in ".!?", joined and not lead))
            prev, joined = raw, bool(tok) and not trail
    return tuple(out)


@dataclass(frozen=True)
class Text:
    """One text, folded and tokenized once (`prepare`); every target is matched against it."""
    folded: str
    tokens: frozenset[str]
    paths: frozenset[str]
    hosts: frozenset[str]
    amounts: tuple[float, ...]
    phones: frozenset[str]
    words: tuple[Word, ...] = ()


def prepare(text: str) -> Text:
    toks = tokens(text)
    return Text(text.casefold(), frozenset(toks), frozenset(_norm_path(t) for t in toks),
                 frozenset(h for t in raw_tokens(text) if (h := host_of(t))),
                 tuple(n for t in toks if (n := _token_amount(t)) is not None),
                 frozenset(_digits(m.group(0)) for m in _PHONE_RUN.finditer(text)), _words(text))


def _contains(folded: str, needle: str) -> bool:
    start = folded.find(needle)
    while start != -1:
        end = start + len(needle)
        if (start == 0 or not _word(folded[start - 1])) and (end == len(folded) or not _word(folded[end])):
            return True
        start = folded.find(needle, start + 1)
    return False


def target_values(target: Target) -> dict[str, str]:
    out = {k: str(getattr(target, k)) for k in ("account", "site", "path", "contact") if getattr(target, k)}
    out |= {f"recipient:{i}": str(r) for i, r in enumerate(target.recipients)}
    if target.amount is not None:
        out["amount"] = str(target.amount)
    return out


def _matches(kind: str, value: str, aliases: tuple[str, ...], text: Text) -> bool:
    if kind == "amount":
        return any(abs(n - float(value)) < 1e-9 for n in text.amounts)
    if kind == "name" and _PHONE_VALUE.fullmatch(value) and len(_digits(value)) >= MIN_PHONE_DIGITS:
        return _digits(value) in text.phones
    needles = [v.lower() for v in (value, *aliases) if len(v.strip()) >= MIN_LEN]
    if kind == "name" and not any(c.isspace() for c in value):
        kind = "token"
    if kind == "name":                                   # a multi-word name: whole phrase, or by the words used for it
        return any(_contains(text.folded, n.casefold()) for n in needles) or _named_by_words(value, text.words)
    if any(n in text.tokens or (kind == "structured" and _norm_path(n) in text.paths) for n in needles):
        return True
    return kind == "structured" and any(n in text.hosts for n in needles)


def _named_by_words(value: str, words: tuple[Word, ...]) -> bool:
    """A multi-word name is named in the text when every word the text uses for it is a whole word of it, and one of
    those words has at least MIN_LEN characters (OPEN QUESTION 4). The text's name for it is the run of words joined to
    a word of the value (punctuation and line breaks end it), told by shape: from a word typed with a capital, the run
    takes in joined words that are words of the value or capitalized (not a sentence's first word, which any word may
    be); from a word typed in lower case, or in a script without capitals, FERB can't see where the name ends, so the
    run takes in every joined word, and the value is named only when they are all its words. So "message Rahul"
    names "Rahul Verma"; "message Rahul Verma" doesn't name "Rahul Attacker"; "message neha jainson" doesn't name
    "Neha Jain", and neither does "message neha" (lower case): that errs toward asking."""
    wanted = {t.casefold() for raw in value.split() if (t := _strip(raw)[1])}
    for i, word in enumerate(words):
        if word.folded not in wanted:
            continue
        lo = hi = i
        while lo > 0 and words[lo].tight and _joins(words[lo - 1], wanted, word.capital):
            lo -= 1
        while hi + 1 < len(words) and words[hi + 1].tight and _joins(words[hi + 1], wanted, word.capital):
            hi += 1
        run = [w.folded for w in words[lo:hi + 1]]
        if all(w in wanted for w in run) and any(len(w) >= MIN_LEN for w in run):
            return True
    return False


def _joins(other: Word, wanted: set[str], capital: bool) -> bool:
    """Whether a word joined to a run belongs to the text's name for the value (`_named_by_words`)."""
    return other.folded in wanted or not capital or (other.capital and not other.sentence_start)


_EMAIL = re.compile(r"[^\s@]+@[^\s@]+\.[^\s@]+")


def destination_shape(text: str) -> str | None:
    """The shape of a value: "email", "phone" or "url" when the whole value (spaces around it aside) is one email
    address, one phone number (digits, spaces and + - ( ) ., at least DESTINATION_PHONE_DIGITS digits, not a date) or
    one scheme:// URL with a host; else None. A shape, never a meaning (rule 6)."""
    value = text.strip()
    if not value:
        return None
    if not any(c.isspace() for c in value):
        if _EMAIL.fullmatch(value):
            return "email"
        if host_of(value):
            return "url"
    return "phone" if _phone_shape(value) else None


def typed_destination(text: str) -> str | None:
    """The recipient a typed text is, without the punctuation around it ("<a@x.com>", "a@x.com,", "+91 98765
    43210;"): the value when it is an email address or a phone number, else None. A phone number keeps its own
    parentheses. A typed URL is no recipient (GATE-509): it addresses no one, typed into a field it is data, and opening
    it is navigation, which the browser's own address check (the sink rule) and the foreground question cover."""
    value = text.strip()
    if _phone_shape(value):
        return value
    stripped = _strip(value)[1]
    return stripped if stripped and destination_shape(stripped) in ("email", "phone") else None


def sources(value: str, owner_text: str, external_texts: list[tuple[str, str]], aliases: tuple[str, ...] = (),
            kind: str = "name", answer_text: str = "") -> list[str]:
    """Where the value appears: "owner_words" (the request's words), "owner_answer" (the owner's owner.ask answers),
    the external origins that contain it, any of these together, or ["unknown"]. `kind` is "name" (by shape: phone,
    token or phrase), "structured" (whole token, URL host, path forms) or "amount"."""
    return _sources(kind, value, aliases, prepare(owner_text), prepare(answer_text),
                    [(o, prepare(t)) for o, t in external_texts])


def _sources(kind: str, value: str, aliases: tuple[str, ...], owner: Text, answers: Text,
             external: list[tuple[str, Text]]) -> list[str]:
    found = ["owner_words"] if _matches(kind, value, aliases, owner) else []
    found += ["owner_answer"] if _matches(kind, value, aliases, answers) else []
    found += sorted({o for o, text in external if _matches(kind, value, aliases, text)})
    return found or ["unknown"]


def arg_sources(target: Target, owner_text: str, external_texts: list[tuple[str, str]],
                answer_text: str = "") -> dict[str, list[str]]:
    return arg_sources_prepared(target, prepare(owner_text), [(o, prepare(t)) for o, t in external_texts],
                                prepare(answer_text))


def arg_sources_prepared(target: Target, owner: Text, external: list[tuple[str, Text]],
                         answers: Text | None = None) -> dict[str, list[str]]:
    """arg_sources over texts already prepared: the gateway keeps a task's outside texts prepared between calls."""
    answers = answers if answers is not None else prepare("")
    out: dict[str, list[str]] = {}
    for key, value in target_values(target).items():
        aliases: tuple[str, ...] = ()
        if key == "amount":
            kind = "amount"
        elif key in _STRUCTURED or (key.startswith("recipient:") and "@" in value):
            kind = "structured"
            home = str(Path.home())
            if key == "path" and value.startswith(home + "/"):
                aliases = ("~" + value[len(home):],)
        else:
            kind = "name"
        out[key] = _sources(kind, value, aliases, owner, answers, external)
    return out
