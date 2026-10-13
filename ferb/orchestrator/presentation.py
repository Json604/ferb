"""Visible outcome text, separate from the complete inspectable task report (L11).

Only verified model wording can become a success display. An unfinished outcome (failed, needs_user, done but not
confirmed) shows code's status label first, then the short text the model that wrote the answer composed from the
checker's verdict. Code never puts the checker's findings in the short text: they stay in the full answer (Details).
"""
from collections.abc import Sequence


def outcome_display(status: str, answer: str, checked_display: str, label: str, short: str,
                    facts: Sequence[str] = (), question: str = "") -> str:
    """`label`: code's status line (the first line of the answer). `short`: the answer-writing model's short text.
    `facts`: what code shows after the label when no model wrote a short text: the reason work stopped and the files
    recorded; never a checker finding. `question`: code's copy of an open question, always on its own line. A label
    with nothing after it reads as a sentence, never with a dangling colon."""
    if status == 'cancelled':
        return answer
    if status == 'done':
        return checked_display.strip() or answer
    body = short.strip() or '\n'.join(fact.strip() for fact in facts if fact.strip())
    asked = 'Question: ' + question.strip() if question.strip() else ''
    head = label.strip()
    if head.endswith(':') and not (body or asked):
        head = head[:-1] + '.'
    return '\n'.join(part for part in (head, body, asked) if part) or answer
