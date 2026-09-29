"""One worker run (section 7 step 5): a run token scoped to the task, `ferb mcp` as its only tool source, a typed
result; heartbeat + budget watched while it runs (a stalled worker — no sign of life for STALL_S, not counting
waits on the owner — is cancelled); usage counted against its tree. Mixed into the Supervisor."""

from __future__ import annotations

import asyncio
import json
from typing import TYPE_CHECKING, Any, Callable

from pydantic import ValidationError

from ferb.clock import context
from ferb.core.interfaces import AgentRuntime
from ferb.core.types import Outcome, RunHandle, Task
from ferb.orchestrator.tree import TaskRecord

if TYPE_CHECKING:
    from ferb.orchestrator.supervisor import _Tree


class RuntimeViolation(Exception):
    pass


class Stalled(Exception):
    pass


class WorkerRuns:
    """Needs from the host: make_runtime, gateway, tokens, spans, tasks, publish, clock, ids, device_id, activity,
    on_violation, heartbeat_poll_s, stall_s, _system(), _active, _tree_of, _stalled, cancel_tree()."""
    make_runtime: Any
    gateway: Any
    tokens: Any
    spans: Any
    tasks: Any
    publish: Any
    clock: Any
    ids: Any
    device_id: str
    activity: Any
    on_violation: Any
    heartbeat_poll_s: float
    stall_s: float
    _active: dict[str, tuple[AgentRuntime, RunHandle]]
    _tree_of: dict[str, Any]
    _stalled: set[str]

    _system: Callable[[str], str]
    cancel_tree: Callable[["_Tree"], None]


    async def run_worker(self, tree: "_Tree", task: Task, prompt: str, ctx: str, schema: dict[str, Any],
                         span: str) -> tuple[dict[str, Any] | Outcome, str]:
        """Run one worker; returns its parsed JSON (or a failed Outcome) and its worker id."""
        worker_id = "w-" + self.ids.new_id()
        if not tree.budget.can_start_worker():
            return Outcome(task.id, "failed", "worker budget used up", [], []), worker_id
        tree.budget.used_workers += 1
        self._tree_of[task.id] = tree
        started = self.clock.now()
        wspan = self.spans.start("worker", task_id=task.id, parent_id=span, device_id=self.device_id,
                                 input={"goal": task.goal, "role": task.role, "scopes": sorted(task.scopes)})
        if self.tasks.get(task.id) is None:
            self.tasks.add(TaskRecord(task, started, wspan.id, tree.id), status="running")
        self.tasks._tasks[task.id].span_id = wspan.id
        ttl = (task.deadline - started).total_seconds() if task.deadline else 900.0
        token = self.tokens.issue(task.id, worker_id, task.scopes, task.identity, ttl_s=max(ttl, 60.0))
        tools = [t["name"] for t in self.gateway.tools_for(self.tokens.validate(token))]
        rt = self.make_runtime("worker", mode="worker", tool_names=tools, schema=schema)
        names = ", ".join(n.replace(".", "_") for n in tools) or "none"
        full_ctx = (f"{context.header(self.clock.now(), started, task.deadline)}\n\n{ctx}\n\n"
                    f"Your FERB tools (MCP server `ferb`): {names}")
        self.publish("task.started", {"task_id": task.id, "worker_id": worker_id})
        self.activity.start(worker_id)
        try:
            final = await self._run(rt, task, self._system(prompt), full_ctx, token, wspan.id, tree, worker_id)
            result: dict[str, Any] | Outcome = json.loads(final)
            if not isinstance(result, dict):
                raise ValueError("not an object")
        except RuntimeViolation as e:
            result = Outcome(task.id, "failed", f"worker runtime stopped: {e}", [], [])
        except (ValidationError, ValueError, Stalled) as e:
            if worker_id in self._stalled:
                result = Outcome(task.id, "failed", "stalled: no sign of life for 180 s (not counting waits on the "
                                                    "owner)", [], [])
            else:
                result = Outcome(task.id, "cancelled" if tree.stop else "failed",
                                 tree.stop or f"worker's report wasn't usable ({type(e).__name__})", [], [])
        finally:
            self._stalled.discard(worker_id)
            self.activity.end(worker_id)
            self.tokens.revoke_worker(worker_id)
        self.spans.finish(wspan, output={"result": (result.summary if isinstance(result, Outcome)
                                                    else json.dumps(result))[:2000]})
        self.publish("task.finished", {"task_id": task.id, "status": result.status if isinstance(result, Outcome)
                                       else str(result.get("status", "done"))})
        return result, worker_id

    async def _run(self, rt: AgentRuntime, task: Task, system: str, ctx: str, token: str, parent_span: str,
                   tree: "_Tree" | None, worker_id: str) -> str:
        run = await rt.start(task, system, ctx, token)
        self._active[run.id] = (rt, run)
        final = ""
        watchdog = asyncio.create_task(self._watch(rt, run, worker_id, tree)) if worker_id else None
        try:
            async for ev in rt.events(run):
                if worker_id:
                    self.activity.beat(worker_id)
                if ev.kind == "usage":
                    s = self.spans.start("model.usage", task_id=task.id if task.id != task.request_id else None,
                                         parent_id=parent_span, input={"runtime": rt.name})
                    self.spans.finish(s, output=dict(ev.data), cost_usd=ev.data.get("cost_usd"))
                    if tree is not None:
                        tree.budget.usage(int(ev.data.get("input_tokens") or 0) + int(ev.data.get("output_tokens") or 0),
                                          float(ev.data.get("cost_usd") or 0.0))
                elif ev.kind == "violation":
                    self.on_violation(rt.name, str(ev.data))
                    raise RuntimeViolation(f"{rt.name}: {ev.data}")
                elif ev.kind == "error":
                    raise ValueError(str(ev.data.get("message")))
                elif ev.kind == "final":
                    final = str(ev.data.get("text", ""))
        finally:
            if watchdog is not None:
                watchdog.cancel()
            self._active.pop(run.id, None)
            await rt.cancel(run)
        if worker_id in self._stalled:
            raise Stalled()
        if not final:
            raise ValueError("runtime finished without an answer")
        return final

    async def _watch(self, rt: AgentRuntime, run: RunHandle, worker_id: str, tree: "_Tree" | None) -> None:
        """Heartbeat + budget while a worker runs: stalled -> cancel it; tree over budget -> stop the tree."""
        while True:
            await asyncio.sleep(self.heartbeat_poll_s)
            if self.activity.stalled(worker_id, self.stall_s):
                self._stalled.add(worker_id)
                await rt.cancel(run)
                return
            over = tree.budget.over(self.clock.monotonic()) if tree is not None else None
            if over and tree is not None and not tree.stop:
                tree.stop = over
                self.cancel_tree(tree)
