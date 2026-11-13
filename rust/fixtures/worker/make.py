"""Records the Python worker (ferb/orchestrator/worker.py) as the Rust port must reproduce it (ENG-3).

Run from the repo root:  uv run python rust/fixtures/worker/make.py

The worker is a mixin of the supervisor (`WorkerRuns`): everything it touches on `self` is a seam. Here a small host
gives it every seam as a recording fake and runs the real `run_worker`, `_run`, `tool_progress` and the module's
helpers on invented runs. The model runtime is tests/conftest.py's `FakeRuntime`, extended to play a scripted list of
steps (events, clock moves, the gateway's callbacks, a steer, a stop, a stall, a raise). The instructions are made by
the real `Supervisor._system` and `_with_newly_always_on` on invented inputs. Every seam call is appended, in order,
to the scenario's `log`; the Rust test replays each scenario against its own fakes and compares the logs and results
byte for byte. Nothing here reaches a model, the network, the Keychain or ~/.ferb. All data is invented.

Determinism: FakeClock (2026-01-01T09:00:00Z, monotonic 1000.0, moved only by a script's `advance`), SequentialIds,
counters for run tokens, spans and run handles; token counts use the UTF-8 byte ceiling (the tokenizer file is never
looked for, as rust/fixtures/daemon). The stall watchdog polls in real time every 10 ms; a stalled script waits until
it is cancelled, so the order of what is logged never depends on that timing.
"""

import asyncio
import errno
import hashlib
import json
import os
import sys
import tempfile
import time
from dataclasses import asdict
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
ROOT = Path(tempfile.mkdtemp(prefix="ferbwk"))
os.environ["FERB_HOME"] = str(ROOT / "home")
os.environ["HOME"] = str(ROOT / "user")
os.environ["TZ"] = "UTC"
time.tzset()
(ROOT / "user").mkdir()
sys.path.insert(0, str(REPO))

import ferb.core.context as core_context  # noqa: E402
import ferb.sense.network as network  # noqa: E402

core_context.tokenizer_sources = lambda: []        # the UTF-8 byte ceiling, as rust/fixtures/daemon
core_context._encoding.cache_clear()
ROUTE = {"has": True}
network.has_route = lambda: ROUTE["has"]

from pydantic import ValidationError  # noqa: E402

from ferb.clock.clock import FakeClock  # noqa: E402
from ferb.core.context import ContextBlock  # noqa: E402
from ferb.core.errors import (AuthError, FerbError, InputTooLarge, ModelLimitsUnknown, NoTarget,  # noqa: E402
                              PlanRejected, Refused, RuntimeUnavailable, Unsupported)
from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.core.structured import InvalidAnswer  # noqa: E402
from ferb.core.types import Identity, ModelImage, ModelLimits, Outcome, RunHandle, RuntimeEvent, Task  # noqa: E402
from ferb.orchestrator import worker as W  # noqa: E402
from ferb.orchestrator.guards import Activity  # noqa: E402
from ferb.orchestrator.schemas import CHECKER_SCHEMA, WORKER_SCHEMA, WorkerReport  # noqa: E402
from ferb.orchestrator.steering import Steering, deliver  # noqa: E402
from ferb.orchestrator.supervisor import Supervisor  # noqa: E402
from ferb.runtimes.answer_rules import EmptyAnswer, ToolLoop  # noqa: E402
from ferb.runtimes.catalog import SignedOut  # noqa: E402
from tests.conftest import FakeRuntime  # noqa: E402

BODIES: dict[str, str] = {}
BODY_MIN = 2048
SCHEMAS: dict[str, Any] = {}


def sha(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def intern(node: Any) -> Any:
    """Texts of BODY_MIN characters or more are stored once in bodies.json, referenced as {"sha256": ...}."""
    if isinstance(node, dict):
        return {k: intern(v) for k, v in node.items()}
    if isinstance(node, list):
        return [intern(v) for v in node]
    if isinstance(node, str) and len(node) >= BODY_MIN:
        BODIES[sha(node)] = node
        return {"sha256": sha(node)}
    return node


def schema_ref(schema: Any) -> Any:
    if not isinstance(schema, dict):
        return schema
    key = sha(json.dumps(schema))
    SCHEMAS[key] = schema
    return {"schema": key}


def jsonable(v: Any) -> Any:
    """A seam argument as JSON: datetimes as isoformat, floats that JSON can't hold as their repr, sets sorted."""
    if isinstance(v, dict):
        return {str(k): jsonable(x) for k, x in v.items()}
    if isinstance(v, (list, tuple)):
        return [jsonable(x) for x in v]
    if isinstance(v, (set, frozenset)):
        return sorted(jsonable(x) for x in v)
    if isinstance(v, datetime):
        return {"datetime": v.isoformat()}
    if isinstance(v, float) and (v != v or v in (float("inf"), float("-inf"))):
        return {"float": repr(v)}
    if isinstance(v, Identity):
        return v.value
    if isinstance(v, bytes):
        return {"hex": v.hex()}
    return v


def task_json(t: Task) -> dict[str, Any]:
    return {"id": t.id, "parent_id": t.parent_id, "goal": t.goal, "device_id": t.device_id,
            "identity": t.identity.value, "scopes": sorted(t.scopes), "role": t.role, "request_id": t.request_id,
            "success_conditions": list(t.success_conditions),
            "deadline": t.deadline.isoformat() if t.deadline else None}


def block_json(b: ContextBlock) -> dict[str, Any]:
    return jsonable(asdict(b))


def exc_json(e: BaseException) -> dict[str, Any]:
    """An exception as the Rust error enum reads it: class, str, and the attributes the worker reads."""
    out: dict[str, Any] = {"class": type(e).__name__, "str": str(e)}
    for attr in ("runtime", "network", "account", "reason", "progressed", "data", "count", "chars", "max_chars",
                 "problems"):
        if hasattr(e, attr):
            out[attr] = jsonable(getattr(e, attr))
    if isinstance(e, OSError):
        out.update({"errno": e.errno, "strerror": e.strerror, "filename": jsonable(e.filename)})
    return out


def make_exc(spec: dict[str, Any]) -> BaseException:
    """A scripted exception (the classes an adapter, a runtime factory or a host raises)."""
    cls, msg = spec["class"], spec.get("message", "")
    if cls == "RuntimeUnavailable":
        e = RuntimeUnavailable(msg)
        for attr in ("account", "reason"):
            if attr in spec:
                setattr(e, attr, spec[attr])
        return e
    if cls == "SignedOut":
        return SignedOut(msg, spec["runtime"])
    if cls in ("PermissionError", "OSError", "FileNotFoundError"):
        args = [spec["errno"], spec["strerror"]] + ([spec["filename"]] if "filename" in spec else [])
        return {"PermissionError": PermissionError, "OSError": OSError,
                "FileNotFoundError": FileNotFoundError}[cls](*args)
    if cls == "InputTooLarge":
        return InputTooLarge(spec["chars"], spec["max_chars"])
    if cls == "PlanRejected":
        return PlanRejected(spec["problems"])
    named = {"KeyError": KeyError, "ValueError": ValueError, "RuntimeError": RuntimeError, "TypeError": TypeError,
             "Refused": Refused, "NoTarget": NoTarget, "AuthError": AuthError, "FerbError": FerbError,
             "Unsupported": Unsupported, "ModelLimitsUnknown": ModelLimitsUnknown, "InvalidAnswer": InvalidAnswer,
             "TimeoutError": TimeoutError}
    return named[cls](msg)


# ---------------------------------------------------------------------------------------------------------------
# The seams, as recording fakes
# ---------------------------------------------------------------------------------------------------------------

def redact_text(text: str) -> str:
    """The fixture's redactor: any word starting "HIDDEN-" is masked (a stand-in for the gateway's redactor)."""
    return " ".join("[redacted]" if w.startswith("HIDDEN-") else w for w in text.split(" "))


def redact(value: Any) -> Any:
    if isinstance(value, str):
        return redact_text(value)
    if isinstance(value, dict):
        return {k: redact(v) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v) for v in value]
    return value


class Gateway:
    def __init__(self, host: "Host", tools: list[dict[str, Any]]) -> None:
        self.host, self.tools, self.refused = host, tools, 0

    def tools_for(self, grant: Any) -> list[dict[str, Any]]:
        self.host.rec("gateway.tools_for", {"task_id": grant.task_id, "worker_id": grant.worker_id,
                                            "observe_only": grant.observe_only})
        if grant.observe_only:
            return [t for t in self.tools if t["name"] in OBSERVE]
        return list(self.tools)

    def redact(self, value: Any) -> Any:
        self.host.rec("gateway.redact", jsonable(value))
        if self.host.redact_raises:
            raise make_exc(self.host.redact_raises)
        return redact(value)

    def initial_image_given(self, task_id: str) -> None:
        self.host.rec("gateway.initial_image_given", task_id)

    def forget_calls(self, worker_id: str) -> None:
        self.host.rec("gateway.forget_calls", worker_id)

    def calls_refused(self, worker_id: str) -> int:
        self.host.rec("gateway.calls_refused", worker_id)
        return self.refused


class Tokens:
    def __init__(self, host: "Host") -> None:
        self.host, self.n, self.grants = host, 0, {}

    def issue(self, task_id: str, worker_id: str, scopes: set[str], identity: Identity, ttl_s: float,
              observe_only: bool = False) -> str:
        self.n += 1
        token = f"tok-{self.n:03d}"
        self.grants[token] = SimpleNamespace(task_id=task_id, worker_id=worker_id, scopes=frozenset(scopes),
                                             identity=identity, observe_only=observe_only)
        self.host.rec("tokens.issue", {"task_id": task_id, "worker_id": worker_id, "scopes": sorted(scopes),
                                       "identity": identity.value, "ttl_s": jsonable(float(ttl_s)),
                                       "observe_only": observe_only, "token": token})
        return token

    def validate(self, token: str) -> Any:
        self.host.rec("tokens.validate", token)
        return self.grants[token]

    def revoke_worker(self, worker_id: str) -> None:
        self.host.rec("tokens.revoke_worker", worker_id)


class Spans:
    def __init__(self, host: "Host") -> None:
        self.host, self.n = host, 0

    def start(self, kind: str, *, input: dict[str, Any], task_id: str | None = None, parent_id: str | None = None,
              device_id: str | None = None) -> Any:
        self.n += 1
        sid = f"span-{self.n:03d}"
        self.host.rec("spans.start", {"id": sid, "kind": kind, "task_id": task_id, "parent_id": parent_id,
                                      "device_id": device_id, "input": jsonable(input)})
        return SimpleNamespace(id=sid)

    def finish(self, span: Any, *, output: dict[str, Any] | None = None, error: str | None = None,
               cost_usd: float | None = None) -> None:
        self.host.rec("spans.finish", {"id": span.id, "output": jsonable(output), "error": error,
                                       "cost_usd": jsonable(cost_usd)})

    def update_input(self, span_id: str, values: dict[str, Any]) -> None:
        self.host.rec("spans.update_input", {"id": span_id, "input": jsonable(values)})


class Tasks:
    def __init__(self, host: "Host", known: set[str]) -> None:
        self.host, self.known = host, set(known)

    def get(self, task_id: str) -> Any:
        self.host.rec("tasks.get", task_id)
        return SimpleNamespace(task_id=task_id) if task_id in self.known else None

    def add(self, rec: Any, status: str = "running") -> None:
        self.known.add(rec.task.id)
        self.host.rec("tasks.add", {"task": task_json(rec.task), "started_at": rec.started_at.isoformat(),
                                    "span_id": rec.span_id, "tree_id": rec.tree_id, "status": status})

    def set_span(self, task_id: str, span_id: str) -> None:
        self.host.rec("tasks.set_span", {"task_id": task_id, "span_id": span_id})


class Journal:
    def __init__(self, host: "Host") -> None:
        self.host = host

    def append(self, kind: str, data: dict[str, Any]) -> None:
        self.host.rec("journal.append", {"kind": kind, "data": jsonable(data)})


class LoggedActivity:
    """The real Activity; start, beat, waiting and end are logged (the watchdog's `stalled` reads are not: they
    follow real time)."""

    def __init__(self, host: "Host", real: Activity) -> None:
        self.host, self.real = host, real

    def start(self, worker: str) -> None:
        self.host.rec("activity.start", worker)
        self.real.start(worker)

    def beat(self, worker: str) -> None:
        self.host.rec("activity.beat", worker)
        self.real.beat(worker)

    def waiting(self, worker: str, on: bool) -> None:
        self.host.rec("activity.waiting", {"worker": worker, "on": on})
        self.real.waiting(worker, on)

    def stalled(self, worker: str, stall_s: float) -> bool:
        return self.real.stalled(worker, stall_s)

    def end(self, worker: str) -> None:
        self.host.rec("activity.end", worker)
        self.real.end(worker)


# Invented tool rows, in the gateway's listing form ({name, description, input_schema}); the observation subset is
# what an observe-only grant gets.
TOOLS = [
    {"name": "fs.read", "description": "Read a text file (UTF-8) in full or by line range.",
     "input_schema": {"properties": {"path": {"title": "Path", "type": "string"},
                                     "why": {"type": "string", "description": "Optional short explanation."}},
                      "required": ["path"], "title": "Input", "type": "object"}},
    {"name": "fs.write", "description": "Write a file — FERB keeps a recovery copy first.",
     "input_schema": {"properties": {"path": {"title": "Path", "type": "string"},
                                     "content": {"title": "Content", "type": "string"},
                                     "effect": {"enum": ["edit", "send", "pay", "destroy"], "type": "string"}},
                      "required": ["path", "content"], "title": "Input", "type": "object"}},
    {"name": "owner.ask", "description": "Ask the owner one question and wait for the answer (café \U0001f600).",
     "input_schema": {"properties": {"question": {"type": "string"}, "options": {"items": {"type": "string"},
                                                                                 "type": "array"}},
                      "required": ["question"], "type": "object"}},
    {"name": "mac.open", "description": "Open an app.", "input_schema": {
        "properties": {"app": {"type": "string"}, "limit": {"default": 1e16, "type": "number"}}, "type": "object"}},
    {"name": "shell.wait", "description": "Wait for a task's command.", "input_schema": {
        "properties": {"id": {"type": "string"}, "timeout_s": {"default": 30.5, "type": "number"}},
        "type": "object"}},
    {"name": "browser.open", "description": "Open a page in the owner's browser.", "input_schema": {
        "properties": {"url": {"type": "string"}}, "required": ["url"], "type": "object"}},
    {"name": "task.evidence", "description": "Read this task's records.", "input_schema": {
        "properties": {"span_id": {"anyOf": [{"type": "string"}, {"type": "null"}], "default": None}},
        "type": "object"}},
]
OBSERVE = {"fs.read", "task.evidence"}

SYSTEM_INPUTS = {
    "facts": "- macOS 26.1 (invented); FERB's workspace: /FX/home/workspace\n- Paired profiles: none",
    "live_facts": "Layer 2 — right now: Finder in front; \"quotes\" and a tab\tinside.",
    "capabilities": "Installed worker capabilities:\n- files (fs.*)\n- the owner's browser (not connected — "
                    "Settings → Browsers)",
    "identity_context": "Core profile (Tier 1):\n- The owner prefers short answers.\n- Big number 12345678901234567890",
    "skills_catalog": [{"name": "Weekly report", "summary": "How the owner likes the weekly report — short."}],
    "abilities_catalog": [{"id": "google_docs", "version": "1", "available": False, "reason": None,
                           "ratio": 0.5, "big": 1e16}],
    "identity_pending": "",
}


class Host(W.WorkerRuns):
    _system = Supervisor._system
    _approach = Supervisor._approach
    _with_newly_always_on = Supervisor._with_newly_always_on

    def __init__(self, sc: dict[str, Any]) -> None:
        self.log: list[Any] = []
        self.sc = sc
        self.clock = FakeClock()
        self.ids = SequentialIds("id")
        self.device_id = "dev-fx"
        self.activity = LoggedActivity(self, Activity(self.clock.monotonic))
        self.heartbeat_poll_s, self.stall_s = 0.01, float(sc.get("stall_s", 180.0))
        self.gateway = Gateway(self, sc.get("tools", TOOLS))
        self.gateway.refused = sc.get("calls_refused", 0)
        self.tokens = Tokens(self)
        self.spans = Spans(self)
        self.tasks = Tasks(self, set(sc.get("known_tasks", [])))
        self.journal = Journal(self)
        self.redact_raises = sc.get("redact_raises")
        self._active, self._tree_of, self._stalled, self._retrying = {}, {}, set(), {}
        self._connection, self._tools_seen, self._tools_offered, self._worker_runs = {}, {}, {}, {}
        self._live, self._last_steps = {}, {}
        inputs = {**SYSTEM_INPUTS, **sc.get("system_inputs", {})}
        self.inputs = inputs
        self.facts = lambda: inputs["facts"]
        self.live_facts = lambda: inputs["live_facts"]
        self.availability = lambda: SimpleNamespace(text=inputs["capabilities"])
        self.identity_context = lambda: inputs["identity_context"]
        self.identity_pending = lambda: inputs["identity_pending"]
        self.skills_catalog = lambda: inputs["skills_catalog"]
        self.abilities_catalog = lambda: inputs["abilities_catalog"]
        self.runtimes_made = 0
        self.notified: list[Any] = []

    def rec(self, what: str, data: Any) -> None:
        self.log.append([what, data])

    def publish(self, topic: str, data: dict[str, Any]) -> None:
        self.rec("publish", {"topic": topic, "data": jsonable(data)})

    def signals(self) -> tuple[datetime | None, datetime | None]:
        self.rec("signals", None)
        s = self.sc.get("signals", [None, None])
        return tuple(datetime.fromisoformat(x) if x else None for x in s)        # type: ignore[return-value]

    def browser_context(self, task_id: str) -> str:
        self.rec("browser_context", task_id)
        return f"Current browser observations for {task_id} (data): one tab, example.com"

    def apps(self) -> list[tuple[str, str]]:
        self.rec("apps", None)
        if self.sc.get("apps_raise"):
            raise make_exc(self.sc["apps_raise"])
        return [("com.apple.TextEdit", "TextEdit"), ("com.example.Notes", "Notes é")]

    def running_apps(self) -> list[tuple[str, str, int]]:
        self.rec("running_apps", None)
        return [("com.apple.finder", "Finder", 4101)]

    def shell_process(self, pid: str) -> dict[str, Any] | None:
        self.rec("shell_process", pid)
        return self.sc.get("shells", {}).get(pid)

    def on_violation(self, runtime: str, text: str) -> None:
        self.rec("on_violation", {"runtime": runtime, "text": text})

    def runtime_account(self, runtime: str, account: str) -> None:
        self.rec("runtime_account", {"runtime": runtime, "account": account})

    def _set_status(self, task_id: str, status: str, extra: dict[str, Any]) -> None:
        self.rec("set_status", {"task_id": task_id, "status": status, "extra": jsonable(extra)})

    def _context_exposure(self, task: Task, ctx: list[ContextBlock], assembly: Any, tree: Any) -> None:
        self.rec("context_exposure", {"task_id": task.id, "references": [b.reference for b in ctx],
                                      "parts": len(assembly.parts), "shown": list(assembly.shown),
                                      "tree": tree.id if tree is not None else None})

    def cancel_tree(self, tree: Any) -> None:
        raise AssertionError("the worker never calls cancel_tree")

    def make_runtime(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any],
                     effort: str | None = None) -> "ScriptRuntime":
        self.rec("make_runtime", {"role": role, "mode": mode, "tool_names": list(tool_names),
                                  "schema": schema_ref(schema), "effort": effort})
        scripts = self.sc["runtimes"]
        spec = scripts[min(self.runtimes_made, len(scripts) - 1)]
        self.runtimes_made += 1
        if "make_raise" in spec:
            raise make_exc(spec["make_raise"])
        return ScriptRuntime(self, spec, mode, list(tool_names))


class ScriptRuntime(FakeRuntime):
    """tests/conftest.py's FakeRuntime playing a scripted list of steps; every runtime call is logged."""

    def __init__(self, host: Host, spec: dict[str, Any], mode: str = "worker", tools: list[str] | None = None) -> None:
        super().__init__(None, mode, tools or [])      # type: ignore[arg-type]
        self.host, self.spec = host, spec
        self.name = spec.get("name", "fake")
        if "limits" in spec:
            self.limits = ModelLimits(**spec["limits"]) if spec["limits"] else None
        if "output_schema" in spec:
            self.output_schema = spec["output_schema"]
        self.runs = 0

    async def start(self, task: Task, request: Any, run_token: str) -> RunHandle:
        self.runs += 1
        n = self.runs
        self.host.rec("rt.start", {"runtime": self.name, "task": task_json(task), "token": run_token, "request": {
            "system": request.system, "parts": list(request.parts), "cacheable": request.cacheable,
            "images": [{"ref": i.ref, "mime": i.mime, "data": i.data.hex()} for i in request.images]}})
        plan = self.spec.get("calls", [{}])[min(n - 1, len(self.spec.get("calls", [{}])) - 1)]
        for step in plan.get("start", []):
            await self.play_start(step)
        return RunHandle(f"run-{n}", self.name, task.id, {"token": run_token, "cancel": False, "n": n,
                                                          **plan.get("state", {})})

    async def play_start(self, step: dict[str, Any]) -> None:
        if "advance" in step:
            self.host.clock.advance(step["advance"])
        if "stop" in step:
            self.stop_tree(step["stop"], None)
        if "raise" in step:
            raise make_exc(step["raise"])
        if step.get("hang"):
            await asyncio.Event().wait()            # the watchdog cancels it

    def stop_tree(self, reason: str, run: RunHandle | None) -> None:
        """What the supervisor's stop does to a running worker (cancel_tree: the tree's stop words, then each of its
        active runs cancelled)."""
        tree = self.host.tree
        tree.stop = reason
        for rt, active in list(self.host._active.values()):
            asyncio.get_event_loop().create_task(rt.cancel(active))

    async def events(self, run: RunHandle) -> Any:
        plan = self.spec.get("calls", [{}])[min(run.state["n"] - 1, len(self.spec.get("calls", [{}])) - 1)]
        for step in plan.get("steps", []):
            if "event" in step:
                yield RuntimeEvent(step["event"], dict(step.get("data", {})))
            elif "advance" in step:
                self.host.clock.advance(step["advance"])
            elif "tool_progress" in step:
                p = step["tool_progress"]
                grant = self.host.tokens.grants[run.state["token"]]
                self.host.tool_progress(grant, p["name"], p.get("args", {}), p.get("kind", ""), p.get("site"))
            elif "waiting" in step:
                grant = self.host.tokens.grants[run.state["token"]]
                self.host.activity.waiting(grant.worker_id, step["waiting"])
            elif "steer" in step:
                tree = self.host.tree
                async with tree.steering.lock:
                    tree.steering.messages.append(step["steer"])
                    for worker_id, (rt, active) in list(self.host._worker_runs.items()):
                        await deliver(self.host, tree, rt, active)
            elif "stop" in step:
                self.stop_tree(step["stop"], run)
                await asyncio.sleep(0)
            elif "stall" in step:
                while not run.state["cancel"]:
                    await asyncio.sleep(0.005)
                return
            elif "raise" in step:
                raise make_exc(step["raise"])
            elif "refused" in step:
                self.host.gateway.refused = step["refused"]
            if run.state["cancel"]:
                return

    async def continue_(self, run: RunHandle, message: str) -> None:
        self.host.rec("rt.continue", {"run": run.id, "message": message})
        how = self.spec.get("continue", "accept")
        if how == "unsupported":
            raise Unsupported("this runtime can't take same-run input")
        if how == "not_implemented":
            raise NotImplementedError
        if how != "accept":
            raise make_exc({"class": how, "message": "continue failed"})

    async def cancel(self, run: RunHandle) -> None:
        self.host.rec("rt.cancel", run.id)
        run.state["cancel"] = True


def make_tree(host: Host, spec: dict[str, Any]) -> Any:
    st = Steering()
    st.messages.extend(spec.get("steering", []))
    image = spec.get("start_image")
    block = spec.get("start_block")
    tree = SimpleNamespace(
        id=spec.get("id", "t1"), stop=spec.get("stop", ""), steering=st, result_revision=0,
        runtime=spec.get("runtime"), spec=SimpleNamespace(worker_type=spec.get("worker_type", "default")),
        worker=SimpleNamespace(id=spec.get("worker_task", "t1.worker")),
        start_block=ContextBlock(**block) if block else None,
        start_image=ModelImage(bytes.fromhex(image["data"]), image["ref"]) if image else None)
    host.tree = tree
    if spec.get("live"):
        host._live[tree.id] = asyncio.Queue()
    return tree


def make_task(spec: dict[str, Any]) -> Task:
    deadline = spec.get("deadline")
    return Task(id=spec.get("id", "t1.worker"), parent_id=spec.get("parent_id", "t1"),
                goal=spec.get("goal", "Find the invented report and summarize it"), device_id="dev-fx",
                identity=Identity(spec.get("identity", "owner")), scopes=set(spec.get("scopes", ["fs:read", "fs:write"])),
                role=spec.get("role", "worker"), request_id=spec.get("request_id", "req-1"),
                success_conditions=list(spec.get("conditions", ["The summary names the report's three points"])),
                deadline=datetime.fromisoformat(deadline) if deadline else None)


DEFAULT_CTX = [
    {"text": "Owner's complete original request:\nsummarize the report — please", "reference": "request",
     "cache": True, "protected": True},
    {"text": "Task: Find the invented report and summarize it\nWorker type: default", "reference": "task",
     "protected": True},
    {"text": "Layer 3 — relevant activity (data): []", "reference": "activity", "priority": 2},
    {"text": "Success conditions:\n- The summary names the report's three points", "reference": "conditions",
     "protected": True},
]


def leftovers(host: Host) -> dict[str, Any]:
    return {"active": sorted(host._active), "worker_runs": sorted(host._worker_runs),
            "connection": sorted(host._connection), "tools_seen": jsonable(host._tools_seen),
            "tools_offered": jsonable(host._tools_offered), "stalled": sorted(host._stalled),
            "retrying": jsonable(host._retrying), "tree_of": sorted(host._tree_of),
            "last_steps": jsonable(host._last_steps)}


def live_lines(host: Host) -> dict[str, Any]:
    out = {}
    for tree_id, q in host._live.items():
        items = []
        while not q.empty():
            items.append(jsonable(q.get_nowait()))
        out[tree_id] = items
    return out


def result_json(result: Any) -> dict[str, Any]:
    if isinstance(result, Outcome):
        out = {"class": type(result).__name__, "outcome": {"task_id": result.task_id, "status": result.status,
                                                            "summary": result.summary,
                                                            "artifacts": list(result.artifacts),
                                                            "evidence": list(result.evidence)}}
        if isinstance(result, W.ServiceDown):
            out.update({"runtime": result.runtime, "problem": result.problem})
        return out
    return {"class": "report", "report": jsonable(result)}


async def run_worker_scenario(sc: dict[str, Any]) -> dict[str, Any]:
    ROUTE["has"] = sc.get("route", True)
    host = Host(sc)
    tree = make_tree(host, sc.get("tree", {}))
    attempts = []
    for attempt in sc.get("attempts", [{}]):
        task = make_task({**sc.get("task", {}), **attempt.get("task", {})})
        ctx = [ContextBlock(**b) for b in attempt.get("ctx", sc.get("ctx", DEFAULT_CTX))]
        schema = attempt.get("schema", sc.get("schema", WORKER_SCHEMA))
        kw = {k: attempt.get(k, sc.get(k)) for k in ("effort", "roles", "observe_only")}
        kw = {k: (tuple(v) if k == "roles" else v) for k, v in kw.items() if v is not None}
        before = len(host.log)
        result, worker_id = await host.run_worker(tree, task, attempt.get("prompt", sc.get("prompt", "worker.md")),
                                                  ctx, schema, attempt.get("span", "span-parent"), **kw)
        await asyncio.sleep(0.05)                    # cancels the stop step scheduled settle
        attempts.append({"inputs": {"task": task_json(task), "prompt": attempt.get("prompt", sc.get("prompt",
                                                                                                    "worker.md")),
                                    "ctx": [block_json(b) for b in ctx], "schema": schema_ref(schema),
                                    "span": attempt.get("span", "span-parent"), **jsonable(kw),
                                    "system": host._system(attempt.get("prompt", sc.get("prompt", "worker.md")),
                                                           roles=kw.get("roles", ())),
                                    "identity_pending": host.inputs["identity_pending"]},
                         "log": host.log[before:], "result": result_json(result), "worker_id": worker_id,
                         "result_revision": tree.result_revision,
                         "delivered": dict(tree.steering.delivered)})
    return {"leftovers": leftovers(host), "live": live_lines(host), "attempts": attempts}


async def run_call_scenario(sc: dict[str, Any]) -> dict[str, Any]:
    """`_run` for a call that is not a worker's (the planner's turn, a summary, attention, a question)."""
    ROUTE["has"] = sc.get("route", True)
    host = Host(sc)
    tree = make_tree(host, sc["tree"]) if "tree" in sc else None
    task = make_task(sc.get("task", {}))
    rt = ScriptRuntime(host, sc["runtimes"][0])
    ctx = [ContextBlock(**b) for b in sc.get("ctx", DEFAULT_CTX)]
    seen: dict[str, list[Any]] = {"delta": [], "usage": [], "tool": [], "timing": [], "notify": []}
    token = ""
    if sc.get("token"):
        token = host.tokens.issue(task.id, "", task.scopes, task.identity, 60.0)
    cb = sc.get("callbacks", [])
    images = tuple(ModelImage(bytes.fromhex(i["data"]), i["ref"]) for i in sc.get("images", []))
    try:
        text = await host._run(
            rt, task, sc.get("system", "You are FERB's planner (invented)."), ctx, token, "span-parent", tree, "",
            on_delta=(lambda t: seen["delta"].append(t)) if "delta" in cb else None,
            on_usage=(lambda u: seen["usage"].append(jsonable(u))) if "usage" in cb else None,
            images=images,
            notify=(lambda e: seen["notify"].append(jsonable(e))) if "notify" in cb else None,
            on_tool=(lambda: seen["tool"].append(True)) if "tool" in cb else None,
            on_timing=(lambda t: seen["timing"].append(jsonable(t))) if "timing" in cb else None,
            retry_silent=sc.get("retry_silent", True))
        outcome: dict[str, Any] = {"returned": text}
    except BaseException as e:                     # noqa: BLE001
        outcome = {"raised": exc_json(e)}
    await asyncio.sleep(0.05)
    return {"inputs": {"task": task_json(task), "ctx": [block_json(b) for b in ctx], "token": token,
                       "callbacks": cb, "retry_silent": sc.get("retry_silent", True),
                       "system": sc.get("system", "You are FERB's planner (invented)."),
                       "images": sc.get("images", [])},
            "log": host.log, "outcome": outcome, "callbacks_seen": seen, "leftovers": leftovers(host),
            "live": live_lines(host)}


def usage(i: int = 10, o: int = 5, cost: Any = 0.001, timing: Any = None) -> dict[str, Any]:
    d: dict[str, Any] = {"input_tokens": i, "output_tokens": o}
    if cost is not None:
        d["cost_usd"] = cost
    if timing is not None:
        d["timing"] = timing
    return {"event": "usage", "data": d}


REPORT = json.dumps({"status": "done", "answer": "The report's three points: a, b and c.", "reason": "",
                     "question": None, "artifacts": [], "how": "Read it with fs.read."})
NOT_DONE = json.dumps({"status": "not_done", "answer": "", "reason": "The file wasn't there.",
                       "question": "Which folder holds the report?", "artifacts": [], "how": ""})


def final(text: str = REPORT, **more: Any) -> dict[str, Any]:
    return {"event": "final", "data": {"text": text, **more}}


def rt(*steps: Any, start: list[Any] | None = None, state: dict[str, Any] | None = None, **spec: Any
       ) -> dict[str, Any]:
    call: dict[str, Any] = {"steps": list(steps)}
    if start:
        call["start"] = start
    if state:
        call["state"] = state
    return {"calls": [call], **spec}


def calls(*plans: dict[str, Any], **spec: Any) -> dict[str, Any]:
    return {"calls": [p["calls"][0] for p in plans], **spec}


ERR = {"event": "error"}
SMALL = {"model": "tiny", "context_window": 3000, "max_output": 500, "source": "invented"}

WORKER_SCENARIOS: list[dict[str, Any]] = [
    {"name": "plain_report", "what": "a new task row, usage, a valid report",
     "runtimes": [rt(usage(), final())]},
    {"name": "existing_task_row", "what": "the task row exists: its status is set instead",
     "known_tasks": ["t1.worker"], "runtimes": [rt(usage(), final())]},
    {"name": "tool_calls", "what": "a run's tool calls: events, the live lines code writes, usage with and without "
     "the runtime's timing and cost",
     "tree": {"live": True},
     "shells": {"p-1": {"owner_task": "t1.worker", "command": "codex exec `make` --long run", "started": 1767254400.0},
                "p-2": {"owner_task": "other", "command": "sleep 5", "started": "1767254400"}},
     "runtimes": [rt({"event": "message", "data": {"text": "Looking."}}, {"advance": 1.25},
                     {"event": "text_delta", "data": {"text": "partial"}},
                     {"event": "tool_call", "data": {"name": "fs.read", "offered": True}},
                     {"tool_progress": {"name": "fs.read", "args": {"path": "/FX/home/report.txt"}, "kind": "read"}},
                     {"advance": 0.5},
                     usage(cost=None),
                     {"tool_progress": {"name": "mac.open", "args": {"app": "textedit"}, "kind": "open"}},
                     {"tool_progress": {"name": "shell.wait", "args": {"id": "p-1"}, "kind": ""}},
                     {"tool_progress": {"name": "shell.wait", "args": {"id": "p-2"}, "kind": ""}},
                     {"tool_progress": {"name": "browser.open", "args": {"url": "https://example.com/HIDDEN-x"},
                                        "kind": "navigate", "site": "example.com"}},
                     {"event": "progress", "data": {}},
                     usage(20, 7, cost=0.25, timing={"by": "runtime", "ttft_ms": 3, "total_ms": 9}),
                     {"advance": 2},
                     final())]},
    {"name": "invalid_answer_then_repaired", "what": "the adapter's own check finds the answer outside the form, "
     "then the next attempt (the supervisor's retry) answers in it",
     "attempts": [{}, {}],
     "runtimes": [rt({"event": "invalid_answer", "data": {"message": "the answer doesn't match the schema at "
                                                                     "status: 'maybe' is not one of ['done', "
                                                                     "'not_done']"}}),
                  rt(usage(), final(NOT_DONE))]},
    {"name": "final_not_json", "what": "the final answer is not JSON, checked against the runtime's schema",
     "runtimes": [rt(final("Here you go: {"), output_schema=WORKER_SCHEMA)]},
    {"name": "final_not_json_no_schema", "what": "not JSON, no schema of the runtime's: the worker's own words",
     "runtimes": [rt(final("{'status': 'done'}"))]},
    {"name": "final_not_object", "what": "JSON but not an object", "runtimes": [rt(final("[1, 2.5, 1e16]"))]},
    {"name": "final_schema_mismatch", "what": "the runtime's schema refuses the answer",
     "runtimes": [rt(final(json.dumps({"status": "done"})), output_schema=WORKER_SCHEMA)]},
    {"name": "final_numbers_kept", "what": "a report's numbers and key order are kept as Python reads them",
     "runtimes": [rt(final('{"status": "done", "answer": "x", "n": 1e16, "big": 123456789012345678901234567890, '
                           '"f": 2.0, "neg": -0.0, "u": "caf\\u00e9 \\ud83d\\ude00", "z": null, "b": true, '
                           '"dup": 1, "dup": 2}'))]},
    {"name": "empty_final", "what": "the run ended without an answer",
     "runtimes": [rt(usage())]},
    {"name": "question_to_owner", "what": "a question to the owner: the wait suspends the silence guard, the answer "
     "comes back, the report carries it",
     "runtimes": [rt({"event": "tool_call", "data": {"name": "owner.ask", "offered": True}},
                     {"tool_progress": {"name": "owner.ask", "args": {"question": "Which folder?"},
                                        "kind": "owner_question"}},
                     {"waiting": True}, {"advance": 400}, {"waiting": False},
                     {"event": "tool_call", "data": {"name": "fs.read", "offered": True}},
                     usage(), final(NOT_DONE))]},
    {"name": "steering_at_start", "what": "a correction given before the run: in its context and delivered at once",
     "tree": {"steering": ["Use the March report instead."]},
     "runtimes": [rt(usage(), final())]},
    {"name": "steering_mid_run", "what": "a correction while the run works: delivered into the running call",
     "tree": {"steering": ["First words."]},
     "runtimes": [rt({"event": "message", "data": {"text": "on it"}}, {"steer": "Also check the April one."},
                     usage(), final())]},
    {"name": "steering_unsupported", "what": "the runtime can't take a correction: it stays pending",
     "tree": {"steering": ["Use the March report instead."]},
     "runtimes": [rt({"steer": "And April."}, usage(), final(), **{"continue": "unsupported"})]},
    {"name": "steering_not_implemented", "what": "the scripted runtime of the tests (NotImplementedError)",
     "tree": {"steering": ["Use the March report instead."]},
     "runtimes": [rt(usage(), final(), **{"continue": "not_implemented"})]},
    {"name": "steering_failed", "what": "delivering a correction fails: journaled, left pending",
     "tree": {"steering": ["Use the March report instead."]},
     "runtimes": [rt(usage(), final(), **{"continue": "KeyError"})]},
    {"name": "stop_before", "what": "the tree was stopped before the attempt began",
     "tree": {"stop": "Stopped, as you asked."}, "runtimes": [rt(final())]},
    {"name": "stop_while_starting", "what": "stopped while the runtime started",
     "runtimes": [rt(final(), start=[{"stop": "Stopped, as you asked."}])]},
    {"name": "stop_mid_run", "what": "stopped while the run works: its run is cancelled",
     "runtimes": [rt({"event": "message", "data": {"text": "working"}}, {"stop": "Stopped, as you asked."},
                     {"stall": True})]},
    {"name": "start_usage_limit", "what": "the runtime's start reports the plan's usage limit",
     "runtimes": [rt(final(), start=[{"raise": {"class": "RuntimeUnavailable", "message": "Codex: usage limit "
                                                "reached (invented)", "account": "usage_limit"}}], name="codex")]},
    {"name": "start_auth_required", "what": "a sign-in the runtime needs: not a network failure",
     "runtimes": [rt(final(), start=[{"raise": {"class": "RuntimeUnavailable", "message": "sign in first",
                                                "reason": "auth_required"}}], name="claude")]},
    {"name": "start_unavailable_offline", "what": "a start failure while this Mac has no network route",
     "route": False,
     "runtimes": [rt(final(), start=[{"raise": {"class": "RuntimeUnavailable", "message": "connection refused"}}])]},
    {"name": "make_signed_out", "what": "the runtime can't be built: FERB's sign-in is missing",
     "runtimes": [{"make_raise": {"class": "SignedOut", "message": "Claude isn't signed in for FERB",
                                  "runtime": "claude"}}]},
    {"name": "make_raises_bug", "what": "the runtime factory raises something unexpected",
     "runtimes": [{"make_raise": {"class": "KeyError", "message": "worker_type"}}]},
    {"name": "start_permission", "what": "macOS refuses an operation the start needed",
     "runtimes": [rt(final(), start=[{"raise": {"class": "PermissionError", "errno": 1,
                                                "strerror": "Operation not permitted",
                                                "filename": "/FX/home/run/HIDDEN-sock"}}])]},
    {"name": "start_os_error", "what": "another OS error at start, no file",
     "runtimes": [rt(final(), start=[{"raise": {"class": "OSError", "errno": 5,
                                                "strerror": "Input/output error"}}])]},
    {"name": "start_refused", "what": "FERB's protection rules at start",
     "runtimes": [rt(final(), start=[{"raise": {"class": "Refused", "message": "the workspace is protected"}}])]},
    {"name": "start_no_target", "what": "NoTarget is not a protection refusal",
     "runtimes": [rt(final(), start=[{"raise": {"class": "NoTarget", "message": "no such tab"}}])]},
    {"name": "start_ferb_error_blank", "what": "a FERB error with no words is a bug by class",
     "runtimes": [rt(final(), start=[{"raise": {"class": "Unsupported", "message": "  "}}])]},
    {"name": "limits_unknown", "what": "a runtime that reports no context window",
     "runtimes": [rt(final(), limits=None)]},
    {"name": "input_too_large", "what": "the context is over the runtime's per-input character cap",
     "runtimes": [rt(final(), limits={**SMALL, "context_window": 400000, "max_input_chars": 300})]},
    {"name": "budget_omits", "what": "a small window: optional context left out, the reserve counted",
     "ctx": DEFAULT_CTX + [{"text": "Optional notes " + "x" * 2600, "reference": "notes", "priority": 1}],
     "runtimes": [rt(usage(), final(), limits={**SMALL, "context_window": 30000, "max_output": 500})]},
    {"name": "error_overflow", "what": "the runtime's window-exceeded error, as marked",
     "runtimes": [rt({"event": "error", "data": {"message": "context_length_exceeded", "overflow": True}})]},
    {"name": "error_image", "what": "the provider refused the image, as marked",
     "runtimes": [rt({"event": "error", "data": {"message": "image not supported", "image": True}})]},
    {"name": "error_empty", "what": "no answer twice, as marked",
     "runtimes": [rt({"event": "error", "data": {"message": "the model ended without an answer twice",
                                                 "empty": True}})]},
    {"name": "error_account", "what": "an account fact from the stream",
     "runtimes": [rt({"event": "error", "data": {"message": "usage limit", "account": "usage_limit"}}, name="codex")]},
    {"name": "error_account_other", "what": "an account word FERB doesn't know: not a network failure, no offer",
     "runtimes": [rt({"event": "error", "data": {"message": "billing", "account": "billing_hold"}})]},
    {"name": "error_plain", "what": "the service's own error",
     "runtimes": [rt({"event": "error", "data": {"message": "upstream 502 HIDDEN-abc"}})]},
    {"name": "error_no_message", "what": "an error event without words",
     "runtimes": [rt({"event": "error", "data": {}})]},
    {"name": "error_plain_offline", "what": "the service's error while offline", "route": False,
     "runtimes": [rt({"event": "error", "data": {"message": "stream dropped"}})]},
    {"name": "violation_tools", "what": "the runtime used tools FERB never offered",
     "runtimes": [rt({"event": "violation", "data": {"unexpected_tools": ["web_search", "bash"]}})]},
    {"name": "violation_auth", "what": "the runtime signed in a way FERB doesn't allow",
     "runtimes": [rt({"event": "violation", "data": {"auth": "api key", "n": 1.5}})]},
    {"name": "violation_other", "what": "a violation naming neither",
     "runtimes": [rt({"event": "violation", "data": {"what": "wrote outside its folder", "flag": None}})]},
    {"name": "tool_loop_made_up", "what": "ten calls to tools the run wasn't offered stop the run",
     "runtimes": [rt(*[{"event": "tool_call", "data": {"name": "nope", "offered": False}}] * 10, final())]},
    {"name": "tool_loop_refused", "what": "lookups the gateway refused count too",
     "runtimes": [rt({"event": "tool_call", "data": {"name": "nope", "offered": False}}, {"refused": 9},
                     {"event": "progress", "data": {}}, final())]},
    {"name": "tool_loop_not_on_final", "what": "the cap isn't checked on the final event",
     "calls_refused": 12, "runtimes": [rt(final())]},
    {"name": "events_raise_value", "what": "the adapter raises a ValueError of its own mid-run",
     "runtimes": [rt({"event": "message", "data": {"text": "x"}},
                     {"raise": {"class": "ValueError", "message": "codex: bad frame"}})]},
    {"name": "events_raise_unavailable", "what": "RuntimeUnavailable after the start is the run's own words",
     "runtimes": [rt({"raise": {"class": "RuntimeUnavailable", "message": "the app-server exited"}})]},
    {"name": "events_raise_runtime_error", "what": "something unexpected mid-run",
     "runtimes": [rt({"raise": {"class": "RuntimeError", "message": "x" * 400}})]},
    {"name": "events_raise_plan_rejected", "what": "a FERB error that is also a ValueError",
     "runtimes": [rt({"raise": {"class": "PlanRejected", "problems": ["step 2 has no goal", "HIDDEN-zz leaked"]}})]},
    {"name": "retrying_then_answer", "what": "the runtime retries a lost connection; the owner hears once; then it's "
     "back", "tree": {"live": True},
     "runtimes": [rt({"event": "retrying", "data": {"message": "reconnecting 1/5"}},
                     {"event": "retrying", "data": {"message": "reconnecting 2/5."}},
                     {"event": "message", "data": {"text": "back"}}, usage(), final())]},
    {"name": "retrying_offline", "what": "retries while the Mac has no route", "tree": {"live": True},
     "route": False,
     "runtimes": [rt({"event": "retrying", "data": {}}, usage(), final())]},
    {"name": "stall_only_retries", "what": "a run that only retried for the stall window",
     "runtimes": [rt({"event": "retrying", "data": {"message": "stream disconnected."}}, {"advance": 181},
                     {"stall": True})]},
    {"name": "stall_after_progress", "what": "a run that went quiet after answering in part",
     "runtimes": [rt({"event": "text_delta", "data": {"text": "half"}}, {"advance": 200}, {"stall": True})]},
    {"name": "stall_at_start", "what": "a start that never answers is stopped by the watchdog",
     "runtimes": [rt(final(), start=[{"advance": 181}, {"hang": True}])]},
    {"name": "compacted", "what": "the runtime compressed the run's memory",
     "runtimes": [rt({"event": "compacted", "data": {"context_tokens_before": 120000, "window": None}}, usage(),
                     final())]},
    {"name": "tools_not_loaded_rerun", "what": "the runtime never loaded FERB's tools: rerun once with that fact",
     "runtimes": [rt(usage(), final(tools_seen=["shell", "web_search"], returncode=0)),
                  rt(usage(), final(tools_seen=["ferb__fs_read"], returncode=None))]},
    {"name": "tools_loaded", "what": "FERB's tools were seen: no rerun",
     "runtimes": [rt(final(tools_seen=["mcp__ferb__fs_read"]))]},
    {"name": "tools_not_loaded_no_tools", "what": "no tools offered: no rerun", "tools": [],
     "runtimes": [rt(final(tools_seen=[]))]},
    {"name": "checker", "what": "the checker: observe-only token, browser observations, the checker runtime role",
     "prompt": "verifier.md", "schema": CHECKER_SCHEMA, "effort": "high",
     "task": {"id": "t1.worker.verify", "role": "verifier"}, "tree": {"runtime": "claude"},
     "runtimes": [rt(usage(), final(json.dumps({"results": [], "findings": [], "next": "accept"})))]},
    {"name": "checker_default_runtime", "what": "the checker with no runtime switch",
     "prompt": "verifier.md", "schema": CHECKER_SCHEMA,
     "task": {"id": "t1.worker.verify", "role": "verifier"},
     "runtimes": [rt(final(json.dumps({"results": []})))]},
    {"name": "fix_turn_observe_only", "what": "a worker's fix turn: observe-only, its roles",
     "prompt": "worker.md", "roles": ["fix"], "observe_only": True, "task": {"id": "t1.worker.fix"},
     "runtimes": [rt(final(json.dumps({"answer": "fixed"})))]},
    {"name": "start_picture", "what": "the request's worker starts from what the owner saw at send",
     "tree": {"start_block": {"text": "Front window at send: TextEdit — report.txt", "reference": "screen",
                              "protected": True},
              "start_image": {"data": "ffd8ffe000104a464946", "ref": "ev-shot-1"}, "runtime": "codex",
              "worker_type": "codex"},
     "runtimes": [rt(usage(), final())]},
    {"name": "deadline_and_signals", "what": "a deadline and the owner's activity in the time facts",
     "task": {"deadline": "2026-01-01T09:30:00+00:00"},
     "signals": ["2026-01-01T08:50:00+00:00", "2026-01-01T07:00:00+00:00"],
     "runtimes": [rt({"advance": 65}, usage(), final())]},
    {"name": "deadline_past", "what": "a deadline already past: the token still lives a minute",
     "task": {"deadline": "2026-01-01T08:00:00+00:00"}, "runtimes": [rt(final())]},
    {"name": "start_state", "what": "the runtime's own start steps and the context role it reports",
     "runtimes": [rt(usage(), final(), state={"start_steps": {"spawn": 12, "thread": 3},
                                              "context_role": "developer"})]},
    {"name": "newly_always_on", "what": "Tier 1's newly-on entries join after the cached part",
     "system_inputs": {"identity_pending": "Newly always on (MEM-429):\n- Prefers metric units."},
     "runtimes": [rt(final())]},
    {"name": "no_catalog_role", "what": "roles and a role-less prompt name: system assembly",
     "roles": ["step", "inputs"], "system_inputs": {"facts": "", "live_facts": "", "identity_context": ""},
     "runtimes": [rt(final())]},
    {"name": "redacted_reason", "what": "the reason's words are redacted before they're cut",
     "runtimes": [rt({"raise": {"class": "KeyError", "message": "HIDDEN-abcdef " + "word " * 80}})]},
]

CALL_SCENARIOS: list[dict[str, Any]] = [
    {"name": "planner_turn", "what": "a planner-like call: deltas, a tool call restarting the draft, usage, timing",
     "retry_silent": False, "token": True, "callbacks": ["delta", "usage", "tool", "timing"],
     "task": {"id": "req-9", "request_id": "req-9", "role": "mixed"},
     "runtimes": [rt({"event": "text_delta", "data": {"text": "Hel"}}, {"advance": 0.75},
                     {"event": "tool_call", "data": {"name": "memory.read", "offered": True}},
                     {"event": "text_delta", "data": {"text": "lo", "restart": True}},
                     usage(cost=None), {"advance": 0.25}, final("{\"kind\": \"reply\"}"))]},
    {"name": "call_with_task_span", "what": "a call whose task id differs from its request",
     "callbacks": ["usage"], "task": {"id": "t5.summary", "request_id": "req-5", "role": "mixed"},
     "runtimes": [rt(usage(1, 2, cost=0), final("summary"))]},
    {"name": "silent_then_answer", "what": "a call silent from its start is tried once more",
     "task": {"id": "t5.summary", "request_id": "req-5", "role": "mixed"},
     "runtimes": [calls(rt({"advance": 181}, {"stall": True}), rt(usage(), final("ok")))]},
    {"name": "silent_twice", "what": "silent twice: the service's failure",
     "task": {"id": "t5.summary", "request_id": "req-5", "role": "mixed"},
     "runtimes": [calls(rt({"advance": 181}, {"stall": True}), rt({"advance": 181}, {"stall": True}))]},
    {"name": "silent_after_output", "what": "went quiet after answering in part: no second try",
     "task": {"id": "req-5", "request_id": "req-5", "role": "mixed"},
     "runtimes": [rt({"event": "text_delta", "data": {"text": "a"}}, {"advance": 181}, {"stall": True})]},
    {"name": "silent_start_twice", "what": "a start that never answers, twice",
     "task": {"id": "t5.summary", "request_id": "req-5", "role": "mixed"},
     "runtimes": [rt(final(), start=[{"advance": 181}, {"hang": True}])]},
    {"name": "planner_silent", "what": "the planner's turn: a silence is not tried again (ORCH-528)",
     "retry_silent": False, "task": {"id": "req-9", "request_id": "req-9", "role": "mixed"},
     "runtimes": [rt({"advance": 181}, {"stall": True})]},
    {"name": "notify_connection", "what": "a chat turn's live line hears of a lost connection",
     "callbacks": ["notify"], "task": {"id": "req-9", "request_id": "req-9", "role": "mixed"},
     "runtimes": [rt({"event": "retrying", "data": {"message": "lost"}}, {"event": "retrying", "data": {}},
                     {"event": "usage", "data": {"input_tokens": 1}}, final("x"))]},
    {"name": "with_tree_no_worker", "what": "a call with a tree but no worker (a dispatch): its corrections",
     "tree": {"steering": ["Use the April numbers."], "live": True},
     "task": {"id": "t1.question", "request_id": "req-1", "role": "mixed"},
     "runtimes": [rt({"event": "retrying", "data": {"message": "x"}}, final("y"), **{"continue": "accept"})]},
    {"name": "call_images", "what": "images: their evidence rows in the trace, never their bytes",
     "images": [{"data": "89504e47", "ref": "ev-1"}, {"data": "ffd8", "ref": "ev-2"}],
     "task": {"id": "req-9", "request_id": "req-9", "role": "mixed"},
     "runtimes": [rt(final("ok"))]},
    {"name": "call_invalid_final", "what": "the call's schema check",
     "task": {"id": "req-9", "request_id": "req-9", "role": "mixed"},
     "runtimes": [rt(final("{\"kind\": 3}"), output_schema={"type": "object", "properties": {
         "kind": {"type": "string"}}, "required": ["kind"]})]},
    {"name": "call_no_answer", "what": "a call that ends without an answer",
     "task": {"id": "req-9", "request_id": "req-9", "role": "mixed"},
     "runtimes": [rt({"event": "progress", "data": {}})]},
    {"name": "call_start_raises", "what": "an exception from the start of a non-worker call",
     "task": {"id": "req-9", "request_id": "req-9", "role": "mixed"},
     "runtimes": [rt(final(), start=[{"raise": {"class": "RuntimeUnavailable", "message": "no login",
                                                "account": "signed_out", "reason": "auth_required"}}])]},
    {"name": "call_too_large", "what": "too large for the runtime: refused before sending",
     "callbacks": ["timing"], "task": {"id": "req-9", "request_id": "req-9", "role": "mixed"},
     "runtimes": [rt(final(), limits={**SMALL, "context_window": 400000, "max_input_chars": 50})]},
]

TOOL_PROGRESS_CASES: list[dict[str, Any]] = [
    {"name": "no_task", "grant": {"task_id": "nope", "worker_id": "w-x"}, "tool": "fs.read",
     "args": {"path": "/a"}, "kind": "read"},
    {"name": "fs_read", "tool": "fs.read", "args": {"path": "/FX/home/a b.txt"}, "kind": "read"},
    {"name": "mac_app_known", "tool": "mac.open", "args": {"app": "com.example.notes"}, "kind": "open"},
    {"name": "mac_app_running", "tool": "mac.act", "args": {"bundle_id": "com.apple.finder"}, "kind": "act"},
    {"name": "mac_site", "tool": "mac.snapshot", "args": {}, "kind": "read", "site": "TextEdit"},
    {"name": "apps_raise", "tool": "mac.open", "args": {"app": "x"}, "kind": "open",
     "apps_raise": {"class": "OSError", "errno": 5, "strerror": "boom"}},
    {"name": "redact_raises", "tool": "fs.read", "args": {"path": "/a"}, "kind": "read",
     "redact_raises": {"class": "TypeError", "message": "no"}},
    {"name": "shell_own", "tool": "shell.wait", "args": {"id": "p-1"}, "kind": "",
     "shells": {"p-1": {"owner_task": "t1.worker", "command": "make all", "started": 1767254400}}},
    {"name": "shell_other_task", "tool": "shell.read", "args": {"id": "p-1"}, "kind": "",
     "shells": {"p-1": {"owner_task": "t2", "command": "make all", "started": 1767254400}}},
    {"name": "shell_bad_start", "tool": "shell.wait", "args": {"id": "p-1"}, "kind": "",
     "shells": {"p-1": {"owner_task": "t1.worker", "command": "make all", "started": "soon"}}},
    {"name": "shell_no_start", "tool": "shell.wait", "args": {"id": "p-1"}, "kind": "",
     "shells": {"p-1": {"owner_task": "t1.worker", "command": "", "started": None}}},
    {"name": "shell_id_not_text", "tool": "shell.wait", "args": {"id": 7}, "kind": ""},
    {"name": "browser_site", "tool": "ferb_browser.open", "args": {"url": "https://www.example.org/x"},
     "kind": "navigate", "site": "example.org"},
    {"name": "connection_back", "tool": "fs.read", "args": {"path": "/a"}, "kind": "read", "lost": True},
    {"name": "secret_in_args", "tool": "fs.write", "args": {"path": "/FX/HIDDEN-1 note.txt"}, "kind": "write"},
]


async def run_tool_progress(case: dict[str, Any]) -> dict[str, Any]:
    host = Host(case)
    tree = make_tree(host, {"live": True})
    host._tree_of["t1.worker"] = tree
    grant = SimpleNamespace(**{"task_id": "t1.worker", "worker_id": "w-1", **case.get("grant", {})})
    if case.get("lost"):
        notice = W.ConnectionNotice("codex", lambda e: host.rec("notice", jsonable(e)),
                                    lambda: host.clock.now().isoformat(), host.gateway.redact)
        notice.lost = True
        host._connection["w-1"] = notice
    host.clock.advance(125.5)
    host.tool_progress(grant, case["tool"], case["args"], case["kind"], case.get("site"))
    return {"log": host.log, "live": live_lines(host), "last_steps": jsonable(host._last_steps)}


def reason_cases() -> list[dict[str, Any]]:
    """`reason`, `couldnt` and the owner's lines on every exception class the worker and the supervisor hand them."""
    out = []
    specs: list[Any] = [
        ("ServiceError", lambda: W.ServiceError("upstream said no", "Codex")),
        ("ServiceError net online", lambda: W.ServiceError("stream dropped", "Claude", network=True)),
        ("ContextOverflow", lambda: W.ContextOverflow("too long", "codex")),
        ("ImageRefused", lambda: W.ImageRefused("no images", "claude")),
        ("InputTooLarge", lambda: InputTooLarge(1234567, 1048576)),
        ("PermissionError file", lambda: PermissionError(13, "Permission denied", "/FX/a b/c.txt")),
        ("PermissionError bytes file", lambda: PermissionError(1, "Operation not permitted", b"/FX/caf\xc3\xa9")),
        ("PermissionError no file", lambda: PermissionError(13, "Permission denied")),
        ("OSError EACCES", lambda: OSError(errno.EACCES, "denied", "/x")),
        ("OSError no strerror", lambda: OSError("plain words")),
        ("OSError ENOENT", lambda: FileNotFoundError(2, "No such file or directory", "/FX/missing")),
        ("OSError ENOENT no file", lambda: OSError(28, "No space left on device")),
        ("Refused", lambda: Refused("the folder is protected")),
        ("Refused ends", lambda: Refused("the folder is protected!")),
        ("Refused empty", lambda: Refused("")),
        ("NoTarget", lambda: NoTarget("no such window")),
        ("AuthError", lambda: AuthError("run token revoked")),
        ("RuntimeViolation tools", lambda: W.RuntimeViolation("codex: {...}", "codex",
                                                             {"unexpected_tools": ["a", "b"]})),
        ("RuntimeViolation tools str", lambda: W.RuntimeViolation("x", "", {"unexpected_tools": "web"})),
        ("RuntimeViolation auth", lambda: W.RuntimeViolation("x", "claude", {"auth": "key"})),
        ("RuntimeViolation plain", lambda: W.RuntimeViolation("claude: odd", "claude", {})),
        ("PlanRejected", lambda: PlanRejected(["no goal", "two answers"])),
        ("InvalidAnswer", lambda: InvalidAnswer("the answer is not JSON (Expecting value)")),
        ("FerbError", lambda: FerbError("the profile is busy")),
        ("FerbError blank", lambda: FerbError("   ")),
        ("RuntimeUnavailable", lambda: RuntimeUnavailable("Codex isn't installed")),
        ("Unsupported", lambda: Unsupported("no same-run input")),
        ("EmptyAnswer", lambda: EmptyAnswer("the model gave no answer twice", "codex")),
        ("ToolLoop", lambda: ToolLoop("claude", 10)),
        ("KeyError", lambda: KeyError("k")),
        ("ValueError empty", lambda: ValueError()),
        ("TypeError long", lambda: TypeError("word " * 100)),
        ("RuntimeError multiline", lambda: RuntimeError("line one\n\tline  two HIDDEN-x")),
        ("Stalled", lambda: W.Stalled(progressed=False)),
    ]
    for name, make in specs:
        e = make()
        row: dict[str, Any] = {"name": name, "exc": exc_json(e), "reason": W.reason(e),
                               "reason_redacted": W.reason(e, redact_text),
                               "couldnt": W.couldnt("finish the step", e, redact_text)}
        out.append(row)
    for name, data in [("WorkerReport missing", {"status": "done"}), ("WorkerReport wrong", {"status": "maybe",
                                                                                            "answer": 3}),
                       ("WorkerReport not dict", [1])]:
        try:
            WorkerReport.model_validate(data)
        except ValidationError as e:
            out.append({"name": name, "validation": {"model": "WorkerReport", "input": data}, "exc": exc_json(e),
                        "reason": W.reason(e), "reason_redacted": W.reason(e, redact_text),
                        "couldnt": W.couldnt("read the report", e, redact_text)})
    return out


def line_cases() -> dict[str, Any]:
    out: dict[str, Any] = {"service_problem": [], "retry_stall_problem": [], "connection_lost": [],
                           "account_problem": [], "too_large_problem": []}
    for route in (True, False):
        ROUTE["has"] = route
        for msg, rtn, net in [("bad gateway", "Codex", False), ("dropped", "Claude", True), ("", "fake", True)]:
            out["service_problem"].append({"route": route, "args": [msg, rtn, net],
                                           "text": W.service_problem(msg, rtn, net)})
        for rtn, msg, s in [("Codex", "reconnecting.", 180.0), ("claude", "x", 0.5), ("c", "", 1e16)]:
            out["retry_stall_problem"].append({"route": route, "args": [rtn, msg, s],
                                               "text": W.retry_stall_problem(rtn, msg, s)})
        for rtn, msg, task in [("Codex", "stream lost.", True), ("Claude", "", False), ("x", "a..", True)]:
            out["connection_lost"].append({"route": route, "args": [rtn, msg, task],
                                           "text": W.connection_lost(rtn, msg, task)})
    ROUTE["has"] = True
    for rtn in ("codex", "claude", "anthropic", "openai", "google", "gemini", "other"):
        for problem in ("usage_limit", "signed_out", ""):
            out["account_problem"].append({"args": [rtn, problem], "text": W.account_problem(rtn, problem)})
    for chars, mx, rtn in [(1234567, 1048576, "codex"), (5, 4, ""), (10**12, 999, "claude")]:
        e = InputTooLarge(chars, mx)
        e.runtime = rtn
        out["too_large_problem"].append({"args": [chars, mx, rtn], "text": W.too_large_problem(e)})
    out["reserve_tokens"] = []
    for system, tools, schema in [("abc", [], None), ("café", TOOLS, WORKER_SCHEMA), ("", [{"a": 1e16}], {})]:
        out["reserve_tokens"].append({"system": system, "tools": tools, "schema": schema_ref(schema),
                                      "tokens": W.reserve_tokens(system, tools, schema)})
    out["constants"] = {"NO_TOOLS_LOADED": W.NO_TOOLS_LOADED, "BUG": W.BUG, "REASON_CHARS": W.REASON_CHARS,
                        "ACCOUNT_PROBLEMS": list(W.ACCOUNT_PROBLEMS)}
    return out


async def main() -> None:
    fixture: dict[str, Any] = {
        "about": "See README.md. Made by make.py from the Python build; every seam call of each run is in `log`.",
        "tools": TOOLS, "observe": sorted(OBSERVE), "system_inputs": SYSTEM_INPUTS, "redactor": "HIDDEN- words",
        "worker": [], "calls": [], "tool_progress": [],
    }
    for sc in WORKER_SCENARIOS:
        rec = await run_worker_scenario(sc)
        fixture["worker"].append({"name": sc["name"], "what": sc["what"], "scenario": jsonable(sc), **rec})
    for sc in CALL_SCENARIOS:
        rec = await run_call_scenario(sc)
        fixture["calls"].append({"name": sc["name"], "what": sc["what"], "scenario": jsonable(sc), **rec})
    for case in TOOL_PROGRESS_CASES:
        fixture["tool_progress"].append({"case": case, **(await run_tool_progress(case))})
    fixture["reasons"] = reason_cases()
    fixture["lines"] = line_cases()
    fixture["schemas"] = SCHEMAS
    fixture = intern(fixture)
    text = json.dumps(fixture, ensure_ascii=False, indent=1) + "\n"
    (HERE / "worker.json").write_text(text, encoding="utf-8")
    (HERE / "bodies.json").write_text(json.dumps(dict(sorted(BODIES.items())), ensure_ascii=False, indent=0) + "\n",
                                      encoding="utf-8")
    print(f"worker.json: {len(fixture['worker'])} worker runs, {len(fixture['calls'])} calls, "
          f"{len(fixture['tool_progress'])} tool lines, {len(fixture['reasons'])} reasons, "
          f"{len(text):,} bytes; bodies {len(BODIES)}")


if __name__ == "__main__":
    asyncio.run(main())
