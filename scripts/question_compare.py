"""Developer toolbox (GATE-551): put several connections' question_run.py reports side by side, one markdown table
per task (a row per connection and run), then the totals per connection and run, and print it or write it to a file
outside the repository. CPU (RUN-444): FERB's processes (daemon, its children, FERB.app, native hosts) are the task's;
browser CPU is an upper bound that includes the owner's own browsing; "other tasks" counts FERB task trees that ran
beside the task's turns.

    uv run python scripts/question_compare.py ~/ferb-runs/codex.json ~/ferb-runs/anthropic.json [--out table.md]
        [--db ~/.ferb/ferb.db]

GATE-553: a report made by the runner before the clarify fix has no `clarify` records; there a turn FERB answered with
a clarifying question (with --db: a question.answered row for its request; without: no task, reply or outcome) had the
next message taken as its answer: the next task's (both tasks are marked "tangled by the old runner") or the task's own
next turn (that task is). Totals add a row without them. --db is read only (SQLite read-only mode)."""

import argparse
import json
import sqlite3
import sys
from pathlib import Path
from typing import Any

COLUMNS = ("connection", "run", "outcome", "seconds", "steps", "model calls", "questions", "input", "cache read",
           "write 5m", "write 1h", "output", "cost $", "FERB CPU s", "FERB CPU %", "daemon s", "children s", "app s",
           "browsers s (upper bound)", "peak FERB MB", "other tasks", "typed forms", "repairs", "stopped", "note",
           "final answer")
TOTALS = ("connection", "run", "tasks", "seconds", "steps", "cost $", "FERB CPU s", "FERB CPU %", "daemon s",
          "children s", "app s", "browsers s (upper bound)")


def cell(value: Any) -> str:
    return " ".join(str(value).split()).replace("|", "\\|")[:120]


def cpu(task: dict[str, Any]) -> list[str]:
    """RUN-444: the task's CPU columns (blank when the run had no measurement)."""
    res = task.get("resources")
    if not res:
        return [""] * 8
    by = res.get("cpu_s") or {}
    peak = res.get("peak_footprint_mb") or {}
    ferb_mb = sum(v for g, v in peak.items() if not g.startswith("browser:"))
    return [f"{res['ferb_cpu_s']:.1f}", f"{res['ferb_cpu_pct']:.1f}",
            *(f"{by.get(g, 0):.1f}" for g in ("daemon", "daemon_children", "app")),
            f"{res['browser_cpu_s']:.1f}", f"{ferb_mb:.0f}", str(res.get("other_tasks", 0))]


TANGLED = "tangled by the old runner"


def answered_requests(db: Path | None) -> set[str]:
    """The requests whose clarifying question got an answer (the journal's question.answered rows), read only."""
    if db is None:
        return set()
    conn = sqlite3.connect(f"file:{db.expanduser()}?mode=ro", uri=True)
    try:
        return {str(r[0]) for r in conn.execute("SELECT json_extract(data_json, '$.request_id') FROM journal "
                                                "WHERE kind = 'question.answered'") if r[0]}
    finally:
        conn.close()


def old_runner(report: dict[str, Any]) -> bool:
    """A report from the runner before GATE-553: its turns carry no `clarify` records."""
    return any("clarify" not in t for task in report.get("tasks") or [] for t in task.get("turns") or [])


def tangled(report: dict[str, Any], answered: set[str]) -> set[int]:
    """Indexes of the report's tasks the old runner tangled: a turn FERB answered with a clarifying question whose
    answer was the next message (the task's own next turn: that task; the next task's: both)."""
    if not old_runner(report):
        return set()
    tasks = report.get("tasks") or []
    marked: set[int] = set()
    for i, task in enumerate(tasks):
        turns = task.get("turns") or []
        for j, turn in enumerate(turns):
            if turn.get("task_ids"):
                continue
            asked = (str(turn.get("request_id")) in answered) if answered else \
                (not turn.get("replies") and not turn.get("outcomes"))
            if not asked:
                continue
            if j + 1 < len(turns):
                marked.add(i)
            elif i + 1 < len(tasks) and tasks[i + 1].get("session") == task.get("session"):
                marked |= {i, i + 1}
    return marked


def row(report: dict[str, Any], task: dict[str, Any], note: str = "") -> list[str]:
    turns = task.get("turns") or []
    usage = {k: sum(int((t.get("usage") or {}).get(k) or 0) for t in turns)
             for k in ("input_tokens", "cached_input_tokens", "cache_write_5m_tokens", "cache_write_1h_tokens",
                       "output_tokens")}
    typed = task.get("typed") or {}
    statuses = [o.get("status") for t in turns for o in t.get("outcomes") or []] or \
        [r.get("status") or "reply" for t in turns for r in t.get("replies") or []][-1:]
    return [report.get("runtime", "?"), str(task.get("run", 1)), "/".join(str(s) for s in statuses if s) or "?",
            str(task.get("seconds")), str(task.get("steps")), str(sum(t.get("model_calls", 0) for t in turns)),
            str(task.get("questions")), *(str(usage[k]) for k in usage), f"{task.get('cost_usd', 0):.4f}", *cpu(task),
            ", ".join(f"{k} {v}" for k, v in (typed.get("calls") or {}).items()),
            ", ".join(f"{k} {v}" for k, v in (typed.get("repairs") or {}).items() if v),
            task.get("stopped_by_runner") or "", note, task.get("final_answer") or ""]


def totals(report: dict[str, Any], marked: set[int] = frozenset()) -> list[list[str]]:  # type: ignore[assignment]
    """One row per run of a connection's report: its tasks added up; with tangled tasks (GATE-553), a second row
    without them."""
    runs: dict[str, list[dict[str, Any]]] = {}
    for i, task in enumerate(report.get("tasks") or []):
        runs.setdefault(str(task.get("run", 1)), []).append(task)
        if marked and i not in marked:
            runs.setdefault(f"{task.get('run', 1)} without tangled", []).append(task)
    out = []
    for run, tasks in sorted(runs.items()):
        res = [t["resources"] for t in tasks if t.get("resources")]
        wall = sum(r["wall_s"] for r in res)
        ferb = sum(r["ferb_cpu_s"] for r in res)

        def by(g: str, res: list[dict[str, Any]] = res) -> str:
            return f"{sum((r.get('cpu_s') or {}).get(g, 0) for r in res):.1f}" if res else ""
        out.append([report.get("runtime", "?"), str(run), str(len(tasks)),
                    f"{sum(t.get('seconds') or 0 for t in tasks):.1f}", str(sum(t.get("steps") or 0 for t in tasks)),
                    f"{sum(t.get('cost_usd') or 0 for t in tasks):.4f}",
                    f"{ferb:.1f}" if res else "", f"{100 * ferb / wall:.1f}" if wall else "",
                    by("daemon"), by("daemon_children"), by("app"),
                    f"{sum(r['browser_cpu_s'] for r in res):.1f}" if res else ""])
    return out


def table(reports: list[dict[str, Any]], answered: set[str] = frozenset()) -> str:  # type: ignore[assignment]
    by_task: dict[str, list[list[str]]] = {}
    texts: dict[str, str] = {}
    marks = [tangled(report, answered) for report in reports]
    notes = []
    for report, marked in zip(reports, marks):
        for i, task in enumerate(report.get("tasks") or []):
            by_task.setdefault(str(task["id"]), []).append(row(report, task, TANGLED if i in marked else ""))
            texts.setdefault(str(task["id"]), " / ".join(t.get("text", "") for t in task.get("turns") or []))
        if old_runner(report):
            notes.append(f"Note: {report.get('runtime', '?')} (run {report.get('run', '?')}) ran with the runner before "
                         "the clarify fix (GATE-553): a clarifying question took the next message as its answer; "
                         f"{len(marked)} tasks are marked {TANGLED!r}. Runs without this note used the fixed runner.")
    out = notes + ([""] if notes else [])
    for ident, rows in by_task.items():
        out += [f"## Task {ident}: {cell(texts[ident])}", "", "| " + " | ".join(COLUMNS) + " |",
                "|" + "---|" * len(COLUMNS)] + ["| " + " | ".join(cell(c) for c in r) + " |" for r in rows] + [""]
    out += ["## Totals", "", "| " + " | ".join(TOTALS) + " |", "|" + "---|" * len(TOTALS)]
    out += ["| " + " | ".join(cell(c) for c in r) + " |"
            for report, marked in zip(reports, marks) for r in totals(report, marked)] + [""]
    return "\n".join(out)


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("reports", type=Path, nargs="+")
    p.add_argument("--out", type=Path)
    p.add_argument("--db", type=Path, help="FERB's database, read only, to find answered clarifying questions")
    args = p.parse_args()
    text = table([json.loads(path.expanduser().read_text()) for path in args.reports], answered_requests(args.db))
    if args.out:
        args.out.expanduser().write_text(text)
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
