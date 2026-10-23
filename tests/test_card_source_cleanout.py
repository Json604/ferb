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
from ferb.store.db import Database
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
        return int(countdown[0] <= 0)               # partway into that DELETE: SQLite stops it

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
    assert counts["stopped"] is True and counts["cleaned"] == 2
    rows = _rows(db)
    assert sum(rows[n] == pruned[n] for n in original) == 2 and sum(rows[n] == original[n] for n in original) == 4
    assert learning.prune_card_sources()["cleaned"] == 4 and _rows(db) == pruned


def _store_bytes(path: Path) -> int:
    return sum(p.stat().st_size for p in (path, path.with_name(path.name + "-wal")) if p.exists())


def test_compaction_runs_only_with_free_disk_for_the_database_and_its_wal(store: Any, tmp_path: Path) -> None:
    db, spans, learning = store
    _three_tasks(db, spans, learning)
    learning.prune_card_sources()
    db.truncate_wal("test")
    path = tmp_path / "ferb.db"
    size = _store_bytes(path)
    statements: list[str] = []
    db.conn.set_trace_callback(statements.append)
    skipped = db.compact(free=lambda folder: size - 1)
    assert skipped == {"compacted": False, "needed": size, "free": size - 1}
    assert "VACUUM" not in statements and _store_bytes(path) == size
    done = db.compact(free=lambda folder: size)
    assert done["compacted"] is True and "VACUUM" in statements
    assert _store_bytes(path) < size
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
