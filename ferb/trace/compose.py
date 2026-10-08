"""Owns composing one task's permanent record from its recorded spans and journal facts (L1, P26, RQ1–RQ4). Must not
call a model or add anything the records don't say; a worker's words stay a claim.

Input: the `trace.get` shape {task_id, spans: [forest of span dicts], outcome: SpanStore.outcome()}."""

import json
import re
from datetime import datetime
from typing import Any, Iterator

from ferb.core import origin
from ferb.trace.lines import clip, duration, plural

MODEL_KINDS = {"model.turn", "model.dispatch", "model.question", "model.resources", "worker"}
PLANNER_KINDS = ("model.turn", "model.dispatch", "model.question", "model.resources")   # the orchestrator's turns
OPEN, OPEN_TEXT = "⏹", "did not finish (interrupted)"   # a step span never finished: no claim, no check, no time
NO_SCREEN = ("shell.", "fs.")              # tool families that never touch windows, focus or the cursor (RQ2)
ORIGIN_KINDS = {"task", "tool.call", "context.source"}   # what the task's models were shown (S3; task_origins)
_WRAPPED = re.compile(r'<untrusted-data id="([0-9a-f]+)">.*?</untrusted-data id="\1">', re.S)


def walk(nodes: list[dict[str, Any]]) -> Iterator[dict[str, Any]]:
    for n in nodes:
        yield n
        yield from walk(n.get("children") or [])


def _secs(start: str | None, end: str | None) -> float | None:
    if not start or not end:
        return None
    return round((datetime.fromisoformat(end) - datetime.fromisoformat(start)).total_seconds(), 1)


def _json(text: Any) -> dict[str, Any]:
    try:
        v = json.loads(text) if isinstance(text, str) else text
    except ValueError:
        return {}
    return v if isinstance(v, dict) else {}


def _planner(n: dict[str, Any]) -> str:
    out = n.get("output") or {}
    if n["kind"] == "model.turn":
        d = _json(out.get("text"))
        task = d.get("task")
        spec: dict[str, Any] = task if isinstance(task, dict) else {}
        if d.get("kind") == "task" and spec.get("goal"):
            return f"task — {clip(spec['goal'], 160)}"
        return f"{d.get('kind') or 'decided'} — {clip(d.get('text', ''), 160)}".rstrip(" —")
    if n["kind"] == "model.dispatch":
        d = _json(out.get("text"))
        detail = d.get("instructions") or d.get("question") or d.get("stop_reason") or ""
        return f"{d.get('action') or 'decided'} — {clip(detail, 160)}".rstrip(" —")
    if n["kind"] == "model.resources":           # the final-state review after cleanup: it sets the task's status
        d = _json(out.get("text"))
        if not d.get("status"):
            return f"after cleanup: {clip(out['text'], 160)}" if out.get("text") else "after cleanup"
        return f"after cleanup: {d['status']}" + (f" — {clip(d['summary'], 160)}" if d.get("summary") else "")
    d = _json(out.get("decision"))
    if d.get("ask_owner"):
        return "asks you: " + clip(d.get("question", ""), 160)
    return "answered the worker: " + clip(d.get("answer", ""), 160)


def _checker(n: dict[str, Any], total: int) -> tuple[str, str]:
    if n.get("error"):
        return "✗", "check failed: " + clip(n["error"], 120)
    results = _json((n.get("output") or {}).get("result")).get("results") or []
    met = [r.get("met") for r in results]
    if results and all(m is True for m in met):
        return "✓", "verified: " + "; ".join(clip(r.get("evidence", ""), 60) for r in results)
    if all(m is None for m in met):
        why = clip(results[0].get("evidence", ""), 60) if results else ""
        return "⚠", (f"checked nothing — {plural(total or len(results), 'condition', 'conditions')} unknown"
                     + (f" ({why})" if why else ""))
    bad, unknown = met.count(False), met.count(None)
    return ("✗" if bad else "⚠"), f"{bad} not met, {unknown} unknown, {met.count(True)} verified"


def _worker(n: dict[str, Any]) -> tuple[str, str]:
    out = n.get("output") or {}
    report = _json(out.get("result"))
    status = out.get("status") or report.get("status") or ("failed" if n.get("error") else "")
    summary = str(report.get("summary") or "") if report else str(out.get("result") or "")
    calls = sum(1 for c in walk(n.get("children") or []) if c["kind"] == "tool.call")
    head = f"{plural(calls, 'tool call', 'tool calls')} · " if calls else ""
    claim = f"worker reported {status}: " if status else "worker reported: "
    mark = "✗" if status in ("failed", "blocked", "cancelled") or n.get("error") else ""
    return mark, head + claim + clip(summary, 160)


def _request_text(outcome: dict[str, Any]) -> str:
    request = outcome.get("request") or {}
    messages = request.get("messages") or []
    text = (messages[-1].get("data") or {}).get("text") if messages else None
    text = text or request.get("text") or (outcome.get("task_spec") or {}).get("goal") or ""
    return " ".join(_WRAPPED.sub("[pasted text]", str(text)).split())


def compose(trace: dict[str, Any]) -> dict[str, Any]:
    task_id = trace["task_id"]
    forest = trace.get("spans") or []
    nodes = sorted(walk(forest), key=lambda n: n["started_at"])
    outcome = trace.get("outcome") or {}
    spec = outcome.get("task_spec") or {}
    conditions_text = [c.get("text", "") for c in spec.get("success_conditions") or []]
    root: dict[str, Any] = next((n for n in forest if n["kind"] == "task"), {})
    result = outcome.get("result") or {}
    row: dict[str, Any] = next((t for t in outcome.get("tasks") or [] if t.get("id") == task_id), {})

    steps: list[dict[str, Any]] = []
    retry_starts: list[str] = []
    for n in nodes:
        kind, inp = n["kind"], n.get("input") or {}
        if kind in PLANNER_KINDS and (kind != "model.turn" or inp.get("role") == "supervisor"):
            role = "Planner"
        elif kind == "worker":
            role = "Checker" if inp.get("role") == "verifier" else "Worker"
        else:
            continue
        if not n.get("ended_at"):                   # still open when the record was built (restart, cancellation)
            mark, text = OPEN, OPEN_TEXT
        elif role == "Planner" and n.get("error"):  # a turn that failed decided nothing, whatever text it left
            mark, text = "✗", "failed: " + clip(n["error"], 120)
        elif role == "Planner":
            mark, text = "", _planner(n)
            if kind == "model.dispatch" and _json((n.get("output") or {}).get("text")).get("action") == "retry":
                retry_starts.append(n["started_at"])
        elif role == "Checker":
            mark, text = _checker(n, len(conditions_text))
        else:
            mark, text = _worker(n)
        steps.append({"at": n["started_at"], "role": role, "seconds": _secs(n["started_at"], n.get("ended_at")),
                      "text": text, "mark": mark})
    for i, s in enumerate(steps):                   # a worker's claim notes the check that verified it
        if s["role"] == "Worker" and s["mark"] != OPEN:
            nxt = next((j for j in range(i + 1, len(steps)) if steps[j]["role"] in ("Worker", "Checker")), None)
            if nxt is not None and steps[nxt]["role"] == "Checker" and steps[nxt]["mark"] == "✓":
                s["text"] += f" — verified in step {nxt + 1}"

    times = [t for n in nodes for t in (n["started_at"], n.get("ended_at")) if t]
    started = min(times) if times else None
    ended = root.get("ended_at")                    # an open root has no end: the last timestamp is only a lower bound
    problems: list[str] = []
    if retry_starts:
        spent = duration(_secs(retry_starts[0], ended))
        problems.append(f"⚠ {plural(len(retry_starts), 'retry', 'retries')} ({spent} spent after "
                        f"{'it' if len(retry_starts) == 1 else 'the first'})")
    unknown = sum(1 for s in steps if s["role"] == "Checker" and s["mark"] == "⚠")
    if unknown:
        problems.append(f"⚠ {plural(unknown, 'check', 'checks')} left conditions unknown")
    failed = sum(1 for s in steps if s["mark"] == "✗")
    if failed:
        problems.append(f"✗ {plural(failed, 'step', 'steps')} failed")
    unfinished = sum(1 for s in steps if s["mark"] == OPEN)
    if unfinished:
        problems.append(f"{OPEN} {plural(unfinished, 'step', 'steps')} did not finish")

    usage_out = [n.get("output") or {} for n in nodes if n["kind"] == "model.usage"]
    priced = [u["cost_usd"] for u in usage_out if u.get("cost_usd") is not None]
    usage = {"model_calls": sum(1 for n in nodes if n["kind"] in MODEL_KINDS), "usage_records": len(usage_out),
             "input_tokens": sum(int(u.get("input_tokens") or 0) for u in usage_out),
             "cached_input_tokens": sum(int(u.get("cached_input_tokens") or 0) for u in usage_out),
             "output_tokens": sum(int(u.get("output_tokens") or 0) for u in usage_out),
             "cost_usd": round(sum(priced), 6) if priced else None,
             "unpriced_models": sorted({u.get("model") or "an unnamed model" for u in usage_out
                                        if u.get("cost_usd") is None}),
             "billing": sorted({u["billing"] for u in usage_out if u.get("billing")})}

    summary = next((n.get("output") or {} for n in reversed(nodes) if n["kind"] == "disturbance.summary"), {})
    raw = summary.get("changes")
    near: list[dict[str, Any]] = []
    for n in nodes:
        out = n.get("output") or {}
        tools = sorted({str(a.get("tool", "?")) for a in out.get("actions") or []
                        if not str(a.get("tool", "")).startswith(NO_SCREEN)})
        if n["kind"] == "disturbance.change" and tools:
            near.append({"at": n["started_at"], "changed": sorted(out.get("changes") or {}), "tools": tools})

    tools_used: dict[str, dict[str, int]] = {}
    origins: set[str] = set()
    for n in nodes:
        if n["kind"] in ORIGIN_KINDS:              # the request, context sources and tool results (S3)
            origins |= origin.split(n.get("origin"))
        if n["kind"] == "tool.call":
            t = tools_used.setdefault(str((n.get("input") or {}).get("tool", "?")), {"calls": 0, "errors": 0})
            t["calls"] += 1
            t["errors"] += 1 if n.get("error") else 0

    checked = next((r["verification"] for r in reversed(outcome.get("outcome_records") or [])
                    if isinstance(r.get("verification"), list)), None)
    conditions = ([{"text": v.get("condition", ""), "met": v.get("met"), "evidence": v.get("evidence", "")}
                   for v in checked] if checked
                  else [{"text": t, "met": None, "evidence": ""} for t in conditions_text])
    answer = next((str(r["text"]) for r in reversed(outcome.get("outcome_records") or []) if "text" in r),
                  str(result.get("summary") or ""))
    return {"task_id": task_id, "request_id": (outcome.get("request") or {}).get("id"),
            "request_text": _request_text(outcome), "goal": spec.get("goal") or row.get("goal") or "",
            "status": str(result.get("status") or row.get("status") or "unknown"), "verified": result.get("verified"),
            "started_at": started, "ended_at": ended, "seconds": _secs(started, ended), "answer": answer,
            "conditions": conditions, "steps": steps, "problems": problems, "tools": tools_used,
            "origins": sorted(origins), "usage": usage,
            "desktop": {"changes": len(raw) if isinstance(raw, (list, dict)) else int(raw or 0), "near_ferb": near}}
