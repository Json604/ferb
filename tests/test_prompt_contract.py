"""Runtime prompts are executable input (CLAUDE.md). These pin the instructions the F4 live run showed missing (task
79bc5bfa, 2026-10-09): the agent the owner names does the work, conditions cover what the owner actually asked, reports
say who did each part, a lasting choice the owner didn't ask for is asked about, and FERB speaks to the owner as "you"
(V6, D34, P27). Whether a model follows them is a live check; these keep the words from silently disappearing."""
from pathlib import Path

import pytest

PROMPTS = Path(__file__).resolve().parents[1] / "ferb" / "orchestrator" / "prompts"


def _words(name: str) -> str:
    return " ".join((PROMPTS / name).read_text().split())


SENTENCES = [
        ("worker.md",
     'When the owner hands the work to a particular agent or person that does it on its own (a coding agent like Codex Astra, a colleague, a service), that one does it, not you. An app you drive as a tool (WhatsApp, Spotify, Gmail, a browser) is not that: you do the work through it. Your part is to start the agent, give it what it needs (answer its questions, supply data or files, do the browser or Mac steps it cannot), monitor it and check its result. If it is stuck, help it, or use owner_ask; do none of its work yourself unless the owner says so (the orchestrator relays the answer), and then say in your report that you did it.'),
    ("worker.md",
     "- summary: the concrete result, useful details and remaining limitations, in FERB's first-person voice. Say who "
     "did each part (FERB itself, or the agent, app or person that did it) and what was used, as the records show."),
        ("supervisor.md",
     'When the owner hands the work to a named agent or person that does it on its own (not an app FERB uses as a tool), the goal says that one does it and FERB starts it, helps it and checks its result without doing that work itself.'),
        ("supervisor.md",
     'When the owner hands the work to a named agent or person (not an app FERB uses as a tool), one condition is that they did it, not FERB or a substitute.'),
    ("supervisor.md",
     "monitor its result, inspect artifacts and report when it finishes; if it is stuck, FERB helps it or asks the "
     "owner before doing its work."),
    ("interaction.md",
     "A lasting choice the owner did not ask for and the request does not need (for example which submission counts as "
     "final, or changing a setting or default) is the owner's decision, never a side step: bring it to the orchestrator "
     "as a question instead of making it."),
    ("interaction.md",
     'Everything the owner reads (answers, summaries, questions, success conditions, stop reasons, and a check\'s '
     'evidence and how) speaks to them as "you" and "your", never "the owner" or "the user". In a success condition, '
     '"you" means the owner.'),
        ("dispatch.md",
     'When the owner handed the work to a named agent or person (not an app FERB uses as a tool), never instruct a worker to do that work itself: have it help the named one, or use action=ask to ask the owner whether FERB should take over, saying what is stuck.'),
        ("question.md",
     "A worker asking to do work the owner handed to a named agent or person (not an app FERB uses as a tool) is the owner's decision about who does the work, not a method question: ask_owner=true, saying what is stuck and what FERB would do instead."),
        ("verifier.md",
     'A condition that the agent or person the owner handed the work to did it is met only by evidence that they produced it; work FERB did itself is a substitute: give `met=false` and say in the evidence that FERB did it. An app FERB used as a tool is not such an agent.'),
]


@pytest.mark.parametrize("name, sentence", SENTENCES, ids=[f"{n}:{s[:40]}" for n, s in SENTENCES])
def test_the_prompt_carries_the_instruction(name: str, sentence: str) -> None:
    assert " ".join(sentence.split()) in _words(name)


def test_the_old_monitoring_line_still_says_launching_is_not_completion() -> None:
    # The new paragraph sits after it; the existing instruction is kept, not replaced.
    assert "launching it is not completion." in _words("worker.md")


@pytest.mark.parametrize("name, sentence", [
    ("worker.md", "An aggregator's InStock metadata is a discovery lead, not proof that an official seller currently has tickets available."),
    ("verifier.md", "A caveat at the end does not repair unsupported availability or price claims in individual rows."),
    ("supervisor.md", "For exhaustive comparisons, require substantive coverage of every requested source, not just an inventory or keyword-ranked excerpts."),
    ("worker.md", "Do not invent a numerical ranking score without a reproducible rubric and supporting observations."),
    ("verifier.md", "Inventory counts, opening/closing excerpts and a shortlist do not establish that all source content was analyzed."),
    ("worker.md", "For unclassified split boxes, pass their exact fresh snapshot IDs in split_elements, in code order, with element set to the first ID."),
])
def test_owner_report_audit_contract(name, sentence):
    # Real owner traces: session review 726ab98b and ticket list 6eb4bffd.
    # These guard executable instructions; live model adherence remains separately audited.
    assert sentence in _words(name)
