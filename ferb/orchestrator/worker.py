"""Owns one contained model attempt, liveness and trace recording. Must not infer intent, choose a target or claim semantic success from transport facts.

One worker adapter attempt: scoped run token, registered tools, liveness, cancellation and typed report.

No action-count, attempt-count or task-duration cutoff. The orchestrator chooses recovery."""

from __future__ import annotations

import asyncio
import errno
import json
import os
from functools import partial
from typing import TYPE_CHECKING, Any, Awaitable, Callable

from pydantic import ValidationError

from ferb.clock import context
from ferb.core.context import ContextBlock, assemble, count_tokens, runtime_limits
from ferb.core.errors import (AuthError, FerbError, InputTooLarge, ModelLimitsUnknown, NoTarget, PlanRejected,
                              Refused, RuntimeUnavailable)
from ferb.core.prompts import load_prompt
from ferb.core.structured import InvalidAnswer, check_final
from ferb.core.interfaces import AgentRuntime
from ferb.core.types import ModelImage, ModelLimits, ModelRequest, Outcome, RunHandle, Task
from ferb.orchestrator.tree import TaskRecord
from ferb.orchestrator.steering import deliver as deliver_steering
from ferb.orchestrator import progress

if TYPE_CHECKING:
    from ferb.orchestrator.supervisor import _Tree

NO_TOOLS_LOADED = ("Runtime fact: your previous attempt at this exact step never loaded FERB's tools, so it could not "
                   "act or inspect anything. FERB's tools may be deferred: first call tool_search with the query \"ferb\" "
                   "(and with an exact tool name if one you need is still missing), then do the step.")


class ServiceError(ValueError):
    """The model service (Codex / Claude) itself failed: its message is kept and told to the owner plainly. `network`:
    a failure reaching the service (its account check, a dropped stream), which this Mac being offline explains."""

    def __init__(self, message: str, runtime: str = "Codex", network: bool = False) -> None:
        super().__init__(message)
        self.runtime, self.network = runtime, network


class ContextOverflow(ServiceError):
    """The run's model context overflowed: the runtime's documented window-exceeded error (Codex's
    `contextWindowExceeded`, OpenAI's `context_length_exceeded`, Anthropic's `model_context_window_exceeded` stop), as
    the adapter marked it (`overflow` on its error event). Not a network failure."""


class Overflowed(Outcome):
    """An attempt that ended because its model context overflowed: refused by FERB before sending (InputTooLarge) or
    the runtime's window-exceeded error (ContextOverflow). A caller with optional context may try once with less
    (the final checker without its recorded calls, fix wave 6b round 1)."""


def service_problem(message: str, runtime: str = "Codex", network: bool = False) -> str:
    """The owner-facing sentence for a model-service failure. A failure reaching the service while this Mac has no
    network route (a fact, ferb/sense/network.py) says the Mac seems offline, not "try again shortly"."""
    from ferb.sense import network as route
    if network and route.has_route() is False:
        return (f"This Mac seems to be offline (it has no network route), so I couldn't reach the AI service I use "
                f"({runtime}). Ask again once it's back online.")
    return f"The AI service I use ({runtime}) reported an error: {message}"


def problem_of(e: "ServiceError") -> str:
    return service_problem(str(e), e.runtime, e.network)


def retry_stall_problem(runtime: str, message: str, stall_s: float) -> str:
    """The owner-facing sentence for a run that did nothing but retry for `stall_s` (Codex retries a lost connection
    without end): the offline sentence when this Mac has no network route, else the service's last retry, as it said
    it."""
    from ferb.sense import network as route
    if route.has_route() is False:
        return service_problem(message, runtime, network=True)
    return (f"The AI service I use ({runtime}) only kept retrying for {stall_s:g} s, so I stopped this attempt. "
            f"Its last word: {message.rstrip('.')}.")


def connection_lost(runtime: str, message: str, task: bool = True) -> str:
    """The owner's line the moment a run's AI service connection drops and the runtime retries it itself (fix wave 7;
    owner 2026-10-21: "it should be instant"): the offline sentence when this Mac has no network route, else the
    service and its own words. `task`: a task's run carries on; a chat turn with no task yet answers when it's back."""
    from ferb.sense import network as route
    if route.has_route() is False:
        return "Your Mac seems to be offline: I'll carry on when it's back."
    said = f" It said: {message.rstrip('.')}." if message else ""
    goes_on = "it's retrying and the task carries on" if task else "it's retrying, and I'll answer when it's back"
    return f"Lost the connection to the AI service I use ({runtime}); {goes_on}.{said}"


class ConnectionNotice:
    """One run's connection lines on the live line of the work the owner waits on (fix wave 7): the first retry of an
    outage says so at once, further retries stay quiet, and the run's next real event (or tool call) says once that it
    reconnected. `tell` puts a `progress` event, marked `connection: lost|back`, where that work's live line goes."""

    def __init__(self, runtime: str, tell: Callable[[dict[str, Any]], None], now: Callable[[], str],
                 redact: Callable[[str], str], task: bool = True) -> None:
        self.runtime, self.tell, self.now, self.redact, self.task = runtime, tell, now, redact, task
        self.lost = False

    def retrying(self, message: str) -> None:
        if not self.lost:
            self.lost = True
            self._say("lost", connection_lost(self.runtime, message, self.task))

    def back(self) -> None:
        if self.lost:
            self.lost = False
            self._say("back", f"Reconnected to the AI service I use ({self.runtime}).")

    def _say(self, state: str, text: str) -> None:
        self.tell({"type": "progress", "text": self.redact(text), "started_at": self.now(), "connection": state})


def too_large_problem(e: InputTooLarge) -> str:
    """The owner-facing sentence for a call FERB didn't send: over the runtime's documented per-input character cap
    (L28). FERB refused it; the service reported nothing."""
    service = f"the AI service I use ({e.runtime})" if e.runtime else "the AI service I use"
    return (f"I didn't send this to {service}: its context is {e.chars:,} characters, over that service's limit of "
            f"{e.max_chars:,} characters for one input.")


class RuntimeViolation(Exception):
    """The AI runtime broke one of FERB's rules, as its adapter reported it (a `violation` event): `data` is the
    adapter's own fact (`unexpected_tools`: tools it called that FERB never offered; `auth`: how it signed in). The
    message, kept whole in the records, is "<runtime>: <data>"."""

    def __init__(self, message: str, runtime: str = "", data: dict[str, Any] | None = None) -> None:
        super().__init__(message)
        self.runtime, self.data = runtime, dict(data or {})

    def plain(self) -> str:
        """What it did, in the owner's words, from the adapter's fact; the message itself when it names none."""
        if tools := self.data.get("unexpected_tools"):
            names = ", ".join(str(t) for t in tools) if isinstance(tools, (list, tuple, set)) else str(tools)
            return f"it used tools FERB didn't offer it: {names}"
        if auth := self.data.get("auth"):
            return f"it signed in a way FERB doesn't allow: {auth}"
        return str(self)


REASON_CHARS = 300      # an exception's own words in the owner's line, at most; the records keep them whole
BUG = "That's a bug in FERB, not something you asked wrong."


def _words(text: str, redact: Callable[[str], str]) -> str:
    """An exception's words for the owner's line: redacted first (a secret cut in half no longer matches), on one line,
    and at most REASON_CHARS characters."""
    text = " ".join(str(redact(text)).split())
    return text if len(text) <= REASON_CHARS else text[:REASON_CHARS - 1].rstrip() + "…"


def _ended(text: str) -> str:
    """A sentence with its own end punctuation, else a period."""
    return text if text.endswith((".", "!", "?", "…")) else text + "."


def _file(name: object, redact: Callable[[str], str]) -> str:
    return _words(os.fsdecode(name) if isinstance(name, (str, bytes)) else str(name), redact)


def reason(e: BaseException, redact: Callable[[str], str] | None = None) -> str:
    """Why something FERB was doing failed, in plain words for the owner (owner 2026-10-24: an internal or permission
    restriction is stated, never a vague "something went wrong on my side"). Classified by exception type and errno
    only (R2): the AI service's own error, or a call FERB didn't send, as always; macOS refusing (EPERM/EACCES), with
    the path and macOS's own words; FERB's protection rules; the AI runtime breaking FERB's rules; a plan or an answer
    that failed FERB's checks; a FERB error's own message; macOS's other errors; anything else is a bug in FERB, named
    by its class. `redact` (the redactor) runs before the cap. The exact failure stays in the records."""
    clean: Callable[[str], str] = redact or (lambda text: text)
    if isinstance(e, ServiceError):
        return problem_of(e)
    if isinstance(e, InputTooLarge):
        return too_large_problem(e)
    if isinstance(e, OSError) and (isinstance(e, PermissionError) or e.errno in (errno.EPERM, errno.EACCES)):
        said = _words(e.strerror or str(e) or type(e).__name__, clean)
        if e.filename is not None:
            return f"macOS didn't let FERB open {_file(e.filename, clean)} ({said})."
        return f"macOS refused an operation FERB needed ({said})."
    if (isinstance(e, Refused) and not isinstance(e, NoTarget)) or isinstance(e, AuthError):
        return _ended(f"FERB's protection rules stopped it: {_words(str(e) or type(e).__name__, clean)}")
    if isinstance(e, RuntimeViolation):
        who = f"the AI runtime ({e.runtime})" if e.runtime else "the AI runtime"
        return _ended(f"{who} broke one of FERB's rules: {_words(e.plain(), clean)}")
    if isinstance(e, PlanRejected):
        return _ended("the plan still had problems after one repair: " + _words("; ".join(e.problems), clean))
    if isinstance(e, ValidationError):
        first = e.errors()[0] if e.error_count() else None
        where = ".".join(str(part) for part in first["loc"]) if first else ""
        what = (str(first["msg"]) + (f" at {where}" if where else "")) if first else str(e)
        return f"the AI's answer didn't fit the form FERB requires ({_words(what, clean)})."
    if isinstance(e, InvalidAnswer):
        return f"the AI's answer didn't fit the form FERB requires ({_words(str(e), clean)})."
    if isinstance(e, (FerbError, RuntimeUnavailable)) and str(e).strip():
        return _ended(_words(str(e), clean))           # FERB's own message for a failure it names
    if isinstance(e, OSError) and e.errno is not None:
        said = _words(e.strerror or str(e), clean)
        if e.filename is not None:
            return f"macOS reported an error for {_file(e.filename, clean)} ({said})."
        return f"macOS reported an error ({said})."
    detail = _words(str(e), clean)
    return f"something broke inside FERB ({type(e).__name__}{': ' + detail if detail else ''}). {BUG}"


def couldnt(what: str, e: BaseException, redact: Callable[[str], str] | None = None) -> str:
    """The owner's line for work that failed: "I couldn't <what>: <reason>". The AI service's own sentence, or the
    call FERB didn't send, is told as it always was."""
    if isinstance(e, (ServiceError, InputTooLarge)):
        return reason(e)
    return f"I couldn't {what}: {reason(e, redact)}"


def reserve_tokens(system: str, tool_schemas: list[Any], output_schema: Any) -> int:
    """Tokens `_run` keeps beside the context: FERB's instructions, the tool and answer schemas, and room for the
    runtime's own framing."""
    return (count_tokens(system) + count_tokens(json.dumps(tool_schemas)) + count_tokens(json.dumps(output_schema)) +
            1024)


def model_limits(rt: AgentRuntime) -> ModelLimits:
    """The runtime's documented model limits (L28). A runtime that reports none is an explicit failure the owner is told
    about, never a guessed window."""
    try:
        return runtime_limits(rt)
    except ModelLimitsUnknown as exc:
        raise ServiceError(f"no documented context window: {exc}", rt.name) from None


class Stalled(Exception):
    """A run showed no sign of life for stall_s. `progressed`: it had produced output before going quiet."""
    def __init__(self, progressed: bool = True) -> None:
        super().__init__("no sign of life")
        self.progressed = progressed


class WorkerRuns:
    """Needs from the host: make_runtime, gateway, tokens, spans, tasks, publish, journal, clock, ids, device_id,
    activity, on_violation, heartbeat_poll_s, stall_s, apps(), running_apps(), _system(), _active, _tree_of, _stalled,
    _retrying, _connection,
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
    _retrying: dict[str, tuple[str, str]]
    _connection: dict[str, ConnectionNotice]
    _tools_seen: dict[str, list[str]]
    _tools_offered: dict[str, bool]
    _worker_runs: dict[str, tuple[AgentRuntime, RunHandle]]
    _live: dict[str, asyncio.Queue[dict[str, Any]]]

    _system: Callable[[str], str]
    _approach: Callable[[], str]
    _set_status: Callable[..., None]
    cancel_tree: Callable[["_Tree"], None]


    def tool_progress(self, grant: Any, name: str, args: dict[str, Any], kind: str, site: str | None) -> None:
        """L8: a worker (or the final checker) started a tool call. Its task's live line says what it is doing, built
        by code from the tool and its target (`progress.describe`), never from the worker's words: a Mac app by the
        name FERB knows it by. A line that can't be built is noted in the journal; the call goes on. A call the model
        asks for after an outage is the connection back: that line comes first (fix wave 7)."""
        if (notice := self._connection.get(grant.worker_id)) is not None:
            notice.back()
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

    def _tell_tree(self, tree_id: str, event: dict[str, Any]) -> None:
        """An event on a tree's live line (its run's stream, L8), while the tree runs."""
        if (live := self._live.get(tree_id)) is not None:
            live.put_nowait({**event, "task_id": tree_id})

    async def run_worker(self, tree: "_Tree", task: Task, prompt: str, ctx: list[ContextBlock], schema: dict[str, Any],
                         span: str, *, effort: str | None = None) -> tuple[dict[str, Any] | Outcome, str]:
        """One attempt. The orchestrator, rather than a report-word classifier, chooses recovery. A run whose runtime
        shows FERB's tools were never loaded is a runtime fault, not a judgment: it is re-run once with that fact.
        `effort`: the reasoning effort the run asks its runtime for (L39: "high" for a checker of a step that changed
        files); None keeps the runtime's default."""
        result, worker_id = await self._run_worker_once(tree, task, prompt, ctx, schema, span, effort)
        seen = self._tools_seen.pop(worker_id, None)
        offered = self._tools_offered.pop(worker_id, False)
        if (seen is not None and offered and not any("ferb" in name for name in seen)
                and not isinstance(result, Outcome) and not tree.stop):
            s = self.spans.start("runtime.tools_not_loaded", task_id=task.id, parent_id=span,
                                 input={"tools_seen": seen})
            self.spans.finish(s, output={"rerun": True})
            fact = ContextBlock(NO_TOOLS_LOADED, "runtime fact: FERB tools were not loaded", protected=True)
            result, worker_id = await self._run_worker_once(tree, task, prompt, ctx + [fact], schema, span, effort)
            self._tools_seen.pop(worker_id, None)
            self._tools_offered.pop(worker_id, False)
        return result, worker_id

    async def _run_worker_once(self, tree: "_Tree", task: Task, prompt: str, ctx: list[ContextBlock], schema: dict[str, Any],
                               span: str, effort: str | None = None) -> tuple[dict[str, Any] | Outcome, str]:
        if tree.stop:
            return Outcome(task.id, "cancelled", tree.stop, [], []), ""
        worker_id = "w-" + self.ids.new_id()
        self._tree_of[task.id] = tree
        started = self.clock.now()
        wspan = self.spans.start("worker", task_id=task.id, parent_id=span, device_id=self.device_id,
                                 input={"goal": task.goal, "role": task.role, "scopes": sorted(task.scopes),
                                        **({"effort": effort} if effort else {})})
        if self.tasks.get(task.id) is None:
            self.tasks.add(TaskRecord(task, started, wspan.id, tree.id), status="running")
        else:
            self._set_status(task.id, "running", {})
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
            rt = self.make_runtime(role, mode="worker", tool_names=tools, schema=schema,
                                   **({"effort": effort} if effort else {}))   # D32: each adapter's own setting
            final = await self._run(rt, task, self._system(prompt), full_ctx, token, wspan.id, tree, worker_id,
                                    images=images)
            try:
                result: dict[str, Any] | Outcome = json.loads(final)
            except json.JSONDecodeError as exc:     # outside FERB's form, as check_final words it
                raise InvalidAnswer(f"the answer is not JSON ({exc.msg})") from None
            if not isinstance(result, dict):
                raise InvalidAnswer("the answer is not a JSON object")
        except RuntimeViolation as e:
            failure = f"{type(e).__name__}: {e}"
            # Plain words with the reason, which the owner may read (L11, owner 2026-10-24); the exact failure is
            # evidence for recovery
            result = Outcome(task.id, "failed", "This attempt was stopped: " + reason(e, self.gateway.redact), [],
                             [failure])
        except ServiceError as e:
            failure = f"{e.runtime}: {type(e).__name__}: {e}"
            result = (Overflowed if isinstance(e, ContextOverflow) else Outcome)(task.id, "blocked", problem_of(e),
                                                                                  [], [])
        except InputTooLarge as e:              # refused before sending (L28): not the service's error
            failure = f"{type(e).__name__}: {e}"
            result = Overflowed(task.id, "blocked", too_large_problem(e), [], [])
        except (ValidationError, ValueError, Stalled) as e:
            failure = f"{type(e).__name__}: {e}"
            if worker_id in self._stalled:
                retried = self._retrying.get(worker_id)          # it only retried since its last progress
                result = Outcome(task.id, "failed", retry_stall_problem(*retried, self.stall_s) if retried else
                                 f"This attempt stopped responding: the AI runtime showed no sign of life for "
                                 f"{self.stall_s:g} s, so FERB stopped it.", [],
                                 [f"no sign of life for {self.stall_s:g} s, not counting waits on the owner" +
                                  (" (only retries)" if retried else "")])
            else:
                # an answer outside FERB's form says what was wrong; another ValueError here is the run's own words
                # for why it gave no usable report ("runtime finished without an answer")
                why = (reason(e, self.gateway.redact) if isinstance(e, (ValidationError, InvalidAnswer))
                       else _ended(_words(str(e) or type(e).__name__, self.gateway.redact)))
                result = Outcome(task.id, "cancelled" if tree.stop else "failed",
                                 tree.stop or "The report from this attempt wasn't usable: " + why, [],
                                 [] if tree.stop else [failure])
        except Exception as e:
            failure = f"{type(e).__name__}: {e}"
            result = Outcome(task.id, "blocked", "This attempt couldn't run: " + reason(e, self.gateway.redact), [],
                             [failure])
        finally:
            self._stalled.discard(worker_id)
            self._retrying.pop(worker_id, None)
            self.activity.end(worker_id)
            self.tokens.revoke_worker(worker_id)
        self.spans.finish(wspan, output={"result": (result.summary if isinstance(result, Outcome)
                                                    else json.dumps(result)),
                                         "status": result.status if isinstance(result, Outcome)
                                         else result.get("status")}, error=failure)
        return result, worker_id          # `task.finished` is the tree's, published once when it ends

    async def _run(self, rt: AgentRuntime, task: Task, system: str, ctx: list[ContextBlock], token: str, parent_span: str,
                   tree: "_Tree" | None, worker_id: str, *, on_delta: Callable[[str], None] | None = None,
                   on_usage: Callable[[dict[str, Any]], None] | None = None,
                   images: tuple[ModelImage, ...] = (),
                   notify: Callable[[dict[str, Any]], None] | None = None) -> str:
        """A worker's run, once (its tree's orchestrator decides any retry). Any other call (the planner's turn, a
        dispatch, a question, a summary) is watched for silence too, and one that showed no sign of life for stall_s
        from its very start is tried once more (owner 2026-10-27, after a scheduled visa check's planner call sat silent
        for 15 minutes); a second silence, or a call that went quiet after answering in part, fails with that reason."""
        once = partial(self._run_once, rt, task, system, ctx, token, parent_span, tree, worker_id, on_delta=on_delta,
                       on_usage=on_usage, images=images, notify=notify)
        if worker_id:
            return await once()
        for attempt in (1, 2):
            try:
                return await once()
            except Stalled as stall:
                s = self.spans.start("model.stall", task_id=task.id if task.id != task.request_id else None,
                                     parent_id=parent_span, input={"runtime": rt.name, "attempt": attempt})
                self.spans.finish(s, error=f"no sign of life for {self.stall_s:g} s")
                if stall.progressed or attempt == 2:
                    raise ServiceError(f"the AI service ({rt.name}) showed no sign of life for {self.stall_s:g} s"
                                       + (" twice" if attempt == 2 else ""), rt.name, network=True) from None
        raise AssertionError("unreachable")

    async def _run_once(self, rt: AgentRuntime, task: Task, system: str, ctx: list[ContextBlock], token: str,
                        parent_span: str, tree: "_Tree" | None, worker_id: str, *,
                        on_delta: Callable[[str], None] | None = None,
                        on_usage: Callable[[dict[str, Any]], None] | None = None,
                        images: tuple[ModelImage, ...] = (),
                        notify: Callable[[dict[str, Any]], None] | None = None) -> str:
        """`notify`: where a chat turn's live line goes (its stream); a task's run tells its tree's live line, and a run
        with neither (a learner, the attention check, the chat summary) tells no one of a lost connection."""
        revision = len(tree.steering.messages) if tree is not None else 0
        if tree is not None and task.role == 'worker':
            # A startup failure is a current-attempt failure, not a stale result to retry forever.
            tree.result_revision = revision
        if tree is not None and revision:
            # Internal adapter retries may reuse a context captured before a correction arrived.
            ctx = ctx + [ContextBlock('\n\n'.join(tree.steering.messages), 'latest owner corrections', protected=True)]
        tool_schemas = self.gateway.tools_for(self.tokens.validate(token)) if token else []
        output_schema = getattr(rt, "output_schema", None)
        limits = model_limits(rt)
        try:                                    # the model's window and its runtime's per-input character cap (L28)
            assembly = assemble(ctx, budget_tokens=limits.context_budget,
                                reserved_tokens=reserve_tokens(system, tool_schemas, output_schema),
                                max_chars=limits.max_input_chars)
        except InputTooLarge as exc:            # refused by name, never sent to be rejected
            exc.runtime = rt.name
            # which block was too big: each one's retrieval reference and size, never its text
            refused = self.spans.start("context.too_large", task_id=task.id if task.id != task.request_id else None,
                                       parent_id=parent_span, input={"runtime": rt.name, "blocks": [
                                           {"reference": block.reference, "chars": len(block.text)}
                                           for block in ctx if block.text]})
            self.spans.finish(refused, output={"chars": exc.chars, "max_input_chars": exc.max_chars}, error=str(exc))
            raise
        exposure = getattr(self, '_context_exposure', None)
        if exposure is not None:
            exposure(task, ctx, assembly, tree)
        request = ModelRequest(system, tuple(assembly.parts), assembly.cacheable, images)
        self.spans.update_input(parent_span, {"context": request.context, "system_prompt": system,
                                             "context_budget_tokens": limits.context_budget,
                                             "model_limits": {"model": limits.model,
                                                              "context_window": limits.context_window,
                                                              "max_output": limits.max_output,
                                                              "max_input_chars": limits.max_input_chars,
                                                              "source": limits.source},
                                             "cacheable_parts": assembly.cacheable,
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
        # answers its account check would otherwise hold the attempt until Stop. Every other call is watched as well,
        # under its own id (owner 2026-10-27: a planner call sat silent for 15 minutes).
        watch_id = worker_id or f"call:{parent_span}"
        if not worker_id:
            self.activity.start(watch_id)
        watchdog = asyncio.create_task(self._watch(stop, watch_id))
        progressed = False
        try:
            run = await start
        except BaseException as exc:
            watchdog.cancel()
            if not worker_id:
                self.activity.end(watch_id)
            stalled = watch_id in self._stalled
            if not worker_id:
                self._stalled.discard(watch_id)
            if isinstance(exc, RuntimeUnavailable):     # a login problem is never the network's
                raise ServiceError(str(exc), rt.name, getattr(exc, "reason", "") != "auth_required") from None
            current = asyncio.current_task()
            if (isinstance(exc, asyncio.CancelledError) and stalled
                    and not (current is not None and current.cancelling())):
                raise Stalled(progressed=False) from None   # the watchdog's cancellation, not Stop or shutdown
            raise
        started.append(run)
        # which role held the context's parts (P20, review I3): "developer" for a cached Codex planner turn, which says
        # so in its run state; every other run puts them in the user turn
        state = getattr(run, "state", None)
        self.spans.update_input(parent_span, {"context_role": str(state.get("context_role", "user"))
                                              if isinstance(state, dict) else "user"})
        self._active[run.id] = (rt, run)
        if notify is None and tree is not None:
            notify = partial(self._tell_tree, tree.id)
        notice = (ConnectionNotice(rt.name, notify, lambda: self.clock.now().isoformat(), self.gateway.redact,
                                   task=tree is not None) if notify is not None else None)
        if worker_id:
            self._worker_runs[worker_id] = (rt, run)
            if notice is not None:
                self._connection[worker_id] = notice
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
                # a retry is no sign of life (Codex retries a lost connection without end): a run of only retries
                # ends as a stall after stall_s, with the last retry as its reason
                if ev.kind != "retrying":
                    self.activity.beat(watch_id)
                    if worker_id:
                        self._retrying.pop(worker_id, None)
                    progressed = progressed or ev.kind in ("text_delta", "final", "tool_call", "message")
                if notice is not None and ev.kind not in ("retrying", "error", "violation"):
                    notice.back()                        # a real event: the connection is back (an error ends the run)
                if ev.kind == "text_delta":
                    if on_delta is not None:
                        on_delta(str(ev.data.get('text', '')))
                elif ev.kind == "usage":
                    if on_usage is not None:
                        on_usage(dict(ev.data))
                    s = self.spans.start("model.usage", task_id=task.id if task.id != task.request_id else None,
                                         parent_id=parent_span, input={"runtime": rt.name})
                    self.spans.finish(s, output=dict(ev.data), cost_usd=ev.data.get("cost_usd"))            # traced, never limited
                elif ev.kind == "retrying":              # an error the runtime retries itself: recorded, run goes on
                    message = str(ev.data.get("message", ""))
                    s = self.spans.start("model.retry", task_id=task.id if task.id != task.request_id else None,
                                         parent_id=parent_span, input={"runtime": rt.name})
                    self.spans.finish(s, error=message)
                    if worker_id:
                        self._retrying[worker_id] = (rt.name, message)
                    if notice is not None:               # the owner hears at once, once per outage (fix wave 7)
                        notice.retrying(message)
                elif ev.kind == "violation":
                    self.on_violation(rt.name, str(ev.data))
                    raise RuntimeViolation(f"{rt.name}: {ev.data}", rt.name, dict(ev.data))
                elif ev.kind == "error" and ev.data.get("overflow"):     # the window-exceeded error, as marked
                    raise ContextOverflow(str(ev.data.get("message")), rt.name)
                elif ev.kind == "error":
                    raise ServiceError(str(ev.data.get("message")), rt.name, network=True)
                elif ev.kind == "invalid_answer":      # the adapter's own schema check found the answer outside it
                    raise InvalidAnswer(str(ev.data.get("message", "")))
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
            watchdog.cancel()
            if not worker_id:
                self.activity.end(watch_id)
            async def close() -> None:
                if tree is not None:
                    applied = tree.steering.delivered.pop(run.id, revision)
                    if task.role == 'worker':
                        tree.result_revision = applied
                self._active.pop(run.id, None)
                self._worker_runs.pop(worker_id, None)
                self._connection.pop(worker_id, None)
                await rt.cancel(run)
            if tree is not None:
                async with tree.steering.lock:
                    await close()
            else:
                await close()
        if watch_id in self._stalled:
            if not worker_id:
                self._stalled.discard(watch_id)
            raise Stalled(progressed=progressed)
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
