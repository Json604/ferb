"""Owns the one structural change FERB's code makes to a worker's answer after a check (ORCH-545): removing a sentence
the checker quoted, or marking it inline "(I couldn't confirm this)". Must not judge meaning (rule 4): it only finds the
quoted text in the answer and removes or marks that occurrence; which of the two follows from the checker's own label of
the finding (contradicted: removed; unsupported: marked). It also tells whether a fix turn's revised answer changed
anything outside the quoted sentences (`only_quoted_changed`). Must not write new words into the answer."""

import re
from collections.abc import Iterable
from dataclasses import dataclass, field

UNCONFIRMED_MARK = "(I couldn't confirm this)"            # the one inline marker FERB's code may add
# ORCH-605: code's own opening sentence of an unfinished or stopped task's reply; the worker's text follows as its own
# paragraph
NOT_FINISHED = "I couldn't finish this task."
STOPPED = "Stopped, as you asked."
WHERE_STOOD = "This is where things stood:"      # ORCH-611: after STOPPED, before the worker's answer
# ORCH-613: the whole reply of a task the owner declined on its own card when the worker wrote nothing to say so
DECLINED_LINE = "Not done, as you asked."


def headline(text: str) -> str:
    """An answer's first non-empty line as a short form reads it (the planner's history, a schedule's last run, what
    happened since): when that line is only code's own opener (NOT_FINISHED, STOPPED), the next non-empty line joins
    it, so the short form says what happened. Structure only: code's own fixed sentences, never the model's words."""
    lines = [line.strip() for line in str(text or "").splitlines() if line.strip()]
    if len(lines) > 1 and lines[0] in (NOT_FINISHED, STOPPED, f"{STOPPED} {WHERE_STOOD}"):
        return f"{lines[0]} {lines[1]}"
    return lines[0] if lines else ""


@dataclass
class Settled:
    """An answer after code removed or marked the quoted sentences: what was removed, marked, or not found."""
    answer: str
    removed: list[str] = field(default_factory=list)
    marked: list[str] = field(default_factory=list)
    missing: list[str] = field(default_factory=list)
    kept_contradicted: list[str] = field(default_factory=list)   # contradicted, but marked: removing emptied the answer


def removes_or_marks(find: str, replace: str) -> bool:
    """Whether an older checker edit ({find, replace}, before ORCH-545) only removes its text or marks it unconfirmed:
    `replace` is empty, or `find` with UNCONFIRMED_MARK inserted once (a typographic apostrophe counts as a plain one).
    Anything else would have written new words into the owner's answer. Structure, not content."""
    if not replace:
        return True
    plain, mark = replace.replace("’", "'"), UNCONFIRMED_MARK
    at = plain.find(mark)
    if at < 0:
        return False
    rest = plain[:at] + plain[at + len(mark):]
    target = find.replace("’", "'")
    return rest == target or (at > 0 and rest[:at - 1] + rest[at:] == target and plain[at - 1] == " ")


def mark_unconfirmed(answer: str, find: str) -> str:
    """The answer with the first occurrence of `find` marked inline with UNCONFIRMED_MARK, before its closing
    punctuation ("It opens at 10am (I couldn't confirm this)."); unchanged when that occurrence is already marked or
    `find` isn't in the answer. Structure only: no word is judged."""
    at = answer.find(find)
    if at < 0:
        return answer
    end = at + len(find)
    body = find.rstrip()
    tail = len(body) - len(body.rstrip(".!?;:,"))
    core, stop = body[:len(body) - tail].rstrip(), body[len(body) - tail:]
    after = answer[at + len(core):].lstrip()
    if after.replace("’", "'").startswith(UNCONFIRMED_MARK):
        return answer
    return answer[:at] + core + " " + UNCONFIRMED_MARK + stop + find[len(body):] + answer[end:]


def remove_quote(answer: str, find: str) -> str:
    """The answer without the first occurrence of `find`, and without the doubled space or blank line that leaves."""
    at = answer.find(find)
    if at < 0:
        return answer
    before, after = answer[:at], answer[at + len(find):]
    if before.endswith(" ") and (not after or after[0] in " \n.,;:!?"):
        before = before[:-1]
    joined = before + after
    while "\n\n\n" in joined:
        joined = joined.replace("\n\n\n", "\n\n")
    return joined.strip()


def settle(answer: str, findings: Iterable[tuple[str, str]]) -> Settled:
    """The answer with each quoted sentence removed (kind "contradicted") or marked once (kind "unsupported"), in the
    order given; a quote not in the answer (blank, or already gone) changes nothing and is listed as missing. A removal
    never leaves the answer empty (ORCH-547): when removing the contradicted sentences would leave no letter or digit
    (nothing, or only a list marker or emphasis such as "-" or "**"), they are marked instead, so the owner is never
    told "done" with nothing said; `kept_contradicted` then lists the contradicted ones left standing, marked."""
    findings = list(findings)
    done = _settled(answer, findings)
    if done.removed and not has_text(done.answer):
        done = _settled(answer, [(quote, "unsupported") for quote, _ in findings])
        done.kept_contradicted = [q for q, kind in findings if kind == "contradicted" and q in done.marked]
    return done


def unmarked(answer: str, findings: Iterable[tuple[str, str]]) -> str:
    """ORCH-608: the answer as `settle` leaves it but without its inline marks, for a reply that ends with the one
    fixed line instead (ORCH-606): each contradicted sentence removed, every other sentence as written; "" when the
    removal would leave nothing said (ORCH-547 then keeps its marks: a contradicted sentence never stands unmarked)."""
    contradicted = [(quote, kind) for quote, kind in findings if kind == "contradicted"]
    done = _settled(answer, contradicted)
    return done.answer if has_text(done.answer) else ""


def has_text(text: str) -> bool:
    """Whether a text says anything: it has a letter or a digit (structure, never meaning)."""
    return any(ch.isalnum() for ch in text)


def sentences(text: str) -> int:
    """How many sentences a text holds, by structure only: the pieces between sentence ends (., ! or ? before a space)
    and line breaks that have a letter or a digit."""
    return sum(1 for piece in re.split(r"(?<=[.!?])\s+|\n+", text) if has_text(piece))


def only_quoted_changed(original: str, revised: str, quotes: Iterable[str]) -> bool:
    """Whether `revised` changed only the quoted sentences of `original` (ORCH-547): everything outside them stands
    exactly as it was and in order (only the whitespace touching a quote's place may change), and each quote's place
    holds nothing (removed) or its fix, with no more sentences or lines than the quotes it replaces, so nothing new can
    be inserted beside a fixed sentence. Only the first occurrence of each quote counts, as in `settle`; a quote not in
    `original` frees nothing; adjacent quotes share one place. Structure only: text is compared and counted, never
    judged."""
    spans: list[list[int]] = []                       # [start, end, sentences allowed, lines allowed]
    for start, end in sorted((at, at + len(q)) for q in quotes if q.strip() and (at := original.find(q)) >= 0):
        quote = original[start:end]
        if spans and (start <= spans[-1][1] or not original[spans[-1][1]:start].strip()):
            spans[-1][1] = max(spans[-1][1], end)
            spans[-1][2] += sentences(quote)
            spans[-1][3] += quote.strip().count("\n")
        else:
            spans.append([start, end, sentences(quote), quote.strip().count("\n")])
    text = revised.strip()
    if not spans:
        return text == original.strip()
    pieces = [original[:spans[0][0]]] + [original[a[1]:b[0]] for a, b in zip(spans, spans[1:])] + \
        [original[spans[-1][1]:]]
    head, tail, middle = pieces[0].strip(), pieces[-1].strip(), [piece.strip() for piece in pieces[1:-1]]
    if not text.startswith(head) or not text.endswith(tail) or len(head) + len(tail) > len(text):
        return False
    places: list[str] = []
    at, stop = len(head), len(text) - len(tail)
    for piece in middle:
        found = text.find(piece, at, stop)
        if found < 0:
            return False
        places.append(text[at:found])
        at = found + len(piece)
    places.append(text[at:stop])
    return all(sentences(place) <= allowed and place.strip().count("\n") <= lines
               for place, (_, _, allowed, lines) in zip(places, spans))


def _settled(answer: str, findings: list[tuple[str, str]]) -> Settled:
    done = Settled(answer)
    for quote, kind in findings:
        if not quote.strip() or quote not in done.answer:
            done.missing.append(quote)
        elif kind == "contradicted":
            done.answer = remove_quote(done.answer, quote)
            done.removed.append(quote)
        else:
            marked = mark_unconfirmed(done.answer, quote)
            if marked != done.answer:
                done.marked.append(quote)
            done.answer = marked
    return done


# ORCH-604: a closing markup tag a model left in a text field with no opening tag of its name before it in that field
# (live 2026-11-10 18:12: a worker's reason ended "…appeared.</CLARIFICATION>"). Structure only: names are never
# listed, code spans and fenced blocks are left alone, and a tag with attributes (FERB's own
# `</untrusted-data id="…">`) is never this kind.
_CODE = re.compile(r"```.*?(?:```|\Z)|`[^`\n]*`", re.S)
_TAG = re.compile(r"<(/?)([A-Za-z][A-Za-z0-9_.:-]*)(\s[^<>]*)?\s*(/?)>")


def without_stray_closing_tags(text: str) -> str:
    """`text` with every closing tag `</name>` that no earlier opening `<name …>` in the same text (outside code)
    pairs with removed, and the whitespace that ends the text after it; anything else unchanged."""
    if "</" not in text:
        return text
    code = [m.span() for m in _CODE.finditer(text)]
    opened: dict[str, int] = {}
    cut: list[tuple[int, int]] = []
    for m in _TAG.finditer(text):
        if any(a <= m.start() < b for a, b in code) or m.group(4):
            continue
        name = m.group(2).lower()
        if not m.group(1):
            opened[name] = opened.get(name, 0) + 1
        elif m.group(3):                                  # a closing tag with attributes: not a plain closing tag
            continue
        elif opened.get(name):
            opened[name] -= 1
        else:
            cut.append(m.span())
    if not cut:
        return text
    out, last = [], 0
    for a, b in cut:
        if text[a - 1:a] == " " and text[b:b + 1] == " ":    # one space stays where a tag sat between two words
            b += 1
        out.append(text[last:a])
        last = b
    out.append(text[last:])
    kept = "".join(out)
    return kept.rstrip() if text[cut[-1][1]:].strip() == "" else kept
