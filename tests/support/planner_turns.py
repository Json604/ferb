"""Planner turns for daemon-level tests of the planner's own runtime (P20's cached Codex turn, MP2).

`PlannerRoute` sends the planner's chat turn (the orchestrator's SUPERVISOR_SCHEMA call) to the runtime the daemon
builds, so `[runtimes]` config applies, recording each ModelRequest FERB assembled and each final answer; every other
model call (workers, checkers, dispatch) goes to a scripted FakeRuntime, so nothing acts. `planner_answer` and
`planner_task` write a complete typed planner answer, as a model would return it."""
import json
from typing import Any

from ferb.orchestrator.schemas import SUPERVISOR_SCHEMA
from tests.conftest import FakeRuntime, RunningDaemon, Script


def planner_answer(kind: str, text: str, task: dict[str, Any] | None = None) -> str:
    return json.dumps({"kind": kind, "answer_question": None, "task": task, "control": None, "remember": [],
                       "timers": [], "schedule": None, "schedule_changes": [], "reminder_cancels": [], "forget": [],
                       "conversation_changes": [], "text": text})


def planner_task(goal: str, recipients: list[str] | None = None) -> dict[str, Any]:
    check = {"kind": "none", "path": None, "text": None, "url_contains": None, "site": None, "expected": None,
             "tool": None}
    return {"goal": goal, "identity": "owner", "worker_type": "default",
            "targets": {"apps": [], "sites": [], "paths": [], "recipients": recipients or []},
            "success_conditions": [{"text": goal, "owner_text": "", "layer": "outcome", "check": check}],
            "plan": None, "folders": [], "keep_tabs_open": False}


class PlannerRoute:
    def __init__(self, d: RunningDaemon) -> None:
        self.d = d
        # one record per planner turn on its runtime: (the ModelRequest FERB assembled, the runtime, the run it started,
        # or None while starting or when the start raised), so a failed start can't shift the pairing
        self.turns: list[tuple[Any, Any, Any]] = []
        self.finals: list[str] = []            # each final answer that runtime gave

    @property
    def requests(self) -> list[Any]:
        return [request for request, _, _ in self.turns]

    def use(self, script: Script, planner_on_runtime: bool = True) -> None:
        """Route the daemon's model calls: the planner's turn to its own runtime (or, False, to `script.sup` like every
        other call), everything else to `script`."""
        daemon = self.d.daemon

        def make(role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any]) -> Any:
            if not planner_on_runtime or role != "orchestrator" or schema is not SUPERVISOR_SCHEMA:
                return FakeRuntime(script, mode, tool_names)
            runtime = daemon._make_runtime(role, mode=mode, tool_names=tool_names, schema=schema)
            start, events = runtime.start, runtime.events

            async def recorded_start(task: Any, request: Any, run_token: str) -> Any:
                index = len(self.turns)
                self.turns.append((request, runtime, None))
                run = await start(task, request, run_token)
                self.turns[index] = (request, runtime, run)
                return run

            async def recorded_events(run: Any) -> Any:
                async for event in events(run):
                    if event.kind == "final":
                        self.finals.append(str(event.data.get("text", "")))
                    yield event
            runtime.start, runtime.events = recorded_start, recorded_events
            return runtime
        daemon.supervisor.make_runtime = make
