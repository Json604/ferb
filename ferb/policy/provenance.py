"""Owns matching a consequential call's targets against where they could have come from (S3). Must not decide or block.

A recipient, contact, site, path, account or amount is matched as a whole value (case-insensitive; word boundaries by
Unicode category, so Hindi words stay whole) against the owner's words for the request and against the text of
outside content the task has read. Paths also match the owner's own spelling (~ or as typed); amounts match by value.
The result is a fact the trace shows and F5's confirmations use; nothing here judges intent."""

import re
import unicodedata
from pathlib import Path
from typing import Any

from ferb.core.types import Target

MIN_LEN = 3
_NUMBER = re.compile(r"\d[\d,]*(?:\.\d+)?")


def _word(ch: str) -> bool:
    cat = unicodedata.category(ch)
    return cat[0] in "LNM" or cat == "Co"


def _contains(text: str, needle: str) -> bool:
    text, needle = text.casefold(), needle.casefold()
    start = text.find(needle)
    while start != -1:
        end = start + len(needle)
        if (start == 0 or not _word(text[start - 1])) and (end == len(text) or not _word(text[end])):
            return True
        start = text.find(needle, start + 1)
    return False


def _has_number(text: str, value: float) -> bool:
    for m in _NUMBER.finditer(text):
        try:
            if abs(float(m.group(0).replace(",", "")) - value) < 1e-9:
                return True
        except ValueError:
            continue
    return False


def target_values(target: Target) -> dict[str, str]:
    out = {k: str(getattr(target, k)) for k in ("account", "site", "path", "contact") if getattr(target, k)}
    out |= {f"recipient:{i}": str(r) for i, r in enumerate(target.recipients)}
    if target.amount is not None:
        out["amount"] = str(target.amount)
    return out


def sources(value: str, owner_text: str, external_texts: list[tuple[str, str]], aliases: tuple[str, ...] = (),
            numeric: bool = False) -> list[str]:
    if numeric:
        amount = float(value)
        found = ["owner_words"] if _has_number(owner_text, amount) else []
        found += sorted({o for o, text in external_texts if _has_number(text, amount)})
        return found or ["unknown"]
    needles = [v for v in (value, *aliases) if len(v.strip()) >= MIN_LEN]
    if not needles:
        return ["unknown"]
    found = ["owner_words"] if any(_contains(owner_text, n) for n in needles) else []
    found += sorted({o for o, text in external_texts if any(_contains(text, n) for n in needles)})
    return found or ["unknown"]


def arg_sources(target: Target, owner_text: str, external_texts: list[tuple[str, str]],
                raw_path: str | None = None) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for key, value in target_values(target).items():
        aliases: tuple[str, ...] = ()
        if key == "path":
            home = str(Path.home())
            tilde = "~" + value[len(home):] if value.startswith(home + "/") else ""
            aliases = tuple(a for a in (raw_path or "", tilde) if a and a != value)
        out[key] = sources(value, owner_text, external_texts, aliases, numeric=key == "amount")
    return out
