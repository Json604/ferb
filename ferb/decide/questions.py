"""The typed questions FERB asks Jev (section 12). Each names its possible answers, the answer FERB's own logic gives
today (the baseline), and which of Jev's answers may have an effect once the owner enables it — only ever stricter
than the baseline (extra redaction or dropping a memory proposal). Task decisions belong to the orchestrator.
"""

from dataclasses import dataclass
from typing import Any, Literal


@dataclass(frozen=True)
class Question:
    id: str
    type: Literal["noul", "choice"]
    instructions: str
    criteria: dict[str, str]                   # noul: {"true", "false"}; choice: option -> description
    stricter: frozenset[str] = frozenset()     # Jev answers that may act (compared with the baseline, below)
    shadow_sample: float = 1.0                 # fraction asked while in shadow mode (Jev is billed per call)
    threshold: float = 0.5                     # noul: probability for "yes"
    effect: str = ""                           # what a stricter answer does, for the owner

    def api(self) -> dict[str, Any]:
        q: dict[str, Any] = {"type": self.type, "instructions": self.instructions}
        if self.criteria:
            q["criteria"] = self.criteria
        return q

    def value(self, answer: dict[str, Any]) -> tuple[str, float | None]:
        """Jev's typed answer -> (FERB's value, confidence)."""
        if self.type == "noul":
            p = float(answer["noul"])
            return ("yes" if p >= self.threshold else "no"), p
        choice = str(answer["choice"])
        if choice not in self.criteria:
            raise ValueError(f"{self.id}: unknown choice {choice!r}")
        conf = answer.get("confidence")
        return choice, float(conf) if conf is not None else None

    def is_stricter(self, jev: str, baseline: str) -> bool:
        return jev in self.stricter and jev != baseline


QUESTIONS: dict[str, Question] = {q.id: q for q in (
    Question("memory.worth_keeping", "noul",
             "Is this worth remembering about the owner long term (durable, useful, not sensitive)?",
             {"true": "worth keeping", "false": "not worth keeping"},
             stricter=frozenset({"no"}), effect="no -> the proposal is dropped"),
    Question("trail.relevance", "choice",
             "How relevant is this activity event to understanding the owner's day?",
             {"ignore": "noise", "note": "worth noting", "summarize": "important for the day's summary"},
             shadow_sample=0.1, effect="stored on the row only; no task decisions"),
    Question("trail.sensitive", "noul",
             "Does this activity event reveal something sensitive (health, finances, intimate life, credentials, "
             "other people's private information)?",
             {"true": "sensitive", "false": "not sensitive"},
             stricter=frozenset({"yes"}), shadow_sample=0.1,
             effect="yes -> the row's titles and URLs are redacted"),
)}


def base_id(key: str) -> str:
    """Batched questions carry a suffix per item ("memory.worth_keeping#3")."""
    return key.split("#", 1)[0]
