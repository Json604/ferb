"""Own the daemon's side of memory import (notes/memory-import.md): the RPCs the app calls, the one background job, its
resume after a restart and its cancel. Must not run an import on the owner's turn, or keep what it read."""

import asyncio
from typing import Any

from ferb.core.errors import FerbError
from ferb.daemon.server import Session


class MemoryImportGlue:
    """A mixin of the Daemon (ferb/daemon/main.py)."""

    _import: Any = None
    _import_task: "asyncio.Future[Any] | None" = None
    _import_cancel: str = ""

    def _import_job(self) -> Any:
        from ferb.memory.import_job import ImportJob
        if self._import is None:
            self._import = ImportJob(      # late-bound: each call reads the daemon's current runtime and state
                self.memory, make_runtime=lambda *a, **k: self._make_runtime(*a, **k), redact=self._redact_text,
                append=lambda kind, data: self._journal().append(kind, data), publish=self._publish,
                tell=lambda *a, **k: self._tell_owner(*a, **k), similar=lambda q: self._memory_item_search(q),
                busy=lambda: self._import_busy(),
                new_id=self.ids.new_id, spans=self.spans, usage=self._record_learning_usage("memory_import"),
                device_id=str(self.config.get("device", "id", "dev")))
        return self._import

    def _import_busy(self) -> bool:
        """The owner's chat turn is under way (its planner call is the critical path): an import call waits."""
        return any(not t.done() for t in self._turns)

    def _resume_imports(self) -> None:
        """At start: a run a restart interrupted goes on from its last finished chunk."""
        job = self._import_job()
        for run_id in job.running():
            if self._import_task is None or self._import_task.done():
                self._import_task = self._spawn(self._run_import(run_id), "memory import")
            else:                                 # one job at a time; any other interrupted run ends as it was
                job.end(run_id, "failed", "another import was running")

    async def _run_import(self, run_id: str) -> None:
        job = self._import_job()
        try:
            await job.run(run_id)
        except asyncio.CancelledError:
            if self._import_cancel == run_id:     # the owner's cancel; a shutdown leaves it running, to resume
                job.end(run_id, "cancelled")
            raise
        except Exception as e:                    # FERB's own failure: the run ends (never resumed in a loop), told once
            job.end(run_id, "failed", self._redact_text(f"{type(e).__name__}: {e}")[:300])
        await job.tell_done(run_id)

    def _import_params(self, params: dict[str, Any]) -> tuple[str, str]:
        from ferb.memory.import_job import check
        from ferb.memory.import_sources import ImportUnreadable
        source, path = str(params.get("source") or ""), str(params.get("path") or "")
        try:
            check(source, path)
        except ImportUnreadable as e:
            raise FerbError(str(e)) from None
        return source, path

    async def _memory_import_estimate(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """The dry run: conversations, messages and chunks in the last three months, and the model calls an import
        would make (nothing is sent anywhere)."""
        from ferb.memory.import_sources import ImportUnreadable
        source, path = self._import_params(params)
        try:
            return dict(await self._import_job().estimate(source, path))
        except (ImportUnreadable, OSError) as e:
            raise FerbError(str(e)) from None

    async def _memory_import_start(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Starts the import in the background; progress comes as `memory.import.progress` events and
        memory.import.status. The answer carries the estimate (model calls up front)."""
        from ferb.memory.import_job import ImportBusy
        from ferb.memory.import_sources import ImportUnreadable
        source, path = self._import_params(params)
        job = self._import_job()
        try:
            estimate = await job.estimate(source, path)
            run_id = job.create(source, path)
        except (ImportUnreadable, ImportBusy, OSError) as e:
            raise FerbError(str(e)) from None
        self._import_cancel = ""
        self._import_task = self._spawn(self._run_import(run_id), "memory import")
        return {"run": run_id, "estimate": estimate}

    async def _memory_import_status(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return dict(self._import_job().status())

    async def _memory_import_cancel(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Stops the running import after its current call; what it saved stays, and a later start goes on."""
        job = self._import_job()
        running = job.running()
        if not running:
            return {"cancelled": None}
        self._import_cancel = running[0]
        task = self._import_task
        if task is not None and not task.done():
            task.cancel()
            await asyncio.wait({task})
        else:
            job.end(running[0], "cancelled")
        return {"cancelled": running[0], "status": job.row(running[0])}
