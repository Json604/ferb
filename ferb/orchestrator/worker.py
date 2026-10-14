"""Owns one contained model attempt, liveness and trace recording. Must not infer intent, choose a target or claim semantic success from transport facts.

One worker adapter attempt: scoped run token, registered tools, liveness, cancellation and typed report.

No action-count, attempt-count or task-duration cutoff. The orchestrator chooses recovery."""

from __future__ import annotations

import asyncio
import json
from typing import TYPE_CHECKING, Any, Awaitable, Callable

from pydantic import ValidationError

from ferb.clock import context
from ferb.core.context import ContextBlock, assemble_parts, count_tokens
from ferb.core.errors import RuntimeUnavailable
from ferb.core.prompts import load_prompt
from ferb.core.structured import check_final
from ferb.core.interfaces import AgentRuntime
from ferb.core.types import ModelImage, ModelRequest, Outcome, RunHandle, Task
from ferb.orchestrator.tree import TaskRecord
from ferb.orchestrator.steering import deliver as deliver_steering
from ferb.orchestrator import progress

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


class InvalidAnswer(ValueError):
    """The model answered, but its answer doesn't match the requested schema: output validation, not a failed call."""


class Stalled(Exception):
    pass


class WorkerRuns:
    """Needs from the host: make_runtime, gateway, tokens, spans, tasks, publish, journal, clock, ids, device_id,
    activity, on_violation, heartbeat_poll_s, stall_s, apps(), running_apps(), _system(), _active, _tree_of, _stalled,
    _live, cancel_tree()."""
    make_runtime: Any
    gateway: Any
    tokens: Any
    spans: Any
    tasks: Any
    publish: Any
    journal: Any
    clock: Any
    ids: Any
    device_id: str
    activity: Any
    signals: Any
    browser_context: Callable[[str], str]
    apps: Callable[[], list[tuple[str, str]]]
    running_apps: Callable[[], list[tuple[str, str, int]]]
    on_violation: Any
    heartbeat_poll_s: float
    stall_s: float
    _active: dict[str, tuple[AgentRuntime, RunHandle]]
    _tree_of: dict[str, Any]
    _stalled: set[str]
    _tools_seen: dict[str, list[str]]
    _tools_offered: dict[str, bool]
    _worker_runs: dict[str, tuple[AgentRuntime, RunHandle]]
    _live: dict[str, asyncio.Queue[dict[str, Any]]]

    _system: Callable[[str], str]
    _approach: Callable[[], str]
    cancel_tree: Callable[["_Tree"], None]
    context_budget_tokens: int


    def tool_progress(self, grant: Any, name: str, args: dict[str, Any], kind: str, site: str | None) -> None:
        """L8: a worker (or the final checker) started a tool call. Its task's live line says what it is doing, built
        by code from the tool and its target (`progress.describe`), never from the worker's words: a Mac app by the
        name FERB knows it by. A line that can't be built is noted in the journal; the call goes on."""
        tree = self._tree_of.get(grant.task_id)
        live = self._live.get(tree.id) if tree is not None else None
        if tree is None or live is None:        # no running task stream to show it on
            return
        try:
            app = progress.app_name(args, site, self.apps(), self.running_apps) if name.startswith("mac.") else None
            text = self.gateway.redact(progress.describe(name, args, kind=kind, site=site, app=app))
        except Exception as e:
            self.journal.append("progress.failed", {"task_id": grant.task_id, "tool": name, "error": type(e).__name__})
            return
        live.put_nowait({"type": "progress", "task_id": tree.id, "text": text, "started_at": self.clock.now().isoformat()})

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
        # The final checker's token is observe-only: the gateway offers and runs observation tools only (V3, W4).
        token = self.tokens.issue(task.id, worker_id, task.scopes, task.identity, ttl_s=max(ttl, 60.0),
                                  observe_only=task.role == "verifier")
        tools = [t["name"] for t in self.gateway.tools_for(self.tokens.validate(token))]
        self._tools_offered[worker_id] = bool(tools)
        names = ", ".join(n.replace(".", "_") for n in tools) or "none"
        owner_active, last_event = self.signals()
        if task.role == "verifier":
            ctx = ctx + [ContextBlock(self.browser_context(task.id), "current browser observations", protected=True)]
        images: tuple[ModelImage, ...] = ()
        if task.role == "worker":                 # L12: the request's worker starts from what the owner saw at send
            if tree.start_block is not None:
                ctx = ctx + [tree.start_block]
            if tree.start_image is not None:      # its start-of-task screenshot: no second capture (D26 clarified)
                images = (tree.start_image,)
                self.gateway.initial_image_given(task.id)
        head = context.time_block(self.clock.now(), started, task.deadline, owner_active, last_event)
        full_ctx = ctx + [ContextBlock(load_prompt(self._approach() + "/runtime_tools.md", {"tool_names": names}) +
                                       "\n\n" + head, "runtime tools and time facts", protected=True)]
        self.publish("task.started", {"task_id": task.id, "worker_id": worker_id})
        self.activity.start(worker_id)
        failure = None
        try:
            if task.role == 'worker':
                # Runtime construction can itself fail (for example missing subscription login).
                tree.result_revision = len(tree.steering.messages)
            role = "checker" if task.role == "verifier" else "worker:" + tree.spec.worker_type
            rt = self.make_runtime(role, mode="worker", tool_names=tools, schema=schema)
            final = await self._run(rt, task, self._system(prompt), full_ctx, token, wspan.id, tree, worker_id,
                                    images=images)
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
                                                    else json.dumps(result)),
                                         "status": result.status if isinstance(result, Outcome)
                                         else result.get("status")}, error=failure)
        self.publish("task.finished", {"task_id": task.id, "status": result.status if isinstance(result, Outcome)
                                       else str(result.get("status", "done"))})
        return result, worker_id

    async def _run(self, rt: AgentRuntime, task: Task, system: str, ctx: list[ContextBlock], token: str, parent_span: str,
                   tree: "_Tree" | None, worker_id: str, *, on_delta: Callable[[str], None] | None = None,
                   images: tuple[ModelImage, ...] = ()) -> str:
        revision = len(tree.steering.messages) if tree is not None else 0
        if tree is not None and task.role == 'worker':
            # A startup failure is a current-attempt failure, not a stale result to retry forever.
            tree.result_revision = revision
        if tree is not None and revision:
            # Internal adapter retries may reuse a context captured before a correction arrived.
            ctx = ctx + [ContextBlock('\n\n'.join(tree.steering.messages), 'latest owner corrections', protected=True)]
        tool_schemas = self.gateway.tools_for(self.tokens.validate(token)) if token else []
        output_schema = getattr(rt, "output_schema", None)
        parts, cacheable = assemble_parts(ctx, budget_tokens=self.context_budget_tokens,
                                          reserved_tokens=count_tokens(system) + count_tokens(json.dumps(tool_schemas)) +
                                                          count_tokens(json.dumps(output_schema)) + 1024)
        exposure = getattr(self, '_context_exposure', None)
        if exposure is not None:
            exposure(task, ctx, parts, tree)
        request = ModelRequest(system, tuple(parts), cacheable, images)
        self.spans.update_input(parent_span, {"context": request.context, "system_prompt": system,
                                             "context_budget_tokens": self.context_budget_tokens,
                                             # the images' evidence rows, never their bytes (L12)
                                             **({"image_evidence": [i.ref for i in images]} if images else {})})
        start = asyncio.ensure_future(rt.start(task, request, token))
        started: list[RunHandle] = []

        async def stop() -> None:
            if started:
                await rt.cancel(started[0])
            else:
                start.cancel()
        # The stall watchdog covers startup too: a worker runtime has no startup timeout, so an app-server that never
        # answers its account check would otherwise hold the attempt until Stop.
        watchdog = asyncio.create_task(self._watch(stop, worker_id)) if worker_id else None
        try:
            run = await start
        except BaseException as exc:
            if watchdog is not None:
                watchdog.cancel()
            if isinstance(exc, RuntimeUnavailable):
                raise ServiceError(str(exc), rt.name) from None
            current = asyncio.current_task()
            if (isinstance(exc, asyncio.CancelledError) and worker_id in self._stalled
                    and not (current is not None and current.cancelling())):
                raise Stalled() from None              # the watchdog's cancellation, not Stop or shutdown
            raise
        started.append(run)
        self._active[run.id] = (rt, run)
        if worker_id:
            self._worker_runs[worker_id] = (rt, run)
        if tree is not None:
            tree.steering.delivered[run.id] = revision
        final = ""
        try:
            if tree is not None and tree.stop:
                raise ValueError("task stopped while the runtime was starting")
            if tree is not None:
                async with tree.steering.lock:
                    await deliver_steering(self, tree, rt, run)
            async for ev in rt.events(run):
                if worker_id:
                    self.activity.beat(worker_id)
                if ev.kind == "text_delta":
                    if on_delta is not None:
                        on_delta(str(ev.data.get('text', '')))
                elif ev.kind == "usage":
                    s = self.spans.start("model.usage", task_id=task.id if task.id != task.request_id else None,
                                         parent_id=parent_span, input={"runtime": rt.name})
                    self.spans.finish(s, output=dict(ev.data), cost_usd=ev.data.get("cost_usd"))            # traced, never limited
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
            async def close() -> None:
                if tree is not None:
                    applied = tree.steering.delivered.pop(run.id, revision)
                    if task.role == 'worker':
                        tree.result_revision = applied
                self._active.pop(run.id, None)
                self._worker_runs.pop(worker_id, None)
                await rt.cancel(run)
            if tree is not None:
                async with tree.steering.lock:
                    await close()
            else:
                await close()
        if worker_id in self._stalled:
            raise Stalled()
        if not final:
            raise ValueError("runtime finished without an answer")
        try:
            return check_final(final, output_schema)
        except ValueError as e:
            raise InvalidAnswer(str(e)) from None

    async def _watch(self, stop: Callable[[], Awaitable[None]], worker_id: str) -> None:
        """Stop a silent worker (starting or running); owner waits suspend liveness timing."""
        while True:
            await asyncio.sleep(self.heartbeat_poll_s)
            if self.activity.stalled(worker_id, self.stall_s):
                self._stalled.add(worker_id)
                await stop()
                return
