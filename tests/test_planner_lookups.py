"""The planner looks things up only when it needs them (ORCH-515), read-only, through FERB's gateway (ORCH-516).

End to end through the real daemon and gateway with the scripted runtime: a scripted planner answer that is a function
gets the turn's run token and calls tools over the daemon's socket, as the MCP shim does."""
import json
from typing import Any

from ferb.orchestrator.supervisor import LOOKUPS, PLANNER_LOOKUPS, PLANNER_TOOLS
from tests.conftest import RunningDaemon, chat, done, install, outcome, sup, task, tool
from tests.support.memory_items import owner_fact


def _planner_calls(script: Any) -> list[tuple[str, Any, list[str]]]:
    """(context, ModelRequest, tool names) of each planner call."""
    requests = [r for _, key, r in script.requests if key == "supervisor"]
    tools = [names for _, key, _, names in script.contexts if key == "supervisor"]
    return [(r.context, r, names) for r, names in zip(requests, tools)]


def _tasks_block(context: str) -> str:
    """The planner's task block ("" when it has none)."""
    if "Layer 6 — tasks:" not in context:
        return ""
    return context.split("Layer 6 — tasks:", 1)[1].split("Layer 6 — open questions", 1)[0]


def _lookup_calls(d: RunningDaemon) -> list[dict[str, Any]]:
    """Every tool call a planner's lookup record made (`<request>.lookups`), oldest first."""
    async def go() -> list[dict[str, Any]]:
        return [dict(r) for r in d.daemon.db.conn.execute(
            "SELECT task_id, json_extract(input_json, '$.tool') AS tool, origin FROM spans WHERE kind = 'tool.call' "
            "AND task_id LIKE ? ORDER BY rowid", ("%" + LOOKUPS,))]
    return d.run(go())                    # type: ignore[no-any-return]


# decision: ORCH-515, ORCH-516
def test_a_hi_turn_looks_nothing_up_and_is_pushed_no_memory_activity_or_ended_task(daemon: RunningDaemon) -> None:
    owner_fact(daemon, "dentist", "my dentist is Dr. Rao")
    install(daemon, [sup("task", "Checking.", task("Check the invoice folder"))], {"worker": done("Checked it.")})
    first = outcome(chat(daemon, "check the invoice folder for my dentist bill"))
    script = install(daemon, [sup("reply", "Hi! What can I do for you?")])
    chat(daemon, "hi")
    [(context, _, tools)] = _planner_calls(script)
    assert tools == sorted(PLANNER_TOOLS)                       # offered, read-only
    assert "Layer 4 —" not in context and "Layer 3 —" not in context     # no retrieved memory, chat or activity pushed
    assert "Dr. Rao" not in context.split("Layer 5 — conversation:")[0]
    assert first["task_id"] not in _tasks_block(context)          # an ended task isn't pushed
    assert _lookup_calls(daemon) == []                           # a "hi" needs no lookup

    async def held() -> tuple[int, Any]:                         # nothing of the lookup turn is left behind
        return (len(daemon.daemon.tokens._grants),
                next((k for k in daemon.daemon.tasks._tasks if k.endswith(LOOKUPS)), None))
    assert daemon.run(held()) == (0, None)


# decision: ORCH-515, ORCH-516, ORCH-517
def test_a_question_about_an_older_task_is_looked_up_and_answered_from_it(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Reading it.", task("Read the newest invoice", "owner"))],
            {"worker": done("The newest invoice is from Acme, total 412 EUR, due 30 Nov.")})
    first = outcome(chat(daemon, "read my newest invoice"))      # done: the task block no longer shows it
    found: dict[str, Any] = {}

    def look(token: str) -> str:
        listed = tool(daemon.sock, token, "chat.history", include_tasks=True)
        assert not listed["is_error"], listed
        ids = [r["id"] for r in listed["content"]["records"] if r["kind"] == "task"]
        found["ids"] = ids
        found["row"] = tool(daemon.sock, token, "chat.history", task_id=first["task_id"])
        listed = tool(daemon.sock, token, "task.evidence", task_id=first["task_id"], kind="worker")
        found["records"] = tool(daemon.sock, token, "task.evidence", task_id=first["task_id"],
                                span_id=listed["content"]["records"][0]["id"])
        found["stranger"] = tool(daemon.sock, token, "task.evidence", task_id="not-a-task-of-this-chat")
        found["act"] = tool(daemon.sock, token, "fs.write", path="/tmp/x", content="x", effect="edit")
        return json.dumps({"kind": "reply", "answer_question": None, "task": None, "control": None, "remember": [],
                           "timers": [], "schedule": None, "schedule_changes": [], "reminder_cancels": [],
                           "forget": [], "conversation_changes": [],
                           "text": "Going by that task's record: Acme, 412 EUR, due 30 Nov."})
    script = install(daemon, [look])
    events = chat(daemon, "what was the total on that invoice again?")
    assert any("412 EUR" in str(e.get("text", "")) for e in events)
    [(context, _, _)] = _planner_calls(script)
    assert first["task_id"] not in _tasks_block(context)          # not pushed: looked up
    assert first["task_id"] in found["ids"]
    assert not found["row"]["is_error"] and "412 EUR" in json.dumps(found["row"]["content"])
    assert not found["records"]["is_error"] and "412 EUR" in json.dumps(found["records"]["content"])
    assert found["stranger"]["is_error"]                         # only this conversation's tasks
    assert found["act"]["is_error"] and not (daemon.daemon.home / "x").exists()   # no acting tool
    assert [c["tool"] for c in _lookup_calls(daemon)] == ["chat.history", "chat.history", "task.evidence",
                                                         "task.evidence", "task.evidence"]
    assert all("Origin: " in json.dumps(r) for r in (found["row"], found["records"]))   # each result labelled


# decision: ORCH-516
def test_a_planner_call_makes_at_most_its_lookups(daemon: RunningDaemon) -> None:
    results: list[dict[str, Any]] = []

    def loop(token: str) -> str:
        for i in range(PLANNER_LOOKUPS + 2):
            results.append(tool(daemon.sock, token, "memory.read", query=f"topic {i}"))
        return json.dumps({"kind": "reply", "answer_question": None, "task": None, "control": None, "remember": [],
                           "timers": [], "schedule": None, "schedule_changes": [], "reminder_cancels": [],
                           "forget": [], "conversation_changes": [], "text": "Nothing found."})
    install(daemon, [loop])
    chat(daemon, "what do you remember about me?")
    assert [r["is_error"] for r in results] == [False] * PLANNER_LOOKUPS + [True, True]
    assert "lookups" in json.dumps(results[-1]["content"])


# decision: ORCH-557, ORCH-516
def test_a_repeated_lookup_is_answered_from_the_first_and_a_spent_turn_gets_a_last_call(daemon: RunningDaemon) -> None:
    """Live 2026-11-08 08:49: memory.read {"query": "model"} ten times, then "the model ended without an answer" and the
    turn failed. A repeat with the same arguments is answered from the first call, said so, without counting; a call
    that spent every lookup and failed gets one last call with no tools and what the lookups found."""
    from ferb.core.types import RuntimeEvent
    seen: list[dict[str, Any]] = []

    def repeat(token: str) -> str:
        seen.append(tool(daemon.sock, token, "memory.read", query="model"))
        seen.append(tool(daemon.sock, token, "memory.read", query="model"))
        for i in range(PLANNER_LOOKUPS):
            tool(daemon.sock, token, "memory.read", query=f"other {i}")
        return "not json: the model ended without an answer"
    script = install(daemon, [repeat, json.dumps({"kind": "reply", "task": None, "text": "You use the model in the "
                                                  "facts."})])
    events = chat(daemon, "what model are you using???")
    assert seen[1]["content"]["same_as_your_earlier_call"] is True
    assert seen[1]["content"]["result"] == seen[0]["content"]
    calls = _planner_calls(script)
    assert len(calls) == 2 and calls[1][2] == []                  # the last call: no tools offered
    assert "your lookups for this message are used up" in calls[1][0]
    assert any(e["type"] == "reply" and "facts" in e["text"] for e in events)
    assert "The AI FERB uses now, from its settings" in calls[0][0]   # the model per role, from records


# decision: ORCH-516
def test_the_intro_preview_and_tool_less_roles_get_no_lookups(daemon: RunningDaemon) -> None:
    from ferb.orchestrator.supervisor import NO_CATALOG_ROLES, TOOLLESS_ROLES
    assert "supervisor.md" not in TOOLLESS_ROLES and "supervisor.md" in NO_CATALOG_ROLES
    script = install(daemon, [sup("reply", "Sure.")])

    async def preview() -> list[dict[str, Any]]:
        return [e async for e in daemon.daemon.supervisor.preview("preview:intro", "tidy my downloads")]
    daemon.run(preview())
    assert [names for _, key, _, names in script.contexts if key == "supervisor"] == [[]]
