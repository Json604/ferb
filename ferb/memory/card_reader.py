"""Reads one episode card in a process of its own (L28). Must not write: the daemon writes the card.

The lineage resolution behind a card is CPU-bound (1.3-1.6 s on the owner's data). In a thread it would still hold the
interpreter lock against the daemon's event loop, so it runs here, on a read-only SQLite connection of its own.

Started as exactly `python -I -m ferb.memory.card_reader`, with no arguments: the packaged app's stub accepts only exact
entry points. The request comes on stdin as JSON `{"task_id": ..., "apps": ..., "home": ...}`. The database is the one
in the owner's FERB home (`ferb.paths.ferb_home()`, which the daemon passes as FERB_HOME), never a path a caller names:
the request's `home` is only checked against it, and a request naming another home is refused.
It prints `{"prepared": ...}` or `{"error": "..."}`."""
import json
import sqlite3
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any

from ferb.paths import ferb_home


class _Rows(list[Any]):
    """A query's complete result. The statement is already finished, so the reader holds no WAL snapshot while it
    decodes rows, and a forget can empty the WAL meanwhile (review: TRUNCATE waited 5.21 s, then returned busy)."""

    def fetchone(self) -> Any:
        return self[0] if self else None

    def fetchall(self) -> list[Any]:
        return list(self)


class _Fetched:
    """The reader's connection: every query is read to its end before any row is used."""

    def __init__(self, conn: sqlite3.Connection) -> None:
        self._conn = conn

    def execute(self, sql: str, params: Any = ()) -> _Rows:
        return _Rows(self._conn.execute(sql, params).fetchall())


def read(task_id: str, apps: dict[str, str] | None, home: Path | None = None) -> dict[str, Any] | None:
    """Every read of the task's card (`Learning.prepare_episode`), on a read-only connection to `<home>/ferb.db`. The
    path becomes a file: URI by `Path.as_uri()`, so '?', '#' or '%' in it can't change which file is opened."""
    from ferb.memory.learning import Learning
    path = ((home or ferb_home()) / "ferb.db").absolute()
    conn = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    try:
        reader = Learning(SimpleNamespace(conn=_Fetched(conn), path=path), None, lambda *a, **k: None,
                          lambda text: text)
        return reader.prepare_episode(task_id, apps)
    finally:
        conn.close()


def main() -> None:
    if len(sys.argv) > 1:
        out: dict[str, Any] = {"error": "the card reader takes no arguments: it reads the owner's FERB home"}
    else:
        try:
            request = json.loads(sys.stdin.read())
            named = request.get("home")              # only checked, never opened
            if not named or Path(str(named)).resolve() != ferb_home().resolve():
                raise ValueError("the request names another FERB home than the reader's")
            out = {"prepared": read(str(request["task_id"]), request.get("apps"))}
        except Exception as exc:             # the daemon journals it (redacted); no traceback with record contents
            out = {"error": f"{type(exc).__name__}: {exc}"}
    sys.stdout.write(json.dumps(out))


if __name__ == "__main__":
    main()
