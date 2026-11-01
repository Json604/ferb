"""Step 12a's confirmed storage bugs (kb area storage): a continued tree's episode reads its last run; episodes keep
the outside-content labels of what the planner was shown; non-ASCII evidence is found whether it was written before
(JSON escapes) or after (UTF-8); a forget's scrub finds a told text in both forms. The 2026-11-01 review's: the first
start's memory clean-up survives a writer committing between its read and its write; the compaction asks for the room
VACUUM really takes; the hourly checkpoint never waits on a reader; a plan rewrite is all or nothing."""
import sqlite3
import threading
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.orchestrator import episodes
from ferb.store.db import Database
from ferb.store.spans import SpanStore


def _store(tmp_path: Path) -> tuple[Database, SpanStore, FakeClock]:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    clock = FakeClock()
    return db, SpanStore(db, clock, SequentialIds("s"), lambda value: value), clock


def _task_row(db: Database, root: str, at: str) -> None:
    db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, "
                    "updated_at) VALUES (?, ?, 'r1', 'tea', 'owner', 'worker', '[]', 'done', ?, ?)", (root, root, at, at))


def test_a_continued_trees_episode_has_its_last_runs_result_and_end(tmp_path: Path) -> None:
    db, spans, clock = _store(tmp_path)
    _task_row(db, "t1", clock.now().isoformat())
    first = spans.start("task", task_id="t1", input={"request": "order tea", "request_id": "r1"})
    spans.finish(first, output={"status": "failed", "verified": False})
    clock.advance(3600)                                  # continued an hour later: a second run of the same tree
    second = spans.start("task", task_id="t1", input={"request": "order tea, it's back in stock", "request_id": "r1"})
    clock.advance(60)
    spans.finish(second, output={"status": "done", "verified": True})
    ep = episodes.build(spans, "t1", clock.now().isoformat())
    assert ep["verified"] is True and ep["status"] == "done"
    assert ep["ended_at"] == clock.now().isoformat()                    # the last run's end, not the first's
    assert ep["request_text"]                                            # the request is still the first run's
    db.close()


def test_an_episode_keeps_the_origins_of_the_blocks_its_planner_was_shown(tmp_path: Path) -> None:
    db, spans, clock = _store(tmp_path)
    _task_row(db, "t1", clock.now().isoformat())
    root = spans.start("task", task_id="t1", input={"request": "summarize the page", "request_id": "r1"})
    spans.set_origin(root.id, "owner")
    shown = spans.start("context.exposure", task_id="t1", input={"source_refs": [], "reference": "Layer 3"})
    spans.finish(shown, output={"text": "a page"}, origin="external:web:x.com")
    spans.finish(root, output={"status": "done", "verified": True})
    ep = episodes.build(spans, "t1", clock.now().isoformat())
    assert "external:web:x.com" in ep["origins"]                         # outlives the spans with the episode
    db.close()


def test_non_ascii_evidence_is_found_as_written_now_and_as_older_rows_hold_it(tmp_path: Path) -> None:
    db, spans, clock = _store(tmp_path)
    new = spans.start("tool.call", task_id="t1", input={"tool": "browser.read", "args": {}})
    spans.finish(new, output={"text": "चाय का ऑर्डर"})
    assert "चाय" in db.conn.execute("SELECT output_json FROM spans WHERE id = ?", (new.id,)).fetchone()[0]
    db.conn.execute("INSERT INTO spans (id, task_id, kind, started_at, input_json, output_json) VALUES ('old', 't1', "
                    "'tool.call', ?, '{\"tool\": \"browser.read\"}', ?)",
                    (clock.now().isoformat(), '{"text": "\\u091a\\u093e\\u092f"}'))       # before step 12a: escaped
    found = {r["id"] for r in spans.tool_calls("t1", query="चाय")["records"]}
    assert found == {new.id, "old"}
    db.close()


def test_a_forget_scrub_finds_a_told_text_in_both_forms(tmp_path: Path) -> None:
    from ferb.orchestrator.attention import scrub_spans
    db, spans, clock = _store(tmp_path)
    told = '"text": "मिलते हैं"'
    new = spans.start("tool.call", task_id="t1", input={"tool": "chat.history", "args": {}})
    spans.finish(new, output={"context": f"{{{told}}}"})                # stored as UTF-8 now
    db.conn.execute("INSERT INTO spans (id, task_id, kind, started_at, input_json, output_json) VALUES ('old', 't1', "
                    "'tool.call', ?, '{}', ?)", (clock.now().isoformat(),
                                                 '{"context": "{\\"text\\": \\"\\u092e\\u093f\\u0932\\u0924\\u0947 '
                                                 '\\u0939\\u0948\\u0902\\"}"}'))
    scrub_spans(db, [(told, '"text": "[removed]"'), ('"text": "\\u092e\\u093f\\u0932\\u0924\\u0947 \\u0939\\u0948\\u0902"',
                                                     '"text": "[removed]"')], every_kind=True)
    for ident in (new.id, "old"):
        out = db.conn.execute("SELECT output_json FROM spans WHERE id = ?", (ident,)).fetchone()[0]
        assert "[removed]" in out and "मिलते" not in out and "\\u092e" not in out, (ident, out)
    db.close()


class _Interleaved:
    """The loop's connection, with another connection's write started right after the statement `after` (a batch of
    the search re-index committing in between); it waits up to `wait` seconds for that write to finish."""

    def __init__(self, conn: sqlite3.Connection, path: Path, after: str, wait: float = 0.5) -> None:
        self.conn, self.path, self.after, self.wait = conn, path, after, wait
        self.writer: threading.Thread | None = None
        self.wrote: list[bool] = []

    def __getattr__(self, name: str) -> Any:
        return getattr(self.conn, name)

    def execute(self, sql: str, *args: Any) -> Any:
        out = self.conn.execute(sql, *args)
        if self.writer is None and sql.startswith(self.after):
            def write() -> None:
                other = sqlite3.connect(self.path, timeout=10)
                other.execute("INSERT INTO forgotten_refs VALUES ('trail:x', 'now')")
                other.commit()
                other.close()
                self.wrote.append(True)
            self.writer = threading.Thread(target=write)
            self.writer.start()
            self.writer.join(self.wait)      # with the write lock held it waits for our commit; otherwise it is done
        return out


# decision: STORE-415
def test_the_first_starts_memory_cleanup_survives_a_write_between_its_read_and_its_write(tmp_path: Path) -> None:
    """Fix 1 (2026-11-01): the search re-index worker commits batches while the first start cleans memory; the
    clean-up's transaction holds the write lock from its start, so its first write never fails on a stale read."""
    from ferb.core.ids import SequentialIds
    from ferb.memory.items import Memory
    from ferb.memory.upgrade import clean_before_tier1
    db, _, clock = _store(tmp_path)
    db.conn.execute("INSERT INTO memory_items (id, kind, subject, key, content, origin, status, confidence, "
                    "observed_at, valid_from, created_at) VALUES ('m1', 'preference', 'owner', 'tea', 'green tea', "
                    "'inferred', 'proposed', 1.0, 'x', 'x', 'x')")
    real, interleaved = db.conn, _Interleaved(db.conn, db.path, "SELECT id FROM memory_items")
    db.conn = interleaved                                                  # type: ignore[assignment]
    out = clean_before_tier1(Memory(db, clock, SequentialIds("m"), lambda *a: None), lambda kind, data: None)
    db.conn = real
    assert interleaved.writer is not None
    interleaved.writer.join(10)
    assert out is not None and out["guesses_kept"] == 1 and interleaved.wrote == [True]   # both writes landed
    assert db.conn.execute("SELECT status FROM memory_items WHERE id = 'm1'").fetchone()[0] == "active"
    db.close()


def _filled(tmp_path: Path, rows: int = 400) -> Database:
    """A store from before incremental auto-vacuum, with some rows."""
    made = sqlite3.connect(tmp_path / "ferb.db")
    made.executescript("PRAGMA auto_vacuum = NONE; CREATE TABLE made (x); DROP TABLE made;")
    made.close()
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    db.conn.executemany("INSERT INTO forgotten_refs VALUES (?, ?)",
                        [(f"ref:{i}:" + "x" * 500, "at") for i in range(rows)])
    return db


# decision: STORE-416
def test_compaction_asks_for_twice_what_the_database_holds_and_says_so(tmp_path: Path) -> None:
    """Fix 2 (2026-11-01): VACUUM in WAL mode writes the new database into the WAL beside its temporary copy, so the
    free disk it needs is at least twice the used bytes, not the file's size; the refusal says the figure."""
    from ferb.store.db import compaction_needs, store_size
    db = _filled(tmp_path)
    db.conn.execute("DELETE FROM forgotten_refs WHERE rowid % 2 = 0")       # free pages: the file is larger than used
    db.truncate_wal("test")
    pages, free_pages, size = (int(db.conn.execute(f"PRAGMA {p}").fetchone()[0])
                               for p in ("page_count", "freelist_count", "page_size"))
    used = (pages - free_pages) * size
    needed = compaction_needs(db.conn)
    assert needed >= 2 * used
    out = db.compact(free=lambda folder: max(store_size(db.path), 2 * used))  # the old check passed here
    assert out == {"compacted": False, "needed": needed, "free": max(store_size(db.path), 2 * used)}
    assert not db.incremental()                                            # nothing written
    assert db.compact(free=lambda folder: needed)["compacted"] is True and db.incremental()
    db.close()


# decision: STORE-409
def test_the_hourly_checkpoint_never_waits_for_a_reader(tmp_path: Path) -> None:
    """Fix 3 (2026-11-01): the routine checkpoint runs on the event loop; a reader holding an older snapshot used to
    hold it for the whole busy timeout (5 s). Now it copies what it can, says the WAL wasn't emptied, and leaves the
    timeout as it was; once the reader is gone it empties the WAL."""
    db = _filled(tmp_path, rows=10)
    reader = sqlite3.connect(db.path)
    reader.execute("BEGIN")
    reader.execute("SELECT COUNT(*) FROM forgotten_refs").fetchone()      # a snapshot the checkpoint can't pass
    db.conn.execute("INSERT INTO forgotten_refs VALUES ('late', 'at')")
    started = time.monotonic()
    assert db.checkpoint() is False
    assert time.monotonic() - started < 1
    assert db.conn.execute("PRAGMA busy_timeout").fetchone()[0] == 5000
    reader.rollback()
    reader.close()
    assert db.checkpoint() is True
    assert (db.path.with_name(db.path.name + "-wal")).stat().st_size == 0
    db.close()


# decision: STORE-415
def test_a_plan_rewrite_that_fails_halfway_leaves_the_plan_as_it_was(tmp_path: Path) -> None:
    """Fix 5 (2026-11-01): the connection is autocommit, so `with conn:` opened no transaction and a replace that failed
    after moving the old steps left them moved under the old plan. It is one transaction now."""
    from ferb.orchestrator.plan_store import PlanStore
    from ferb.orchestrator.plans import Plan, Step
    db, _, _ = _store(tmp_path)
    store = PlanStore(db, lambda value: value)
    old = Plan(steps=[Step(id="a", kind="do", goal="find"), Step(id="b", kind="do", goal="send")])
    store.create("t1", old, {"text": "find and send"}, None)
    store.finish("t1", "a", {"status": "done"})
    before = store.steps("t1"), db.conn.execute("SELECT plan, status FROM task_plans").fetchall()

    class Broken(PlanStore):
        def _json(self, value: Any) -> str:
            raise RuntimeError("disk full")                              # after the step rows were rewritten
    with pytest.raises(RuntimeError):
        Broken(db, lambda value: value).replace("t1", Plan(steps=[Step(id="c", kind="do", goal="reply")]))
    assert (store.steps("t1"), db.conn.execute("SELECT plan, status FROM task_plans").fetchall()) == before
    assert not db.conn.in_transaction
    with pytest.raises(RuntimeError):
        Broken(db, lambda value: value).create("t2", old, {}, None)
    assert db.conn.execute("SELECT COUNT(*) FROM plan_steps WHERE task_id = 't2'").fetchone()[0] == 0
    db.close()


# decision: STORE-5
def test_typed_and_copied_text_goes_by_its_row_of_the_retention_table(tmp_path: Path) -> None:
    """Fix 6 (2026-11-01): the seven days typed and copied text is kept are a row of `retention.RULES` (what the About
    page and FERB's self-description show), and the expiry reads its window from there."""
    from datetime import timedelta
    from types import SimpleNamespace
    from ferb.sense.perception import Perception
    from ferb.store import retention
    [rule] = [r for r in retention.RULES if r.default == retention.TYPED_DAYS and "typed" in r.records]
    assert rule.setting is None                                          # fixed days (the owner's D14), no setting
    db, _, clock = _store(tmp_path)
    for ident, age in (("old", retention.TYPED_DAYS + 1), ("new", retention.TYPED_DAYS - 1)):
        at = clock.now() - timedelta(days=age)
        db.conn.execute("INSERT INTO trail (id, at, day, device_id, actor, source, data_json) VALUES "
                        "(?, ?, ?, 'd', 'owner', 'typed_text', '{\"text\": \"chai\", \"app\": \"Notes\"}')",
                        (ident, at.isoformat(), at.date().isoformat()))
    perception = Perception.__new__(Perception)
    perception.daemon = SimpleNamespace(db=db, clock=clock, daily_activity=SimpleNamespace(roll_over=lambda: None))
    assert perception.purge_text() == 1
    held = {r[0]: r[1] for r in db.conn.execute("SELECT id, data_json FROM trail")}
    assert "chai" not in held["old"] and "Notes" in held["old"] and "chai" in held["new"]
    db.close()
