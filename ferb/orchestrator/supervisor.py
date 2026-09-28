"""Supervisor (section 7): one chat turn -> reply | clarify | plan; a plan runs one worker (build step 4).

- The owner's pasted/quoted content is marked untrusted before the supervisor sees it (section 10.2).
- The supervisor has no FERB tools; the typed intent comes from its plan at plan time and is recorded write-once.
  Code checks every intent target appears in the owner's own words.
- Continuity: the chat history is re-sent each turn (the Codex supervisor is ephemeral: nothing is written into
  the owner's ~/.codex sessions).
- A worker gets a run token + `ferb mcp` as its only tool source and returns a typed Outcome. The outcome is the
  worker's claim; verification against success conditions comes with build step 8.
"""

import asyncio
from dataclasses import dataclass, field
from datetime import timedelta
from pathlib import Path
from typing import Any, AsyncIterator, Awaitable, Callable

from pydantic import ValidationError

from ferb.act.fs.files import normalize_path
from ferb.clock import context
from ferb.core.interfaces import AgentRuntime, Clock, IdSource
from ferb.core.types import Budget, Identity, Outcome, RunHandle, Task
from ferb.daemon.auth import RunTokens
from ferb.daemon.gateway import Gateway
from ferb.memory.journal import Journal
from ferb.orchestrator.schemas import (KINDS, SUPERVISOR_SCHEMA, WORKER_SCHEMA, Plan, SupervisorOutcome,
                                       WorkerOutcome)
from ferb.orchestrator.tree import TaskRecord, TaskTree
from ferb.policy.engine import PolicyEngine
from ferb.policy.intent import IMPLIES, RequestIntent
from ferb.policy.rules import SCOPES_BY_IDENTITY
from ferb.sanitize.untrusted import mark, targets_in_own_words
from ferb.store.spans import SpanStore

PROMPTS = Path(__file__).parent / "prompts"
HISTORY_TURNS = 20
MAX_DEADLINE_MIN = 120

# role ("orchestrator" | "worker"), mode, tool names, schema -> runtime
MakeRuntime = Callable[..., AgentRuntime]
Publish = Callable[[str, dict[str, Any]], None]


@dataclass
class ChatSession:
    id: str
    history: list[tuple[str, str]] = field(default_factory=list)   # (speaker, text as the supervisor saw it)


class RuntimeViolation(Exception):
    pass


class Supervisor:
    def __init__(self, *, make_runtime: MakeRuntime, gateway: Gateway, engine: PolicyEngine, tokens: RunTokens,
                 tasks: TaskTree, spans: SpanStore, journal: Journal, publish: Publish, clock: Clock, ids: IdSource,
                 device_id: str, workspace: str, on_violation: Callable[[str, str], None],
                 on_task_end: Callable[[str], Awaitable[None]] | None = None,
                 facts: Callable[[], str] = lambda: "") -> None:
        self.make_runtime, self.gateway, self.engine, self.tokens = make_runtime, gateway, engine, tokens
        self.tasks, self.spans, self.journal, self.publish = tasks, spans, journal, publish
        self.clock, self.ids, self.device_id, self.workspace = clock, ids, device_id, workspace
        self.on_violation = on_violation
        self.on_task_end, self.facts = on_task_end, facts
        self.sessions: dict[str, ChatSession] = {}
        self._active: dict[str, tuple[AgentRuntime, RunHandle]] = {}

    def _system(self, name: str) -> str:
        facts = self.facts()
        return (PROMPTS / name).read_text() + (f"\n\nFacts about this FERB:\n{facts}\n" if facts else "")

    def cancel_all(self) -> None:
        """Stop hook (section 10.5): cancel every running supervisor/worker run."""
        for rt, run in list(self._active.values()):
            asyncio.get_event_loop().create_task(rt.cancel(run))

    # ---------- one chat turn ----------

    async def send(self, session_id: str, text: str) -> AsyncIterator[dict[str, Any]]:
        session = self.sessions.setdefault(session_id, ChatSession(session_id))
        m = mark(text)
        request_id = self.ids.new_id()
        self.journal.append("chat.message", {"session": session_id, "request_id": request_id, "text": m.for_model,
                                             "untrusted_parts": len(m.untrusted)})
        try:
            out = await self._supervise(session, m.for_model, request_id)
        except RuntimeViolation as e:
            yield {"type": "error", "text": f"The supervisor runtime was stopped: {e}"}
            return
        except (ValidationError, ValueError) as e:
            yield {"type": "error", "text": f"The supervisor's answer wasn't usable ({type(e).__name__})."}
            return
        session.history.append(("owner", m.for_model))
        if out.kind in ("reply", "clarify") or out.plan is None:
            session.history.append(("ferb", out.text))
            self.journal.append(f"chat.{out.kind}", {"session": session_id, "text": out.text})
            yield {"type": "clarify" if out.kind == "clarify" else "reply", "text": out.text}
            return
        problems = self.validate(out.plan, m.own_words)
        if problems:
            question = "Before I do that: " + "; ".join(problems)
            session.history.append(("ferb", question))
            yield {"type": "clarify", "text": question}
            return
        task = self._task(out.plan, request_id)
        self.engine.intents.record(self._intent(out.plan, request_id))          # write-once, plan time
        yield {"type": "plan", "text": out.text, "task_id": task.id, "goal": task.goal,
               "success_conditions": task.success_conditions}
        result: dict[str, Any] = {}
        async for ev in self.run_worker(task):
            if ev["type"] == "outcome":
                result = ev
            yield ev
        session.history.append(("ferb", f"[task {task.id} {result.get('status')}] {result.get('summary', '')}"))

    async def _supervise(self, session: ChatSession, message: str, request_id: str) -> SupervisorOutcome:
        rt = self.make_runtime("orchestrator", mode="supervisor", tool_names=[], schema=SUPERVISOR_SCHEMA)
        history = "\n".join(f"{'Owner' if who == 'owner' else 'FERB'}: {t}"
                            for who, t in session.history[-2 * HISTORY_TURNS:])
        now = self.clock.now()
        ctx = (f"{context.header(now)}\n\nConversation so far:\n{history or '(none)'}\n\nOwner's new message:\n"
               f"{message}")
        chat_task = Task(request_id, None, "chat turn", self.device_id, Identity.OWNER, set(), "mixed", request_id,
                         [], None, Budget())
        span = self.spans.start("model.turn", task_id=None, input={"role": "supervisor", "request_id": request_id})
        final = await self._run(rt, chat_task, self._system("supervisor.md"), ctx, "", span.id)
        self.spans.finish(span, output={"text": final[:2000]})
        return SupervisorOutcome.model_validate_json(final)

    # ---------- plan -> intent + task ----------

    def validate(self, plan: Plan, own_words: str) -> list[str]:
        problems = []
        unknown_verbs = set(plan.verbs) - set(IMPLIES)
        if unknown_verbs:
            problems.append(f"I don't know how to '{', '.join(sorted(unknown_verbs))}'")
        if set(plan.extra_kinds) - set(KINDS):
            problems.append("the plan names an unknown kind of action")
        identity = Identity(plan.identity)
        extra = set(plan.scopes) - SCOPES_BY_IDENTITY[identity]
        if extra:
            problems.append(f"a {identity.value} task can't have {', '.join(sorted(extra))}")
        available = set().union(*(c.scopes for c in self.gateway.caps.values()))
        if set(plan.scopes) - available:
            problems.append(f"FERB can't do {', '.join(sorted(set(plan.scopes) - available))} yet")
        missing = targets_in_own_words(plan.recipients + plan.sites + plan.accounts + plan.paths, own_words)
        if missing:
            problems.append("please name these yourself (I won't take them from pasted or quoted text): "
                            + ", ".join(missing))
        if not 1 <= plan.deadline_minutes <= MAX_DEADLINE_MIN:
            problems.append(f"the deadline must be 1-{MAX_DEADLINE_MIN} minutes")
        return problems

    def _intent(self, plan: Plan, request_id: str) -> RequestIntent:
        return RequestIntent.from_plan(
            request_id, set(plan.verbs), set(plan.extra_kinds), recipients=frozenset(plan.recipients),
            sites=frozenset(plan.sites), accounts=frozenset(plan.accounts),
            paths=frozenset(normalize_path(p, self.workspace) for p in plan.paths), max_amount=plan.max_amount)

    def _task(self, plan: Plan, request_id: str) -> Task:
        deadline = self.clock.now() + timedelta(minutes=plan.deadline_minutes)
        return Task(self.ids.new_id(), None, plan.goal, self.device_id, Identity(plan.identity), set(plan.scopes),
                    plan.role, request_id, list(plan.success_conditions), deadline,
                    Budget(max_seconds=plan.deadline_minutes * 60.0))

    # ---------- the single worker ----------

    async def run_worker(self, task: Task) -> AsyncIterator[dict[str, Any]]:
        worker_id = "w-" + self.ids.new_id()
        root = self.spans.start("task", task_id=task.id, device_id=self.device_id,
                                input={"goal": task.goal, "identity": task.identity.value,
                                       "scopes": sorted(task.scopes), "success": task.success_conditions})
        started = self.clock.now()
        self.tasks.add(TaskRecord(task, started, root.id))
        self.publish("task.created", {"task_id": task.id, "goal": task.goal})
        ttl = (task.deadline - started).total_seconds() if task.deadline else 900.0
        token = self.tokens.issue(task.id, worker_id, task.scopes, task.identity, ttl_s=ttl)
        grant = self.tokens.validate(token)
        tools = [t["name"] for t in self.gateway.tools_for(grant)]
        rt = self.make_runtime("worker", mode="worker", tool_names=tools, schema=WORKER_SCHEMA)
        ctx = (f"{context.header(self.clock.now(), started, task.deadline)}\n\nTask: {task.goal}\n"
               f"Success conditions:\n" + "\n".join(f"- {c}" for c in task.success_conditions) +
               f"\n\nFERB's workspace (relative paths resolve here): {self.workspace}")
        self.publish("task.started", {"task_id": task.id, "worker_id": worker_id})
        outcome: Outcome
        try:
            final = await self._run(rt, task, self._system("worker.md"), ctx, token, root.id)
            w = WorkerOutcome.model_validate_json(final)
            outcome = Outcome(task.id, w.status, w.summary, w.artifacts, w.evidence)
        except RuntimeViolation as e:
            outcome = Outcome(task.id, "failed", f"worker runtime stopped: {e}", [], [])
        except (ValidationError, ValueError) as e:
            outcome = Outcome(task.id, "failed", f"worker's report wasn't usable ({type(e).__name__})", [], [])
        finally:
            self.tokens.revoke_worker(worker_id)
            if self.on_task_end is not None:
                try:
                    await self.on_task_end(task.id)          # e.g. close the task's browser tabs
                except Exception:
                    pass
        self.spans.finish(root, output={"status": outcome.status, "summary": outcome.summary[:2000]})
        self.journal.append("task.outcome", {"task_id": task.id, "status": outcome.status, "summary": outcome.summary,
                                             "artifacts": outcome.artifacts, "verified": False})
        self.publish("task.finished", {"task_id": task.id, "status": outcome.status})
        self.tasks.remove(task.id)
        yield {"type": "outcome", "task_id": task.id, "status": outcome.status, "summary": outcome.summary,
               "artifacts": outcome.artifacts, "evidence": outcome.evidence, "verified": False}

    async def _run(self, rt: AgentRuntime, task: Task, system: str, ctx: str, token: str, parent_span: str) -> str:
        run = await rt.start(task, system, ctx, token)
        self._active[run.id] = (rt, run)
        final = ""
        try:
            async for ev in rt.events(run):
                if ev.kind == "usage":
                    s = self.spans.start("model.usage", task_id=task.id if task.id != task.request_id else None,
                                         parent_id=parent_span, input={"runtime": rt.name})
                    self.spans.finish(s, output=dict(ev.data), cost_usd=ev.data.get("cost_usd"))
                elif ev.kind == "violation":
                    self.on_violation(rt.name, str(ev.data))
                    raise RuntimeViolation(f"{rt.name}: {ev.data}")
                elif ev.kind == "error":
                    raise ValueError(str(ev.data.get("message")))
                elif ev.kind == "final":
                    final = str(ev.data.get("text", ""))
        finally:
            self._active.pop(run.id, None)
            await rt.cancel(run)
        if not final:
            raise ValueError("runtime finished without an answer")
        return final
