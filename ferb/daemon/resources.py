"""Own FERB's resource log (L30, owner 2026-10-23): every SAMPLE_SECONDS the daemon writes one row of numbers, kept
`[retention] resource_days` (30) and shown per day by `ferb stats`. Must not record content: no arguments, paths, text
or titles; a process is named only by its executable's basename, through the daemon's redactor (rule 14).

A row: the daemon's CPU (getrusage) and physical footprint (libproc); its descendants' (walked from its pid; a process
FERB opens through Launch Services belongs to launchd and isn't one); FERB.app's processes outside that tree; the
database's size; and the window's work, each count a seq or rowid range from the last sample's maximum, never a table
scan. The first window starts at the log's creation, during the daemon's start-up."""

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
    children_cpu_s: float = 0.0   # its children that ended and that it waited for, theirs included


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


def mach_timebase(lib: Any = None) -> tuple[int, int]:
    """The mach timebase (numer, denom). OSError when it can't be read: CPU must never silently read as 0."""
    lib = lib if lib is not None else ctypes.CDLL("/usr/lib/libSystem.B.dylib")
    call = lib.mach_timebase_info
    call.argtypes, call.restype = [ctypes.POINTER(_Timebase)], ctypes.c_int
    timebase = _Timebase()
    result = call(ctypes.byref(timebase))
    if result != 0 or not timebase.numer or not timebase.denom:
        raise OSError(f"mach_timebase_info: {result} ({timebase.numer}/{timebase.denom})")
    return int(timebase.numer), int(timebase.denom)


class MacReadings:
    """The kernel's numbers: getrusage, and libproc through ctypes (as sense/front_window.py and act/shell/local.py
    read it). Raises OSError when libproc or the mach timebase can't be read."""

    def __init__(self) -> None:
        lib = ctypes.CDLL("/usr/lib/libproc.dylib")
        for name, args in (("proc_pid_rusage", [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]),
                           ("proc_listchildpids", [ctypes.c_int, ctypes.c_void_p, ctypes.c_int]),
                           ("proc_listallpids", [ctypes.c_void_p, ctypes.c_int]),
                           ("proc_pidpath", [ctypes.c_int, ctypes.c_void_p, ctypes.c_uint32])):
            getattr(lib, name).argtypes, getattr(lib, name).restype = args, ctypes.c_int
        self._lib = lib
        self._timebase = mach_timebase()

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
                       int(info.ri_phys_footprint), int(info.ri_lifetime_max_phys_footprint),
                       mach_seconds(info.ri_child_user_time + info.ri_child_system_time, *self._timebase))

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


Owed = dict[Key | None, float]


@dataclass(frozen=True)
class _Seen:
    cpu_s: float            # its own CPU at the last sample: what has been counted of it
    children_cpu_s: float   # its waited-for children's CPU at the last sample: counted too
    parent: Key | None      # the nearest readable ancestor it was under (None: the daemon)
    depth: int              # how far below the daemon (ended processes are settled deepest first)


@dataclass
class _Mark:
    """What one sample leaves for the next."""
    mono: float
    own_cpu: float
    reaped_cpu: float                                         # children_cpu() at the sample
    cursors: dict[str, int]
    perception: tuple[int, dict[str, int]]
    tracked: dict[Key, _Seen] = field(default_factory=dict)  # the daemon's descendants alive at the sample
    # CPU already counted for ended processes that their parent's children counter (None: the daemon's
    # children_cpu()) hasn't shown yet: taken off that counter's growth in a later window
    owed: Owed = field(default_factory=dict)
    app: dict[Key, float] = field(default_factory=dict)      # FERB.app's processes' CPU at the sample
    preview: dict[str, float] = field(default_factory=dict)  # the live preview's counters at the sample


class ResourceLog:
    """Creating one takes the baseline; `sample` writes the window since the last sample (or the creation)."""

    def __init__(self, db: Any, clock: Any, readings: Readings, redact: Callable[[str], str],
                 perception: Callable[[], tuple[int, dict[str, int]]] = lambda: (0, {}),
                 pid: int | None = None, preview: Callable[[], dict[str, float]] = lambda: {}) -> None:
        self.db, self.clock, self.readings, self.redact, self.perception = db, clock, readings, redact, perception
        self.preview = preview                  # the live preview's cumulative counters (APP-405)
        self.pid = os.getpid() if pid is None else pid
        tree, walked = self._walk()
        app = self._app(walked | {self.pid})
        self._mark = _Mark(clock.monotonic(), readings.own_cpu(), readings.children_cpu(),
                           {name: int(db.conn.execute(sql).fetchone()[0]) for name, sql in CURSORS.items()},
                           self._perception(),
                           {key: _Seen(proc.cpu_s, proc.children_cpu_s, parent, depth)
                            for key, (proc, parent, depth) in tree.items()},
                           app={key: proc.cpu_s for key, proc in app.items()}, preview=self._preview())

    def sample(self) -> dict[str, Any]:
        """One window's row, written and returned. Nothing is kept from a sample that fails: the next covers both."""
        mark, now, mono = self._mark, self.clock.now(), self.clock.monotonic()
        own, reaped = self.readings.own_cpu(), self.readings.children_cpu()   # before the walk: see _descendants
        me = self.readings.process(self.pid)
        tree, walked = self._walk()
        live, children_cpu, tracked, owed = self._descendants(tree, reaped, mark)
        app = self._app(walked | {self.pid})
        cursors, work = self._work(mark)
        busiest = sorted(((cpu, key) for key, cpu in live.items() if cpu > 0), reverse=True)[:TOP]
        work["top"] = [[self.redact(os.path.basename(self.readings.path(key[0]) or "") or "unknown"), round(cpu, 3)]
                       for cpu, key in busiest]
        counts = self._perception()
        previews = self._preview()
        # the live preview's work in the window (APP-405: measured before any frame-rate cap is decided)
        # (`max_*`: the window's peak, read as it is; DMN-406)
        work["preview"] = {key: value if key.startswith("max_") else round(value - mark.preview.get(key, 0), 1)
                           if isinstance(value, float) else value - int(mark.preview.get(key, 0))
                           for key, value in previews.items()}
        work["perception"] = {"wakes": max(0, counts[0] - mark.perception[0]),
                              "batches": {reason: n - mark.perception[1].get(reason, 0)
                                          for reason, n in sorted(counts[1].items())
                                          if n > mark.perception[1].get(reason, 0)}}
        row = {"at": now.isoformat(), "day": now.astimezone().date().isoformat(),
               "window_s": round(mono - mark.mono, 3), "cpu_s": round(max(0.0, own - mark.own_cpu), 3),
               "footprint": me.footprint if me else None, "peak_footprint": me.peak_footprint if me else None,
               "child_count": len(tree), "child_footprint": sum(proc.footprint for proc, _, _ in tree.values()),
               "child_cpu_s": round(children_cpu, 3),
               "app_cpu_s": round(sum(max(0.0, proc.cpu_s - self._counted(key, mark)) for key, proc in app.items()),
                                  3) if app else None,
               "app_footprint": sum(proc.footprint for proc in app.values()) if app else None,
               "db_bytes": store_size(Path(self.db.path)),
               "work_json": json.dumps({key: work[key] for key in ("events", "perception", "jev", "models",
                                                                   "tool_calls", "tasks_running", "top", "preview")})}
        self.db.conn.execute(f"INSERT INTO resource_samples ({', '.join(COLUMNS)}) "
                             f"VALUES ({', '.join('?' * len(COLUMNS))})", [row[column] for column in COLUMNS])
        self._mark = _Mark(mono, own, reaped, cursors, counts, tracked, owed,
                           {key: proc.cpu_s for key, proc in app.items()}, previews)
        return row

    def _walk(self) -> tuple[dict[Key, tuple[Process, Key | None, int]], set[int]]:
        """The daemon's descendants that can be read, each with the nearest readable ancestor it is under (None: the
        daemon) and its depth, and every pid walked. One that can't be read is skipped; its children are still
        walked."""
        found: dict[Key, tuple[Process, Key | None, int]] = {}
        walked: set[int] = set()
        queue: list[tuple[int, Key | None, int]] = [(pid, None, 1) for pid in self.readings.children(self.pid)]
        while queue:
            pid, parent, depth = queue.pop()
            if pid in walked or pid == self.pid:
                continue
            walked.add(pid)
            proc = self.readings.process(pid)
            under = parent
            if proc is not None:
                under = (pid, proc.start)
                found[under] = (proc, parent, depth)
            queue += [(child, under, depth + 1) for child in self.readings.children(pid)]
        return found, walked

    def _descendants(self, tree: dict[Key, tuple[Process, Key | None, int]], reaped: float, mark: _Mark) -> tuple[
            dict[Key, float], float, dict[Key, _Seen], Owed]:
        """The descendants' CPU in the window. Each live one adds its own CPU since the last sample (since it started,
        when first seen) and what its waited-for children's counter grew by; the daemon adds what children_cpu()
        grew by. From each counter's growth comes off what was already counted for the processes behind it: an ended
        process owes its parent (its nearest readable ancestor, or the daemon) the CPU counted of it and of its own
        waited-for children, plus what was still owed to it; never below 0, and what a counter hasn't shown yet is
        taken off a later window. One still running outside the tree (its parent ended; launchd's now) owes nothing:
        no counter of FERB's will show it. So a command a long-lived child runs counts in the window it ends."""
        owed = dict(mark.owed)
        for key in sorted((key for key in mark.tracked if key not in tree), key=lambda key: -mark.tracked[key].depth):
            seen = mark.tracked[key]                                  # deepest first: a parent that ended passes up
            still = self.readings.process(key[0])
            if still is not None and still.start == key[1]:
                owed.pop(key, None)
                continue
            if seen.parent is None or seen.parent in mark.tracked:
                owed[seen.parent] = owed.get(seen.parent, 0.0) + seen.cpu_s + seen.children_cpu_s + owed.pop(key, 0.0)
        live: dict[Key, float] = {}
        for key, (proc, _, _) in tree.items():
            before = mark.tracked.get(key)
            own = proc.cpu_s - (before.cpu_s if before else 0.0)
            grown = proc.children_cpu_s - (before.children_cpu_s if before else 0.0)
            live[key] = max(0.0, own) + _settle(owed, key, grown)
        finished = _settle(owed, None, reaped - mark.reaped_cpu)
        tracked = {key: _Seen(proc.cpu_s, proc.children_cpu_s, parent, depth)
                   for key, (proc, parent, depth) in tree.items()}
        return (live, sum(live.values()) + finished, tracked,
                {key: cpu for key, cpu in owed.items() if key is None or key in tracked})

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
        """The window's counts, each the range from the last sample's maximum to the current one: rows deleted below it
        change nothing. The cursor follows the table, so a `spans` rowid SQLite hands out again (no AUTOINCREMENT,
        after retention deleted the newest spans) goes uncounted at most in the window it comes back."""
        c = self.db.conn
        cursors = {name: int(c.execute(sql).fetchone()[0]) for name, sql in CURSORS.items()}

        def counted(sql: str, name: str) -> dict[Any, int]:
            return {r[0]: int(r[1]) for r in c.execute(sql, (mark.cursors[name], cursors[name]))}
        jev, models = counted(JEV, "decisions"), counted(MODELS, "spans")
        return cursors, {"events": counted(EVENTS, "trail"),
                         "jev": {"asked": jev.get(0, 0), "not_sampled": jev.get(1, 0)},
                         "models": {kind: n for kind, n in models.items() if kind != "tool.call"},
                         "tool_calls": models.get("tool.call", 0),
                         "tasks_running": int(c.execute(RUNNING).fetchone()[0])}

    def _preview(self) -> dict[str, float]:
        """The live preview's cumulative counters (requests, frames, capture_ms, cached, busy); {} when unreadable."""
        try:
            return dict(self.preview() or {})
        except Exception:
            return {}

    def _perception(self) -> tuple[int, dict[str, int]]:
        wakes, batches = self.perception()
        return wakes, dict(batches)


def _settle(owed: Owed, parent: Key | None, grown: float) -> float:
    """A children counter's growth less what is owed against it, never below 0; what it doesn't cover stays owed."""
    pending, grown = owed.pop(parent, 0.0), max(0.0, grown)
    if pending > grown:
        owed[parent] = pending - grown
    return max(0.0, grown - pending)


def purge(db: Any, today: date, days: Any) -> int:
    """Samples from before the last `days` days (`[retention] resource_days`, at least 1 like every retention
    setting), by local day; the hourly retention pass runs it."""
    cutoff = (today - timedelta(days=retention.days(days))).isoformat()
    return int(db.conn.execute("DELETE FROM resource_samples WHERE day < ?", (cutoff,)).rowcount)
