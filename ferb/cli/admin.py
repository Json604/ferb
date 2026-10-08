"""Render daemon health, settings, task traces and administrative commands. Must not infer task success from display formatting.

Includes `ferb trace <task-id>` and `ferb daemon run`."""

import json
import re
import subprocess
import time
from datetime import datetime
from typing import Any, Callable, TextIO

from ferb.cli.client import Client
from ferb.trace import render


ANSI = re.compile(r"\x1b\[[0-9;?<>=]*[A-Za-z]|\x1b\][^\x07]*\x07|\x1b[()][A-Z0-9]|\x1b[78=>]|[\x00-\x08\x0e-\x1f]")
ARG_KEYS = ("url", "path", "command", "text", "query", "app", "bundle_id", "action", "allow_foreground", "element", "direction", "amount", "distance", "x", "y", "profile", "tab", "id")


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


def trace(client: Client, task_id: str | None, out: TextIO, full: bool = False, raw: bool = False,
          steps: bool = False) -> int:
    """`ferb trace`: the story (default), --steps (one line per tool call), --full / --json (raw, for debugging)."""
    result = client.call("trace.get", {"task_id": task_id or "last"})
    if raw:
        print(json.dumps(result, ensure_ascii=False, indent=2), file=out)
        return 0
    if full:
        for line in render_trace(result, True):
            print(line, file=out)
        return 0
    if steps and result.get("spans"):
        print("\n".join(render.steps(result)) or "(no tool calls)", file=out)
        return 0
    if steps:
        print("This task's steps aren't kept any more (traces age out after 30 days); its story:", file=out)
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
        print(f"{browser}: {count} live extension connection(s) have no pairing. A reinstall can leave an old "
              "profile pairing behind. Run `ferb browsers pair <profile> --browser " + browser +
              "` in the intended profile; its identity cannot be inferred from this connection.", file=out)
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


# ---------- Jev ----------

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
            flag = f"{st['flag_rate']:.0%}" if st["flag_rate"] is not None else "-"
            head += f", flagged {flag}"
            if st["acted"]:
                head += f", acted {st['acted']}x"
            print(head, file=out)
            for ex in st.get("examples", []):
                print(f"      e.g. {ex}", file=out)
        if r.get("dropped"):
            print(f"({r['dropped']} shadow calls dropped while Jev was behind)", file=out)
        if r.get('retired_enabled'):
            print('Retired configured questions (inactive): ' + ', '.join(r['retired_enabled']), file=out)
        return 0
    on = args.dcmd == "enable"
    if on:
        st = next((x for x in client.call("jev.stats")["stats"] if x["question"] == args.question), None)
        if st is not None:
            print(f"{args.question}: Jev flagged "
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


# ---------- reminders, device ----------

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
    rows = client.call("schedules.list")["schedules"]
    for r in rows:
        nxt = datetime.fromisoformat(r["next_run"]).astimezone().strftime("%a %d %b %H:%M") if r["next_run"] else "-"
        extra = f", failed {r['failures']}x in a row" if r["failures"] else ""
        print(f"  {r['id']}  {r['state']:<7} {r['rule']:<28} next {nxt}  {r['request']}{extra}", file=out)
    if not rows:
        print("  (no schedules)", file=out)
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
        # V4: done (every condition verified), done but unverified (a check ran and couldn't confirm) and failed are
        # counted apart; done % counts only done.
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
            "avg_trace_bytes": int(avg_bytes or 0)}
    target = root / "baselines" / f"{captured}.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps(data, indent=2) + "\n")
    print(str(target), file=out)
    return 0


def stats(home: Any, out: TextIO, days: int = 7) -> int:
    """How FERB is really doing (audit 2026-10-02): task outcomes, the top tool-error reasons, model runs. Read
    straight from the database, read-only, so it works whether or not the daemon runs."""
    import sqlite3
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
        # V4: done means every condition verified; unverified means a check ran and couldn't confirm; failed, not done.
        shares = ", ".join(f"{label} {count.get(st, 0)} ({100 * count.get(st, 0) // total}%)"
                           for label, st in (("done", "done"), ("unverified", "done_unverified"), ("failed", "failed")))
        print(f"Requests in the last {days} day(s): {total}" + (f" — {shares}" if total else ""), file=out)
        for status, n in outcomes:
            print(f"  {status:<16}{n}", file=out)
        calls, errors = c.execute("SELECT COUNT(*), SUM(error IS NOT NULL) FROM spans WHERE kind = 'tool.call' "
                                  "AND started_at >= ?", (since,)).fetchone()
        print(f"Tool calls: {calls or 0}, errored: {errors or 0}", file=out)
        for reason, n in c.execute("SELECT substr(error, 1, 90), COUNT(*) FROM spans WHERE kind = 'tool.call' AND "
                                   "error IS NOT NULL AND started_at >= ? GROUP BY 1 ORDER BY 2 DESC LIMIT 10",
                                   (since,)):
            print(f"  {n:>4}  {reason}", file=out)
        runs = c.execute("SELECT kind, COUNT(*), ROUND(AVG((julianday(ended_at) - julianday(started_at)) * 86400), 1) "
                         "FROM spans WHERE kind IN ('model.turn', 'worker') AND started_at >= ? GROUP BY kind",
                         (since,)).fetchall()
        for kind, n, avg in runs:
            print(f"Model runs ({kind}): {n}, avg {avg or 0} s", file=out)
    finally:
        c.close()
    return 0


def regress(client: Client, connect: Callable[[], Client], set_path: Any, home: Any, out: TextIO) -> int:
    """Run a regression set; print the `ferb verdict` line per task. Never decides pass or fail."""
    from datetime import datetime

    from ferb.eval.regress import load_set, run_set
    tasks = load_set(set_path)
    rows = run_set(client, tasks, out, connect=connect)
    expect = {t.id: t.expect for t in tasks}
    print("\nYour verdicts (acceptance is your word):", file=out)
    for r in rows:
        r["verdict_possible"] = bool(r["task_id"])
        if r["task_id"]:
            print(f"  ferb verdict {r['task_id']} pass|fail   # {r['id']}: {expect.get(r['id'], '')}", file=out)
        elif r["status"] in ("reply", "clarify"):
            print(f"  {r['id']}: answered in chat — no task to give a verdict on ({r['status']}): "
                  f"{r['outcome_summary']}   # {expect.get(r['id'], '')}", file=out)
        else:
            print(f"  {r['id']}: no task started ({r['status']}): {r['outcome_summary']}   # {expect.get(r['id'], '')}", file=out)
    dest = home / "regression"
    dest.mkdir(parents=True, exist_ok=True)
    path = dest / f"{datetime.now().strftime('%Y%m%dT%H%M%S')}.json"
    path.write_text(json.dumps(rows, indent=2))
    print(f"Results: {path}", file=out)
    return 0
