"""Owns the one-time clean-up of data no code uses any more (STORE-402), run by setup with the daemon
stopped, only after the owner's OK on exactly what it removes. Must not touch memory, episodes, chat, tasks, schedules
or traces still in their window.

What it removes (`preview` lists each with its row count; nothing else is ever deleted):
- the data of switched-off code, whose tables keep their schema (SENSE-2): Jev's decisions, perception
  receipts, batches and labels, notification receipts, and Jev's relevance labels on activity rows;
- data of removed code: the old embedding model's vectors and evaluations (step 9b), the memory refs tasks were given
  (writers removed in step 3), and the day/week/month rollup nodes (step 3) with their source rows;
- episode cards' source rows the current card rule no longer gives them (`Learning.card_source_plan`);
- tables no code reads or writes any more (`DROPPED`): dropped, with whatever rows they hold, only here (until then an
  install keeps them, unread).
Then the file is compacted once with incremental auto-vacuum (`Database.compact`, only with the free disk
`compaction_needs` gives: about twice what the database holds), so later hourly passes hand free pages back to the
disk.

The owner's OK is the preview's `ok` digest: `apply` reads the plan again with the daemon stopped and refuses, deleting
nothing, when it differs from what the owner saw (its steps, the compaction, and the rows of each step only the
clean-up changes: a running daemon's hourly pass also trims Jev's decisions, perception receipts and relevance labels,
and its new cards and aging traces change the card source count, so those steps are approved by name, not by count).
Deletes run in batches, each its own transaction, so a stop or a crash keeps what was done and the next run finishes.
The counts are journaled (`store.cleaned`), and so is the compaction (`store.compacted`)."""

import hashlib
import json
import shutil
from pathlib import Path
from typing import Any, Callable

from ferb.store.db import compaction_needs, store_size

CLEANED = "store.cleaned"            # journal: what a run removed (counts only)
COMPACTED = "store.compacted"        # journal: the compaction (sizes only)
ROLLUP_MAKER = "code:rollup/1"
# Ruling (step 12a): rows a clean-up transaction takes (the daemon is stopped: larger than the hourly pass's batches)
BATCH = 5000
# (table, what it held): emptied whole, schema kept
EMPTIED = (
    ("decisions", "Jev's decisions (Jev is switched off)"),
    ("perception_receipts", "perception batch receipts (Jev is switched off)"),
    ("perception_batches", "perception batches (Jev is switched off)"),
    ("perception_labels", "perception labels (Jev is switched off)"),
    ("notification_told", "notification receipts (Jev is switched off)"),
    ("learning_vectors", "the old embedding model's vectors (replaced in step 9b)"),
    ("learning_embedding_evals", "the old embedding model's evaluations (replaced in step 9b)"),
    ("task_memory_refs", "the memory refs tasks were given (no longer written or read)"),
)
# (table, what it held): no code reads or writes it any more; the run drops the table itself
DROPPED = (
    ("owner_questions", "folder-access questions (never used)"),
    ("notification_rules", "notification rules (removed in step 3)"),
    ("learning_receipts", "procedure receipts (learning removed in step 3)"),
    ("learning_jobs", "learning job states (learning removed in step 3)"),
)
# Tables of EMPTIED whose rows the hourly retention pass also trims while the daemon runs (decisions after 30 days,
# perception receipts with their activity rows): their steps are `live` (see `plan`)
TRIMMED = frozenset({"decisions", "perception_receipts"})
# Never written by a run (the test asserts it): what STORE-402 protects and what FERB keeps forever
PROTECTED = ("memory_items", "memory_sources", "memory_profile", "memory_context", "memory_set_aside",
             "memory_import_runs", "memory_import_done", "memory_import_items", "memory_forgotten", "forgotten_refs",
             "episodes", "episode_how", "journal", "tasks", "task_results", "task_result_cards", "task_result_names", "task_context",
             "schedules", "schedule_ends", "schedule_names", "timers", "timer_names", "conversations", "owner_verdicts", "spans", "trail",
             "known_destinations", "standing_permissions", "recall_vectors", "chat_fts", "learning_nodes", "chat_history")
# The protected rows of a table a run also removes from: every learned node (episode cards, playbooks, how-notes) but
# the old rollups
PROTECTED_ROWS = {"learning_nodes": f"made_by IS NOT '{ROLLUP_MAKER}'"}


def _count(conn: Any, sql: str, *params: Any) -> int:
    return int(conn.execute(sql, params).fetchone()[0])


def _learning(db: Any) -> Any:
    from ferb.clock.clock import SystemClock
    from ferb.memory.learning import Learning
    return Learning(db, SystemClock(), lambda *args, **kwargs: None, lambda text: text)


def plan(db: Any) -> dict[str, Any]:
    """What a run would remove, read without writing (`db`: anything with a `conn`, a read-only one included):
    `steps` (table, rows, what; `live` when the running daemon's hourly pass also changes its rows), `rows` in all,
    `compact` (the file still needs its one-time compaction), and `ok`, the digest the owner's OK carries. `cards` is
    the card plan `apply` follows."""
    conn = db.conn
    steps: list[dict[str, Any]] = []
    for table, what in EMPTIED:
        if n := _count(conn, f"SELECT COUNT(*) FROM {table}"):
            steps.append({"table": table, "rows": n, "what": what} | ({"live": True} if table in TRIMMED else {}))
    if n := _count(conn, "SELECT COUNT(*) FROM trail WHERE relevance IS NOT NULL"):     # activity rows age out hourly
        steps.append({"table": "trail.relevance", "rows": n, "what": "Jev's relevance labels on activity rows (only "
                                                                     "the label; the rows stay)", "live": True})
    if n := _count(conn, "SELECT COUNT(*) FROM learning_nodes WHERE made_by = ?", ROLLUP_MAKER):
        sources = _count(conn, "SELECT COUNT(*) FROM learning_sources WHERE node_id IN (SELECT id FROM learning_nodes "
                               "WHERE made_by = ?)", ROLLUP_MAKER)
        changes = _count(conn, "SELECT COUNT(*) FROM learning_changes WHERE node_id IN (SELECT id FROM learning_nodes "
                               "WHERE made_by = ?)", ROLLUP_MAKER)
        steps.append({"table": "learning_nodes", "rows": n, "what": "day/week/month rollups (removed in step 3)"})
        if sources:
            steps.append({"table": "learning_sources", "rows": sources, "what": "the rollups' source rows"})
        if changes:
            steps.append({"table": "learning_changes", "rows": changes, "what": "the rollups' change history"})
    rollups = [r[0] for r in conn.execute("SELECT id FROM learning_nodes WHERE made_by = ?", (ROLLUP_MAKER,))]
    cards = _learning(db).card_source_plan(removing=rollups)        # as they will be once the rollups are gone
    if n := sum(map(len, cards["drop"].values())):
        steps.append({"table": "learning_sources", "rows": n,
                      "what": f"episode cards' source rows the current card rule no longer gives them "
                              f"({len(cards['drop'])} of {cards['nodes']} card versions)",
                      "live": True})                         # new cards, and traces aging out, change these counts
    names = {r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'")}
    for table, what in DROPPED:
        if table in names:
            steps.append({"table": table, "rows": _count(conn, f"SELECT COUNT(*) FROM {table}"),
                          "what": what + "; the table is dropped", "drop": True})
    compact = int(conn.execute("PRAGMA auto_vacuum").fetchone()[0]) != 2
    # What the OK approves: every step and the compaction, with the exact rows of each step nothing else changes; a
    # `live` step (rows the hourly pass trims, or cards it adds, between the preview and the OK) by its table only
    approved = {"steps": [{"table": s["table"], "live": True} if s.get("live") else s for s in steps],
                "compact": compact}
    return {"steps": steps, "compact": compact, "rows": sum(s["rows"] for s in steps),
            "ok": hashlib.sha256(json.dumps(approved, sort_keys=True).encode()).hexdigest()[:16], "cards": cards}


def preview(path: Path, free: Callable[[Path], int] = lambda folder: shutil.disk_usage(folder).free) -> dict[str, Any]:
    """`plan` on a read-only connection (a daemon may be running), with the file's size, the free disk the compaction
    needs (`needed`, read before anything is removed, so at most what it will need) and the disk's free space, without
    the card plan: what the app shows the owner."""
    import sqlite3

    class ReadOnly:
        def __init__(self) -> None:
            self.path = path
            self.conn = sqlite3.connect(path.absolute().as_uri() + "?mode=ro", uri=True)
            self.conn.row_factory = sqlite3.Row
    db = ReadOnly()
    try:
        out = plan(db)
        needed = compaction_needs(db.conn)
    finally:
        db.conn.close()
    out.pop("cards")
    return out | {"bytes": store_size(path), "needed": needed, "free": free(path.parent)}


def pending(path: Path) -> bool:
    """Whether the clean-up has something to do, cheaply (doctor): data of switched-off or removed code (a dropped
    table's rows included; an empty one waits, unread, for whenever the clean-up runs), rollups, or a file not compacted
    yet. Read-only; a store it can't read has nothing to do."""
    import sqlite3
    if not path.exists():
        return False
    try:
        conn = sqlite3.connect(path.absolute().as_uri() + "?mode=ro", uri=True)
    except sqlite3.Error:
        return False
    try:
        if int(conn.execute("PRAGMA auto_vacuum").fetchone()[0]) != 2:
            return True
        names = {r[0] for r in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'")}
        tables = [t for t, _ in EMPTIED] + [t for t, _ in DROPPED if t in names]
        return any(conn.execute(f"SELECT EXISTS (SELECT 1 FROM {table})").fetchone()[0] for table in tables) or \
            bool(conn.execute("SELECT EXISTS (SELECT 1 FROM learning_nodes WHERE made_by = ?)",
                              (ROLLUP_MAKER,)).fetchone()[0])
    except sqlite3.Error:
        return False
    finally:
        conn.close()


def apply(db: Any, journal: Any, ok: str, *, stop: Callable[[], bool] = lambda: False,
          say: Callable[[str], None] = lambda line: None,
          free: Callable[[Path], int] = lambda folder: shutil.disk_usage(folder).free) -> dict[str, Any]:
    """The clean-up itself, on a connection nothing else shares (setup, the daemon stopped). Refused (`refused`,
    nothing written) unless `ok` is the digest of the plan read now. Each step deletes in batches; `stop`, asked
    between batches, ends the run there (journaled with `stopped`; the next run finishes). Then the compaction, when
    the disk has room. Returns `removed` (rows by step), `stopped`, and the compaction's result."""
    now = plan(db)
    if ok != now["ok"]:
        return {"refused": True, "plan": {k: v for k, v in now.items() if k != "cards"}}
    removed: dict[str, int] = {}
    dropped: list[str] = []
    conn = db.conn

    def batches(label: str, sql: str, *params: Any) -> bool:
        """Run `sql` (a statement touching at most BATCH rows) until it touches none; False when stopped."""
        while True:
            if stop():
                return False
            conn.execute("BEGIN IMMEDIATE")
            try:
                n = int(conn.execute(sql, params).rowcount)
                conn.execute("COMMIT")
            except BaseException:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                raise
            removed[label] = removed.get(label, 0) + n
            if n < BATCH:
                return True

    def run() -> bool:
        cards = now["cards"]
        if cards["drop"]:                     # first: the deletes that take longest, stoppable between cards
            say(f"Pruning the source rows of {len(cards['drop'])} episode card versions…")
            counts = _learning(db).prune_card_sources(stop=stop, plan=cards)
            removed["learning_sources (cards)"] = counts["rows"]
            if counts["stopped"]:
                return False
        for table, _ in EMPTIED:
            say(f"Emptying {table}…")
            if not batches(table, f"DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} LIMIT {BATCH})"):
                return False
        if not batches("trail.relevance", "UPDATE trail SET relevance = NULL WHERE rowid IN (SELECT rowid FROM trail "
                                          f"WHERE relevance IS NOT NULL LIMIT {BATCH})"):
            return False
        say("Removing the old rollup nodes…")
        rollups = f"SELECT id FROM learning_nodes WHERE made_by = '{ROLLUP_MAKER}'"
        if not batches("learning_changes (rollups)", f"DELETE FROM learning_changes WHERE rowid IN (SELECT rowid FROM "
                                                     f"learning_changes WHERE node_id IN ({rollups}) LIMIT {BATCH})"):
            return False
        before = _count(conn, "SELECT COUNT(*) FROM learning_sources")
        if not batches("learning_nodes (rollups)", f"DELETE FROM learning_nodes WHERE rowid IN (SELECT rowid FROM "
                                                   f"learning_nodes WHERE made_by = '{ROLLUP_MAKER}' LIMIT {BATCH})"):
            return False
        removed["learning_sources (rollups)"] = before - _count(conn, "SELECT COUNT(*) FROM learning_sources")
        for step in now["steps"]:
            if not step.get("drop"):
                continue
            if stop():
                return False
            say(f"Dropping {step['table']}…")
            conn.execute("BEGIN IMMEDIATE")
            try:
                rows = _count(conn, f"SELECT COUNT(*) FROM {step['table']}")
                conn.execute(f"DROP TABLE IF EXISTS {step['table']}")
                conn.execute("COMMIT")
            except BaseException:
                if conn.in_transaction:
                    conn.execute("ROLLBACK")
                raise
            removed[step["table"]] = rows
            dropped.append(step["table"])
        return True

    finished = run()
    counts = {k: v for k, v in removed.items() if v}
    journal.append(CLEANED, {"removed": counts, "rows": sum(counts.values()), "dropped": dropped}
                   | ({} if finished else {"stopped": True}))
    out: dict[str, Any] = {"removed": counts, "dropped": dropped, "stopped": not finished}
    if not finished or stop():                # VACUUM can't be stopped halfway: the next run compacts
        return out | {"compacted": False}
    if not now["compact"] and not counts and not dropped:
        return out | {"compacted": False}
    before = store_size(db.path)
    say("Compacting FERB's database (it writes a full copy, so it can take a few minutes)…")
    compaction = db.compact(free)
    out |= compaction
    if compaction["compacted"]:
        after = store_size(db.path)
        journal.append(COMPACTED, {"bytes_before": before, "bytes_after": after})
        out |= {"bytes_before": before, "bytes_after": after}
    return out
