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


def stop(client: Client, out: TextIO) -> int:
    r = client.call("stop.engage")
    print(f"FERB stopped: {r['tokens_revoked']} run token(s) revoked, {r['holds_undone']} pending send(s) cancelled. "
          "`ferb resume-tasks` lifts it.", file=out)
    for name, err in r.get("hook_errors", {}).items():
        print(f"  warning: stop hook {name} failed ({err})", file=out)
    return 0


def resume(client: Client, out: TextIO) -> int:
    client.call("stop.lift")
    print("FERB resumed: actions are allowed again.", file=out)
    return 0


def undo(client: Client, hold_id: str | None, out: TextIO) -> int:
    r = client.call("undo", {} if hold_id is None else {"hold_id": hold_id})
    if r.get("undone"):
        print(f"undone: {r['undone']}", file=out)
        return 0
    pending = r.get("pending", [])
    if not pending:
        print("nothing pending to undo", file=out)
        return 1
    print("several sends are pending; name one:", file=out)
    for h in pending:
        print(f"  {h['hold_id']}  {h['kind']} via {h['capability']}", file=out)
    return 1


def autonomy(client: Client, level: str | None, out: TextIO) -> int:
    r = client.call("autonomy.get") if level is None else client.call("autonomy.set", {"level": level})
    print(f"autonomy: {r['level']}", file=out)
    return 0


def browsers(client: Client, cmd: str | None, profile: str | None, browser: str | None, out: TextIO) -> int:
    """`ferb browsers [pair|unpair <profile>]` (section 15)."""
    if cmd == "pair":
        print(f"Opening the FERB pairing page in {profile}… (the extension must be loaded there)", file=out)
        r = client.call("browsers.pair", {"profile": profile, "browser": browser})
        if r["paired"]:
            print(f"Paired {r['browser']} profile “{r['name']}” ({r['folder']}).", file=out)
            return 0
        print(f"Not paired: {r.get('hint', 'no answer from the extension')}", file=out)
        return 1
    if cmd == "unpair":
        r = client.call("browsers.unpair", {"profile": profile, "browser": browser})
        print("Unpaired." if r["unpaired"] else "That profile wasn't paired.", file=out)
        return 0
    r = client.call("browsers.list")
    for p in r["profiles"]:
        if p["ferb_identity"]:
            state = "FERB's identity profile (build step 6b)"
        elif not p["paired"]:
            state = "not paired"
        elif p["refused"]:
            state = f"refused: {p['refused']}"
        else:
            state = f"paired, {'connected' if p['connected'] else 'not connected'}" + \
                    (f", extension {p['version']}" if p["version"] else "")
        email = f" <{p['email']}>" if p["email"] else ""
        print(f"{p['browser']:<7} {p['name']}{email} ({p['folder']}): {state}", file=out)
    return 0
