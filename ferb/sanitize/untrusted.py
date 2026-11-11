"""Own source provenance markers for outside data in what FERB records and shows the models. Must not discard the
complete request or infer its meaning.

The owner's chat message is all theirs (owner 2026-11-04, ORCH-518: "(b) pastes count as your words"): what they type,
paste, quote with `>` or put in a code block. `mark` records it whole, only cleaned of bracketed-paste markers. Outside
content FERB shows a model (FERB's own earlier question, attachment paths, tool results) is wrapped in
<untrusted-data id="..."> tags with a random id (so data can't forge a closing tag); messages recorded before
2026-11-04 may still hold pasted parts wrapped that way, and readers of old records leave those out."""

import json
import re
import secrets
import unicodedata
from dataclasses import dataclass

PASTE_START, PASTE_END = "\x1b[200~", "\x1b[201~"
# A bracketed-paste marker that leaked in as text (copied from a terminal, with or without its ESC byte): pure
# clean-up, never a reading of what the text means (owner 2026-11-04)
_LEAKED = re.compile(r"^(?:[ \t]*\x1b?\[20[01]~)+|(?:\x1b?\[20[01]~[ \t]*)+$", re.M)
_LEAKED_BARE = re.compile(r"^(?:[ \t]*\[20[01]~)+|(?:\[20[01]~[ \t]*)+$", re.M)
_LEAKED_PAIR = re.compile(r"\[200~(.*?)\[201~", re.S)              # a whole leaked paste, wherever it sits
_MARKERS = re.compile("(" + re.escape(PASTE_START) + "|" + re.escape(PASTE_END) + ")")
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
    for_model: str        # the message as recorded and shown to the planner
    own_words: str        # the owner's words, on one line
    untrusted: list[str]  # parts held as outside data: none in a message since ORCH-518
    # the owner's words with their line breaks: what memory capture saves from, so a saved clause ends at a line break
    typed_lines: str = ""


def _lines(parts: list[str]) -> str:
    return "\n".join(" ".join(line.split()) for part in parts for line in part.split("\n") if line.strip())


def typed_lines_of(for_model: str) -> str:
    """The owner's words with their line breaks (`Marked.typed_lines`) read back from a message as stored
    (`Marked.for_model`): a part an older FERB wrapped as pasted is left out and ends a line (L36 §2: distillation
    checks its quotes against these)."""
    parts, pos = [], 0
    for m in WRAPPED.finditer(for_model):
        parts.append(for_model[pos:m.start()])
        pos = m.end()
    parts.append(for_model[pos:])
    return _lines(parts)


def without_leaked_markers(message: str) -> str:
    """The message without bracketed-paste markers that leaked in as text ("[200~remind me …[201~", copied from a
    terminal): a pair around text wherever it sits, a lone one at a line's start or end. The composer's own markers
    (ESC[200~ … ESC[201~) stay; a leaked one just inside them goes too."""
    out, inside = [], False
    for piece in _MARKERS.split(message):
        if piece in (PASTE_START, PASTE_END):
            inside = piece == PASTE_START
            out.append(piece)
        else:
            out.append((_LEAKED if inside else _LEAKED_BARE).sub("", _LEAKED_PAIR.sub(r"\1", piece)))
    return "".join(out)


def mark(message: str) -> Marked:
    """The owner's chat message as FERB records it (ORCH-518): all of it is their words, whatever they typed, pasted
    (the composer still brackets a paste, for its own local commands, APP-596), quoted or fenced. Only leaked paste
    markers (`without_leaked_markers`) and the composer's brackets are removed."""
    plain = without_leaked_markers(message).replace(PASTE_START, "").replace(PASTE_END, "")
    return Marked(plain.strip(), " ".join(plain.split()), [], _lines([plain]))


_PARTS = re.compile(r'<untrusted-data id="([0-9a-f]+)">\n?(.*?)\n?</untrusted-data id="\1">', re.S)


def pasted_parts(payload: str) -> list[str]:
    """The pasted or quoted parts of a request's payload (`mark` wrapped them), without FERB's own earlier question,
    which is wrapped the same way but is FERB's text, not the owner's paste."""
    return [m.group(2) for m in _PARTS.finditer(_EARLIER.sub("", payload or "")) if m.group(2).strip()]


def earlier_questions(payload: str) -> list[str]:
    """FERB's own earlier question(s) a payload carries as data (an answer continues the request it asked about)."""
    return [p.group(2) for m in _EARLIER.finditer(payload or "") for p in _PARTS.finditer(m.group(0))
            if p.group(2).strip()]


def as_data(text: str) -> str:
    """Text shown as one quoted JSON string: its line breaks escaped, so it can't start a line of its own (a history
    line in FERB's format, say), and every character that is invisible or breaks a line written out as an escape:
    format characters (Unicode Tag characters, zero-width marks), private-use, controls (DEL, NEL), the line and
    paragraph separators U+2028/U+2029, and lone surrogates (which Codex's JSON reader refuses). So nothing in it is
    hidden from the reader, and it is always one line. Deterministic, unlike `wrap_untrusted`, so a recorded line stays
    the same across turns and its prompt prefix stays cacheable (P20)."""
    return "".join(_shown(c) for c in json.dumps(text, ensure_ascii=False))


_ESCAPED = ("Cf", "Co", "Cc", "Zl", "Zp", "Cs")


def _shown(c: str) -> str:
    """A character as written in `as_data`: itself, or an escape when it is invisible or breaks a line."""
    if unicodedata.category(c) not in _ESCAPED:
        return c
    return f"\\u{ord(c):04x}" if ord(c) <= 0xFFFF else f"\\U{ord(c):08x}"


def wrap_untrusted(text: str, uid: str = "") -> str:
    """Data, not instructions: in tags with a random id; a forged closing tag inside can't end ours. `uid`: a fixed id
    for a block that must read the same on every call (a cached prefix); the escape alone keeps it closed."""
    tag, uid = "untrusted-data", uid or secrets.token_hex(6)
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


def recorded_own_words(for_model: str, pasted: int | None) -> str | None:
    """The owner's own words of a recorded chat message (`Marked.for_model`): its pasted or quoted parts, in FERB's
    wrapper, left out. `pasted`: how many such parts were recorded with it. None (nothing counts as the owner's words)
    when that count is unknown or the wrappers found differ from it, so a damaged record never passes pasted text off
    as the owner's."""
    if pasted is None or len(WRAPPED.findall(for_model)) != pasted:
        return None
    return " ".join(WRAPPED.sub(" ", for_model).split())
