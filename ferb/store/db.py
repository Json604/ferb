"""Own SQLite connection and schema migrations. Must not infer task outcomes or rewrite source content."""

import sqlite3
from pathlib import Path
from typing import Any, Callable

MIGRATIONS = Path(__file__).parent / "migrations"


class Database:
    """One connection, owned by the daemon's event loop thread."""

    def __init__(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        self.path = path
        self.conn = sqlite3.connect(path, isolation_level=None)   # autocommit; explicit transactions below
        self.conn.row_factory = sqlite3.Row
        self.conn.execute("PRAGMA journal_mode=WAL")
        self.conn.execute("PRAGMA foreign_keys=ON")
        self.conn.execute("PRAGMA secure_delete=ON")              # "forget" must really erase
        path.chmod(0o600)
        # Told when the WAL couldn't be emptied after a forget (the daemon journals it)
        self.on_wal_kept: Callable[[str, dict[str, Any]], None] = lambda after, details: None

    def truncate_wal(self, after: str) -> bool:
        """Empty the WAL, so no copy of what `after` (e.g. a forget) erased stays on disk. When a reader keeps it from
        finishing within the busy timeout, `on_wal_kept` is told what was left; returns whether it was emptied."""
        busy, frames, moved = self.conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()
        if busy:
            self.on_wal_kept(after, {"wal_frames": frames, "checkpointed": moved})
        return not busy

    def schema_version(self) -> int:
        self.conn.execute("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")
        row = self.conn.execute("SELECT MAX(version) AS v FROM schema_version").fetchone()
        return int(row["v"] or 0)

    def migrate(self) -> list[int]:
        """Apply every migrations/NNNN_*.sql newer than the current version, each in its own transaction."""
        applied = []
        current = self.schema_version()
        for f in sorted(MIGRATIONS.glob("[0-9][0-9][0-9][0-9]_*.sql")):
            version = int(f.name[:4])
            if version <= current:
                continue
            self.conn.execute("BEGIN")
            try:
                for statement in _split_sql(f.read_text()):
                    self.conn.execute(statement)
                self.conn.execute("INSERT INTO schema_version (version) VALUES (?)", (version,))
                self.conn.execute("COMMIT")
            except Exception:
                self.conn.execute("ROLLBACK")
                raise
            applied.append(version)
        return applied

    def close(self) -> None:
        self.conn.close()


def _split_sql(script: str) -> list[str]:
    """Split a migration into statements (executescript would commit implicitly, breaking the transaction).
    Uses sqlite3.complete_statement so triggers with inner semicolons stay whole."""
    statements, buf = [], ""
    for line in script.splitlines(keepends=True):
        if line.strip().startswith("--") and not buf.strip():
            continue
        buf += line
        if sqlite3.complete_statement(buf):
            if buf.strip():
                statements.append(buf.strip())
            buf = ""
    if buf.strip():
        raise ValueError(f"incomplete SQL statement at end of migration: {buf.strip()[:60]}")
    return statements
