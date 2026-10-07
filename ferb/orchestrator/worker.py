"""Owns one contained model attempt, liveness and trace recording. Must not infer intent, choose a target or claim semantic success from transport facts.

One worker adapter attempt: scoped run token, registered tools, liveness, cancellation and typed report.

No action-count, attempt-count or task-duration cutoff. The orchestrator chooses recovery."""

from __future__ import annotations

import asyncio
import json
from typing import TYPE_CHECKING, Any, Callable

from pydantic import ValidationError

from ferb.clock import context
from ferb.core.context import ContextBlock, assemble_context, count_tokens
from ferb.core.prompts import load_prompt
from ferb.core.interfaces import AgentRuntime
from ferb.core.types import Outcome, RunHandle, Task
from ferb.orchestrator.tree import TaskRecord

if TYPE_CHECKING:
    from ferb.orchestrator.supervisor import _Tree

NO_TOOLS_LOADED = ("Runtime fact: your previous attempt at this exact step never loaded FERB's tools, so it could not "
                   "act or inspect anything. FERB's tools may be deferred: first call tool_search with the query \"ferb\" "
                   "(and with an exact tool name if one you need is still missing), then do the step.")


class ServiceError(ValueError):
    """The model service (Codex / Claude) itself failed: its message is kept and told to the owner plainly."""

    def __init__(self, message: str, runtime: str = "Codex") -> None:
        super().__init__(message)
        self.runtime = runtime


def service_problem(message: str, runtime: str = "Codex") -> str:
    """The owner-facing sentence for a model-service failure."""
    return f"The AI service I use ({runtime}) reported an error: {message}"



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
    _tools_seen: dict[str, list[str]]
    _tools_offered: dict[str, bool]
    _worker_runs: dict[str, tuple[AgentRuntime, RunHandle]]

    _system: Callable[[str], str]
    _approach: Callable[[], str]
    cancel_tree: Callable[["_Tree"], None]
    context_budget_tokens: int


    async def run_worker(self, tree: "_Tree", task: Task, prompt: str, ctx: list[ContextBlock], schema: dict[str, Any],
                         span: str) -> tuple[dict[str, Any] | Outcome, str]:
        """One attempt. The orchestrator, rather than a report-word classifier, chooses recovery. A run whose runtime
        shows FERB's tools were never loaded is a runtime fault, not a judgment: it is re-run once with that fact."""
        result, worker_id = await self._run_worker_once(tree, task, prompt, ctx, schema, span)
        seen = self._tools_seen.pop(worker_id, None)
        offered = self._tools_offered.pop(worker_id, False)
        if (seen is not None and offered and not any("ferb" in name for name in seen)
                and not isinstance(result, Outcome) and not tree.stop):
            s = self.spans.start("runtime.tools_not_loaded", task_id=task.id, parent_id=span,
                                 input={"tools_seen": seen})
            self.spans.finish(s, output={"rerun": True})
            fact = ContextBlock(NO_TOOLS_LOADED, "runtime fact: FERB tools were not loaded", protected=True)
            result, worker_id = await self._run_worker_once(tree, task, prompt, ctx + [fact], schema, span)
            self._tools_seen.pop(worker_id, None)
            self._tools_offered.pop(worker_id, False)
        return result, worker_id

    async def _run_worker_once(self, tree: "_Tree", task: Task, prompt: str, ctx: list[ContextBlock], schema: dict[str, Any],
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
        else:
            self.tasks.set_status(task.id, "running", {})
        self.tasks.set_span(task.id, wspan.id)
        ttl = (task.deadline - started).total_seconds() if task.deadline else float("inf")
        token = self.tokens.issue(task.id, worker_id, task.scopes, task.identity, ttl_s=max(ttl, 60.0))
        tools = [t["name"] for t in self.gateway.tools_for(self.tokens.validate(token))]
        self._tools_offered[worker_id] = bool(tools)
        names = ", ".join(n.replace(".", "_") for n in tools) or "none"
        owner_active, last_event = self.signals()
        if task.role == "verifier":
            ctx = ctx + [ContextBlock(self.browser_context(task.id), "current browser observations", protected=True)]
        head = context.time_block(self.clock.now(), started, task.deadline, owner_active, last_event)
        full_ctx = ctx + [ContextBlock(load_prompt(self._approach() + "/runtime_tools.md", {"tool_names": names}) +
                                       "\n\n" + head, "runtime tools and time facts", protected=True)]
        self.publish("task.started", {"task_id": task.id, "worker_id": worker_id})
        self.activity.start(worker_id)
        failure = None
        try:
            rt = self.make_runtime("worker:" + tree.spec.worker_type, mode="worker", tool_names=tools, schema=schema)
            final = await self._run(rt, task, self._system(prompt), full_ctx, token, wspan.id, tree, worker_id)
            result: dict[str, Any] | Outcome = json.loads(final)
            if not isinstance(result, dict):
                raise ValueError("not an object")
        except RuntimeViolation as e:
            failure = f"{type(e).__name__}: {e}"
            result = Outcome(task.id, "failed", f"worker runtime stopped: {e}", [], [])
        except ServiceError as e:
            failure = f"{e.runtime}: {type(e).__name__}: {e}"
            result = Outcome(task.id, "blocked", service_problem(str(e), e.runtime), [], [])
        except (ValidationError, ValueError, Stalled) as e:
            failure = f"{type(e).__name__}: {e}"
            if worker_id in self._stalled:
                result = Outcome(task.id, "failed", f"stalled: no sign of life for {self.stall_s:g} s (not counting waits on the "
                                                    "owner)", [], [])
            else:
                result = Outcome(task.id, "cancelled" if tree.stop else "failed",
                                 tree.stop or f"worker's report wasn't usable ({type(e).__name__}: {e})", [], [])
        except Exception as e:
            failure = f"{type(e).__name__}: {e}"
            result = Outcome(task.id, "blocked", f"Worker runtime unavailable: {type(e).__name__}: {e}", [], [])
        finally:
            self._stalled.discard(worker_id)
            self.activity.end(worker_id)
            self.tokens.revoke_worker(worker_id)
        self.spans.finish(wspan, output={"result": (result.summary if isinstance(result, Outcome)
                                                    else json.dumps(result))}, error=failure)
        self.publish("task.finished", {"task_id": task.id, "status": result.status if isinstance(result, Outcome)
                                       else str(result.get("status", "done"))})
        return result, worker_id

    async def _run(self, rt: AgentRuntime, task: Task, system: str, ctx: list[ContextBlock], token: str, parent_span: str,
                   tree: "_Tree" | None, worker_id: str) -> str:
        tool_schemas = self.gateway.tools_for(self.tokens.validate(token)) if token else []
        output_schema = getattr(rt, "output_schema", None)
        ctx_text = assemble_context(ctx, budget_tokens=self.context_budget_tokens,
                                    reserved_tokens=count_tokens(system) + count_tokens(json.dumps(tool_schemas)) +
                                                    count_tokens(json.dumps(output_schema)) + 1024)
        self.spans.update_input(parent_span, {"context": ctx_text, "system_prompt": system,
                                             "context_budget_tokens": self.context_budget_tokens})
        run = await rt.start(task, system, ctx_text, token)
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
                    raise ServiceError(str(ev.data.get("message")), rt.name)
                elif ev.kind == "final":
                    if "tools_seen" in ev.data:
                        if worker_id:
                            self._tools_seen[worker_id] = list(ev.data["tools_seen"])
                        observed = self.spans.start("runtime.tools", task_id=task.id,
                                                    parent_id=parent_span, input={"runtime": rt.name})
                        self.spans.finish(observed, output={"tools_seen": ev.data["tools_seen"],
                            "returncode": ev.data.get("returncode"),
                            "meaning": "tool names observed in provider request traces; not proof of invocation or complete deferred inventory"})
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
