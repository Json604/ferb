"""Developer toolbox (GATE-551): put several connections' question_run.py reports side by side, one markdown table
per task (a row per connection and run), and print it or write it to a file outside the repository.

    uv run python scripts/question_compare.py ~/ferb-runs/codex.json ~/ferb-runs/anthropic.json [--out table.md]"""

import argparse
import json
import sys
from pathlib import Path
from typing import Any

COLUMNS = ("connection", "run", "outcome", "seconds", "steps", "model calls", "questions", "input", "cache read",
           "write 5m", "write 1h", "output", "cost $", "typed forms", "repairs", "stopped", "final answer")


def cell(value: Any) -> str:
    return " ".join(str(value).split()).replace("|", "\\|")[:120]


def row(report: dict[str, Any], task: dict[str, Any]) -> list[str]:
    turns = task.get("turns") or []
    usage = {k: sum(int((t.get("usage") or {}).get(k) or 0) for t in turns)
             for k in ("input_tokens", "cached_input_tokens", "cache_write_5m_tokens", "cache_write_1h_tokens",
                       "output_tokens")}
    typed = task.get("typed") or {}
    statuses = [o.get("status") for t in turns for o in t.get("outcomes") or []] or \
        [r.get("status") or "reply" for t in turns for r in t.get("replies") or []][-1:]
    return [report.get("runtime", "?"), str(task.get("run", 1)), "/".join(str(s) for s in statuses if s) or "?",
            str(task.get("seconds")), str(task.get("steps")), str(sum(t.get("model_calls", 0) for t in turns)),
            str(task.get("questions")), *(str(usage[k]) for k in usage), f"{task.get('cost_usd', 0):.4f}",
            ", ".join(f"{k} {v}" for k, v in (typed.get("calls") or {}).items()),
            ", ".join(f"{k} {v}" for k, v in (typed.get("repairs") or {}).items() if v),
            task.get("stopped_by_runner") or "", task.get("final_answer") or ""]


def table(reports: list[dict[str, Any]]) -> str:
    by_task: dict[str, list[list[str]]] = {}
    texts: dict[str, str] = {}
    for report in reports:
        for task in report.get("tasks") or []:
            by_task.setdefault(str(task["id"]), []).append(row(report, task))
            texts.setdefault(str(task["id"]), " / ".join(t.get("text", "") for t in task.get("turns") or []))
    out = []
    for ident, rows in by_task.items():
        out += [f"## Task {ident}: {cell(texts[ident])}", "", "| " + " | ".join(COLUMNS) + " |",
                "|" + "---|" * len(COLUMNS)] + ["| " + " | ".join(cell(c) for c in r) + " |" for r in rows] + [""]
    return "\n".join(out)


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("reports", type=Path, nargs="+")
    p.add_argument("--out", type=Path)
    args = p.parse_args()
    text = table([json.loads(path.expanduser().read_text()) for path in args.reports])
    if args.out:
        args.out.expanduser().write_text(text)
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
