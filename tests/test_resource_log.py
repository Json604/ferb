"""L30 (perf plan 2026-10-23, Task 4): FERB's resource log. Every 5 minutes the daemon writes one row of numbers (its
CPU and memory, its descendants', FERB.app's, the database's size and the window's work counts), kept
`[retention] resource_days` (30), shown per day by `ferb stats`. Numbers only, never content.

The readings are injected (`Fake` below); only `test_libproc_reads_this_process_and_its_children` reads the machine."""
import asyncio
import io
import itertools
import json
import os
import sqlite3
import subprocess
import sys
import time
from datetime import date, datetime, timedelta, timezone
from pathlib import Path
from typing import Any, Callable

import pytest

from ferb.clock.clock import FakeClock
from ferb.config import Config
from ferb.core.ids import SequentialIds
from ferb.daemon import resources
from ferb.daemon.resources import MacReadings, Process, ResourceLog, mach_seconds
from ferb.store.db import Database, store_size
from tests.conftest import RunningDaemon

DAEMON = 100
MB = 1_000_000


class Fake:
    """Readings as the tests set them: processes alive now, their children, their executables, and the daemon's own
    and finished children's CPU (getrusage)."""

    def __init__(self, pid: int = DAEMON) -> None:
        self.pid = pid
        self.own = 0.0
        self.reaped = 0.0
        self.procs: dict[int, Process] = {pid: Process(pid, 1, 0.0, 100 * MB, 100 * MB)}
        self.kids: dict[int, list[int]] = {}
        self.paths: dict[int, str] = {}
        self.fail = False

    def own_cpu(self) -> float:
        if self.fail:
            raise OSError("can't read")
        return self.own

    def children_cpu(self) -> float:
        return self.reaped

    def process(self, pid: int) -> Process | None:
        return self.procs.get(pid)

    def children(self, pid: int) -> list[int]:
        return list(self.kids.get(pid, []))

    def pids(self) -> list[int]:
        return list(self.procs)

    def path(self, pid: int) -> str | None:
        return self.paths.get(pid)

    def spawn(self, pid: int, parent: int, start: int, cpu: float, footprint: int = 0, path: str | None = None) -> None:
        self.procs[pid] = Process(pid, start, cpu, footprint, footprint)
        self.kids.setdefault(parent, []).append(pid)
        if path:
            self.paths[pid] = path

    def run(self, pid: int, cpu: float) -> None:
        self.procs[pid] = self.procs[pid]._replace(cpu_s=cpu)

    def waited(self, pid: int, cpu: float) -> None:
        """What `pid`'s children it waited for have used, theirs included (ri_child_user/system_time)."""
        self.procs[pid] = self.procs[pid]._replace(children_cpu_s=cpu)

    def end(self, pid: int) -> None:
        del self.procs[pid]
        for kids in self.kids.values():
            if pid in kids:
                kids.remove(pid)


def _db(tmp_path: Path) -> Database:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    return db


def _log(tmp_path: Path, fake: Fake, redact: Callable[[str], str] = lambda text: text,
         perception: Callable[[], tuple[int, dict[str, int]]] = lambda: (0, {})) -> tuple[Database, FakeClock, ResourceLog]:
    db, clock = _db(tmp_path), FakeClock()
    return db, clock, ResourceLog(db, clock, fake, redact, perception, pid=fake.pid)


def _sample(log: ResourceLog, clock: FakeClock, seconds: float = 300) -> dict[str, Any]:
    clock.advance(seconds)
    return log.sample()


def _rows(db: Database) -> list[dict[str, Any]]:
    return [dict(r) for r in db.conn.execute("SELECT * FROM resource_samples ORDER BY id")]


# ---------- the readings ----------

def test_a_sample_takes_the_daemons_cpu_and_memory_from_the_readings(tmp_path: Path) -> None:
    fake = Fake()
    fake.own = 10.0
    db, clock, log = _log(tmp_path, fake)
    fake.own = 13.5
    fake.procs[DAEMON] = Process(DAEMON, 1, 13.5, 400 * MB, 900 * MB)
    size = store_size(db.path)
    _sample(log, clock)
    [row] = _rows(db)
    assert row["window_s"] == pytest.approx(300) and row["cpu_s"] == pytest.approx(3.5)
    assert row["footprint"] == 400 * MB and row["peak_footprint"] == 900 * MB
    assert row["at"] == clock.now().isoformat() and row["day"] == clock.now().astimezone().date().isoformat()
    assert row["db_bytes"] == size > 0                                  # read before the row is written
    assert row["child_count"] == 0 and row["child_cpu_s"] == 0 and row["child_footprint"] == 0
    assert row["app_cpu_s"] is None and row["app_footprint"] is None          # FERB.app isn't running


def test_a_daemon_that_cant_be_read_has_no_memory_numbers(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    del fake.procs[DAEMON]
    _sample(log, clock)
    [row] = _rows(db)
    assert row["footprint"] is None and row["peak_footprint"] is None and row["cpu_s"] == 0


def test_mach_time_converts_with_the_timebase() -> None:
    assert mach_seconds(24_000_000, 125, 3) == pytest.approx(1.0)          # Apple silicon: one unit is 41.67 ns
    assert mach_seconds(1_500_000_000, 1, 1) == pytest.approx(1.5)          # Intel: one unit is one nanosecond


def test_a_timebase_that_cant_be_read_fails_instead_of_reading_zero(monkeypatch: pytest.MonkeyPatch) -> None:
    class Lib:
        mach_timebase_info: Any = None

    def failing(timebase: Any) -> int:
        return 5                                                         # KERN_FAILURE
    def untouched(timebase: Any) -> int:
        return 0                                                         # success, but nothing written
    for call in (failing, untouched):
        Lib.mach_timebase_info = staticmethod(call)
        with pytest.raises(OSError, match="mach_timebase_info"):
            resources.mach_timebase(Lib())
    monkeypatch.setattr(resources, "mach_timebase", lambda: (_ for _ in ()).throw(OSError("mach_timebase_info: 5")))
    with pytest.raises(OSError):
        MacReadings()                       # the log then fails to start (journaled), never reads CPU as 0


@pytest.mark.skipif(sys.platform != "darwin", reason="libproc is macOS's")
def test_libproc_reads_this_process_and_its_children() -> None:
    readings = MacReadings()
    child = subprocess.Popen(["/bin/sleep", "5"])
    try:
        burn = time.process_time() + 0.2
        while time.process_time() < burn:                    # some CPU of its own to compare with getrusage
            pass
        me, own = readings.process(os.getpid()), readings.own_cpu()
        assert me is not None and me.pid == os.getpid()
        assert me.footprint > 0 and me.peak_footprint >= me.footprint and me.cpu_s >= 0
        assert me.cpu_s == pytest.approx(own, rel=0.1, abs=0.1)   # mach units converted (not 24 times too small)
        assert readings.children_cpu() >= 0
        assert child.pid in readings.children(os.getpid()) and os.getpid() in readings.pids()
        kid = readings.process(child.pid)
        assert kid is not None and kid.start > me.start
        assert Path(readings.path(child.pid) or "").name == "sleep"
        assert readings.process(-5) is None and readings.path(-5) is None
        numer, denom = resources.mach_timebase()
        assert numer > 0 and denom > 0
    finally:
        child.kill()
        child.wait()


# ---------- the window's work counts ----------

_ROWS = itertools.count()


def _trail(db: Database, *sources: str) -> None:
    for source in sources:
        db.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, source, data_json) "
                        "VALUES (?, '2026-10-23T10:00:00+00:00', '2026-10-23', 'dev', 'owner', ?, '{}')",
                        (f"t{next(_ROWS)}", source))


def _decision(db: Database, ident: str, error: str | None) -> None:
    db.conn.execute("INSERT INTO decisions (id, at, question, baseline, mode, error) "
                    "VALUES (?, '2026-10-23T10:00:00+00:00', 'trail.attention', 'no', 'shadow', ?)", (ident, error))


def _span(db: Database, ident: str, kind: str) -> None:
    db.conn.execute("INSERT INTO spans (id, kind, started_at, input_json) VALUES (?, ?, '2026-10-23T10:00:00+00:00', "
                    "'{}')", (ident, kind))


def _task(db: Database, ident: str, status: str) -> None:
    db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, "
                    "updated_at) VALUES (?, ?, 'r', 'g', 'owner', 'mixed', '[]', ?, 'x', 'x')", (ident, ident, status))


def test_the_work_counts_are_what_was_added_since_the_last_sample(tmp_path: Path) -> None:
    counters = [(2, {"age": 1})]
    fake = Fake()
    db, clock = _db(tmp_path), FakeClock()
    _trail(db, "accessibility", "accessibility")                     # before start-up: never counted
    _decision(db, "d0", None)
    _span(db, "s0", "worker")
    previews = [{"requests": 4, "frames": 3, "capture_ms": 90.0, "cached": 1, "busy": 0}]
    log = ResourceLog(db, clock, fake, lambda text: text, lambda: counters[-1], pid=DAEMON,
                      preview=lambda: previews[-1])
    _trail(db, "accessibility", "app_focus", "accessibility", "app_focus", "accessibility")
    _decision(db, "d1", "not sampled")
    _decision(db, "d2", "not sampled")
    _decision(db, "d3", None)
    _decision(db, "d4", "unavailable")                                 # asked; Jev didn't answer
    for i, kind in enumerate(("model.turn", "worker", "model.learning", "tool.call", "tool.call", "tool.call", "task")):
        _span(db, f"s{i + 1}", kind)
    _task(db, "a", "running")
    _task(db, "b", "done")
    counters.append((7, {"age": 2, "tokens": 1}))
    previews.append({"requests": 10, "frames": 8, "capture_ms": 340.5, "cached": 2, "busy": 1})
    work = json.loads(_sample(log, clock)["work_json"])
    assert work == {"events": {"accessibility": 3, "app_focus": 2},
                    "perception": {"wakes": 5, "batches": {"age": 1, "tokens": 1}},
                    "jev": {"asked": 2, "not_sampled": 2},
                    "models": {"model.learning": 1, "model.turn": 1, "worker": 1}, "tool_calls": 3,
                    "tasks_running": 1, "top": [],
                    # the live preview's work in the window (notes/preview.md decision 3)
                    "preview": {"requests": 6, "frames": 5, "capture_ms": 250.5, "cached": 1, "busy": 1}}
    # Deleting rows the last sample already counted (retention, forget) changes nothing.
    for table in ("trail", "decisions", "spans"):
        db.conn.execute(f"DELETE FROM {table}")
    _trail(db, "file")
    work = json.loads(_sample(log, clock)["work_json"])
    assert work == {"events": {"file": 1}, "perception": {"wakes": 0, "batches": {}},
                    "jev": {"asked": 0, "not_sampled": 0}, "models": {}, "tool_calls": 0, "tasks_running": 1,
                    "top": [], "preview": {"requests": 0, "frames": 0, "capture_ms": 0.0, "cached": 0, "busy": 0}}


def test_the_work_counts_are_range_searches_never_a_table_scan(tmp_path: Path) -> None:
    db = _db(tmp_path)
    db.conn.execute("BEGIN")
    for i in range(3000):
        db.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, source, data_json) VALUES "
                        "(?, 'x', 'd', 'dev', 'owner', ?, '{}')", (f"t{i}", ("accessibility", "window", "file")[i % 3]))
        if i % 10 == 0:
            _decision(db, f"d{i}", "not sampled" if i % 20 else None)
            _span(db, f"s{i}", ("model.turn", "tool.call", "task")[i % 3])
    db.conn.execute("COMMIT")
    db.conn.execute("ANALYZE")                  # statistics must not turn a range into a scan of an index or table
    for sql in (resources.EVENTS, resources.JEV, resources.MODELS):
        plan = " | ".join(r[3] for r in db.conn.execute("EXPLAIN QUERY PLAN " + sql, (10, 20)))
        assert "INTEGER PRIMARY KEY (rowid>? AND rowid<?)" in plan and "SCAN" not in plan, plan
    for sql in (*resources.CURSORS.values(), resources.RUNNING):
        plan = " | ".join(r[3] for r in db.conn.execute("EXPLAIN QUERY PLAN " + sql))
        assert "SCAN" not in plan, plan


def test_spans_reusing_deleted_rowids_are_missed_for_one_window_at_most(tmp_path: Path) -> None:
    """`spans` has no AUTOINCREMENT: after retention deletes the newest spans, new ones can take their rowids. Only
    those at or below the last sample's cursor in the next window go uncounted; the cursor follows the table."""
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    for i in range(5):
        _span(db, f"s{i}", "tool.call")
    assert json.loads(_sample(log, clock)["work_json"])["tool_calls"] == 5
    db.conn.execute("DELETE FROM spans WHERE rowid > 3")
    _span(db, "s5", "tool.call")                                         # takes rowid 4
    assert json.loads(_sample(log, clock)["work_json"])["tool_calls"] == 0
    _span(db, "s6", "tool.call")                                         # rowid 5
    assert json.loads(_sample(log, clock)["work_json"])["tool_calls"] == 1


def test_the_perception_batcher_counts_its_wakes_and_batches_by_reason(tmp_path: Path) -> None:
    from ferb.sanitize.redact import Redactor
    from ferb.sense.batches import EventBatcher
    from ferb.sense.exclusions import Exclusions
    from ferb.sense.notifications import NotificationPolicy
    from ferb.sense.trail import Observation, Trail
    db, clock, ids = _db(tmp_path), FakeClock(), SequentialIds("b")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *args: None, lambda: "dev", lambda: False)

    class Jev:
        async def answers(self, state: Any, asks: Any) -> dict[str, str]:
            return {asks[0].key: "no"}
    jev: list[Any] = [None]
    batcher = EventBatcher(db, clock, ids, lambda: jev[0], NotificationPolicy(db, clock, ids, Redactor()),
                           lambda *args: None)
    assert batcher.wakes == 0 and batcher.batches_by_reason == {}

    async def go() -> None:
        stop = asyncio.Event()
        job = asyncio.ensure_future(batcher.run(stop))
        for _ in range(3):
            await asyncio.sleep(0)
        assert batcher.wakes == 1                       # the first look, then asleep: no timer
        batcher.notify(None)
        for _ in range(10):
            await asyncio.sleep(0)
        assert batcher.wakes == 2
        stop.set()
        await job
    asyncio.run(go())
    trail.record(Observation("notification", {"bundle_id": "com.whatsapp.WhatsApp", "conversation": "self",
                                              "body": "hi"}))
    trail.record(Observation("window", {"bundle_id": "com.apple.Notes", "title": "x"}))
    jev[0] = Jev()
    asyncio.run(batcher.flush("interrupt"))
    assert batcher.batches_by_reason == {"interrupt": 2}        # one batch per notification, one per app window


# ---------- FERB's other processes ----------

def test_a_new_childs_cpu_is_counted_once(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    fake.spawn(200, DAEMON, start=7, cpu=2.0, footprint=50 * MB)
    row = _sample(log, clock)
    assert row["child_count"] == 1 and row["child_footprint"] == 50 * MB
    assert row["child_cpu_s"] == pytest.approx(2.0)                  # first seen: its CPU since it started
    fake.run(200, 2.5)
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(0.5)
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(0.0)


def test_descendants_are_walked_through_their_parents(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    fake.spawn(200, DAEMON, start=7, cpu=1.0, footprint=10 * MB)
    fake.spawn(300, 200, start=8, cpu=2.0, footprint=20 * MB)
    fake.spawn(400, 300, start=9, cpu=3.0, footprint=30 * MB)
    fake.procs[500] = Process(500, 3, 9.0, 99 * MB, 99 * MB)           # someone else's process
    row = _sample(log, clock)
    assert row["child_count"] == 3 and row["child_footprint"] == 60 * MB
    assert row["child_cpu_s"] == pytest.approx(6.0)


def test_a_reused_pid_with_a_new_start_time_is_counted_fresh(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    fake.spawn(200, DAEMON, start=7, cpu=5.0)
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(5.0)
    fake.end(200)
    fake.reaped += 5.0                                                  # it ended, with nothing unseen
    fake.spawn(200, DAEMON, start=9, cpu=1.0)                           # a new process with the same pid
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(1.0)


def test_a_finished_direct_child_is_not_counted_again_through_rusage_children(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    fake.spawn(200, DAEMON, start=7, cpu=2.0)
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(2.0)
    fake.end(200)
    fake.reaped += 2.3                                                  # its whole CPU, 0.3 s of it since last seen
    row = _sample(log, clock)
    assert row["child_cpu_s"] == pytest.approx(0.3) and row["child_count"] == 0
    fake.spawn(201, DAEMON, start=8, cpu=0.5)
    fake.end(201)
    fake.reaped += 0.5                                                  # never seen alive: counted from RUSAGE_CHILDREN
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(0.5)


def test_a_child_reaped_after_the_sample_that_missed_it_is_counted_once(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    fake.spawn(200, DAEMON, start=7, cpu=2.0)
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(2.0)
    fake.end(200)                                                       # gone, but its CPU not added to ours yet
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(0.0)
    fake.reaped += 2.4
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(0.4)
    fake.reaped += 1.0
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(1.0)    # nothing left to take off


def test_a_grandchild_its_parent_waited_for_is_not_counted_again(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    fake.spawn(200, DAEMON, start=7, cpu=1.0)                           # a shell
    fake.spawn(300, 200, start=8, cpu=4.0)                              # the command it runs
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(5.0)
    fake.end(300)
    fake.waited(200, 4.5)                                               # the shell waited for it: 0.5 s unseen
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(0.5)
    fake.end(200)
    fake.reaped += 1.2 + 4.5                                            # the shell's CPU and its children's
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(0.2)


def test_a_command_a_live_child_ran_counts_in_the_window_it_ended(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    fake.spawn(200, DAEMON, start=7, cpu=1.0)                           # a long-lived agent
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(1.0)
    fake.waited(200, 3.0)                                               # commands it ran and waited for, unseen
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(3.0)
    fake.run(200, 1.5)
    fake.end(200)
    fake.reaped += 1.5 + 3.0
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(0.5)    # only its own unseen tail


def test_descendants_under_an_unreadable_process_are_walked_and_counted_once(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    fake.spawn(200, DAEMON, start=7, cpu=1.0)
    fake.spawn(300, 200, start=8, cpu=2.0)
    fake.spawn(400, 300, start=9, cpu=4.0)
    del fake.procs[300]                                                 # can't be read; its child still is
    row = _sample(log, clock)
    assert row["child_count"] == 2 and row["child_cpu_s"] == pytest.approx(5.0)
    fake.end(400)                                                       # 300 waited for it
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(0.0)
    fake.kids[200].remove(300)
    fake.waited(200, 2.5 + 4.5)                                         # 200 waited for 300: 300's CPU and 400's
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(2.5 + 0.5)


def test_the_orphaned_child_of_an_unreadable_process_is_never_taken_off(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    fake.spawn(200, DAEMON, start=7, cpu=1.0)
    del fake.procs[200]                                                 # can't be read
    fake.spawn(300, 200, start=8, cpu=4.0)
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(4.0)
    fake.kids[DAEMON].remove(200)
    fake.kids.pop(200)
    fake.kids.setdefault(1, []).append(300)                             # 200 ended; 300 runs on as launchd's
    fake.reaped += 1.0
    fake.run(300, 4.2)
    row = _sample(log, clock)
    assert row["child_cpu_s"] == pytest.approx(1.0) and row["child_count"] == 0


def test_a_descendant_left_running_by_its_parent_is_never_taken_off(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake)
    fake.spawn(200, DAEMON, start=7, cpu=1.0)
    fake.spawn(300, 200, start=8, cpu=4.0)
    assert _sample(log, clock)["child_cpu_s"] == pytest.approx(5.0)
    fake.end(200)
    fake.reaped += 1.1                                                  # only the parent's own CPU: 300 still runs
    fake.procs[300] = Process(300, 8, 4.2, 0, 0)
    fake.kids.setdefault(1, []).append(300)                             # launchd's now, no longer FERB's
    row = _sample(log, clock)
    assert row["child_cpu_s"] == pytest.approx(0.1) and row["child_count"] == 0


def test_the_top_three_processes_are_named_by_their_redacted_executable(tmp_path: Path) -> None:
    fake = Fake()
    db, clock, log = _log(tmp_path, fake, redact=lambda text: text.replace("secret", "[REDACTED]"))
    fake.spawn(200, DAEMON, 7, 30.0, path="/Users/k/.ferb/bin/codex")
    fake.spawn(201, DAEMON, 8, 2.0, path="/bin/zsh")
    fake.spawn(202, DAEMON, 9, 5.0, path="/Users/k/project/secret-tool")
    fake.spawn(203, DAEMON, 10, 0.1, path="/bin/sleep")
    fake.spawn(204, DAEMON, 11, 0.0, path="/bin/cat")
    fake.spawn(205, DAEMON, 12, 9.0)                                   # an executable that can't be read
    row = _sample(log, clock)
    assert json.loads(row["work_json"])["top"] == [["codex", 30.0], ["unknown", 9.0], ["[REDACTED]-tool", 5.0]]
    assert "/Users" not in row["work_json"] and "project" not in row["work_json"]


def test_ferb_app_is_its_processes_outside_the_daemons_tree(tmp_path: Path) -> None:
    fake = Fake()
    fake.paths[DAEMON] = "/Applications/FERB.app/Contents/Helpers/FERB.app/Contents/MacOS/FERB"   # the agent app
    fake.procs[500] = Process(500, 2, 100.0, 80 * MB, 90 * MB)
    fake.paths[500] = "/Applications/FERB.app/Contents/MacOS/FERBNotch"
    fake.procs[700] = Process(700, 3, 50.0, 1, 1)
    fake.paths[700] = "/Applications/Safari.app/Contents/MacOS/Safari"
    fake.spawn(200, DAEMON, start=4, cpu=9.0, path="/Applications/FERB.app/Contents/Resources/python/bin/python3.12")
    db, clock, log = _log(tmp_path, fake)                              # start-up: what each has used so far
    fake.run(500, 103.0)
    fake.run(200, 10.0)
    fake.procs[600] = Process(600, 5, 0.5, 7 * MB, 7 * MB)            # opened later, by launchd (not a descendant)
    fake.paths[600] = "/Applications/FERB.app/Contents/MacOS/FERBNotch"
    fake.run(700, 60.0)
    row = _sample(log, clock)
    assert row["app_cpu_s"] == pytest.approx(3.5) and row["app_footprint"] == 87 * MB
    assert row["child_cpu_s"] == pytest.approx(1.0) and row["child_count"] == 1      # counted once, as the daemon's
    fake.end(500)
    fake.end(600)
    row = _sample(log, clock)
    assert row["app_cpu_s"] is None and row["app_footprint"] is None


def test_a_process_leaving_the_daemons_tree_for_ferb_app_is_counted_once(tmp_path: Path) -> None:
    fake = Fake()
    fake.spawn(200, DAEMON, start=4, cpu=1.0)                                       # a shell
    fake.spawn(300, 200, start=5, cpu=2.0, path="/Applications/FERB.app/Contents/MacOS/FERBNotch")                         # something inside FERB.app it ran
    db, clock, log = _log(tmp_path, fake)
    fake.run(300, 3.0)
    row = _sample(log, clock)
    assert row["child_cpu_s"] == pytest.approx(1.0) and row["app_cpu_s"] is None   # the daemon's while in its tree
    fake.end(200)
    fake.reaped += 1.0                                                              # the shell's own CPU
    fake.procs[300] = Process(300, 5, 3.5, 7 * MB, 7 * MB)                          # still running, now launchd's
    fake.kids.setdefault(1, []).append(300)
    row = _sample(log, clock)
    assert row["child_cpu_s"] == pytest.approx(0.0) and row["child_count"] == 0
    assert row["app_cpu_s"] == pytest.approx(0.5) and row["app_footprint"] == 7 * MB   # not its 3.5 s since it started


# ---------- retention ----------

def _put(db: Database, day: str, cpu: float = 1.0, at: str | None = None, window: float = 300, child: float = 0.0,
         app: float | None = None, footprint: int = 300 * MB, peak: int = 900 * MB, db_bytes: int = 2 * 10**9,
         work: dict[str, Any] | None = None) -> None:
    db.conn.execute("INSERT INTO resource_samples (at, day, window_s, cpu_s, footprint, peak_footprint, child_count, "
                    "child_footprint, child_cpu_s, app_cpu_s, app_footprint, db_bytes, work_json) "
                    "VALUES (?, ?, ?, ?, ?, ?, 1, 10, ?, ?, ?, ?, ?)",
                    (at or f"{day}T10:00:00+05:30", day, window, cpu, footprint, peak, child, app,
                     None if app is None else 50 * MB, db_bytes, json.dumps(work or {})))


def _days(db: Database) -> list[str]:
    return [r[0] for r in db.conn.execute("SELECT day FROM resource_samples ORDER BY day DESC")]


def test_samples_older_than_resource_days_are_purged(tmp_path: Path) -> None:
    db, today = _db(tmp_path), date(2026, 10, 23)
    for back in (0, 1, 2, 30, 31, 40):
        _put(db, (today - timedelta(days=back)).isoformat())
    assert resources.purge(db, today, 30) == 2
    assert _days(db) == ["2026-10-23", "2026-10-22", "2026-10-21", "2026-09-23"]
    assert resources.purge(db, today, 0) == 2                           # below 1 counts as 1 day
    assert _days(db) == ["2026-10-23", "2026-10-22"]


def test_resource_days_defaults_to_thirty_and_the_owners_value_stays(tmp_path: Path) -> None:
    config = Config(tmp_path / "home")
    config.ensure_privacy_defaults()
    assert config.get("retention", "resource_days") == 30
    config.set("retention", "resource_days", 3)
    config.ensure_privacy_defaults()
    assert config.get("retention", "resource_days") == 3


def test_the_hourly_retention_pass_purges_old_samples(home: Path) -> None:
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    try:
        today = d.daemon.clock.now().astimezone().date()

        async def seed() -> None:
            d.daemon.config.set("retention", "resource_days", 2)
            for back in (0, 2, 3, 9):
                _put(d.daemon.db, (today - timedelta(days=back)).isoformat())
        d.run(seed())
        result: dict[str, int] = {}
        d.call_soon(lambda: result.update(d.daemon._purge_records()))
        assert result["resources"] == 2

        async def left() -> list[str]:
            return _days(d.daemon.db)
        assert d.run(left()) == [today.isoformat(), (today - timedelta(days=2)).isoformat()]
    finally:
        d.stop()


def test_a_failing_resource_purge_never_stops_the_retention_pass(daemon: RunningDaemon,
                                                                  monkeypatch: pytest.MonkeyPatch) -> None:
    def broken(*args: Any) -> int:
        raise sqlite3.OperationalError("disk I/O error")
    monkeypatch.setattr(resources, "purge", broken)
    result = daemon.run(daemon.daemon._purge_pass())
    assert result["resources"] == 0 and "snapshots" in result           # the pass went on to its last step

    async def failed() -> Any:
        return daemon.daemon.db.conn.execute(
            "SELECT data_json FROM journal WHERE kind = 'retention.failed'").fetchone()[0]
    assert json.loads(daemon.run(failed())) == {"resources": True, "error": "OperationalError: disk I/O error"}


# ---------- the daemon's loop ----------

def _daemon(home: Path, monkeypatch: pytest.MonkeyPatch, fake: Fake) -> RunningDaemon:
    monkeypatch.setattr(resources, "MacReadings", lambda: fake)
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False, resources=True)
    d.run(asyncio.sleep(0.01))                                          # the loop is asleep until the first sample
    return d


def _read(d: RunningDaemon, sql: str) -> Any:
    async def read() -> Any:
        return d.daemon.db.conn.execute(sql).fetchone()[0]
    return d.run(read())


def _wait(d: RunningDaemon, sql: str, until: Callable[[Any], bool]) -> Any:
    deadline = time.monotonic() + 5
    while not until(value := _read(d, sql)) and time.monotonic() < deadline:
        time.sleep(0.02)
    return value


def _settled(d: RunningDaemon, sql: str) -> Any:
    """What the loop has done once it has had time to act on a clock change."""
    d.run(asyncio.sleep(0.1))
    return _read(d, sql)


def test_the_daemon_samples_every_five_minutes(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    assert resources.SAMPLE_SECONDS == 300                              # the owner's "every 5 minutes"
    d = _daemon(home, monkeypatch, Fake(os.getpid()))
    try:
        d.call_soon(d.daemon.clock.advance, 299)
        assert _settled(d, "SELECT COUNT(*) FROM resource_samples") == 0
        d.call_soon(d.daemon.clock.advance, 1)
        assert _wait(d, "SELECT COUNT(*) FROM resource_samples", lambda n: n == 1) == 1
        assert _read(d, "SELECT window_s FROM resource_samples") == pytest.approx(300)
        d.call_soon(d.daemon.clock.advance, 300)
        assert _wait(d, "SELECT COUNT(*) FROM resource_samples", lambda n: n == 2) == 2
    finally:
        d.stop()


def test_shutdown_writes_the_partial_window(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    fake = Fake(os.getpid())
    d = _daemon(home, monkeypatch, fake)
    d.call_soon(d.daemon.clock.advance, 120)
    fake.own = 4.0
    assert _settled(d, "SELECT COUNT(*) FROM resource_samples") == 0
    d.stop()
    db = Database(home / "ferb.db")
    try:
        [row] = _rows(db)
        assert row["window_s"] == pytest.approx(120) and row["cpu_s"] == pytest.approx(4.0)
    finally:
        db.close()


def test_a_failing_sample_is_journaled_and_the_next_one_still_runs(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    fake = Fake(os.getpid())
    d = _daemon(home, monkeypatch, fake)
    try:
        fake.fail = True
        d.call_soon(d.daemon.clock.advance, 300)
        assert _wait(d, "SELECT COUNT(*) FROM journal WHERE kind = 'resources.failed'", lambda n: n == 1) == 1
        assert json.loads(_read(d, "SELECT data_json FROM journal WHERE kind = 'resources.failed'")) == {
            "error": "OSError: can't read"}
        assert _read(d, "SELECT COUNT(*) FROM resource_samples") == 0
        fake.fail = False
        d.call_soon(d.daemon.clock.advance, 300)
        assert _wait(d, "SELECT COUNT(*) FROM resource_samples", lambda n: n == 1) == 1
        assert _read(d, "SELECT window_s FROM resource_samples") == pytest.approx(600)   # nothing lost
    finally:
        d.stop()


def _unjournaled(d: RunningDaemon) -> None:
    """Journaling `resources.failed` fails too (a broken database)."""
    journal = d.daemon.journal
    assert journal is not None
    real = journal.append

    def append(kind: str, data: Any = None, origin: Any = None) -> Any:
        if kind == "resources.failed":
            raise sqlite3.OperationalError("disk I/O error")
        return real(kind, data, origin)
    journal.append = append                                             # type: ignore[method-assign]


def test_a_failing_sample_at_shutdown_never_stops_shutdown(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    fake = Fake(os.getpid())
    d = _daemon(home, monkeypatch, fake)
    _unjournaled(d)
    fake.fail = True
    d.stop()                                                            # raises if shutdown did
    db = Database(home / "ferb.db")
    try:
        assert _rows(db) == []
        assert db.conn.execute("SELECT COUNT(*) FROM journal WHERE kind = 'daemon.stopped'").fetchone()[0] == 1
    finally:
        db.close()


def test_the_loop_goes_on_when_even_its_failure_cant_be_journaled(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    fake = Fake(os.getpid())
    d = _daemon(home, monkeypatch, fake)
    try:
        _unjournaled(d)
        fake.fail = True
        d.call_soon(d.daemon.clock.advance, 300)
        assert _settled(d, "SELECT COUNT(*) FROM resource_samples") == 0
        fake.fail = False
        d.call_soon(d.daemon.clock.advance, 300)
        assert _wait(d, "SELECT COUNT(*) FROM resource_samples", lambda n: n == 1) == 1
        assert _read(d, "SELECT window_s FROM resource_samples") == pytest.approx(600)
    finally:
        d.stop()


def test_the_perception_batchers_counters_reach_a_row(home: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    d = _daemon(home, monkeypatch, Fake(os.getpid()))
    try:
        async def flushed() -> None:
            batcher = d.daemon.perception.batches                     # the daemon's real batcher
            assert batcher.wakes >= 1                                  # its first look, after start-up's baseline
            batcher.batches_by_reason["age"] = batcher.batches_by_reason.get("age", 0) + 2   # as two age batches
            batcher.notify(None)                                       # and a real wake
        d.run(flushed())
        d.run(asyncio.sleep(0.05))
        d.call_soon(d.daemon.clock.advance, 300)
        assert _wait(d, "SELECT COUNT(*) FROM resource_samples", lambda n: n == 1) == 1
        perception = json.loads(_read(d, "SELECT work_json FROM resource_samples"))["perception"]
        assert perception["batches"] == {"age": 2} and perception["wakes"] >= 2
    finally:
        d.stop()


def test_a_log_that_cant_start_is_journaled_and_the_daemon_runs_without_it(home: Path,
                                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    fake = Fake(os.getpid())
    fake.fail = True
    d = _daemon(home, monkeypatch, fake)
    try:
        assert d.daemon.resource_log is None and d.daemon._resources_task is None
        assert json.loads(_read(d, "SELECT data_json FROM journal WHERE kind = 'resources.failed'")) == {
            "error": "OSError: can't read"}
    finally:
        d.stop()
    db = Database(home / "ferb.db")
    try:
        assert _rows(db) == []
    finally:
        db.close()


def test_test_daemons_never_sample(daemon: RunningDaemon) -> None:
    assert daemon.daemon.resource_log is None and daemon.daemon._resources_task is None


# ---------- ferb stats ----------

def _stats(home: Path, days: int = 7) -> str:
    from ferb.cli.admin import stats
    out = io.StringIO()
    assert stats(home, out, days=days) == 0
    return out.getvalue()


def test_stats_shows_each_days_resources_and_the_hottest_windows(tmp_path: Path) -> None:
    db = _db(tmp_path)
    today = datetime.now().astimezone().date()
    day, before = today.isoformat(), (today - timedelta(days=1)).isoformat()
    busy = {"events": {"accessibility": 1200, "window": 40, "file": 3, "clipboard": 1},
            "perception": {"wakes": 40, "batches": {"age": 2, "tokens": 1}}, "jev": {"asked": 5, "not_sampled": 20},
            "models": {"model.turn": 2, "worker": 1}, "tool_calls": 14, "tasks_running": 1,
            "top": [["codex", 30.1], ["zsh", 2.0]]}
    _put(db, day, cpu=3.0, at=f"{day}T10:00:00+05:30", footprint=300 * MB, peak=500 * MB, db_bytes=900 * MB)
    _put(db, day, cpu=150.0, at=f"{day}T10:05:00+05:30", child=9.0, app=1.0, footprint=500 * MB, peak=1200 * MB,
         db_bytes=950 * MB, work=busy)
    _put(db, day, cpu=0.0, at=f"{day}T10:06:00+05:30", window=60, footprint=400 * MB, peak=1200 * MB,
         db_bytes=1_200 * MB)
    _put(db, before, cpu=30.0, at=f"{before}T23:55:00+05:30", child=3.0, db_bytes=800 * MB)
    _put(db, "2000-01-01", cpu=999.0)                                    # outside --days
    db.close()
    text = _stats(tmp_path)
    lines = text[text.index("Resources"):].splitlines()
    assert lines[0] == ("Resources (FERB's own use over the last 7 local day(s), today included; sampled every 5 "
                        "minutes; CPU as a share of one core):")
    assert lines[1] == (f"  {before}: 0.1 h; daemon CPU avg 10.0%, peak 10.0% at 23:55; memory avg 300.0 MB, "
                        "peak 900.0 MB; other processes CPU avg 1.0%; FERB.app not running; database 800.0 MB")
    assert lines[2] == (f"  {day}: 0.2 h; daemon CPU avg 23.2%, peak 50.0% at 10:05; memory avg 400.0 MB, "
                        "peak 1.2 GB; other processes CPU avg 1.4%; FERB.app CPU avg 0.3%; database 1.2 GB")
    assert lines[3] == "  Hottest windows (CPU of the daemon, its descendants and FERB.app):"
    assert lines[4] == (f"    {day} 10:05, 5.0 min: 160.0 s CPU (daemon 150.0, other processes 9.0, FERB.app 1.0); "
                        "events 1244 (accessibility 1200, window 40, file 3); perception wakes 40, batches 3 "
                        "(age 2, tokens 1); Jev asked 5, not sampled 20; model runs 3 (model.turn 2, worker 1); "
                        "tool calls 14; tasks running 1; top processes codex 30.1 s, zsh 2.0 s")
    assert lines[5].startswith(f"    {before} 23:55, 5.0 min: 33.0 s CPU (daemon 30.0, other processes 3.0)")
    assert lines[6].startswith(f"    {day} 10:00, 5.0 min: 3.0 s CPU")
    assert len(lines) == 8 and lines[7].startswith(f"    {day} 10:06, 1.0 min: 0.0 s CPU")
    assert "2000-01-01" not in text


def test_the_days_peak_is_its_busiest_window_never_a_short_one(tmp_path: Path) -> None:
    db = _db(tmp_path)
    day = datetime.now().astimezone().date().isoformat()
    _put(db, day, cpu=150.0, at=f"{day}T10:05:00+05:30")                # 50% for 5 minutes
    _put(db, day, cpu=1.8, at=f"{day}T10:07:00+05:30", window=2)        # 90% for the 2 s before a stop
    db.close()
    assert "peak 50.0% at 10:05;" in _stats(tmp_path)


def test_stats_days_1_shows_only_today(tmp_path: Path) -> None:
    db = _db(tmp_path)
    today = datetime.now().astimezone().date()
    _put(db, today.isoformat())
    _put(db, (today - timedelta(days=1)).isoformat())
    db.close()
    text = _stats(tmp_path, days=1)
    days = [line.split(":")[0].strip() for line in text[text.index("Resources"):].splitlines()[1:]
            if line.startswith("  2")]
    assert days == [today.isoformat()]


def test_stats_says_when_there_are_no_resource_samples(tmp_path: Path) -> None:
    db = _db(tmp_path)
    db.close()
    assert "Resources: no samples yet (the daemon writes one every 5 minutes).\n" in _stats(tmp_path)
    db = Database(tmp_path / "ferb.db")
    _put(db, "2000-01-01")
    db.close()
    assert "Resources: no samples in the last 7 local day(s), today included.\n" in _stats(tmp_path)


def test_stats_on_a_store_from_before_the_resource_log_says_when_it_starts(tmp_path: Path) -> None:
    """The checkout's `ferb stats` against the store of a daemon installed before L30: no table, so no daemon writes
    samples until the next update installs one that does."""
    db = _db(tmp_path)
    db.conn.execute("DROP TABLE resource_samples")
    db.close()
    text = _stats(tmp_path)
    assert "Resources: no samples yet; the resource log starts after the next `ferb setup --update`.\n" in text, text
