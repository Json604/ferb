"""Own the shell's egress decisions as the daemon relays them (from proxy threads to the gateway on the loop), local
connection reports and the `ferb ps` RPCs. Must not allow a destination itself: the gateway decides."""

import asyncio
import json
from typing import Any, AsyncIterator

from ferb.act.shell.manager import ProcInfo
from ferb.act.shell.proxy import local_host
from ferb.core.errors import FerbError
from ferb.daemon.server import Session
from ferb.policy import confirm


class Egress:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _allow_destination(self, proc: str, host: str, port: int) -> tuple[bool, str]:
        """On a proxy thread: the gateway's answer, from the daemon's loop. Anything that stops it answering refuses."""
        try:
            assert self.gateway is not None and self._loop is not None
            fut = asyncio.run_coroutine_threadsafe(self.gateway.destination_allowed(proc, host, port), self._loop)
            return fut.result(10)
        except Exception:
            return False, "FERB could not check this destination"

    def _allow_local_destination(self, proc: str, host: str, port: int) -> tuple[bool, str]:
        """Only the proxy calls this, after verifying an explicit local connected peer against its configured pin.
        The pin is the standing local permission; task ownership and an explicit No still apply."""
        try:
            assert self.gateway is not None and self._loop is not None
            fut = asyncio.run_coroutine_threadsafe(
                self.gateway.destination_allowed(proc, host, port, local_approved=True), self._loop)
            return fut.result(10)
        except Exception:
            return False, "FERB could not check this local destination"

    def _local_refused(self, task_id: str) -> bool:
        """Whether this task's tree has the owner's No to one of this Mac's addresses (the shared host key, SQ6): its
        shell commands then reach pinned local ports only through the proxy, which keeps that No final for each
        connection (L24). A task FERB can't place counts as refused."""
        root = self.tasks.tree_of(task_id)
        if root is None:
            return True
        confirmations = self.gateway.confirmations if self.gateway is not None else None
        for key in confirmations.refused(root) if confirmations is not None else []:
            try:
                rule, host = json.loads(key)
            except (ValueError, TypeError):
                continue
            if rule == confirm.SINK_DESTINATION and isinstance(host, str) and (
                    local_host(host) or host.rstrip(".").endswith(".localhost")):
                return True
        return False

    def _local_reports_stopped(self, why: str) -> None:
        """The kernel's reports of direct local connections stopped by themselves (the stream ended or couldn't
        start; told once, from the stream's thread): from now on those records say unknown. `ferb doctor` shows it."""
        self._journal_from_any_thread("shell.local_reports_stopped", {"why": why})

    def _local_reports_status(self) -> dict[str, Any] | None:
        """For `ferb doctor`: whether the kernel's reports of direct local connections are being read (None: no
        pinned local ports, nothing to read)."""
        reports = self.direct_reports
        return None if reports is None else {"running": reports.running, "stopped": reports.ended}

    def _shell_network(self, info: ProcInfo, facts: dict[str, Any]) -> None:
        """A finished shell process's final network record (ferb trace / journal). The proxy may report it from a
        tunnel's own thread when a setsid child closes it, so the write is scheduled on the daemon loop."""
        self._journal_from_any_thread("shell.network", {"task_id": info.owner_task, "process": info.id, **facts})

    def _journal_from_any_thread(self, kind: str, record: dict[str, Any]) -> None:
        """A journal row from the daemon loop or another thread (written on the loop)."""
        def write() -> None:
            self._journal().append(kind, record)
        try:
            on_loop = asyncio.get_running_loop() is self._loop
        except RuntimeError:                                  # a proxy thread
            on_loop = False
        if on_loop or self._loop is None or not self._loop.is_running():
            write()                                           # at shutdown too: before the database closes
        else:
            self._loop.call_soon_threadsafe(write)

    async def _ps_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.shells
        return {"processes": [{**p.__dict__, "command": self._redact_text(p.command)}   # rule 14: as `ferb ps` shows it
                              for p in self.shells.list()]}

    async def _ps_kill(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.shells
        return {"killed": await self.shells.kill_id(str(params.get("id", "")))}

    async def _ps_attach(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """Owner-only stream. Connection lifetime owns input; disconnect releases it even after a CLI crash."""
        assert self.shells
        proc_id = str(params.get('id', ''))
        task = self.shells.owner_of(proc_id)
        if task is None:
            raise FerbError('That process is not available.')
        handle = self.shells.handle(proc_id, task)
        lease = self.shells.take_input(proc_id) if params.get('take') else ''
        offset = max(0, int(params.get('since', 0)))
        try:
            yield {'lease': lease, 'id': proc_id}
            while True:
                text, offset, _ = await self.shells.read(handle, offset)
                info = self.shells.info(handle)
                yield {'text': text, 'offset': offset, 'running': info.running, 'exit_code': info.exit_code}
                if not info.running:
                    return
                await asyncio.sleep(0.1)
        finally:
            self.shells.release_input(proc_id, lease)

    async def _ps_send(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        assert self.shells
        self.shells.owner_send(str(params.get('id', '')), str(params.get('lease', '')), str(params.get('text', '')))
        return {'sent': True}
