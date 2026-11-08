"""Developer toolbox (GATE-546): run a list of tasks in the running FERB, on one connection, in test mode's question
recorder, and write each task's results to a report outside the repository.

    uv run python scripts/question_run.py TASKS --runtime anthropic --out ~/ferb-runs/run1.json [--repeat 3]

TASKS is a text file, one task per line ("||" splits the turns of a two-turn task, "#" starts a comment), or a JSON
list of {"id", "turns": [...]}. Each task runs in a chat of its own, tagged with the run and the connection
(GATE-550), so no run sees another's chats, memory, learning or activity, and FERB learns nothing from them. The run
needs test mode with questions recorded (Settings, or `--enable "me@example.com" ...` here, which turns it on with that
send allowlist; it stays on after). The connection is used for every role during the run and the roles go back as
they were at the end.

The run tool's own limits (not FERB's): --task-limit minutes per task (default 20), after which the runner stops the
task through FERB's stop and records it; --cost-cap dollars for the whole run, after which no new task starts.

Per task: start and end, seconds (each step's and each model call's too), steps (tool calls), model calls, tokens
(input, cache reads, cache writes by TTL, output), cost, each question FERB recorded (card, rules, the table's answer,
when), the typed-answer forms and repairs (RUN-440), the outcome and FERB's final answer, and the acting calls that may
have left something in the owner's accounts (a cart add, a draft: check by hand). Nothing is cleaned up: FERB closes
only its own tabs (HAND-201). scripts/question_compare.py puts several connections' reports side by side."""

import argparse
import json
import sys
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from ferb.cli.client import Client  # noqa: E402
from ferb.paths import socket_path  # noqa: E402

# Calls whose kind may leave something behind in an account or on the Mac (rules.Kind): everything but reads,
# navigation, search, tabs, snapshots and stopping the task's own processes
LEAVES_NOTHING = frozenset({"read", "navigate", "search", "open_tab", "snapshot", "stop_process", "mailbox_read"})
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


def typed_forms(nodes: list[dict[str, Any]]) -> dict[str, Any]:
    """RUN-440: model calls by the form their typed answer took (strict / stated / stated_after_refusal / none), the
    stated answers told back once in the run (asked) and how that ended (fixed / failed), the seconds those repair
    calls took, and the planner's own repair turns after a failed answer (and how many of them ended without error)."""
    by_form: dict[str, int] = {}
    repairs = {"asked": 0, "fixed": 0, "failed": 0}
    extra_ms = 0
    for n in nodes:
        if n["kind"] != "model.usage":
            continue
        out = n.get("output") or {}
        form = str(out.get("typed") or "none")
        by_form[form] = by_form.get(form, 0) + 1
        if out.get("repair") in repairs:
            repairs[out["repair"]] += 1
            if out["repair"] != "asked":
                extra_ms += int((out.get("timing") or {}).get("total_ms") or 0)
    planner = [n for n in nodes if n["kind"] == "model.turn" and (n.get("input") or {}).get("repair")]
    return {"calls": by_form, "repairs": repairs, "repair_seconds": round(extra_ms / 1000, 1),
            "planner_repairs": len(planner), "planner_repairs_saved": sum(1 for n in planner if not n.get("error"))}


def add_forms(total: dict[str, Any], one: dict[str, Any]) -> dict[str, Any]:
    out = {"calls": dict(total.get("calls") or {}), "repairs": dict(total.get("repairs") or {})}
    for part in ("calls", "repairs"):
        for k, v in one[part].items():
            out[part][k] = out[part].get(k, 0) + v
    for k in ("repair_seconds", "planner_repairs", "planner_repairs_saved"):
        out[k] = round(total.get(k, 0) + one[k], 1)
    return out


def send(session: str, text: str, tag: dict[str, str], limit_s: float) -> dict[str, Any]:
    """One chat turn, streamed to its end (the turn's tasks finished), on a connection of its own. Past `limit_s` the
    runner stops the turn's tasks through FERB's own stop (task.stop) and waits for the stream to end."""
    started = time.monotonic()
    turn: dict[str, Any] = {"text": text, "request_id": None, "task_ids": [], "replies": [], "outcomes": [],
                            "started_at": datetime.now().astimezone().isoformat(timespec="seconds")}

    def stream() -> None:
        c = Client(socket_path(), timeout=60)
        try:
            for ev in c.stream("chat.send", {"session_id": session, "text": text, "test_run": tag}):
                turn["request_id"] = turn["request_id"] or ev.get("request_id")
                if ev.get("task_id") and ev["task_id"] not in turn["task_ids"]:
                    turn["task_ids"].append(ev["task_id"])
                if ev.get("type") == "reply":
                    turn["replies"].append({"text": ev.get("text", ""), "status": ev.get("status")})
                elif ev.get("type") == "outcome":
                    turn["outcomes"].append({k: ev.get(k) for k in ("task_id", "status", "summary")})
        except Exception as e:                      # FERB's words, kept with the turn
            turn.setdefault("problems", []).append(f"chat.send: {e}")
        finally:
            c.close()
    worker = threading.Thread(target=stream, daemon=True)
    worker.start()
    worker.join(limit_s)
    if worker.is_alive():
        roots = sorted({t.split(".", 1)[0] for t in turn["task_ids"]})
        turn["stopped_by_runner"] = f"stopped by the runner after {limit_s / 60:g} min"
        if roots:
            stopper = Client(socket_path(), timeout=60)
            try:
                turn["stopped"] = stopper.call("task.stop", {"task_ids": roots}).get("stopped")
            finally:
                stopper.close()
        else:
            turn["stopped_by_runner"] += " (no task had started: nothing to stop)"
        worker.join(120)
    turn["ended_at"] = datetime.now().astimezone().isoformat(timespec="seconds")
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
    usage = {k: 0 for k in USAGE_KEYS} | {"calls_without_cache_write_count": 0}   # RUN-438: unknown, not zero
    cost, unpriced = 0.0, set()
    for n in nodes:
        if n["kind"] == "model.usage":
            out = n.get("output") or {}
            for k in USAGE_KEYS:
                usage[k] += int(out.get(k) or 0)
            usage["calls_without_cache_write_count"] += out.get("cache_write_reported") is False
            if out.get("cost_usd") is None:
                unpriced.add(str(out.get("model") or "?"))
            else:
                cost += float(out["cost_usd"])
    forms = typed_forms(nodes)
    calls = [n for n in nodes if n["kind"] == "tool.call"]
    model_calls = [{"model": (n.get("output") or {}).get("model"),
                    "seconds": round(int(((n.get("output") or {}).get("timing") or {}).get("total_ms") or 0) / 1000, 1)}
                   for n in nodes if n["kind"] == "model.usage"]
    steps = [{"tool": (n.get("input") or {}).get("tool"), "kind": (n.get("input") or {}).get("kind"),
              "seconds": secs(n), "error": bool(n.get("error"))}
             for n in calls if (n.get("input") or {}).get("tool") != "owner.ask"]
    leftovers = [{"tool": st["tool"], "kind": st["kind"], "target": (n.get("input") or {}).get("target"),
                  "effect": (n.get("input") or {}).get("effect")}
                 for st, n in zip(steps, [c for c in calls if (c.get("input") or {}).get("tool") != "owner.ask"])
                 if st["kind"] and st["kind"] not in LEAVES_NOTHING and not st["error"]]
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
    answer = next((e["answer"] for e in reversed(episodes) if e.get("answer")), "") or \
        next((o["summary"] for o in reversed(turn["outcomes"]) if o.get("summary")), "") or \
        next((r["text"] for r in reversed(turn["replies"]) if r.get("text")), "")
    return {**turn, "questions": questions, "steps": len(steps), "step_times": steps,
            "model_calls": len(model_calls), "model_call_times": model_calls, "usage": usage, "typed": forms,
            "cost_usd": round(cost, 6), "unpriced_models": sorted(unpriced), "episodes": episodes,
            "final_answer": answer, "possible_leftovers": leftovers}


def set_model(client: Client, runtime: str, model: str, role: str | None = None) -> None:
    """The connection's model (or one role's; "" clears a role's own), through runtimes.model, which checks a model
    before it saves it; a refusal stops the run with the provider's words."""
    client.set_timeout(120)
    got = client.call("runtimes.model", {"runtime": runtime, "model": model} | ({"role": role} if role else {}))
    client.set_timeout(60)
    if not got.get("ok"):
        raise SystemExit(f"{runtime} didn't take {model or 'the connection model'}: {got.get('detail')}")


def secs(n: dict[str, Any]) -> float | None:
    if not n.get("ended_at"):
        return None
    return round((datetime.fromisoformat(n["ended_at"]) - datetime.fromisoformat(n["started_at"])).total_seconds(), 1)


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("tasks", type=Path)
    p.add_argument("--runtime", required=True, help="codex, claude, anthropic, openai or google")
    p.add_argument("--out", type=Path, required=True, help="the report (JSON), outside the repository")
    p.add_argument("--repeat", type=int, default=1)
    p.add_argument("--enable", nargs="*", metavar="RECIPIENT",
                   help="turn on test mode with questions recorded and this send allowlist")
    p.add_argument("--model", default="", help="the connection's model for every role during the run (checked first, "
                                               "as Settings does; the models go back at the end)")
    p.add_argument("--task-limit", type=float, default=20.0, help="minutes per task before the runner stops it")
    p.add_argument("--cost-cap", type=float, default=None, help="dollars: no new task starts once the run reached it")
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
    models = client.call("runtimes.models", {"runtime": args.runtime})
    if args.model:                                     # every role on this model: the roles' own ones set aside
        set_model(client, args.runtime, args.model)
        for role, own in (models.get("roles") or {}).items():
            if isinstance(own, dict) and own.get("runtime") == args.runtime and own.get("model"):
                set_model(client, args.runtime, "", role)
    client.call("runtimes.use", {"runtime": args.runtime})
    stamp = datetime.now().strftime("%Y%m%d-%H%M%S")
    report: dict[str, Any] = {"started": datetime.now().isoformat(timespec="seconds"), "runtime": args.runtime,
                              "model": client.call("runtimes.models", {"runtime": args.runtime}).get("model"),
                              "run": stamp, "test_mode": mode, "task_limit_min": args.task_limit,
                              "cost_cap_usd": args.cost_cap, "tasks": []}
    tag = {"run": stamp, "connection": args.runtime}
    out.parent.mkdir(parents=True, exist_ok=True)
    try:
        for task in tasks:
            for run in range(1, args.repeat + 1):
                spent = sum(t["cost_usd"] for t in report["tasks"])
                if args.cost_cap is not None and spent >= args.cost_cap:
                    report["stopped"] = f"the cost cap ${args.cost_cap:g} was reached (${spent:.4f})"
                    raise StopIteration
                session = f"qrun-{stamp}-{args.runtime}-{task['id']}-{run}"
                turns = [collect(client, send(session, text, tag, args.task_limit * 60)) for text in task["turns"]]
                row = {"id": task["id"], "run": run, "session": session, "turns": turns,
                       "started_at": turns[0]["started_at"], "ended_at": turns[-1]["ended_at"],
                       "stopped_by_runner": next((t["stopped_by_runner"] for t in turns
                                                  if t.get("stopped_by_runner")), ""),
                       "final_answer": turns[-1]["final_answer"],
                       "seconds": round(sum(t["seconds"] for t in turns), 1),
                       "questions": sum(len(t["questions"]) for t in turns),
                       "steps": sum(t["steps"] for t in turns),
                       "cost_usd": round(sum(t["cost_usd"] for t in turns), 6)}
                for t in turns:
                    row["typed"] = add_forms(row.get("typed") or {}, t["typed"])
                    report["typed"] = add_forms(report.get("typed") or {}, t["typed"])    # the run's summary
                report["tasks"].append(row)
                out.write_text(json.dumps(report, ensure_ascii=False, indent=2))   # kept as it goes
                print(f"{task['id']} run {run}: {row['seconds']} s, {row['steps']} steps, {row['questions']} "
                      f"questions, ${row['cost_usd']}")
    except StopIteration:
        print(report["stopped"])
    finally:
        for role, runtime in before.items():           # the roles as they were
            client.call("runtimes.use", {"runtime": runtime, "roles": [role]})
        if args.model:                                 # and the models
            if models.get("model") and models["model"] != args.model:
                set_model(client, args.runtime, str(models["model"]))
            for role, own in (models.get("roles") or {}).items():
                if isinstance(own, dict) and own.get("runtime") == args.runtime and own.get("model"):
                    set_model(client, args.runtime, str(own["model"]), role)
        report["ended"] = datetime.now().isoformat(timespec="seconds")
        out.write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(f"report: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
