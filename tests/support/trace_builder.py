"""Build `trace.get`-shaped dicts ({task_id, spans, outcome}) for composer and renderer tests, with the span kinds,
inputs and outputs FERB records (supervisor, worker, gateway, disturbance) and SpanStore.outcome()'s keys."""
import json
import re
from datetime import datetime, timedelta, timezone
from typing import Any

T0 = datetime(2026, 10, 7, 11, 4, 37, tzinfo=timezone(timedelta(hours=5, minutes=30)))


class TraceBuilder:
    def __init__(self, task_id: str = "f29fee46", request: str = "delete the hf cache completely",
                 goal: str = "delete ~/.cache/huggingface", conditions: list[str] | None = None) -> None:
        self.task_id, self.request, self.goal = task_id, request, goal
        self.conditions = conditions or ["~/.cache/huggingface no longer exists", "nothing else was deleted"]
        self.t, self.n = T0, 0
        self.top: list[dict[str, Any]] = []                 # spans without a parent (the intake planner)
        self.root = self._node("task", 0, {"request": request, "task_spec": self._spec()})
        self.verification: list[dict[str, Any]] = []
        self.verdict: dict[str, Any] | None = None

    def _spec(self) -> dict[str, Any]:
        return {"goal": self.goal, "success_conditions": [{"text": c, "check": {"kind": "none"}} for c in self.conditions]}

    def _node(self, kind: str, seconds: float, inp: dict[str, Any], out: dict[str, Any] | None = None,
              error: str | None = None, start: datetime | None = None, task_id: str | None = "") -> dict[str, Any]:
        self.n += 1
        begin = start or self.t
        end = begin + timedelta(seconds=seconds)
        if start is None:
            self.t = end
        return {"id": f"s{self.n}", "parent_id": None, "kind": kind,
                "task_id": self.task_id if task_id == "" else task_id, "device_id": "dev",
                "started_at": begin.isoformat(), "ended_at": end.isoformat(), "input": inp, "output": out,
                "cost_usd": None, "error": error, "origin": None, "summary": None, "children": []}

    def _child(self, parent: dict[str, Any], node: dict[str, Any]) -> dict[str, Any]:
        node["parent_id"] = parent["id"]
        parent["children"].append(node)
        return node

    def planner(self, seconds: float, decision: dict[str, Any]) -> dict[str, Any]:
        node = self._node("model.turn", seconds, {"role": "supervisor", "request_id": "req-1"},
                          {"text": json.dumps(decision)}, task_id=None)
        self.top.append(node)
        return node

    def usage(self, parent: dict[str, Any], inp: int, cached: int, out: int, cost: float | None,
              billing: str = "subscription", model: str = "gpt-5.5") -> None:
        start = datetime.fromisoformat(parent["started_at"])
        self._child(parent, self._node("model.usage", 0, {"runtime": "codex"}, {
            "model": model, "input_tokens": inp, "cached_input_tokens": cached, "cache_write_tokens": 0,
            "output_tokens": out, "reasoning_tokens": 0, "cost_usd": cost,
            "cost_basis": "price_table" if cost is not None else "unknown", "billing": billing},
            start=start, task_id=parent["task_id"]))

    def worker(self, seconds: float, status: str, summary: str) -> dict[str, Any]:
        report = {"status": status, "summary": summary, "artifacts": [], "evidence": []}
        return self._child(self.root, self._node("worker", seconds, {"goal": self.goal, "role": "worker", "scopes": []},
                                                 {"result": json.dumps(report), "status": status},
                                                 task_id=f"{self.task_id}.worker"))

    def checker(self, seconds: float, met: list[bool | None], evidence: list[str]) -> dict[str, Any]:
        results = [{"index": i + 1, "met": m, "evidence": e} for i, (m, e) in enumerate(zip(met, evidence))]
        self.verification = [{"index": r["index"], "condition": c, "met": r["met"], "path": None,
                              "evidence": r["evidence"]} for r, c in zip(results, self.conditions)]
        return self._child(self.root, self._node("worker", seconds,
                                                 {"goal": "verify", "role": "verifier", "scopes": []},
                                                 {"result": json.dumps({"results": results}), "status": None},
                                                 task_id=f"{self.task_id}.worker.verify"))

    def dispatch(self, seconds: float, action: str, instructions: str = "") -> dict[str, Any]:
        decision = {"action": action, "worker_type": "default", "instructions": instructions, "question": ""}
        return self._child(self.root, self._node("model.dispatch", seconds, {}, {"text": json.dumps(decision)}))

    def question(self, seconds: float, decision: dict[str, Any]) -> dict[str, Any]:
        """The orchestrator resolving a worker's question: recorded without a parent (supervisor.route_question)."""
        node = self._node("model.question", seconds, {"question": "which folder?"}, {"decision": json.dumps(decision)},
                          task_id=f"{self.task_id}.worker")
        self.top.append(node)
        return node

    def reconcile(self, seconds: float, status: str, summary: str) -> dict[str, Any]:
        """The orchestrator's final-state review after resource cleanup (supervisor._reconcile_resources)."""
        report = {"status": status, "summary": summary, "artifacts": [], "evidence": []}
        observed = {"report": {}, "resources_after_cleanup": []}
        return self._child(self.root, self._node("model.resources", seconds, observed, {"text": json.dumps(report)},
                                                 task_id=f"{self.task_id}.worker.resources"))

    def tool(self, parent: dict[str, Any], offset: float, seconds: float, tool: str, args: dict[str, Any],
             summary: str, origin: str | None = "private", why: str = "", error: str | None = None,
             kind: str | None = "run_shell", output: dict[str, Any] | None = None) -> str:
        """`origin` None: recorded before F3 labeled results; `kind` is the action kind the gateway records (None:
        recorded before F0 kept it); `output` defaults to a shell result, and an error path records none."""
        start = datetime.fromisoformat(parent["started_at"]) + timedelta(seconds=offset)
        inp = {"tool": tool, "args": args, "effect": ""} | ({"kind": kind} if kind else {}) | {"target": {}}
        node = self._node("tool.call", seconds, inp | ({"why": why} if why else {}),
                          None if error else (output or {"exit_code": 0}), error, start=start,
                          task_id=parent["task_id"])
        node["origin"], node["summary"] = origin, summary
        self._child(parent, node)
        return str(node["id"])

    def source(self, label: str, text: str, origin: str | None) -> None:
        """A context block the task's models were shown, stored with where it came from (supervisor._source_block)."""
        node = self._node("context.source", 0, {"label": label}, {"text": text})
        node["origin"] = origin
        self._child(self.root, node)

    def desktop(self, changes: int, near: list[tuple[Any, ...]] | tuple = (),
                on_screen: dict[str, bool] | None = None) -> None:
        """`near`: (tool, changed keys[, the tool.call span id the sampler saw in flight]). `on_screen`: the flag the
        sentinel recorded per tool (left out: a record made before the flag existed)."""
        for tool, changed, *span in near:
            action: dict[str, Any] = {"tool": tool, "span_id": span[0] if span else "x"}
            if on_screen is not None and tool in on_screen:
                action["on_screen"] = on_screen[tool]
            self._child(self.root, self._node("disturbance.change", 0, {}, {
                "changes": {k: {"before": None, "after": None} for k in changed},
                "actions": [action], "owner_input": False}))
        self._child(self.root, self._node("disturbance.summary", 0, {}, {"samples": 99, "changes": changes, "errors": 0}))

    def came_forward(self, app: str, bundle_id: str, actions: list[dict[str, Any]] | tuple = (),
                     processes: list[dict[str, Any]] | tuple | None = (), space: bool = False,
                     on_screen_in_task: bool = False, last_ended_s: int | None = None) -> None:
        """A disturbance.change where an app came to the front (and the Space changed when `space`), with the actions,
        the FERB-started processes and the on-screen timing the sentinel recorded (`processes` None: a record made
        before processes and timing were kept)."""
        changes: dict[str, Any] = {"frontmost_app": {
            "before": {"pid": 1, "bundle_id": "com.example.before", "name": "Before"},
            "after": {"pid": 2, "bundle_id": bundle_id, "name": app}}}
        if space:
            changes["active_space_epoch"] = {"before": 0, "after": 1}
        out: dict[str, Any] = {"changes": changes, "actions": list(actions), "owner_input": False}
        if processes is not None:
            out["processes"] = list(processes)
            if not any(a.get("on_screen") for a in actions):
                out |= {"on_screen_in_task": on_screen_in_task, "last_on_screen_ended_s": last_ended_s}
        self._child(self.root, self._node("disturbance.change", 0, {}, out))

    def front(self, before: dict[str, Any] | None, after: dict[str, Any] | None,
              actions: list[dict[str, Any]] | tuple = ()) -> None:
        """A disturbance.change of the front app with the exact readings the sentinel took (None: unreadable)."""
        out: dict[str, Any] = {"changes": {"frontmost_app": {"before": before, "after": after}},
                               "actions": list(actions), "owner_input": False, "processes": [],
                               "on_screen_in_task": False, "last_on_screen_ended_s": None}
        if after is None:
            out["front_app_unreadable"] = True
        self._child(self.root, self._node("disturbance.change", 0, {}, out))

    def paused(self, seconds: float, resumed: bool = True) -> None:
        """`ferb pause` during the task for `seconds`, then `ferb resume` (unless `resumed` is False)."""
        self._child(self.root, self._node("disturbance.paused", seconds, {}, {"reason": "sensing paused"}))
        if resumed:
            self._child(self.root, self._node("disturbance.resumed", 0, {}, {}))

    def finish(self, status: str = "done", verified: bool | None = True,
               answer: str = "Deleted ~/.cache/huggingface.") -> dict[str, Any]:
        self.root["ended_at"] = self.t.isoformat()
        self.root["output"] = {"status": status, "summary": answer, "verified": verified, "resources": []}
        message = {"id": "m1", "at": T0.isoformat(),
                   "data": {"session_id": "s1", "request_id": "req-1", "text": self.request, "untrusted_parts": 0}}
        return {"task_id": self.task_id, "spans": self.top + [self.root], "outcome": {
            "schema_version": 1, "task_id": self.task_id,
            "request": {"id": "req-1", "text": self.request, "authority_words": _typed(self.request),
                        "messages": [message]},
            "task_spec": self._spec(),
            "tasks": [{"id": self.task_id, "tree_id": self.task_id, "parent_id": None, "goal": self.goal,
                       "status": status}],
            "result": self.root["output"],
            "outcome_records": [
                {"task_id": self.task_id, "request_id": "req-1", "status": status, "text": answer},
                {"task_id": self.task_id, "status": status, "summary": answer, "verified": verified,
                 "verification": self.verification, "subtasks": [], "resources": []}],
            "owner_verdict": self.verdict}}


def _typed(request: str) -> str:
    """The owner's own words of a request, as intake records them: pasted parts removed, whitespace collapsed."""
    return " ".join(re.sub(r'<untrusted-data id="([0-9a-f]+)">.*?</untrusted-data id="\1">', " ", request,
                           flags=re.S).split())


def hf_cache_retry() -> dict[str, Any]:
    """The f29fee46 case from the plan's mockup: a checker that verified nothing, a retry, then a verified finish."""
    b = TraceBuilder()
    p = b.planner(12, {"kind": "task", "text": "Deleting it.", "task": {"goal": b.goal}})
    b.usage(p, 9000, 6000, 300, 0.02)
    w = b.worker(46, "done", "Deleted ~/.cache/huggingface (22 GB); it no longer exists.")
    b.tool(w, 1, 0.4, "shell.run", {"command": "du -sh ~/.cache/huggingface"}, "exit 0 · 1 line", why="check size first")
    b.tool(w, 3, 2.1, "shell.run", {"command": "rm -rf ~/.cache/huggingface"}, "exit 0 · 0 lines",
           why="delete the exact path")
    b.usage(w, 300000, 250000, 3000, 0.40)
    c = b.checker(8, [None, None], ["Tool discovery was not attempted", "Tool discovery was not attempted"])
    b.usage(c, 40000, 30000, 400, 0.05)
    d = b.dispatch(9, "retry", "verify only, delete nothing more")
    b.usage(d, 24000, 21000, 200, 0.01)
    w2 = b.worker(37, "done", "Re-verified: the folder is gone and nothing else was removed.")
    b.usage(w2, 250000, 230000, 2500, 0.30)
    c2 = b.checker(31, [True, True], ["fs.list shows no huggingface folder", "ledger shows one deletion only"])
    b.usage(c2, 77000, 43000, 600, 0.06)
    b.desktop(36)
    return b.finish()
