"""Own SQLite connection and schema migrations. Must not infer task outcomes or rewrite source content."""

import hashlib
import shutil
import sqlite3
from pathlib import Path
from typing import Any, Callable

from ferb.core.errors import FerbError

MIGRATIONS = Path(__file__).parent / "migrations"
# Ruling (step 12a): how long a connection waits for another's write lock before "database is locked": the 5 s sqlite3
# already waited, now explicit on every connection FERB opens (the loop's and each worker thread's)
BUSY_TIMEOUT_MS = 5000
# Ruling (step 12a): the size the WAL file is cut back to after a checkpoint (it never stays at its largest)
JOURNAL_SIZE_LIMIT = 64 * 1024 * 1024
# Ruling (step 12a): free pages handed back to the disk after each hourly retention pass, in slices of this many pages
# (4 KB each) with the loop free between slices; only on a store compacted with incremental auto-vacuum
VACUUM_SLICE_PAGES = 512
VACUUM_PASS_PAGES = 8192


class MigrationError(FerbError):
    """The migrations can't be applied safely; the daemon refuses to start rather than run on a wrong schema."""


def store_size(path: Path) -> int:
    """The bytes a store takes on disk: its database file and its WAL."""
    return sum(p.stat().st_size for p in (path, path.with_name(path.name + "-wal")) if p.exists())


class Database:
    """One connection, owned by the daemon's event loop thread."""

    def __init__(self, path: Path) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        self.path = path
        new = not path.exists() or path.stat().st_size == 0
        self.conn = sqlite3.connect(path, isolation_level=None)   # autocommit; explicit transactions below
        self.conn.row_factory = sqlite3.Row
        self.conn.execute(f"PRAGMA busy_timeout={BUSY_TIMEOUT_MS}")
        if new:                                   # only before the first table: an older store gets it by `compact`
            self.conn.execute("PRAGMA auto_vacuum=INCREMENTAL")
        self.conn.execute("PRAGMA journal_mode=WAL")
        self.conn.execute(f"PRAGMA journal_size_limit={JOURNAL_SIZE_LIMIT}")
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

    def checkpoint(self) -> bool:
        """The routine WAL checkpoint after an hourly pass, cutting the file back (TRUNCATE); a reader in the way only
        means it waits for the next pass. Returns whether the WAL was emptied."""
        return not self.conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").fetchone()[0]

    def incremental(self) -> bool:
        """Whether the store hands free pages back to the disk on its own (`auto_vacuum` INCREMENTAL: a store created
        by this code, or one `compact` rebuilt)."""
        return int(self.conn.execute("PRAGMA auto_vacuum").fetchone()[0]) == 2

    def vacuum_slice(self, pages: int = VACUUM_SLICE_PAGES) -> int:
        """Up to `pages` free pages handed back to the disk (a no-op unless `incremental`); returns how many free pages
        are left."""
        self.conn.execute(f"PRAGMA incremental_vacuum({int(pages)})").fetchall()
        return int(self.conn.execute("PRAGMA freelist_count").fetchone()[0])

    def compact(self, free: Callable[[Path], int] = lambda folder: shutil.disk_usage(folder).free) -> dict[str, Any]:
        """Shrink the file to what it holds, and from then on let it give free pages back by itself: the WAL
        checkpointed, `auto_vacuum` set to INCREMENTAL, VACUUM (which makes that setting take effect), the WAL
        checkpointed again. Only on a connection nothing else shares (setup, with the daemon stopped). VACUUM writes a
        full copy of the database, so it runs only when the disk has at least the database and its WAL free (`free`:
        the free bytes of the folder's volume); otherwise nothing is written. Returns whether it compacted, the bytes
        it needed and those free."""
        self.truncate_wal("store.compact")
        needed, room = store_size(self.path), free(self.path.parent)
        if room < needed:
            return {"compacted": False, "needed": needed, "free": room}
        self.conn.execute("PRAGMA auto_vacuum=INCREMENTAL")
        self.conn.execute("VACUUM")
        self.truncate_wal("store.compact")
        return {"compacted": True, "needed": needed, "free": room}

    def schema_version(self) -> int:
        self._version_table()
        row = self.conn.execute("SELECT MAX(version) AS v FROM schema_version").fetchone()
        return int(row["v"] or 0)

    def _version_table(self) -> None:
        self.conn.execute("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL, sha256 TEXT)")
        columns = {r["name"] for r in self.conn.execute("PRAGMA table_info(schema_version)")}
        if "sha256" not in columns:                       # an install migrated before checksums were recorded
            self.conn.execute("ALTER TABLE schema_version ADD COLUMN sha256 TEXT")

    def migrate(self, folder: Path | None = None) -> list[int]:
        """Apply every NNNN_*.sql not yet applied, in order, each in its own transaction, recording its checksum.
        Refused (MigrationError, nothing applied): two files with one number; a new file numbered at or below the
        newest applied one (it would otherwise never run on existing installs); an applied file whose content
        changed; a database migrated by newer code than this. A recorded version whose file is gone is fine below the
        newest. Rows from before checksums get theirs on the first run."""
        folder = folder if folder is not None else MIGRATIONS     # looked up per call (tests patch it)
        files: dict[int, Path] = {}
        for f in sorted(folder.glob("[0-9][0-9][0-9][0-9]_*.sql")):
            version = int(f.name[:4])
            if version in files:
                raise MigrationError(f"two migrations are numbered {version:04d}: {files[version].name}, {f.name}")
            files[version] = f
        self._version_table()
        recorded = {int(r["version"]): r["sha256"] for r in self.conn.execute("SELECT version, sha256 FROM schema_version")}
        newest = max(recorded, default=0)
        if newest > max(files, default=0):
            raise MigrationError(f"the database is newer than this FERB (schema {newest}, this build knows up to "
                                 f"{max(files, default=0)}); install the newer FERB")
        sums = {version: hashlib.sha256(f.read_bytes()).hexdigest() for version, f in files.items()}
        for version, f in files.items():
            if version in recorded:
                if recorded[version] is not None and recorded[version] != sums[version]:
                    raise MigrationError(f"migration {f.name} changed after it was applied; add a new migration instead")
            elif version < newest:
                raise MigrationError(f"migration {f.name} is numbered below the newest applied ({newest:04d}) and would "
                                     f"never run on existing installs; renumber it above {newest:04d}")
        for version in recorded:
            if version in files and recorded[version] is None:
                self.conn.execute("UPDATE schema_version SET sha256 = ? WHERE version = ?", (sums[version], version))
        applied = []
        for version, f in files.items():
            if version in recorded:
                continue
            self.conn.execute("BEGIN")
            try:
                for statement in _split_sql(f.read_text()):
                    self.conn.execute(statement)
                self.conn.execute("INSERT INTO schema_version (version, sha256) VALUES (?, ?)", (version, sums[version]))
                self.conn.execute("COMMIT")
            except Exception:
                self.conn.execute("ROLLBACK")
                raise
            applied.append(version)
        return applied

    def close(self) -> None:
        self.conn.close()


Row = tuple[Any, ...]


def swap(db: Any, table: str, key: str, ident: Any, columns: tuple[str, ...], change: Callable[[Row], Row],
         held: Row | None = None) -> Row | None:
    """One row's `columns` rewritten through `change` from what they hold now (fix wave 6a). The write applies only
    while the row still holds what was read (compare and set): when another connection wrote the row in between, it is
    read and changed again, so a rewrite never writes from a stale read. `held`: the values as the caller already read
    them. `change` giving them back unchanged writes nothing. Returns the values written, or None. Table and column
    names are FERB's own constants, never input."""
    read = f"SELECT {', '.join(columns)} FROM {table} WHERE {key} = ?"
    write = (f"UPDATE {table} SET {', '.join(c + ' = ?' for c in columns)} WHERE {key} = ? AND "
             + " AND ".join(c + " IS ?" for c in columns))
    while True:
        if held is None:
            row = db.conn.execute(read, (ident,)).fetchone()
            if row is None:
                return None
            held = tuple(row)
        new = tuple(change(held))
        if new == held:
            return None
        if db.conn.execute(write, (*new, ident, *held)).rowcount:
            return new
        held = None


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
