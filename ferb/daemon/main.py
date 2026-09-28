"""Daemon startup / shutdown lifecycle (section 2) and the step-1 methods: ping, trace.get, logs.subscribe."""

import asyncio
import os
import signal
from dataclasses import asdict
from pathlib import Path
from typing import Any, AsyncIterator, Callable

from ferb.bus import DROPPED, Bus, Overflow
from ferb.clock.clock import SystemClock
from ferb.core.errors import FerbError
from ferb.core.ids import RandomIds
from ferb.core.interfaces import Clock, IdSource
from ferb.daemon.auth import RunTokens
from ferb.daemon.server import Server, Session, ensure_socket_free
from ferb.memory.journal import Journal
from ferb.store.db import Database
from ferb.store.spans import SpanNode, SpanStore

Redact = Callable[[dict[str, Any]], dict[str, Any]]


def ferb_home() -> Path:
    return Path(os.environ.get("FERB_HOME", Path.home() / ".ferb"))


def passthrough_redact(data: dict[str, Any]) -> dict[str, Any]:
    """Build step 1 seam: every writer calls this; step 2 replaces it with sanitize/redact.py."""
    return data


# `ferb logs` filter flags -> bus topic patterns
LOG_FILTERS = {"tasks": "task.*", "trail": "trail.event", "tools": "tool.called", "browsers": "browser.*"}


class Daemon:
    def __init__(self, home: Path | None = None, clock: Clock | None = None, ids: IdSource | None = None,
                 redact: Redact = passthrough_redact, owner_uid: int | None = None) -> None:
        self.home = home or ferb_home()
        self.clock = clock or SystemClock()
        self.ids = ids or RandomIds()
        self.redact = redact
        self.bus = Bus(self.clock)
        self.tokens = RunTokens(self.clock)
        self.server = Server(self.home / "run" / "ferb.sock", self.tokens, owner_uid)
        self.db: Database | None = None
        self.journal: Journal | None = None
        self.spans: SpanStore | None = None
        self._stopped = asyncio.Event()

    async def start(self) -> None:
        self.home.mkdir(parents=True, exist_ok=True)
        os.chmod(self.home, 0o700)
        ensure_socket_free(self.server.path)   # refuse before touching the database if another daemon runs
        self.db = Database(self.home / "ferb.db")
        self.db.migrate()
        self.journal = Journal(self.db, self.clock, self.ids, self.redact)
        self.spans = SpanStore(self.db, self.clock, self.ids, self.redact)
        self.server.unary["ping"] = self._ping
        self.server.unary["trace.get"] = self._trace
        self.server.streams["logs.subscribe"] = self._logs
        await self.server.start()          # refuses to start if another daemon answers on the socket
        self.journal.append("daemon.started", {"pid": os.getpid()})

    async def stop(self) -> None:
        await self.server.close()
        if self.journal:
            self.journal.append("daemon.stopped", {"pid": os.getpid()})
        if self.db:
            self.db.close()
        self._stopped.set()

    async def run(self) -> None:
        """Run until SIGTERM / SIGINT, then shut down cleanly."""
        await self.start()
        loop = asyncio.get_running_loop()
        stop = asyncio.Event()
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.add_signal_handler(sig, stop.set)
        await stop.wait()
        await self.stop()

    # ---------- methods ----------

    async def _ping(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"pid": os.getpid(), "role": session.role.value, "now": self.clock.now().isoformat()}

    async def _trace(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        task_id = str(params.get("task_id", ""))
        if self.spans is None:
            raise FerbError("daemon not started")
        return {"task_id": task_id, "spans": [_node_json(n) for n in self.spans.tree(task_id)]}

    async def _logs(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        wanted = {LOG_FILTERS[f] for f in params.get("filters", []) if f in LOG_FILTERS} or None
        sub = self.bus.subscribe(wanted, maxsize=int(params.get("maxsize", 1000)))
        try:
            async for ev in sub:
                yield {"topic": ev.topic, "at": ev.at.isoformat(),
                       "class": "notice" if ev.topic == DROPPED else ev.cls.value, "data": ev.data}
        except Overflow:
            yield {"topic": "bus.overflow", "at": self.clock.now().isoformat(), "class": "notice",
                   "data": {"message": "fell behind on critical events; restart `ferb logs`"}}
        finally:
            sub.close()


def _node_json(node: SpanNode) -> dict[str, Any]:
    s = asdict(node.span)
    for k in ("started_at", "ended_at"):
        s[k] = s[k].isoformat() if s[k] else None
    s["children"] = [_node_json(c) for c in node.children]
    return s


def main() -> None:
    asyncio.run(Daemon().run())
