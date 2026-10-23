"""Plan 2026-10-23 Task 1 (L28, C1): the one-time clean-out of episode cards' source rows, and the compaction after it.

The composer before memory review C1 (`code:evidence-card/1`, installed until the owner's next update) copied into a
card the refs of every block its planner was shown, and each earlier version's: 22.6M rows on 438 card versions of 85
tasks in the owner's 8.1 GB store. The clean-out keeps on each version of a traced task's card exactly what the current
rule gives it (`Learning.card_source_refs`). It leaves a card whose trace aged out as it is, never adds an edge and
never touches another kind of node. Each node is cleaned in its own transaction."""
import sqlite3
from pathlib import Path
from typing import Any

import pytest

from ferb.core import origin
from ferb.memory.learning import Learning
from ferb.store.db import Database, store_size
from ferb.store.retention import purge_task_spans
from tests.support.cards import card_store, copied_cards, sources_of, traced_task


@pytest.fixture
def store(tmp_path: Path) -> Any:
    db, redactor, spans, learning = card_store(tmp_path)
    yield db, spans, learning
    db.close()


def _rows(db: Database) -> dict[str, set[str]]:
    out: dict[str, set[str]] = {}
    for node, ref in db.conn.execute("SELECT node_id, source_ref FROM learning_sources"):
        out.setdefault(node, set()).add(ref)
    return out


def test_card_source_refs_are_what_a_current_card_cites(store: Any) -> None:
    """The source collection factored out of `prepare_episode` is the card rule itself: a card the current composer
    writes cites exactly `card_source_refs`; a task with no trace left gives None."""
    db, spans, learning = store
    kept, shown = traced_task(db, spans, learning, "c0ffee01")
    assert learning.card_source_refs("c0ffee01") == kept
    card = learning.capture_episode("c0ffee01")
    assert card is not None and set(card["sources"]) == kept
    assert learning.card_source_refs("c0ffee99") is None


def test_each_version_of_a_traced_card_keeps_only_what_the_current_rule_gives(store: Any) -> None:
    """Every version, superseded or rolled back too, loses exactly the rows from blocks its planner was only shown
    and keeps its episode, the request entry, the procedure's sources and the sources of its own actions."""
    db, spans, learning = store
    kept, shown = traced_task(db, spans, learning, "c0ffee01")
    versions = copied_cards(learning, "c0ffee01", kept | shown)
    db.conn.execute("UPDATE learning_nodes SET state = 'rolled_back' WHERE id = ?", (versions[0],))
    partial = learning.put("episode", "c0ffee01", {"task_id": "c0ffee01", "version": "partial"},
                           (kept - {"episode:other"}) | shown, made_by="code:evidence-card/1",
                           origin_label=origin.FERB, state="active", labels={origin.PRIVATE})["id"]
    counts = learning.prune_card_sources()
    for node in versions:
        assert sources_of(db, node) == kept
    assert sources_of(db, partial) == kept - {"episode:other"}            # an edge is never added
    assert counts == {"tasks": 1, "nodes": 4, "cleaned": 4, "rows": 4 * len(shown), "untraced": 0,
                      "stopped": False}


def test_a_card_the_current_composer_built_loses_nothing(store: Any) -> None:
    db, spans, learning = store
    kept, _ = traced_task(db, spans, learning, "c0ffee01")
    card = learning.capture_episode("c0ffee01")
    assert card is not None
    before = _rows(db)
    counts = learning.prune_card_sources()
    assert _rows(db) == before and counts["rows"] == 0 and counts["cleaned"] == 0 and counts["nodes"] == 1


def test_a_card_whose_trace_aged_out_is_left_alone_and_counted(store: Any) -> None:
    """Review focus 1: its lineage can't be read again, so the clean-out doesn't guess at it."""
    db, spans, learning = store
    kept, shown = traced_task(db, spans, learning, "c0ffee02")
    versions = copied_cards(learning, "c0ffee02", kept | shown, versions=2)
    purge_task_spans(db, "c0ffee02")                                     # what retention does after 30 days
    before = _rows(db)
    counts = learning.prune_card_sources()
    assert _rows(db) == before
    assert counts["untraced"] == 2 and counts["rows"] == 0 and counts["nodes"] == 2
    assert all(sources_of(db, v) == kept | shown for v in versions)


def test_other_kinds_of_node_are_untouched(store: Any) -> None:
    db, spans, learning = store
    kept, shown = traced_task(db, spans, learning, "c0ffee01")
    card = copied_cards(learning, "c0ffee01", kept | shown, versions=1)[0]
    labels = {origin.PRIVATE}
    others = [learning.put("activity_summary", "2026-10-14", {"text": "a busy morning"}, shown, made_by="test",
                           labels=labels)["id"],
              learning.put("summary", "week", {"text": "a busy week"}, shown | {"journal:x"}, made_by="test",
                           labels=labels)["id"],
              learning.put("episode_annotation", card, {"text": "went well"}, [card, *sorted(shown)], made_by="test",
                           labels=labels)["id"]]
    before = {node: sources_of(db, node) for node in others}
    learning.prune_card_sources()
    assert {node: sources_of(db, node) for node in others} == before
    assert sources_of(db, card) == kept


def test_a_second_run_removes_nothing(store: Any) -> None:
    db, spans, learning = store
    for task in ("c0ffee01", "c0ffee02"):
        kept, shown = traced_task(db, spans, learning, task)
        copied_cards(learning, task, kept | shown)
    first = learning.prune_card_sources()
    after_first = _rows(db)
    second = learning.prune_card_sources()
    assert first["rows"] > 0 and second["rows"] == 0 and second["cleaned"] == 0 and _rows(db) == after_first


def test_forgetting_a_row_an_action_used_still_forgets_the_card_and_what_was_built_on_it(store: Any) -> None:
    """Review focus 2: an action's sources stay edges, so a forget still reaches every version and its descendants."""
    db, spans, learning = store
    kept, shown = traced_task(db, spans, learning, "c0ffee01")
    versions = copied_cards(learning, "c0ffee01", kept | shown)
    lesson = learning.put("lesson", "tea", {"text": "check the order page first"}, [versions[-1]], made_by="test",
                          labels={origin.PRIVATE})["id"]
    learning.prune_card_sources()
    doomed = set(learning.forget(["trail:ev-c0ffee01-b"]))
    assert set(versions) | {lesson} <= doomed
    assert learning.current_card("c0ffee01") is None and learning.get(lesson) is None
    assert learning.prepare_episode("c0ffee01") is None                  # its key's tombstone: never rebuilt (C1)


def test_forgetting_a_row_the_planner_was_only_shown_no_longer_touches_the_card(store: Any) -> None:
    """The current design (`prepare_episode`): what the planner was only shown never touches the card."""
    db, spans, learning = store
    kept, shown = traced_task(db, spans, learning, "c0ffee01")
    versions = copied_cards(learning, "c0ffee01", kept | shown)
    learning.prune_card_sources()
    assert learning.forget(["trail:shown-c0ffee01-0", "trail:snap-c0ffee01"]) == []
    assert all(learning.get(v) is not None for v in versions)


def _three_tasks(db: Database, spans: Any, learning: Any) -> tuple[dict[str, set[str]], dict[str, set[str]]]:
    """Three traced tasks, two copied versions of each card: each node's rows before, and after a full clean-out."""
    original: dict[str, set[str]] = {}
    pruned: dict[str, set[str]] = {}
    for task in ("c0ffee01", "c0ffee02", "c0ffee03"):
        kept, shown = traced_task(db, spans, learning, task, shown=300)
        for node in copied_cards(learning, task, kept | shown, versions=2):
            original[node], pruned[node] = kept | shown, kept
    return original, pruned


def test_an_interrupted_run_leaves_each_node_whole_and_the_next_run_finishes(store: Any) -> None:
    """Review focus 3: SQLite is stopped in the middle of a node's DELETE (as a crash or Ctrl-C stops it). Every node
    is then either fully cleaned or as it was, and the next run finishes the rest."""
    db, spans, learning = store
    original, pruned = _three_tasks(db, spans, learning)
    deletes = 0
    countdown: list[int] = []

    def trace(sql: str) -> None:
        nonlocal deletes
        if sql.startswith("DELETE FROM learning_sources"):
            deletes += 1
            if deletes == 4:                         # the fourth node's DELETE: three nodes are already clean
                countdown.append(300)

    def progress() -> int:
        if not countdown:
            return 0
        countdown[0] -= 1
        if countdown[0] > 0:
            return 0
        countdown.clear()                           # once, partway into that DELETE: SQLite stops it
        return 1

    db.conn.set_trace_callback(trace)
    db.conn.set_progress_handler(progress, 10)
    with pytest.raises(sqlite3.OperationalError, match="interrupted"):
        learning.prune_card_sources()
    db.conn.set_progress_handler(None, 0)
    db.conn.set_trace_callback(None)
    assert not db.conn.in_transaction
    rows = _rows(db)
    states = {node: "pruned" if rows[node] == pruned[node] else "untouched" if rows[node] == original[node] else "torn"
              for node in original}
    assert sorted(states.values()) == ["pruned"] * 3 + ["untouched"] * 3, states
    assert _ref_index(db)                                               # made again on the way out
    counts = learning.prune_card_sources()
    assert counts["cleaned"] == 3 and _rows(db) == pruned


def test_a_stop_request_ends_the_run_between_cards_and_the_next_run_finishes(store: Any) -> None:
    """A stop (Ctrl-C, FERB.app quitting) asked for during the clean-out takes effect before the next node."""
    db, spans, learning = store
    original, pruned = _three_tasks(db, spans, learning)
    asked: list[int] = []

    def stop() -> bool:
        asked.append(1)
        return len(asked) > 2                       # asked before each node: two are cleaned, then it stops
    counts = learning.prune_card_sources(stop)
    assert counts["stopped"] is True and counts["cleaned"] == 2 and _ref_index(db)
    rows = _rows(db)
    assert sum(rows[n] == pruned[n] for n in original) == 2 and sum(rows[n] == original[n] for n in original) == 4
    assert learning.prune_card_sources()["cleaned"] == 4 and _rows(db) == pruned


def test_compaction_runs_only_with_free_disk_for_the_database_and_its_wal(store: Any, tmp_path: Path) -> None:
    db, spans, learning = store
    _three_tasks(db, spans, learning)
    learning.prune_card_sources()
    db.truncate_wal("test")
    path = tmp_path / "ferb.db"
    size = store_size(path)
    statements: list[str] = []
    db.conn.set_trace_callback(statements.append)
    skipped = db.compact(free=lambda folder: size - 1)
    assert skipped == {"compacted": False, "needed": size, "free": size - 1}
    assert "VACUUM" not in statements and store_size(path) == size
    done = db.compact(free=lambda folder: size)
    assert done["compacted"] is True and "VACUUM" in statements
    assert store_size(path) < size
    assert db.conn.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
    assert db.conn.execute("PRAGMA journal_mode").fetchone()[0] == "wal"


def test_progress_is_told_after_each_card_version(store: Any) -> None:
    """Setup prints how far the clean-out is (it can take minutes): every version cleaned or left alone counts."""
    db, spans, learning = store
    _three_tasks(db, spans, learning)
    kept, shown = traced_task(db, spans, learning, "c0ffee04")
    copied_cards(learning, "c0ffee04", kept | shown, versions=2)
    purge_task_spans(db, "c0ffee04")                                     # two untraced versions
    told: list[tuple[int, int]] = []
    learning.prune_card_sources(progress=lambda done, total: told.append((done, total)))
    assert told[-1] == (8, 8) and [d for d, _ in told] == sorted(d for d, _ in told) and len(told) >= 6


def _ref_index(db: Database) -> bool:
    """`learning_source_ref` (migration 0030) is there, and a lookup by source ref uses it (forget, missing sources)."""
    plan = " ".join(str(r[3]) for r in db.conn.execute(
        "EXPLAIN QUERY PLAN SELECT node_id FROM learning_sources WHERE source_ref = ?", ("trail:x",)))
    return "learning_source_ref" in plan


def test_the_deletes_run_without_the_ref_index_which_is_made_again_after(store: Any) -> None:
    """Measured on a synthetic store of the owner's shape (23.3M rows): each node's delete rewrote ~50k random pages of
    `learning_source_ref`, ~7 s a node, about 47 minutes with the daemon stopped; without it the clean-out took 91.7 s
    (the drop's ~40 s included). It goes right before the first delete and is made again whatever happens, over the
    rows that are left."""
    db, spans, learning = store
    _three_tasks(db, spans, learning)
    statements: list[str] = []
    db.conn.set_trace_callback(statements.append)
    learning.prune_card_sources()
    db.conn.set_trace_callback(None)
    first_delete = next(i for i, sql in enumerate(statements) if sql.startswith("DELETE FROM learning_sources"))
    drop = next(i for i, sql in enumerate(statements) if sql.startswith("DROP INDEX IF EXISTS learning_source_ref"))
    make = max(i for i, sql in enumerate(statements) if sql.startswith("CREATE INDEX IF NOT EXISTS learning_source_ref"))
    last_delete = max(i for i, sql in enumerate(statements) if sql.startswith("DELETE FROM learning_sources"))
    assert drop < first_delete and last_delete < make and _ref_index(db)


def _drops(db: Database, learning: Learning) -> bool:
    """Whether a clean-out run dropped `learning_source_ref`; the index is there after it either way."""
    statements: list[str] = []
    db.conn.set_trace_callback(statements.append)
    learning.prune_card_sources()
    db.conn.set_trace_callback(None)
    assert _ref_index(db)
    return any(sql.startswith("DROP INDEX") for sql in statements)


def test_a_store_with_nothing_to_clean_keeps_its_ref_index(store: Any) -> None:
    """Untraced cards, cards the current composer wrote, and copied cards already cleaned (the retry after a "not
    compacted" run, or a stop after the last card): nothing to delete, so the index is never dropped or rebuilt."""
    db, spans, learning = store
    kept, shown = traced_task(db, spans, learning, "c0ffee02")
    copied_cards(learning, "c0ffee02", kept | shown, versions=1)
    purge_task_spans(db, "c0ffee02")                                     # an untraced card
    traced_task(db, spans, learning, "c0ffee01")
    learning.capture_episode("c0ffee01")                                 # a card the current composer wrote
    assert not _drops(db, learning)
    kept, shown = traced_task(db, spans, learning, "c0ffee03")
    copied_cards(learning, "c0ffee03", kept | shown)
    assert _drops(db, learning)                                          # copied cards: cleaned without it
    assert not _drops(db, learning)                                      # the re-run over them


def test_the_ref_index_is_made_whenever_it_is_missing(store: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Every run makes a missing index (one an earlier run lost after 0043 was applied), even one with nothing to
    clean, and even one an error ends before anything was dropped."""
    db, spans, learning = store
    kept, shown = traced_task(db, spans, learning, "c0ffee02")
    copied_cards(learning, "c0ffee02", kept | shown, versions=1)
    purge_task_spans(db, "c0ffee02")                                     # nothing to clean
    db.conn.execute("DROP INDEX learning_source_ref")
    learning.prune_card_sources()
    assert _ref_index(db)

    def broken(self: Learning, task_id: str) -> set[str] | None:
        raise sqlite3.OperationalError("disk I/O error")
    _three_tasks(db, spans, learning)
    db.conn.execute("DROP INDEX learning_source_ref")
    monkeypatch.setattr(Learning, "card_source_refs", broken)
    with pytest.raises(sqlite3.OperationalError, match="disk I/O error"):
        learning.prune_card_sources()
    assert _ref_index(db) and not db.conn.in_transaction


def test_a_stop_after_the_drop_says_the_index_is_being_made_again(store: Any) -> None:
    """After a stop the index is made over every row left, which can take a while with FERB stopped: `rebuilding` is
    told first. A full run, or one that never dropped it, says nothing."""
    db, spans, learning = store
    _three_tasks(db, spans, learning)
    told: list[str] = []
    asked: list[int] = []

    def stop() -> bool:
        asked.append(1)
        return len(asked) > 1                                            # after the first card
    counts = learning.prune_card_sources(stop, rebuilding=lambda: told.append("rebuild"))
    assert counts["stopped"] and told == ["rebuild"] and _ref_index(db)
    told.clear()
    learning.prune_card_sources(rebuilding=lambda: told.append("rebuild"))
    assert told == []                                                    # it finished: few rows left to index
    learning.prune_card_sources(lambda: True, rebuilding=lambda: told.append("rebuild"))
    assert told == []                                                    # stopped before any drop


def test_the_next_start_makes_the_ref_index_again_after_a_power_loss_mid_clean_out(store: Any) -> None:
    """A power loss between the drop and the remake leaves no index; the daemon's next start (its migrations) makes it
    again, so forgets and missing-source lookups never scan the whole table."""
    db, spans, learning = store
    db.conn.execute("DROP INDEX learning_source_ref")
    db.conn.execute("DELETE FROM schema_version WHERE version >= 43")    # the first start of the new code
    assert not _ref_index(db)
    assert 43 in db.migrate() and _ref_index(db)


def test_a_card_reads_its_trace_one_span_at_a_time(store: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """Span outputs have no size limit, so a card is read as the base read it: one parsed span at a time, never the
    whole tree held at once. `card_trace` parses a span only when it is asked for the next one, and `prepare_episode`
    and `card_source_refs` deal with each span (`_span_refs`) before they take the next."""
    import json
    db, spans, learning = store
    traced_task(db, spans, learning, "c0ffee01")
    parsed: list[int] = []
    real_loads = json.loads
    monkeypatch.setattr(json, "loads", lambda text, *a, **k: (parsed.append(1), real_loads(text, *a, **k))[1])
    trace = learning.card_trace("c0ffee01")
    assert parsed == []                                                  # nothing read or parsed yet
    next(trace)
    assert len(parsed) <= 2                                              # one span: its input, and a call's output
    monkeypatch.setattr(json, "loads", real_loads)
    events: list[str] = []
    real_trace, real_refs = Learning.card_trace, Learning._span_refs

    def watched_trace(self: Learning, task_id: str) -> Any:
        for span in real_trace(self, task_id):
            events.append("next")
            yield span

    def watched_refs(self: Learning, span: Any) -> set[str]:
        events.append("used")
        return real_refs(self, span)
    monkeypatch.setattr(Learning, "card_trace", watched_trace)
    monkeypatch.setattr(Learning, "_span_refs", watched_refs)
    for read in (lambda: learning.prepare_episode("c0ffee01"), lambda: learning.card_source_refs("c0ffee01")):
        events.clear()
        read()
        assert events == ["next", "used"] * 7, events                   # 7 spans, each used before the next is read
