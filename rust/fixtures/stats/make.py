"""Fixtures for the Rust port of `ferb stats` (rust/crates/ferbd/src/stats.rs), made by the Python code (ENG-3).

    cd <repo> && uv run python rust/fixtures/stats/make.py

Each case builds a FERB home in a temporary folder (never ~/.ferb): no database, an empty file, or a store migrated by
ferb's own `Database.migrate` and filled with invented rows (tasks, spans, confirmations, journal rows, learning nodes
and episodes, resource samples), some with tables dropped. `ferb.cli.admin.stats` then runs with its clock fixed
(`datetime` in ferb.cli.admin replaced by a subclass whose `now` answers a set instant; TZ set for the local date) and
the case records `days`, the instant (UTC and local), the statements that filled the store, what it printed and its
exit status. Writes the same bytes on every run. Starts no program and touches no network.
"""

import io
import json
import os
import random
import shutil
import sqlite3
import sys
import tempfile
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.dont_write_bytecode = True
sys.path.insert(0, str(REPO))

TMP = Path(tempfile.mkdtemp(prefix="ferbstats-"))
os.environ["FERB_HOME"] = str(TMP / "home")
os.environ["HOME"] = str(TMP / "user")
(TMP / "user").mkdir()

from ferb.cli import admin  # noqa: E402
from ferb.store.db import Database  # noqa: E402

NOW = datetime(2026, 11, 15, 14, 30, 0, 250000, tzinfo=timezone.utc)
STATE: dict[str, datetime] = {"now": NOW}
REAL_DATETIME = admin.datetime


class FixedDatetime(datetime):
    """`datetime` whose `now` is the case's instant: aware in the zone asked for, naive local without one."""

    @classmethod
    def now(cls, tz: Any = None) -> datetime:  # type: ignore[override]
        at = STATE["now"]
        return at.astimezone(tz) if tz is not None else at.astimezone().replace(tzinfo=None)


def iso(at: datetime) -> str:
    return at.isoformat()


def ago(days: float = 0, hours: float = 0, minutes: float = 0, base: datetime = NOW,
        zone: timezone = timezone.utc) -> str:
    return iso((base - timedelta(days=days, hours=hours, minutes=minutes)).astimezone(zone))


class Rows:
    """The statements that fill one store, kept to replay in Rust."""

    def __init__(self) -> None:
        self.rows: list[list[Any]] = []
        self.n = 0

    def add(self, sql: str, *params: Any) -> None:
        self.rows.append([sql, list(params)])

    def nid(self, prefix: str) -> str:
        self.n += 1
        return f"{prefix}{self.n:04d}"

    def task(self, status: str, created: str, parent: str | None = None) -> str:
        tid = self.nid("t")
        self.add("INSERT INTO tasks (id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, "
                 "created_at, updated_at) VALUES (?, ?, ?, ?, ?, 'owner', 'worker', '[]', ?, ?, ?)",
                 tid, parent or tid, parent, "r" + tid, "invented goal " + tid, status, created, created)
        return tid

    def span(self, kind: str, started: str, inp: Any = None, out: Any = None, error: str | None = None,
             ended: str | None = None, task: str | None = None, raw_in: str | None = None,
             raw_out: str | None = None) -> str:
        sid = self.nid("s")
        self.add("INSERT INTO spans (id, task_id, kind, started_at, ended_at, input_json, output_json, error) "
                 "VALUES (?, ?, ?, ?, ?, ?, ?, ?)", sid, task, kind, started, ended,
                 raw_in if raw_in is not None else json.dumps(inp if inp is not None else {}),
                 raw_out if raw_out is not None else (None if out is None else json.dumps(out)), error)
        return sid

    def journal(self, kind: str, at: str, data: Any) -> None:
        jid = self.nid("j")
        self.add("INSERT INTO journal (id, at, kind, data_json, origin) VALUES (?, ?, ?, ?, 'ferb')", jid, at, kind,
                 json.dumps(data, ensure_ascii=False))

    def confirmation(self, granted: int, at: str) -> None:
        cid = self.nid("c")
        self.add("INSERT INTO confirmations (id, tree_id, rule, key, granted, at) VALUES (?, 'tree', "
                 "'floor:pay', ?, ?, ?)", cid, "floor:pay " + cid, granted, at)

    def node(self, nid: str, kind: str, key: str, body: Any, recorded: str, state: str = "active",
             valid_to: str | None = None) -> None:
        self.add("INSERT INTO learning_nodes (id, kind, key, body_json, origin, made_by, recorded_at, valid_from, "
                 "valid_to, state) VALUES (?, ?, ?, ?, 'ferb', 'test', ?, ?, ?, ?)", nid, kind, key,
                 json.dumps(body), recorded, recorded, valid_to, state)

    def episode(self, task: str, started: str | None, seconds: float | None, steps: Any, verified: int | None) -> None:
        self.add("INSERT INTO episodes (task_id, request_text, goal, status, verified, started_at, seconds, answer, "
                 "conditions_json, steps_json, problems_json, tools_json, origins, external_read, usage_json, "
                 "desktop_json, made_by, built_at) VALUES (?, 'r', 'g', 'done', ?, ?, ?, '', '[]', ?, '[]', '{}', "
                 "'', 0, '{}', '{}', 'test', ?)", task, verified, started, seconds, json.dumps(steps), ago(0))

    def sample(self, at: str, day: str, window_s: float, cpu_s: float, child_cpu_s: float,
               app_cpu_s: float | None, footprint: int | None, peak: int | None, db_bytes: int | None,
               work: Any) -> None:
        self.add("INSERT INTO resource_samples (at, day, window_s, cpu_s, footprint, peak_footprint, child_count, "
                 "child_footprint, child_cpu_s, app_cpu_s, db_bytes, work_json) VALUES (?, ?, ?, ?, ?, ?, 0, 0, ?, ?, "
                 "?, ?)", at, day, window_s, cpu_s, footprint, peak, child_cpu_s, app_cpu_s, db_bytes,
                 json.dumps(work) if not isinstance(work, str) else work)


def full_store() -> Rows:
    r = Rows()
    rnd = random.Random(20261115)
    # requests: ties between statuses, children not counted, older ones outside a short window
    for status, when in [("done", ago(hours=1)), ("done", ago(days=2)), ("done", ago(days=6)),
                         ("done_unverified", ago(hours=3)), ("done_unverified", ago(days=3)),
                         ("failed", ago(minutes=10)), ("failed", ago(days=1, hours=2)), ("cancelled", ago(days=5)),
                         ("needs_user", ago(hours=5, zone=timezone(timedelta(hours=5, minutes=30)))),
                         ("done", ago(days=12)), ("failed", ago(days=20)), ("interrupted", ago(days=40))]:
        root = r.task(status, when)
        r.task("done", when, parent=root)
    # tool calls: errors (one longer than 90 characters, one with non-ASCII text), sandbox timings, questions,
    # point checks, network
    long_error = "the element could not be found on the page after waiting for it to appear: " + "x" * 40
    for i in range(3):
        r.span("tool.call", ago(hours=i + 1), {"tool": "browser.click", "args": {}}, {}, error=long_error)
    for i in range(2):
        r.span("tool.call", ago(days=1, hours=i), {"tool": "files.read"}, {}, error="permission denied — “Ünïcödé” 文件")
    r.span("tool.call", ago(days=2), {"tool": "mac.type"}, {}, error="refused: the owner said No")
    r.span("tool.call", ago(days=2, hours=1), {"tool": "mac.type"}, {}, error="refused: the owner said No")
    r.span("tool.call", ago(days=9), {"tool": "mac.type"}, {}, error="an old error outside the week")
    for i in range(21):
        ms = rnd.choice([rnd.randint(5, 900), round(rnd.uniform(5, 900), 3)])
        r.span("tool.call", ago(hours=2 * i + 1), {"tool": "shell.run", "args": {"command": "ls"}},
               {"sandbox_ms": ms, "output": "ok"})
    r.span("tool.call", ago(days=8), {"tool": "shell.run"}, {"sandbox_ms": 99999})
    r.span("tool.call", ago(hours=4), {"tool": "shell.start"}, {"sandbox_ms": 12.25})
    r.span("tool.call", ago(hours=6), {"tool": "owner.ask", "ferb_question": 1}, {})
    r.span("tool.call", ago(hours=7), {"tool": "mac.click", "ferb_question": True}, {})
    r.span("tool.call", ago(hours=8), {"tool": "mac.click", "point_checks": ["hit_test", "hit_test"]}, {})
    r.span("tool.call", ago(hours=9), {"tool": "mac.click", "point_checks": ["window_list", "refused: cant_tell"]}, {})
    r.span("tool.call", ago(hours=9, minutes=5), {"tool": "mac.click", "point_checks": ["something_else"]}, {})
    r.span("tool.call", ago(hours=10), {"tool": "shell.run"},
           {"id": "p1", "network": {"destinations": [{"host": "example.org", "port": 443}],
                                    "refused": [{"host": "x.example", "count": 2}]}})
    r.span("tool.call", ago(hours=9, minutes=30), {"tool": "shell.run"},
           {"id": "p1", "network": {"destinations": [{"host": "example.org", "port": 443},
                                                     {"host": "api.example.net", "port": 8443}],
                                    "refused": [{"host": "x.example", "count": 3}, {"host": "y.example"}]}})
    r.span("tool.call", ago(hours=11), {"tool": "shell.start"},
           {"id": "p2", "network": {"destinations": [{"host": "example.org", "port": 443}], "refused": []}})
    r.span("tool.call", ago(hours=11, minutes=1), {"tool": "shell.poll"}, {"id": "", "network": {"destinations": []}})
    r.span("tool.call", ago(hours=11, minutes=2), {"tool": "browser.open"}, {"id": "p3", "network": "not a dict"})
    r.span("tool.call", ago(hours=12), {"tool": "fs.list"}, raw_out=None)
    # model runs
    for i, sec in enumerate([12.0, 7.5, 3.25]):
        start = NOW - timedelta(hours=i + 1)
        r.span("model.turn", iso(start), {"goal": "plan"}, ended=iso(start + timedelta(seconds=sec)))
    r.span("model.turn", ago(minutes=1), {"goal": "plan"})
    for i, sec in enumerate([65.5, 30.0]):
        start = NOW - timedelta(days=1, hours=i)
        r.span("worker", iso(start), {"goal": "work"}, ended=iso(start + timedelta(seconds=sec)))
    # learning calls: errors grouped with their numbers set aside, the latest wording shown
    r.span("model.learning", ago(days=3), {"role": "consolidate", "call": "c-001"})
    r.span("model.learning", ago(days=2), {"role": "consolidate", "call": "c-002"},
           error="the model answered after 120 s (attempt 1 of 3)")
    r.span("model.learning", ago(days=1), {"role": "consolidate", "call": "c-003"},
           error="the model answered after 95 s (attempt 2 of 3)")
    r.span("model.learning", ago(hours=20), {"role": "distill", "call": "c-004"}, error="bad JSON at 17")
    r.span("model.learning", ago(hours=19), {"call": "c-005"}, error="no role " + "é" * 200)
    r.span("model.learning", ago(hours=18), {"role": "distill", "call": "c-006"}, error="usage limit")
    # confirmations
    r.confirmation(1, ago(hours=2))
    r.confirmation(1, ago(days=1))
    r.confirmation(0, ago(days=2))
    r.confirmation(0, ago(days=10))
    # the daemon's own problems
    frames = ["ferb/daemon/main.py:120 in call", "ferb/store/db.py:88 in execute", "ferb/x.py:7 in f",
              "ferb/y.py:9 in g"]
    r.journal("daemon.internal_error", ago(days=2), {"method": "tasks.list", "error": "KeyError", "message": "'id'",
                                                     "frames": frames})
    r.journal("daemon.internal_error", ago(days=1), {"method": "tasks.list", "error": "KeyError",
                                                     "message": "'name'", "frames": frames[:2], "repeats": 4})
    r.journal("daemon.internal_error", ago(hours=5), {"method": "chat.send", "error": "ValueError",
                                                      "message": "m" * 200, "repeats": "2"})
    r.journal("daemon.internal_error", ago(hours=4), {"method": "about.get", "error": "TypeError", "repeats": 2.9,
                                                      "frames": "ab"})
    r.journal("daemon.internal_error", ago(days=9), {"method": "old", "error": "OldError"})
    for i in range(12):
        data: dict[str, Any] = {"method": f"m.{i}", "seconds": [12.5, 30, 10.25][i % 3]}
        if i % 4 == 0:
            data["repeats"] = i + 1
        r.journal("daemon.slow_call", ago(hours=30 - i), data)
    r.journal("escape.used", ago(hours=3), {"rule": "invented"})
    r.journal("escape.used", ago(days=8), {"rule": "invented"})
    # playbooks: runs with attempts and seconds; later runs shown the playbook or an earlier run's note
    r.node("pb1", "playbook", "k1", {"title": "Rename the weekly report", "tasks": ["e3", "e1", "e2", "e9"]},
           ago(days=4))
    r.node("pb2", "playbook", "k2", {"title": None, "tasks": ["e5"]}, ago(days=3))
    r.node("pb3", "playbook", "k3", {"title": "Empty", "tasks": []}, ago(days=2))
    r.node("pb4", "playbook", "k4", {"title": "Retired", "tasks": ["e1"]}, ago(days=1), state="retired")
    r.node("pb5", "playbook", "k5", {"title": "Old version", "tasks": ["e1"]}, ago(days=1), valid_to=ago(hours=1))
    r.node("ep-e1", "episode", "e1", {"how": "invented"}, ago(days=4))
    r.episode("e1", ago(days=4), 120.04, [{"role": "Worker"}, {"role": "Worker"}, {"role": "Checker"}], 1)
    r.episode("e2", ago(days=3), 45.0, [{"role": "Worker"}], 1)
    r.episode("e3", ago(days=2), 1234567.0, [], 0)
    r.episode("e5", None, None, [{"role": "Worker"}], None)
    r.span("worker", ago(days=3), {"goal": "w"}, task="e2.1")
    r.span("context.exposure", ago(days=3), {"refs": ["ep-e1"]}, task="e2")
    r.span("worker", ago(days=2), {"goal": "w"}, task="e3")
    for i in range(10):
        r.node(f"pbx{i}", "playbook", f"kx{i}", {"title": f"Extra {i}", "tasks": ["e2"]}, ago(hours=10 - i))
    return r


def work(i: int) -> dict[str, Any]:
    w: dict[str, Any] = {"events": {"tick": 3 + i, "focus": i % 3, "files": 2, "clip": 1},
                         "models": {"planner": i % 2, "worker": 1} if i % 3 else {},
                         "perception": {"wakes": i, "batches": {"screen": 1, "ax": i % 2}} if i % 2 else {},
                         "jev": {"asked": i % 2, "not_sampled": 1} if i % 4 else {},
                         "tool_calls": i * 2, "tasks_running": i % 3}
    if i % 5 == 1:
        w["top"] = [["Google Chrome Helper", 12.345], ["node", 0.05]]
    return w


def samples(r: Rows, days: int, local: timezone) -> None:
    """Resource samples over several local days (some days with FERB.app not running or memory not read)."""
    base = NOW.astimezone(local)
    i = 0
    for back in range(days):
        day = (base - timedelta(days=back)).date()
        for k in range(3):
            at = datetime(day.year, day.month, day.day, 9 + 4 * k, 5 * k, 0, tzinfo=local)
            if at > base:
                continue
            i += 1
            r.sample(iso(at), day.isoformat(), 300.0 - 7 * k + 0.5 * back, 1.5 * k + 0.25 * back + 0.05 * i,
                     0.5 * ((i * 7) % 5), None if back % 3 == 1 else 0.125 * i,
                     None if back == 2 else 150_000_000 + 1_000_000 * i,
                     None if back == 2 else 900_000_000 + 30_000_000 * i,
                     None if back == 4 else 2_400_000 * (i + 1) ** 3, work(i))
    r.sample(iso(base), base.date().isoformat(), 0.0, 0.0, 0.0, None, 999, 999, 999, "{}")


CASES: list[dict[str, Any]] = []


def run(name: str, days: int, tz: str, db: str = "migrated", rows: Rows | None = None,
        drop: tuple[str, ...] = (), now: datetime = NOW) -> None:
    os.environ["TZ"] = tz
    time.tzset()
    STATE["now"] = now
    home = TMP / name
    home.mkdir()
    path = home / "ferb.db"
    statements = rows.rows if rows is not None else []
    if db == "empty_file":
        path.write_bytes(b"")
    elif db == "migrated":
        d = Database(path)
        d.migrate()
        d.conn.close()
        c = sqlite3.connect(path)
        c.execute("PRAGMA foreign_keys=OFF")
        for sql, params in statements:
            c.execute(sql, params)
        for table in drop:
            c.execute(f"DROP TABLE {table}")
        c.commit()
        c.close()
    out = io.StringIO()
    admin.datetime = FixedDatetime
    error = None
    try:
        status = admin.stats(home, out, days)
    except Exception as e:  # noqa: BLE001 - uncaught in `ferb`: a traceback and exit status 1
        status, error = 1, f"{type(e).__name__}: {e}"
    finally:
        admin.datetime = REAL_DATETIME
    CASES.append({"name": name, "days": days, "tz": tz, "now_utc": now.isoformat(),
                  "now_local": now.astimezone().replace(tzinfo=None).isoformat(), "db": db, "drop": list(drop),
                  "rows": statements, "stdout": out.getvalue().replace(str(home), "{home}"), "status": status,
                  "python_error": error})


def main() -> None:
    kolkata, auckland = timezone(timedelta(hours=5, minutes=30)), timezone(timedelta(hours=13))
    run("no_database", 7, "UTC", db="none")
    run("empty_file", 7, "UTC", db="empty_file")
    run("empty_store", 7, "UTC")
    run("empty_store_days_0", 0, "UTC")
    for days in (7, 1, 0, 3, 30):
        r = full_store()
        samples(r, 9, kolkata)
        run(f"full_days_{days}", days, "Asia/Kolkata", rows=r)
    r = full_store()
    samples(r, 9, auckland)
    run("full_auckland", 7, "Pacific/Auckland", rows=r)
    r = full_store()
    samples(r, 9, kolkata)
    run("no_confirmations_no_samples_table", 7, "Asia/Kolkata", rows=r, drop=("confirmations", "resource_samples"))
    r = full_store()
    run("no_learning_tables", 7, "UTC", rows=r, drop=("episodes",))
    r = Rows()
    r.sample(ago(days=20), (NOW - timedelta(days=20)).date().isoformat(), 300.0, 1.0, 0.0, None, None, None, None,
             "{}")
    run("samples_only_old", 7, "UTC", rows=r)
    run("samples_none_table_present", 7, "UTC", rows=Rows())
    r = Rows()
    r.span("tool.call", ago(hours=1), {"tool": "shell.run"}, {"sandbox_ms": 40})
    r.span("tool.call", ago(hours=2), {"tool": "shell.run"}, {"sandbox_ms": "12.5"})
    r.task("done", ago(hours=1))
    run("two_sandbox_values_one_done", 7, "UTC", rows=r)
    r = Rows()
    r.span("tool.call", ago(hours=1), {"tool": "shell.run"}, {"sandbox_ms": 0.25})
    r.task("failed", ago(hours=1))
    r.task("running", ago(hours=1))
    run("one_sandbox_value", 7, "UTC", rows=r, now=NOW.replace(microsecond=0))
    # tests/test_resource_log.py's scenarios: each day's line and the hottest windows; a short window never the peak
    mb = 1_000_000
    busy = {"events": {"accessibility": 1200, "window": 40, "file": 3, "clipboard": 1},
            "perception": {"wakes": 40, "batches": {"age": 2, "tokens": 1}}, "jev": {"asked": 5, "not_sampled": 20},
            "models": {"model.turn": 2, "worker": 1}, "tool_calls": 14, "tasks_running": 1,
            "top": [["codex", 30.1], ["zsh", 2.0]]}
    day, before = "2026-11-15", "2026-11-14"
    r = Rows()
    r.sample(f"{day}T10:00:00+05:30", day, 300.0, 3.0, 0.0, None, 300 * mb, 500 * mb, 900 * mb, {})
    r.sample(f"{day}T10:05:00+05:30", day, 300.0, 150.0, 9.0, 1.0, 500 * mb, 1200 * mb, 950 * mb, busy)
    r.sample(f"{day}T10:06:00+05:30", day, 60.0, 0.0, 0.0, None, 400 * mb, 1200 * mb, 1200 * mb, {})
    r.sample(f"{before}T23:55:00+05:30", before, 300.0, 30.0, 3.0, None, None, None, 800 * mb, {})
    r.sample(f"{day}T10:07:00+05:30", day, 2.0, 1.8, 0.0, None, None, None, None, {})
    r.sample("2000-01-01T10:00:00+05:30", "2000-01-01", 300.0, 999.0, 0.0, None, None, None, None, {})
    run("resource_log_days", 7, "Asia/Kolkata", rows=r)
    run("resource_log_days_1", 1, "Asia/Kolkata", rows=r)
    run("resource_log_a_century", 36500, "Asia/Kolkata", rows=r)
    run("resource_log_negative_days", -2, "Asia/Kolkata", rows=r)
    shutil.rmtree(TMP)
    text = json.dumps({"cases": CASES}, ensure_ascii=False, indent=1, sort_keys=False) + "\n"
    (HERE / "stats.json").write_text(text, encoding="utf-8")
    print(f"{len(CASES)} cases")


if __name__ == "__main__":
    main()
