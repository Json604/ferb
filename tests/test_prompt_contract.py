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
     "When the owner names who does the work (a coding agent, an app, a person), that one does it, not you. Your part "
     "is to start it, give it what it needs (answer its questions, supply data or files, do the browser or Mac steps "
     "it cannot), monitor it and check its result. If it is stuck or slow, help it, or use owner_ask; do none of its "
     "work yourself unless the owner says so (the orchestrator relays the answer), and then say in your report that "
     "you did it."),
    ("worker.md",
     "- summary: the concrete result, useful details and remaining limitations, in FERB's first-person voice. Say who "
     "did each part (FERB itself, or the agent, app or person that did it) and what was used, as the records show."),
    ("supervisor.md",
     "When the owner names who does the work (an agent, app or person), the goal says that one does it and FERB starts "
     "it, helps it and checks its result without doing that work itself."),
    ("supervisor.md",
     'When the owner names who does the work, one condition is that they did it, not FERB or a substitute. Each '
     'quality or limit the owner states for the result ("score well", "no overfitting", "no hardcoding") is its own '
     'condition, in their words.'),
    ("supervisor.md",
     "monitor its result, inspect artifacts and report when it finishes; if it is stuck, FERB helps it or asks the "
     "owner before doing its work."),
    ("interaction.md",
     "A lasting choice the owner did not ask for and the request does not need (for example which submission counts as "
     "final, or changing a setting or default) is also an outcome to confirm, never a side step."),
    ("interaction.md",
     'Everything the owner reads (answers, summaries, questions, success conditions, stop reasons, and a check\'s '
     'evidence and how) speaks to them as "you" and "your" ("your Brave Work profile"), never "the owner" or "the '
     'user". In a success condition, "you" means the owner.'),
    ("dispatch.md",
     "When the owner named who does the work (an agent, app or person), never instruct a worker to do that work "
     "itself: have it help the named one, or use action=ask to ask the owner whether FERB should take over, saying "
     "what is stuck."),
    ("question.md",
     "A worker asking to do work the owner gave to a named agent, app or person is the owner's decision: "
     "ask_owner=true, saying what is stuck and what FERB would do instead."),
    ("verifier.md",
     "A condition that the agent, app or person the owner named did the work is met only by evidence that they "
     "produced it; work FERB did itself is a substitute: give `met=false` and say in the evidence that FERB did it."),
]


@pytest.mark.parametrize("name, sentence", SENTENCES, ids=[f"{n}:{s[:40]}" for n, s in SENTENCES])
def test_the_prompt_carries_the_instruction(name: str, sentence: str) -> None:
    assert " ".join(sentence.split()) in _words(name)


def test_the_old_monitoring_line_still_says_launching_is_not_completion() -> None:
    # The new paragraph sits after it; the existing instruction is kept, not replaced.
    assert "launching it is not completion." in _words("worker.md")
