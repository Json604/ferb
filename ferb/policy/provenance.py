"""Owns matching a consequential call's targets against where they could have come from (S3). Must not decide or block.

Structured values (account, site, path, recipient addresses) match only as a whole whitespace-delimited token of the
owner's words or of outside content (case-insensitive; a site also matches the host of a URL token; a path also
matches its ~ form). A contact or recipient without @ is, by shape: a phone number (digits, spaces and + - ( ) .,
at least 6 digits), matched when its digits equal the digits of a phone-like run in the text, exactly; a value with
no whitespace, matched as a whole token; or a multi-word name, matched as a whole phrase with word boundaries by
Unicode category so Hindi words stay whole. Amounts match a token that is wholly a number (one leading currency
symbol allowed), by value.
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
from urllib.parse import urlsplit

from ferb.core.types import Target

MIN_LEN = 3
MIN_PHONE_DIGITS = 6
_LEAD, _TRAIL = "\"'([{<", "\"')]}>,;:!?."
_AMOUNT = re.compile(r"(\d+(?:\.\d+)?|\d{1,3}(?:,\d{3})+(?:\.\d+)?)")
_PHONE_VALUE = re.compile(r"[\d\s\-+().]+")
_PHONE_RUN = re.compile(r"[+\d][\d\s\-().]{4,}\d")
_STRUCTURED = ("account", "site", "path")


def _word(ch: str) -> bool:
    cat = unicodedata.category(ch)
    return cat[0] in "LNM" or cat == "Co"


def _tokens(text: str) -> list[str]:
    out = []
    for raw in text.lower().split():                     # lower(), not casefold(): strasse.de is not straße.de
        tok = raw
        while tok and (tok[0] in _LEAD or tok[-1] in _TRAIL):
            tok = tok[1:] if tok[0] in _LEAD else tok[:-1]
        if tok:
            out.append(tok)
    return out


def _host(token: str) -> str:
    if "://" not in token:
        return ""
    try:
        return (urlsplit(token).hostname or "").lower()
    except ValueError:
        return ""


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
class Text:
    """One text, folded and tokenized once (`prepare`); every target is matched against it."""
    folded: str
    tokens: frozenset[str]
    paths: frozenset[str]
    hosts: frozenset[str]
    amounts: tuple[float, ...]
    phones: frozenset[str]


def prepare(text: str) -> Text:
    toks = _tokens(text)
    return Text(text.casefold(), frozenset(toks), frozenset(_norm_path(t) for t in toks),
                 frozenset(h for t in toks if (h := _host(t))),
                 tuple(n for t in toks if (n := _token_amount(t)) is not None),
                 frozenset(_digits(m.group(0)) for m in _PHONE_RUN.finditer(text)))


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
    if kind == "name":                                   # a multi-word name: whole phrase
        return any(_contains(text.folded, n.casefold()) for n in needles)
    if any(n in text.tokens or (kind == "structured" and _norm_path(n) in text.paths) for n in needles):
        return True
    return kind == "structured" and any(n in text.hosts for n in needles)


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
