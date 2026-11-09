"""Owns age-based retention in one place: what goes when, table by table (STORE-400 and 3, target
design 1; SENSE-1), read first and deleted in short batches. Must not delete what nothing else preserves.

Kept forever (never named here): the task list and task results, episodes and how-notes, memory and learned nodes,
owner verdicts, schedules and reminders still to come, known destinations, conversations, and the journal's owner and
chat rows and day summaries.

Aged out:
- spans: a task tree's 30 days (`retention.trace_days`) after it ended, and only once its permanent episode exists
  (the daemon builds a missing one first); spans tied to no task 30 days after they started;
- operational records (`retention.operational_days`, 90) after their task or timer ended: the action ledger,
  confirmations, plans and their steps, loop state, a tree's shell download count, delivered or cancelled timers and their deliveries, and the
  journal's operational kinds (`OPERATIONAL_KINDS`). A record whose task isn't in the task list counts from its own
  last write; a journal row a memory fact cites is kept;
- Jev decisions after `retention.decision_days` (30);
- activity rows (ferb/sense/trail.py): accessibility after 7 days, the rest after `sense.trail_retention_days` (30) once
  their day's summary exists; the text of typed and copied rows, and the notice text derived from it, after
  `TYPED_DAYS` (7; the rows stay, ferb/sense/perception.py `purge_text`);
- evidence media and shell logs (7, ferb/act/evidence/store.py), recovery copies (7, ferb/act/fs/recovery.py), resource
  samples (30, ferb/daemon/resources.py): purged by the stores that own their files, in the same pass.

Reads that scan (the intake planner's spans by request id) run off the daemon's loop on a read-only connection
(`plan`); deletes run on the loop's own connection, `BATCH` rows a transaction, dependents first (`delete_rows`), so no
write lock is held long and a worker thread's write never waits on a whole purge (kb area daemon)."""

from dataclasses import dataclass, field
from typing import Any, Iterator

from ferb.store.spans import tree_where

MIN_DAYS = 1        # a retention of 0 days would purge what was just kept, before the checker or the owner could use it
OPERATIONAL_DAYS = 90                   # STORE-401 (owner, 2026-10-30)
ACCESSIBILITY_DAYS = 7                  # SENSE-1 (owner): accessibility rows are kept 7 days
TYPED_DAYS = 7                          # D14/P8 (owner): typed and copied text expires after 7 days
# Ruling (step 12a): rows a delete transaction takes; small enough that a write elsewhere waits a few milliseconds
BATCH = 200
# STORE-401's "operational journal kinds": the daemon's own bookkeeping, failures and progress records. Owner and chat
# kinds, day summaries, schedule, timer-set and memory records, and audit records (security, network, escape, clipboard)
# are not among them and stay (ruling, step 12a; the audit kinds are the owner's call, kb area storage).
OPERATIONAL_KINDS = frozenset({
    "daemon.started", "daemon.stopped", "daemon.slow_call", "daemon.internal_error", "daemon.unhandled",
    "daemon.background_failed", "daemon.loop_failed",
    "retention.purged", "retention.snapshot_purged", "retention.failed", "retention.downloads", "trail.purged",
    "store.wal_not_truncated", "resources.failed", "sense.watcher_failed",
    "voice.attached", "voice.switched", "browser.focus_returned", "screen.image_refused", "screen.capture_timeout",
    "screen.capture_stalled", "preview.slow", "preview.failed",
    "runtime.failed", "runtime.stale_runs_swept", "runtime.cache_skipped", "runtime.cache_fallback",
    "plan.step", "plan.repair", "plan.extended", "plan.tidied", "loop.round",
    "task.send_digest", "task.steering_delivered", "task.dispatch_failed", "tasks.interrupted_after_restart",
    "timer.failed", "attention.failed", "episode.failed", "mac.back_in_use",
    "memory.episode_failed", "memory.episode_deferred", "memory.episode_refs_damaged", "memory.episode_refs_expired",
    "memory.distill_failed", "memory.consolidate_failed", "memory.cards_failed", "memory.playbooks_failed",
    "memory.profile_failed", "memory.mac_profile_failed", "memory.search_reindex_failed",
})
# A task id's tree: the part before its first "." (tasks of a tree are <root>.<step>…)
_ROOT = "CASE WHEN instr({0}, '.') > 0 THEN substr({0}, 1, instr({0}, '.') - 1) ELSE {0} END"
_ENDED = ("SELECT id FROM tasks WHERE parent_id IS NULL AND status NOT IN ('running', 'planned', 'paused') "
          "AND updated_at < :cutoff")
_KNOWN = "SELECT id FROM tasks WHERE parent_id IS NULL"
_LIVE = ("SELECT id FROM tasks WHERE parent_id IS NULL AND (status IN ('running', 'planned', 'paused') "
         "OR updated_at >= :cutoff)")
# (table, the column naming its task or tree, its own last-write column or None): records that go with their task
_BY_TASK = (("action_ledger", "task_id", "updated_at"), ("confirmations", "tree_id", "at"),
            ("loop_state", "task_id", "updated_at"), ("shell_downloads", "tree_id", "at"))
# What goes with a deleted row: (dependent table, its column, the parent's column it names)
DEPENDENTS: dict[str, tuple[tuple[str, str, str], ...]] = {
    "task_plans": (("plan_steps", "task_id", "task_id"),),
    "timers": (("timer_deliveries", "timer_id", "id"),),
    "trail": (("perception_receipts", "event_id", "id"),),
}


def days(value: Any) -> int:
    """A `retention.*_days` setting in whole days, never under MIN_DAYS."""
    return max(MIN_DAYS, int(value))


DB = "FERB's database on this Mac (~/.ferb/ferb.db)"


@dataclass(frozen=True)
class Rule:
    """One row of what FERB keeps and for how long: the hourly pass reads its windows from here (`windows`), and the
    About page and FERB's self-description show the same rows (`describe`), so neither can drift from what runs.
    `window`: the pass's name for its days; `setting`: the config key (section, key) the owner changes, with `default`;
    no setting and a `default`: fixed days; neither: kept until `kept` says."""
    records: str
    where: str
    kept: str
    window: str | None = None
    setting: tuple[str, str] | None = None
    default: int | None = None


# The rule table (STORE-400 and STORE-401; SENSE-1). Evidence, recovery and resource samples are purged by their
# own stores in the same pass, with the days read here; the Keychain row is no retention rule but belongs in "what FERB
# keeps about you".
RULES: tuple[Rule, ...] = (
    Rule("your tasks, their results and your verdicts", DB, "forever"),
    Rule("what each task did (episodes) and how-notes", DB, "forever"),
    Rule("what FERB remembers about you and what it learned", DB, "until you forget it (Memory page)"),
    Rule("your chats with FERB, notices and day summaries", DB, "forever (a notice's text from typed or copied text: "
         "with that text)"),
    Rule("schedules, reminders still to come and the sites and people you approved", DB,
         "until you remove them (a reminder that went off or was cancelled goes with the operational records)"),
    Rule("each task's full trace (every model and tool call)", DB, "after the task ends, once its episode exists",
         "trace_days", ("retention", "trace_days"), 30),
    Rule("operational records (actions taken, confirmations, plans, loop state, finished reminders and timers, FERB's "
         "own bookkeeping)", DB, "after their task or timer ends", "operational_days", ("retention", "operational_days"),
         OPERATIONAL_DAYS),
    Rule("Jev's decisions (Jev is switched off)", DB, "after they were made", "decision_days",
         ("retention", "decision_days"), 30),
    Rule("text read from apps on screen (accessibility)", DB, "after it was read", None, None, ACCESSIBILITY_DAYS),
    Rule("text you typed or copied, and notice text that came from it", DB,
         "after it was typed or copied (the record that you typed or copied stays)", None, None, TYPED_DAYS),
    Rule("other activity on this Mac (apps, windows, pages, files, notifications)", DB,
         "once that day's summary exists", "trail_days", ("sense", "trail_retention_days"), 30),
    Rule("screenshots, evidence and shell output", "~/.ferb/evidence and ~/.ferb/logs/shell on this Mac",
         "after they were captured", "evidence_days", ("retention", "evidence_days"), 7),
    Rule("copies of files before FERB changed them, records of what it moved to the Trash, disk snapshots",
         "~/.ferb/recovery on this Mac (snapshots: your disk)", "after FERB made them", "recovery_days",
         ("retention", "recovery_days"), 7),
    Rule("FERB's own CPU and memory use", DB, "after it was measured", "resource_days",
         ("retention", "resource_days"), 30),
    Rule("secrets and card details you saved", "your login Keychain (never the database)",
         "until you delete them (Settings → Data)"),
)


def windows(get: Any) -> dict[str, int]:
    """The pass's windows in days from config (`get(section, key, default)`), each at least MIN_DAYS."""
    return {str(r.window): days(get(r.setting[0], r.setting[1], r.default)) for r in RULES
            if r.window and r.setting and r.default is not None}


def describe(get: Any) -> list[dict[str, Any]]:
    """The rule table as the owner reads it: what, where, for how long (days from config), and the setting that
    changes it."""
    out: list[dict[str, Any]] = []
    for r in RULES:
        n = days(get(r.setting[0], r.setting[1], r.default)) if r.setting and r.default is not None else r.default
        out.append({"records": r.records, "where": r.where,
                    "kept": f"{n} day{'s' if n != 1 else ''} {r.kept}" if n is not None else r.kept,
                    **({"days": n} if n is not None else {}),
                    **({"setting": ".".join(r.setting)} if r.setting else {})})
    return out


@dataclass
class Plan:
    """What one pass deletes, read before anything is deleted: each old tree's spans (children before parents), loose
    spans, Jev decisions and operational rows by table (rowids). `left`: old trees waiting for the next pass."""
    roots: list[str] = field(default_factory=list)
    trees: dict[str, list[str]] = field(default_factory=dict)
    loose: list[str] = field(default_factory=list)
    decisions: list[int] = field(default_factory=list)
    operational: dict[str, list[int]] = field(default_factory=dict)
    left: int = 0


def plan(conn: Any, trace_cutoff: str, decision_cutoff: str, operational_cutoff: str, roots_limit: int) -> Plan:
    """Read what a pass deletes (any connection; the daemon reads it off its loop). At most `roots_limit` old trees,
    oldest first; the rest are counted in `left`."""
    roots = finished_roots_before(conn, trace_cutoff)
    out = Plan(roots=roots[:roots_limit], left=max(0, len(roots) - roots_limit))
    requests = {r[0]: r[1] for r in conn.execute(
        f"SELECT id, request_id FROM tasks WHERE id IN ({','.join('?' * len(out.roots))})", out.roots)}
    # the intake planner's spans carry the request id, not a task id: one scan for every tree of the pass
    intake: dict[str, list[str]] = {}
    if requests:
        wanted = set(requests.values())
        for ident, request in conn.execute("SELECT id, json_extract(input_json, '$.request_id') FROM spans "
                                           "WHERE task_id IS NULL"):
            if request in wanted:
                intake.setdefault(request, []).append(ident)
    for root in out.roots:
        where, params = tree_where(root)
        seeds = [r[0] for r in conn.execute(f"SELECT id FROM spans WHERE {where}", params)]
        out.trees[root] = _with_descendants(conn, seeds + intake.get(requests.get(root, ""), []))
    loose = [r[0] for r in conn.execute(
        "SELECT id FROM spans WHERE task_id IS NULL AND parent_id IS NULL AND started_at < ? AND "
        "(json_extract(input_json, '$.request_id') IS NULL OR "
        " json_extract(input_json, '$.request_id') NOT IN (SELECT request_id FROM tasks))", (trace_cutoff,))]
    out.loose = _with_descendants(conn, loose)
    out.decisions = [r[0] for r in conn.execute("SELECT rowid FROM decisions WHERE at < ?", (decision_cutoff,))]
    out.operational = operational(conn, operational_cutoff)
    return out


def _with_descendants(conn: Any, seeds: list[str]) -> list[str]:
    """These spans and every span under them, deepest first: a parent is never deleted before its child
    (spans.parent_id is a foreign key)."""
    depth: dict[str, int] = {ident: 0 for ident in seeds}
    frontier, level = list(depth), 0
    while frontier:
        level += 1
        children: list[str] = []
        for i in range(0, len(frontier), 500):
            chunk = frontier[i:i + 500]
            children += [r[0] for r in conn.execute(
                f"SELECT id FROM spans WHERE parent_id IN ({','.join('?' * len(chunk))})", chunk)]
        frontier = [c for c in children if depth.get(c, -1) < level]     # walked again when found deeper
        depth.update((c, level) for c in frontier)
    return sorted(depth, key=lambda ident: -depth[ident])


def operational(conn: Any, cutoff: str) -> dict[str, list[int]]:
    """STORE-401: the rowids of operational records whose task or timer ended before `cutoff`, by table."""
    args = {"cutoff": cutoff}
    found: dict[str, list[int]] = {}
    for table, column, written in _BY_TASK:
        root = _ROOT.format(column)
        found[table] = [r[0] for r in conn.execute(
            f"SELECT rowid FROM {table} WHERE {root} IN ({_ENDED}) OR ({root} NOT IN ({_KNOWN}) AND {written} < :cutoff)",
            args)]
    found["task_plans"] = [r[0] for r in conn.execute(
        f"SELECT rowid FROM task_plans WHERE {_ROOT.format('task_id')} IN ({_ENDED}) "
        "AND status NOT IN ('running', 'suspended', 'steps_done')", args)]
    found["timers"] = [r[0] for r in conn.execute(
        "SELECT rowid FROM timers WHERE state IN ('delivered', 'cancelled') AND COALESCE(delivered_at, fire_at) < :cutoff",
        args)]
    found["timer_deliveries"] = [r[0] for r in conn.execute(      # one left by a timer already gone
        "SELECT rowid FROM timer_deliveries WHERE at < :cutoff AND timer_id NOT IN (SELECT id FROM timers)", args)]
    task = _ROOT.format("json_extract(data_json, '$.task_id')")
    kinds = {f"k{i}": kind for i, kind in enumerate(sorted(OPERATIONAL_KINDS))}
    found["journal"] = [r[0] for r in conn.execute(
        f"SELECT rowid FROM journal WHERE kind IN ({','.join(':' + k for k in kinds)}) "
        "AND julianday(at) < julianday(:cutoff) "
        f"AND (json_extract(data_json, '$.task_id') IS NULL OR {task} NOT IN ({_LIVE})) "
        "AND id NOT IN (SELECT source_id FROM memory_sources WHERE kind = 'journal')", args | kinds)]
    return {table: ids for table, ids in found.items() if ids}


def finished_roots_before(conn: Any, cutoff: str) -> list[str]:
    """Root tasks that ended before the cutoff and still have spans, oldest first (never a running or planned
    one)."""
    return [r[0] for r in conn.execute(
        "SELECT id FROM tasks t WHERE parent_id IS NULL AND status NOT IN ('running', 'planned', 'paused') AND updated_at < ? "
        "AND EXISTS (SELECT 1 FROM spans WHERE task_id = t.id OR (task_id >= t.id || '.' AND task_id < t.id || '/')) "
        "ORDER BY updated_at, rowid",
        (cutoff,))]


def delete_rows(db: Any, table: str, key: str, ids: list[Any], batch: int = BATCH) -> Iterator[int]:
    """Delete `table`'s rows whose `key` is in `ids`, in the order given, `batch` rows a transaction, each batch's
    dependents (DEPENDENTS) in the same transaction first; yields each batch's count, so the daemon's loop runs between
    batches. A failed batch is rolled back whole and raises. Table and column names are FERB's own constants."""
    for i in range(0, len(ids), batch):
        chunk = ids[i:i + batch]
        marks = ",".join("?" * len(chunk))
        db.conn.execute("BEGIN IMMEDIATE")
        try:
            for dependent, column, parent in DEPENDENTS.get(table, ()):
                db.conn.execute(f"DELETE FROM {dependent} WHERE {column} IN "
                                f"(SELECT {parent} FROM {table} WHERE {key} IN ({marks}))", chunk)
            n = int(db.conn.execute(f"DELETE FROM {table} WHERE {key} IN ({marks})", chunk).rowcount)
            db.conn.execute("COMMIT")
        except BaseException:
            if db.conn.in_transaction:            # SQLite may have rolled back already; keep the real error
                db.conn.execute("ROLLBACK")
            raise
        yield n
