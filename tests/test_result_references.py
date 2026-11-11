"""What the planner is shown of FERB's earlier results, and how a task uses one (ORCH-616..ORCH-620). Live 2026-11-11
03:03 (request 7c3a2abbd6c6470f9d01e5f495481d20): "put that laptop res in a google sheet and give me the lin" right after
a two-part card; the planner saw the laptop row only as "### Top 3 coding laptops under ₹80,000" and spent three
task.evidence lookups (the first with a kind no record has) and 25.5 s copying the comparison into the goal."""
import json
from typing import Any

from ferb.orchestrator.plans import Plan, Step
from tests.conftest import RunningDaemon, chat, cond, done, install, outcome, sup, task, tool
from tests.support.schedules import run

LAPTOPS = ("### Top 3 coding laptops under ₹80,000\n\n"
           "1. **Acer Swift Neo SFN14-55 — ₹79,999** — **Best overall**\n"
           "   - Core Ultra 5 125H, 16GB LPDDR5, 512GB SSD\n\n"
           "2. **Acer Aspire 14 AS14H-53 — ₹69,999** — **Best value**\n\n"
           "**My pick:** Swift Neo for the best complete coding machine.")
TABLE = "No table was booked. You changed your mind before any reservation was submitted."


def _part(pid: str, quote: str, label: str, step: str, index: int) -> dict[str, Any]:
    return {"id": pid, "quote": quote, "label": label, "direct": None, "answer_step": step, "conditions": [index]}


def two_parts(d: RunningDaemon) -> str:
    """The live case's first turn: a table booking and a laptop comparison, one card; returns its task id."""
    spec = task("Book Toit and compare laptops", conds=(cond("The booking is answered"), cond("3 laptops compared")))
    spec["parts"] = [_part("table", "book a table for 2 at toit", "Toit table booking", "book", 0),
                     _part("laptops", "top 3 laptops under 80k for coding", "Coding laptop comparison", "compare", 1)]
    spec["plan"] = Plan(steps=[Step(id="book", kind="do", goal="Book the table"),
                               Step(id="compare", kind="do", goal="Compare laptops")]).model_dump()
    install(d, [sup("task", "On it.", spec)], {"step.book": done(TABLE), "step.compare": done(LAPTOPS)})
    final = outcome(chat(d, "book a table for 2 at toit and top 3 laptops under 80k for coding", session="owner"))
    return str(final["task_id"])


def planner_context(script: Any, n: int = 0) -> str:
    return [c for _, k, c, _ in script.contexts if k == "supervisor"][n]


# decision: ORCH-616
def test_a_rows_heading_is_not_its_short_form(daemon: RunningDaemon) -> None:
    two_parts(daemon)
    script = install(daemon, [sup("reply", "Sure.")])
    chat(daemon, "thanks", session="owner")
    history = planner_context(script)
    row = next(line for line in history.split("\n") if "part “Coding laptop comparison”" in line)
    assert "Acer Swift Neo" in row and "###" not in row


# decision: ORCH-617
def test_a_kind_with_no_record_answers_with_the_kinds_the_task_has(daemon: RunningDaemon) -> None:
    tree = two_parts(daemon)
    seen: dict[str, Any] = {}

    def look(token: str) -> str:
        seen.update(tool(daemon.sock, token, "task.evidence", task_id=tree, kind="reports")["content"])
        return sup("reply", "Looked.")
    install(daemon, [look])
    chat(daemon, "what did the laptop step find?", session="owner")
    assert seen["records"] == [] and seen["kinds_here"].get("worker", 0) >= 2
    assert "reports" not in seen["kinds_here"]
