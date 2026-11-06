"""Owns the one structural change FERB's code makes to a worker's answer after a check (ORCH-545): removing a sentence
the checker quoted, or marking it inline "(I couldn't confirm this)". Must not judge meaning (rule 4): it only finds the
quoted text in the answer and removes or marks that occurrence; which of the two follows from the checker's own label of
the finding (contradicted: removed; unsupported: marked). It also tells whether a fix turn's revised answer changed
anything outside the quoted sentences (`only_quoted_changed`). Must not write new words into the answer."""

from collections.abc import Iterable
from dataclasses import dataclass, field

UNCONFIRMED_MARK = "(I couldn't confirm this)"            # the one inline marker FERB's code may add


@dataclass
class Settled:
    """An answer after code removed or marked the quoted sentences: what was removed, marked, or not found."""
    answer: str
    removed: list[str] = field(default_factory=list)
    marked: list[str] = field(default_factory=list)
    missing: list[str] = field(default_factory=list)


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
    never leaves the answer empty (ORCH-547): when removing the contradicted sentences would leave no text, they are
    marked instead, so the owner is never told "done" with nothing said."""
    findings = list(findings)
    done = _settled(answer, findings)
    if done.removed and not done.answer.strip():
        done = _settled(answer, [(quote, "unsupported") for quote, _ in findings])
    return done


def only_quoted_changed(original: str, revised: str, quotes: Iterable[str]) -> bool:
    """Whether `revised` keeps everything of `original` outside the quoted sentences unchanged and in order (ORCH-547):
    the text before the first quote starts it, the text after the last ends it, and each piece between two quotes
    stands between them; a quote's own place may hold anything (its fix) or nothing (removed). Only the first
    occurrence of each quote counts, as in `settle`; a quote not in `original` frees nothing. Structure only: text is
    compared, never judged."""
    spans = sorted((at, at + len(quote)) for quote in quotes if quote.strip() and (at := original.find(quote)) >= 0)
    kept: list[str] = []
    pos = 0
    for start, end in spans:
        kept.append(original[pos:start] if start > pos else "")
        pos = max(pos, end)
    kept.append(original[pos:])
    pieces = [piece.strip() for piece in kept]
    text = revised.strip()
    if len(pieces) == 1:
        return text == pieces[0]
    head, tail = pieces[0], pieces[-1]
    if not text.startswith(head):
        return False
    at = len(head)
    for piece in pieces[1:-1]:
        if piece:
            found = text.find(piece, at)
            if found < 0:
                return False
            at = found + len(piece)
    return text.endswith(tail) and len(text) - len(tail) >= at


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
