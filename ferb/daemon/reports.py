"""Own the failure report the owner can send the developer (L35, the simple cut the owner approved 2026-10-25): one
plain-text file per failed task, under ~/.ferb/reports, redacted whole again before it is written. Must not send
anything or judge why a task failed: FERB writes the file and the notch opens a new email in the owner's own Mail app;
the owner reads the file and presses Send."""

import json
import os
import platform
import re
import tempfile
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, Callable

import ferb
from ferb.core.errors import FerbError
from ferb.store.db import Database
from ferb.trace import lines

# Where reports go (owner, 2026-10-25: "just send to ktky.attri@gmail.com as of now"). Config `[feedback] address`
# replaces it and an empty one refuses; the app never holds it: the daemon returns it with each report.
FEEDBACK_ADDRESS = "ktky.attri@gmail.com"
KEEP_DAYS = 30                       # the span retention the L35 design names
ERROR_LINES = 200                    # the daemon error log's last lines
REPORTABLE = ("failed", "interrupted")
PLANNING_FAILED = "planning failed"  # a turn that ended on a planning failure (journaled `chat.failed`; no task)
SECTIONS = ("Summary", "Request", "Steps", "Recent FERB errors")
SUBJECT_CHARS = 80
FOLDER = "reports"
NAME = re.compile(r"FERB-report-\d{4}-\d{2}-\d{2}-[A-Za-z0-9._-]+\.txt")
SAFE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")


@dataclass(frozen=True)
class Failure:
    """What failed, from FERB's own records."""
    task_id: str            # the root task, or the request a planning failure ended
    status: str             # the task's status, or PLANNING_FAILED
    reason: str             # the owner-facing words (L11): the outcome's summary, else what FERB answered
    request: str            # the owner's request words
    answer: str             # FERB's answer
    started: str | None
    ended: str | None


def failure(db: Database, task_id: str) -> Failure:
    """A root task that ended failed or interrupted, or a request whose turn a planning failure ended; anything else
    is refused with the reason."""
    unknown = FerbError(f"FERB has no failed task or request {task_id!r}")
    if not SAFE_ID.fullmatch(task_id):
        raise unknown
    row = db.conn.execute("SELECT * FROM tasks WHERE id = ?", (task_id,)).fetchone()
    if row is not None:
        if row["parent_id"] is not None:
            raise FerbError(f"task {task_id} is part of {row['tree_id']}; send the report for {row['tree_id']}")
        if row["status"] not in REPORTABLE:
            raise FerbError("Only a failed or interrupted task can be sent to the developer; this one is "
                            f"{str(row['status']).replace('_', ' ')}.")
        outcome = json.loads(row["outcome_json"] or "{}")
        told, _ = _told(db, task_id)
        answer = str(outcome.get("answer") or told or outcome.get("summary") or "")
        return Failure(task_id, str(row["status"]), str(outcome.get("summary") or answer),
                       _message(db, str(row["request_id"])) or str(row["goal"]), answer,
                       row["created_at"], row["updated_at"])
    failed = db.conn.execute("SELECT at, data_json FROM journal WHERE kind = 'chat.failed' AND "
                             "json_extract(data_json, '$.request_id') = ? ORDER BY seq DESC LIMIT 1",
                             (task_id,)).fetchone()
    if failed is None:
        raise unknown
    reply = db.conn.execute("SELECT data_json FROM journal WHERE kind = 'chat.reply' AND "
                            "json_extract(data_json, '$.request_id') = ? ORDER BY seq DESC LIMIT 1",
                            (task_id,)).fetchone()
    told, goal = _told(db, task_id)                  # a scheduled run's: no chat message, no chat reply
    answer = told or (str(json.loads(reply[0]).get("text") or "") if reply else "")
    why = json.loads(failed["data_json"]).get("why") or ""          # FERB's own record when no answer was kept
    asked = db.conn.execute("SELECT at FROM journal WHERE kind IN ('chat.ack', 'chat.message') AND "
                            "json_extract(data_json, '$.request_id') = ? ORDER BY seq LIMIT 1", (task_id,)).fetchone()
    return Failure(task_id, PLANNING_FAILED, answer or str(why), _message(db, task_id) or goal, answer,
                   asked[0] if asked else None, failed["at"])


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


def build(found: Failure, trace: dict[str, Any], facts: dict[str, Any], *, home: Path, now: datetime,
          redact: Callable[[str], str], app_version: str = "") -> dict[str, str]:
    """Writes the report and returns {path, subject, summary}. The whole text passes the daemon's redactor (its sealed
    values included) once more before it is written (rule 14: stored records were redacted at capture; this is
    defense in depth). Reports older than KEEP_DAYS go first."""
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
                ("Task", found.task_id), ("Status", found.status),
                ("Failure reason", found.reason.strip() or "(none recorded)"),
                ("Started", found.started or "unknown"), ("Ended", found.ended or "unknown"),
                ("Duration", lines.duration(seconds) if seconds is not None else "unknown")]
    errors = error_tail(home / "logs" / "daemon.err.log")
    parts = [f"FERB report for {found.task_id}", "",
             "## Summary", *(f"{key}: {value}" for key, value in summary), "",
             "## Request", "Your request:", found.request.strip() or "(not recorded)", "",
             "FERB's answer:", found.answer.strip() or "(none recorded)", "",
             "## Steps", *render_trace(trace, True), "",
             "## Recent FERB errors", f"The last {ERROR_LINES} lines of FERB's error log (logs/daemon.err.log):",
             *(errors or ["(none recorded)"]), ""]
    text = redact("\n".join(parts))
    reason = redact(found.reason)
    short = next((line.strip() for line in reason.splitlines() if line.strip()), found.status)
    folder = home / FOLDER
    purge(folder, now)
    path = write(folder, f"FERB-report-{now.astimezone():%Y-%m-%d}-{found.task_id}.txt", text)
    told = redact("\n".join([f"What happened: {lines.clip(short, 300)}",
                             f"Task: {found.task_id} ({found.status})",
                             f"FERB {ferb.__version__}, macOS {platform.mac_ver()[0] or 'unknown'}"]))
    return {"path": str(path), "subject": "FERB report: " + lines.clip(short, SUBJECT_CHARS), "summary": told}


def error_tail(log: Path, n: int = ERROR_LINES) -> list[str]:
    """The log's last `n` lines, read from its end; none when there is no log."""
    try:
        with log.open("rb") as f:
            pos = f.seek(0, os.SEEK_END)
            data = b""
            while pos > 0 and data.count(b"\n") <= n:
                step = min(65536, pos)
                pos -= step
                f.seek(pos)
                data = f.read(step) + data
    except FileNotFoundError:
        return []
    return data.decode("utf-8", "replace").splitlines()[-n:]


def purge(folder: Path, now: datetime, days: int = KEEP_DAYS) -> int:
    """Removes FERB's own reports (by name) last written more than `days` ago; how many."""
    if not folder.is_dir():
        return 0
    cutoff = now.timestamp() - days * 86400
    gone = 0
    for p in folder.iterdir():
        if NAME.fullmatch(p.name) and not p.is_symlink() and p.is_file() and p.stat().st_mtime < cutoff:
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
