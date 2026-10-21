"""A scrub and a live write, interleaved deterministically (fix wave 6a). The daemon runs a forget's scrub in a worker
thread on a connection of its own while the event loop keeps writing spans on its own connection; here both
connections are open on one database file and the live write lands right after a chosen statement of the scrub."""
import json
from pathlib import Path
from typing import Any, Callable

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.store.db import Database
from ferb.store.spans import SpanStore

When = Callable[[str, tuple[Any, ...]], bool]


class Interleaved:
    """Stands in for a Database's connection: right after the first statement `when(sql, params)` accepts, `then()`
    runs once. That statement's read is already taken, as a scrub's read is when a live write lands after it. Every
    statement is kept in `statements`."""

    def __init__(self, db: Database, when: When, then: Callable[[], Any] = lambda: None) -> None:
        self._conn, self._when, self._then = db.conn, when, then
        self.fired = False
        self.statements: list[tuple[str, tuple[Any, ...]]] = []
        db.conn = self                                    # type: ignore[assignment]

    def execute(self, sql: str, params: Any = ()) -> Any:
        cursor = self._conn.execute(sql, params)
        self.statements.append((sql, tuple(params)))
        if not self.fired and self._when(sql, tuple(params)):
            self.fired = True
            self._then()
        return cursor

    def __getattr__(self, name: str) -> Any:
        return getattr(self._conn, name)


def reads(ident: str) -> When:
    """A read of one row by its id (a span's recorded text, as a scrub reads it before rewriting it)."""
    return lambda sql, params: sql.lstrip().upper().startswith("SELECT") and params == (ident,)


def lists(fragment: str) -> When:
    """A read listing rows, told apart by a fragment of its SQL (a scrub's candidates, read before it rewrites them)."""
    return lambda sql, params: sql.lstrip().upper().startswith("SELECT") and fragment in sql


def runs(fragment: str) -> When:
    """Any statement told apart by a fragment of its SQL (a write in the middle of a scrub's rewrite)."""
    return lambda sql, params: fragment in sql


class TwoConnections:
    """The scrub's connection (`scrub`) and the event loop's (`loop`, with its `spans` store) on one database."""

    def __init__(self, tmp_path: Path) -> None:
        path = tmp_path / "ferb.db"
        self.loop = Database(path)
        self.loop.migrate()
        self.clock = FakeClock()
        self.spans = SpanStore(self.loop, self.clock, SequentialIds("s"), lambda value: value)
        self.scrub = Database(path)

    def held(self, ident: str) -> tuple[Any, Any, Any]:
        """A span's recorded input and output (decoded) and when it ended, read on the loop's connection."""
        row = self.loop.conn.execute("SELECT input_json, output_json, ended_at FROM spans WHERE id = ?",
                                     (ident,)).fetchone()
        return json.loads(row[0]), json.loads(row[1] or "null"), row[2]

    def close(self) -> None:
        self.scrub.close()
        self.loop.close()
