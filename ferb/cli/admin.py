"""Render daemon health, settings, task traces and administrative commands. Must not infer task success from display formatting.

Includes `ferb trace <task-id>` and `ferb daemon run`."""

import json
import re
import subprocess
import time
from datetime import datetime, timedelta
from typing import Any, Callable, TextIO

from ferb.cli.client import Client
from ferb.trace import render


ANSI = re.compile(r"\x1b\[[0-9;?<>=]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Z0-9]|\x1b[78=>]|[\x00-\x08\x0e-\x1f]")
ARG_KEYS = ("url", "path", "command", "text", "query", "app", "bundle_id", "action", "allow_foreground", "element", "direction", "amount", "distance", "x", "y", "profile", "tab", "id")


def status_label(status: Any) -> str:
    """A task status as the owner reads it: done_unverified is "not confirmed" (V5); the others as stored."""
    return "not confirmed" if status == "done_unverified" else str(status)


def _clip(s: str, n: int, full: bool) -> str:
    s = " ".join(str(s).split())
    return s if full or len(s) <= n else s[: n - 1] + "…"


def _plain(text: str) -> str:
    """Terminal output as text: screen-control codes removed, blank and repeated lines dropped (Claude Code etc.)."""
    lines, seen = [], set()
    for line in ANSI.sub("", text).replace("\r", "\n").splitlines():
        t = line.strip()
        if len(t) > 2 and not set(t) <= set("─│╭╮╰╯·✻✽✶✢✳ ") and t not in seen:
            seen.add(t)
            lines.append(t)
    return "\n".join(lines)


def _span_text(node: dict[str, Any], full: bool) -> tuple[str, list[str]]:
    """(the span's one-line label, the lines under it: what came back)."""
    inp, out = node.get("input") or {}, node.get("output") or {}
    if full:
        label = f"{node['kind']}  {inp.get('tool') or inp.get('goal') or ''}".rstrip()
        details = {key: value for key, value in node.items() if key != 'children'}
        return label, json.dumps(details, ensure_ascii=False, indent=2).splitlines()
    if node["kind"].startswith("disturbance."):
        if node["kind"] == "disturbance.change":
            actions = ", ".join(f"{a['tool']} ({a['span_id']})" for a in out.get("actions", [])) or "no overlapping action"
            return "desktop changed: " + ", ".join(out.get("changes", {})), [
                f"Actions near change: {actions}; cause and authorization not inferred.",
                "→ " + json.dumps(out.get("changes", {}), ensure_ascii=False)]
        return node["kind"], ["→ " + json.dumps(out, ensure_ascii=False)]
    if node["kind"] == "tool.call":
        args = inp.get("args") or {}
        shown = [f"{k}={_clip(args[k], 120, full)}" for k in ARG_KEYS if args.get(k) not in (None, "")]
        label = f"{inp.get('tool', '?')}  {'  '.join(shown)}"
        if args.get("action") == "scroll":
            label += "  " + {"up": "↑", "down": "↓", "left": "←", "right": "→"}.get(args.get("direction", "down"), "↕")
        if inp.get("why"):
            label += f"  (why: {_clip(inp['why'], 100, full)})"
        if node["error"]:
            return label, [f"✗ {_clip(node['error'], 300, full)}"]
        body = out.get("output") if isinstance(out, dict) else None
        if isinstance(body, str):
            text = _plain(body).splitlines()
            return label, ["→ " + ln for ln in (text if full else text[-6:])]
        keys = ("title", "url", "error", "detail", "opened", "tab", "matches", "met", "path")
        brief = [f"{k}={_clip(out[k], 100, full)}" for k in keys if isinstance(out, dict) and k in out]
        return label, (["→ " + "  ".join(brief)] if brief else [])
    goal = inp.get("goal") or inp.get("tool") or ""
    label = f"{node['kind']}  {_clip(goal, 140, full)}"
    lines: list[str] = []
    if node["error"]:
        lines.append(f"✗ {_clip(node['error'], 300, full)}")
    result = out.get("result") if isinstance(out, dict) else None
    if isinstance(result, str):
        try:
            r = json.loads(result)
            lines.append(f"→ {r.get('status', '')}: {_clip(r.get('summary', ''), 400, full)}".rstrip(": "))
        except ValueError:
            lines.append("→ " + _clip(result, 300, full))
    return label, lines


def render_trace(result: dict[str, Any], full: bool = False) -> list[str]:
    """`ferb trace`: a task and all its steps — each worker's goal and result, every tool call with what it opened,
    typed or ran and what came back (terminal output as plain text). `full`: nothing shortened."""
    if not result["spans"]:
        return [f"no spans for task {result['task_id']}"]
    lines = [f"task {result['task_id']}"]

    def walk(node: dict[str, Any], depth: int) -> None:
        start = datetime.fromisoformat(node["started_at"])
        dur = (f"{(datetime.fromisoformat(node['ended_at']) - start).total_seconds():.1f}s" if node["ended_at"]
               else "running")
        label, under = _span_text(node, full)
        pad = "  " * depth
        lines.append(f"{pad}{start.strftime('%H:%M:%S')}  {label}  [{dur}]")
        lines.extend(f"{pad}    {u}" for u in under)
        for child in node["children"]:
            walk(child, depth + 1)
    for root in result["spans"]:
        walk(root, 0)
    return lines


def verdict(client: Client, task_id: str, verdict_: str, note: str, out: TextIO) -> int:
    r = client.call("verdict.set", {"task_id": task_id, "verdict": verdict_, "note": note})
    print(f"Recorded {r['verdict']} for {r['task_id']}.", file=out)
    return 0


def trace(client: Client, task_id: str | None, out: TextIO, full: bool = False, raw: bool = False) -> int:
    """`ferb trace`: the story (default), --full / --json (raw, for debugging)."""
    result = client.call("trace.get", {"task_id": task_id or "last"})
    if raw:
        print(json.dumps(result, ensure_ascii=False, indent=2), file=out)
        return 0
    if full:
        for line in render_trace(result, True):
            print(line, file=out)
        return 0
    episode = result.get("episode")
    if episode is None:                                  # nothing composed: show what the raw trace has
        for line in render_trace(result, False):
            print(line, file=out)
        return 0
    print("\n".join(render.story(episode, (result.get("outcome") or {}).get("owner_verdict"))), file=out)
    return 0


def stop(client: Client, out: TextIO) -> int:
    r = client.call("stop.engage")
    print(f"FERB stopped: {r['tokens_revoked']} run token(s) revoked. "
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
    """`ferb browsers [pair|unpair <profile>]`."""
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
                    (f", extension {p['version']}" if p["connected"] and p["version"] else "")
        if not p['connected'] and (last := p.get('last_version') or p.get('version')):
            state += f", last connected version {last}"
        email = f" <{p['email']}>" if p["email"] else ""
        print(f"{p['browser']:<7} {p['name']}{email} ({p['folder']}): {state}", file=out)
    for browser, count in r.get('unpaired_connections', {}).items():
        print(f"{browser}: {count} live extension connection(s) have no pairing (a reinstalled extension gets a new "
              "identity). Pair it in FERB's Settings → Browsers, or run `ferb browsers pair <profile> --browser "
              + browser + "`.", file=out)
    return 0


def ps(client: Client, out: TextIO) -> int:
    """`ferb ps`: FERB's processes."""
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


# ---------- memory ----------

from ferb.memory.items import LABELS as ORIGIN


def _last_day(stamp: str) -> str:
    """The last day an item held, in the offset its end was stamped in: an end date's item ends at the next midnight
    (L36 §6), so the day before it; a version replaced at 15:00 held that day."""
    return (datetime.fromisoformat(stamp) - timedelta(microseconds=1)).date().isoformat()


def _item_line(i: dict[str, Any]) -> str:
    extra = ""
    if i["status"] == "proposed" and i["origin"] == "observed":
        extra = f" — seen on {i['evidence_days']} of 3 days"
    if i.get("in_profile"):
        extra += " — in the core profile"
    until = f" (until {_last_day(i['valid_to'])})" if i.get("valid_to") else ""
    return (f"  {i['id']}  [{i['kind']} · {i['subject']} · {i['key']}] {i['content']}  "
            f"({ORIGIN[i['origin']]}, {i['status']}{until}){extra}")


def memory(client: Client, args: Any, ask: Any, out: TextIO) -> int:
    c = args.mcmd
    if c in ("embeddings", "eval-export"):
        import json
        params = {"op": "embedding" if c == "embeddings" else "eval_export"}
        params.update({k: getattr(args, k) for k in ("action", "manifest", "dataset", "evaluation")
                       if getattr(args, k, None) is not None})
        client.set_timeout(None)
        print(json.dumps(client.call("memory.learning", params), ensure_ascii=False, indent=2), file=out)
        return 0
    if c == "profile":
        print(client.call("memory.profile", {})["markdown"], file=out)
        return 0
    if c in ("digest", "search", "rollback"):
        import json
        params = {"op": c}
        params.update({k: getattr(args, k) for k in ("query", "kind", "app", "person", "text", "key", "batch")
                       if getattr(args, k, None) is not None})
        print(json.dumps(client.call("memory.learning", params), ensure_ascii=False, indent=2), file=out)
    elif c in ("edit", "forget") and args.id.startswith("m-"):
        import json
        params = {"op": c, "id": args.id}
        if c == "edit":
            params["text"] = args.content
        if c == "forget":
            plan = client.call("memory.learning", params)
            print(json.dumps(plan, ensure_ascii=False, indent=2), file=out)
            if not args.yes and ask("Permanently forget these memories? [y/N] ").strip().lower() not in ("y", "yes"):
                return 1
            params["confirm"] = True
        print(json.dumps(client.call("memory.learning", params), ensure_ascii=False, indent=2), file=out)
    elif c == "list":
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
    elif c == "edit":
        print("Updated (the old version stays as history):\n" +
              _item_line(client.call("memory.edit", {"id": args.id, "content": args.content})), file=out)
    elif c == "retract":
        client.call("memory.retract", {"id": args.id})
        print(f"Retracted {args.id}: kept as history, never used.", file=out)
    elif c == "forget":
        p = {"id": args.id, "keep_sources": args.keep_sources}
        planned = client.call("memory.forget", p)
        print("This permanently deletes:\n" + "\n".join(f"  - {x}" for x in planned["will_delete"]) +
              (f"\n{planned['keeps']}" if planned.get("keeps") else ""), file=out)
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
            text = f"{status_label(d.get('status'))}: {str(d.get('summary', ''))[:160]}"
        elif e["kind"] == "config.upgraded":            # every change in full: the owner audits what FERB rewrote
            changes = "; ".join(f"{c.get('key')} {c.get('from')} → {'removed' if c.get('to') is None else c.get('to')}"
                                for c in d.get("changes") or [])
            text = f"{d.get('upgrade')}: {changes or 'settings already current'}"
        else:
            text = ", ".join(f"{k}={v}" for k, v in d.items())[:160]
        print(f"{e['at'][:16].replace('T', ' ')}  {e['kind']:<22} {text}", file=out)
    return 0


# ---------- Jev ----------

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


# ---------- schedules ----------

def schedules(client: Client, action: str, sid: str | None, out: TextIO) -> int:
    if action != "list":
        if not sid:
            print(f"usage: ferb schedules {action} <id>", file=out)
            return 2
        print(client.call("schedules.change", {"id": sid, "action": action})["text"], file=out)
        return 0
    from datetime import datetime
    from ferb.orchestrator.schedules import WATCH_SAID, until_said
    rows = client.call("schedules.list")["schedules"]
    for r in rows:
        nxt = datetime.fromisoformat(r["next_run"]).astimezone().strftime("%a %d %b %H:%M") if r["next_run"] else "-"
        watch = WATCH_SAID.get(r.get("watch") or "none", "")          # a watch says which kind it is
        ends = until_said(datetime.fromisoformat(r["until"])) if r.get("until") else ""   # decision 22
        extra = (f", {ends}" if ends else "") + (f" ({watch})" if watch else "") + \
            (f", failed {r['failures']}x in a row" if r["failures"] else "")
        print(f"  {r['id']}  {r['state']:<7} {r['rule']:<28} next {nxt}  {r['request']}{extra}", file=out)
    if not rows:
        print("  (no schedules)", file=out)
    return 0


# ---------- restore (S1) ----------

REASONS = {"overwrite": "overwrote", "edit": "edited", "trash": "moved to Trash",
           "trash-unverified": "moved to Trash", "trash copy": "Trash copy"}   # L39: the copy kept beside a trash


def _shown(path: str) -> str:
    """A file name as plain text: no terminal codes (a name is whatever its creator chose)."""
    return "".join(ch if ch.isprintable() else "?" for ch in ANSI.sub("", str(path)))


def restore(client: Client, rid: str | None, out: TextIO) -> int:
    """`ferb restore`: the files FERB changed or moved to the Trash lately; `ferb restore <id>` puts one back. Never
    over a change made since: then the earlier version goes beside the file (P27: the owner's names, short paths)."""
    if rid:
        r = client.call("recovery.restore", {"id": rid})
        was, now = _shown(r["original"]), _shown(r["restored_to"])
        if r["kind"] == "trash":
            text = (f"Put {was} back from the Trash." if r["in_place"] else
                    f"Something is at {was} now, so the file from the Trash was put back beside it as {now}.")
        else:
            text = (f"Restored {was} to what it was before FERB changed it." if r["in_place"] else
                    f"{was} has changed since FERB changed it, so it was left as it is. The version from before "
                    f"FERB's change is now at {now}.")
        print(text, file=out)
        return 0
    r = client.call("recovery.list")
    rows, days, total = r["records"], r["days"], r.get("total", len(r["records"]))
    if not rows:
        print("Nothing to put back: FERB hasn't changed or deleted any files lately.", file=out)
        return 0
    print(f"What FERB can put back, newest first (copies of changed files are kept {days} days; items it moved to "
          f"the Trash, while they are still there):", file=out)
    if total > len(rows):
        print(f"Showing the newest {len(rows)} of {total}.", file=out)
    for row in rows:
        at = datetime.fromisoformat(row["at"]).astimezone().strftime("%a %d %b %H:%M")
        shown_id = row["id"][:8] if len(row["id"]) > 12 else row["id"]
        if row["kind"] == "snapshot":
            print(f"  {shown_id}  {at}  {'disk snapshot':<14}  {_shown(row.get('saved_to') or '')}: a recovery point "
                  "of the whole disk you can open from macOS Recovery or Time Machine (admin rights)", file=out)
            continue
        what = REASONS.get(row["reason"] or "", row["reason"] or row["kind"])
        gone = "" if row["available"] else ("  (not found in the Trash)" if row["kind"] == "trash" else
                                            "  (its copy is gone)")
        if row.get("needs_owner"):
            gone = ("  (it changed while FERB moved it, so FERB won't put it back: look at it in the Trash and move "
                    "it yourself if it's yours)")
        print(f"  {shown_id}  {at}  {what:<14}  {_shown(row['original'])}{gone}", file=out)
    print("Put one back: ferb restore <id>", file=out)
    return 0


def baseline(home: Any, out: TextIO) -> int:
    """Capture the numbers every milestone is compared against into ~/.ferb/baselines/<ISO8601>.json. Read-only
    against the database, so it works whether or not the daemon runs."""
    import sqlite3
    from datetime import timezone
    from pathlib import Path as _P
    root = _P(str(home))
    db = root / "ferb.db"
    if not db.exists():
        print(f"no database at {db}", file=out)
        return 1

    def pct(num: int, den: int) -> int:
        return (100 * num // den) if den else 0

    c = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        # V4: done (every condition verified), done but not confirmed (done_unverified) and failed are counted apart;
        # done % counts only done. The JSON keys (`unverified`, `unverified_pct`) stay for data compatibility.
        requests, done, unverified, failed, verified = c.execute(
            "SELECT COUNT(*), COALESCE(SUM(status = 'done'), 0), COALESCE(SUM(status = 'done_unverified'), 0), "
            "COALESCE(SUM(status = 'failed'), 0), COALESCE(SUM(json_extract(outcome_json, '$.verified') = 1), 0) "
            "FROM tasks WHERE parent_id IS NULL").fetchone()
        owner_verdicts, owner_pass = c.execute(
            "SELECT COUNT(*), COALESCE(SUM(verdict = 'pass'), 0) FROM ("
            "SELECT v.verdict FROM owner_verdicts v JOIN tasks t ON t.id = v.task_id AND t.parent_id IS NULL "
            "WHERE v.seq = (SELECT MAX(seq) FROM owner_verdicts WHERE task_id = v.task_id))").fetchone()
        calls, errors = c.execute("SELECT COUNT(*), COALESCE(SUM(error IS NOT NULL), 0) FROM spans "
                                  "WHERE kind = 'tool.call'").fetchone()
        runs = {kind: {"n": n, "avg_s": avg or 0} for kind, n, avg in c.execute(
            "SELECT kind, COUNT(*), ROUND(AVG((julianday(ended_at) - julianday(started_at)) * 86400), 1) "
            "FROM spans WHERE kind IN ('model.turn', 'worker') GROUP BY kind")}
        cached, total_in = c.execute(
            "SELECT COALESCE(SUM(json_extract(output_json, '$.cached_input_tokens')), 0), "
            "COALESCE(SUM(json_extract(output_json, '$.input_tokens')), 0) "
            "FROM spans WHERE kind = 'model.usage'").fetchone()
        avg_bytes = c.execute("SELECT AVG(LENGTH(input_json) + LENGTH(COALESCE(output_json, ''))) "
                              "FROM spans").fetchone()[0]
        avg_task = c.execute(
            "SELECT AVG((julianday(updated_at) - julianday(created_at)) * 86400) FROM tasks WHERE parent_id IS NULL "
            "AND status IN ('done', 'done_unverified', 'failed', 'needs_user', 'cancelled', 'interrupted', 'unknown', "
            "'skipped')"
        ).fetchone()[0]
        questions = c.execute("SELECT COUNT(*) FROM journal WHERE kind IN ('approval.resolved', 'chat.clarify')"
                              ).fetchone()[0]
        latency = _latency(c)
    except sqlite3.OperationalError as e:
        if "no such table" not in str(e):
            raise
        print("ferb: this database is older than FERB 0.27 F0; start the daemon once to migrate it, then retry",
              file=out)
        return 1
    finally:
        c.close()
    captured = datetime.now(timezone.utc).isoformat(timespec="seconds")
    data = {"captured_at": captured, "days": "all", "requests": requests, "done": done,
            "done_pct": pct(done, requests), "unverified": unverified, "unverified_pct": pct(unverified, requests),
            "failed": failed, "failed_pct": pct(failed, requests),
            "verified": verified, "verified_pct": pct(verified, requests),
            "owner_verdicts": owner_verdicts, "owner_pass": owner_pass, "tool_calls": calls,
            "tool_errors": errors, "model_runs": runs, "avg_task_s": round(avg_task or 0, 1),
            "owner_questions": questions, "cache_pct": pct(cached, total_in),
            "avg_trace_bytes": int(avg_bytes or 0), "latency_s": latency}
    target = root / "baselines" / f"{captured}.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(data, indent=2) + "\n")
    print(str(target), file=out)
    return 0


# What each journal row is for a message's timeline (L8). An answer is the first reply, question or task answer.
TIMELINE = {"chat.ack": "ack", "chat.message": "message", "chat.plan": "plan", "chat.reply": "answer",
            "chat.clarify": "answer", "task.answer": "answer"}


def _latency(c: Any) -> dict[str, dict[str, Any]]:
    """L8: seconds from a message's arrival (its ack's `received_at`; its `chat.message` row when it has no ack) to
    its ack, its plan line and its first answer, and the context building from `chat.message` to its first planner
    turn. Only requests with a chat message count, so scheduled runs are left out."""
    import statistics
    seen: dict[str, dict[str, datetime]] = {}
    rows = c.execute("SELECT kind, at, json_extract(data_json, '$.request_id'), json_extract(data_json, "
                     "'$.received_at') FROM journal WHERE kind IN (" + ",".join("?" * len(TIMELINE)) + ") ORDER BY seq",
                     tuple(TIMELINE))
    for kind, at, request_id, received in rows:
        if request_id:
            times = seen.setdefault(str(request_id), {})
            times.setdefault(TIMELINE[kind], datetime.fromisoformat(at))
            if received:
                times.setdefault("received", datetime.fromisoformat(received))
    for request_id, started in c.execute(
            "SELECT json_extract(input_json, '$.request_id'), MIN(started_at) FROM spans WHERE kind = 'model.turn' "
            "AND json_extract(input_json, '$.role') = 'supervisor' GROUP BY 1"):
        if request_id in seen:
            seen[request_id]["planner"] = datetime.fromisoformat(started)
    spans: dict[str, list[float]] = {"ack": [], "plan": [], "answer": [], "context": []}
    for times in seen.values():
        if "message" not in times:
            continue
        arrived = times.get("received", times["message"])
        for name in ("ack", "plan", "answer"):
            if name in times:
                spans[name].append((times[name] - arrived).total_seconds())
        if "planner" in times:
            spans["context"].append((times["planner"] - times["message"]).total_seconds())
    return {name: {"n": len(values), "avg_s": round(statistics.fmean(values), 3) if values else 0,
                   "median_s": round(statistics.median(values), 3) if values else 0} for name, values in spans.items()}


def stats(home: Any, out: TextIO, days: int = 7) -> int:
    """How FERB is really doing (audit 2026-10-02): task outcomes, the top tool-error reasons, model runs, and FERB's
    own resource use (L30). Read straight from the database, read-only, so it works whether or not the daemon runs."""
    import sqlite3
    import math
    from datetime import timedelta, timezone
    from pathlib import Path as _P
    db = _P(str(home)) / "ferb.db"
    if not db.exists():
        print(f"no database at {db}", file=out)
        return 1
    since = (datetime.now(timezone.utc) - timedelta(days=days)).isoformat()
    c = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        outcomes = c.execute("SELECT status, COUNT(*) FROM tasks WHERE parent_id IS NULL AND created_at >= ? "
                             "GROUP BY status ORDER BY 2 DESC", (since,)).fetchall()
        total = sum(n for _, n in outcomes)
        count = dict(outcomes)
        # V4: done means every condition verified; not confirmed (done_unverified) means done but it stands
        # unconfirmed; failed, not done. V5: the label never says "unverified".
        shares = ", ".join(f"{label} {count.get(st, 0)} ({100 * count.get(st, 0) // total}%)"
                           for label, st in (("done", "done"), ("not confirmed", "done_unverified"),
                                             ("failed", "failed"))
                           ) if total else ""
        print(f"Requests in the last {days} day(s): {total}" + (f" — {shares}" if total else ""), file=out)
        for status, n in outcomes:
            print(f"  {status_label(status):<16}{n}", file=out)
        calls, errors = c.execute("SELECT COUNT(*), SUM(error IS NOT NULL) FROM spans WHERE kind = 'tool.call' "
                                  "AND started_at >= ?", (since,)).fetchone()
        print(f"Tool calls: {calls or 0}, errored: {errors or 0}", file=out)
        for reason, n in c.execute("SELECT substr(error, 1, 90), COUNT(*) FROM spans WHERE kind = 'tool.call' AND "
                                   "error IS NOT NULL AND started_at >= ? GROUP BY 1 ORDER BY 2 DESC LIMIT 10",
                                   (since,)):
            print(f"  {n:>4}  {reason}", file=out)
        values = [float(r[0]) for r in c.execute(
            "SELECT json_extract(output_json, '$.sandbox_ms') FROM spans WHERE kind='tool.call' AND started_at >= ? "
            "AND json_extract(output_json, '$.sandbox_ms') IS NOT NULL", (since,))]
        if values:
            import statistics
            values.sort()
            print(f"Contained shell start to first output/exit: n={len(values)}, "
                  f"median={statistics.median(values):.1f} ms, p95={values[max(0, math.ceil(len(values)*.95)-1)]:.1f} ms "
                  "(includes command startup; not an isolated sandbox overhead)", file=out)
        prompts = c.execute("SELECT COUNT(*) FROM spans WHERE kind='tool.call' AND started_at >= ? "
                            "AND json_extract(input_json, '$.ferb_question') = 1", (since,)).fetchone()[0]
        print(f"Code confirmation prompts: {prompts}", file=out)
        checks = dict(c.execute("SELECT j.value, COUNT(*) FROM spans, json_each(spans.input_json, '$.point_checks') "
                                "AS j WHERE spans.kind = 'tool.call' AND spans.started_at >= ? GROUP BY j.value",
                                (since,)).fetchall())
        if checks:                                   # which check decided each checked click (D31 fix round 4)
            from ferb.trace.lines import point_checks
            print(point_checks(checks), file=out)
        tables = {row[0] for row in c.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        if 'confirmations' in tables:
            answers = dict(c.execute("SELECT granted, COUNT(*) FROM confirmations WHERE at >= ? GROUP BY granted", (since,)))
            print(f"Confirmation answers: Yes {answers.get(1, 0)}, No {answers.get(0, 0)}", file=out)
        latest = {}
        shell_calls = 0
        for raw_in, raw_out in c.execute("SELECT input_json, output_json FROM spans WHERE kind='tool.call' "
                                         "AND started_at >= ? ORDER BY rowid", (since,)):
            inp, result = json.loads(raw_in or '{}'), json.loads(raw_out or '{}')
            if str(inp.get('tool', '')).startswith('shell.'):
                shell_calls += 1
            if result.get('id') and isinstance(result.get('network'), dict):
                latest[result['id']] = result['network']
        destinations = {(d['host'], d['port']) for net in latest.values() for d in net.get('destinations', [])}
        refused = sum(r.get('count', 0) for net in latest.values() for r in net.get('refused', []))
        escapes = c.execute("SELECT COUNT(*) FROM journal WHERE kind='escape.used' AND at >= ?", (since,)).fetchone()[0]
        print(f"Containment: shell calls {shell_calls}, observed destinations {len(destinations)}, "
              f"refused connections {refused}, escapes used {escapes}", file=out)
        runs = c.execute("SELECT kind, COUNT(*), ROUND(AVG((julianday(ended_at) - julianday(started_at)) * 86400), 1) "
                         "FROM spans WHERE kind IN ('model.turn', 'worker') AND started_at >= ? GROUP BY kind",
                         (since,)).fetchall()
        for kind, n, avg in runs:
            print(f"Model runs ({kind}): {n}, avg {avg or 0} s", file=out)
        # each learning model call is one model.learning span (memory/calls.py): consolidation and distillation
        learned, failed = c.execute("SELECT COUNT(*), SUM(error IS NOT NULL) FROM spans WHERE kind = 'model.learning' "
                                    "AND started_at >= ?", (since,)).fetchone()
        print(f"Learning model calls: {learned or 0}, failed: {failed or 0}", file=out)
        # grouped with their numbers set aside (a count Codex measured differs per call); the latest wording shown
        groups: dict[tuple[str, str], list[Any]] = {}
        for role, error in c.execute("SELECT json_extract(input_json, '$.role'), error FROM spans WHERE "
                                     "kind = 'model.learning' AND error IS NOT NULL AND started_at >= ? "
                                     "ORDER BY started_at, rowid", (since,)):
            seen = groups.setdefault((str(role), re.sub(r"\d+", "#", str(error))), [0, ""])
            seen[0], seen[1] = seen[0] + 1, str(error)
        for (role, _), (n, latest) in sorted(groups.items(), key=lambda item: -item[1][0])[:10]:
            print(f"  {n:>4}  {role}: {latest[:160]}", file=out)
        latest = c.execute("SELECT json_extract(input_json, '$.call') FROM spans WHERE kind = 'model.learning' AND "
                           "error IS NOT NULL AND started_at >= ? ORDER BY started_at DESC, rowid DESC LIMIT 1",
                           (since,)).fetchone()
        if latest and latest[0]:
            print(f"  latest failed call: ferb trace {latest[0]}", file=out)
        _daemon_problems(c, since, out)
        _resources(c, days, "resource_samples" in tables, out)
    finally:
        c.close()
    return 0


def _daemon_problems(c: Any, since: str, out: TextIO) -> None:
    """Requests that failed inside FERB (the server's catch-all: the reply named only the exception's type) and those
    answered after `CLIENT_WAIT_S` (the notch had shown a failure by then; a CLI call may have chosen a longer wait), as
    the daemon journaled them (L36): failures grouped by method and type, with the latest message and innermost frames.
    A row with `repeats` stands for that many more of the same after the first (review Q-I2)."""
    from ferb.daemon.server import CLIENT_WAIT_S, INTERNAL_ERROR, SLOW_CALL

    def rows(kind: str) -> list[dict[str, Any]]:
        return [json.loads(r[0]) for r in c.execute("SELECT data_json FROM journal WHERE kind = ? AND at >= ? "
                                                    "ORDER BY seq", (kind, since))]

    def times(d: dict[str, Any]) -> int:                 # a window's row stands for its repeats, any other for one
        return int(d.get("repeats") or 1)
    failed = rows(INTERNAL_ERROR)
    print(f"Requests that failed inside FERB: {sum(map(times, failed))}", file=out)
    groups: dict[tuple[str, str], list[Any]] = {}
    for d in failed:
        seen = groups.setdefault((str(d.get("method")), str(d.get("error"))), [0, d])
        seen[0], seen[1] = seen[0] + times(d), d
    for (method, error), (n, d) in sorted(groups.items(), key=lambda item: -item[1][0])[:10]:
        print(f"  {n:>4}  {method}: {error}: {str(d.get('message', ''))[:160]}", file=out)
        for frame in list(d.get("frames") or [])[-3:]:
            print(f"          at {frame}", file=out)
    slow = rows(SLOW_CALL)
    print(f"Requests answered after {CLIENT_WAIT_S:g} s: {sum(map(times, slow))}", file=out)
    for d in slow[-10:]:
        more = f" ({d['repeats']} times in its window; this is the latest)" if d.get("repeats") else ""
        print(f"        {d.get('method')}: {d.get('seconds')} s{more}", file=out)


def shown_bytes(n: float) -> str:
    """A size as the owner reads it (decimal units, as Finder shows them)."""
    return f"{n / 1e9:.1f} GB" if n >= 1e9 else f"{n / 1e6:.1f} MB" if n >= 1e6 else f"{n / 1e3:.0f} KB"


def _share(cpu: float, seconds: float) -> str:
    """CPU seconds over a time, as a share of one core (Activity Monitor's % CPU)."""
    return f"{100 * cpu / seconds:.1f}%" if seconds > 0 else "—"


def _resources(c: Any, days: int, present: bool, out: TextIO) -> None:
    """L30: FERB's resource log for the last `days` local days, today included, one line per day, then the period's five
    windows with the most CPU (the daemon's, its descendants' and FERB.app's) with what FERB was doing in each. Whole
    local days, since each line is one (the sections above count back `days` × 24 h from now), as its header says.
    `present`: the store has the log's table (a store from before L30 has none until the next update)."""
    from datetime import timedelta
    days = max(1, days)
    period = f"the last {days} local day(s), today included"
    cutoff = (datetime.now().astimezone().date() - timedelta(days=days - 1)).isoformat()
    rows = [dict(zip(("at", "day", "window_s", "cpu_s", "footprint", "peak_footprint", "child_cpu_s", "app_cpu_s",
                      "db_bytes", "work_json"), r))
            for r in c.execute("SELECT at, day, window_s, cpu_s, footprint, peak_footprint, child_cpu_s, app_cpu_s, "
                               "db_bytes, work_json FROM resource_samples WHERE day >= ? ORDER BY day, id",
                               (cutoff,))] if present else []
    if not rows:
        any_row = present and c.execute("SELECT 1 FROM resource_samples LIMIT 1").fetchone()
        print(f"Resources: no samples in {period}." if any_row else
              "Resources: no samples yet (the daemon writes one every 5 minutes)." if present else
              "Resources: no samples yet; the resource log starts after the next `ferb setup --update`.", file=out)
        return
    print(f"Resources (FERB's own use over {period}; sampled every 5 minutes; CPU as a share of one core):", file=out)
    by_day: dict[str, list[dict[str, Any]]] = {}
    for row in rows:
        by_day.setdefault(row["day"], []).append(row)
    for day, samples in by_day.items():
        seconds = sum(r["window_s"] for r in samples)
        peak = max(samples, key=lambda r: r["cpu_s"])        # its busiest window: a short one never wins by rate
        memory = [r["footprint"] for r in samples if r["footprint"] is not None]
        peaks = [r["peak_footprint"] for r in samples if r["peak_footprint"] is not None]
        app = [r for r in samples if r["app_cpu_s"] is not None]
        print(f"  {day}: {seconds / 3600:.1f} h; daemon CPU avg {_share(sum(r['cpu_s'] for r in samples), seconds)}, "
              f"peak {_share(peak['cpu_s'], peak['window_s'])} at {peak['at'][11:16]}; "
              + (f"memory avg {shown_bytes(sum(memory) / len(memory))}, peak {shown_bytes(max(peaks))}; "
                 if memory and peaks else "memory not read; ")
              + f"other processes CPU avg {_share(sum(r['child_cpu_s'] for r in samples), seconds)}; "
              + (f"FERB.app CPU avg {_share(sum(r['app_cpu_s'] for r in app), sum(r['window_s'] for r in app))}; "
                 if app else "FERB.app not running; ")
              + f"database {shown_bytes(samples[-1]['db_bytes'] or 0)}", file=out)
    print("  Hottest windows (CPU of the daemon, its descendants and FERB.app):", file=out)
    for row in sorted(rows, key=lambda r: -(r["cpu_s"] + r["child_cpu_s"] + (r["app_cpu_s"] or 0)))[:5]:
        print(f"    {_window(row)}", file=out)


def _window(row: dict[str, Any]) -> str:
    """One resource sample: its time, CPU and what FERB was doing in it (counts only)."""
    def parts(counts: dict[str, int], top: int | None = None) -> str:
        shown = sorted(counts.items(), key=lambda kv: -kv[1])[:top]
        return f" ({', '.join(f'{name} {n}' for name, n in shown)})" if shown else ""
    work = json.loads(row["work_json"] or "{}")
    events, models = work.get("events", {}), work.get("models", {})
    perception = work.get("perception", {})
    batches, jev = perception.get("batches", {}), work.get("jev", {})
    total = row["cpu_s"] + row["child_cpu_s"] + (row["app_cpu_s"] or 0)
    split = f"daemon {row['cpu_s']:.1f}, other processes {row['child_cpu_s']:.1f}" + (
        f", FERB.app {row['app_cpu_s']:.1f}" if row["app_cpu_s"] is not None else "")
    top = ", ".join(f"{name} {cpu:.1f} s" for name, cpu in work.get("top", []))
    return (f"{row['day']} {row['at'][11:16]}, {row['window_s'] / 60:.1f} min: {total:.1f} s CPU ({split}); "
            f"events {sum(events.values())}{parts(events, 3)}; perception wakes {perception.get('wakes', 0)}, "
            f"batches {sum(batches.values())}{parts(batches)}; Jev asked {jev.get('asked', 0)}, not sampled "
            f"{jev.get('not_sampled', 0)}; model runs {sum(models.values())}{parts(models)}; "
            f"tool calls {work.get('tool_calls', 0)}; tasks running {work.get('tasks_running', 0)}"
            + (f"; top processes {top}" if top else ""))
