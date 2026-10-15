"""Memory review I3 and M5 (fix wave 3): the sleep service's cost grows with what changed, not with history squared.

A pass re-reads only cards whose inputs changed (missing, built by an older composer, the episode rebuilt, a new owner
verdict). Episodes not yet consolidated go to the model grouped by structure (the same tools and apps), one call per
group, split where the model's real window can't hold the group; never one call per episode carrying every episode
that shares a word. A failed job records its error and waits for its inputs to change or for `ferb memory
consolidate`; a failed distill day waits for its evidence to change or `ferb memory distill`. Ported from the review's
probes P5 and P5b, which asserted the bug."""
import asyncio
import json
from datetime import datetime, timezone
from typing import Any

import pytest

from ferb.core.types import ModelLimits
from ferb.memory.consolidate import Consolidator
from ferb.memory.learning import Learning
from ferb.store.db import Database
from tests.conftest import FakeDistillRuntime


class Clock:
    def now(self) -> datetime:
        return datetime(2026, 10, 11, tzinfo=timezone.utc)


class NoMemory:
    def items(self, **_: Any) -> list[Any]:
        return []

    def get(self, _ident: str) -> None:
        return None


@pytest.fixture
def learning(tmp_path):
    db = Database(tmp_path / "memory.db")
    db.migrate()
    store = Learning(db, Clock(), lambda *args: None, lambda s: s)
    yield store
    db.close()


EMPTY = {"procedures": [], "lessons": [], "identity": [], "episodes": []}


def card(store: Learning, ident: str, tool: str = "browser.open", request: str = "open the page and read it") -> Any:
    return store.put("episode", ident, {"task_id": ident, "verified": True, "request": request, "apps": {},
                                        "slots": {}, "status": "done",
                                        "actions": [{"id": ident + "-a", "tool": tool, "args": {"n": ident}}]},
                     [], origin_label="ferb", made_by="code:evidence-card/2", state="active")


def consolidator(store: Learning, rt: Any, **kw: Any) -> Consolidator:
    return Consolidator(store, NoMemory(), lambda *a, **k: rt, **kw)


def shown(rt: FakeDistillRuntime, ident: str) -> int:
    return sum(ident in "\n".join(context) if isinstance(context, (list, tuple)) else ident in str(context)
               for context in rt.contexts)


def test_n_episodes_of_one_shape_are_one_model_call_and_each_card_is_sent_once(learning) -> None:
    """P5: 40 cards whose requests shared "the" made 40 model calls, each carrying all 40 cards (quadratic)."""
    cards = [card(learning, f"t{i}") for i in range(40)]
    rt = FakeDistillRuntime(EMPTY)
    asyncio.run(consolidator(learning, rt).catch_up())
    assert len(rt.contexts) == 1
    assert all(shown(rt, c["id"]) == 1 for c in cards)


def test_episodes_of_different_shapes_never_share_a_call(learning) -> None:
    a, b = card(learning, "a", tool="mail.send"), card(learning, "b", tool="browser.open")
    rt = FakeDistillRuntime(EMPTY)
    asyncio.run(consolidator(learning, rt).catch_up())
    assert len(rt.contexts) == 2 and shown(rt, a["id"]) == shown(rt, b["id"]) == 1


def test_a_group_the_window_cannot_hold_is_split_never_failed(learning, monkeypatch) -> None:
    cards = [card(learning, f"t{i}", request="open the page " + "word " * 300) for i in range(12)]
    rt = FakeDistillRuntime(EMPTY)
    monkeypatch.setattr(FakeDistillRuntime, "limits", ModelLimits("small", 3_500, 1_000, "test double"))
    out = asyncio.run(consolidator(learning, rt).catch_up())
    assert len(rt.contexts) > 1 and all(shown(rt, c["id"]) == 1 for c in cards)
    assert not [r for r in out["sources"] if "error" in r]
    assert {r[0] for r in learning.db.conn.execute("SELECT status FROM learning_jobs")} == {"done"}


def test_a_second_pass_with_nothing_changed_reads_no_card_and_calls_no_model(learning) -> None:
    """P5b: every pass re-captured every card through the card reader (1.2-1.6 s each)."""
    for i in range(5):
        learning.db.conn.execute(
            "INSERT INTO episodes (task_id, request_text, goal, status, answer, conditions_json, steps_json, "
            "problems_json, tools_json, origins, external_read, usage_json, desktop_json, made_by, built_at, ended_at) "
            "VALUES (?, 'r', 'g', 'done', '', '[]', '[]', '[]', '{}', 'ferb', 0, '{}', '{}', 'x', 'now', ?)",
            (f"e{i}", f"2026-10-0{i + 1}"))
    captured: list[str] = []

    async def capture(task_id: str) -> Any:
        captured.append(task_id)
        return learning.capture_episode(task_id)
    rt = FakeDistillRuntime(EMPTY)
    sleep = consolidator(learning, rt, capture=capture)
    asyncio.run(sleep.catch_up())
    assert sorted(captured) == [f"e{i}" for i in range(5)] and len(rt.contexts) == 1
    asyncio.run(sleep.catch_up())
    assert len(captured) == 5 and len(rt.contexts) == 1
    learning.db.conn.execute("INSERT INTO owner_verdicts (id, task_id, verdict, note, at) VALUES "
                             "('v1', 'e2', 'fail', '', 'now')")                 # one card's input changed
    asyncio.run(sleep.catch_up())
    assert captured[5:] == ["e2"]


def test_a_failed_job_waits_for_a_change_or_an_explicit_consolidate(learning) -> None:
    """M5: a failed consolidation was tried again with a full model call every idle hour."""
    card(learning, "a")
    failing = FakeDistillRuntime(RuntimeError("usage limit reached"))
    sleep = consolidator(learning, failing)
    out = asyncio.run(sleep.catch_up())
    assert "usage limit reached" in out["sources"][0]["error"]
    row = learning.db.conn.execute("SELECT status, error FROM learning_jobs").fetchone()
    assert row[0] == "failed" and "usage limit reached" in row[1]
    asyncio.run(sleep.catch_up())                                     # the next idle hour: nothing changed
    assert len(failing.contexts) == 1
    asyncio.run(sleep.catch_up(force=True))                           # `ferb memory consolidate`
    assert len(failing.contexts) == 2
    card(learning, "b")                                               # a new episode of the same shape
    working = FakeDistillRuntime(EMPTY)
    asyncio.run(consolidator(learning, working).catch_up())
    assert len(working.contexts) == 1
    assert {r[0] for r in learning.db.conn.execute("SELECT status FROM learning_jobs")} == {"done"}


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


def test_the_consolidator_is_shown_only_the_groups_procedures(learning) -> None:
    mail = card(learning, "m1", tool="mail.send")
    learning.put("procedure", "open pages", {"intent": "open pages", "steps": [{"tool": "browser.open"}],
                                             "preconditions": {"apps": {}}}, [], made_by="model:t", state="candidate")
    rt = FakeDistillRuntime(EMPTY)
    asyncio.run(consolidator(learning, rt).catch_up())
    assert shown(rt, mail["id"]) == 1 and "open pages" not in json.dumps(rt.contexts)
