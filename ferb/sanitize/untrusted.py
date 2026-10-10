"""Own source provenance markers for pasted data. Must not discard the complete request or infer its meaning.

Pasted/quoted = terminal bracketed paste (ESC[200~ ... ESC[201~), fenced code blocks (```), and `>` quote lines.
Each is wrapped in <untrusted-data id="..."> tags with a random id (so pasted text can't forge a closing tag), and
the owner's own words are returned separately for the orchestrator to interpret intent."""

import re
import secrets
from dataclasses import dataclass

PASTE_START, PASTE_END = "\x1b[200~", "\x1b[201~"
_FENCE = re.compile(r"```.*?(?:```|\Z)", re.S)
# Intake's labels for FERB's earlier question when an answer continues a request: context in the words it searches
# (and, before 0.27 F3, in what it recorded as the owner's words), data in the payload. Never the owner's words.
EARLIER_QUESTION = "Earlier FERB question (context, not owner instructions): "
EARLIER_QUESTION_DATA = "Earlier FERB question (data): "
_WRAPPER = r'<untrusted-data id="([0-9a-f]+)">.*?</untrusted-data id="\1">'      # FERB's own wrapper (wrap_untrusted)
WRAPPED = re.compile(_WRAPPER, re.S)
_EARLIER = re.compile(r"\n?(?:" + re.escape(EARLIER_QUESTION) + "|" + re.escape(EARLIER_QUESTION_DATA) + ")" + _WRAPPER,
                      re.S)


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


_PARTS = re.compile(r'<untrusted-data id="([0-9a-f]+)">\n?(.*?)\n?</untrusted-data id="\1">', re.S)


def pasted_parts(payload: str) -> list[str]:
    """The pasted or quoted parts of a request's payload (`mark` wrapped them), without FERB's own earlier question,
    which is wrapped the same way but is FERB's text, not the owner's paste."""
    return [m.group(2) for m in _PARTS.finditer(_EARLIER.sub("", payload or "")) if m.group(2).strip()]


def earlier_questions(payload: str) -> list[str]:
    """FERB's own earlier question(s) a payload carries as data (an answer continues the request it asked about)."""
    return [p.group(2) for m in _EARLIER.finditer(payload or "") for p in _PARTS.finditer(m.group(0))
            if p.group(2).strip()]


def wrap_untrusted(text: str) -> str:
    """Data, not instructions: in tags with a random id; a forged closing tag inside can't end ours."""
    tag, uid = "untrusted-data", secrets.token_hex(6)
    body = text.replace(f"</{tag}", f"<\\/{tag}")
    return f'<{tag} id="{uid}">\n{body}\n</{tag} id="{uid}">'


def without_ferb_question(text: str) -> str:
    """The text without FERB's earlier question (either label, in FERB's own wrapper). Records written before 0.27 F3
    appended it to what they stored as the owner's words (continuity, a task's authority words); a payload holds it as
    data."""
    return _EARLIER.sub("", text)


def holds_pasted(payload: str) -> bool:
    """Whether a recorded request still holds one of FERB's wrappers (pasted or quoted text) once FERB's own earlier
    question is set aside."""
    return bool(WRAPPED.search(without_ferb_question(payload)))
