"""Developer toolbox (GATE-545): run a list of tasks in the running FERB, on one connection, in test mode's question
recorder, and write each task's results to a report outside the repository.

    uv run python scripts/question_run.py TASKS --runtime anthropic --out ~/ferb-runs/run1.json [--repeat 3]

TASKS is a text file, one task per line ("||" splits the turns of a two-turn task, "#" starts a comment), or a JSON
list of {"id", "turns": [...]}. Each task runs in a chat of its own. The run needs test mode with questions recorded
(Settings, or `--enable "me@example.com" ...` here, which turns it on with that send allowlist; it stays on after).
The connection is used for every role during the run and the roles go back as they were at the end.

Per task: each question FERB recorded (card, rules, the table's answer, when), the time, steps (tool calls), model
calls, tokens (input, cached reads, cache writes by TTL, output), cost and the outcome. Nothing is cleaned up: FERB
closes only its own tabs (HAND-201)."""

import argparse
import json
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from ferb.cli.client import Client  # noqa: E402
from ferb.paths import socket_path  # noqa: E402

USAGE_KEYS = ("input_tokens", "cached_input_tokens", "cache_write_tokens", "cache_write_5m_tokens",
              "cache_write_1h_tokens", "output_tokens", "reasoning_tokens")


def tasks_from(path: Path) -> list[dict[str, Any]]:
    text = path.read_text()
    if path.suffix == ".json":
        rows = json.loads(text)
        return [{"id": str(r.get("id") or i + 1), "turns": list(r.get("turns") or [r["text"]])}
                for i, r in enumerate(rows)]
    lines = [ln.strip() for ln in text.splitlines() if ln.strip() and not ln.strip().startswith("#")]
    return [{"id": str(i + 1), "turns": [t.strip() for t in ln.split("||") if t.strip()]} for i, ln in enumerate(lines)]


def walk(nodes: list[dict[str, Any]]) -> list[dict[str, Any]]:
    out = []
    for n in nodes:
        out.append(n)
        out += walk(n.get("children") or [])
    return out


def send(client: Client, session: str, text: str) -> dict[str, Any]:
    """One chat turn, streamed to its end (the turn's tasks finished): its request, tasks, replies and time."""
    started = time.monotonic()
    turn: dict[str, Any] = {"text": text, "request_id": None, "task_ids": [], "replies": [], "outcomes": []}
    for ev in client.stream("chat.send", {"session_id": session, "text": text}):
        turn["request_id"] = turn["request_id"] or ev.get("request_id")
        if ev.get("task_id") and ev["task_id"] not in turn["task_ids"]:
            turn["task_ids"].append(ev["task_id"])
        if ev.get("type") == "reply":
            turn["replies"].append({"text": ev.get("text", ""), "status": ev.get("status")})
        elif ev.get("type") == "outcome":
            turn["outcomes"].append({k: ev.get(k) for k in ("task_id", "status", "summary")})
    turn["seconds"] = round(time.monotonic() - started, 1)
    return turn


def collect(client: Client, turn: dict[str, Any]) -> dict[str, Any]:
    """The turn's spans (its tasks' trees, and its planner turns by request), counted once each."""
    spans: dict[str, dict[str, Any]] = {}
    episodes = []
    for ref in turn["task_ids"] or [turn["request_id"]]:
        if not ref:
            continue
        try:
            got = client.call("trace.get", {"task_id": ref})
        except Exception as e:                         # a task the trace can't resolve: say so, keep going
            turn.setdefault("problems", []).append(f"trace {ref}: {e}")
            continue
        for n in walk(got.get("spans") or []):
            spans[n["id"]] = n
        if got.get("episode"):
            episodes.append({k: got["episode"].get(k) for k in ("task_id", "status", "verified", "answer", "seconds")})
    nodes = sorted(spans.values(), key=lambda n: n["started_at"])
    usage = {k: 0 for k in USAGE_KEYS}
    cost, unpriced = 0.0, set()
    for n in nodes:
        if n["kind"] == "model.usage":
            out = n.get("output") or {}
            for k in USAGE_KEYS:
                usage[k] += int(out.get(k) or 0)
            if out.get("cost_usd") is None:
                unpriced.add(str(out.get("model") or "?"))
            else:
                cost += float(out["cost_usd"])
    calls = [n for n in nodes if n["kind"] == "tool.call"]
    questions = []
    for n in calls:
        inp, out = n.get("input") or {}, n.get("output") or {}
        if inp.get("tool") != "owner.ask":
            continue
        questions.append({"at": n["started_at"], "ferb_question": bool(inp.get("ferb_question")),
                          "question": (inp.get("args") or {}).get("question"),
                          "rules": [c.get("rule") for c in (inp.get("args") or {}).get("confirm") or []],
                          "card": out.get("card"), "needs": out.get("needs"),
                          "answer": out.get("answer") or out.get("reason") or out.get("state"),
                          "recorded": bool(out.get("test_mode"))})
    return {**turn, "questions": questions, "steps": len([c for c in calls if (c.get("input") or {}).get("tool")
                                                          != "owner.ask"]),
            "model_calls": sum(1 for n in nodes if n["kind"] == "model.usage"), "usage": usage,
            "cost_usd": round(cost, 6), "unpriced_models": sorted(unpriced), "episodes": episodes}


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("tasks", type=Path)
    p.add_argument("--runtime", required=True, help="codex, claude, anthropic, openai or google")
    p.add_argument("--out", type=Path, required=True, help="the report (JSON), outside the repository")
    p.add_argument("--repeat", type=int, default=1)
    p.add_argument("--enable", nargs="*", metavar="RECIPIENT",
                   help="turn on test mode with questions recorded and this send allowlist")
    args = p.parse_args()
    out = args.out.expanduser().resolve()
    if out.is_relative_to(ROOT):
        p.error("--out must be outside the repository")
    tasks = tasks_from(args.tasks)
    client = Client(socket_path(), timeout=60)
    if args.enable is not None:
        client.call("test.mode", {"allow_send": args.enable, "questions": "record"})
    mode = client.call("test.mode", {"status": True})
    if mode.get("questions") != "record":
        p.error("FERB's test mode isn't recording questions: turn it on, or pass --enable with the send allowlist")
    before = client.call("runtimes.status").get("roles") or {}
    client.call("runtimes.use", {"runtime": args.runtime})
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    report: dict[str, Any] = {"started": datetime.now().isoformat(timespec="seconds"), "runtime": args.runtime,
                              "test_mode": mode, "tasks": []}
    out.parent.mkdir(parents=True, exist_ok=True)
    try:
        for task in tasks:
            for run in range(1, args.repeat + 1):
                session = f"qrun-{stamp}-{task['id']}-{run}"
                turns = [collect(client, send(client, session, text)) for text in task["turns"]]
                row = {"id": task["id"], "run": run, "session": session, "turns": turns,
                       "seconds": round(sum(t["seconds"] for t in turns), 1),
                       "questions": sum(len(t["questions"]) for t in turns),
                       "steps": sum(t["steps"] for t in turns),
                       "cost_usd": round(sum(t["cost_usd"] for t in turns), 6)}
                report["tasks"].append(row)
                out.write_text(json.dumps(report, ensure_ascii=False, indent=2))   # kept as it goes
                print(f"{task['id']} run {run}: {row['seconds']} s, {row['steps']} steps, {row['questions']} "
                      f"questions, ${row['cost_usd']}")
    finally:
        for role, runtime in before.items():           # the roles as they were
            client.call("runtimes.use", {"runtime": runtime, "roles": [role]})
        report["ended"] = datetime.now().isoformat(timespec="seconds")
        out.write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(f"report: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
