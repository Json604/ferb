"""Step 8d end-to-end runs (ORCH-319 and 20, the owner-approved mock demo/answers-in-steps): the real
daemon, gateway and scripted model. One ask is unchanged; several asks post each part once, in its own row, when its
answer step is checked, in any order; a direct part is answered in the first reply; a part's question pauses only its
row; a failed part says so and the count is honest; a later step updates a posted row; a restart posts nothing twice;
no summarizing call after the last part."""
import json
import threading
import time
from typing import Any

from ferb.cli.client import Client
from ferb.orchestrator.plans import Plan, Step
from ferb.orchestrator.attempts import CHECK_IN_AFTER
from tests.conftest import (STALL, RunningDaemon, answer_question, chat, done, install, outcome, report, sup, task,
                            cond, tool, turn_events, verdict)
from tests.support.schedules import own_daemon, run


def _part(pid: str, quote: str, label: str, *, direct: str | None = None, step: str | None = None,
          conds: tuple[int, ...] = ()) -> dict[str, Any]:
    return {"id": pid, "quote": quote, "label": label, "direct": direct, "answer_step": step,
            "conditions": list(conds)}


def _spec(goal: str, conds: list[str], parts: list[dict[str, Any]], steps: list[Step] | None = None) -> dict[str, Any]:
    spec = task(goal, conds=tuple(cond(c) for c in conds))
    spec["parts"] = parts
    if steps is not None:
        spec["plan"] = Plan(steps=steps).model_dump()
    return spec


def _not_done(reason: str, question: str | None = None) -> str:
    return json.dumps(report("not_done", reason=reason, question=question))


def _slow(answer: str, seconds: float) -> Any:
    def act(_token: str) -> str:
        time.sleep(seconds)
        return done(answer)
    return act


class Turn:
    """One chat turn streamed in the background, so the test can act while the task runs."""

    def __init__(self, d: RunningDaemon, text: str) -> None:
        self.events: list[dict[str, Any]] = []
        self.error: BaseException | None = None
        self.thread = threading.Thread(target=self._run, args=(d, text), daemon=True)
        self.thread.start()

    def _run(self, d: RunningDaemon, text: str) -> None:
        try:
            self.events = chat(d, text, timeout=60)
        except BaseException as e:
            self.error = e

    def done(self, timeout: float = 30) -> list[dict[str, Any]]:
        self.thread.join(timeout)
        assert not self.thread.is_alive(), "the turn didn't end"
        if self.error is not None:
            raise self.error
        return self.events


def _until(check: Any, timeout: float = 20) -> None:
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if check():
            return
        time.sleep(0.02)
    raise AssertionError("the condition never held")


def _parts(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [e for e in events if e["type"] == "part"]


def _records(d: RunningDaemon, tree: str) -> list[dict[str, Any]]:
    return run(d, lambda: [e.data for e in d.daemon.journal.of_kind("chat.part", "task_id", tree)])


PERMIT = "any news about the work permit and also when is trump's term ending"


def _permit_and_trump() -> dict[str, Any]:
    return _spec("Permit news and Trump's term", ["Permit news is reported", "The end of Trump's term is given"],
                 [_part("p1", "any news about the work permit", "work permit news", step="permit", conds=(0,)),
                  _part("p2", "when is trump's term ending", "When Trump's term ends", step="term", conds=(1,))],
                 [Step(id="permit", kind="do", goal="Check work permit news"),
                  Step(id="term", kind="do", goal="Find when Trump's term ends")])


# decision: ORCH-319
def test_one_ask_is_unchanged_plan_line_one_answer_no_card(daemon: RunningDaemon) -> None:
    """Scenario 1: a single ask gets the plan line, then one answer: no card, no rows, no count, no extra summary."""
    script = install(daemon, [sup("task", "I'll put together a short list.", task("List easy piano pieces"))],
                     {"worker": done("Satie, Gymnopédie No. 1.")})
    events = chat(daemon, "what are some easy piano pieces that sound heavenly?")
    plan = next(e for e in events if e["type"] == "plan")
    final = outcome(events)
    assert "card" not in plan and not _parts(events)
    assert "card" not in final and " of 1 done" not in final["answer"]
    assert final["answer"].startswith("Satie, Gymnopédie No. 1.") and final["status"] == "done"
    assert script.calls("worker") == ["worker"]


# decision: ORCH-319
def test_a_one_ask_plan_answers_with_its_last_step_and_no_summarizing_call(daemon: RunningDaemon) -> None:
    """ORCH-319: no code-added final worker; a one-ask plan's last step is checked against the task's success
    conditions and its checked answer is the answer."""
    spec = task("Find the cheapest flight", conds=(cond("The cheapest flight is named"),))
    spec["plan"] = Plan(steps=[Step(id="search", kind="do", goal="Search the flights"),
                               Step(id="pick", kind="do", goal="Pick the cheapest", depends_on=["search"])]).model_dump()
    script = install(daemon, [sup("task", "I'll compare the flights.", spec)],
                     {"step.search": done("Found 3 flights."), "step.pick": done("IndiGo 6E 2134 at ₹4,210.")})
    final = outcome(chat(daemon, "find the cheapest flight to Delhi"))
    assert script.calls("worker") == ["step.search", "step.pick"]          # no "worker": no summarizing call
    checked = next(c for _, k, c, _ in script.contexts if k == "step.pick.verify")
    assert "The cheapest flight is named" in checked                        # the task's condition, on the last step
    assert final["status"] == "done" and final["answer"].startswith("IndiGo 6E 2134 at ₹4,210.")
    assert "card" not in final


# decision: ORCH-319
def test_two_parts_fill_in_any_order_each_once_and_code_adds_the_count(daemon: RunningDaemon) -> None:
    """Scenario 2: one reply opens a card with a row per ask in the owner's order; the Trump row fills first, the permit
    row later, each once; no model summary afterwards, only "2 of 2 done" and the sites."""
    script = install(daemon, [sup("task", "I'll check the permit news and Trump's term.", _permit_and_trump())],
                     {"step.permit": _slow("No new permit rule changes this week.", 0.6),
                      "step.term": done("His term ends at noon ET on 20 January 2029.")})
    events = chat(daemon, PERMIT)
    plan = next(e for e in events if e["type"] == "plan")
    assert [r["part"] for r in plan["card"]["rows"]] == ["p1", "p2"]
    assert {r["state"] for r in plan["card"]["rows"]} == {"working"}
    assert plan["plan_line"].startswith("I'll check the permit news")
    posted = [(e["part"], e["state"]) for e in _parts(events)]
    assert posted == [("p2", "done"), ("p1", "done")]                      # out of order, once each
    final = outcome(events)
    assert final["card"]["done"] == 2 and final["card"]["total"] == 2 and final["card"]["line"] == "2 of 2 done"
    assert final["status"] == "done" and final["verified"] is True
    assert final["answer"].split("\n")[:3] == ["work permit news: No new permit rule changes this week.",
                                                "When Trump's term ends: His term ends at noon ET on 20 January 2029.",
                                                "2 of 2 done"]                # the index (ORCH-534)
    assert sorted(script.calls("worker")) == ["step.permit", "step.term"]     # no summarizing call
    tree = plan["task_id"]
    assert [(r["part"], r["state"]) for r in _records(daemon, tree)] == [("p2", "done"), ("p1", "done")]


# decision: ORCH-535
def test_a_multi_part_answer_is_saved_as_an_index_the_planner_reads_whole(daemon: RunningDaemon) -> None:
    """Freelance task 0218bb01: the saved answer re-listed every row in full (10,909 chars, 58 hosts) and the planner's
    short form kept only its first paragraph. Now the saved answer is one paragraph: each part's outcome line, where
    the results are, the count and a site count; the full rows stay in chat.part; the planner sees every line."""
    path = daemon.daemon.home / "workspace" / "permits.md"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("notes")
    permit = "No new permit rule changes this week.\nThe ministry's last notice was on 2 November.\nNothing pending."
    script = install(daemon, [sup("task", "I'll check both.", _permit_and_trump()), sup("reply", "Noted.")],
            {"step.permit": json.dumps(report("done", permit, artifacts=(str(path),))),
             "step.term": done("His term ends at noon ET on 20 January 2029.")})
    final = outcome(chat(daemon, PERMIT))
    assert final["answer"] == "\n".join(["work permit news: No new permit rule changes this week.",
                                         "When Trump's term ends: His term ends at noon ET on 20 January 2029.",
                                         f"Results: {path}", "2 of 2 done"])
    rows = {r["part"]: r["text"] for r in _records(daemon, final["task_id"])}
    assert rows["p1"] == permit                                                # the full row stays in the card
    chat(daemon, "thanks")
    history = [c for _, k, c, _ in script.contexts if k == "supervisor"][-1]
    assert "No new permit rule changes this week." in history and "20 January 2029" in history
    assert "Nothing pending." not in history                                    # rows past their first line aren't


# decision: ORCH-319, ORCH-543
def test_a_direct_part_is_answered_in_the_first_reply_and_counts_as_done(daemon: RunningDaemon) -> None:
    """Scenario 3: "who are you" is answered in the first reply, above the plan line; only the research gets a row;
    the count includes the direct part."""
    spec = _spec("Research keyboards", ["Three keyboards under ₹10k are named"],
                 [_part("who", "who are you", "Who I am", direct="I'm FERB. I run on your Mac."),
                  _part("kb", "research the 3 best mechanical keyboards under ₹10k", "3 best keyboards under ₹10k",
                        step="worker", conds=(0,))])
    install(daemon, [sup("task", "I'll research keyboards under ₹10,000.", spec)],
            {"worker": done("Keychron K2, Royal Kludge RK84, Redragon K552.")})
    events = chat(daemon, "who are you, research the 3 best mechanical keyboards under ₹10k")
    plan = next(e for e in events if e["type"] == "plan")
    assert plan["text"].index("I'm FERB.") < plan["text"].index("I'll research keyboards")
    assert plan["direct"] == [{"part": "who", "label": "Who I am", "text": "I'm FERB. I run on your Mac."}]
    rows = plan["card"]["rows"]
    assert [(r["part"], r["state"], r["direct"]) for r in rows] == [("who", "done", True), ("kb", "working", False)]
    assert [(e["part"], e["state"]) for e in _parts(events)] == [("kb", "done")]   # the direct part isn't re-sent
    final = outcome(events)
    assert (final["card"]["done"], final["card"]["total"]) == (2, 2) and final["status"] == "done"
    # ORCH-543 (owner 2026-11-06: "if theres 2 boxes why is it saying /3 ???"): every part the count includes has a
    # row, the direct one too, in every picture of the card
    for card in (plan["card"], final["card"]):
        assert card["total"] == len(card["rows"])
    assert all(e["total"] == len(rows) for e in _parts(events))


# decision: ORCH-319
def test_a_part_asks_in_its_row_while_another_part_fills(daemon: RunningDaemon) -> None:
    """Scenario 4: the booking row asks which time (its question tied to the part); the other row fills meanwhile;
    the answer goes back to that row only, which then fills."""
    spec = _spec("Book Toit and order lunch", ["A table at Toit is booked", "Paneer tikka is ordered"],
                 [_part("book", "book a table at Toit for tonight", "Table at Toit tonight", step="book", conds=(0,)),
                  _part("food", "order paneer tikka from Orderly", "Paneer tikka from Orderly", step="food", conds=(1,))],
                 [Step(id="book", kind="do", goal="Book a table at Toit tonight"),
                  Step(id="food", kind="do", goal="Order paneer tikka on Orderly")])
    install(daemon, [sup("task", "I'll book Toit and order lunch.", spec)],
            {"step.book": [_not_done("Two times are free.", "Which time: 8:00 or 9:30 pm?"), done("Booked 8:00 pm.")],
             "step.food": _slow("Ordered paneer tikka.", 0.3)})
    seen: list[tuple[str, dict[str, Any]]] = []
    sv = daemon.daemon.supervisor
    original = sv.publish
    sv.publish = lambda topic, data: (seen.append((topic, data)), original(topic, data))[1]  # type: ignore[method-assign]
    turn = Turn(daemon, "book a table at Toit for tonight and order paneer tikka from Orderly for lunch")
    asked = answer_question(daemon, text="8:00 pm")
    events = turn.done()
    rows = [(e["part"], e["state"]) for e in _parts(events)]
    assert ("book", "asks") in rows and rows.index(("book", "asks")) < rows.index(("book", "done"))
    assert ("food", "done") in rows and rows.count(("book", "done")) == 1 and rows.count(("food", "done")) == 1
    ask = next(e for e in _parts(events) if e["state"] == "asks")
    assert ask["question"]["approval_id"] == asked.id and "Which time" in ask["question"]["text"]
    requested = next(d for t, d in seen if t == "approval.requested" and d.get("approval_id") == asked.id)
    assert requested["part"] == "book"
    assert ask["step_task_id"].endswith(".step.book")
    assert outcome(events)["card"]["line"] == "2 of 2 done"


# decision: ORCH-319
def test_a_failed_part_says_so_in_its_row_and_the_count_is_honest(daemon: RunningDaemon) -> None:
    """Scenario 5: one link can't be read: its row says "Couldn't do: reason"; the others fill; 2 of 3 done."""
    spec = _spec("Summarize three links", ["Link 1 summarized", "Link 2 summarized", "Link 3 summarized"],
                 [_part("a", "mysite.example/posts/notch-chat", "mysite.example/posts/notch-chat", step="a", conds=(0,)),
                  _part("b", "ft.com/content/ai-agents-india", "ft.com/content/ai-agents-india", step="b", conds=(1,)),
                  _part("c", "simonwillison.net/2026/Oct/22/agent-loops", "agent-loops", step="c", conds=(2,))],
                 [Step(id="a", kind="do", goal="Summarize link 1"), Step(id="b", kind="do", goal="Summarize link 2"),
                  Step(id="c", kind="do", goal="Summarize link 3")])
    install(daemon, [sup("task", "I'll read each link and summarize it.", spec)],
            {"step.a": done("The notch chat design."), "step.c": done("Small checkable loops."),
             "step.b": _not_done("The page needs a login FERB doesn't have (FT subscription).")})
    events = chat(daemon, "summarize these three links\nmysite.example/posts/notch-chat\n"
                          "ft.com/content/ai-agents-india\nsimonwillison.net/2026/Oct/22/agent-loops")
    failed = next(e for e in _parts(events) if e["part"] == "b")
    assert failed["state"] == "failed" and failed["text"].startswith("Couldn't do:")
    assert "FT subscription" in failed["text"]
    final = outcome(events)
    assert final["status"] == "failed" and final["card"]["line"] == "2 of 3 done"
    assert "2 of 3 done" in final["answer"]


# decision: ORCH-319
def test_a_later_step_updates_a_posted_row_once_with_why(daemon: RunningDaemon) -> None:
    """Scenario 6: a later step finds a posted answer out of date: the row gets the new text, "updated" and one line
    why (one record); no new message, no second copy of the old answer."""
    spec = _spec("Flight and metro", ["The flight's departure is given", "The metro's status is given"],
                 [_part("flight", "when does my Delhi flight leave tomorrow", "Delhi flight tomorrow", step="email",
                        conds=(0,)),
                  _part("metro", "is the airport metro running yet", "Airport metro", step="metro", conds=(1,))],
                 [Step(id="email", kind="do", goal="Find the flight in the booking email"),
                  Step(id="metro", kind="do", goal="Check the airport metro and the airline's web check-in",
                       depends_on=["email"])])
    update = {"part": "flight", "text": "IndiGo 6E 2134 now leaves at 7:55 am.",
              "why": "IndiGo's web check-in shows a schedule change."}
    script = install(daemon, [sup("task", "I'll find your flight and check the metro.", spec)],
                     {"step.email": done("IndiGo 6E 2134 leaves at 7:15 am."),
                      "step.metro": json.dumps(report("done", "Not yet: the airport line is still being built.",
                                                      part_updates=[update]))})
    events = chat(daemon, "when does my Delhi flight leave tomorrow, and is the airport metro running yet?")
    rows = [(e["part"], e["state"]) for e in _parts(events)]
    assert rows == [("flight", "done"), ("metro", "done"), ("flight", "updated")]
    updated = _parts(events)[-1]
    assert updated["text"] == update["text"] and updated["why"] == update["why"]
    context = next(c for _, k, c, _ in script.contexts if k == "step.metro")
    assert "IndiGo 6E 2134 leaves at 7:15 am." in context                   # the step saw the posted answer
    checked = next(c for _, k, c, _ in script.contexts if k == "step.metro.verify")
    assert "now leaves at 7:55 am" in checked                               # its check judged the correction
    assert [e["type"] for e in events].count("outcome") == 1
    final = outcome(events)
    assert final["answer"].count("7:55 am") == 1 and "7:15 am" not in final["answer"]
    assert final["card"]["line"] == "2 of 2 done"


# decision: ORCH-307, ORCH-319
def test_a_part_that_keeps_failing_pauses_only_its_row_and_checks_in(daemon: RunningDaemon) -> None:
    """Scenario 9: after 5 failed attempts of one part's step, only that row is paused with "keep going?" in it; the
    other row is already done. No stops it: the row says it couldn't be done; 1 of 2 done."""
    spec = _spec("Tatkal seats and screenshots", ["Tatkal availability is reported", "The screenshots are renamed"],
                 [_part("seats", "check Tatkal seats Bangalore to Jaipur", "Tatkal seats", step="seats", conds=(0,)),
                  _part("shots", "rename the screenshots on my Desktop", "Rename screenshots", step="shots",
                        conds=(1,))],
                 [Step(id="seats", kind="do", goal="Check Tatkal seats on IRCTC"),
                  Step(id="shots", kind="do", goal="Rename the Desktop screenshots by date")])
    retry = verdict(None, next="retry", missing="No seats were read.", instructions="Try the search again.")
    script = install(daemon, [sup("task", "I'll check Tatkal and rename your screenshots.", spec)],
                     {"step.seats": _not_done("IRCTC answered \"Unable to process your request\"."),
                      "step.seats.verify": retry, "step.shots": done("Renamed 37 screenshots.")})
    seen: list[tuple[str, dict[str, Any]]] = []
    sv = daemon.daemon.supervisor
    original = sv.publish
    sv.publish = lambda topic, data: (seen.append((topic, data)), original(topic, data))[1]  # type: ignore[method-assign]
    turn = Turn(daemon, "check Tatkal seats Bangalore to Jaipur for tomorrow, rename the screenshots on my Desktop")
    asked = answer_question(daemon, choice="No")
    events = turn.done()
    assert script.calls("worker").count("step.seats") == CHECK_IN_AFTER
    paused = next(d for t, d in seen if t == "task.part" and d["state"] == "paused")
    assert paused["part"] == "seats" and paused["question"]["approval_id"] == asked.id
    assert paused["question"]["options"] == list(asked.options) and "Unable to process" in paused["text"]
    rows = [(e["part"], e["state"]) for e in _parts(events)]
    assert ("shots", "done") in rows and rows[-1] == ("seats", "failed")
    assert outcome(events)["card"]["line"] == "1 of 2 done"


# decision: ORCH-319, ORCH-532
def test_a_restart_mid_task_posts_nothing_twice(home: Any, monkeypatch: Any) -> None:
    """Scenario 8: FERB restarts after one row filled: the resumed task's card comes from the record, the filled row is
    never posted again, the working row runs again and fills once."""
    spec = _permit_and_trump()
    first = own_daemon(home, "a")
    try:
        install(first, [sup("task", "I'll check both.", spec)],
                {"step.term": done("His term ends on 20 January 2029."), "step.permit": STALL})
        turn = Turn(first, PERMIT)
        sv = first.daemon.supervisor
        _until(lambda: bool(run(first, sv.running_trees)))
        tree = run(first, sv.running_trees)[0][0]
        _until(lambda: [r["part"] for r in _records(first, tree)] == ["p2"])
        _until(lambda: run(first, lambda: sv.plans.steps(tree))["term"]["status"] == "done")
    finally:
        first.stop()
    turn.thread.join(10)
    import ferb.orchestrator.supervisor as supervisor_module
    resume = supervisor_module.Supervisor.resume_plans

    async def held(self: Any) -> None:
        return None
    monkeypatch.setattr(supervisor_module.Supervisor, "resume_plans", held)
    second = own_daemon(home, "b")
    try:
        monkeypatch.setattr(supervisor_module.Supervisor, "resume_plans", resume)
        sv = second.daemon.supervisor
        published: list[tuple[str, dict[str, Any]]] = []
        original = sv.publish
        sv.publish = lambda topic, data: (published.append((topic, data)), original(topic, data))[1]  # type: ignore
        script = install(second, [], {"step.permit": done("No new permit rule changes this week.")})
        second.loop.call_soon_threadsafe(lambda: second.loop.create_task(sv.resume_plans()))
        _until(lambda: any(t == "task.resumed_event" and d.get("type") == "outcome" for t, d in published))
        assert script.calls("worker") == ["step.permit"]                      # only the unfinished step ran again
        resumed_plan = next(d for t, d in published if t == "task.resumed_event" and d.get("type") == "plan")
        assert resumed_plan["resumed"] is True
        assert resumed_plan["card"]["steps"]["term"]["status"] == "done"     # ORCH-531: the steps from the record
        assert {r["part"]: r["state"] for r in resumed_plan["card"]["rows"]} == {"p1": "working", "p2": "done"}
        parts = [(d["part"], d["state"]) for t, d in published if t == "task.part"]
        assert parts == [("p1", "done")]                                    # p2 is never posted again
        assert [(r["part"], r["state"]) for r in _records(second, tree)] == [("p2", "done"), ("p1", "done")]
        final = next(d for t, d in published if t == "task.resumed_event" and d.get("type") == "outcome")
        assert final["card"]["line"] == "2 of 2 done"
    finally:
        second.stop()


# decision: ORCH-319
def test_parts_are_checked_and_told_back_for_repair(daemon: RunningDaemon) -> None:
    """Code validates the planner's parts (ORCH-319): a summing-up step, a condition in no part, a quote that isn't
    the owner's words are told back; the repaired plan runs."""
    bad = _spec("Permit news and Trump's term", ["Permit news is reported", "The end of Trump's term is given"],
                [_part("p1", "any news about the work permit", "Permit", step="permit", conds=(0,)),
                 _part("p2", "who won the election", "Term", step="term")],
                [Step(id="permit", kind="do", goal="Check permit news"), Step(id="term", kind="do", goal="Trump's term"),
                 Step(id="sum", kind="do", goal="Combine both answers", depends_on=["permit", "term"])])
    script = install(daemon, [sup("task", "On it.", bad), sup("task", "On it.", _permit_and_trump())],
                     {"step.permit": done("No news."), "step.term": done("20 January 2029.")})
    final = outcome(chat(daemon, PERMIT))
    repair = run(daemon, lambda: [e.data["problems"] for e in daemon.daemon.journal.of_kind("task.repair")])[0]
    said = " ".join(repair)
    assert "lead to no part's answer" in said and "['sum']" in said
    assert "belong to no part" in said and "owner's own words" in said
    assert final["card"]["line"] == "2 of 2 done" and "step.sum" not in script.calls("worker")


# decision: ORCH-319, ORCH-520, ORCH-522
def test_a_retry_message_may_quote_the_owners_earlier_request_for_its_parts(daemon: RunningDaemon) -> None:
    """Live 2026-11-02: the owner's "now??" retried a request that had failed; the parts quoted the earlier message,
    and the turn was rejected after one repair. A part's quote may come from the owner's recent messages in this chat,
    the one source every quote check reads (ORCH-520)."""
    install(daemon, [sup("reply", "I couldn't reach the news sites just now."), sup("task", "On it.", _permit_and_trump())],
            {"step.permit": done("No news."), "step.term": done("20 January 2029.")})
    chat(daemon, PERMIT)
    final = outcome(chat(daemon, "now??"))
    assert final["card"]["line"] == "2 of 2 done"
    assert run(daemon, lambda: daemon.daemon.journal.of_kind("task.repair")) == []


def test_a_plan_saved_before_parts_resumes_and_answers_from_its_last_steps(daemon: RunningDaemon) -> None:
    """Upgrade (STORE-403): a plan saved before step 8d ends in two steps (made for the removed final
    worker). Resumed after the update with both finished, it answers with their saved answers, its success conditions
    unconfirmed (nothing checks them now), and makes no model call."""
    from ferb.orchestrator.schemas import TaskSpec
    sv = daemon.daemon.supervisor
    plan = Plan(steps=[Step(id="a", kind="do", goal="Do A"), Step(id="b", kind="do", goal="Do B")])
    old = plan.model_dump()
    old["steps"][0]["tells_owner"] = True                  # the 10-27 field (reverted 9a55719) still loads
    assert Plan.model_validate(old) == plan
    spec = TaskSpec.model_validate(task("Do A and B", conds=(cond("A and B are done"),)) | {"plan": plan.model_dump()})
    saved = {"spec": {k: v for k, v in spec.model_dump().items() if k != "parts"}, "request_id": "r-old",
             "words": "do A and B", "payload": "do A and B", "origins": [], "message_origins": [], "unattended": False}
    script = install(daemon, [], {})
    published: list[tuple[str, dict[str, Any]]] = []
    original = sv.publish
    sv.publish = lambda topic, data: (published.append((topic, data)), original(topic, data))[1]  # type: ignore

    async def setup() -> None:
        sv.plans.create("old", plan, saved, "s1")
        sv.plans.finish("old", "a", {"status": "done", "summary": "A done", "answer": "Did A."})
        sv.plans.finish("old", "b", {"status": "done", "summary": "B done", "answer": "Did B."})
        sv.plans.set_status("old", "suspended")
        await sv.resume_plans()
    daemon.run(setup(), timeout=20)
    final = next(d for t, d in published if t == "task.resumed_event" and d.get("type") == "outcome")
    assert final["status"] == "done_unverified" and "Did A." in final["answer"] and "Did B." in final["answer"]
    assert "A and B are done" in final["note"]
    assert not script.calls("worker")


# decision: ORCH-319, ORCH-324
def test_continuing_a_multi_part_task_never_shows_a_finished_part_again(daemon: RunningDaemon) -> None:
    """Rows are keyed by (task, part) across turns: the owner's follow-up on the failed link runs only that part; its
    card carries only that row, which updates to its answer; the finished parts stay as posted in the first card,
    shown once; the count covers the rows this card shows (ORCH-543: rows and count always agree)."""
    spec = _spec("Summarize three links", ["Link 1 summarized", "Link 2 summarized", "Link 3 summarized"],
                 [_part("a", "mysite.example/posts/notch-chat", "notch-chat", step="a", conds=(0,)),
                  _part("b", "ft.com/content/ai-agents-india", "ai-agents-india", step="b", conds=(1,)),
                  _part("c", "simonwillison.net/2026/Oct/22/agent-loops", "agent-loops", step="c", conds=(2,))],
                 [Step(id="a", kind="do", goal="Summarize link 1"), Step(id="b", kind="do", goal="Summarize link 2"),
                  Step(id="c", kind="do", goal="Summarize link 3")])
    install(daemon, [sup("task", "I'll read each link and summarize it.", spec)],
            {"step.a": done("The notch chat design."), "step.c": done("Small checkable loops."),
             "step.b": _not_done("The page needs a login FERB doesn't have.")})
    first = chat(daemon, "summarize these three links\nmysite.example/posts/notch-chat\n"
                         "ft.com/content/ai-agents-india\nsimonwillison.net/2026/Oct/22/agent-loops")
    tree = outcome(first)["task_id"]
    assert outcome(first)["card"]["line"] == "2 of 3 done"
    follow = json.loads(sup("task", "Trying the FT link again.", task("Summarize three links")))
    follow["control"] = {"task_id": tree, "action": "continue", "new_goal": False, "standing_sends": "unchanged",
                         "supersedes": []}
    script = install(daemon, [json.dumps(follow)], {"step.b": done("India's agent startups, in numbers.")})
    second = chat(daemon, "I logged in to FT, try that one again")
    assert script.calls("worker") == ["step.b"]                             # the finished parts don't run again
    plan = next(e for e in second if e["type"] == "plan")
    assert [r["part"] for r in plan["card"]["rows"]] == ["b"]               # only the part this run changes
    assert [(e["part"], e["state"]) for e in _parts(second)] == [("b", "updated")]
    updated = _parts(second)[0]
    assert updated["text"] == "India's agent startups, in numbers." and updated["why"]
    final = outcome(second)
    assert final["card"]["line"] == "1 of 1 done" and [r["part"] for r in final["card"]["rows"]] == ["b"]
    assert final["status"] in ("done", "done_unverified")                   # every ask is done
    assert "notch chat design" not in final["answer"] and "checkable loops" not in final["answer"]
    shown = [(r["part"], r["state"]) for r in _records(daemon, tree)]
    assert shown.count(("a", "done")) == 1 and shown.count(("c", "done")) == 1     # each finished part, once


# ---------- the step lifecycle (ORCH-531) ----------

def _steps(events: list[dict[str, Any]], step: str) -> list[dict[str, Any]]:
    return [e for e in events if e["type"] == "step" and e["step"] == step]


def _fetch(sock: Any, answer: str) -> Any:
    def act(token: str) -> str:
        tool(sock, token, "task.evidence")            # one tool call: the step's live line
        return done(answer)
    return act


# decision: ORCH-531
def test_step_states_follow_each_step_from_waiting_to_done_with_its_row_and_times(daemon: RunningDaemon) -> None:
    """The plan event's card names each step's state (a step that depends on another waits on it); each step then
    reports starting, running, checking and done, with its row, attempt and times, and its live line names the step's
    own task."""
    spec = _spec("Mail and leads", ["The mail is summarized", "Leads are listed"],
                 [_part("mail", "check my mail", "Mail", step="mail", conds=(0,)),
                  _part("leads", "find leads from it", "Leads", step="leads", conds=(1,))],
                 [Step(id="mail", kind="do", goal="Read the mail"),
                  Step(id="leads", kind="do", goal="Find leads in the mail", depends_on=["mail"])])
    install(daemon, [sup("task", "I'll read your mail, then find leads.", spec)],
            {"step.mail": _fetch(daemon.sock, "3 new mails."), "step.leads": done("2 leads.")})
    c = Client(daemon.sock, timeout=30)
    try:
        raw = list(c.stream("chat.send", {"session_id": "s1", "text": "check my mail and find leads from it"}))
    finally:
        c.close()
    events = turn_events(raw)
    plan = next(e for e in events if e["type"] == "plan")
    assert plan["card"]["steps"]["leads"]["status"] == "waiting"
    assert plan["card"]["steps"]["leads"]["waits_on"] == ["mail"]
    mail = [e["status"] for e in _steps(events, "mail")]
    assert mail[:4] == ["starting", "running", "checking", "done"]
    first, last = _steps(events, "mail")[0], _steps(events, "mail")[3]
    assert first["part"] == "mail" and first["step_task_id"].endswith(".step.mail") and first["started_at"]
    assert last["ended_at"] and last["started_at"] == first["started_at"] and last["attempt"] == 1
    assert [e["seq"] for e in events if e["type"] == "step"] == sorted(e["seq"] for e in events if e["type"] == "step")
    leads = [e["status"] for e in _steps(events, "leads")]
    assert leads[0] == "waiting" and leads[-1] == "done"
    assert events.index(_steps(events, "leads")[1]) > events.index(_steps(events, "mail")[3])   # only after mail
    progress = [e for e in raw if e["type"] == "progress" and not e.get("connection")]
    assert progress and all(e["step_task_id"].endswith(".step.mail") for e in progress)


# decision: ORCH-532
def test_a_failed_prerequisite_settles_its_dependent_row_at_once(daemon: RunningDaemon) -> None:
    """A row whose step depends on a step that couldn't be done is settled as soon as that step ends (won't run), while
    an independent step is still working; not at the plan's end."""
    spec = _spec("Resumes and gigs", ["Resumes found", "Resume revised", "Gigs listed"],
                 [_part("find", "find my resumes", "Resumes", step="find", conds=(0,)),
                  _part("revise", "revise the best one", "Revised resume", step="revise", conds=(1,)),
                  _part("gigs", "list some gigs", "Gigs", step="gigs", conds=(2,))],
                 [Step(id="find", kind="do", goal="Find the resumes"),
                  Step(id="revise", kind="do", goal="Revise the best resume", depends_on=["find"]),
                  Step(id="gigs", kind="do", goal="List gigs")])
    install(daemon, [sup("task", "I'll find and revise your resume and list gigs.", spec)],
            {"step.find": _not_done("No resume files are in the folder."),
             "step.gigs": _slow("Three gigs.", 0.8)})
    events = chat(daemon, "find my resumes, revise the best one, and list some gigs")
    wont = _steps(events, "revise")
    assert wont[-1]["status"] == "wont_run" and wont[-1]["waits_on"] == ["find"]
    rows = _parts(events)
    revise = next(i for i, e in enumerate(rows) if e["part"] == "revise")
    gigs = next(i for i, e in enumerate(rows) if e["part"] == "gigs")
    assert rows[revise]["state"] == "failed" and revise < gigs              # settled before the slow row filled
    assert outcome(events)["card"]["line"] == "1 of 3 done"
