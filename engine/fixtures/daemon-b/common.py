"""Shared by the group-B recorders (rust/fixtures/daemon-b/*.py): rust/fixtures/daemon/make.py's harness, imported as
a module (its stubs, FakeClock, SequentialIds, counters, raw-socket connections and masks), under a temporary root of
the recorder's own so recorders never share a folder. Adds what group B needs on top: Daemon keyword arguments per
scenario (Full Disk Access granted, invented browser folders), the journal rows each scenario wrote and the events it
published, and the files a scenario starts from (so a Rust test can rebuild the same home).

Use:   from common import H, BScenario, write      (after `load("<tag>")`)
"""

import importlib.util
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
H: Any = None


def load(tag: str) -> Any:
    """Import the daemon harness with its root under $TMPDIR/fxb-<tag> (fixed per recorder: FERB hashes real paths
    into a few records, so the folder must not change run to run)."""
    global H
    base = Path(tempfile.gettempdir()) / f"fxb-{tag}"
    base.mkdir(exist_ok=True)
    tempfile.tempdir = str(base)
    spec = importlib.util.spec_from_file_location("daemon_make", HERE.parent / "daemon" / "make.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["daemon_make"] = module
    spec.loader.exec_module(module)
    H = module
    _patch_new_host(module)
    return module


EXTRA: dict[str, Any] = {}          # Daemon(...) keyword arguments for the next host (e.g. data_access)
BEFORE_START: list[Any] = []        # fn(daemon) run before the daemon's start, after the harness's own


def _patch_new_host(m: Any) -> None:
    def new_host(home: Path, ids: str, model: Any) -> Any:
        m._NEXT["model"] = model
        prepare = list(m.PREPARE)
        m.PREPARE[:] = prepare + list(BEFORE_START)
        try:
            h = m.Host(home, clock=m.FakeClock(), ids=m.SequentialIds(ids), watch_owner_input=False, **EXTRA)
        finally:
            m.PREPARE[:] = prepare
        m.LIVE.append(h)
        d = h.daemon
        m.egress.install(getattr(d, "proxy", None))
        if d.supervisor is not None:
            d.supervisor.availability = d.supervisor._default_availability
        d.server.is_app = lambda token: True
        model.script.sock = h.sock
        return h
    m.new_host = new_host


def journal_rows(h: Any, after_seq: int) -> list[list[Any]]:
    def read() -> list[list[Any]]:
        rows = h.daemon.db.conn.execute("SELECT kind, data_json, origin FROM journal WHERE seq > ? ORDER BY seq",
                                        (after_seq,)).fetchall()
        return [[r[0], json.loads(r[1]), r[2]] for r in rows]
    return H.on_loop(h, read)  # type: ignore[no-any-return]


def journal_seq(h: Any) -> int:
    return int(H.on_loop(h, lambda: h.daemon.db.conn.execute("SELECT COALESCE(MAX(seq), 0) FROM journal")
                         .fetchone()[0]))


class BScenario:
    """A daemon/make.py Scenario that also records the journal rows and published events from its first step on,
    and the setup a Rust test rebuilds (`setup`: files written into the home, config text, seam values)."""

    def __init__(self, name: str, what: str, entry: list[str], *, extra: dict[str, Any] | None = None,
                 before_start: list[Any] | None = None, setup: dict[str, Any] | None = None,
                 model: Any = None) -> None:
        EXTRA.clear()
        EXTRA.update(extra or {})
        BEFORE_START[:] = list(before_start or [])
        self.events: list[list[Any]] = []
        self.s = H.Scenario(name, what, entry, model=model)
        EXTRA.clear()
        BEFORE_START.clear()
        self.setup: dict[str, Any] = dict(setup or {})
        self._hook()
        self.seq = journal_seq(self.s.host)

    def _hook(self) -> None:
        d = self.s.host.daemon

        def rec(topic: str, data: dict[str, Any]) -> None:
            self.events.append([topic, json.loads(json.dumps(data, default=str))])
        H.on_loop(self.s.host, lambda: d.events.on(None, rec))

    @property
    def host(self) -> Any:
        return self.s.host

    @property
    def d(self) -> Any:
        return self.s.host.daemon

    def conn(self, label: str, role: str = "cli") -> Any:
        return self.s.conn(label, role)

    def call(self, c: Any, method: str, params: Any = None) -> dict[str, Any]:
        msg = self.s.call(c, method, params)
        H.settle(self.s.host)
        return msg  # type: ignore[no-any-return]

    def calls(self, c: Any, *pairs: tuple[str, Any]) -> None:
        for method, params in pairs:
            self.call(c, method, params)

    def finish(self) -> dict[str, Any]:
        H.settle(self.s.host)
        rows = journal_rows(self.s.host, self.seq)
        out = self.s.finish(tables=False)
        extra = {"journal": rows, "published": self.events, "setup": self.setup}
        extra = H.scrub(json.loads(H.fx(json.dumps(extra))), H.VOLATILE_KEYS, set())
        out.update(H.intern(extra))
        return out  # type: ignore[no-any-return]


def write(name: str, scenarios: list[dict[str, Any]]) -> int:
    """`name` in this folder, and its long texts in bodies-<name>, by SHA-256 (each recorder its own file)."""
    text = H.fx(json.dumps({"scenarios": scenarios}, ensure_ascii=False, indent=1)) + "\n"
    (HERE / name).write_text(text, encoding="utf-8")
    bodies_path = HERE / f"bodies-{name}"
    if H.BODIES:
        bodies_path.write_text(json.dumps(dict(sorted(H.BODIES.items())), ensure_ascii=False, indent=1) + "\n",
                               encoding="utf-8")
    elif bodies_path.exists():
        bodies_path.unlink()
    return len(text.encode("utf-8"))


def run(tag: str, name: str, scenario_fns: list[Any]) -> None:
    """A recorder's main: each scenario, then the file; the harness's temporary root removed at the end."""
    import contextlib
    import shutil
    import time
    only = set(sys.argv[1:])
    out = []
    try:
        for fn in scenario_fns:
            if only and fn.__name__ not in only:
                continue
            t0 = time.monotonic()
            out.append(fn())
            print(f"{fn.__name__}: {time.monotonic() - t0:.1f}s", file=sys.stderr)
        size = write(name, out)
        print(f"{name}: {size} bytes")
    finally:
        for h in list(H.LIVE):
            with contextlib.suppress(Exception):
                h.stop()
        H.MP.undo()
        shutil.rmtree(H.ROOT, ignore_errors=True)
        os.environ.pop("FERB_HOME", None)
