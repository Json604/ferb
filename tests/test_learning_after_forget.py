"""Memory review C1 (fix wave 3): learning keeps going after the 30-day trail purge and after a forget.

A card cites its task's direct sources only (its request entry, its own tool results and evidence, the procedure it
used), never every conversation entry its planner was shown. A source a forget deleted leaves a tombstone (its id,
never its content): a card built on one is never written or rebuilt, and nothing else is touched. A source retention
deleted is kept as an edge with labels of unknown content. Ported from the review's probes P6, P6b, P8 and P9, which
asserted the bug."""
import json
from typing import Any

from tests.conftest import RunningDaemon, chat, done, install, outcome, sup, task


def _on_loop(d: RunningDaemon, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


def _episode_rows(d: RunningDaemon) -> list[tuple[str, str]]:
    return _on_loop(d, lambda: [(r[0], r[1]) for r in d.daemon.db.conn.execute(
        "SELECT kind, data_json FROM journal WHERE kind IN ('memory.episode_failed', 'memory.episode_hook_failed')")])


def _card(d: RunningDaemon, task_id: str) -> dict[str, Any] | None:
    return _on_loop(d, lambda: next((n for n in d.daemon.learning.items("episode") if n["key"] == task_id), None))


def _run_task(d: RunningDaemon, words: str, answer: str = "ok", session: str = "s1") -> str:
    install(d, [sup("task", "Checking.", task(words))], {"worker": done(answer)})
    task_id = outcome(chat(d, words, session=session))["task_id"]
    d.run(d.daemon.supervisor.finish_episodes())                # cards are written after the outcome (L28)
    return str(task_id)


def _remembered_dentist(d: RunningDaemon) -> Any:
    install(d, [json.dumps({"kind": "reply", "text": "Okay.", "remember": [
        {"kind": "person", "subject": "owner", "key": "dentist", "content": "my dentist is Dr. Rao",
         "quote": "my dentist is Dr. Rao"}]})])
    chat(d, "remember that my dentist is Dr. Rao")
    return _on_loop(d, lambda: [i for i in d.daemon.memory.items("active") if "Rao" in i.content][0])


def test_card_capture_continues_after_a_forget_in_the_same_conversation(daemon: RunningDaemon) -> None:
    """P6: the owner's message and the item are forgotten while FERB's reply that cited them stays in the chat. The
    next task's card is written, and it holds nothing of what was forgotten."""
    item = _remembered_dentist(daemon)
    install(daemon, [json.dumps({"kind": "reply", "text": "Dr. Rao."})])
    chat(daemon, "who is my dentist?")
    plan = _on_loop(daemon, lambda: daemon.daemon.memory.forget(item.id))
    task_id = _run_task(daemon, "check the gate number", "Gate B12.")
    card = _card(daemon, task_id)
    assert card is not None and not _episode_rows(daemon), _episode_rows(daemon)
    gone = {"memory:" + i for i in plan.items} | {"journal:" + j for j in plan.journal}
    assert not gone & set(card["sources"])
    assert "Rao" not in json.dumps(card)


def test_card_capture_continues_after_forgetting_a_memory_a_reply_used(daemon: RunningDaemon) -> None:
    """P6b: a `ferb memory add` item retrieved into one reply turn, then forgotten: later cards are still written."""
    d = daemon.daemon
    src = str(_on_loop(daemon, lambda: d._journal().append("memory.added", {"content": "My dentist is Dr. Rao"}).id))
    item = _on_loop(daemon, lambda: d.memory.propose("person", "owner", "dentist", "My dentist is Dr. Rao",
                                                     "owner_stated", [("journal", src)], 1.0))
    install(daemon, [json.dumps({"kind": "reply", "text": "Dr. Rao."})])
    chat(daemon, "who is my dentist?")
    _on_loop(daemon, lambda: d.memory.forget(item.id))
    task_id = _run_task(daemon, "check the gate number")
    assert _card(daemon, task_id) is not None and not _episode_rows(daemon), _episode_rows(daemon)


def test_one_forget_keeps_the_cards_of_unrelated_tasks(daemon: RunningDaemon) -> None:
    """P8: one fact retrieved into one reply turn, three unrelated tasks after it in the same chat. Forgetting the fact
    used to delete all three cards (their planners saw the reply); now they stay."""
    d = daemon.daemon
    src = str(_on_loop(daemon, lambda: d._journal().append("memory.added", {"content": "My dentist is Dr. Rao"}).id))
    item = _on_loop(daemon, lambda: d.memory.propose("person", "owner", "dentist", "My dentist is Dr. Rao",
                                                     "owner_stated", [("journal", src)], 1.0))
    install(daemon, [json.dumps({"kind": "reply", "text": "Dr. Rao."})])
    chat(daemon, "who is my dentist?")
    tasks = [_run_task(daemon, words) for words in
             ("check the gate number", "check the weather in Pune", "check my train seat")]

    def cards() -> int:
        return int(_on_loop(daemon, lambda: d.db.conn.execute(
            "SELECT COUNT(*) FROM learning_nodes WHERE kind='episode'").fetchone()[0]))
    assert cards() == 3
    _on_loop(daemon, lambda: d.memory.forget(item.id))
    assert cards() == 3
    assert all(_card(daemon, t) is not None for t in tasks)


def test_card_capture_continues_after_the_30_day_trail_purge(daemon: RunningDaemon) -> None:
    """P9: a reply turn cited today's activity (`trail:<id>`); thirty days later retention deletes that row while the
    reply stays in the chat. A later task's card is still written."""
    d = daemon.daemon
    now = d.clock.now().astimezone()
    tid = "trail-notif-1"
    _on_loop(daemon, lambda: d.db.conn.execute(
        "INSERT INTO trail (id, at, day, device_id, actor, source, data_json) VALUES (?,?,?,?,?,?,?)",
        (tid, now.isoformat(), now.date().isoformat(), "dev", "owner", "notification",
         json.dumps({"app": "Messages", "title": "Asha", "body": "dinner at eight tonight"}))))
    _on_loop(daemon, lambda: d.daily_activity.record({"id": tid, "source": "notification", "what": "notification",
                                                      "where": {"app": "Messages", "body": "dinner at eight tonight"}}))
    install(daemon, [json.dumps({"kind": "reply", "text": "Asha said dinner at eight."})])
    chat(daemon, "what did Asha say about dinner tonight?")
    _on_loop(daemon, lambda: d.db.conn.execute("DELETE FROM trail WHERE id=?", (tid,)))   # what the purge does
    task_id = _run_task(daemon, "check the gate number")
    assert _card(daemon, task_id) is not None and not _episode_rows(daemon), _episode_rows(daemon)


# ---------- I5: a procedure id that isn't a current procedure never fails the task ----------

def _plan_with_procedure(d: RunningDaemon, procedure_id: str) -> dict[str, Any]:
    spec = task("check the gate number")
    spec["procedure_id"] = procedure_id
    install(d, [sup("task", "Checking.", spec)], {"worker": done("Gate B12.")})
    return outcome(chat(d, "check the gate number", session="gate"))


def _ignored(d: RunningDaemon) -> list[Any]:
    return _on_loop(d, lambda: [(json.loads(r[0]), json.loads(r[1] or "{}")) for r in d.daemon.db.conn.execute(
        "SELECT input_json, output_json FROM spans WHERE kind='procedure.ignored'")])


def test_a_planner_procedure_id_that_is_an_episode_is_ignored_and_the_task_runs(daemon: RunningDaemon) -> None:
    """Probe P4b: an episode's id as procedure_id failed the whole task ("LearningRefused: not a procedure")."""
    lr = daemon.daemon.learning
    node = _on_loop(daemon, lambda: lr.put("episode", "old-task", {"task_id": "old-task", "verified": True,
                                                                   "actions": []}, [], origin_label="ferb",
                                           made_by="code:evidence-card/2", state="active"))
    final = _plan_with_procedure(daemon, node["id"])
    assert final["status"] == "done"
    assert not _on_loop(daemon, lambda: daemon.daemon.journal.of_kind("task.execution_failed"))
    [(inp, out)] = _ignored(daemon)
    assert inp["procedure_id"] == node["id"] and "not a procedure" in out["why"]


def test_an_unknown_procedure_id_is_ignored_and_the_task_runs(daemon: RunningDaemon) -> None:
    final = _plan_with_procedure(daemon, "m-" + "0" * 64)
    assert final["status"] == "done" and len(_ignored(daemon)) == 1
