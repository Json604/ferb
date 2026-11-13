"""Fixtures for the supervisor's control and recovery (ENG-3): a stop, a pause or a correction arriving at each await
of a task the Python tests cover, recorded from the real Python daemon. README.md beside this file is the map.

Run from the repo root:  uv run python rust/fixtures/supervisor/make_control.py

It reuses both recorders by import: rust/fixtures/daemon/make.py runs each scenario on a real `ferb.daemon.main.Daemon`
with the test suite's fake model (its stand-ins, its fixed folder `$TMPDIR/ferbdx` written `/FX`, its ids and
counters), and rust/fixtures/supervisor/make.py records the supervisor's composition points while it runs (what other
parts gave the supervisor: each planner call's environment and live facts, each role's instruction inputs, each
context's activity search and desktop observations). Nothing here calls a tool: no file is written, nothing is
launched, no network is used (the scenarios' models answer from the script; the daemon's stand-ins are make.py's).

Output (in this folder):
- `control_scenarios.json`: `{"scenarios": [...], "records": {name: [...]}, "catalogs": {sha: catalog}}`. Each scenario
  has rust/fixtures/daemon/chat_scenarios.json's shape (steps, model_calls, tables); `records` has
  rust/fixtures/supervisor/scenarios.json's.
- `control_bodies.json`: texts of 2,048 characters or more that neither rust/fixtures/daemon/bodies.json nor
  bodies.json here hold.

The awaits, from the Python tests that cover them (tests/test_stop.py, test_execution_runs.py,
test_claude_steering.py, test_questions_persistence.py, test_owner_answers.py):
- the checker's model call: a stop (`task.stop`) and a correction (control continue: the attempt is superseded and the
  next starts from it, ORCH-509);
- a paused task (ORCH-308): a stop by `task.stop`, a stop said in chat, and a pause with new work in one answer
  (ORCH-638);
- the check-in after five attempts (ORCH-307): Yes (five more), a stop while it waits, and the owner's correction while
  it waits (a correction resumes it, the question is set aside);
- a plan step's question asked in place: a stop while it waits (the question closes as superseded);
- a stop of a task that already ended (ORCH-508: "That attempt has already ended.").
"""

import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
_spec = importlib.util.spec_from_file_location("supervisor_make", HERE / "make.py")
assert _spec is not None and _spec.loader is not None
sm = importlib.util.module_from_spec(_spec)
sys.modules["supervisor_make"] = sm
_spec.loader.exec_module(sm)          # imports rust/fixtures/daemon/make.py: the stand-ins, the fixed folder
dm = sm.dm

STALL = dm.STALL
CHAT = dm.CHAT
Model, Scenario, Gate = dm.Model, dm.Scenario, dm.Gate
planner, plan_task, control, worker, checker = dm.planner, dm.plan_task, dm.control, dm.worker, dm.checker
plan, step = dm.plan, dm.step
chat_turn, stalled_task, tree_of, row_status = dm.chat_turn, dm.stalled_task, dm.tree_of, dm.row_status
pending_approvals = dm.pending_approvals

SUP = ["ferb/orchestrator/supervisor.py:2866 Supervisor._control", "ferb/orchestrator/supervisor.py:1760 stop_tree",
       "ferb/orchestrator/supervisor.py:1810 pause_tree", "ferb/orchestrator/supervisor.py:1907 _wait_for_owner",
       "ferb/orchestrator/supervisor.py:6790 _check_in", "ferb/orchestrator/steering.py:221 apply"]


def stop_rpc(scn: Any, tree: str) -> None:
    app = scn.conn("app")
    scn.call(app, "task.stop", {"task_ids": [tree]})
    scn.close(app)


def cc_stop_while_checking() -> dict[str, Any]:
    """tests/test_execution_runs.py:99 (stop mid-task), at the checker's call."""
    m = Model([planner("task", "On it.", plan_task("Count the invented files", name="File count"))],
              {"worker": worker("done", "There are 3 files."), "worker.verify": STALL})
    scn = Scenario("stop_while_checking", "The owner's Stop (task.stop) while the checker's model call runs: the "
                   "call is cancelled and the task ends stopped; what the worker had found is shown, not checked "
                   "(ORCH-530).", CHAT + SUP, m)
    turn, tree = stalled_task(scn, "count my files", key="worker.verify")
    stop_rpc(scn, tree)
    scn.end(turn)
    return scn.finish()


def cc_steer_while_checking() -> dict[str, Any]:
    """ORCH-509 (tests/test_execution_runs.py, test_claude_steering.py): a correction while the check runs keeps the
    finished report for the next attempt, which starts from the correction."""
    m = Model([planner("task", "On it.", plan_task("Write the invented greeting", name="Greeting"))],
              {"worker": [worker("done", "Hello."), worker("done", "Hello there.")]})
    scn = Scenario("steer_while_checking", "A correction (control continue) while the checker's call runs: it isn't "
                   "delivered to the checker; the attempt is superseded once the check ends and the next attempt "
                   "starts from the correction.", CHAT + SUP, m)
    gate = Gate(scn, "worker.verify", checker(True))
    m.script.workers["worker.verify"] = [gate.answer, checker(True)]
    turn, tree = stalled_task(scn, "write a greeting", key="worker.verify")
    m.script.sup.append(planner("reply", "Making it longer.", control=control(tree, "continue")))
    chat_turn(scn, "make it a bit longer")
    gate.open(scn)
    scn.end(turn)
    return scn.finish()


def paused(scn: Any, tree: str, m: Any) -> None:
    m.script.sup.append(planner("reply", "Paused.", control=control(tree, "pause")))
    chat_turn(scn, "hold on")
    scn.wait("the task is paused", lambda: row_status(scn, tree) == "paused")


def cc_stop_while_paused() -> dict[str, Any]:
    """tests/test_execution_runs.py:177 (pause), then the app's Stop."""
    m = Model([planner("task", "On it.", plan_task("Tidy the invented notes", name="Tidy notes"))],
              {"worker": STALL})
    scn = Scenario("stop_while_paused", "A paused task stopped by the app's Stop (task.stop): the pause's wait ends "
                   "and the task ends stopped.", CHAT + SUP, m)
    turn, tree = stalled_task(scn, "tidy my notes")
    paused(scn, tree, m)
    stop_rpc(scn, tree)
    scn.end(turn)
    return scn.finish()


def cc_stop_chat_while_paused() -> dict[str, Any]:
    m = Model([planner("task", "On it.", plan_task("Tidy the invented notes", name="Tidy notes"))],
              {"worker": STALL})
    scn = Scenario("stop_chat_while_paused", "A paused task stopped in chat (control stop): the reply is the task's "
                   "end as code recorded it (ORCH-579).", CHAT + SUP, m)
    turn, tree = stalled_task(scn, "tidy my notes")
    paused(scn, tree, m)
    m.script.sup.append(planner("reply", "", control=control(tree, "stop")))
    chat_turn(scn, "stop it")
    scn.end(turn)
    return scn.finish()


def cc_pause_plus_new_task() -> dict[str, Any]:
    """ORCH-638 with a pause: code's pause line, then the new task's plan, in one event."""
    m = Model([planner("task", "On it.", plan_task("Tidy the invented notes", name="Tidy notes"))],
              {"worker": STALL})
    scn = Scenario("pause_plus_new_task", "One message pauses the running task and starts another (ORCH-638): "
                   "the new task's plan event says the pause first.", CHAT + SUP, m)
    turn, tree = stalled_task(scn, "tidy my notes")
    m.script.sup.append(planner("task", "Checking the weather for Avalon.",
                                plan_task("Check the invented weather for Avalon", name="Avalon weather"),
                                control=control(tree, "pause")))
    m.script.workers["worker"] = [worker("done", "Avalon: sunny, 21C.")]
    m.script.workers["worker.verify"] = checker(True)
    chat_turn(scn, "hold that, check the weather in Avalon first")
    stop_rpc(scn, tree)
    scn.end(turn)
    return scn.finish()


def check_in_model(after: list[str], checks: list[str]) -> Any:
    return Model([planner("task", "On it.", plan_task("Book the invented table", name="Table booking"))],
                 {"worker": [worker("not_done", "", reason="The site errored.") for _ in range(5)] + after,
                  "worker.verify": [checker(None, next="retry", missing="The table isn't booked.",
                                            instructions="Try another way.") for _ in range(5)] + checks})


def cc_check_in_yes() -> dict[str, Any]:
    """tests/test_execution_runs.py:260: Yes at the check-in allows five more attempts."""
    m = check_in_model([worker("done", "Booked for 8pm.")], [checker(True)])
    scn = Scenario("check_in_yes", "Five failed attempts, the check-in, Yes (chat.approve choice 1): the sixth "
                   "attempt runs and the task ends done.", CHAT + SUP, m)
    app = scn.conn("app")
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "book the table"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    ap = pending_approvals(scn)[0].id
    scn.call(app, "chat.approve", {"approval_id": ap, "granted": True, "choice": 1})
    scn.end(turn)
    scn.close(app)
    return scn.finish()


def cc_stop_during_check_in() -> dict[str, Any]:
    m = check_in_model([], [])
    scn = Scenario("stop_during_check_in", "The app's Stop while the check-in waits: the question is closed as "
                   "superseded (never a No) and the task ends stopped.", CHAT + SUP, m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "book the table"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    stop_rpc(scn, tree_of(scn))
    scn.end(turn)
    return scn.finish()


def cc_steer_during_check_in() -> dict[str, Any]:
    """ORCH-538: the owner's correction while the check-in waits resumes it; the check-in is set aside."""
    m = check_in_model([worker("done", "Booked for 8pm by phone.")], [checker(True)])
    scn = Scenario("steer_during_check_in", "A correction in chat while the check-in waits (control continue): the "
                   "wait ends with it, the question is set aside, and the next attempt starts from it.", CHAT + SUP, m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "book the table"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    m.script.sup.append(planner("reply", "Trying by phone.", control=control(tree_of(scn), "continue")))
    chat_turn(scn, "try calling them instead")
    scn.end(turn)
    return scn.finish()


def cc_stop_while_step_asks() -> dict[str, Any]:
    """tests/test_owner_answers.py:284 (a plan's question closed as superseded by a stop)."""
    spec = plan_task("Sort the invented downloads then report", name="Sort downloads",
                     plan=plan(step("sort", "Sort the downloads"),
                               step("report", "Report what was sorted", depends_on=["sort"])))
    m = Model([planner("task", "On it.", spec)],
              {"step.sort": worker("not_done", "", reason="Which folder should I sort?",
                                   question="Which folder should I sort?"),
               "step.sort.verify": checker(None, next="ask", question="Which folder should I sort?")})
    scn = Scenario("stop_while_step_asks", "A plan step's question waits in place; the app's Stop ends the wait, the "
                   "question closes as superseded and the step after it never runs.", CHAT + SUP, m)
    turn = scn.stream(scn.conn("chat"), "chat.send", {"session_id": "s1", "text": "sort my downloads and tell me"})
    scn.wait("an approval is pending", lambda: bool(pending_approvals(scn)))
    scn.settle()
    stop_rpc(scn, tree_of(scn, "step.sort"))
    scn.end(turn)
    return scn.finish()


def cc_stop_after_end() -> dict[str, Any]:
    """ORCH-508: a stop that arrives after the work finished leaves it done."""
    m = Model([planner("task", "On it.", plan_task("Count the invented files", name="File count"))],
              {"worker": worker("done", "There are 3 files."), "worker.verify": checker(True)})
    scn = Scenario("stop_after_end", "A stop said in chat after the task ended: the reply says it already ended and "
                   "the task stays done.", CHAT + SUP, m)
    chat_turn(scn, "count my files", label="chat")
    tree = tree_of(scn)
    m.script.sup.append(planner("reply", "", control=control(tree, "stop")))
    chat_turn(scn, "stop that")
    return scn.finish()


CONTROL_SCENARIOS = [cc_stop_while_checking, cc_steer_while_checking, cc_stop_while_paused, cc_stop_chat_while_paused,
                     cc_pause_plus_new_task, cc_check_in_yes, cc_stop_during_check_in, cc_steer_during_check_in,
                     cc_stop_while_step_asks, cc_stop_after_end]


# What the Rust replay reads of the composition records (tests/support_control): each planner call's environment and
# live facts, each role's instruction inputs, each context's activity search and desktop observations. The rest of
# make.py's records is left out here (scenarios.json holds them for the forward path's scenarios).
KEPT = {"supervise": ("environment", "live_facts"), "system": ("live", "inputs"),
        "context": ("activity", "disturbance")}


def kept_records(records: dict[str, list[dict[str, Any]]]) -> tuple[dict[str, Any], dict[str, Any]]:
    """The records the replay reads, each role's instruction inputs stored once by the SHA-256 of their JSON."""
    inputs: dict[str, Any] = {}
    out: dict[str, Any] = {}
    for name, recs in records.items():
        kept = []
        for r in recs:
            keys = KEPT.get(r["kind"])
            if keys is None:
                continue
            row = {"kind": r["kind"], **{k: r.get(k) for k in keys}}
            if r["kind"] == "system":
                text = json.dumps(row["inputs"], sort_keys=True)
                key = dm.sha(text)
                inputs[key] = row["inputs"]
                row["inputs"] = {"inputs": key}
            kept.append(row)
        out[name] = kept
    return out, inputs


def write(name: str, value: Any) -> int:
    """Compact JSON, keys in the order the recorders wrote them (the files are read by the Rust replay)."""
    text = dm.fx(json.dumps(value, ensure_ascii=False, separators=(",", ":"))) + "\n"
    (HERE / name).write_text(text, encoding="utf-8")
    return len(text.encode("utf-8"))


def main() -> None:
    sm.patch_methods()
    only = set(sys.argv[1:])
    out: list[dict[str, Any]] = []
    records: dict[str, Any] = {}
    for fn in CONTROL_SCENARIOS:
        if only and fn.__name__ not in only:
            continue
        sm.CURRENT["records"] = []
        sm.SENT.clear()
        try:
            got = fn()
        except Exception as e:                          # noqa: BLE001
            import traceback
            traceback.print_exc()
            raise SystemExit(f"{fn.__name__}: {type(e).__name__}: {e}")
        sent = [dm.fx(json.dumps([dm.BODIES[p["sha256"]] if isinstance(p, dict) else p for p in c["parts"]]))
                for c in got.get("model_calls", [])]
        if [dm.fx(json.dumps(parts)) for parts in sm.SENT] != sent:
            raise SystemExit(f"{fn.__name__}: the recorded assemblies aren't the parts the model received")
        out.append(got)
        records[got["name"]] = sm.CURRENT["records"]
        sm.CURRENT["records"] = None
        print(f"{fn.__name__}: {len(got.get('model_calls', []))} model calls, {len(records[got['name']])} records",
              file=sys.stderr)
    if dm.PROBLEMS:
        raise SystemExit("problems:\n" + "\n".join(dm.PROBLEMS))
    kept, inputs = kept_records(records)
    value = {"scenarios": out, "records": sm.interned(kept), "inputs": sm.interned(inputs),
             "catalogs": sm.interned(sm.CATALOGS)}
    size = write("control_scenarios.json", value)
    known: set[str] = set()
    for rel in ("rust/fixtures/daemon/bodies.json", "rust/fixtures/supervisor/bodies.json"):
        known |= set(json.loads((REPO / rel).read_text(encoding="utf-8")))
    bodies = {k: v for k, v in sorted({**dm.BODIES, **sm.BODIES}.items()) if k not in known}
    size_b = write("control_bodies.json", bodies)
    print(f"control_scenarios.json: {size} bytes; control_bodies.json: {size_b} bytes")


if __name__ == "__main__":
    try:
        main()
    finally:
        dm.MP.undo()
        import shutil
        shutil.rmtree(dm.ROOT, ignore_errors=True)
