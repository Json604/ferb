"""Own the failure report the owner can send the developer (L35; relay owner decisions 2026-11-01): one plain-text file
per turn FERB recorded as ending badly, under ~/.ferb/reports, every part of it redacted. Must not judge why a task
failed or send anything the owner didn't preview: FERB writes the file, the owner reads it in FERB.app and presses Send,
and only then the daemon sends that same text to the developer's report relay (relay/), which saves it as one file in a
private repo and files one issue holding a short summary and a link to it. Raw stderr (logs/daemon.err.log, never
redacted at capture) never goes in."""

import asyncio
import base64
import json
import os
import platform
import re
import stat
import tempfile
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Callable, Iterator
from urllib.parse import urlencode, urlsplit

import ferb
from ferb.core.errors import FerbError
from ferb.core.interfaces import Clock, IdSource
from ferb.daemon.server import INTERNAL_ERROR, SLOW_CALL
from ferb.store.db import Database
from ferb.store.spans import SpanStore
from ferb.trace import lines

# Where Send sends a report (owner, 2026-11-01): the developer's relay (relay/worker.mjs), which saves the file in a
# private repo and files an issue linking it. Built in once it is deployed; config `[feedback] relay_url` overrides it.
# With neither, the report is still written and kept, and Send says sending isn't set up yet. The app never holds the
# address. It is the relay's own address (an address ending in /report, as the first relay took, works too).
RELAY_URL = ""
NOT_SET_UP = "Sending reports to the developer isn't set up yet"
MAX_REPORT_BYTES = 25_000_000        # one sent report file at most (owner, 2026-11-01; relay/worker.mjs MAX_REPORT)
UPLOAD_MAX_BYTES = -(-MAX_REPORT_BYTES // 3) * 4 + 4096   # its base64 in GitHub's JSON (relay/worker.mjs MAX_UPLOAD)
SUMMARY_CHARS = 30_000               # the issue's summary at most (relay/worker.mjs MAX_SUMMARY)
SEND_TIMEOUT_S = 60.0                # each connect, read or write step of a request to the relay
SEND_DEADLINE_S = 540.0              # one whole Send, inside the app's wait (FailureReport.swift sendWait, 600 s)
CONFLICT_TRIES = 3                   # the file again when another report's commit landed at the same moment
TITLE_CHARS = 200                    # the relay's limits on a title and on the version label (relay/worker.mjs)
VERSION_CHARS = 50
LOCAL_HOSTS = ("127.0.0.1", "localhost", "::1")   # the only hosts a plain-http relay address may name (tests)
KEEP_DAYS = 30                       # the span retention the L35 design names
REPORTABLE = ("failed", "interrupted")
TURN_FAILED = "turn failed"          # a turn whose request has a `chat.failed` row, whatever its task's row says
SECTIONS = ("Summary", "Request", "Steps", "FERB's recorded problems")
# FERB's recorded problems (review 1, I3 ruling): the daemon's own rows of failed and slow requests, redacted at
# capture and with frames, from the task's time and this many latest overall
PROBLEM_KINDS = (INTERNAL_ERROR, SLOW_CALL)
PROBLEMS_LAST = 20
SUBJECT_CHARS = 80
FOLDER = "reports"
NAME = re.compile(r"FERB-report-\d{4}-\d{2}-\d{2}-[A-Za-z0-9._-]+\.txt")
SAFE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
SPAN_CONTENT = ("input", "output", "error", "summary")   # a span's recorded content; the rest is FERB's own structure
SPAN_LABEL = re.compile(r" *\d{2}:\d{2}:\d{2}  ")        # where `render_trace` starts a span's lines
# Settings → Advanced → Export a diagnostic file (L34 Phase E): FERB's state for debugging, one text file per export
DIAGNOSTIC = re.compile(r"FERB-diagnostic-\d{4}-\d{2}-\d{2}-\d{6}\.txt")
DIAGNOSTIC_SECTIONS = ("Summary", "Health", "Recording", "Recent tasks", "FERB's recorded problems",
                       "Recent setup, pairing and watcher events")
# What went wrong on a first install, for Send diagnostics to developer (2026-10-27): a permission asked, a pairing
# page left unanswered, a watcher that keeps failing, a human check handed over, a scheduled run retried
SETUP_EVENTS = ("setup.%", "browsers.%", "browser.handoff%", "sense.watcher_failed", "schedule.retry_queued")
SETUP_EVENTS_LAST = 40
RECENT_TASKS = 20


@dataclass(frozen=True)
class Failure:
    """What failed, from FERB's own records."""
    task_id: str            # what the report is named by and whose trace it holds: the root task, else the request
    status: str             # the task's status, or TURN_FAILED
    reason: str             # the owner-facing words (L11): the outcome's summary, else what FERB answered
    request: str            # the owner's request words
    answer: str             # FERB's answer
    started: str | None
    ended: str | None
    task_status: str = ""   # a failed turn's task, when it had one: that task's recorded status


def failure(db: Database, task_id: str, request_id: str = "") -> Failure:
    """The rule the notch shares (review 1, I2): a report is built exactly when FERB recorded that the turn ended
    badly: its root task's row is failed or interrupted, or its request has a `chat.failed` row (a planning failure,
    the turn's error, a failure after its plan line). The notch sends both ids it holds; an earlier notch sends the task
    alone (its row names its request), and a scheduled run's result names its request as the task."""
    given = [i for i in (task_id, request_id) if i]
    unknown = FerbError(f"FERB has no failed task or request {(task_id or request_id)!r}")
    if not given or not all(SAFE_ID.fullmatch(i) for i in given):
        raise unknown
    row = db.conn.execute("SELECT * FROM tasks WHERE id = ?", (task_id,)).fetchone() if task_id else None
    if row is not None and row["parent_id"] is None and row["status"] in REPORTABLE:
        outcome = json.loads(row["outcome_json"] or "{}")
        told, _ = _told(db, task_id)
        answer = str(outcome.get("answer") or told or outcome.get("summary") or "")
        return Failure(task_id, str(row["status"]), str(outcome.get("summary") or answer),
                       _message(db, str(row["request_id"])) or str(row["goal"]), answer,
                       row["created_at"], row["updated_at"])
    request = request_id or (str(row["request_id"]) if row is not None else task_id)
    failed = db.conn.execute("SELECT at, data_json FROM journal WHERE kind = 'chat.failed' AND "
                             "json_extract(data_json, '$.request_id') = ? ORDER BY seq DESC LIMIT 1",
                             (request,)).fetchone()
    if failed is None:
        if row is not None and row["parent_id"] is not None:
            raise FerbError(f"task {task_id} is part of {row['tree_id']}; send the report for {row['tree_id']}")
        if row is not None:
            raise FerbError("Only a failed or interrupted task can be sent to the developer; this one is "
                            f"{str(row['status']).replace('_', ' ')}.")
        raise unknown
    reply = db.conn.execute("SELECT data_json FROM journal WHERE kind = 'chat.reply' AND "
                            "json_extract(data_json, '$.request_id') = ? ORDER BY seq DESC LIMIT 1",
                            (request,)).fetchone()
    told, goal = _told(db, request)                  # a scheduled run's: no chat message, no chat reply
    answer = (str(json.loads(reply[0]).get("text") or "") if reply else "") or told
    why = str(json.loads(failed["data_json"]).get("why") or "")    # FERB's own record when no answer was kept
    asked = db.conn.execute("SELECT at FROM journal WHERE kind IN ('chat.ack', 'chat.message') AND "
                            "json_extract(data_json, '$.request_id') = ? ORDER BY seq LIMIT 1", (request,)).fetchone()
    return Failure(str(row["tree_id"]) if row is not None else request, TURN_FAILED, answer or why,
                   _message(db, request) or goal or (str(row["goal"]) if row is not None else ""), answer,
                   asked[0] if asked else None, failed["at"], str(row["status"]) if row is not None else "")


def _message(db: Database, request_id: str) -> str:
    """The owner's message for this request, as the daemon recorded it (redacted at capture)."""
    row = db.conn.execute("SELECT data_json FROM journal WHERE kind = 'chat.message' AND "
                          "json_extract(data_json, '$.request_id') = ? ORDER BY seq LIMIT 1", (request_id,)).fetchone()
    return str(json.loads(row[0]).get("text") or "") if row else ""


def _told(db: Database, task_id: str) -> tuple[str, str]:
    """What FERB told the owner about this task's end and its goal (`task_results`: a restart's interruption, a
    scheduled run); empty when there is none."""
    row = db.conn.execute("SELECT answer, goal FROM task_results WHERE task_id = ?", (task_id,)).fetchone()
    return (str(row[0] or ""), str(row[1] or "")) if row else ("", "")


def problems(db: Database, started: str | None, ended: str | None) -> list[tuple[str, str, Any]]:
    """FERB's recorded problems (`daemon.internal_error`, `daemon.slow_call`) from the task's time, and the latest
    PROBLEMS_LAST overall, oldest first, as (at, kind, data)."""
    found: dict[int, Any] = {}
    if started and ended:
        for r in db.conn.execute("SELECT seq, at, kind, data_json FROM journal WHERE kind IN (?, ?) AND at >= ? AND "
                                 "at <= ?", (*PROBLEM_KINDS, started, ended)):
            found[r["seq"]] = r
    for r in db.conn.execute("SELECT seq, at, kind, data_json FROM journal WHERE kind IN (?, ?) ORDER BY seq DESC "
                             "LIMIT ?", (*PROBLEM_KINDS, PROBLEMS_LAST)):
        found[r["seq"]] = r
    return [(r["at"], r["kind"], json.loads(r["data_json"])) for _, r in sorted(found.items())]


def setup_events(db: Database) -> list[tuple[str, str, Any]]:
    """The latest SETUP_EVENTS_LAST setup, pairing and watcher journal entries, oldest first, as (at, kind, data)."""
    where = " OR ".join("kind LIKE ?" for _ in SETUP_EVENTS)
    rows = db.conn.execute(f"SELECT at, kind, data_json FROM journal WHERE {where} ORDER BY seq DESC LIMIT ?",
                           (*SETUP_EVENTS, SETUP_EVENTS_LAST)).fetchall()
    return [(r["at"], r["kind"], json.loads(r["data_json"])) for r in reversed(rows)]


def report(db_path: Path, task_id: str, request_id: str, facts: dict[str, Any], *, home: Path, now: datetime,
           clock: Clock, ids: IdSource, walk: Callable[[Any], Any], text: Callable[[str], str],
           app_version: str = "") -> tuple[Failure, dict[str, str]]:
    """The whole build, off the event loop on a connection of its own (review 1, M1): what failed, its trace and its
    problems are read here, then `build` writes the file."""
    db = Database(db_path)
    try:
        found = failure(db, task_id, request_id)
        spans = SpanStore(db, clock, ids, walk).tree_json(found.task_id)
        rows = problems(db, found.started, found.ended)
    finally:
        db.close()
    return found, build(found, spans, rows, facts, home=home, now=now, walk=walk, text=text, app_version=app_version)


def build(found: Failure, spans: list[dict[str, Any]], rows: list[tuple[str, str, Any]], facts: dict[str, Any], *,
          home: Path, now: datetime, walk: Callable[[Any], Any], text: Callable[[str], str],
          app_version: str = "") -> dict[str, str]:
    """Writes the report and returns {path, subject, summary}. Every part is redacted (rule 14). The trace and the
    problem rows go through the daemon's structural redaction (`walk`, which sees each value unescaped) before they are
    rendered; then each span's block and each section goes through its text pass (`text`). Both match sealed values,
    their escaped forms too. Small pieces keep the event loop's share of the interpreter (review 1, I1 and M1).
    Reports older than KEEP_DAYS go first."""
    from ferb.cli.admin import render_trace     # the renderer behind `ferb trace TASK --full`: no second formatter
    try:
        seconds: float | None = (datetime.fromisoformat(found.ended or "")
                                 - datetime.fromisoformat(found.started or "")).total_seconds()
    except (TypeError, ValueError):                # a time not recorded, or one without its offset
        seconds = None
    summary = [("FERB version", ferb.__version__), ("macOS version", platform.mac_ver()[0] or "unknown")]
    if app_version:
        summary.append(("Notch version", lines.clip(app_version, 60)))
    summary += [("Extension version", "; ".join(facts.get("extensions") or []) or "unknown"),
                ("AI runtime and model", "; ".join(facts.get("runtimes") or []) or "unknown"),
                ("Task", found.task_id), ("Status", found.status)]
    if found.task_status:
        summary.append(("Task status", found.task_status))
    summary += [("Failure reason", found.reason.strip() or "(none recorded)"),
                ("Started", found.started or "unknown"), ("Ended", found.ended or "unknown"),
                ("Duration", lines.duration(seconds) if seconds is not None else "unknown")]
    trace = {"task_id": found.task_id, "spans": [_redacted(node, walk) for node in spans]}
    steps = [text("\n".join(block)) for block in _blocks(render_trace(trace, True))]
    recorded = [text(f"{at}  {kind}  {json.dumps(walk(data), ensure_ascii=False)}") for at, kind, data in rows]
    parts = [text(f"FERB report for {found.task_id}"), "",
             "## Summary", text("\n".join(f"{key}: {value}" for key, value in summary)), "",
             "## Request", text("\n".join(["Your request:", found.request.strip() or "(not recorded)", "",
                                           "FERB's answer:", found.answer.strip() or "(none recorded)"])), "",
             "## Steps", *steps, "",
             "## FERB's recorded problems",
             f"FERB's own records of requests that failed inside it or answered slowly: those during this task, and "
             f"the latest {PROBLEMS_LAST}.", *(recorded or ["(none recorded)"]), ""]
    reason = text(found.reason)
    short = next((line.strip() for line in reason.splitlines() if line.strip()), found.status)
    folder = home / FOLDER
    purge(folder, now)
    path = write(folder, f"FERB-report-{now.astimezone():%Y-%m-%d}-{found.task_id}.txt", "\n".join(parts))
    told = text("\n".join([f"What happened: {lines.clip(short, 300)}",
                           f"Task: {found.task_id} ({found.status})",
                           f"FERB {ferb.__version__}, macOS {platform.mac_ver()[0] or 'unknown'}"]))
    return {"path": str(path), "subject": "FERB report: " + lines.clip(short, SUBJECT_CHARS), "summary": told}


def diagnostic(db_path: Path, health: dict[str, Any], recording: dict[str, Any], facts: dict[str, Any], *,
               home: Path, now: datetime, walk: Callable[[Any], Any], text: Callable[[str], str],
               app_version: str = "") -> dict[str, str]:
    """Settings → Advanced → Export a diagnostic file (L34 Phase E): versions, the daemon's own health check, what it
    records, its latest tasks by id and status, its recorded problems and its recent setup, pairing and watcher events,
    every part redacted as a failure report is (rule 14; never raw stderr). Nothing is sent here: Send diagnostics to
    developer previews it, then `reports.send` posts it as a failure report. Runs off the event loop on its own
    connection; diagnostic files older than KEEP_DAYS go first. Returns {path, summary, subject}."""
    db = Database(db_path)
    try:
        tasks = [dict(r) for r in db.conn.execute(
            "SELECT id, status, created_at FROM tasks WHERE parent_id IS NULL ORDER BY created_at DESC, rowid DESC "
            "LIMIT ?", (RECENT_TASKS,))]
        rows = problems(db, None, None)
        events = setup_events(db)
    finally:
        db.close()
    mac = platform.mac_ver()[0] or "unknown"
    summary = [("FERB version", ferb.__version__), ("macOS version", mac),
               ("App version", lines.clip(app_version, 60) if app_version else "unknown"),
               ("Extension version", "; ".join(facts.get("extensions") or []) or "unknown"),
               ("AI runtime and model", "; ".join(facts.get("runtimes") or []) or "unknown"),
               ("Written", now.astimezone().isoformat(timespec="seconds"))]

    def shown(value: Any) -> str:
        return text(json.dumps(walk(value), ensure_ascii=False, indent=2, sort_keys=True, default=str))
    parts = [text("FERB diagnostic file"), "",
             "## Summary", text("\n".join(f"{key}: {value}" for key, value in summary)), "",
             "## Health", shown(health), "",
             "## Recording", shown(recording), "",
             "## Recent tasks",
             *([text(f"{t['created_at']}  {t['id']}  {t['status']}") for t in tasks] or ["(none recorded)"]), "",
             "## FERB's recorded problems",
             f"FERB's own records of requests that failed inside it or answered slowly: the latest {PROBLEMS_LAST}.",
             *([text(f"{at}  {kind}  {json.dumps(walk(data), ensure_ascii=False)}") for at, kind, data in rows]
               or ["(none recorded)"]), "",
             "## Recent setup, pairing and watcher events",
             *([text(f"{at}  {kind}  {json.dumps(walk(data), ensure_ascii=False)}") for at, kind, data in events]
               or ["(none recorded)"]), ""]
    folder = home / FOLDER
    purge(folder, now, names=DIAGNOSTIC)
    path = write(folder, f"FERB-diagnostic-{now.astimezone():%Y-%m-%d-%H%M%S}.txt", "\n".join(parts))
    told = text(f"FERB {ferb.__version__}, macOS {mac}")
    return {"path": str(path), "summary": told, "subject": f"FERB diagnostics ({told})"}


def _redacted(node: dict[str, Any], walk: Callable[[Any], Any]) -> dict[str, Any]:
    """A span and its children with their recorded content through the structural redaction, as at capture."""
    own = {key: walk(value) if key in SPAN_CONTENT else value for key, value in node.items() if key != "children"}
    return own | {"children": [_redacted(child, walk) for child in node.get("children") or []]}


def _blocks(rendered: list[str]) -> Iterator[list[str]]:
    """`render_trace`'s lines, one block per span (its label line and the lines under it)."""
    block: list[str] = []
    for line in rendered:
        if block and SPAN_LABEL.match(line):
            yield block
            block = []
        block.append(line)
    if block:
        yield block


def purge(folder: Path, now: datetime, days: int = KEEP_DAYS, names: re.Pattern[str] = NAME) -> int:
    """Removes FERB's own files of one kind (`names`: failure reports, or diagnostic files) last written more than
    `days` ago; how many."""
    if not folder.is_dir():
        return 0
    cutoff = now.timestamp() - days * 86400
    gone = 0
    for p in folder.iterdir():
        if names.fullmatch(p.name) and not p.is_symlink() and p.is_file() and p.stat().st_mtime < cutoff:
            p.unlink(missing_ok=True)
            gone += 1
    return gone


def write(folder: Path, name: str, text: str) -> Path:
    """The folder owner-only (0700), the file 0600, written whole: a temporary file in the same folder, then renamed."""
    folder.mkdir(parents=True, exist_ok=True)
    os.chmod(folder, 0o700)
    fd, tmp = tempfile.mkstemp(prefix=".report.", suffix=".txt", dir=folder)      # created 0600
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            f.write(text)
            f.flush()
            os.fsync(f.fileno())
        os.chmod(tmp, 0o600)
        os.replace(tmp, folder / name)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
    return folder / name


class Unsent(FerbError):
    """Why a report wasn't sent; the file stays where it is and Send can be pressed again."""


def relay_url(configured: Any) -> str:
    """Where Send sends: config `[feedback] relay_url`, else RELAY_URL; the relay's own address, without a trailing
    `/report` (the first relay's single endpoint). An https address, or plain http to this Mac only; anything else is
    refused (code checks the destination, rule 3)."""
    url = str(configured or RELAY_URL or "").strip()
    if not url:
        raise Unsent(NOT_SET_UP)
    try:
        parts = urlsplit(url)
        host = parts.hostname or ""
    except ValueError:
        parts, host = None, ""
    if parts is None or not host or parts.username or parts.password or not (
            parts.scheme == "https" or (parts.scheme == "http" and host in LOCAL_HOSTS)):
        raise Unsent("The report relay address in config.toml ([feedback] relay_url) isn't a valid https address")
    path = parts.path.rstrip("/")
    path = path[:-len("/report")] if path.endswith("/report") else path
    return f"{parts.scheme}://{parts.netloc}{path}"


def saved(home: Path, path: str) -> tuple[Path, str]:
    """A report or diagnostic file FERB wrote, by the path `build`/`diagnostic` returned: only a regular file in
    ~/.ferb/reports with one of their names; (path, kind) with kind "failure" or "diagnostic"."""
    p = Path(path)
    folder = home / FOLDER
    kind = "failure" if NAME.fullmatch(p.name) else "diagnostic" if DIAGNOSTIC.fullmatch(p.name) else ""
    if not kind or p.parent != folder or not p.is_absolute():
        raise FerbError("FERB can send only a report or diagnostic file it wrote in ~/.ferb/reports")
    try:
        st = p.lstat()
    except FileNotFoundError:
        raise FerbError("That report is no longer saved; send it again from where it was offered") from None
    if not stat.S_ISREG(st.st_mode):
        raise FerbError("FERB can send only a report or diagnostic file it wrote in ~/.ferb/reports")
    return p, kind


# Reading a report's Steps (the blocks `build` writes from `render_trace(..., full=True)`): structural facts only, the
# span's kind and tool and whether its recorded details hold an error, never a judgment of what a step meant
STEPS = "\n## Steps\n"
PROBLEMS = "\n## FERB's recorded problems\n"
LABEL = re.compile(r" *(\d{2}:\d{2}:\d{2})  (\S+)(?:  (.*?))?  \[[^\]]*\]$")
HAS_ERROR = re.compile(r'"error": (?!null\b)')
FIRST_ERROR = re.compile(r'"error": ("(?:[^"\\]|\\.)*")')
QUESTION = re.compile(r'"question": ("(?:[^"\\]|\\.)*")')
ANSWER = re.compile(r'"answer": ("(?:[^"\\]|\\.)*")')
QUESTION_KINDS = ("model.question",)         # a worker's question to the orchestrator
QUESTION_TOOLS = ("owner.ask",)              # a question to the owner, FERB's own confirmations included
CUT = "[FERB cut {n} steps here to fit {mb} MB]"


def _steps(body: str) -> tuple[int, int, list[str]] | None:
    """Where the Steps section's blocks start and end in a report, and its blocks (the first is the `task` line)."""
    at = body.find(STEPS)
    if at < 0:
        return None
    start = at + len(STEPS)
    end = body.find("\n## ", start)
    end = len(body) if end < 0 else end
    while end > start and body[end - 1] == "\n":
        end -= 1
    return start, end, ["\n".join(b) for b in _blocks(body[start:end].split("\n"))]


def _label(block: str) -> tuple[str, str, str]:
    """(time, span kind, tool or goal) of a Steps block's first line."""
    m = LABEL.match(block.split("\n", 1)[0])
    return (m.group(1), m.group(2), m.group(3) or "") if m else ("", "", "")


def _is_question(block: str) -> bool:
    _, kind, name = _label(block)
    return kind in QUESTION_KINDS or (kind == "tool.call" and name in QUESTION_TOOLS)


def _string(pattern: re.Pattern[str], block: str) -> str:
    m = pattern.search(block)
    if not m:
        return ""
    try:
        return str(json.loads(m.group(1)))
    except ValueError:
        return m.group(1)


def shorten(body: str, limit: int = MAX_REPORT_BYTES, name: str = "") -> tuple[str, str]:
    """A report cut to at most `limit` bytes (owner, 2026-11-01), and the note at its top saying what was cut ("" when
    it fits whole). Kept: everything outside Steps (the summary, the request, FERB's recorded problems), every step
    with an error and every question, and as many of the last steps as fit; each run of cut steps leaves a line saying
    so. When even that is too big (or there are no Steps), the middle is cut and the beginning and the end kept."""
    size = len(body.encode())
    if size <= limit:
        return body, ""
    mb = limit // 1_000_000
    kept_as = f" The full report is saved on the sender's Mac as {name}." if name else ""
    found = _steps(body)
    if found is not None:
        start, end, blocks = found
        keep = [i == 0 or bool(HAS_ERROR.search(b)) or _is_question(b) for i, b in enumerate(blocks)]
        sizes = [len(b.encode()) + 1 for b in blocks]
        budget = limit - len(body[:start].encode()) - len(body[end:].encode()) - 4096 - 80 * (sum(keep) + 2)
        budget -= sum(n for n, k in zip(sizes, keep) if k)
        if budget >= 0:
            for i in range(len(blocks) - 1, 0, -1):          # the last steps, back to the first that doesn't fit
                if keep[i]:
                    continue
                if sizes[i] > budget:
                    break
                keep[i] = True
                budget -= sizes[i]
            out: list[str] = []
            run = cut = cut_bytes = 0
            for block, k, n in zip(blocks, keep, sizes):
                if k:
                    if run:
                        out.append(CUT.format(n=run, mb=mb))
                    out.append(block)
                    run = 0
                else:
                    run, cut, cut_bytes = run + 1, cut + 1, cut_bytes + n
            if run:
                out.append(CUT.format(n=run, mb=mb))
            note = (f"[Shortened to fit {mb} MB: {cut} of {len(blocks) - 1} steps ({cut_bytes / 1_000_000:.1f} MB) "
                    f"were cut from Steps. The summary, the request, every error and question, FERB's recorded "
                    f"problems and the last steps are kept.{kept_as}]")
            text = note + "\n\n" + body[:start] + "\n".join(out) + body[end:]
            if len(text.encode()) <= limit:
                return text, note
    data = body.encode()
    room = limit - 2048
    head = room // 5
    note = (f"[Shortened to fit {mb} MB: {(size - room) / 1_000_000:.1f} MB were cut from the middle; the beginning "
            f"(the summary) and the end are kept.{kept_as}]")
    cut_line = f"\n[FERB cut {(size - room) / 1_000_000:.1f} MB here to fit {mb} MB]\n"
    return (note + "\n\n" + data[:head].decode(errors="ignore") + cut_line
            + data[size - (room - head):].decode(errors="ignore")), note


def summarize(body: str, note: str = "", limit: int = SUMMARY_CHARS) -> str:
    """The issue's short summary, from the report's own redacted text: what was cut (when it was), the Summary section
    (what failed, the versions), each step's error and each question, and FERB's recorded problems, one line each,
    within `limit` characters ("… n more in the full report" for the rest)."""
    out: list[str] = [note, ""] if note else []
    at = body.find("\n## Summary\n")
    if at >= 0:
        start = at + len("\n## Summary\n")
        end = body.find("\n\n", start)
        out += body[start:end if end >= 0 else len(body)].split("\n")
    groups: list[tuple[str, list[str]]] = []
    found = _steps(body)
    if found is not None:
        blocks = found[2][1:]
        errors, questions = [], []
        for block in blocks:
            when, kind, name = _label(block)
            what = f"{when}  {kind}  {name}".rstrip()
            if HAS_ERROR.search(block):
                errors.append(f"- {what}: {lines.clip(_string(FIRST_ERROR, block) or '(error recorded)', 300)}")
            if _is_question(block):
                asked = lines.clip(_string(QUESTION, block) or "(question not recorded)", 300)
                answer = _string(ANSWER, block)
                questions.append(f"- {what}: {asked}" + (f" → {lines.clip(answer, 200)}" if answer else ""))
        groups += [(f"Errors ({len(errors)})", errors), (f"Questions ({len(questions)})", questions)]
    at = body.find(PROBLEMS)
    if at >= 0:
        start = at + len(PROBLEMS)
        end = body.find("\n## ", start)
        section = body[start:end if end >= 0 else len(body)]
        rows = [r for r in section.split("\n")[1:] if r.strip() and r != "(none recorded)"]
        groups.append((f"FERB's recorded problems ({len(rows)})", [f"- {lines.clip(r, 300)}" for r in rows]))
    size = sum(len(line) + 1 for line in out)
    for title, items in groups:
        out += ["", title]
        size += len(title) + 2
        for n, item in enumerate(items):
            if size + len(item) + 1 > limit - 200:
                out.append(f"… {len(items) - n} more in the full report")
                size += 40
                break
            out.append(item)
            size += len(item) + 1
    return "\n".join(out).strip()[:limit]


PIECE_CHARS = 256 * 1024


def _redacted_text(written: str, text: Callable[[str], str]) -> str:
    """A saved file through the text pass in pieces cut only where `build` started a new piece (a step's block or a
    section), so a multi-MB report never holds the interpreter in one long pass (review 1, M1) and no piece `build`
    redacted whole is split."""
    pieces: list[str] = []
    piece: list[str] = []
    size = 0
    for line in written.split("\n"):
        if size > PIECE_CHARS and (SPAN_LABEL.match(line) or line.startswith("## ")):
            pieces.append(text("\n".join(piece)))
            piece, size = [], 0
        piece.append(line)
        size += len(line) + 1
    pieces.append(text("\n".join(piece)))
    return "\n".join(pieces)


@dataclass(frozen=True)
class Outgoing:
    """One Send, ready: the file as GitHub's "create a file" JSON (the relay streams it on untouched) and the issue's
    fields but the relay's ticket."""
    kind: str
    report_id: str
    file: bytes
    issue: dict[str, Any]
    shortened: bool


def prepare(path: Path, kind: str, title: str, app_version: str, *, text: Callable[[str], str]) -> Outgoing:
    """What Send sends for one saved file, off the event loop. The file is what the owner previewed; it goes through
    the redaction once more (a value sealed since the build is hidden too, and the saved file is rewritten to match,
    so what is kept is what was sent). A report over MAX_REPORT_BYTES is sent shortened (`shorten`); the saved file
    stays whole. The summary comes from the same redacted text."""
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, encoding="utf-8") as f:
        written = f.read()
    body = _redacted_text(written, text)
    if body != written:
        write(path.parent, path.name, body)
    report_id = path.stem[:128]
    sent, note = shorten(body, MAX_REPORT_BYTES, path.name)
    default = "FERB report" if kind == "failure" else "FERB diagnostics"
    version = " ".join("".join(c for c in app_version if c.isprintable()).split()) or ferb.__version__
    summary = text(summarize(body, note))[:SUMMARY_CHARS] or default
    file = json.dumps({"message": f"FERB {kind} report {report_id}",
                       "content": base64.b64encode(sent.encode()).decode("ascii")}).encode()
    if len(file) > UPLOAD_MAX_BYTES:                # never: `shorten` keeps it within; the relay refuses it anyway
        raise Unsent(f"FERB couldn't fit this report into {MAX_REPORT_BYTES // 1_000_000} MB. It stays saved at {path}.")
    return Outgoing(kind, report_id, file, {"version": 2, "title": lines.clip(text(title), TITLE_CHARS) or default,
                                            "summary": summary, "app_version": version[:VERSION_CHARS]}, bool(note))


async def send(base: str, out: Outgoing, timeout: float = SEND_TIMEOUT_S, deadline: float = SEND_DEADLINE_S,
               pause: float = 1.0) -> str:
    """One Send to the relay, as other daemon-side calls reach their services (httpx, no FERB proxy: that one is for
    FERB's shell): the file (again when the relay says another report's commit landed at that moment, up to
    CONFLICT_TRIES), then the issue with the relay's ticket; the relay's report_id. Any failure is Unsent with a
    reason the owner can read."""
    import httpx
    headers = {"Content-Type": "application/json", "User-Agent": f"FERB/{ferb.__version__}"}

    async def call(client: httpx.AsyncClient, url: str, content: bytes) -> tuple[int, dict[str, Any]]:
        try:
            r = await client.post(url, content=content, headers=headers)
        except httpx.TimeoutException:
            raise Unsent("FERB couldn't send the report: the report service didn't answer in time") from None
        except httpx.HTTPError as e:
            raise Unsent(f"FERB couldn't reach the report service ({type(e).__name__})") from None
        try:
            answer = r.json()
        except ValueError:
            answer = None
        answer = answer if isinstance(answer, dict) else {}
        if r.status_code == 429:
            raise Unsent("Too many reports were sent from this network just now; try again in a few minutes")
        return r.status_code, answer

    def refused(status: int, answer: dict[str, Any]) -> Unsent:
        why = lines.clip(str(answer.get("error") or "no reason given"), 200)
        return Unsent(f"The report service didn't take the report (HTTP {status}: {why})")

    async def go() -> str:
        async with httpx.AsyncClient(timeout=timeout) as client:
            query = urlencode({"kind": out.kind, "report_id": out.report_id})
            for attempt in range(CONFLICT_TRIES):
                status, answer = await call(client, f"{base}/file?{query}", out.file)
                if not (status == 409 and answer.get("retry") is True and attempt + 1 < CONFLICT_TRIES):
                    break
                await asyncio.sleep(pause * (attempt + 1))
            ticket = answer.get("ticket")
            if status != 200 or answer.get("ok") is not True or not isinstance(ticket, str):
                raise refused(status, answer)
            status, answer = await call(client, f"{base}/report",
                                        json.dumps(out.issue | {"ticket": ticket}, ensure_ascii=False).encode())
            if status != 200 or answer.get("ok") is not True:
                raise refused(status, answer)
            return str(answer.get("report_id") or out.report_id)
    try:
        return await asyncio.wait_for(go(), deadline)
    except asyncio.TimeoutError:
        raise Unsent("FERB couldn't send the report: it didn't finish in time") from None
