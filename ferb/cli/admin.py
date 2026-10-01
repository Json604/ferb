"""Admin commands. Build step 1: `ferb trace <task-id>` and `ferb daemon run`."""

import json
import subprocess
import time
from datetime import datetime
from typing import Any, Callable, TextIO

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


INSTALL_WAIT_S = 300


def _guide_install(client: Client, r: dict[str, Any], profile: str | None, browser: str | None, out: TextIO,
                   sleep: Callable[[float], None]) -> bool:
    """FERB's extension isn't in that profile and Chrome only allows installing it by hand ("Load unpacked"; the
    command-line way was removed in Chrome 137). Its extensions page is open there; the folder path is on the
    clipboard; FERB waits until Chrome has it, then pairs."""
    path = r["extension_dir"]
    try:
        subprocess.run(["pbcopy"], input=path.encode(), check=False)
        copied = " (copied to your clipboard)"
    except OSError:
        copied = ""
    print(f"FERB's extension isn't installed in {r['browser'].capitalize()} “{r['name']}” yet. Its extensions page "
          "is open there:", file=out)
    print("  (If it didn't open: in that profile's window, ⋮ menu → Extensions → Manage Extensions.)", file=out)
    print("  1. Turn on Developer mode (top right).", file=out)
    print("  2. Click “Load unpacked”.", file=out)
    print(f"  3. Press ⌘⇧G, paste the folder{copied}, press Return, then click Select:", file=out)
    print(f"     {path}", file=out)
    print("Waiting for it…", file=out, flush=True)
    for _ in range(int(INSTALL_WAIT_S / 2)):
        if client.call("browsers.installed", {"profile": profile, "browser": browser}).get("installed"):
            return True
        sleep(2)
    print("Didn't see it installed after 5 minutes. Run this again when it's loaded.", file=out)
    return False


def browsers(client: Client, cmd: str | None, profile: str | None, browser: str | None, out: TextIO,
             sleep: Callable[[float], None] = time.sleep) -> int:
    """`ferb browsers [pair|unpair <profile>]` (section 15)."""
    if cmd == "pair":
        print(f"Opening the FERB pairing page in {profile}…", file=out)
        r = client.call("browsers.pair", {"profile": profile, "browser": browser})
        if r.get("needs_install"):
            if not _guide_install(client, r, profile, browser, out, sleep):
                return 1
            print("Installed. Now pairing…", file=out)
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
        if p["ferb_identity"]:                            # FERB opens its own profile when a task needs it
            if not p["paired"]:
                state = "FERB's own profile, not paired yet (`ferb setup --identity`)"
            elif p["connected"]:
                state = "FERB's own profile, connected" + (f", extension {p['version']}" if p["version"] else "")
            else:
                state = "FERB's own profile, paired; not open right now (FERB opens it when a task needs it)"
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


# ---------- Jev (build step 13) ----------

def decide(client: Client, args: Any, ask: Any, out: TextIO) -> int:
    if args.dcmd == "stats":
        r = client.call("jev.stats")
        if not r["configured"]:
            print("Jev isn't set up (`ferb setup --jev`).", file=out)
            return 0
        for st in r["stats"]:
            if st["answered"] == 0 and st["asked"] == 0:
                continue
            head = f"{st['question']:<28} {'ENABLED' if st['enabled'] else 'shadow '}  answered {st['answered']}/{st['asked']}"
            if st["real_baseline"]:
                agree = f"{st['agreement']:.0%}" if st["agreement"] is not None else "-"
                head += f", agreement {agree} (enable at 200+ and 95%+)"
            else:
                flag = f"{st['flag_rate']:.0%}" if st["flag_rate"] is not None else "-"
                head += f", flagged {flag} (no existing logic: your call)"
            if st["acted"]:
                head += f", acted {st['acted']}x"
            print(head, file=out)
            for ex in st.get("examples", []):
                print(f"      e.g. {ex}", file=out)
        if r.get("dropped"):
            print(f"({r['dropped']} shadow calls dropped while Jev was behind)", file=out)
        return 0
    on = args.dcmd == "enable"
    if on:
        st = next((x for x in client.call("jev.stats")["stats"] if x["question"] == args.question), None)
        if st is not None and not st["real_baseline"]:
            print(f"{args.question}: FERB has no logic of its own here. Jev flagged "
                  f"{(st['flag_rate'] or 0):.0%} of {st['answered']} answers. Effect when enabled: {st['effect']}.",
                  file=out)
            for ex in st.get("examples", []):
                print(f"  e.g. {ex}", file=out)
            if ask("Enable it? [y/N] ").strip().lower() not in ("y", "yes"):
                print("Left in shadow mode.", file=out)
                return 1
    r = client.call("jev.enable", {"question": args.question, "on": on})
    print(f"Enabled questions: {', '.join(r['enabled']) or 'none (all in shadow mode)'}", file=out)
    return 0


# ---------- reminders, device (build step 14) ----------

def reminders(client: Client, action: str, rid: str | None, out: TextIO) -> int:
    if action == "cancel":
        if not rid:
            print("usage: ferb reminders cancel <id>", file=out)
            return 2
        client.call("reminders.cancel", {"id": rid})
        print(f"Cancelled {rid}.", file=out)
        return 0
    from datetime import datetime
    pending = client.call("reminders.list")["pending"]
    for t in pending:
        at = datetime.fromisoformat(t["at"]).astimezone()
        print(f"  {t['id']}  {at:%a %d %b %H:%M}  {t['kind']:<8} {t['message']}", file=out)
    if not pending:
        print("  (no pending reminders)", file=out)
    return 0


def device(home: Any, action: str, connect: Any, out: TextIO) -> int:
    """The device profile (read locally), with live browser status from the daemon when it runs."""
    from ferb.config import Config
    from ferb.device.profile import ProfileStore, diff
    from ferb.device.scan import scan
    did = Config(home).get("device", "id")
    if not did:
        print("No device profile yet: run `ferb setup`.", file=out)
        return 1
    store = ProfileStore(home)
    if action == "scan":
        changed = store.save(scan(str(did)), "ferb device scan")
        print("Rescanned: " + ("the profile changed (committed)." if changed else "no changes."), file=out)
    p = store.load(str(did))
    if p is None:
        print("No device profile yet: run `ferb setup`.", file=out)
        return 1
    print(f"{p.os} {p.os_version} · scanned {p.scanned_at} · {len(p.apps)} apps, "
          f"{sum(a.scriptable for a in p.apps)} scriptable", file=out)
    d = diff(p, scan(p.device_id)) if action != "scan" else None
    if d is not None and not d.empty:
        print(f"  since then: {len(d.added)} added, {len(d.removed)} removed, {len(d.changed)} changed "
              "(`ferb device scan`)", file=out)
    live: dict[str, list[str]] = {}
    try:
        c = connect()
        for pr in c.call("browsers.list")["profiles"]:
            if pr["paired"]:
                state = "connected" if pr["connected"] else "paired, not connected"
                who = "FERB's identity" if pr.get("ferb_identity") else "owner"
                live.setdefault(pr["browser"], []).append(f"{pr['name']} ({who}, {state})")
        c.close()
    except Exception:
        pass
    print("Browsers:", file=out)
    for b in p.browsers:
        print(f"  {b.name}: " + "; ".join(f"{k} — {v}" for k, v in b.drivers.items()), file=out)
        for line in live.get(b.name, []):
            print(f"      {line}", file=out)
    return 0


# ---------- schedules (build step 15) ----------

def schedules(client: Client, action: str, sid: str | None, out: TextIO) -> int:
    if action != "list":
        if not sid:
            print(f"usage: ferb schedules {action} <id>", file=out)
            return 2
        print(client.call("schedules.change", {"id": sid, "action": action})["text"], file=out)
        return 0
    from datetime import datetime
    rows = client.call("schedules.list")["schedules"]
    for r in rows:
        nxt = datetime.fromisoformat(r["next_run"]).astimezone().strftime("%a %d %b %H:%M") if r["next_run"] else "-"
        extra = f", failed {r['failures']}x in a row" if r["failures"] else ""
        print(f"  {r['id']}  {r['state']:<7} {r['rule']:<28} next {nxt}  {r['request']}{extra}", file=out)
    if not rows:
        print("  (no schedules)", file=out)
    return 0
