"""Own the retention pass (D21; notes/storage.md decisions 1 and 3) and the resource log's samples (L30). The rules are
ferb/store/retention.py's; this runs them hourly. Must not purge the task list, episodes, chat, results or memory."""

import asyncio
import sqlite3
import sys
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Iterator, NamedTuple

from ferb.act.fs import recovery
from ferb.daemon import resources as resource_use
from ferb.orchestrator import episodes
from ferb.store import retention
from ferb.store.db import BUSY_TIMEOUT_MS, VACUUM_PASS_PAGES, VACUUM_SLICE_PAGES


ROOTS_PER_PURGE = 200


class _PurgePlan(NamedTuple):
    """One retention pass: its clock, its windows, and what it deletes (read before anything is deleted)."""
    now: datetime
    trace_days: int
    decision_days: int
    evidence_days: int
    recovery_days: int
    resource_days: int
    operational_days: int
    trail_days: int
    rows: retention.Plan

    @property
    def roots(self) -> list[str]:
        return self.rows.roots

    @property
    def left(self) -> int:
        return self.rows.left


def _read_plan(path: Path, *cutoffs: Any) -> retention.Plan:
    """The pass's reads on a read-only connection of their own, in a worker thread: the intake planner's spans are
    found by a scan, which must never hold the loop."""
    conn = sqlite3.connect(path.absolute().as_uri() + "?mode=ro", uri=True)
    try:
        conn.execute(f"PRAGMA busy_timeout={BUSY_TIMEOUT_MS}")
        return retention.plan(conn, *cutoffs)
    finally:
        conn.close()


class Retention:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _purge_records(self) -> dict[str, int]:
        """One synchronous pass (tests, call sites): the same steps as the hourly `_purge_pass`, without yielding."""
        plan = self._purge_plan()
        counts = self._new_counts()
        for _ in self._purge_steps(plan, counts):
            pass
        return self._purge_rest(plan, counts)

    async def _purge_pass(self) -> dict[str, int]:
        """The hourly pass: what goes is read off the loop (`_read_plan`), then deleted on the loop's connection in
        short batches with the loop free between them, then the free pages handed back and the WAL cut back. If
        shutdown cancels it, what it already purged is still journaled (`stopped`)."""
        assert self.db is not None
        plan = self._purge_plan(await asyncio.to_thread(_read_plan, self.db.path, *self._cutoffs()))
        counts = self._new_counts()
        finished = False
        try:
            for _ in self._purge_steps(plan, counts):
                await asyncio.sleep(0)
            finished = True
        finally:
            if not finished and any(counts[k] for k in ("spans", "decisions", "operational", "trail")):
                self._journal_purged(plan, counts, plan.left + len(plan.roots) - counts["roots_done"], stopped=True)
        result = self._purge_rest(plan, counts)              # journaled before the slow steps
        await self._give_back_space()
        result["snapshots"] = await self._purge_snapshots(plan)
        return result

    @staticmethod
    def _new_counts() -> dict[str, int]:
        return {"spans": 0, "decisions": 0, "operational": 0, "trail": 0, "roots_done": 0}

    def _windows(self) -> dict[str, int]:
        return retention.windows(self.config.get)          # the rule table's windows, each at least 1 day

    def _cutoffs(self, now: datetime | None = None) -> tuple[str, str, str, int]:
        now = now or self.clock.now()
        w = self._windows()
        return ((now - timedelta(days=w["trace_days"])).isoformat(),
                (now - timedelta(days=w["decision_days"])).isoformat(),
                (now - timedelta(days=w["operational_days"])).isoformat(), ROOTS_PER_PURGE)

    def _purge_plan(self, rows: retention.Plan | None = None) -> _PurgePlan:
        """The pass's clock and windows, with what it deletes (`rows`, else read now on the loop's connection)."""
        now = self.clock.now()
        if rows is None:
            rows = retention.plan(self.db.conn, *self._cutoffs(now))
        return _PurgePlan(now=now, rows=rows, **self._windows())

    def _purge_steps(self, plan: _PurgePlan, counts: dict[str, int]) -> Iterator[None]:
        """Every delete of the pass, one short batch per step: old trees (each once its episode exists), loose
        spans, Jev decisions, operational records, then activity rows. `counts` holds what went so far."""
        for root in plan.roots:
            batches = iter(self._purge_root(root, plan))
            n = next(batches, None)                          # one batch runs between two steps
            while True:
                counts["spans"] += n or 0
                n = next(batches, None)
                if n is None:
                    counts["roots_done"] += 1                # counted before its last step: a stop then knows it
                    yield
                    break
                yield
        for n in retention.delete_rows(self.db, "spans", "id", plan.rows.loose):
            counts["spans"] += n
            yield
        for n in retention.delete_rows(self.db, "decisions", "rowid", plan.rows.decisions):
            counts["decisions"] += n
            yield
        for table, ids in plan.rows.operational.items():
            for n in retention.delete_rows(self.db, table, "rowid", ids):
                counts["operational"] += n
                counts[table] = counts.get(table, 0) + n
                yield
        yield from self._purge_trail(plan, counts)

    def _purge_root(self, root: str, plan: _PurgePlan) -> Iterator[int]:
        """One tree: its spans (as the plan read them) go only once its episode exists (built now if missing), in
        batches (each batch's count yielded). Isolated: a failure is journaled and the pass goes on."""
        try:
            if episodes.get(self.db, root) is None:          # build it while the spans still exist
                episodes.record(self.spans, self._journal(), root, plan.now.isoformat())
            if episodes.get(self.db, root) is None:
                return                                       # no episode: keep the spans
            if self.tasks.column(root, "status") in ("running", "planned", "paused"):
                return                                       # resumed since the pass read it
            yield from retention.delete_rows(self.db, "spans", "id", plan.rows.trees.get(root, []))
        except Exception as e:
            self._journal().append("retention.failed", {"task_id": root, "error": f"{type(e).__name__}: {e}"})

    def _purge_trail(self, plan: _PurgePlan, counts: dict[str, int]) -> Iterator[None]:
        """Activity rows: accessibility after 7 days (memory decision 3), the rest after `sense.trail_retention_days`
        once their day's summary exists. Journaled `trail.purged` when any went."""
        if getattr(self, "trail", None) is None:
            return
        try:
            gone = 0
            for n in self.trail.purge_source_steps("accessibility", retention.ACCESSIBILITY_DAYS):
                gone += n
                yield
            if gone:
                self._journal().append("trail.purged", {"source": "accessibility", "rows": gone})
            counts["trail"] += gone
            purged, _, steps = self.trail.purge_steps(plan.trail_days)
            for n in steps:
                counts["trail"] += n
                yield
            if purged:
                self._journal().append("trail.purged", {"days": purged})
        except Exception as e:
            self._journal().append("retention.failed", {"trail": True, "error": f"{type(e).__name__}: {e}"})

    async def _give_back_space(self) -> None:
        """After the hourly deletes: free pages handed back to the disk a slice at a time (a store compacted with
        incremental auto-vacuum; `ferb setup --cleanup` does that once), then the WAL checkpointed and cut back.
        Never fails the pass."""
        try:
            if self.db.incremental():
                handed = 0
                while handed < VACUUM_PASS_PAGES and self.db.vacuum_slice(VACUUM_SLICE_PAGES):
                    handed += VACUUM_SLICE_PAGES
                    await asyncio.sleep(0)
            self.db.checkpoint()
        except sqlite3.Error as e:
            self._journal().append("retention.failed", {"vacuum": True, "error": f"{type(e).__name__}: {e}"})

    async def _purge_snapshots(self, plan: _PurgePlan) -> int:
        """FERB's own snapshots older than recovery_days (S1): each deletion takes seconds (measured 15.5 s) and is
        journaled as it happens, so shutdown can cancel the pass at any point without losing the record; a tmutil
        run already started is left to finish on its own."""
        kept = self._recovery()

        def gone(name: str, deleted: bool) -> None:
            self._journal().append("retention.snapshot_purged", {"snapshot": name, "deleted": deleted,
                                                                 "recovery_days": plan.recovery_days})
        return await recovery.purge_snapshots((plan.now - timedelta(days=plan.recovery_days)).isoformat(), kept.db,
                                              self.snapshot_deleter, gone)

    def _purge_rest(self, plan: _PurgePlan, counts: dict[str, int]) -> dict[str, int]:
        """Old evidence and old recovery copies and records (a trash record stays while its item is in the Trash,
        TRASH_RECORD_MAX_DAYS at most); snapshots are the async pass's last step. `retention.purged` is journaled only
        when the pass purged something or left roots for the next one. Old resource samples (L30, numbers only) go
        too, counted in the result, not journaled."""
        evidence = self.evidence.purge_before((plan.now - timedelta(days=plan.evidence_days)).isoformat()) \
            if self.evidence is not None else 0
        kept = self._recovery()
        copies = recovery.purge_before(
            (plan.now - timedelta(days=plan.recovery_days)).isoformat(), kept.db, kept.root,
            trash_backstop_iso=(plan.now - timedelta(days=recovery.TRASH_RECORD_MAX_DAYS)).isoformat())
        counts = counts | {"evidence": evidence, "recovery": copies}
        if any(counts[k] for k in ("spans", "decisions", "operational", "evidence", "recovery")) or plan.left:
            self._journal_purged(plan, counts, plan.left)
        trashed = self._purge_downloads()
        if self.perception is not None:
            self.perception.purge_text()
        return {"spans": counts["spans"], "decisions": counts["decisions"], "operational": counts["operational"],
                "trail": counts["trail"], "evidence": evidence, "recovery": copies, "downloads": trashed,
                "resources": self._purge_resources(plan), "shell_logs": self._purge_shell_logs(plan)}

    def _purge_shell_logs(self, plan: _PurgePlan) -> int:
        """Shell output logs (redacted, owner-only) of finished commands older than evidence_days (7): output is
        evidence of what a command did, kept as long as screenshots. Isolated: a failure is journaled and the pass
        goes on."""
        if self.shells is None:
            return 0
        try:
            return self.shells.purge_logs((plan.now - timedelta(days=plan.evidence_days)).timestamp())
        except Exception as e:
            self._journal().append("retention.failed", {"shell_logs": True, "error": f"{type(e).__name__}: {e}"})
            return 0

    def _purge_resources(self, plan: _PurgePlan) -> int:
        """L30: resource samples older than resource_days, by local day. Isolated: a failure is journaled and the pass
        goes on."""
        try:
            return resource_use.purge(self.db, plan.now.astimezone().date(), plan.resource_days)
        except Exception as e:
            self._journal().append("retention.failed", {"resources": True, "error": f"{type(e).__name__}: {e}"})
            return 0

    def _purge_downloads(self) -> int:
        """D35: when FERB's downloads folder passes browser.download_total_gb, its oldest working copies go to the
        Trash (no time-based clearing, owner 2026-10-09); their records stay. Journaled when any moved."""
        if self.downloads is None:
            return 0
        try:
            trashed = self.downloads.purge()
        except Exception as e:
            self._journal().append("retention.failed", {"downloads": True, "error": f"{type(e).__name__}: {e}"})
            return 0
        if trashed:
            self._journal().append("retention.downloads", {
                "trashed": trashed, "total_mb": self.downloads.limits().total_bytes // (1024 * 1024)})
        return trashed

    def _journal_purged(self, plan: _PurgePlan, counts: dict[str, int], left: int, stopped: bool = False) -> None:
        operational = {k: v for k, v in counts.items() if k in plan.rows.operational and v}
        self._journal().append("retention.purged", {"trace_days": plan.trace_days, "spans": counts["spans"],
                                                    "decision_days": plan.decision_days,
                                                    "decisions": counts["decisions"],
                                                    "evidence_days": plan.evidence_days,
                                                    "evidence": counts.get("evidence", 0),
                                                    "recovery_days": plan.recovery_days,
                                                    "recovery": counts.get("recovery", 0),
                                                    "operational_days": plan.operational_days,
                                                    "operational": operational,
                                                    "roots_left": left} | ({"stopped": True} if stopped else {}))

    async def _retention_loop(self) -> None:
        """Hourly, starting a minute after start-up so start-up stays quick. A failed pass is journaled, and the loop
        goes on even when that can't be (then it goes to stderr, redacted), as the resource loop does."""
        await asyncio.sleep(60)
        while True:
            try:
                await self._purge_pass()
            except Exception as e:
                error = f"{type(e).__name__}: {e}"
                try:
                    self._journal().append("retention.failed", {"error": error})
                except Exception as journal_error:
                    print(f"FERB: a retention pass failed ({self._redact_text(error)}) and couldn't be journaled "
                          f"({type(journal_error).__name__})", file=sys.stderr)
            await asyncio.sleep(3600)

    async def _resources_loop(self) -> None:
        """L30: a resource sample every SAMPLE_SECONDS (the owner's "every 5 minutes"); shutdown writes the last."""
        while True:
            await self.clock.sleep_until(self.clock.now() + timedelta(seconds=resource_use.SAMPLE_SECONDS))
            self._sample_resources()

    def _sample_resources(self) -> None:
        """One sample; a failure is journaled and the next sample covers this window too. Never raises: the loop goes
        on and shutdown finishes even when the failure can't be journaled (then it goes to stderr)."""
        try:
            self.resource_log.sample()
        except Exception as e:
            error = f"{type(e).__name__}: {e}"
            try:
                self._journal().append("resources.failed", {"error": error})
            except Exception as journal_error:
                print(f"FERB: a resource sample failed ({self._redact_text(error)}) and couldn't be journaled "
                      f"({type(journal_error).__name__})", file=sys.stderr)

    def _perception_counts(self) -> tuple[int, dict[str, int]]:
        """The perception batcher's wakes and its batches by flush reason since start, for the resource log; none
        before perception is built (start-up's baseline)."""
        if self.perception is None:
            return 0, {}
        batcher = self.perception.batches
        return batcher.wakes, dict(batcher.batches_by_reason)
