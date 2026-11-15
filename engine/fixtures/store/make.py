"""Fixtures for the Rust store port (rust/crates/ferb-store), made by the Python store code (ENG-3).

    cd <repo> && uv run python rust/fixtures/store/make.py

creates a fresh store in a temporary folder with ferb/store/db.py (never ~/.ferb), applies every migration, and writes
fresh_store.json next to this file: the whole sqlite_master, the schema_version rows, the settings SQLite reports and
the file's mode. The Rust tests compare a store made by the Rust code with it.

Two more commands let a test check that each daemon opens the other's store:
    make.py create PATH   a fresh migrated store at PATH
    make.py accept PATH   open the store at PATH and migrate it; prints what Python sees as JSON
"""

import json
import sqlite3
import stat
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO))

from ferb.store.db import Database, known_schema  # noqa: E402

PRAGMAS = ("auto_vacuum", "journal_mode", "journal_size_limit", "foreign_keys", "secure_delete", "busy_timeout",
           "page_size", "encoding", "user_version", "application_id")


def describe(db: Database, applied: list[int]) -> dict[str, object]:
    conn = db.conn
    return {
        "applied": applied,
        "known_schema": known_schema(),
        "schema_version": [[r["version"], r["sha256"]] for r in
                           conn.execute("SELECT version, sha256 FROM schema_version ORDER BY version")],
        "pragmas": {p: conn.execute(f"PRAGMA {p}").fetchone()[0] for p in PRAGMAS},
        "mode": oct(stat.S_IMODE(db.path.stat().st_mode)),
        "sqlite_master": [dict(r) for r in conn.execute(
            "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name")],
    }


def main(argv: list[str]) -> None:
    if argv[:1] == ["create"]:
        db = Database(Path(argv[1]))
        db.migrate()
        db.close()
    elif argv[:1] == ["accept"]:
        db = Database(Path(argv[1]))
        seen = describe(db, db.migrate())
        del seen["sqlite_master"]
        db.close()
        print(json.dumps(seen))
    else:
        with tempfile.TemporaryDirectory() as folder:
            db = Database(Path(folder) / "ferb.db")
            fixture = {"made_by": "rust/fixtures/store/make.py", "python_sqlite": sqlite3.sqlite_version,
                       **describe(db, db.migrate())}
            db.close()
        out = Path(__file__).with_name("fresh_store.json")
        out.write_text(json.dumps(fixture, indent=1, ensure_ascii=False) + "\n")
        print(f"wrote {out} ({len(fixture['sqlite_master'])} schema rows, schema {fixture['known_schema']})")


if __name__ == "__main__":
    main(sys.argv[1:])
