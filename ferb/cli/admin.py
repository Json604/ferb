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
        args = json.dumps(node["input"], separators=(",", ":"), ensure_ascii=False)
        if len(args) > 60:
            args = args[:57] + "..."
        lines.append(f"{prefix}{'└─' if last else '├─'} {node['kind']} [{node['id']}] {dur}{status} {args}")
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


def pause(client: Client, out: TextIO) -> int:
    client.call("sense.pause")
    print("FERB paused: every watcher is stopped and nothing is recorded; FERB won't use the real mouse / keyboard "
          "until `ferb resume`.", file=out)
    return 0


def sense_resume(client: Client, out: TextIO) -> int:
    r = client.call("sense.resume")
    print(f"FERB is watching again ({', '.join(r['watchers']) or 'no watchers running'}).", file=out)
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


def ps(client: Client, out: TextIO) -> int:
    """`ferb ps`: FERB's processes (section 15)."""
    procs = client.call("ps.list")["processes"]
    if not procs:
        print("No processes.", file=out)
    for p in procs:
        state = "running" if p["running"] else f"exited {p['exit_code']}"
        bg = ", background" if p["long_lived"] else ""
        print(f"{p['id']}  {state}{bg}  task {p['owner_task']}  {p['command'][:80]}", file=out)
    return 0


def kill(client: Client, proc_id: str, out: TextIO) -> int:
    ok = client.call("ps.kill", {"id": proc_id})["killed"]
    print("Stopped." if ok else f"No process {proc_id}.", file=out)
    return 0 if ok else 1


# ---------- memory (build step 11) ----------

ORIGIN = {"owner_stated": "you said", "observed": "observed", "inferred": "inferred", "third_party": "third-party"}


def _item_line(i: dict[str, Any]) -> str:
    extra = ""
    if i["status"] == "proposed" and i["origin"] == "observed":
        extra = f" — seen on {i['evidence_days']} of 3 days"
    elif i["status"] == "proposed":
        extra = f" — `ferb memory confirm {i['id']}` to use it"
    until = f" (until {i['valid_to'][:10]})" if i.get("valid_to") else ""
    return (f"  {i['id']}  [{i['kind']} · {i['subject']} · {i['key']}] {i['content']}  "
            f"({ORIGIN[i['origin']]}, {i['status']}{until}){extra}")


def memory(client: Client, args: Any, ask: Any, out: TextIO) -> int:
    c = args.mcmd
    if c == "list":
        status = "all" if args.all else "proposed" if args.proposed else "active"
        items = client.call("memory.list", {"status": status})["items"]
        print("\n".join(_item_line(i) for i in items) or "  (nothing)", file=out)
    elif c == "show":
        r = client.call("memory.show", {"subject": args.subject})
        print(f"{r['subject']}:", file=out)
        for i in r["items"]:
            print(_item_line(i), file=out)
            print(f"      sources: {', '.join(i['sources']) or '-'}", file=out)
    elif c == "add":
        i = client.call("memory.add", {"content": args.content, "kind": args.kind, "subject": args.subject,
                                       "key": args.key})
        print("Remembered:\n" + _item_line(i), file=out)
    elif c == "confirm":
        print("Confirmed:\n" + _item_line(client.call("memory.confirm", {"id": args.id})), file=out)
    elif c == "edit":
        print("Updated (the old version stays as history):\n" +
              _item_line(client.call("memory.edit", {"id": args.id, "content": args.content})), file=out)
    elif c == "retract":
        client.call("memory.retract", {"id": args.id})
        print(f"Retracted {args.id}: kept as history, never used.", file=out)
    elif c == "forget":
        p = {"id": args.id, "keep_sources": args.keep_sources}
        plan = client.call("memory.forget", p)["will_delete"]
        print("This permanently deletes:\n" + "\n".join(f"  - {x}" for x in plan), file=out)
        if not args.yes and ask("Forget it? [y/N] ").strip().lower() not in ("y", "yes"):
            print("Nothing deleted.", file=out)
            return 1
        client.call("memory.forget", p | {"confirm": True})
        print("Forgotten.", file=out)
    return 0


def timeline(client: Client, since: str | None, out: TextIO) -> int:
    for e in client.call("timeline.get", {"since": since} if since else {})["entries"]:
        d = e["data"]
        if e["kind"] == "trail.day_summary":
            text = (f"day {d.get('day')}: {d.get('events', 0)} events; apps "
                    + ", ".join(f"{k} {v}m" for k, v in list(d.get("apps_min", {}).items())[:5]))
        elif e["kind"] == "trail.day_narrative":
            text = f"day {d.get('day')}: {d.get('text', '')}"
        elif e["kind"] in ("chat.message", "chat.reply"):
            text = str(d.get("text", ""))[:160]
        elif e["kind"] == "task.outcome":
            text = f"{d.get('status')}: {str(d.get('summary', ''))[:160]}"
        else:
            text = ", ".join(f"{k}={v}" for k, v in d.items())[:160]
        print(f"{e['at'][:16].replace('T', ' ')}  {e['kind']:<22} {text}", file=out)
    return 0
