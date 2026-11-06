"""Owns the one structural change FERB's code makes to a worker's answer after a check (ORCH-545): removing a sentence
the checker quoted, or marking it inline "(I couldn't confirm this)". Must not judge meaning (rule 4): it only finds the
quoted text in the answer and removes or marks that occurrence; which of the two follows from the checker's own label of
the finding (contradicted: removed; unsupported: marked). Must not write new words into the answer."""

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
    order given; a quote not in the answer (blank, or already gone) changes nothing and is listed as missing."""
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
