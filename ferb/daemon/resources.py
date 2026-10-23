"""Own FERB's resource log (L30, owner 2026-10-23): every SAMPLE_SECONDS the daemon writes one row of numbers, kept
`[retention] resource_days` (30) and shown per day by `ferb stats`. Must not record content: no arguments, paths, text
or titles; a process is named only by its executable's basename, through the daemon's redactor (rule 14).

A row: the daemon's CPU (getrusage) and physical footprint (libproc); its descendants' (walked from its pid; a process
FERB opens through Launch Services belongs to launchd and isn't one); FERB.app's processes outside that tree; the
database's size; and the window's work, each count a seq or rowid range from the last value seen, never a table scan.
The first window starts at the log's creation, during the daemon's start-up."""

import ctypes
import json
import os
import resource
from dataclasses import dataclass, field
from datetime import date, timedelta
from pathlib import Path
from typing import Any, Callable, NamedTuple, Protocol

from ferb.store import retention
from ferb.store.db import store_size

SAMPLE_SECONDS = 300                 # owner 2026-10-23: "every 5 minutes"
DEFAULT_DAYS = 30                    # `[retention] resource_days`: the owner's "30 days"
TOP = 3                              # the window's busiest descendants named in a row
APP_BUNDLE = "FERB.app"              # FERB.app: a process whose executable lies inside a bundle named so
RUSAGE_INFO_V4 = 4                   # <sys/resource.h>: the first flavor with ri_lifetime_max_phys_footprint

# Where each window's counts start: the highest seq or rowid at the last sample. NOT INDEXED keeps the trail's count a
# range of its seq (the rowid) whatever statistics say (with ANALYZE, SQLite picks a skip-scan of trail_source).
CURSORS = {"trail": "SELECT COALESCE(MAX(seq), 0) FROM trail",
           "decisions": "SELECT COALESCE(MAX(seq), 0) FROM decisions",
           "spans": "SELECT COALESCE(MAX(rowid), 0) FROM spans"}
EVENTS = "SELECT source, COUNT(*) FROM trail NOT INDEXED WHERE seq > ? AND seq <= ? GROUP BY source"
JEV = "SELECT error IS 'not sampled', COUNT(*) FROM decisions WHERE seq > ? AND seq <= ? GROUP BY 1"
MODELS = ("SELECT kind, COUNT(*) FROM spans WHERE rowid > ? AND rowid <= ? "
          "AND kind IN ('model.turn', 'worker', 'model.learning', 'tool.call') GROUP BY kind")
RUNNING = "SELECT COUNT(*) FROM tasks WHERE status = 'running'"
COLUMNS = ("at", "day", "window_s", "cpu_s", "footprint", "peak_footprint", "child_count", "child_footprint",
           "child_cpu_s", "app_cpu_s", "app_footprint", "db_bytes", "work_json")


class Process(NamedTuple):
    pid: int
    start: int              # when it started (mach absolute time): with the pid, which process this is
    cpu_s: float            # user + system CPU since it started
    footprint: int          # physical footprint in bytes (Activity Monitor's Memory column)
    peak_footprint: int     # its lifetime maximum


class Readings(Protocol):
    """What a sample reads from the machine; the daemon uses MacReadings, tests a fake."""

    def own_cpu(self) -> float: ...                     # this process's user + system CPU seconds
    def children_cpu(self) -> float: ...                # its children that ended and were waited for (and theirs)
    def process(self, pid: int) -> Process | None: ...  # None: gone or not readable
    def children(self, pid: int) -> list[int]: ...
    def pids(self) -> list[int]: ...
    def path(self, pid: int) -> str | None: ...         # its executable


def mach_seconds(units: int, numer: int, denom: int) -> float:
    """Mach absolute time units in seconds, by the timebase (on Apple silicon one unit is 125/3 ns, not 1 ns)."""
    return units * numer / denom / 1e9


class _RusageInfoV4(ctypes.Structure):
    _fields_ = [("ri_uuid", ctypes.c_uint8 * 16)] + [(name, ctypes.c_uint64) for name in (
        "ri_user_time ri_system_time ri_pkg_idle_wkups ri_interrupt_wkups ri_pageins ri_wired_size ri_resident_size "
        "ri_phys_footprint ri_proc_start_abstime ri_proc_exit_abstime ri_child_user_time ri_child_system_time "
        "ri_child_pkg_idle_wkups ri_child_interrupt_wkups ri_child_pageins ri_child_elapsed_abstime "
        "ri_diskio_bytesread ri_diskio_byteswritten ri_cpu_time_qos_default ri_cpu_time_qos_maintenance "
        "ri_cpu_time_qos_background ri_cpu_time_qos_utility ri_cpu_time_qos_legacy ri_cpu_time_qos_user_initiated "
        "ri_cpu_time_qos_user_interactive ri_billed_system_time ri_serviced_system_time ri_logical_writes "
        "ri_lifetime_max_phys_footprint ri_instructions ri_cycles ri_billed_energy ri_serviced_energy "
        "ri_interval_max_phys_footprint ri_runnable_time").split()]


class _Timebase(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]


class MacReadings:
    """The kernel's numbers: getrusage, and libproc through ctypes (as sense/front_window.py and act/shell/local.py
    read it). Raises OSError when libproc can't be loaded."""

    def __init__(self) -> None:
        lib = ctypes.CDLL("/usr/lib/libproc.dylib")
        for name, args in (("proc_pid_rusage", [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]),
                           ("proc_listchildpids", [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]),
                           ("proc_listallpids", [ctypes.c_void_p, ctypes.c_int]),
                           ("proc_pidpath", [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32])):
            getattr(lib, name).argtypes, getattr(lib, name).restype = args, ctypes.c_int
        self._lib = lib
        timebase = _Timebase()
        ctypes.CDLL("/usr/lib/libSystem.B.dylib").mach_timebase_info(ctypes.byref(timebase))
        self._timebase = (int(timebase.numer), int(timebase.denom) or 1)

    def own_cpu(self) -> float:
        used = resource.getrusage(resource.RUSAGE_SELF)
        return used.ru_utime + used.ru_stime

    def children_cpu(self) -> float:
        used = resource.getrusage(resource.RUSAGE_CHILDREN)
        return used.ru_utime + used.ru_stime

    def process(self, pid: int) -> Process | None:
        info = _RusageInfoV4()
        if pid <= 0 or self._lib.proc_pid_rusage(pid, RUSAGE_INFO_V4, ctypes.byref(info)) != 0:
            return None
        return Process(pid, int(info.ri_proc_start_abstime),
                       mach_seconds(info.ri_user_time + info.ri_system_time, *self._timebase),
                       int(info.ri_phys_footprint), int(info.ri_lifetime_max_phys_footprint))

    def children(self, pid: int) -> list[int]:
        return self._list(self._lib.proc_listchildpids, pid)

    def pids(self) -> list[int]:
        return self._list(self._lib.proc_listallpids)

    def path(self, pid: int) -> str | None:
        buf = ctypes.create_string_buffer(4096)              # PROC_PIDPATHINFO_MAXSIZE
        if pid <= 0 or self._lib.proc_pidpath(pid, buf, len(buf)) <= 0:
            return None
        return os.fsdecode(buf.value)

    @staticmethod
    def _list(call: Any, *args: int) -> list[int]:
        """A libproc pid list (its answer is a count of pids); a buffer it fills is too small, so it is asked again."""
        size = 256
        while True:
            buf = (ctypes.c_int * size)()
            n = call(*args, buf, ctypes.sizeof(buf))
            if n < 0:
                return []
            if n < size:
                return [pid for pid in buf[:n] if pid > 0]
            size *= 2


def in_app_bundle(path: str) -> bool:
    """Whether an executable lies inside a FERB.app bundle (the packaged app, its helpers, its Python)."""
    return any(part.endswith(APP_BUNDLE) for part in Path(path).parts[:-1])


Key = tuple[int, int]                                         # (pid, start): one process, never a reused pid


@dataclass(frozen=True)
class _Seen:
    cpu_s: float            # its CPU at the last sample: what has been counted for it
    top: Key                # the daemon's direct child it descends from (itself, for a direct child)


@dataclass
class _Mark:
    """What one sample leaves for the next."""
    mono: float
    own_cpu: float
    reaped_cpu: float                                         # children_cpu() at the sample
    cursors: dict[str, int]
    perception: tuple[int, dict[str, int]]
    tracked: dict[Key, _Seen] = field(default_factory=dict)  # the daemon's descendants alive at the sample
    under: dict[Key, float] = field(default_factory=dict)    # CPU counted for ended descendants, by direct child
    credit: float = 0.0           # CPU counted for ended processes that children_cpu() hasn't shown yet
    app: dict[Key, float] = field(default_factory=dict)      # FERB.app's processes' CPU at the sample


class ResourceLog:
    """Creating one takes the baseline; `sample` writes the window since the last sample (or the creation)."""

    def __init__(self, db: Any, clock: Any, readings: Readings, redact: Callable[[str], str],
                 perception: Callable[[], tuple[int, dict[str, int]]] = lambda: (0, {}),
                 pid: int | None = None) -> None:
        self.db, self.clock, self.readings, self.redact, self.perception = db, clock, readings, redact, perception
        self.pid = os.getpid() if pid is None else pid
        tree, walked = self._walk()
        app = self._app(walked | {self.pid})
        self._mark = _Mark(clock.monotonic(), readings.own_cpu(), readings.children_cpu(),
                           {name: int(db.conn.execute(sql).fetchone()[0]) for name, sql in CURSORS.items()},
                           self._perception(),
                           {key: _Seen(proc.cpu_s, top) for key, (proc, top) in tree.items()},
                           app={key: proc.cpu_s for key, proc in app.items()})

    def sample(self) -> dict[str, Any]:
        """One window's row, written and returned. Nothing is kept from a sample that fails: the next covers both."""
        mark, now, mono = self._mark, self.clock.now(), self.clock.monotonic()
        own, reaped = self.readings.own_cpu(), self.readings.children_cpu()   # before the walk: see _descendants
        me = self.readings.process(self.pid)
        tree, walked = self._walk()
        live, children_cpu, tracked, under, credit = self._descendants(tree, reaped, mark)
        app = self._app(walked | {self.pid})
        cursors, work = self._work(mark)
        busiest = sorted(((cpu, key) for key, cpu in live.items() if cpu > 0), reverse=True)[:TOP]
        work["top"] = [[self.redact(os.path.basename(self.readings.path(key[0]) or "") or "unknown"), round(cpu, 3)]
                       for cpu, key in busiest]
        counts = self._perception()
        work["perception"] = {"wakes": max(0, counts[0] - mark.perception[0]),
                              "batches": {reason: n - mark.perception[1].get(reason, 0)
                                          for reason, n in sorted(counts[1].items())
                                          if n > mark.perception[1].get(reason, 0)}}
        row = {"at": now.isoformat(), "day": now.astimezone().date().isoformat(),
               "window_s": round(mono - mark.mono, 3), "cpu_s": round(max(0.0, own - mark.own_cpu), 3),
               "footprint": me.footprint if me else None, "peak_footprint": me.peak_footprint if me else None,
               "child_count": len(tree), "child_footprint": sum(proc.footprint for proc, _ in tree.values()),
               "child_cpu_s": round(children_cpu, 3),
               "app_cpu_s": round(sum(max(0.0, proc.cpu_s - self._counted(key, mark)) for key, proc in app.items()),
                                  3) if app else None,
               "app_footprint": sum(proc.footprint for proc in app.values()) if app else None,
               "db_bytes": store_size(Path(self.db.path)),
               "work_json": json.dumps({key: work[key] for key in ("events", "perception", "jev", "models",
                                                                   "tool_calls", "tasks_running", "top")})}
        self.db.conn.execute(f"INSERT INTO resource_samples ({', '.join(COLUMNS)}) "
                             f"VALUES ({', '.join('?' * len(COLUMNS))})", [row[column] for column in COLUMNS])
        self._mark = _Mark(mono, own, reaped, cursors, counts, tracked, under, credit,
                           {key: proc.cpu_s for key, proc in app.items()})
        return row

    def _walk(self) -> tuple[dict[Key, tuple[Process, Key]], set[int]]:
        """The daemon's descendants that can be read, each with the direct child it is under, and every pid walked.
        One that can't be read is skipped; its children are still walked, as if they were direct children."""
        found: dict[Key, tuple[Process, Key]] = {}
        walked: set[int] = set()
        queue: list[tuple[int, Key | None]] = [(pid, None) for pid in self.readings.children(self.pid)]
        while queue:
            pid, top = queue.pop()
            if pid in walked or pid == self.pid:
                continue
            walked.add(pid)
            proc = self.readings.process(pid)
            if proc is not None:
                key = (pid, proc.start)
                top = top or key
                found[key] = (proc, top)
            queue += [(child, top) for child in self.readings.children(pid)]
        return found, walked

    def _descendants(self, tree: dict[Key, tuple[Process, Key]], reaped: float, mark: _Mark) -> tuple[
            dict[Key, float], float, dict[Key, _Seen], dict[Key, float], float]:
        """The descendants' CPU in the window: each live one's since the last sample (since it started, when first
        seen), plus what children_cpu() grew by, less what was already counted for the processes behind that growth,
        never below 0. A direct child that ended adds what was counted for it and for the descendants that ended
        under it while it waited for them; one still running after its parent ended (now launchd's) adds nothing.
        What children_cpu() doesn't show yet (a child not waited for at the read) is taken off a later window."""
        live = {key: max(0.0, proc.cpu_s - mark.tracked[key].cpu_s) if key in mark.tracked else proc.cpu_s
                for key, (proc, _) in tree.items()}
        tracked = {key: _Seen(proc.cpu_s, top) for key, (proc, top) in tree.items()}
        under, credit = dict(mark.under), mark.credit
        gone = [key for key in mark.tracked if key not in tree]
        for key in sorted(gone, key=lambda key: mark.tracked[key].top == key):   # deeper ones first
            seen = mark.tracked[key]
            if seen.top == key:
                credit += seen.cpu_s + under.pop(key, 0.0)
                continue
            still = self.readings.process(key[0])
            if (still is None or still.start != key[1]) and seen.top in mark.tracked:
                under[seen.top] = under.get(seen.top, 0.0) + seen.cpu_s
        finished = max(0.0, reaped - mark.reaped_cpu)
        return (live, sum(live.values()) + max(0.0, finished - credit), tracked,
                {key: cpu for key, cpu in under.items() if key in tracked}, max(0.0, credit - finished))

    @staticmethod
    def _counted(key: Key, mark: _Mark) -> float:
        """A FERB.app process's CPU already counted: as FERB.app's, or as the daemon's descendant until it left the
        tree (its parent ended and launchd took it); 0 for one first seen, so its CPU since it started counts."""
        return mark.app[key] if key in mark.app else mark.tracked[key].cpu_s if key in mark.tracked else 0.0

    def _app(self, tree: set[int]) -> dict[Key, Process]:
        """FERB.app's processes outside the daemon and its descendants (those are counted as the daemon's)."""
        app: dict[Key, Process] = {}
        for pid in self.readings.pids():
            if pid in tree or not in_app_bundle(self.readings.path(pid) or ""):
                continue
            proc = self.readings.process(pid)
            if proc is not None:
                app[(pid, proc.start)] = proc
        return app

    def _work(self, mark: _Mark) -> tuple[dict[str, int], dict[str, Any]]:
        """The window's counts, each a range above the last sample's cursor (rows deleted below it change nothing)."""
        c = self.db.conn
        cursors = {name: max(mark.cursors[name], int(c.execute(sql).fetchone()[0])) for name, sql in CURSORS.items()}

        def counted(sql: str, name: str) -> dict[Any, int]:
            return {r[0]: int(r[1]) for r in c.execute(sql, (mark.cursors[name], cursors[name]))}
        jev, models = counted(JEV, "decisions"), counted(MODELS, "spans")
        return cursors, {"events": counted(EVENTS, "trail"),
                         "jev": {"asked": jev.get(0, 0), "not_sampled": jev.get(1, 0)},
                         "models": {kind: n for kind, n in models.items() if kind != "tool.call"},
                         "tool_calls": models.get("tool.call", 0),
                         "tasks_running": int(c.execute(RUNNING).fetchone()[0])}

    def _perception(self) -> tuple[int, dict[str, int]]:
        wakes, batches = self.perception()
        return wakes, dict(batches)


def purge(db: Any, today: date, days: Any) -> int:
    """Samples from before the last `days` days (`[retention] resource_days`, at least 1 like every retention
    setting), by local day; the hourly retention pass runs it."""
    cutoff = (today - timedelta(days=retention.days(days))).isoformat()
    return int(db.conn.execute("DELETE FROM resource_samples WHERE day < ?", (cutoff,)).rowcount)
