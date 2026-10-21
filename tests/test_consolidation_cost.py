"""Memory review I3, M5 (fix wave 3) and L29: the sleep service's calls follow what changed, never one per episode.

A pass re-reads only cards whose inputs changed (missing, built by an older composer, the episode rebuilt, a new owner
verdict). Cards go to the model by intent (L29: the label the learning model gave each card), one call per intent,
never one call per episode carrying every episode that shares a word: an intent is first consolidated once it has three
verified runs, and from then on (or once it has a procedure) again whenever its runs change; a call carries its
procedure's example runs, then the newest others, as many as fit the model's window, each whole (so each call re-sends
the intent's runs up to the window). A call that failed is tried on the next pass; an answer outside the schema, a call
whose procedures were all refused, or runs that don't fit, wait for the intent's runs to change or for `ferb memory
consolidate`; a failed distill day waits for its evidence to change or `ferb memory distill`. Ported from the review's
probes P5 and P5b, which asserted the bug."""
import asyncio
import json
from typing import Any, Callable

import pytest

from ferb.core.types import ModelLimits
from ferb.memory import intents
from ferb.memory.consolidate import FIRST_LEARNING_RUNS, PROPOSAL_SCHEMA, Consolidator
from ferb.memory.intents import INTENT_SCHEMA
from ferb.memory.learning import Learning, digest, encoded
from tests.conftest import FakeDistillRuntime
from tests.support.learning_fixtures import EMPTY, NoMemory, TickingClock, label_card, new_intent, new_store
from tests.support.learning_runtime import SchemaAnswers


@pytest.fixture
def learning(tmp_path):
    db, store = new_store(tmp_path / "memory.db", TickingClock())       # each card recorded after the one before
    yield store
    db.close()


def card(store: Learning, ident: str, tool: str = "browser.open", request: str = "open the page and read it",
         intent: str = "a test intent", verified: bool = True) -> Any:
    """A card already labelled with `intent` (L29): these tests count consolidation calls only."""
    node = store.put("episode", ident, {"task_id": ident, "verified": verified, "request": request, "apps": {},
                                        "slots": {}, "status": "done" if verified else "failed",
                                        "actions": [{"id": ident + "-a", "tool": tool, "args": {"n": ident}}]},
                     [], origin_label="ferb", made_by="code:evidence-card/2", state="active")
    label_card(store, node, intent)
    return node


def runs(store: Learning, prefix: str, intent: str, n: int = FIRST_LEARNING_RUNS, **kw: Any) -> list[Any]:
    return [card(store, f"{prefix}{i}", intent=intent, **kw) for i in range(n)]


def fake(answer: Any = EMPTY, **kw: Any) -> SchemaAnswers:
    """Answers each consolidation call with `answer` (or `answer(context)` when it is a function)."""
    return SchemaAnswers({id(PROPOSAL_SCHEMA): answer if callable(answer) else lambda ctx: answer}, **kw)


def failing(message: str) -> Callable[[str], Any]:
    """A consolidation call that fails as a call: the runtime raises (the service, the connection, a timeout)."""
    def answer(ctx: str) -> Any:
        raise RuntimeError(message)
    return answer


def consolidator(store: Learning, make: Callable[..., Any], **kw: Any) -> Consolidator:
    return Consolidator(store, NoMemory(), make, **kw)


def consolidation_calls(rt: SchemaAnswers) -> list[str]:
    """The contexts of `rt`'s consolidation calls; every card here is labelled, so it is never asked for a label."""
    assert all(schema == id(PROPOSAL_SCHEMA) for schema, _ in rt.calls)
    return [ctx for _, ctx in rt.calls]


def job(store: Learning, intent: str) -> tuple[str, str | None, str]:
    """The intent's consolidation job (L29: `intent:<intent id>`): status, error, inputs."""
    row = store.db.conn.execute("SELECT status, error, inputs FROM learning_jobs WHERE source_ref = ?",
                                ("intent:" + intents.intent_id(intent),)).fetchone()
    return tuple(row) if row else ()          # type: ignore[return-value]


def group(cards: list[Any]) -> str:
    return digest(sorted(c["id"] for c in cards))


def test_an_intent_waits_for_three_verified_runs(learning) -> None:
    """L29: three runs of "find the cheapest egg curry" landed in three groups by tools and apps, so no call ever saw
    them together. Runs of one intent are one group whatever tools they used; it is first learned from three verified
    runs."""
    food = [card(learning, "a1", tool="browser.open", intent="food"),
            card(learning, "a2", tool="browser.type", intent="food")]
    rt = fake()
    asyncio.run(consolidator(learning, rt.make).catch_up())
    assert consolidation_calls(rt) == [] and job(learning, "food") == ()
    food.append(card(learning, "a3", tool="mac.act", intent="food"))
    asyncio.run(consolidator(learning, rt.make).catch_up())
    [call] = consolidation_calls(rt)
    shown = json.loads(call.split("\n\n")[0])
    [known] = intents.intents(learning)
    assert shown["intent"] == {k: known[k] for k in ("id", "name", "description", "slots")}
    assert [e["id"] for e in shown["episodes"]] == [c["id"] for c in reversed(food)]     # its runs, newest first
    status, _, inputs = job(learning, "food")
    assert status == "done" and inputs == group(food)


def test_failed_runs_do_not_count_toward_three_but_go_with_the_call(learning) -> None:
    done = runs(learning, "b", "x", n=2)
    failed = card(learning, "bf", intent="x", verified=False)
    rt = fake()
    asyncio.run(consolidator(learning, rt.make).catch_up())
    assert consolidation_calls(rt) == []
    done.append(card(learning, "b2", intent="x"))
    asyncio.run(consolidator(learning, rt.make).catch_up())
    [call] = consolidation_calls(rt)
    assert failed["id"] in call and all(c["id"] in call for c in done)          # the failure goes along, for lessons


def test_each_intent_is_its_own_call_each_card_is_sent_once_and_nothing_new_means_no_call(learning) -> None:
    """P5: 40 cards whose requests shared "the" made 40 model calls, each carrying all 40 cards (quadratic). Two
    intents are two calls, even when their runs used the same tools."""
    food, mail = runs(learning, "f", "food", n=40), runs(learning, "m", "mail")
    rt = fake()
    asyncio.run(consolidator(learning, rt.make).catch_up())
    calls = consolidation_calls(rt)
    assert len(calls) == 2 and all(sum(c["id"] in call for call in calls) == 1 for c in food + mail)
    assert not any(food[0]["id"] in call and mail[0]["id"] in call for call in calls)
    asyncio.run(consolidator(learning, rt.make).catch_up())
    assert len(consolidation_calls(rt)) == 2


def test_an_intent_with_a_procedure_is_consolidated_again_on_each_new_run(learning) -> None:
    """Once an intent has a procedure, every new run of it (verified or failed) updates the procedure and its lessons,
    three verified runs or not."""
    first = card(learning, "p0", intent="p")
    procedure = learning.put("procedure", intents.intent_id("p"), {"intent": "p", "slots": {}, "steps": [],
                                                                    "examples": []},
                             [first["id"]], made_by="model:test", state="candidate")
    rt = fake()
    asyncio.run(consolidator(learning, rt.make).catch_up())
    [call] = consolidation_calls(rt)
    assert first["id"] in call and procedure["id"] in call                       # the procedure goes with its runs
    asyncio.run(consolidator(learning, rt.make).catch_up())                      # nothing new: no call
    assert len(consolidation_calls(rt)) == 1
    new = card(learning, "p1", intent="p", verified=False)                       # a failed new run
    asyncio.run(consolidator(learning, rt.make).catch_up())
    calls = consolidation_calls(rt)
    assert len(calls) == 2 and new["id"] in calls[1] and first["id"] in calls[1]


def _window_for(cards_room: int, intent: str = "a test intent", procedures: tuple[Any, ...] = ()) -> ModelLimits:
    """A window that leaves room for about `cards_room` tokens of runs beside the consolidation prompt and the intent's
    `procedures` (no lessons), measured with the same count FERB uses (o200k, or its byte ceiling when the tokenizer is
    missing)."""
    from ferb.core.context import count_tokens, overhead_tokens
    from ferb.core.prompts import load_prompt
    from ferb.memory.learning import encoded
    system = count_tokens(load_prompt("memory/consolidate.md"))
    about = {"id": intents.intent_id(intent), "name": intent, "description": intent, "slots": {}}
    base = count_tokens(encoded({"intent": about, "episodes": [], "existing_lessons": [], "rejected_by_owner": [],
                                 "existing_procedures": [{"id": p["id"], "state": p["state"], "body": p["body"]}
                                                         for p in procedures]}))
    budget = system + overhead_tokens(10 ** 6, system) + base + cards_room
    return ModelLimits("small", budget + 1_000, 1_000, "test double")


def test_runs_that_do_not_all_fit_go_newest_first(learning) -> None:
    from ferb.core.context import count_tokens
    from ferb.memory.learning import card_digest, encoded
    cards = runs(learning, "w", "w", n=12, request="read the page " + "word " * 300)
    one = count_tokens(encoded(card_digest(cards[-1]))) + count_tokens(",")
    rt = fake(limits=_window_for(4 * one + one // 2, "w"))                     # about four runs
    out = asyncio.run(consolidator(learning, rt.make).catch_up())
    [call] = consolidation_calls(rt)
    assert [c["id"] in call for c in cards] == [False] * 8 + [True] * 4
    assert call.index(cards[-1]["id"]) < call.index(cards[-4]["id"])             # newest first
    assert not [r for r in out["sources"] if "error" in r]
    status, _, inputs = job(learning, "w")
    assert status == "done" and inputs == group(cards)          # the older runs wait for the intent's runs to change


def test_newest_means_when_the_task_ended_so_a_re_read_card_never_makes_an_old_run_newest(learning) -> None:
    """A verdict re-reads an old run's card (a new node, written last): its task still ended first, so it is still the
    oldest run (the card's `ended_at`, not when it was written)."""
    from ferb.core.context import count_tokens
    from ferb.memory.learning import card_digest, encoded
    body = {"verified": True, "request": "read the page " + "word " * 300, "apps": {}, "slots": {}, "status": "done"}
    runs_ = [learning.put("episode", f"r{n}", {**body, "task_id": f"r{n}", "ended_at": f"2026-10-0{n + 1}T09:00:00",
                                               "actions": [{"id": f"r{n}-a", "tool": "browser.open", "args": {}}]},
                          [], origin_label="ferb", made_by="code:evidence-card/2", state="active") for n in range(3)]
    reread = learning.put("episode", "r0", {**runs_[0]["body"], "feedback": [{"verdict": "pass"}]}, [],
                          origin_label="ferb", made_by="code:evidence-card/2", state="active")
    for node in (reread, *runs_[1:]):
        label_card(learning, node, "r")
    assert reread["recorded_at"] > runs_[2]["recorded_at"]                       # written last
    one = count_tokens(encoded(card_digest(reread))) + count_tokens(",")
    rt = fake(limits=_window_for(2 * one + one // 2, "r"))                      # about two runs
    asyncio.run(consolidator(learning, rt.make).catch_up())
    [call] = consolidation_calls(rt)
    assert runs_[2]["id"] in call and runs_[1]["id"] in call and reread["id"] not in call
    assert call.index(runs_[2]["id"]) < call.index(runs_[1]["id"])


def test_a_second_pass_with_nothing_changed_reads_no_card_and_calls_no_model(learning) -> None:
    """P5b: every pass re-captured every card through the card reader (1.2-1.6 s each)."""
    for i in range(5):
        learning.db.conn.execute(
            "INSERT INTO episodes (task_id, request_text, goal, status, verified, answer, conditions_json, steps_json, "
            "problems_json, tools_json, origins, external_read, usage_json, desktop_json, made_by, built_at, ended_at) "
            "VALUES (?, 'r', 'g', 'done', 1, '', '[]', '[]', '[]', '{}', 'ferb', 0, '{}', '{}', 'x', 'now', ?)",
            (f"e{i}", f"2026-10-0{i + 1}"))
    captured: list[str] = []

    async def capture(task_id: str) -> Any:
        captured.append(task_id)
        return learning.capture_episode(task_id)
    rt = SchemaAnswers({id(INTENT_SCHEMA): lambda ctx: new_intent("open pages", "a page"),     # each new card's label
                        id(PROPOSAL_SCHEMA): lambda ctx: EMPTY})

    def calls(schema: dict[str, Any]) -> int:
        return sum(s == id(schema) for s, _ in rt.calls)
    sleep = Consolidator(learning, NoMemory(), rt.make, capture=capture)
    asyncio.run(sleep.catch_up())
    assert sorted(captured) == [f"e{i}" for i in range(5)] and calls(PROPOSAL_SCHEMA) == 1 and calls(INTENT_SCHEMA) == 5
    asyncio.run(sleep.catch_up())
    assert len(captured) == 5 and calls(PROPOSAL_SCHEMA) == 1 and calls(INTENT_SCHEMA) == 5
    learning.db.conn.execute("INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES "
                             "('v1', 'e2', 'fail', '', 'now')")                 # one card's input changed
    asyncio.run(sleep.catch_up())
    assert captured[5:] == ["e2"] and calls(INTENT_SCHEMA) == 5                # its re-read card keeps its label
    assert len(intents.labels(learning)) == 5


def test_a_failed_call_is_tried_again_on_the_next_pass(learning) -> None:
    """L29, Task 2's rule for label calls: a call that failed (the service, the connection, a timeout, the runtime)
    is tried on the next pass; its job's inputs say so (the runs' digest and ":retry")."""
    cards = runs(learning, "a", "a test intent")
    broken = fake(failing("usage limit reached"))
    out = asyncio.run(consolidator(learning, broken.make).catch_up())
    assert "usage limit reached" in out["sources"][0]["error"]
    status, error, inputs = job(learning, "a test intent")
    assert status == "failed" and "usage limit reached" in str(error) and inputs == group(cards) + ":retry"
    asyncio.run(consolidator(learning, broken.make).catch_up())                # the next pass, nothing changed
    assert len(consolidation_calls(broken)) == 2
    working = fake()
    asyncio.run(consolidator(learning, working.make).catch_up())
    assert len(consolidation_calls(working)) == 1 and job(learning, "a test intent") == ("done", None, group(cards))
    asyncio.run(consolidator(learning, working.make).catch_up())
    assert len(consolidation_calls(working)) == 1


def test_an_answer_outside_the_schema_waits_for_a_change_or_an_explicit_consolidate(learning) -> None:
    """M5: a failed consolidation was tried again with a full model call every idle hour. An answer outside the schema
    waits for the intent's runs to change or `ferb memory consolidate` (its job's inputs: the runs' digest)."""
    cards = runs(learning, "a", "a test intent")
    bad = fake({"procedures": "not a list"})
    out = asyncio.run(consolidator(learning, bad.make).catch_up())
    assert "InvalidAnswer" in out["sources"][0]["error"]
    status, error, inputs = job(learning, "a test intent")
    assert status == "failed" and "InvalidAnswer" in str(error) and inputs == group(cards)
    asyncio.run(consolidator(learning, bad.make).catch_up())                   # the next idle hour: nothing changed
    assert len(consolidation_calls(bad)) == 1
    asyncio.run(consolidator(learning, bad.make).catch_up(force=True))         # `ferb memory consolidate`
    assert len(consolidation_calls(bad)) == 2
    cards.append(card(learning, "b", intent="a test intent"))                  # a new run of the same intent
    working = fake()
    asyncio.run(consolidator(learning, working.make).catch_up())
    assert len(consolidation_calls(working)) == 1 and job(learning, "a test intent") == ("done", None, group(cards))


def test_a_failed_distill_day_waits_for_its_evidence_to_change(daemon) -> None:
    """M5: a failed distillation day was retried with a full model call every hour."""
    from tests.conftest import trail_row

    d = daemon.daemon
    trail_row(daemon, "2025-12-30")
    calls: list[int] = []

    def failing(*_a: Any, **_k: Any) -> Any:
        calls.append(1)
        return FakeDistillRuntime(RuntimeError("model unavailable"))
    d.distiller.make_runtime = failing

    async def passes() -> list[Any]:
        return [await d.distiller.catch_up(), await d.distiller.catch_up()]
    first, second = daemon.run(passes(), timeout=30)
    assert [r["status"] for r in first if r["day"] == "2025-12-30"] == ["failed"]
    assert not [r for r in second if r["day"] == "2025-12-30"]                     # not retried: nothing changed
    assert len(calls) == 1
    row = daemon.run(_row(d, "2025-12-30"))
    assert row["status"] == "failed" and "model unavailable" in row["detail"]


async def _row(d: Any, day: str) -> Any:
    return d.db.conn.execute("SELECT * FROM distill_runs WHERE day=?", (day,)).fetchone()


def test_the_consolidator_is_shown_only_the_intents_procedure(learning) -> None:
    """Another intent's procedure never goes with these runs, even one whose steps use only their tools; a procedure
    of this intent the owner turned down goes as its intent text, never to be proposed again (M3)."""
    mail = runs(learning, "m", "send mail", tool="mail.send")
    learning.put("procedure", intents.intent_id("open pages"), {"intent": "open pages", "preconditions": {"apps": {}},
                                                                "steps": [{"tool": "mail.send"}]},
                 [], made_by="model:t", state="candidate")
    learning.put("procedure", intents.intent_id("send mail"), {"intent": "mail the weekly report", "steps": [],
                                                               "preconditions": {"apps": {}}},
                 [], made_by="model:t", state="retired")
    rt = fake()
    asyncio.run(consolidator(learning, rt.make).catch_up())
    [call] = consolidation_calls(rt)
    assert all(c["id"] in call for c in mail) and "open pages" not in call
    assert '"existing_procedures":[]' in call
    assert '"rejected_by_owner":[{"intent":"mail the weekly report"}]' in call
    assert context_of(call)["procedure_rejected"] is True             # any procedure for it would be refused (M3)


# ---------- I4: the day rollups never run on the event loop, nor once a task started ----------

def _summary_day(store: Learning, day: str) -> None:
    store.db.conn.execute("INSERT INTO journal (id, at, kind, data_json) VALUES (?, ?, 'trail.day_summary', ?)",
                          ("sum-" + day, day + "T23:00:00+00:00", json.dumps({"day": day, "made_by": "code:summary/1"})))


def test_rollups_run_in_a_worker_thread_and_only_for_days_that_changed(learning, monkeypatch) -> None:
    """Review I4: rollups() rebuilt every summarized day on the event loop every pass (8.5 s for 30 days, 23 s for 90
    on synthetic data), even after a task had started."""
    import threading
    from ferb.memory import summary
    for day in ("2026-10-01", "2026-10-02"):
        _summary_day(learning, day)
    built: list[tuple[str, str]] = []
    real = summary.build

    def counted(db: Any, day: str) -> Any:
        built.append((day, threading.current_thread().name))
        return real(db, day)
    monkeypatch.setattr(summary, "build", counted)
    sleep = consolidator(learning, fake().make)

    async def run_pass() -> str:
        await sleep.catch_up()
        return threading.current_thread().name
    loop_thread = asyncio.run(run_pass())
    assert sorted(day for day, _ in built) == ["2026-10-01", "2026-10-02"]
    assert all(thread != loop_thread for _, thread in built)
    assert {n["key"] for n in learning.items("activity_summary")} == {"trail.day_summary:2026-10-01",
                                                                      "trail.day_summary:2026-10-02"}
    asyncio.run(sleep.catch_up())                                       # nothing changed: no day is built again
    assert len(built) == 2


def test_rollups_never_start_once_a_task_has_started(learning, monkeypatch) -> None:
    from ferb.memory import summary
    _summary_day(learning, "2026-10-01")
    runs(learning, "a", "a test intent")
    built: list[str] = []
    monkeypatch.setattr(summary, "build", lambda db, day: built.append(day))
    checks = iter([True, True])                    # idle at the start and before the model call, then a task starts
    rt = fake()
    sleep = consolidator(learning, rt.make, idle=lambda: next(checks, False))
    asyncio.run(sleep.catch_up())
    assert built == [] and len(consolidation_calls(rt)) == 1


def test_a_rollup_written_while_a_forget_ran_never_keeps_what_was_forgotten(learning, monkeypatch) -> None:
    """Review minor: the rollups run in a worker thread; a forget on the loop meanwhile could delete a card after a
    rollup read it, leaving a day summary citing (and naming) the forgotten card. What a pass's rollups wrote is checked
    once they finish: a node built on a forgotten source is forgotten too."""
    gone = card(learning, "gone")
    real = Consolidator.rollups

    def racing(self: Consolidator, own: Any = None, stop: Any = lambda: False) -> None:
        real(self, own, stop)                                       # writes the day summary citing the card
        own.db.conn.execute("DELETE FROM learning_nodes WHERE id = ?", (gone["id"],))     # the forget, meanwhile
        own.db.conn.execute("INSERT INTO learning_forgotten VALUES (?, 'now')", (gone["id"],))
    monkeypatch.setattr(Consolidator, "rollups", racing)
    forgot: list[list[str]] = []

    def forget(ids: list[str]) -> list[str]:                     # the daemon's forget path (round 5 re-review)
        forgot.append(list(ids))
        return learning.forget(ids)
    asyncio.run(consolidator(learning, fake().make, forget=forget).catch_up())
    citing = [n for n in learning.items("summary") if gone["id"] in n["sources"]]
    assert citing == [] and forgot


def test_a_card_read_that_failed_is_tried_once_more_then_waits(learning) -> None:
    """Review minor: one card-reader crash parked the task until its inputs changed. It is tried once more on the next
    pass; a second failure on the same inputs waits for a change or `ferb memory consolidate`."""
    from ferb.memory.learning import LearningRefused
    learning.db.conn.execute(
        "INSERT INTO episodes (task_id, request_text, goal, status, answer, conditions_json, steps_json, "
        "problems_json, tools_json, origins, external_read, usage_json, desktop_json, made_by, built_at, ended_at) "
        "VALUES ('e1', 'r', 'g', 'done', '', '[]', '[]', '[]', '{}', 'ferb', 0, '{}', '{}', 'x', 'now', 'now')")
    tried: list[str] = []

    async def crashing(task_id: str) -> Any:
        tried.append(task_id)
        raise LearningRefused("the card reader exited -9 without an answer")
    sleep = consolidator(learning, fake().make, capture=crashing)
    for _ in range(3):
        asyncio.run(sleep.catch_up())
    assert tried == ["e1", "e1"]
    asyncio.run(sleep.catch_up(force=True))
    assert tried == ["e1", "e1", "e1"]


def test_procedures_too_large_for_the_window_are_named_as_the_reason_and_wait(learning) -> None:
    first = card(learning, "a")
    learning.put("procedure", intents.intent_id("a test intent"), {
        "intent": "open pages", "steps": [{"tool": "browser.open"}], "ui_hints": "hint " * 3000,
        "preconditions": {"apps": {}}}, [first["id"]], made_by="model:t", state="candidate")
    rt = fake(limits=_window_for(200))                                          # far less than the procedure
    out = asyncio.run(consolidator(learning, rt.make).catch_up())
    assert "procedures" in out["sources"][0]["error"] and "too large for the model's window; sent whole" \
        not in out["sources"][0]["error"]
    assert consolidation_calls(rt) == []
    status, _, inputs = job(learning, "a test intent")
    assert status == "failed" and inputs == group([first])                     # waits for its runs to change
    assert asyncio.run(consolidator(learning, rt.make).catch_up())["sources"] == []


def test_forgetting_a_run_deletes_its_intents_job(learning) -> None:
    """A job's error may quote what its call was sent (Codex's words, an answer outside the schema): forgetting one of
    the intent's runs deletes the job with the run's label, as a forget always deleted the card's own job. Only that
    intent's job goes; with two runs left and no procedure, the next pass makes no call and writes no job."""
    cards, others = runs(learning, "a", "a test intent"), runs(learning, "o", "other")
    asyncio.run(consolidator(learning, fake(lambda ctx: {"procedures": "not a list"} if cards[0]["id"] in ctx
                                            else EMPTY).make).catch_up())
    assert job(learning, "a test intent")[0] == "failed" and job(learning, "other")[0] == "done"
    learning.forget([cards[0]["id"]])
    assert job(learning, "a test intent") == () and job(learning, "other")[:2] == ("done", None)
    rt = fake()
    asyncio.run(consolidator(learning, rt.make).catch_up())
    assert consolidation_calls(rt) == [] and job(learning, "a test intent") == ()


# ---------- fix round 1 (Task 3 review I1, I2, M2-M5) ----------

DISHES = ("egg curry", "paneer", "dal", "rajma", "chole", "biryani", "korma", "saag", "aloo", "gobi", "tikka", "kofta",
          "pulao", "idli")


def dish_run(store: Learning, n: int, intent: str = "food") -> Any:
    """A verified run of a dish search: one typed dish, the slot a procedure abstracts."""
    node = store.put("episode", f"d{n}", {"task_id": f"d{n}", "verified": True, "status": "done", "apps": {},
                                          "slots": {}, "request": "find the cheapest dish " + "word " * 300,
                                          "actions": [{"id": f"d{n}-a", "tool": "browser.type",
                                                       "args": {"text": DISHES[n]}, "outside_action": False}]},
                     [], origin_label="ferb", made_by="code:evidence-card/2", state="active")
    label_card(store, node, intent)
    return node


def food_procedure(cards: list[Any]) -> dict[str, Any]:
    """A procedure proposal (PROPOSAL_SCHEMA shape) built on `cards`, each an example with its dish."""
    return {"intent": "food", "slots": [{"name": "dish", "value": "string"}], "preconditions": {"apps": []},
            "ui_hints": "", "steps": [{"tool": "browser.type", "args": [{"name": "text", "value": '"{dish}"'}],
                                       "expect": "typed", "optional": False,
                                       "evidence": [c["body"]["actions"][0]["id"] for c in cards]}],
            "examples": [{"episode": c["id"], "slots": [{"name": "dish", "value": json.dumps(
                c["body"]["actions"][0]["args"]["text"])}]} for c in cards]}


def proposing(*procedures: dict[str, Any]) -> dict[str, Any]:
    return {**EMPTY, "procedures": list(procedures)}


def run_tokens(node: Any) -> int:
    """One run's cost in a call, as sent: its card digest, marked for annotation."""
    from ferb.core.context import count_tokens
    from ferb.memory.learning import card_digest, encoded
    return count_tokens(encoded(card_digest(node) | {"annotate": True})) + count_tokens(",")


def context_of(call: str) -> Any:
    return json.loads(call.split("\n\n")[0])


def test_a_procedures_example_runs_go_first_so_its_revision_is_never_cut_out(learning) -> None:
    """Review I2: newest-first fitting cut a procedure's example runs once its intent had more runs than the window
    held; every revision keeping its examples was refused while the job said done."""
    cards = [dish_run(learning, n) for n in range(3)]
    asyncio.run(consolidator(learning, fake(proposing(food_procedure(cards))).make).catch_up())
    [first] = learning.items("procedure")
    newer = [dish_run(learning, n) for n in range(3, 12)]
    one = run_tokens(newer[-1])
    rt = fake(proposing(food_procedure(cards + [newer[-1]])),                   # keeps its examples, adds the newest
              limits=_window_for(4 * one + one // 2, "food", (first,)))          # about four runs beside it
    asyncio.run(consolidator(learning, rt.make).catch_up())
    [call] = consolidation_calls(rt)
    sent = [c["id"] for c in context_of(call)["episodes"]]
    assert set(sent) == {c["id"] for c in cards + [newer[-1]]}                  # its examples, then the newest
    assert sent == [newer[-1]["id"], *reversed([c["id"] for c in cards])]       # shown newest first
    [revised] = learning.items("procedure")
    assert revised["id"] != first["id"] and len(revised["body"]["examples"]) == 4
    assert job(learning, "food")[:2] == ("done", None)


def test_a_procedure_whose_example_runs_dont_fit_waits_naming_them(learning) -> None:
    cards = [dish_run(learning, n) for n in range(3)]
    asyncio.run(consolidator(learning, fake(proposing(food_procedure(cards))).make).catch_up())
    [first] = learning.items("procedure")
    new = dish_run(learning, 3)
    rt = fake(limits=_window_for(2 * run_tokens(new), "food", (first,)))         # two runs: not its three examples
    out = asyncio.run(consolidator(learning, rt.make).catch_up())
    assert consolidation_calls(rt) == [] and "example runs" in out["sources"][0]["error"]
    status, _, inputs = job(learning, "food")
    assert status == "failed" and inputs == group([*cards, new])                # waits for its runs to change


def test_a_call_whose_procedures_were_all_refused_says_why_and_waits(learning) -> None:
    """Review I2: a call whose every procedure was refused wrote its job as done, so nothing showed it."""
    cards = [dish_run(learning, n) for n in range(3)]
    stranger = dish_run(learning, 3, intent="other")                            # never supplied with these runs
    rt = fake(proposing(food_procedure(cards + [stranger])))
    asyncio.run(consolidator(learning, rt.make).catch_up())
    status, error, inputs = job(learning, "food")
    assert status == "failed" and "procedure cites evidence outside its supplied context" in str(error)
    assert inputs == group(cards) and learning.items("procedure") == []
    asyncio.run(consolidator(learning, rt.make).catch_up())                     # waits: no call each pass
    assert len(consolidation_calls(rt)) == 1
    asyncio.run(consolidator(learning, rt.make).catch_up(force=True))
    assert len(consolidation_calls(rt)) == 2


def test_an_example_run_read_again_is_named_by_its_current_card(learning) -> None:
    """A card read again (a verdict, a new card composer) has a new id: the procedure is shown citing the run's current
    card, which goes first, so a revision keeping its examples passes."""
    cards = [dish_run(learning, n) for n in range(3)]
    asyncio.run(consolidator(learning, fake(proposing(food_procedure(cards))).make).catch_up())
    again = learning.put("episode", "d0", {**cards[0]["body"], "feedback": [{"verdict": "pass"}]}, [],
                         origin_label="ferb", made_by="code:evidence-card/2", state="active")
    label_card(learning, again, "food")
    rt = fake(proposing(food_procedure([again, *cards[1:]])))
    asyncio.run(consolidator(learning, rt.make).catch_up())
    [call] = consolidation_calls(rt)
    [shown] = context_of(call)["existing_procedures"]
    assert [e["episode"] for e in shown["body"]["examples"]] == [again["id"], cards[1]["id"], cards[2]["id"]]
    [revised] = learning.items("procedure")
    assert [e["episode"] for e in revised["body"]["examples"]] == [again["id"], cards[1]["id"], cards[2]["id"]]


def test_the_intents_lessons_go_with_its_next_call_and_a_revised_lesson_keeps_the_owners_counts(learning) -> None:
    """Review I1 (spec 3): a call carries the intent's current lessons, so the model revises them instead of deriving
    them again from the same failed run; a revision (the same scope and trigger) keeps the owner's helpful/harmful
    counts. Another intent's lesson stays out."""
    done, failed = runs(learning, "s", "x"), card(learning, "sf", intent="x", verified=False)
    scope = {"app": "", "site": "shop.example", "intent": "x"}

    def lesson(text: str) -> dict[str, Any]:
        return {**EMPTY, "lessons": [{"scope": scope, "trigger": "the cart is empty", "text": text,
                                      "failure": failed["id"], "success": done[0]["id"]}]}
    asyncio.run(consolidator(learning, fake(lesson("reload the cart first")).make).catch_up())
    [first] = learning.items("lesson")
    rated = learning.lesson_feedback(first["id"], True)                         # cites the lesson, not the runs
    yf, ys = card(learning, "yf", intent="y", verified=False), card(learning, "ys", intent="y")
    elsewhere = learning.put("lesson", "elsewhere", {"text": "another kind of task", "helpful": 0, "harmful": 0},
                             [yf["id"], ys["id"]], made_by="model:t")
    card(learning, "s3", intent="x")                                            # a new run
    rt = fake(lesson("reload the cart, then wait for it"))
    asyncio.run(consolidator(learning, rt.make).catch_up())
    [call] = consolidation_calls(rt)
    assert [n["id"] for n in context_of(call)["existing_lessons"]] == [rated["id"]] and elsewhere["id"] not in call
    revised = next(n for n in learning.items("lesson") if n["key"] == first["key"])
    assert revised["body"]["text"] == "reload the cart, then wait for it"
    assert (revised["body"]["helpful"], revised["body"]["harmful"]) == (1, 0)


def test_a_forgotten_procedure_is_never_learned_again_and_the_call_says_so(learning) -> None:
    """Review M2: the owner forgot the intent's procedure; relearning it from the same runs would undo the forget. The
    call says so (`procedure_forgotten`), and a procedure proposed anyway is refused, visibly."""
    cards = [dish_run(learning, n) for n in range(3)]
    asyncio.run(consolidator(learning, fake(proposing(food_procedure(cards))).make).catch_up())
    [procedure] = learning.items("procedure")
    learning.forget([procedure["id"]])
    dish_run(learning, 3)
    rt = fake(proposing(food_procedure(cards)))
    asyncio.run(consolidator(learning, rt.make).catch_up())
    shown = context_of(consolidation_calls(rt)[0])
    assert shown["procedure_forgotten"] is True and shown["existing_procedures"] == []
    assert "procedure_rejected" not in shown
    assert learning.items("procedure") == [] and "forgotten" in str(job(learning, "food")[1])


def test_a_run_forgotten_or_read_again_during_another_intents_call_is_not_sent(learning) -> None:
    """Review M3: the groups are read once a pass; a run forgotten (or read again) while another intent's call ran was
    still sent to the model."""
    first, second = runs(learning, "a", "first"), runs(learning, "b", "second")

    def answer(ctx: str) -> Any:
        if first[0]["id"] in ctx:                      # meanwhile, the owner forgets one run and a verdict re-reads one
            learning.forget([second[0]["id"]])
            learning.put("episode", "b1", {**second[1]["body"], "feedback": [{"verdict": "pass"}]}, [],
                         origin_label="ferb", made_by="code:evidence-card/2", state="active")
        return EMPTY
    rt = fake(answer)
    asyncio.run(consolidator(learning, rt.make).catch_up())
    calls = consolidation_calls(rt)
    assert len(calls) == 2 and second[2]["id"] in calls[1]
    assert second[0]["id"] not in calls[1] and second[1]["id"] not in calls[1]


def test_unlabelled_cards_and_stale_labels_stay_out_of_a_call(learning) -> None:
    labelled = runs(learning, "a", "a test intent")
    unlabelled = learning.put("episode", "u", {**labelled[0]["body"], "task_id": "u"}, [], origin_label="ferb",
                              made_by="code:evidence-card/2", state="active")
    old = card(learning, "s", intent="a test intent")
    moved = learning.put("episode", "s", {**old["body"], "feedback": [{"verdict": "pass"}]}, [], origin_label="ferb",
                         made_by="code:evidence-card/2", state="active")
    label_card(learning, moved, "elsewhere")              # read again and labelled anew: its old label is stale
    refused = {"intent_id": "", "name": "", "description": "", "slots": [], "values": []}   # its label is refused
    rt = SchemaAnswers({id(INTENT_SCHEMA): lambda ctx: refused, id(PROPOSAL_SCHEMA): lambda ctx: EMPTY})
    asyncio.run(consolidator(learning, rt.make).catch_up())
    [call] = [ctx for schema, ctx in rt.calls if schema == id(PROPOSAL_SCHEMA)]
    assert all(c["id"] in call for c in labelled)
    assert unlabelled["id"] not in call and old["id"] not in call and moved["id"] not in call


def test_a_runtime_that_cannot_be_made_is_tried_again_on_the_next_pass(learning) -> None:
    cards = runs(learning, "a", "a test intent")
    tried: list[Any] = []

    def make(role: str, **k: Any) -> Any:
        tried.append(k["schema"])
        raise RuntimeError("no learning runtime is configured")
    out = asyncio.run(consolidator(learning, make).catch_up())
    assert "no learning runtime is configured" in out["sources"][0]["error"]
    status, _, inputs = job(learning, "a test intent")
    assert status == "failed" and inputs == group(cards) + ":retry"
    asyncio.run(consolidator(learning, make).catch_up())
    assert tried == [PROPOSAL_SCHEMA, PROPOSAL_SCHEMA]


def test_only_runs_without_an_annotation_are_asked_for_one(learning) -> None:
    """Review M5: every call re-annotated every run it carried."""
    cards = runs(learning, "a", "a test intent")

    def annotating(reflection: str) -> Any:
        return lambda ctx: {**EMPTY, "episodes": [{"episode": e["id"], "intent_signature": "s", "slots": [],
                                                   "reflection": reflection, "narrative": "n"}
                                                  for e in context_of(ctx)["episodes"]]}
    asyncio.run(consolidator(learning, fake(annotating("first")).make).catch_up())
    assert len(learning.items("episode_annotation")) == 3
    new = card(learning, "a3", intent="a test intent")
    rt = fake(annotating("again"))
    asyncio.run(consolidator(learning, rt.make).catch_up())
    shown = context_of(consolidation_calls(rt)[0])
    assert [e["id"] for e in shown["episodes"] if e.get("annotate")] == [new["id"]]
    assert len(learning.items("episode_annotation", history=True)) == 4          # the others were not rewritten
    assert {n["key"] for n in learning.items("episode_annotation")} == {c["id"] for c in [*cards, new]}


def test_lessons_the_owner_turned_down_or_wrote_are_never_proposed_again(learning) -> None:
    """A lesson of this intent's runs the owner turned down goes with the call as a refusal; one the owner edited is
    shown as the owner's, never to be revised or restated. Another intent's turned-down lesson stays out."""
    done, failed = runs(learning, "s", "x"), card(learning, "sf", intent="x", verified=False)
    scope = {"app": "", "site": "shop.example", "intent": "x"}
    learning.put("lesson", encoded(scope) + ":the cart is empty", {
        "scope": scope, "trigger": "the cart is empty", "text": "never reload", "failure": failed["id"],
        "success": done[0]["id"], "helpful": 0, "harmful": 0}, [failed["id"], done[0]["id"]], made_by="model:t",
        state="retired")
    model = learning.put("lesson", encoded(scope) + ":the page is slow", {
        "scope": scope, "trigger": "the page is slow", "text": "wait", "failure": failed["id"],
        "success": done[1]["id"], "helpful": 0, "harmful": 0}, [failed["id"], done[1]["id"]], made_by="model:t")
    mine = learning.edit(model["id"], "wait for the price to load")
    yf, ys = card(learning, "yf", intent="y", verified=False), card(learning, "ys", intent="y")
    learning.put("lesson", "other", {"text": "another kind of task", "helpful": 0, "harmful": 0},
                 [yf["id"], ys["id"]], made_by="model:t", state="retired")
    rt = fake()
    asyncio.run(consolidator(learning, rt.make).catch_up())
    shown = context_of(consolidation_calls(rt)[0])
    lessons = [r for r in shown["rejected_by_owner"] if "lesson" in r]
    assert lessons == [{"lesson": "never reload", "scope": scope, "trigger": "the cart is empty"}]
    assert [(n["id"], n.get("owner_edited")) for n in shown["existing_lessons"]] == [(mine["id"], True)]
    assert "procedure_rejected" not in shown
