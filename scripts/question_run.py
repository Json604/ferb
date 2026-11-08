"""Developer toolbox (GATE-546): run a list of tasks in the running FERB, on one connection, in test mode's question
recorder, and write each task's results to a report outside the repository.

    uv run python scripts/question_run.py TASKS --runtime anthropic --out ~/ferb-runs/run1.json [--repeat 3]

TASKS is a text file, one task per line ("||" splits the turns of a multi-turn task, "#" starts a comment), or a JSON
list of {"id", "turns": [...]}. A run sends every task, in file order, into one chat (GATE-552: one provider runs the
whole test, the next gets a fresh chat), so context builds up as in real use (--chat-per-task: a fresh chat per task).
Every message is tagged with the run and the connection (GATE-550), so no run sees another's chats, summary, memory,
learning or activity, and FERB learns nothing from any of them. The run
needs test mode with questions recorded (Settings, or `--enable "me@example.com" ...` here, which turns it on with that
send allowlist; it stays on after). The connection is used for every role during the run and the roles go back as
they were at the end.

The run tool's own limits (not FERB's): --turn-limit minutes per turn (default 20), after which the runner stops that
turn's task through FERB's stop and records it; --cost-cap dollars for the whole run (unset by default), after which
no new task starts. The report is rewritten after every turn.

Per task: start and end, seconds (each step's and each model call's too), steps (tool calls), model calls, tokens
(input, cache reads, cache writes by TTL, output), cost, each question FERB recorded (card, rules, the table's answer,
when), the typed-answer forms and repairs (RUN-440), the outcome and FERB's final answer, and the acting calls that may
have left something in the owner's accounts (a cart add, a draft: check by hand). Nothing is cleaned up: FERB closes
only its own tabs (HAND-201). scripts/question_compare.py puts several connections' reports side by side.

Per turn (RUN-443, RUN-444) the CPU and memory of FERB's daemon, its child processes, FERB.app, FERB's browser native
hosts and each running browser (see Meter), and the other FERB tasks and resource_samples rows that overlapped the turn."""

import argparse
import json
import os
import re
import sqlite3
import subprocess
import sys
import threading
import time
from collections.abc import Callable
from datetime import datetime
from pathlib import Path
from typing import Any, Protocol

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from ferb.cli.client import Client  # noqa: E402
from ferb.paths import ferb_home, socket_path  # noqa: E402

# Calls whose kind may leave something behind in an account or on the Mac (rules.Kind): everything but reads,
# navigation, search, tabs, snapshots and stopping the task's own processes
LEAVES_NOTHING = frozenset({"read", "navigate", "search", "open_tab", "snapshot", "stop_process", "mailbox_read"})
USAGE_KEYS = ("input_tokens", "cached_input_tokens", "cache_write_tokens", "cache_write_5m_tokens",
              "cache_write_1h_tokens", "output_tokens", "reasoning_tokens")

# RUN-444: what each turn's CPU and memory figures are made of. FERB's groups: the daemon (the process running
# `-m ferb.daemon`), everything under it (Codex / Claude Code, MCP shims, sandboxed shell commands, ffmpeg...), FERB.app's
# FERBNotch and its children, and FERB's native hosts (`ferb.native_host`, started by the browsers). Each browser is
# its bundle's processes and their children.
BROWSERS = ("Google Chrome", "Brave Browser", "Chromium", "Microsoft Edge", "Arc", "Dia", "Opera", "Vivaldi")
FERB_GROUPS = ("daemon", "daemon_children", "app", "native_host")
SAMPLE_S = 2.0                       # memory peaks and the busiest children; CPU comes from the kernel's counters
DAEMON = re.compile(r"(^|\s)-m ferb\.daemon(\s|$)")
BROWSER_BASIS = "upper bound: includes the owner's own browsing during the turn"
FERB_BASIS = "the turn's task (the run is sequential), plus any background work listed under background"
CPU_NOTE = ("CPU: the kernel's cumulative counters (each process's own user+system CPU plus that of its ended children "
            "it waited for) summed over the group's live processes at the turn's start and end, so a command that "
            "starts and ends within the turn is counted without sampling; lost only if it was re-parented away from "
            f"the group (e.g. a double fork). Memory: summed RSS and physical footprint, sampled every {SAMPLE_S:g} s "
            "(peak and end). cpu_pct is CPU seconds / wall seconds (100 = one core).")


class Usage(Protocol):
    """ferb.daemon.resources.MacReadings in a run (libproc's proc_pid_rusage), a fake in tests."""

    def process(self, pid: int) -> Any: ...             # .start, .cpu_s, .children_cpu_s, .footprint; None: gone
    def path(self, pid: int) -> str | None: ...


def ps_text() -> str:
    return subprocess.run(["/bin/ps", "-axww", "-o", "pid=,ppid=,rss=,command="], capture_output=True, text=True,
                          timeout=20, check=True).stdout


def ps_table(text: str) -> dict[int, tuple[int, int, str]]:
    """pid -> (ppid, rss KB, command) from `ps -o pid=,ppid=,rss=,command=`."""
    out = {}
    for line in text.splitlines():
        parts = line.split(None, 3)
        if len(parts) == 4 and parts[0].isdigit() and parts[1].isdigit() and parts[2].isdigit():
            out[int(parts[0])] = (int(parts[1]), int(parts[2]), parts[3])
    return out


def groups(table: dict[int, tuple[int, int, str]]) -> dict[int, str]:
    """Each process's group (RUN-444); processes in none are left out. The daemon's tree first, then FERB.app's, the
    native hosts, then each browser's bundle and its children."""
    kids: dict[int, list[int]] = {}
    for pid, (ppid, _, _) in table.items():
        kids.setdefault(ppid, []).append(pid)
    got: dict[int, str] = {}

    def tree(root: int, name: str, below: str) -> None:
        got.setdefault(root, name)
        todo = list(kids.get(root, []))
        while todo:
            pid = todo.pop()
            if pid not in got:
                got[pid] = below
                todo += kids.get(pid, [])
    for pid, (_, _, cmd) in table.items():
        if DAEMON.search(cmd):
            tree(pid, "daemon", "daemon_children")
    for pid, (_, _, cmd) in table.items():
        if ".app/Contents/MacOS/FERBNotch" in cmd:
            tree(pid, "app", "app")
    for pid, (_, _, cmd) in table.items():
        if "ferb.native_host" in cmd:
            got.setdefault(pid, "native_host")
    for name in BROWSERS:
        for pid, (_, _, cmd) in table.items():
            if f"/{name}.app/" in cmd:
                tree(pid, f"browser:{name}", f"browser:{name}")
    return got


def label(path: str | None, cmd: str) -> str:
    """A process's name for the busiest list: its executable's basename, with the module for `python -m X`."""
    name = os.path.basename(path or cmd.split(" ", 1)[0]) or "unknown"
    module = re.search(r"(?:^|\s)-m\s+(\S+)", cmd)
    return f"{name} -m {module.group(1)}" if module and name.startswith("python") else name


class Meter:
    """One turn's CPU and memory by group (RUN-444): `start` before the turn, `stop` after it. Between them a thread
    looks every SAMPLE_S seconds for memory peaks and the daemon's busiest children."""

    def __init__(self, usage: Usage, ps: Callable[[], str] | None = None, every: float = SAMPLE_S,
                 clock: Callable[[], float] = time.monotonic) -> None:
        self.usage, self.ps, self.every, self.clock = usage, ps or ps_text, every, clock
        self.problems: list[str] = []
        self.peak: dict[str, dict[str, float]] = {}
        self.first: dict[tuple[int, int], tuple[str, str, float]] = {}   # key -> (group, name, CPU when first seen)
        self.last: dict[tuple[int, int], float] = {}
        self.at_start: set[tuple[int, int]] = set()
        self.daemons: set[tuple[int, int]] = set()            # the daemon process(es) at the last look
        self.begin: dict[str, dict[str, float]] | None = None
        self.begin_daemons: set[tuple[int, int]] = set()
        self.samples = 0
        self._done = threading.Event()
        self._thread: threading.Thread | None = None

    def look(self) -> dict[str, dict[str, float]] | None:
        """One look: per group, the CPU counters' sum (own CPU, with the daemon's ended children credited to
        daemon_children), the processes, RSS and footprint in MB; peaks and per-process CPU kept. None: ps failed."""
        try:
            table = ps_table(self.ps())
        except (OSError, subprocess.SubprocessError) as e:
            self.problems.append(f"ps: {e}")
            return None
        out: dict[str, dict[str, float]] = {}
        self.daemons = set()
        for pid, group in groups(table).items():
            proc = self.usage.process(pid)
            if proc is None:
                continue
            ended = "daemon_children" if group == "daemon" else group
            for g in (group, ended):
                out.setdefault(g, {"cpu_s": 0.0, "processes": 0, "rss_mb": 0.0, "footprint_mb": 0.0})
            out[group]["cpu_s"] += proc.cpu_s
            out[ended]["cpu_s"] += proc.children_cpu_s
            out[group]["processes"] += 1
            out[group]["rss_mb"] += table[pid][1] / 1024
            out[group]["footprint_mb"] += proc.footprint / 2**20
            key = (pid, int(proc.start))
            if group == "daemon":
                self.daemons.add(key)
            if not self.samples:
                self.at_start.add(key)
            self.first.setdefault(key, (group, label(self.usage.path(pid), table[pid][2]),
                                        proc.cpu_s + proc.children_cpu_s if not self.samples else 0.0))
            self.last[key] = proc.cpu_s + proc.children_cpu_s
        for g, v in out.items():
            peak = self.peak.setdefault(g, {"rss_mb": 0.0, "footprint_mb": 0.0})
            for k in peak:
                peak[k] = max(peak[k], v[k])
        self.samples += 1
        return out

    def start(self) -> None:
        self.t0, self.at0 = self.clock(), datetime.now().astimezone().isoformat()
        self.begin = self.look()
        self.begin_daemons = set(self.daemons) if self.begin is not None else set()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _run(self) -> None:
        while not self._done.wait(self.every):
            self.look()

    def stop(self) -> dict[str, Any]:
        self._done.set()
        if self._thread:
            self._thread.join(30)
        end, wall = self.look(), max(self.clock() - self.t0, 1e-9)
        result: dict[str, Any] = {"from": self.at0, "to": datetime.now().astimezone().isoformat(),
                                  "wall_s": round(wall, 1), "sample_every_s": self.every, "samples": self.samples}
        if self.begin is None or end is None:          # no start or end figures: no difference to give
            result["problems"] = sorted(set(self.problems))
            return result
        if self.begin_daemons and self.daemons != self.begin_daemons:
            self.problems.append("FERB's daemon restarted during the turn: the daemon and daemon_children CPU figures are unreliable")
        result["groups"] = {}
        for g in sorted(set(self.begin) | set(end), key=lambda g: (g not in FERB_GROUPS, g)):
            b, e = self.begin.get(g, {}), end.get(g, {})
            cpu = max(0.0, e.get("cpu_s", 0.0) - b.get("cpu_s", 0.0))
            row = {"cpu_s": round(cpu, 2), "cpu_pct": round(100 * cpu / wall, 1),
                   "peak_rss_mb": round(self.peak.get(g, {}).get("rss_mb", 0.0), 1),
                   "end_rss_mb": round(e.get("rss_mb", 0.0), 1),
                   "peak_footprint_mb": round(self.peak.get(g, {}).get("footprint_mb", 0.0), 1),
                   "end_footprint_mb": round(e.get("footprint_mb", 0.0), 1),
                   "processes_end": int(e.get("processes", 0)),
                   "basis": BROWSER_BASIS if g.startswith("browser:") else FERB_BASIS}
            if g == "daemon_children" or g.startswith("browser:"):
                row["started_in_turn"] = sum(1 for k, (grp, _, _) in self.first.items()
                                             if grp == g and k not in self.at_start)
            if g == "daemon_children":
                row["busiest"] = self.busiest(g)
            result["groups"][g] = row
        ferb = [result["groups"][g] for g in FERB_GROUPS if g in result["groups"]]
        result["ferb_cpu_s"] = round(sum(r["cpu_s"] for r in ferb), 2)
        result["ferb_cpu_pct"] = round(100 * result["ferb_cpu_s"] / wall, 1)
        result["browser_cpu_s"] = round(sum(r["cpu_s"] for g, r in result["groups"].items()
                                            if g.startswith("browser:")), 2)
        if "daemon" not in result["groups"]:
            self.problems.append("no process running -m ferb.daemon was found")
        if self.problems:
            result["problems"] = sorted(set(self.problems))
        return result

    def busiest(self, group: str, top: int = 5) -> list[list[Any]]:
        """The group's CPU by process name, from the looks (a process's CPU after its last look is in its parent's)."""
        by: dict[str, float] = {}
        for key, (grp, name, base) in self.first.items():
            if grp == group:
                by[name] = by.get(name, 0.0) + max(0.0, self.last[key] - base)
        return [[n, round(s, 2)] for n, s in sorted(by.items(), key=lambda x: -x[1])[:top] if s > 0]


def background(db: Path, start: str, end: str, own_trees: set[str]) -> dict[str, Any]:
    """FERB work that may have run beside the turn (RUN-444), read-only from FERB's database: other task trees active
    in the turn's window (no goals), and the resource_samples rows whose 5-minute windows overlap it."""
    if not db.exists():
        return {"problem": f"no database at {db}"}
    t0, t1 = datetime.fromisoformat(start), datetime.fromisoformat(end)
    try:
        conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=5)
        try:
            tasks = conn.execute("SELECT tree_id, role, status, created_at, updated_at FROM tasks WHERE "
                                 "status IN ('planned', 'running', 'paused') OR updated_at >= ?",
                                 (t0.isoformat(),)).fetchall()
            rows = conn.execute("SELECT at, window_s, cpu_s, child_cpu_s, app_cpu_s, work_json FROM resource_samples "
                                "WHERE at >= ? ORDER BY id", (t0.isoformat(),)).fetchall()
        finally:
            conn.close()
    except sqlite3.Error as e:
        return {"problem": f"database: {e}"}
    others: dict[str, dict[str, Any]] = {}
    for tree, role, status, created, updated in tasks:
        if tree in own_trees or datetime.fromisoformat(created) > t1 or \
                (status not in ('planned', 'running', 'paused') and datetime.fromisoformat(updated) < t0):
            continue
        row = others.setdefault(tree, {"tree_id": tree, "roles": [], "statuses": [], "created_at": created,
                                       "updated_at": updated})
        row["roles"].append(role)
        row["statuses"].append(status)
        row["created_at"], row["updated_at"] = min(row["created_at"], created), max(row["updated_at"], updated)
    samples = []
    for at, window_s, cpu_s, child_cpu_s, app_cpu_s, work in rows:
        end_at = datetime.fromisoformat(at)
        if end_at.timestamp() - window_s > t1.timestamp():
            continue
        w = json.loads(work or "{}")
        samples.append({"at": at, "window_s": window_s, "daemon_cpu_s": cpu_s, "child_cpu_s": child_cpu_s,
                        "app_cpu_s": app_cpu_s} | {k: w.get(k) for k in ("tasks_running", "tool_calls", "models",
                                                                         "perception", "top")})
    return {"other_tasks": list(others.values()), "resource_samples": samples}


def roots(turn: dict[str, Any]) -> set[str]:
    return {t.split(".", 1)[0] for t in turn.get("task_ids") or []}


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
    calls took, and the planner's own repair turns after a failed answer (and how many of them ended without error).
    RUN-446: empty answers' follow-ups (asked, then fixed or failed); RUN-445: runs stopped at the wrong-call cap;
    ORCH-558: the planner's last tool-free calls by why, and how many ended without error."""
    by_form: dict[str, int] = {}
    repairs = {"asked": 0, "fixed": 0, "failed": 0}
    follow_ups = {"asked": 0, "fixed": 0, "failed": 0}
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
        if out.get("follow_up") in follow_ups:
            follow_ups[out["follow_up"]] += 1
    planner = [n for n in nodes if n["kind"] == "model.turn" and (n.get("input") or {}).get("repair")]
    last = [n for n in nodes if n["kind"] == "model.turn" and (n.get("input") or {}).get("last_call")]
    last_calls: dict[str, int] = {}
    for n in last:
        why = str(n["input"]["last_call"])
        last_calls[why] = last_calls.get(why, 0) + 1
    return {"calls": by_form, "repairs": repairs, "repair_seconds": round(extra_ms / 1000, 1),
            "planner_repairs": len(planner), "planner_repairs_saved": sum(1 for n in planner if not n.get("error")),
            "follow_ups": follow_ups, "last_calls": last_calls,
            "last_calls_saved": sum(1 for n in last if not n.get("error")),
            "tool_loops": sum(1 for n in nodes if n["kind"] == "model.tool_loop")}


def add_forms(total: dict[str, Any], one: dict[str, Any]) -> dict[str, Any]:
    parts = ("calls", "repairs", "follow_ups", "last_calls")
    out: dict[str, Any] = {part: dict(total.get(part) or {}) for part in parts}
    for part in parts:
        for k, v in one[part].items():
            out[part][k] = out[part].get(k, 0) + v
    for k in ("repair_seconds", "planner_repairs", "planner_repairs_saved", "last_calls_saved", "tool_loops"):
        out[k] = round(total.get(k, 0) + one[k], 1)
    return out


# GATE-553: the reply the runner gives FERB's clarifying question (the line record mode gives a task's own question,
# GATE-546), so the next task's text never answers it; at most this many in one turn
DECLINE = "No answer in this test run; decide yourself or report what you need"
MAX_CLARIFY = 3


def send(session: str, text: str, tag: dict[str, str], limit_s: float) -> dict[str, Any]:
    """One chat turn, streamed to its end (the turn's tasks finished), on a connection of its own. When FERB answers
    with a clarifying question and no task started, the question is recorded and the runner replies DECLINE in the same
    chat, and waits on (GATE-553): the next task is only sent once this turn ended. Past `limit_s` (the whole turn) the
    runner stops the turn's tasks through FERB's own stop (task.stop) and waits for the stream to end."""
    started = time.monotonic()
    turn: dict[str, Any] = {"text": text, "request_id": None, "task_ids": [], "replies": [], "outcomes": [],
                            "clarify": [], "started_at": datetime.now().astimezone().isoformat(timespec="seconds")}

    def stream(message: str, asked: list[str]) -> None:
        c = Client(socket_path(), timeout=60)
        try:
            for ev in c.stream("chat.send", {"session_id": session, "text": message, "test_run": tag}):
                turn["request_id"] = turn["request_id"] or ev.get("request_id")
                if ev.get("task_id") and ev["task_id"] not in turn["task_ids"]:
                    turn["task_ids"].append(ev["task_id"])
                if ev.get("type") == "reply":
                    turn["replies"].append({"text": ev.get("text", ""), "status": ev.get("status")})
                elif ev.get("type") == "clarify":
                    asked.append(str(ev.get("text", "")))
                elif ev.get("type") == "outcome":
                    turn["outcomes"].append({k: ev.get(k) for k in ("task_id", "status", "summary")})
        except Exception as e:                      # FERB's words, kept with the turn
            turn.setdefault("problems", []).append(f"chat.send: {e}")
        finally:
            c.close()
    message = text
    for _ in range(MAX_CLARIFY + 1):
        asked: list[str] = []
        worker = threading.Thread(target=stream, args=(message, asked), daemon=True)
        worker.start()
        worker.join(max(limit_s - (time.monotonic() - started), 0.0))
        if worker.is_alive():
            stop_turn(turn, limit_s)
            worker.join(120)
            break
        if not asked or turn["task_ids"]:
            break
        for question in asked:                      # recorded like FERB's other questions, then declined
            turn["clarify"].append({"at": datetime.now().astimezone().isoformat(timespec="seconds"),
                                    "question": question, "answer": DECLINE})
        message = DECLINE
    turn["ended_at"] = datetime.now().astimezone().isoformat(timespec="seconds")
    turn["seconds"] = round(time.monotonic() - started, 1)
    return turn


def stop_turn(turn: dict[str, Any], limit_s: float) -> None:
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
    questions += [{"at": q["at"], "kind": "clarify", "ferb_question": True, "question": q["question"],
                   "answer": q["answer"], "recorded": True} for q in turn.get("clarify") or []]   # GATE-553
    answer = next((e["answer"] for e in reversed(episodes) if e.get("answer")), "") or \
        next((o["summary"] for o in reversed(turn["outcomes"]) if o.get("summary")), "") or \
        next((r["text"] for r in reversed(turn["replies"]) if r.get("text")), "")
    return {**turn, "questions": questions, "steps": len(steps), "step_times": steps,
            "model_calls": len(model_calls), "model_call_times": model_calls, "usage": usage, "typed": forms,
            "cost_usd": round(cost, 6), "unpriced_models": sorted(unpriced), "episodes": episodes,
            "final_answer": answer, "possible_leftovers": leftovers}


def sum_up(row: dict[str, Any]) -> None:
    """A task's totals from its turns so far (each turn keeps its own figures)."""
    turns = row["turns"]
    row.update({"started_at": turns[0]["started_at"], "ended_at": turns[-1]["ended_at"],
                "stopped_by_runner": next((t["stopped_by_runner"] for t in turns if t.get("stopped_by_runner")), ""),
                "final_answer": turns[-1]["final_answer"], "seconds": round(sum(t["seconds"] for t in turns), 1),
                "questions": sum(len(t["questions"]) for t in turns), "steps": sum(t["steps"] for t in turns),
                "cost_usd": round(sum(t["cost_usd"] for t in turns), 6), "typed": {}})
    for t in turns:
        row["typed"] = add_forms(row["typed"], t["typed"])
    measured = [t["resources"] for t in turns if "groups" in (t.get("resources") or {})]
    if measured:                                       # RUN-444: the task's CPU by group, its turns added up
        wall = sum(r["wall_s"] for r in measured)
        cpu: dict[str, float] = {}
        peak: dict[str, float] = {}
        for r in measured:
            for g, v in r["groups"].items():
                cpu[g] = round(cpu.get(g, 0.0) + v["cpu_s"], 2)
                peak[g] = max(peak.get(g, 0.0), v["peak_footprint_mb"])
        ferb = round(sum(r["ferb_cpu_s"] for r in measured), 2)
        row["resources"] = {"wall_s": round(wall, 1), "ferb_cpu_s": ferb,
                            "ferb_cpu_pct": round(100 * ferb / wall, 1) if wall else 0.0,
                            "browser_cpu_s": round(sum(r["browser_cpu_s"] for r in measured), 2),
                            "cpu_s": cpu, "peak_footprint_mb": peak,
                            "other_tasks": sum(len((r.get("background") or {}).get("other_tasks") or [])
                                               for r in measured)}


def mac_usage() -> tuple[Usage | None, str]:
    """The kernel's per-process numbers (libproc), as the daemon's own resource log reads them."""
    try:
        from ferb.daemon.resources import MacReadings
        return MacReadings(), ""
    except OSError as e:
        return None, f"libproc can't be read: {e}"


def measured(usage: Usage | None, why: str, client: Client, session: str, text: str, tag: dict[str, str],
             limit_s: float, db: Path) -> dict[str, Any]:
    """One turn sent and collected, with its CPU and memory (RUN-444) and the FERB work beside it."""
    meter = Meter(usage) if usage else None
    if meter:
        meter.start()
    turn = send(session, text, tag, limit_s)
    resources: dict[str, Any] = meter.stop() if meter else {"problems": [why]}
    got = collect(client, turn)
    if "from" in resources:
        resources["background"] = background(db, resources["from"], resources["to"], roots(got))
    return got | {"resources": resources}


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
    p.add_argument("--turn-limit", "--task-limit", dest="turn_limit", type=float, default=20.0,
                   help="minutes per turn before the runner stops that turn's task")
    p.add_argument("--chat-per-task", action="store_true",
                   help="each task in a fresh chat (default: the whole file in one chat per run, as real use)")
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
                              "run": stamp, "test_mode": mode, "turn_limit_min": args.turn_limit,
                              "chat": "per task" if args.chat_per_task else "one per run",
                              "cost_cap_usd": args.cost_cap,
                              "resources_note": {"cpu": CPU_NOTE, "ferb_groups": FERB_BASIS,
                                                 "browsers": BROWSER_BASIS}, "tasks": []}
    usage, why = mac_usage()
    db = ferb_home() / "ferb.db"
    tag = {"run": stamp, "connection": args.runtime}
    out.parent.mkdir(parents=True, exist_ok=True)
    try:
        for run in range(1, args.repeat + 1):                 # GATE-552: each run is the whole file, in order
            for task in tasks:
                spent = sum(t["cost_usd"] for t in report["tasks"])
                if args.cost_cap is not None and spent >= args.cost_cap:
                    report["stopped"] = f"the cost cap ${args.cost_cap:g} was reached (${spent:.4f})"
                    raise StopIteration
                session = (f"qrun-{stamp}-{args.runtime}-{task['id']}-{run}" if args.chat_per_task
                           else f"qrun-{stamp}-{args.runtime}-{run}")   # one chat for the run: context builds up
                row: dict[str, Any] = {"id": task["id"], "run": run, "session": session, "turns": []}
                report["tasks"].append(row)
                for text in task["turns"]:
                    row["turns"].append(measured(usage, why, client, session, text, tag, args.turn_limit * 60, db))
                    sum_up(row)
                    report["typed"] = {}
                    for t in (t for r in report["tasks"] for t in r["turns"]):
                        report["typed"] = add_forms(report["typed"], t["typed"])     # the run's summary
                    out.write_text(json.dumps(report, ensure_ascii=False, indent=2))   # after every turn
                print(f"{task['id']} run {run}: {row['seconds']} s, {row['steps']} steps, {row['questions']} "
                      f"questions, ${row['cost_usd']}, FERB CPU "
                      f"{(row.get('resources') or {}).get('ferb_cpu_s', '?')} s")
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
        for row in report["tasks"]:                     # resource_samples rows written after their turn ended
            for t in row["turns"]:
                if "from" in (t.get("resources") or {}):
                    t["resources"]["background"] = background(db, t["resources"]["from"], t["resources"]["to"],
                                                              roots(t))
            if row["turns"]:
                sum_up(row)
        report["ended"] = datetime.now().isoformat(timespec="seconds")
        out.write_text(json.dumps(report, ensure_ascii=False, indent=2))
    print(f"report: {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
