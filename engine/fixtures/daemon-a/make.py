"""Group A's handlers beyond what rust/fixtures/daemon's recordings reach (ENG-3): memory, timeline, chat history and
schedules on the real Python daemon, through the same harness (rust/fixtures/daemon/make.py, imported: its stubs,
FakeClock, SequentialIds, raw socket lines). Each scenario starts a real `Daemon` in a fixed temp home.

Run from the repo root:  uv run python rust/fixtures/daemon-a/make.py

What the recordings add: every memory.learning op, memory.list filters, the core profile's add and remove, a secret
hidden whole by an edit, a forget that keeps its sources, timeline.get's since and limit forms, chat history's paging
and limit refusals, and schedules created, paused, resumed, fired by the fake clock (a run that succeeds, one that
fails and its retry) and booked again after a restart that lost the next run's timer.

The supervisor and the owner-facing side are not group A's: `supervisor.run_scheduled`, `_task_result` and `_notify`
are replaced by recorders (steps `seams`), and the Rust test's stand-ins answer and are compared from them.
"""

import json
import shutil
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "daemon"))

import make as base  # noqa: E402  (installs the stubs; ROOT is its fixed temp folder)

from ferb.orchestrator.schedules import Rule  # noqa: E402


def entry(i: int, kind: str, day: str, minute: int) -> dict[str, Any]:
    return {"id": f"00000000-0000-4000-8000-{i:012d}", "kind": kind, "at": f"{day}T09:{minute:02d}:00.000000+00:00",
            "day": day, "open": kind == "turn", "body": {"text": f"invented entry {i}"}}


def sc_memory_more() -> dict[str, Any]:
    scn = base.Scenario("memory_more", "Every memory.learning op, memory.list filters, the core profile's add and "
                        "remove, an edit hidden whole, a forget keeping sources, timeline.get forms, chat history "
                        "paging.", base.RPC + ["ferb/daemon/areas/memory.py", "ferb/daemon/areas/chat_history.py"])
    c = scn.conn("app")
    cat = scn.call(c, "memory.add", {"content": "The owner's invented cat is called Biscuit.", "kind": "fact"})
    cat_id = cat["result"]["id"]
    scn.call(c, "memory.add", {"content": "The invented team meets on Mondays.", "subject": "work",
                               "key": "team_meeting"})
    tea = scn.call(c, "memory.add", {"content": "The owner prefers tea to coffee.", "kind": "preference"})
    tea_id = tea["result"]["id"]
    base.calls(scn, c,
               ("memory.list", {"subject": "work"}), ("memory.list", {"imported_from": "any"}),
               ("memory.list", {"status": "all", "history": True, "subject": "owner"}),
               ("memory.profile", {"op": "remove", "id": tea_id}), ("memory.profile", {}),
               ("memory.profile", {"op": "add", "id": tea_id, "section": "Preferences"}),
               ("memory.profile", {"op": "add", "id": "no-such-item"}),
               ("memory.learning", {"op": "backlog"}), ("memory.learning", {"op": "guess"}),
               ("memory.learning", {}), ("memory.learning", {"op": "inventory", "history": True}),
               ("memory.learning", {"op": "get", "id": "no-such-node"}),
               ("memory.learning", {"op": "search", "query": "invented cat", "kind": "fact"}),
               ("memory.learning", {"op": "confirm", "id": "no-such-node"}),
               ("memory.learning", {"op": "edit", "id": "no-such-node", "text": "x"}),
               ("memory.learning", {"op": "retire", "id": "no-such-node"}),
               ("memory.learning", {"op": "forget", "id": "no-such-node"}),
               ("memory.learning", {"op": "forget", "id": "no-such-node", "confirm": True}),
               ("memory.learning", {"op": "rollback", "batch": "no-such-batch"}),
               ("memory.learning", {"op": "digest"}), ("memory.learning", {"op": "eval_export"}),
               ("memory.learning", {"op": "embedding"}),
               ("memory.edit", {"id": cat_id, "content": "4111 1111 1111 1111"}),
               ("memory.retract", {"id": "no-such-item"}),
               ("memory.forget", {"id": tea_id, "keep_sources": True}),
               ("memory.forget", {"id": tea_id, "keep_sources": True, "confirm": True}),
               ("memory.restore", {}), ("memory.show", {"subject": "work"}),
               ("timeline.get", {"limit": 3}), ("timeline.get", {"since": "2026-01-01T09:00:00+00:00", "limit": 2}),
               ("timeline.get", {"since": "2026-01-01T09:00:00", "limit": "2"}), ("timeline.get", {"limit": 2.7}),
               ("timeline.get", {"limit": -5}), ("timeline.get", {"limit": "x"}),
               ("chat.history.put", {"entries": [entry(1, "turn", "2026-01-01", 1), entry(2, "reply", "2026-01-01", 2),
                                                 entry(3, "turn", "2026-01-01", 3), entry(4, "notice", "2026-01-02", 4)]}),
               ("chat.history.page", {"day": "2026-01-01", "limit": 2}),
               ("chat.history.page", {"day": "2026-01-01", "limit": 2,
                                      "before": "2026-01-01T09:02:00.000000+00:00|00000000-0000-4000-8000-000000000002"}),
               ("chat.history.page", {"day": "2026-01-01", "limit": "x"}),
               ("chat.history.page", {"day": "2026-01-01", "limit": 0}),
               ("chat.history.page", {"day": "2026-01-01", "limit": 201}),
               ("chat.history.days", {}),
               ("setup.persona", {"status": True}), ("setup.persona", {"refresh": True}))
    scn.close(c)
    return scn.finish()


SEAMS: list[dict[str, Any]] = []
RUNS: list[dict[str, Any]] = []


def install_seams(d: Any) -> None:
    """The supervisor's run and the owner-facing side, replaced by recorders (they are not group A's)."""
    async def run_scheduled(s: Any, late: bool, overdue_s: float = 0, retry: bool = False) -> dict[str, Any]:
        SEAMS.append({"run_scheduled": s.id, "late": late, "overdue_s": overdue_s, "retry": retry})
        return RUNS.pop(0)

    async def task_result(ev: dict[str, Any], shown: bool, alert: bool = False, publish: bool = True) -> None:
        SEAMS.append({"task_result": json.loads(json.dumps(ev)), "shown": shown, "alert": alert})

    async def notify(title: str, text: str, urgent: bool = False, **k: Any) -> None:
        SEAMS.append({"notify": {"title": title, "text": text, "ident": k.get("ident"), "target": k.get("target"),
                                 "card": k.get("card")}})
    d.supervisor.run_scheduled = run_scheduled
    d.supervisor.schedule_running = lambda sid: False
    d._task_result = task_result
    d._notify = notify


def add_schedule(scn: Any, spec: dict[str, Any]) -> str:
    """A schedule written as `_schedule_create` writes one (the store's add, its first run booked), on the loop."""
    from datetime import datetime
    d = scn.host.daemon

    def go() -> str:
        rule = Rule(**spec["rule"])
        s = d.schedules.add(spec["request"], spec["own_words"], spec["targets"], rule,
                            datetime.fromisoformat(spec["first"]), spec["watch"], None, name=spec["name"])
        t = d._book(s, s.next_run)
        d.schedules.set_timer(s.id, t.id, s.next_run)
        return str(s.id)
    sid = base.on_loop(scn.host, go)
    scn.steps.append({"do": "add_schedule", "spec": spec, "id": sid})
    return sid


def seams(scn: Any) -> None:
    scn.settle()
    scn.steps.append({"do": "seams", "calls": json.loads(json.dumps(SEAMS))})
    SEAMS.clear()


def sc_schedules_more() -> dict[str, Any]:
    if True:
        scn = base.Scenario("schedules_more", "Schedules paused, resumed, refused an unknown action, fired by the "
                            "clock (a run that succeeds, one that fails and its retry), and booked again after a "
                            "restart that lost the next run's timer.", base.RPC + ["ferb/daemon/areas/schedules.py"])
        install_seams(scn.host.daemon)
        once = add_schedule(scn, {"request": "Check the invented permit page", "own_words": "check the permit page "
                                  "in 5 minutes", "targets": {}, "rule": {"repeat": "none", "tz": "UTC"},
                                  "first": "2026-01-01T09:05:00+00:00", "watch": "none", "name": "Permit check"})
        daily = add_schedule(scn, {"request": "Water the invented plants", "own_words": "water the plants every "
                                   "morning at 8", "targets": {}, "rule": {"repeat": "daily", "at": "08:00",
                                                                           "tz": "UTC"},
                                   "first": "2026-01-02T08:00:00+00:00", "watch": "none", "name": ""})
        weekly = add_schedule(scn, {"request": "Tidy the invented desk", "own_words": "tidy the desk every friday",
                                    "targets": {}, "rule": {"repeat": "weekly", "at": "17:00", "weekday": 4,
                                                            "tz": "UTC"},
                                    "first": "2026-01-02T17:00:00+00:00", "watch": "none", "name": "Desk"})
        c = scn.conn("app")
        base.calls(scn, c, ("schedules.list", {}), ("schedules.change", {"id": daily, "action": "pause"}),
                   ("schedules.list", {}), ("schedules.change", {"id": daily, "action": "resume"}),
                   ("schedules.change", {"id": weekly, "action": "cancel", "extra": 1}),
                   ("schedules.change", {"id": weekly, "action": "resume"}),
                   ("reminders.list", {}), ("timers.states", {"ids": []}), ("timers.repeat", {}))
        RUNS.append({"task_id": "invented-run-1", "status": "done", "answer": "No open slots yet."})
        scn.advance(300)
        seams(scn)
        base.calls(scn, c, ("schedules.list", {}))
        RUNS.append({"task_id": "invented-run-2", "status": "failed", "answer": "The invented garden was locked.",
                     "cards": [{"type": "note", "text": "kept"}, "not a card"]})
        scn.advance(82500)                                 # to 2026-01-02 08:00
        seams(scn)
        RUNS.append({"task_id": "invented-run-3", "status": "done_unverified", "summary": "Watered, unconfirmed."})
        scn.advance(600)                                   # the failed run's one retry
        seams(scn)
        base.calls(scn, c, ("schedules.list", {}), ("schedules.change", {"id": daily, "action": "stop"}))
        scn.close(c)
        sc = scn.host.daemon

        def lose_timer() -> None:
            sc.db.conn.execute("UPDATE timers SET state = 'cancelled' WHERE id = (SELECT timer_id FROM schedules "
                               "WHERE id = ?)", (daily,))
        base.on_loop(scn.host, lose_timer)
        scn.note("the daily schedule's booked timer was set cancelled (a crash between handling a run and booking "
                 "the next)")
        scn.restart()
        install_seams(scn.host.daemon)
        c = scn.conn("app")
        base.calls(scn, c, ("schedules.list", {}))
        scn.close(c)
        return scn.finish()


def main() -> int:
    scenarios = [sc_memory_more(), sc_schedules_more()]
    text = base.fx(json.dumps({"scenarios": scenarios}, ensure_ascii=False, indent=1)) + "\n"
    (HERE / "transcripts.json").write_text(text, encoding="utf-8")
    # texts of 2,048 characters or more are stored once by SHA-256 (as rust/fixtures/daemon/bodies.json)
    used = {sha: body for sha, body in base.BODIES.items() if sha in text}
    bodies = base.fx(json.dumps(dict(sorted(used.items())), ensure_ascii=False, indent=1)) + "\n"
    (HERE / "bodies.json").write_text(bodies, encoding="utf-8")
    return len(text.encode("utf-8")) + len(bodies.encode("utf-8"))


if __name__ == "__main__":
    try:
        size = main()
    finally:
        base.MP.undo()
        shutil.rmtree(base.ROOT, ignore_errors=True)
    print(f"transcripts.json: {size} bytes")
