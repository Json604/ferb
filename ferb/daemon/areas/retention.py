"""Own the retention pass (D21) and the resource log's samples (L30). Must not purge the task list, episodes, journal or
memory."""

import asyncio
import sys
from datetime import datetime, timedelta
from typing import NamedTuple

from ferb.act.fs import recovery
from ferb.daemon import resources as resource_use
from ferb.orchestrator import episodes
from ferb.store import retention


ROOTS_PER_PURGE = 200


class _PurgePlan(NamedTuple):
    """One retention pass: its clock, its windows, the roots it handles and how many wait for the next pass."""
    now: datetime
    trace_days: int
    decision_days: int
    evidence_days: int
    recovery_days: int
    resource_days: int
    roots: list[str]
    left: int


class Retention:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _purge_records(self) -> dict[str, int]:
        """D21: spans of tasks that ended more than trace_days ago (once their episode exists) and loose spans;
        Jev decisions older than decision_days; evidence (screenshots, snapshots, recordings) and shell output logs
        older than evidence_days (V1, D20: 7); recovery copies and records older than recovery_days (S1: 7). Never the task list,
        episodes, journal or memory. One synchronous pass (tests, call sites); the hourly loop runs the same steps in
        `_purge_pass`."""
        plan = self._purge_plan()
        return self._purge_rest(plan, sum(self._purge_root(root, plan.now) for root in plan.roots))

    async def _purge_pass(self) -> dict[str, int]:
        """The retention pass on the daemon loop: the same steps as `_purge_records`, yielding between roots. If
        shutdown cancels it between roots, what it already purged is still journaled (`stopped`)."""
        plan = self._purge_plan()
        spans = done = 0
        finished = False
        try:
            for root in plan.roots:
                spans += self._purge_root(root, plan.now)
                done += 1
                await asyncio.sleep(0)
            finished = True
        finally:
            if not finished and spans:
                self._journal_purged(plan, spans, 0, 0, 0, plan.left + len(plan.roots) - done, stopped=True)
        result = self._purge_rest(plan, spans)               # journaled before the slow snapshot step
        result["snapshots"] = await self._purge_snapshots(plan)
        return result

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

    def _purge_plan(self) -> _PurgePlan:
        now = self.clock.now()
        trace_days = retention.days(self.config.get("retention", "trace_days", 30))          # each at least 1 day
        decision_days = retention.days(self.config.get("retention", "decision_days", 30))
        evidence_days = retention.days(self.config.get("retention", "evidence_days", 7))
        recovery_days = retention.days(self.config.get("retention", "recovery_days", 7))
        resource_days = retention.days(self.config.get("retention", "resource_days", resource_use.DEFAULT_DAYS))
        roots = retention.finished_roots_before(self.db, (now - timedelta(days=trace_days)).isoformat())
        return _PurgePlan(now, trace_days, decision_days, evidence_days, recovery_days, resource_days,
                          roots[:ROOTS_PER_PURGE], max(0, len(roots) - ROOTS_PER_PURGE))   # oldest first; the rest wait

    def _purge_root(self, root: str, now: datetime) -> int:
        """One root: its spans go only once its episode exists (built now if missing). Isolated: a failure is
        journaled and the pass goes on."""
        try:
            if episodes.get(self.db, root) is None:          # build it while the spans still exist
                episodes.record(self.spans, self._journal(), root, now.isoformat())
            if episodes.get(self.db, root) is None:
                return 0                                     # no episode: keep the spans
            return retention.purge_task_spans(self.db, root)
        except Exception as e:
            self._journal().append("retention.failed", {"task_id": root, "error": f"{type(e).__name__}: {e}"})
            return 0

    def _purge_rest(self, plan: _PurgePlan, spans: int) -> dict[str, int]:
        """Loose spans, old Jev decisions, old evidence and old recovery copies and records (a trash record stays while
        its item is in the Trash, TRASH_RECORD_MAX_DAYS at most); snapshots are the async pass's last step.
        `retention.purged` is journaled only when the pass purged something or left roots for the next one. Old
        resource samples (L30, numbers only) go too, counted in the result, not journaled."""
        spans += retention.purge_loose_spans(self.db, (plan.now - timedelta(days=plan.trace_days)).isoformat())
        decisions = retention.purge_decisions(self.db, (plan.now - timedelta(days=plan.decision_days)).isoformat())
        evidence = self.evidence.purge_before((plan.now - timedelta(days=plan.evidence_days)).isoformat()) \
            if self.evidence is not None else 0
        kept = self._recovery()
        copies = recovery.purge_before(
            (plan.now - timedelta(days=plan.recovery_days)).isoformat(), kept.db, kept.root,
            trash_backstop_iso=(plan.now - timedelta(days=recovery.TRASH_RECORD_MAX_DAYS)).isoformat())
        if spans or decisions or evidence or copies or plan.left:
            self._journal_purged(plan, spans, decisions, evidence, copies, plan.left)
        trashed = self._purge_downloads()
        if self.perception is not None:
            self.perception.purge_text()
        return {"spans": spans, "decisions": decisions, "evidence": evidence, "recovery": copies, "downloads": trashed,
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
        """D35: when FERB's downloads folder passes browser.download_total_mb, its oldest working copies go to the
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

    def _journal_purged(self, plan: _PurgePlan, spans: int, decisions: int, evidence: int, copies: int, left: int,
                        stopped: bool = False) -> None:
        self._journal().append("retention.purged", {"trace_days": plan.trace_days, "spans": spans,
                                                    "decision_days": plan.decision_days, "decisions": decisions,
                                                    "evidence_days": plan.evidence_days, "evidence": evidence,
                                                    "recovery_days": plan.recovery_days, "recovery": copies,
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
