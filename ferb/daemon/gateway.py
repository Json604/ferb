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
    """What the orchestrator watches through the gateway: tool calls (runaway guards, liveness) and waits on the owner
    (which pause the stall timer). No-ops until the supervisor installs its own."""

    def tool_call(self, grant: RunGrant, done: bool) -> None:
        pass

    def result(self, grant: RunGrant, tool: str, args: dict[str, Any], failed: bool) -> None:
        """Every answered call (runaway guards: the same failing action, loops)."""

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
        self.jev: Any = None                             # decide.ShadowDecider (build step 13): stricter only

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
        r = await self._call(grant, name, arguments)
        content = r.get("content") or {}
        waiting_on_owner = isinstance(content, dict) and "needs_owner_approval" in content   # not a failure
        self.observer.result(grant, name, arguments, bool(r.get("is_error")) and not waiting_on_owner)
        return r

    async def _call(self, grant: RunGrant, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
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
        if self.jev is not None and spec.kind not in REVERSIBLE:          # Jev: only ever stricter (section 12)
            stricter = await self._jev_action(cap, name, spec, action, rec, span.id)
            if stricter is not None:
                if verdict.decision == "hold":
                    self.engine.undo(verdict.hold_id or "")
                kind, why = stricter
                if kind == "block":
                    self.spans.finish(span, error=f"blocked (jev): {why}")
                    self.publish("policy.blocked", {"step": "jev", "reason": why, "capability": name,
                                                    "task_id": grant.task_id, "worker_id": grant.worker_id})
                    return reply({"error": f"blocked by FERB policy: {why}"}, True)
                if self.engine.approvals.consume(action) is None:
                    ap = self.engine.approvals.request(action, why)
                    self.publish("approval.requested", {"approval_id": ap.id, "capability": name, "kind": spec.kind,
                                                        "reason": why, "task_id": grant.task_id})
                    self.spans.finish(span, output={"needs_approval": ap.id})
                    return reply({"needs_owner_approval": ap.id, "reason": why,
                                  "next": "The owner has been asked in `ferb chat`. Retry the exact same call after "
                                          "they approve; don't change it."}, True)
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
        if self.jev is not None:
            stop = await self._jev_result(grant, name, result, content, span.id)
            if stop is not None:
                self.spans.finish(span, output=content, error=stop["error"])
                return reply(stop, True)
        self.spans.finish(span, output=content, error=content.get("error") if result.is_error else None)
        out = reply(content, result.is_error)
        if result.image is not None and not result.is_error:     # to the model only: not in the span, journal or logs
            out["image"] = {"mime": "image/jpeg", "data": base64.b64encode(result.image).decode()}
        return out

    async def _jev_action(self, cap: Capability, name: str, spec: Any, action: ActionRequest, rec: Any,
                          span_id: str) -> tuple[str, str] | None:
        """action.matches_request + action.risk in one request; ("block" | "ask", why) only when Jev is stricter."""
        from ferb.decide.decider import Ask
        req = self.engine.intents.get(action.request_id)
        state = {"request": {"goal": rec.task.goal, "allowed_kinds": sorted(req.kinds) if req else [],
                             "recipients": sorted(req.recipients) if req else [], "sites": sorted(req.sites) if req else [],
                             "paths": sorted(req.paths) if req else []},
                 "action": {"tool": name, "kind": spec.kind, "target": _target_dict(spec.target),
                            "content": (spec.content or "")[:2000]}}
        ref = f"span:{span_id}"
        got = await self.jev.ask(state, [Ask("action.matches_request", "yes", ref),
                                         Ask("action.risk", getattr(cap.risk, "name", "LOW").lower(), ref)])
        if got.get("action.matches_request") == "no":
            self.jev.mark_effect("action.matches_request", ref, "blocked")
            return "block", "a second check says this doesn't match what the owner asked for"
        if got.get("action.matches_request") == "unsure":
            self.jev.mark_effect("action.matches_request", ref, "asked the owner")
            return "ask", "a second check isn't sure this matches what you asked for"
        if got.get("action.risk") == "high":
            self.jev.mark_effect("action.risk", ref, "asked the owner")
            return "ask", "a second check rates this action high-risk"
        return None

    async def _jev_result(self, grant: RunGrant, name: str, result: Any, content: dict[str, Any],
                          span_id: str) -> dict[str, Any] | None:
        """content.injection_suspected (untrusted reads) + browser.login_state (owner snapshots) on one state.
        Returns replacement content when a signed-out page stops the worker."""
        from ferb.decide.decider import Ask
        from ferb.policy.exposure import UNTRUSTED_KINDS
        ref = f"span:{span_id}"
        asks = []
        if any(k in UNTRUSTED_KINDS for k, _ in result.read_sources):
            asks.append(Ask("content.injection_suspected", "no", ref))
        if name == "browser.snapshot":
            asks.append(Ask("browser.login_state", "unknown", ref))
        if not asks or result.is_error:
            return None
        got = await self.jev.ask({"tool": name, "content": content}, asks)
        if got.get("content.injection_suspected") == "yes":
            self.engine.exposure.injection_signal(grant.worker_id, "a second check found instructions in content")
            self.jev.mark_effect("content.injection_suspected", ref, "worker exposed")
            self.publish("worker.exposed", {"worker_id": grant.worker_id, "task_id": grant.task_id,
                                            "reason": "injection signal"})
        if got.get("browser.login_state") == "signed_out":
            self.jev.mark_effect("browser.login_state", ref, "stopped: signed out")
            return {"error": "this page looks signed out; FERB never signs in as the owner — stop and tell the owner"}
        return None

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
