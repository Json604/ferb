"""Owns where a traced turn's time went (ORCH-527): FERB's work before the model, the runtime's start, each model call
split into its parts, the tools between them, and FERB's work after, from the timing the spans recorded
(ORCH-525, ORCH-526, RUN-419, RUN-420). Must not add anything the records don't say: a span recorded before timing
existed shows none.

`breakdown` is the JSON `ferb trace --json` adds; `lines` is the text `ferb trace --full` ends with."""

from datetime import datetime, timedelta
from typing import Any, Iterator

from ferb.trace.compose import walk

LOOKUPS = ".lookups"            # a planner turn's lookup record: its request id with this suffix (supervisor.LOOKUPS)
CALL_PARTS = (("runtime_ms", "runtime before the request"), ("ttft_ms", "to the first output"),
              ("generate_ms", "generating"), ("tail_ms", "to its usage report"))


def _ms(start: str | None, end: str | None) -> int | None:
    if not start or not end:
        return None
    return int(round((datetime.fromisoformat(end) - datetime.fromisoformat(start)).total_seconds() * 1000))


def _timing(node: dict[str, Any]) -> dict[str, Any]:
    out = node.get("output")
    timing = out.get("timing") if isinstance(out, dict) else None
    return timing if isinstance(timing, dict) else {}


def _calls(node: dict[str, Any]) -> list[dict[str, Any]]:
    """Each model call's recorded timing, with when its usage report reached FERB."""
    return [{**_timing(c), "reported_at": c["started_at"]} for c in node.get("children") or []
            if c["kind"] == "model.usage"]


def _tools(nodes: list[dict[str, Any]], task_id: str, start: str, end: str | None) -> list[dict[str, Any]]:
    """The tool calls recorded under `task_id` while the run lasted, each with its own recorded time."""
    return [{"tool": str((n.get("input") or {}).get("tool", "?")), "ms": _ms(n["started_at"], n.get("ended_at")),
             "at": n["started_at"]}
            for n in nodes if n["kind"] == "tool.call" and n.get("task_id") == task_id
            and n["started_at"] >= start and (end is None or n["started_at"] <= end)]


def _planner_turns(nodes: list[dict[str, Any]]) -> Iterator[dict[str, Any]]:
    used: set[str] = set()
    for turn in nodes:
        inp = turn.get("input") or {}
        if turn["kind"] != "model.turn" or inp.get("role") != "supervisor":
            continue
        request_id = str(inp.get("request_id") or "")
        prepare = next((n for n in reversed(nodes) if n["kind"] == "planner.prepare" and n["id"] not in used
                        and (n.get("input") or {}).get("request_id") == request_id
                        and n["started_at"] <= turn["started_at"]), None)
        if prepare is not None:
            used.add(prepare["id"])
        timing = _timing(turn)
        call = held if isinstance(held := timing.get("call"), dict) else {}
        steps = timing.get("steps_ms") or {}
        call_steps = call.get("steps_ms") or {}
        reply = next((n for n in nodes if n["kind"] == "planner.reply" and n["started_at"] >= turn["started_at"]
                      and (n.get("input") or {}).get("request_id") == request_id), None)
        before = (prepare.get("output") or {}) if prepare is not None else {}
        yield {"kind": "planner", "request_id": request_id, "at": (prepare or turn)["started_at"],
               "error": turn.get("error"),
               "ferb_before": {"steps_ms": before.get("steps_ms") or {}, "total_ms": before.get("total_ms"),
                               "in_turn_ms": steps.get("ferb_before"), "request_built_ms": call_steps.get("prepare")},
               "runtime_start": {"ms": call_steps.get("start"), "steps_ms": call.get("start_steps_ms") or {}},
               "model_calls": _calls(turn),
               "tools": _tools(nodes, request_id + LOOKUPS, turn["started_at"], turn.get("ended_at")),
               "stream_ms": call_steps.get("stream"), "close_ms": call_steps.get("close"),
               "ferb_after": {"in_turn_ms": steps.get("ferb_after"),
                              "to_reply_ms": ((reply or {}).get("output") or {}).get("after_decision_ms")},
               "turn_ms": timing.get("total_ms"), "recorded": bool(timing)}


def _workers(nodes: list[dict[str, Any]]) -> Iterator[dict[str, Any]]:
    for run in nodes:
        if run["kind"] != "worker":
            continue
        call = _timing(run)
        steps = call.get("steps_ms") or {}
        yield {"kind": "checker" if (run.get("input") or {}).get("role") == "verifier" else "worker",
               "task_id": run.get("task_id"), "at": run["started_at"], "error": run.get("error"),
               "request_built_ms": steps.get("prepare"),
               "runtime_start": {"ms": steps.get("start"), "steps_ms": call.get("start_steps_ms") or {}},
               "model_calls": _calls(run),
               "tools": _tools(nodes, str(run.get("task_id")), run["started_at"], run.get("ended_at")),
               "stream_ms": steps.get("stream"), "close_ms": steps.get("close"),
               "run_ms": _ms(run["started_at"], run.get("ended_at")), "recorded": bool(call)}


def breakdown(forest: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Each planner turn and worker run of the trace, in start order, with where its time went."""
    nodes = sorted(walk(forest), key=lambda n: n["started_at"])
    return sorted([*_planner_turns(nodes), *_workers(nodes)], key=lambda e: e["at"])


def _d(ms: Any) -> str:
    if not isinstance(ms, (int, float)):
        return "?"
    return f"{ms:.0f} ms" if ms < 1000 else f"{ms / 1000:.2f} s"


def _parts(steps: dict[str, Any]) -> str:
    return ", ".join(f"{name.replace('_', ' ')} {_d(ms)}" for name, ms in steps.items() if ms is not None)


def _began(call: dict[str, Any]) -> datetime:
    """When a model call began: its usage report's arrival less its total (the report can follow a tool already run)."""
    return datetime.fromisoformat(call["reported_at"]) - timedelta(milliseconds=int(call.get("total_ms") or 0))


def _call_line(i: int, timing: dict[str, Any]) -> str:
    if "total_ms" not in timing:
        return f"  Model call {i}: no timing recorded"
    parts = ", ".join(f"{label} {_d(timing[key])}" for key, label in CALL_PARTS if key in timing)
    clock = "the runtime's own stream" if timing.get("by") == "runtime" else "arrival at FERB"
    return f"  Model call {i} {_d(timing.get('total_ms'))}: {parts} (by {clock})"


def _run_lines(entry: dict[str, Any]) -> list[str]:
    out: list[str] = []
    start = entry["runtime_start"]
    out.append(f"  Runtime start {_d(start['ms'])}" + (f": {_parts(start['steps_ms'])}" if start["steps_ms"] else ""))
    calls = entry["model_calls"]
    tools_ms = sum(t["ms"] or 0 for t in entry["tools"])
    # in the order they began: a call when its report arrived less its total, a tool when it started
    events = [(_began(c), 0, _call_line(i, c)) for i, c in enumerate(calls, 1)]
    events += [(datetime.fromisoformat(t["at"]), 1, f"  Tool {t['tool']} {_d(t['ms'])}") for t in entry["tools"]]
    out += [line for _, _, line in sorted(events, key=lambda e: (e[0], e[1]))]
    if isinstance(entry.get("stream_ms"), int):
        counted = sum(int(c.get("total_ms") or 0) for c in calls) + tools_ms
        out.append(f"  Stream {_d(entry['stream_ms'])} in all (model calls and tools {_d(counted)}; the rest is the "
                   "runtime's own work between them, or clock skew)")
    out.append(f"  Runtime close {_d(entry.get('close_ms'))}")
    return out


def lines(entries: list[dict[str, Any]]) -> list[str]:
    """`ferb trace --full`'s timing section: per planner turn FERB before the model, the runtime's start, each model
    call split, the tools, the runtime's close and FERB after; per worker run the same without the planner's parts."""
    if not entries:
        return []
    out = ["", "Where the time went (FERB's clock; each model call on the clock named):"]
    for e in entries:
        when = datetime.fromisoformat(e["at"]).strftime("%H:%M:%S")
        if not e["recorded"]:
            out.append(f"{e['kind'].capitalize()} {when}: no timing recorded (a span from before timing was kept)")
            continue
        if e["kind"] == "planner":
            before, after = e["ferb_before"], e["ferb_after"]
            total = sum(v for v in (before.get("total_ms"), e.get("turn_ms"), after.get("to_reply_ms"))
                        if isinstance(v, int))
            out.append(f"Planner turn {when} (request {e['request_id']}): {_d(total)} from its intake to "
                       + ("its reply" if isinstance(after.get("to_reply_ms"), int) else "its decision")
                       + (" (failed)" if e.get("error") else ""))
            parts = {**before["steps_ms"], "turn setup": before.get("in_turn_ms"),
                     "request built": before.get("request_built_ms")}
            ahead = sum(v for v in (before.get("total_ms"), before.get("in_turn_ms"), before.get("request_built_ms"))
                        if isinstance(v, int))
            out.append(f"  FERB before the model {_d(ahead)}: {_parts(parts)}")
            out += _run_lines(e)
            out.append(f"  FERB after the model: answer check {_d(after.get('in_turn_ms'))}"
                       + (f", to the reply {_d(after['to_reply_ms'])}" if isinstance(after.get("to_reply_ms"), int)
                          else ""))
        else:
            out.append(f"{e['kind'].capitalize()} {e['task_id']} {when}: {_d(e.get('run_ms'))}"
                       + (" (failed)" if e.get("error") else ""))
            out.append(f"  Request built {_d(e.get('request_built_ms'))}")
            out += _run_lines(e)
    return out
