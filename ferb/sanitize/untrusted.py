"""Mark pasted or quoted content in the owner's chat message as untrusted data (section 10.2).

Pasted/quoted = terminal bracketed paste (ESC[200~ ... ESC[201~), fenced code blocks (```), and `>` quote lines.
Each is wrapped in <untrusted-data id="..."> tags with a random id (so pasted text can't forge a closing tag), and
the owner's own words are returned separately: the intent may only use targets that appear in them.
"""

import re
import secrets
from dataclasses import dataclass

PASTE_START, PASTE_END = "\x1b[200~", "\x1b[201~"
_FENCE = re.compile(r"```.*?(?:```|\Z)", re.S)


@dataclass(frozen=True)
class Marked:
    for_model: str        # the message with untrusted parts wrapped in tags
    own_words: str        # only the owner's own words (untrusted parts removed)
    untrusted: list[str]  # the untrusted parts, raw


def mark(message: str, pasted_is_data: bool = True) -> Marked:
    """Split the owner's message into their own words and untrusted parts. `pasted_is_data=False` (the owner's switch,
    `[policy] pasted_text_is_data = false`, 2026-10-01): everything they send counts as their own words."""
    if not pasted_is_data:
        plain = message.replace(PASTE_START, "").replace(PASTE_END, "")
        return Marked(plain.strip(), " ".join(plain.split()), [])
    return _mark(message)


def _mark(message: str) -> Marked:
    parts: list[tuple[bool, str]] = []                         # (untrusted?, text)
    rest = message
    while PASTE_START in rest:                                # 1) bracketed paste
        before, _, after = rest.partition(PASTE_START)
        pasted, sep, rest = after.partition(PASTE_END)
        parts.append((False, before))
        parts.append((True, pasted))
        if not sep:                                           # unterminated paste: all the rest is pasted
            rest = ""
    parts.append((False, rest.replace(PASTE_END, "")))

    split: list[tuple[bool, str]] = []
    for untrusted, text in parts:                             # 2) fenced blocks, 3) > quotes, in own words only
        if untrusted:
            split.append((True, text))
            continue
        pos = 0
        for m in _FENCE.finditer(text):
            split += _quotes(text[pos:m.start()])
            split.append((True, m.group(0)))
            pos = m.end()
        split += _quotes(text[pos:])

    out, own, bad = [], [], []
    for untrusted, text in split:
        if not text:
            continue
        if untrusted:
            out.append(f"\n{wrap_untrusted(text)}\n")
            bad.append(text)
        else:
            out.append(text)
            own.append(text)
    return Marked("".join(out).strip(), " ".join(" ".join(own).split()), bad)


def _quotes(text: str) -> list[tuple[bool, str]]:
    out: list[tuple[bool, str]] = []
    block: list[str] = []
    normal: list[str] = []

    def flush_block() -> None:
        if block:
            out.append((True, "\n".join(block)))
            block.clear()

    def flush_normal() -> None:
        if normal:
            out.append((False, "\n".join(normal)))
            normal.clear()

    for line in text.split("\n"):
        if line.lstrip().startswith(">"):
            flush_normal()
            block.append(line)
        else:
            flush_block()
            normal.append(line)
    flush_block()
    flush_normal()
    return out


def wrap_untrusted(text: str) -> str:
    """Data, not instructions: in tags with a random id; a forged closing tag inside can't end ours."""
    tag, uid = "untrusted-data", secrets.token_hex(6)
    body = text.replace(f"</{tag}", f"<\\/{tag}")
    return f'<{tag} id="{uid}">\n{body}\n</{tag} id="{uid}">'


def targets_in_own_words(targets: list[str], own_words: str) -> list[str]:
    """Targets that do NOT appear in the owner's own words (case-insensitive). Empty list = all present."""
    lw = own_words.lower()
    return [t for t in targets if t.strip() and t.strip().lower() not in lw]
