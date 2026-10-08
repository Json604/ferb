"""Owns matching a consequential call's targets against where they could have come from (S3). Must not decide or block.

Structured values (account, site, path, recipient addresses) match only as a whole whitespace-delimited token of the
owner's words or of outside content (case-insensitive; a site also matches the host of a URL token; a path also
matches its ~ form). Names (contact, recipients without @) match as a whole phrase, with word boundaries by Unicode
category so Hindi words stay whole. Amounts match a token that is wholly a number, by value.
The result is a fact the trace shows and F5's confirmations use; nothing here judges intent. Values under 3
characters, and CJK names inside unspaced CJK text, come out "unknown", which errs toward asking. After a restart the
owner's words for a resumed task are gone, so everything is "unknown"."""

import re
import unicodedata
from pathlib import Path
from urllib.parse import urlsplit

from ferb.core.types import Target

MIN_LEN = 3
_LEAD, _TRAIL = "\"'([{<", "\"')]}>,;:!?."
_AMOUNT = re.compile(r"(\d+(?:\.\d+)?|\d{1,3}(?:,\d{3})+(?:\.\d+)?)")
_STRUCTURED = ("account", "site", "path")


def _word(ch: str) -> bool:
    cat = unicodedata.category(ch)
    return cat[0] in "LNM" or cat == "Co"


def _tokens(text: str) -> list[str]:
    out = []
    for raw in text.casefold().split():
        tok = raw
        while tok and (tok[0] in _LEAD or tok[-1] in _TRAIL):
            tok = tok[1:] if tok[0] in _LEAD else tok[:-1]
        if tok:
            out.append(tok)
    return out


def _contains(folded: str, needle: str) -> bool:
    needle = needle.casefold()
    start = folded.find(needle)
    while start != -1:
        end = start + len(needle)
        if (start == 0 or not _word(folded[start - 1])) and (end == len(folded) or not _word(folded[end])):
            return True
        start = folded.find(needle, start + 1)
    return False


def _host(token: str) -> str:
    if "://" not in token:
        return ""
    try:
        return (urlsplit(token).hostname or "").casefold()
    except ValueError:
        return ""


def _token_amount(token: str) -> float | None:
    if token and not _word(token[0]) and not token[0].isspace():
        token = token[1:]                                # one leading currency sign
    if not _AMOUNT.fullmatch(token):
        return None
    return float(token.replace(",", ""))


def _norm_path(token: str) -> str:
    return token[:-1] if len(token) > 1 and token.endswith("/") else token


def target_values(target: Target) -> dict[str, str]:
    out = {k: str(getattr(target, k)) for k in ("account", "site", "path", "contact") if getattr(target, k)}
    out |= {f"recipient:{i}": str(r) for i, r in enumerate(target.recipients)}
    if target.amount is not None:
        out["amount"] = str(target.amount)
    return out


def _matches(kind: str, value: str, aliases: tuple[str, ...], text: str, folded: str, tokens: list[str]) -> bool:
    if kind == "amount":
        amount = float(value)
        return any((n := _token_amount(t)) is not None and abs(n - amount) < 1e-9 for t in tokens)
    needles = [v.casefold() for v in (value, *aliases) if len(v.strip()) >= MIN_LEN]
    if not needles:
        return False
    if kind == "structured":
        if any(n in tokens for n in needles):
            return True
        if any(_norm_path(n) in {_norm_path(t) for t in tokens} for n in needles):
            return True
        return any(_host(t) in needles for t in tokens)
    return any(_contains(folded, n) for n in needles)


def sources(value: str, owner_text: str, external_texts: list[tuple[str, str]], aliases: tuple[str, ...] = (),
            kind: str = "name") -> list[str]:
    """["owner_words"], the external origins that contain the value, both, or ["unknown"]. `kind` is "name"
    (phrase match), "structured" (whole token) or "amount" (numeric token)."""
    def hit(text: str) -> bool:
        folded = text.casefold()
        return _matches(kind, value, aliases, text, folded, _tokens(text))
    found = ["owner_words"] if hit(owner_text) else []
    found += sorted({o for o, text in external_texts if hit(text)})
    return found or ["unknown"]


def arg_sources(target: Target, owner_text: str, external_texts: list[tuple[str, str]]) -> dict[str, list[str]]:
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
        out[key] = sources(value, owner_text, external_texts, aliases, kind)
    return out
