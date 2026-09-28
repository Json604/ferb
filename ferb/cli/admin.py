"""Admin commands. Build step 1: `ferb trace <task-id>` and `ferb daemon run`."""

import json
from datetime import datetime
from typing import Any, TextIO

from ferb.cli.client import Client


def render_trace(result: dict[str, Any]) -> list[str]:
    if not result["spans"]:
        return [f"no spans for task {result['task_id']}"]
    lines = [f"task {result['task_id']}"]

    def walk(node: dict[str, Any], prefix: str, last: bool) -> None:
        start = datetime.fromisoformat(node["started_at"])
        if node["ended_at"]:
            dur = f"{(datetime.fromisoformat(node['ended_at']) - start).total_seconds():.3f}s"
        else:
            dur = "running"
        status = f" ERROR {node['error']}" if node["error"] else ""
        cost = f" ${node['cost_usd']:.4f}" if node["cost_usd"] is not None else ""
        args = json.dumps(node["input"], separators=(",", ":"), ensure_ascii=False)
        if len(args) > 60:
            args = args[:57] + "..."
        lines.append(f"{prefix}{'└─' if last else '├─'} {node['kind']} [{node['id']}] {dur}{cost}{status} {args}")
        kids = node["children"]
        for i, child in enumerate(kids):
            walk(child, prefix + ("   " if last else "│  "), i == len(kids) - 1)

    roots = result["spans"]
    for i, root in enumerate(roots):
        walk(root, "", i == len(roots) - 1)
    return lines


def trace(client: Client, task_id: str, out: TextIO) -> int:
    for line in render_trace(client.call("trace.get", {"task_id": task_id})):
        print(line, file=out)
    return 0
