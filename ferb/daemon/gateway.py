"""Tool gateway (section 7 step 6): run token -> identity + scope -> policy (intent / exposure / dataflow /
autonomy / approvals / undo) -> ledger intent -> capability -> sanitize -> span -> result + time footer.

Every model action reaches the Mac only through here (rule 12). Failures are values: the worker always gets a
result, never an exception.
"""

import asyncio
import base64
import dataclasses
from dataclasses import dataclass
from typing import Any, Callable

from pydantic import ValidationError

from ferb.act.fs.files import normalize_path
from ferb.act.locks import ResourceLocks
from ferb.policy.taint import Taint
from ferb.clock import context
from ferb.core.interfaces import Capability, Clock
from ferb.core.types import ToolContext
from ferb.daemon.auth import RunGrant
from ferb.orchestrator.ledger import DuplicateUnresolved, Ledger
from ferb.orchestrator.tree import TaskTree
from ferb.policy.engine import PolicyEngine, Verdict
from ferb.policy.exposure import LOST_WHEN_EXPOSED
from ferb.policy.rules import REVERSIBLE, ActionRequest, payload_hash
from ferb.store.spans import SpanStore

Publish = Callable[[str, dict[str, Any]], None]
Redact = Callable[[Any], Any]


@dataclass
class FsRoots:
    workspace: str
    owner_roots: tuple[str, ...]
    untrusted_roots: tuple[str, ...]


class Observer:
    """What the orchestrator watches through the gateway: tool calls (budget, liveness) and waits on the owner
    (which pause the stall timer). No-ops until the supervisor installs its own."""

    def tool_call(self, grant: RunGrant, done: bool) -> None:
        pass

    def waiting(self, worker: str, on: bool) -> None:
        pass


class Gateway:
    def __init__(self, capabilities: dict[str, Capability], engine: PolicyEngine, ledger: Ledger, spans: SpanStore,
                 tasks: TaskTree, clock: Clock, redact: Redact, publish: Publish, roots: FsRoots,
                 device_id: str, hold_poll_s: float = 0.2) -> None:
        self.caps, self.engine, self.ledger, self.spans = capabilities, engine, ledger, spans
        self.tasks, self.clock, self.redact, self.publish = tasks, clock, redact, publish
        self.roots, self.device_id, self.hold_poll_s = roots, device_id, hold_poll_s
        self.locks = ResourceLocks()
        self.taint = Taint()
        self.observer: Observer = Observer()
        self.on_path: Callable[[str, str], None] = lambda path, task_id: None   # trail attribution (build step 10)

    def tools_for(self, grant: RunGrant) -> list[dict[str, Any]]:
        """What this worker may call: scopes granted, identity allowed, and not lost to exposure."""
        exposed = self.engine.exposure.state(grant.worker_id).exposed
        out = []
        for cap in self.caps.values():
            if not cap.scopes <= grant.scopes or grant.identity not in cap.identities:
                continue
            if exposed and cap.scopes & LOST_WHEN_EXPOSED:
                continue
            out.append({"name": cap.name, "description": cap.description,
                        "input_schema": cap.Input.model_json_schema()})
        return out

    async def call(self, grant: RunGrant, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        rec = self.tasks.get(grant.task_id)
        started = rec.started_at if rec else None
        deadline = rec.task.deadline if rec else None

        def reply(content: dict[str, Any], is_error: bool) -> dict[str, Any]:
            return {"content": self.redact(content), "is_error": is_error,
                    "footer": context.footer(self.clock.now(), started, deadline)}

        cap = self.caps.get(name)
        if cap is None or name not in {t["name"] for t in self.tools_for(grant)}:
            return reply({"error": f"tool {name} is not available to this worker"}, True)
        if rec is None:
            return reply({"error": "task is not running"}, True)
        try:
            args = cap.Input.model_validate(arguments)
        except ValidationError as e:
            return reply({"error": "invalid arguments", "details": e.errors(include_url=False)}, True)

        spec = cap.describe(args)
        if rec.task.role == "reader" and spec.kind not in REVERSIBLE:      # readers read; they never act (7.4)
            return reply({"error": f"blocked by FERB policy: a reader subtask can't {spec.kind}"}, True)
        labels = self.taint.labels_for(rec.tree_id, spec.content)          # owner data inside this content (10.2)
        if labels:
            spec = dataclasses.replace(spec, data_labels=spec.data_labels | labels)
        if spec.target.path is not None:                 # the same path form the intent was recorded in
            spec = dataclasses.replace(spec, target=dataclasses.replace(
                spec.target, path=normalize_path(spec.target.path, self.roots.workspace)))
        action = ActionRequest(capability=name, kind=spec.kind, identity=grant.identity, scopes=frozenset(cap.scopes),
                               task_id=grant.task_id, worker_id=grant.worker_id, request_id=rec.task.request_id,
                               task_scopes=grant.scopes, target=spec.target, content=spec.content,
                               data_labels=spec.data_labels, unverified_account=spec.unverified_account)
        span = self.spans.start("tool.call", task_id=grant.task_id, parent_id=rec.span_id, device_id=self.device_id,
                                input={"tool": name, "args": arguments})
        self.publish("tool.called", {"tool": name, "task_id": grant.task_id, "worker_id": grant.worker_id})

        verdict = self.engine.evaluate(action)
        self._events(verdict)
        if verdict.decision == "block":
            self.spans.finish(span, error=f"blocked ({verdict.step}): {verdict.reason}")
            return reply({"error": f"blocked by FERB policy: {verdict.reason}"}, True)
        if verdict.decision == "ask":
            self.spans.finish(span, output={"needs_approval": verdict.approval_id})
            return reply({"needs_owner_approval": verdict.approval_id, "reason": verdict.reason,
                          "next": "The owner has been asked in `ferb chat`. Retry the exact same call after they "
                                  "approve; don't change it."}, True)
        if verdict.decision == "hold":
            self.observer.waiting(grant.worker_id, True)                  # the undo window is the owner's time
            try:
                state = await self._wait_hold(verdict.hold_id or "")
            finally:
                self.observer.waiting(grant.worker_id, False)
            if state != "released":
                self.spans.finish(span, error="cancelled by the owner during the undo window")
                return reply({"error": "cancelled by the owner (undo)"}, True)

        lock_keys = cap.locks(args) if hasattr(cap, "locks") else []
        self.observer.tool_call(grant, done=False)
        try:
            async with self.locks.hold(lock_keys, grant.worker_id):        # after policy, never across model calls
                return await self._execute(cap, name, args, spec, action, grant, rec, span, reply)
        finally:
            self.observer.tool_call(grant, done=True)

    async def _execute(self, cap: Capability, name: str, args: Any, spec: Any, action: ActionRequest,
                       grant: RunGrant, rec: Any, span: Any, reply: Callable[[dict[str, Any], bool], dict[str, Any]]) \
            -> dict[str, Any]:
        intent_id = None
        if spec.kind not in REVERSIBLE:                  # consequential: ledger row BEFORE it runs
            try:
                intent_id = self.ledger.intend(grant.task_id, name, _target_dict(spec.target), payload_hash(action)).id
                self.ledger.mark(intent_id, "executing")
            except DuplicateUnresolved as e:
                self.spans.finish(span, error=str(e))
                return reply({"error": str(e)}, True)

        ctx = ToolContext(task_id=grant.task_id, worker_id=grant.worker_id, request_id=rec.task.request_id,
                          identity=grant.identity, scopes=grant.scopes, device_id=self.device_id, span_id=span.id,
                          workspace=self.roots.workspace, owner_roots=self.roots.owner_roots,
                          untrusted_roots=self.roots.untrusted_roots, intent_id=intent_id)
        if spec.target.path is not None:
            self.on_path(spec.target.path, grant.task_id)              # file events it causes are FERB's
        try:
            result = await cap.run(args, ctx)
        except Exception as e:                           # outcome unknown: the ledger reconciles, never retries blindly
            if intent_id:
                self.ledger.mark(intent_id, "unknown", [f"exception: {type(e).__name__}"])
            self.spans.finish(span, error=f"{type(e).__name__}: {e}")
            return reply({"error": f"{name} failed ({type(e).__name__}); outcome unknown"}, True)
        if intent_id:
            self.ledger.mark(intent_id, "failed" if result.is_error else "done", [span.id])
        for kind, source_id in result.read_sources:
            self.taint.read(grant.worker_id, kind, source_id)
            if self.engine.exposure.read(grant.worker_id, kind, source_id):
                self.publish("worker.exposed", {"worker_id": grant.worker_id, "task_id": grant.task_id,
                                                "reason": f"read {kind}: {source_id}"})
        content = self.redact(result.content)
        self.spans.finish(span, output=content, error=content.get("error") if result.is_error else None)
        out = reply(content, result.is_error)
        if result.image is not None and not result.is_error:     # to the model only: not in the span, journal or logs
            out["image"] = {"mime": "image/jpeg", "data": base64.b64encode(result.image).decode()}
        return out

    async def _wait_hold(self, hold_id: str) -> str:
        while True:
            h = self.engine.holds.get(hold_id)
            if h is None or h.state in ("released", "undone"):
                return h.state if h else "undone"
            await asyncio.sleep(self.hold_poll_s)

    def _events(self, v: Verdict) -> None:
        for topic, data in v.events:
            self.publish(topic, data)


def _target_dict(t: Any) -> dict[str, Any]:
    return {k: v for k, v in {"account": t.account, "site": t.site, "recipients": list(t.recipients),
                              "path": t.path, "amount": t.amount}.items() if v not in (None, [], ())}
