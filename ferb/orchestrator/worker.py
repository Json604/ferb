"""One worker adapter attempt: scoped run token, registered tools, liveness, cancellation and typed report.

No action-count, attempt-count or task-duration cutoff. The orchestrator chooses recovery."""

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


class ServiceError(ValueError):
    """The model service (Codex / Claude) itself failed: its message is kept and told to the owner plainly."""


def service_problem(message: str) -> str:
    """The owner-facing sentence for a model-service failure."""
    m = message.lower()
    if any(k in m for k in ("usage limit", "rate limit", "quota", "too many requests", "429", "limit reached")):
        return ("The AI service I use (Codex) says your usage limit is reached, so I couldn't start. It works again "
                "once the limit resets.")
    if any(k in m for k in ("login", "logged in", "unauthorized", "401", "auth", "token expired")):
        return ("The AI service I use (Codex) isn't signed in for me, so I couldn't start. `ferb doctor` shows how "
                "to sign in.")
    if any(k in m for k in ("timed out", "timeout", "connection", "network", "dns", "unreachable")):
        return "I couldn't reach the AI service I use (Codex) — the connection failed. Please try again in a moment."
    return f"The AI service I use (Codex) returned an error, so I couldn't start: {message[:160]}"


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
    signals: Any
    browser_context: Callable[[str], str]
    on_violation: Any
    heartbeat_poll_s: float
    stall_s: float
    _active: dict[str, tuple[AgentRuntime, RunHandle]]
    _tree_of: dict[str, Any]
    _stalled: set[str]
    _worker_runs: dict[str, tuple[AgentRuntime, RunHandle]]

    _system: Callable[[str], str]
    cancel_tree: Callable[["_Tree"], None]


    async def run_worker(self, tree: "_Tree", task: Task, prompt: str, ctx: str, schema: dict[str, Any],
                         span: str) -> tuple[dict[str, Any] | Outcome, str]:
        """One attempt. The orchestrator, rather than a report-word classifier, chooses recovery."""
        return await self._run_worker_once(tree, task, prompt, ctx, schema, span)

    async def _run_worker_once(self, tree: "_Tree", task: Task, prompt: str, ctx: str, schema: dict[str, Any],
                               span: str) -> tuple[dict[str, Any] | Outcome, str]:
        if tree.stop:
            return Outcome(task.id, "cancelled", tree.stop, [], []), ""
        worker_id = "w-" + self.ids.new_id()
        self._tree_of[task.id] = tree
        started = self.clock.now()
        wspan = self.spans.start("worker", task_id=task.id, parent_id=span, device_id=self.device_id,
                                 input={"goal": task.goal, "role": task.role, "scopes": sorted(task.scopes)})
        if self.tasks.get(task.id) is None:
            self.tasks.add(TaskRecord(task, started, wspan.id, tree.id), status="running")
        self.tasks._tasks[task.id].span_id = wspan.id
        ttl = (task.deadline - started).total_seconds() if task.deadline else float("inf")
        token = self.tokens.issue(task.id, worker_id, task.scopes, task.identity, ttl_s=max(ttl, 60.0))
        tools = [t["name"] for t in self.gateway.tools_for(self.tokens.validate(token))]
        names = ", ".join(n.replace(".", "_") for n in tools) or "none"
        from ferb.clock.durations import typical
        usual = typical(self.spans._db, set(task.scopes), task.role)
        owner_active, last_event = self.signals()
        if task.role == "verifier":
            ctx += self.browser_context(task.id)
        head = context.time_block(self.clock.now(), started, task.deadline, owner_active, last_event, typical=usual)
        full_ctx = (f"{ctx}\n\nYour FERB tools (MCP server `ferb`): {names}. If you can't see them, they're behind "
                    f"tool_search: search for \"ferb\" or the tool's name to load them — they are always there."
                    f"\n\n{head}")                                         # time-dependent text last
        self.publish("task.started", {"task_id": task.id, "worker_id": worker_id})
        self.activity.start(worker_id)
        try:
            rt = self.make_runtime("worker:" + tree.spec.worker_type, mode="worker", tool_names=tools, schema=schema)
            final = await self._run(rt, task, self._system(prompt), full_ctx, token, wspan.id, tree, worker_id)
            result: dict[str, Any] | Outcome = json.loads(final)
            if not isinstance(result, dict):
                raise ValueError("not an object")
        except RuntimeViolation as e:
            result = Outcome(task.id, "failed", f"worker runtime stopped: {e}", [], [])
        except ServiceError as e:
            result = Outcome(task.id, "blocked", service_problem(str(e)), [], [])
        except (ValidationError, ValueError, Stalled) as e:
            if worker_id in self._stalled:
                result = Outcome(task.id, "failed", "stalled: no sign of life for 180 s (not counting waits on the "
                                                    "owner)", [], [])
            else:
                result = Outcome(task.id, "cancelled" if tree.stop else "failed",
                                 tree.stop or f"worker's report wasn't usable ({type(e).__name__})", [], [])
        except Exception as e:
            result = Outcome(task.id, "blocked", f"Worker runtime unavailable: {type(e).__name__}: {e}", [], [])
        finally:
            self._stalled.discard(worker_id)
            self.activity.end(worker_id)
            self.tokens.revoke_worker(worker_id)
            self.gateway.taint.forget(worker_id)
        self.spans.finish(wspan, output={"result": (result.summary if isinstance(result, Outcome)
                                                    else json.dumps(result))[:2000]})
        self.publish("task.finished", {"task_id": task.id, "status": result.status if isinstance(result, Outcome)
                                       else str(result.get("status", "done"))})
        return result, worker_id

    async def _run(self, rt: AgentRuntime, task: Task, system: str, ctx: str, token: str, parent_span: str,
                   tree: "_Tree" | None, worker_id: str) -> str:
        run = await rt.start(task, system, ctx, token)
        self._active[run.id] = (rt, run)
        if worker_id:
            self._worker_runs[worker_id] = (rt, run)
        final = ""
        watchdog = asyncio.create_task(self._watch(rt, run, worker_id, tree)) if worker_id else None
        try:
            if tree is not None and tree.stop:
                raise ValueError("task stopped while the runtime was starting")
            async for ev in rt.events(run):
                if worker_id:
                    self.activity.beat(worker_id)
                if ev.kind == "usage":
                    s = self.spans.start("model.usage", task_id=task.id if task.id != task.request_id else None,
                                         parent_id=parent_span, input={"runtime": rt.name})
                    self.spans.finish(s, output=dict(ev.data))            # traced, never limited
                elif ev.kind == "violation":
                    self.on_violation(rt.name, str(ev.data))
                    raise RuntimeViolation(f"{rt.name}: {ev.data}")
                elif ev.kind == "error":
                    raise ServiceError(str(ev.data.get("message")))
                elif ev.kind == "final":
                    final = str(ev.data.get("text", ""))
        finally:
            if watchdog is not None:
                watchdog.cancel()
            self._active.pop(run.id, None)
            self._worker_runs.pop(worker_id, None)
            await rt.cancel(run)
        if worker_id in self._stalled:
            raise Stalled()
        if not final:
            raise ValueError("runtime finished without an answer")
        return final

    async def _watch(self, rt: AgentRuntime, run: RunHandle, worker_id: str, tree: "_Tree" | None) -> None:
        """Cancel a silent worker; owner waits suspend liveness timing."""
        while True:
            await asyncio.sleep(self.heartbeat_poll_s)
            if self.activity.stalled(worker_id, self.stall_s):
                self._stalled.add(worker_id)
                await rt.cancel(run)
                return
