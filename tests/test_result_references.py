"""What the planner is shown of FERB's earlier results, and how a task uses one (ORCH-616..ORCH-620). Live 2026-11-11
03:03 (request 7c3a2abbd6c6470f9d01e5f495481d20): "put that laptop res in a google sheet and give me the lin" right after
a two-part card; the planner saw the laptop row only as "### Top 3 coding laptops under ₹80,000" and spent three
task.evidence lookups (the first with a kind no record has) and 25.5 s copying the comparison into the goal."""
import json
from typing import Any

from ferb.orchestrator.plans import Plan, Step
from ferb.sanitize.untrusted import as_data
from tests.conftest import RunningDaemon, chat, cond, done, install, outcome, sup, task, tool
from tests.support.schedules import run

LAPTOPS = ("### Top 3 coding laptops under ₹80,000\n\n"
           "1. **Acer Swift Neo SFN14-55 — ₹79,999** — **Best overall**\n"
           "   - Core Ultra 5 125H, 16GB LPDDR5, 512GB SSD\n\n"
           "2. **Acer Aspire 14 AS14H-53 — ₹69,999** — **Best value**\n\n"
           "**My pick:** Swift Neo for the best complete coding machine.")
SHOWN = as_data(LAPTOPS)                     # the comparison as a result block holds it: one quoted string (MP2)
TABLE = "No table was booked. You changed your mind before any reservation was submitted."


def _part(pid: str, quote: str, label: str, step: str, index: int) -> dict[str, Any]:
    return {"id": pid, "quote": quote, "label": label, "direct": None, "answer_step": step, "conditions": [index]}


def two_parts(d: RunningDaemon) -> str:
    """The live case's first turn: a table booking and a laptop comparison, one card; returns its task id."""
    spec = task("Book Toit and compare laptops", conds=(cond("The booking is answered"), cond("3 laptops compared")))
    spec["parts"] = [_part("table", "book a table for 2 at toit", "Toit table booking", "book", 0),
                     _part("laptops", "top 3 laptops under 80k for coding", "Coding laptop comparison", "compare", 1)]
    spec["plan"] = Plan(steps=[Step(id="book", kind="do", goal="Book the table"),
                               # one after the other, so the laptop row is always the newest
                               Step(id="compare", kind="do", goal="Compare laptops", depends_on=["book"])]).model_dump()
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


def rows(d: RunningDaemon, tree: str) -> dict[str, str]:
    """The card's chat.part row ids by part."""
    return run(d, lambda: {str(e.data["part"]): e.id for e in d.daemon.journal.of_kind("chat.part", "task_id", tree)})


def answer_id(d: RunningDaemon, tree: str) -> str:
    return str(run(d, lambda: d.daemon.journal.of_kind("task.answer", "task_id", tree)[-1].id))


def sheet(ids: list[str], goal: str = "Put the laptop comparison in a Google Sheet and give the link") -> dict[str, Any]:
    spec = task(goal, conds=(cond("A sheet holds the comparison and its link is given"),))
    spec["earlier_results"] = ids
    return spec


def given(script: Any, key: str) -> str:
    return str(next(c for _, k, c, _ in reversed(script.contexts) if k == key))


# decision: ORCH-619
def test_every_result_line_carries_its_id(daemon: RunningDaemon) -> None:
    tree = two_parts(daemon)
    script = install(daemon, [sup("reply", "Sure.")])
    chat(daemon, "thanks", session="owner")
    history = planner_context(script)
    for rid in [*rows(daemon, tree).values(), answer_id(daemon, tree)]:
        assert f"[Result id: {rid}; task.evidence task_id={tree}]" in history


# decision: ORCH-619
def test_a_task_naming_a_row_gives_its_worker_and_checker_the_rows_full_text(daemon: RunningDaemon) -> None:
    tree = two_parts(daemon)
    laptops = rows(daemon, tree)["laptops"]
    script = install(daemon, [sup("task", "Putting it in a sheet.", sheet([laptops]))],
                     {"worker": done("https://docs.google.com/spreadsheets/d/abc")})
    final = outcome(chat(daemon, "put that laptop res in a google sheet and give me the lin", session="owner"))
    assert final["status"] == "done"
    assert len(script.calls("planner")) == 1                       # no lookup, no repair: one planner call
    for key in ("worker", "worker.verify"):
        ctx = given(script, key)
        block = ctx[ctx.index("Earlier results this task uses"):]
        assert f"Result {laptops} (" in block and "part “Coding laptop comparison”" in block
        assert SHOWN in block and TABLE not in block
    exposures = run(daemon, lambda: [json.loads(r[0]) for r in daemon.daemon.journal.db.conn.execute(
        "SELECT input_json FROM spans WHERE task_id = ? AND kind = 'context.exposure'", (final["task_id"],))])
    assert any(f"entry:{laptops}" in e.get("source_refs", []) for e in exposures)


# decision: ORCH-619
def test_a_cards_id_gives_every_row_in_full_in_the_owners_order(daemon: RunningDaemon) -> None:
    tree = two_parts(daemon)
    script = install(daemon, [sup("task", "Putting both in a sheet.", sheet([answer_id(daemon, tree)]))],
                     {"worker": done("https://docs.google.com/spreadsheets/d/abc")})
    chat(daemon, "put both of those in a sheet", session="owner")
    ctx = given(script, "worker")
    assert ctx.index(TABLE) < ctx.index(SHOWN)


# decision: ORCH-619
def test_an_older_result_still_reaches_the_worker_in_full(daemon: RunningDaemon) -> None:
    tree = two_parts(daemon)
    laptops = rows(daemon, tree)["laptops"]
    install(daemon, [sup("task", "Checking.", task("Check the weather"))], {"worker": done("Sunny, 31°C.")})
    chat(daemon, "what's the weather", session="owner")
    script = install(daemon, [sup("task", "Putting it in a sheet.", sheet([laptops]))],
                     {"worker": done("https://docs.google.com/spreadsheets/d/abc")})
    chat(daemon, "put the laptop comparison from before in a sheet", session="owner")
    assert SHOWN in given(script, "worker")


# decision: ORCH-619
def test_a_wrong_id_goes_back_once_with_a_plain_problem(daemon: RunningDaemon) -> None:
    tree = two_parts(daemon)
    laptops = rows(daemon, tree)["laptops"]
    script = install(daemon, [sup("task", "Putting it in a sheet.", sheet(["c12"])),
                              sup("task", "Putting it in a sheet.", sheet([laptops]))],
                     {"worker": done("https://docs.google.com/spreadsheets/d/abc")})
    final = outcome(chat(daemon, "put that laptop res in a google sheet", session="owner"))
    repair = planner_context(script, 1)
    assert 'earlier_results \\"c12\\" isn\'t a result shown in this chat' in repair
    assert final["status"] == "done" and SHOWN in given(script, "worker")


# decision: ORCH-619
def test_a_result_of_another_chat_is_not_reachable(daemon: RunningDaemon) -> None:
    tree = two_parts(daemon)
    laptops = rows(daemon, tree)["laptops"]
    script = install(daemon, [sup("task", "Putting it in a sheet.", sheet([laptops])),
                              sup("reply", "Which comparison do you mean?")])
    chat(daemon, "put that laptop res in a google sheet", session="other")
    assert "isn't a result shown in this chat" in planner_context(script, 1)
    assert not script.calls("worker")


# decision: ORCH-619
def test_results_that_dont_fit_the_workers_context_go_back_for_repair(daemon: RunningDaemon,
                                                                       monkeypatch: Any) -> None:
    tree = two_parts(daemon)
    laptops = rows(daemon, tree)["laptops"]
    script = install(daemon, [sup("task", "Putting it in a sheet.", sheet([laptops])),
                              sup("reply", "That comparison is too long for one worker.")])
    monkeypatch.setattr(type(daemon.daemon.supervisor), "_worker_room", lambda self, turn, identity, kind: 100)
    chat(daemon, "put that laptop res in a google sheet", session="owner")
    assert "the earlier results it names need about" in planner_context(script, 1)
    assert not script.calls("worker")


def latest_block(context: str) -> str:
    """The planner's latest-result block ("" when there is none)."""
    if "Layer 2 — the latest result shown in this chat" not in context:
        return ""
    start = context.index("Layer 2 — the latest result shown in this chat")
    return context[start:context.index("Layer 5 — owner's", start)]


# decision: ORCH-620
def test_the_planner_answers_from_the_latest_card_in_full_newest_row_first(daemon: RunningDaemon) -> None:
    tree = two_parts(daemon)
    script = install(daemon, [sup("reply", "The Swift Neo, at 1.25 kg.")])
    chat(daemon, "which of those is lightest?", session="owner")
    block = latest_block(planner_context(script))
    assert f"Result {answer_id(daemon, tree)} names the whole card" in block
    assert SHOWN in block and TABLE in block and block.index(SHOWN) < block.index(TABLE)
    assert len(script.calls("planner")) == 1


# decision: ORCH-620
def test_the_block_rides_along_until_a_newer_result_replaces_it(daemon: RunningDaemon) -> None:
    two_parts(daemon)
    script = install(daemon, [sup("reply", "You're welcome."), sup("task", "Checking.", task("Check the weather")),
                              sup("reply", "Sunny.")], {"worker": done("Sunny, 31°C in Bangalore.")})
    chat(daemon, "thanks", session="owner")
    assert SHOWN in latest_block(planner_context(script, 0))
    chat(daemon, "what's the weather", session="owner")
    chat(daemon, "and tomorrow?", session="owner")
    block = latest_block(planner_context(script, 2))
    assert "Sunny, 31°C in Bangalore." in block and SHOWN not in block


# decision: ORCH-620
def test_a_result_over_the_cap_is_left_out_whole_and_its_id_still_works(daemon: RunningDaemon,
                                                                         monkeypatch: Any) -> None:
    import ferb.orchestrator.supervisor as supervisor
    tree = two_parts(daemon)
    laptops = rows(daemon, tree)["laptops"]
    monkeypatch.setattr(supervisor, "LATEST_RESULT_TOKENS", 40)            # holds neither row whole
    script = install(daemon, [sup("task", "Putting it in a sheet.", sheet([laptops]))],
                     {"worker": done("https://docs.google.com/spreadsheets/d/abc")})
    chat(daemon, "put that laptop res in a google sheet", session="owner")
    context = planner_context(script)
    assert latest_block(context) == "" and f"[Result id: {laptops};" in context
    assert SHOWN in given(script, "worker")
    monkeypatch.setattr(supervisor, "LATEST_RESULT_TOKENS", 200)     # a card: the newest row is too long, so none
    two_parts(daemon)
    script = install(daemon, [sup("reply", "Sure.")])
    chat(daemon, "thanks", session="owner")
    assert latest_block(planner_context(script)) == ""


# decision: ORCH-620
def test_a_card_over_the_cap_keeps_its_newest_whole_rows(daemon: RunningDaemon, monkeypatch: Any) -> None:
    import ferb.orchestrator.supervisor as supervisor
    from ferb.core.context import count_tokens
    tree = two_parts(daemon)
    laptops = rows(daemon, tree)["laptops"]
    row = run(daemon, lambda: daemon.daemon.journal.by_ids([laptops])[0])
    head = supervisor.LATEST_CARD.format(card=f"Result {answer_id(daemon, tree)} names the whole card, ")
    newest = supervisor.result_line(row) + ":\n" + SHOWN
    monkeypatch.setattr(supervisor, "LATEST_RESULT_TOKENS", count_tokens(head) + count_tokens(newest))
    script = install(daemon, [sup("reply", "Sure.")])
    chat(daemon, "thanks", session="owner")
    block = latest_block(planner_context(script))
    assert SHOWN in block and TABLE not in block and "1 older row of this card didn't fit here" in block


# decision: ORCH-620
def test_the_block_adds_no_origin_its_conversation_line_doesnt_already_carry(daemon: RunningDaemon,
                                                                             monkeypatch: Any) -> None:
    import ferb.orchestrator.supervisor as supervisor
    two_parts(daemon)
    install(daemon, [sup("reply", "Sure."), sup("reply", "Sure.")])
    chat(daemon, "thanks", session="owner")
    monkeypatch.setattr(supervisor, "LATEST_RESULT_TOKENS", 0)
    chat(daemon, "thanks again", session="owner")
    with_block, without = run(daemon, lambda: [e.origin for e in daemon.daemon.journal.of_kind("chat.reply")][-2:])
    assert with_block == without
