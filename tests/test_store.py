"""Store (section 3/14): migrations, pragmas, append-only journal, span trees, the redaction seam on every writer."""

import sqlite3
from pathlib import Path
from typing import Any

import pytest

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.memory.journal import Journal
from ferb.store.db import Database
from ferb.store.spans import SpanStore


@pytest.fixture
def db(tmp_path: Path) -> Database:
    d = Database(tmp_path / "ferb.db")
    d.migrate()
    return d


def test_migrations_apply_once_and_rerun_is_a_no_op(tmp_path: Path) -> None:
    d = Database(tmp_path / "ferb.db")
    assert d.migrate() == [1]
    assert d.migrate() == []
    assert d.schema_version() == 1
    tables = {r[0] for r in d.conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'")}
    assert {"spans", "journal", "schema_version"} <= tables


def test_wal_secure_delete_foreign_keys_and_owner_only_file(db: Database) -> None:
    assert db.conn.execute("PRAGMA journal_mode").fetchone()[0] == "wal"
    assert db.conn.execute("PRAGMA secure_delete").fetchone()[0] == 1
    assert db.conn.execute("PRAGMA foreign_keys").fetchone()[0] == 1
    assert db.path.stat().st_mode & 0o077 == 0


def test_journal_is_append_only(db: Database) -> None:
    j = Journal(db, FakeClock(), SequentialIds("j"), lambda d: d)
    j.append("daemon.started", {"pid": 1})
    with pytest.raises(sqlite3.IntegrityError, match="append-only"):
        db.conn.execute("UPDATE journal SET kind = 'forged'")
    assert [e.kind for e in j.entries()] == ["daemon.started"]


def test_span_tree_rebuilds_nesting_and_is_scoped_to_the_task(db: Database) -> None:
    clock = FakeClock()
    sp = SpanStore(db, clock, SequentialIds("s"), lambda d: d)
    root = sp.start("task", task_id="t1", input={})
    a = sp.start("worker", task_id="t1", parent_id=root.id, input={})
    clock.advance(1)
    a1 = sp.start("tool.call", task_id="t1", parent_id=a.id, input={"tool": "fs.read"})
    sp.finish(a1, output={"ok": True})
    b = sp.start("worker", task_id="t1", parent_id=root.id, input={})
    sp.start("task", task_id="t2", input={})                       # other task
    sp.start("daemon.tick", input={})                              # daemon-level span: task_id None
    [r] = sp.tree("t1")
    assert r.span.id == root.id
    assert [c.span.id for c in r.children] == [a.id, b.id]
    assert [c.span.id for c in r.children[0].children] == [a1.id]
    assert r.children[0].children[0].span.output == {"ok": True}
    assert db.conn.execute("SELECT task_id FROM spans WHERE kind = 'daemon.tick'").fetchone()[0] is None


def test_every_writer_passes_through_the_redaction_seam(db: Database) -> None:
    """Build step 1's redact is a pass-through; this proves the seam is on every write path so step 2's
    sanitizer can't be bypassed."""
    seen: list[dict[str, Any]] = []

    def redact(d: dict[str, Any]) -> dict[str, Any]:
        seen.append(d)
        return {k: ("[R]" if k == "secret" else v) for k, v in d.items()}

    clock, ids = FakeClock(), SequentialIds("x")
    j = Journal(db, clock, ids, redact)
    sp = SpanStore(db, clock, ids, redact)
    j.append("chat.message", {"secret": "LEAK-JOURNAL-7f3a", "text": "hi"})
    s = sp.start("tool.call", task_id="t", input={"secret": "LEAK-INPUT-7f3a"})
    sp.finish(s, output={"secret": "LEAK-OUTPUT-7f3a"}, error="failed")
    raw = b"".join(p.read_bytes() for p in db.path.parent.glob("ferb.db*"))
    for leaked in (b"LEAK-JOURNAL-7f3a", b"LEAK-INPUT-7f3a", b"LEAK-OUTPUT-7f3a"):
        assert leaked not in raw
    assert len(seen) == 4        # journal data, span input, span output, span error
