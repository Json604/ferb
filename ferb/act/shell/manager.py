"""Own task process lifetimes, reads, signals and cleanup. Must not rewrite commands or decide their meaning.

ShellManager: start / read / send / wait / kill / list, with task ownership and cleanup.

Processes belong to the task that started them; a worker only sees its own task's. When the task ends its
processes are killed unless started as long-lived (servers); `ferb stop` kills them all; `ferb ps` / `ferb kill`
list and end them. Commands run through /bin/sh in a clean environment (no FERB tokens, no API keys), under FERB's
sandbox (sandbox.py): the daemon never starts a command without it, and a child process can't shed the profile
(macOS applies it to the whole tree). The sandbox bounds files, the Keychain, app/URL launches, launchd jobs and
outbound network (what each rule covers, and its measured residuals, is in sandbox.py and the F5a Task 2 and 3
reports). Each process gets its own credentials for FERB's egress proxy (proxy.py), the only way off the Mac, so what
it reached is a recorded fact (`network`); when the task ends, each finished process's record is reported
(`on_network`, journaled by the daemon) and forgotten."""

import asyncio
import dataclasses
import os
import secrets
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from ferb.act.shell.proxy import EgressProxy
from ferb.act.shell.pty_proc import PtyProc
from ferb.act.shell.sandbox import Sandbox
from ferb.sanitize.redact import RedactionStream, Redactor
from ferb.core.types import EscapeGrant, ShellCommandGrant
from ferb.policy.protection import ADMIN
from ferb.core.errors import Refused

CLEAN_ENV_KEYS = ("PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "SHELL", "TMPDIR")


def clean_env(sandbox: Sandbox | None = None) -> dict[str, str]:
    """The environment of a process FERB contains (the shell, AppleScript): no FERB tokens, no API keys."""
    env = {k: os.environ[k] for k in CLEAN_ENV_KEYS if k in os.environ}
    # launchd gives the daemon a bare PATH; the owner's tools (brew, node, uv, gh) live in these (audit 2026-10-02)
    extras = ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin", str(Path.home() / ".local" / "bin")]
    system = env.get("PATH", "/usr/bin:/bin:/usr/sbin:/sbin").split(":")
    env["PATH"] = ":".join(dict.fromkeys([e for e in extras if os.path.isdir(e)] + system))
    env["TERM"] = "dumb"
    if sandbox is not None:
        env.update(sandbox.env())
    return env


@dataclass(frozen=True)
class ProcHandle:
    id: str
    owner_task: str


@dataclass(frozen=True)
class ProcInfo:
    id: str
    owner_task: str
    command: str
    cwd: str
    started: float
    running: bool
    exit_code: int | None
    long_lived: bool
    network: bool = False                     # it reached at least one host through FERB's proxy (a recorded fact)
    capture_error: str | None = None
    sandboxed: bool = False                   # it runs under FERB's sandbox profile
    lifted: str | None = None                 # the one rule an approved escape lifted for this run (D31)
    read_path: str | None = None
    sandbox_ms: float | None = None
    network_approved: bool = False


class ShellManager:
    def __init__(self, log_dir: Path, redact_text: Callable[[str], str],
                 stream_factory: Callable[[], RedactionStream] | None = None, sandbox: Sandbox | None = None,
                 proxy: EgressProxy | None = None,
                 on_network: Callable[[ProcInfo, dict[str, Any]], None] | None = None,
                 redactor: Redactor | None = None,
                 on_escape: Callable[[str, EscapeGrant], None] | None = None) -> None:
        self.log_dir, self.redact_text = log_dir, redact_text
        self.stream_factory = stream_factory
        self.sandbox = sandbox                    # None only in unit tests of the PTY plumbing; the daemon always sets it
        self.proxy = proxy                        # FERB's egress proxy (None: no network beyond this Mac)
        self.on_network = on_network              # a finished process's final network record (reported when its last
        self.redactor, self.on_escape = redactor, on_escape
        self._procs: dict[str, tuple[PtyProc, ProcInfo]] = {}   # connection closes, so a setsid child's late bytes count)
        self._owner_input: dict[str, str] = {}
        self._command_grants: dict[str, ShellCommandGrant] = {}
        if proxy is not None:
            proxy.on_report = self._report

    def _env(self, proc_id: str | None = None) -> dict[str, str]:
        env = clean_env(self.sandbox)
        if self.proxy is not None and proc_id is not None:
            env.update(self.proxy.env(proc_id))       # this process's own proxy credentials
        return env

    async def start(self, command: str, cwd: Path, owner_task: str, long_lived: bool = False,
                    *, lift: str | None = None, escape: EscapeGrant | None = None,
                    command_grant: ShellCommandGrant | None = None) -> ProcHandle:
        """Run the command text through /bin/sh, under the sandbox profile (with `lift`'s one rule lifted)."""
        pid = "p-" + secrets.token_hex(4)
        if command_grant is not None and (command_grant.command != command
                or command_grant.cwd != str(cwd.resolve()) or command_grant.task_id != owner_task):
            raise Refused("the command approval does not cover this exact command, working folder and task")
        if escape is not None:
            if escape.command != command or escape.cwd != str(cwd.resolve()) or escape.rule != lift:
                raise Refused("the escape grant does not cover this exact command and working folder")
        if lift == ADMIN and (escape is None or not escape.secret or self.redactor is None):
            raise Refused("admin requires the approved command's hidden password")
        if self.sandbox is None:
            if lift is not None:
                raise Refused("no sandbox here to lift a rule from")
            argv, profile = ["/bin/sh", "-c", command], None
        else:
            # Building the profile runs netstat and sandbox_check (listeners.py): off the event loop
            argv = await asyncio.to_thread(self.sandbox.argv, command, None if lift == ADMIN else lift)
            profile = self.sandbox.profile_path(argv)
        secret = escape.secret if escape is not None else None
        if secret and self.redactor is not None:
            self.redactor.retain_secret(secret)
        def finished() -> None:
            Sandbox.cleanup(profile)
            self._command_grants.pop(pid, None)
            if secret and self.redactor is not None:
                self.redactor.release_secret(secret)
        p = PtyProc(argv, cwd, self._env(pid), self.log_dir / f"{pid}.log", self.redact_text, self.stream_factory,
                    on_exit=finished, admin_secret=secret if lift == ADMIN else None)
        entry = self.sandbox.protection.entry(lift) if self.sandbox is not None and lift else None
        self._procs[pid] = (p, ProcInfo(pid, owner_task, command, str(cwd), time.time(), True, None, long_lived,
                                      sandboxed=self.sandbox is not None, lifted=lift,
                                      read_path=str(entry.path) if entry and entry.id.startswith("read:") else None,
                                      network_approved=command_grant is not None and command_grant.network))
        if command_grant is not None and command_grant.network:
            self._command_grants[pid] = command_grant
        try:
            await p.start()
        except BaseException:
            finished()
            self._procs.pop(pid, None)
            if self.proxy is not None:
                self.proxy.forget(pid)
            raise
        if escape is not None and self.on_escape is not None:
            self.on_escape(owner_task, escape)
        return ProcHandle(pid, owner_task)

    def _get(self, h: ProcHandle) -> PtyProc:
        hit = self._procs.get(h.id)
        if hit is None or hit[1].owner_task != h.owner_task:
            raise Refused(f"no process {h.id} for this task")
        return hit[0]

    async def read(self, h: ProcHandle, since: int = 0, limit: int | None = None) -> tuple[str, int, int]:
        """(text, the offset after it, total bytes so far): page forward with `since` = the returned offset."""
        p = self._get(h)
        text, offset, _ = p.read(since, limit)
        return text, offset, p.total

    async def send(self, h: ProcHandle, data: str) -> None:
        if h.id in self._owner_input:
            raise Refused('You have taken over this process. Worker input is paused until you detach.')
        self._get(h).send(data)

    def take_input(self, proc_id: str) -> str:
        if proc_id not in self._procs or self._procs[proc_id][0].exit_code is not None:
            raise Refused('That process is not running.')
        if proc_id in self._owner_input:
            raise Refused('Another attachment already owns input to this process.')
        lease = secrets.token_urlsafe(32)
        self._owner_input[proc_id] = lease
        return lease

    def owner_send(self, proc_id: str, lease: str, data: str) -> None:
        if not lease or not secrets.compare_digest(self._owner_input.get(proc_id, ''), lease):
            raise Refused('This attachment does not own process input.')
        self._procs[proc_id][0].send(data)

    def release_input(self, proc_id: str, lease: str) -> None:
        if lease and secrets.compare_digest(self._owner_input.get(proc_id, ''), lease):
            self._owner_input.pop(proc_id, None)

    async def tail(self, h: ProcHandle, limit: int) -> tuple[str, int, int]:
        p = self._get(h)
        text, offset, _ = p.read(max(0, p.total - limit), limit)
        return text, offset, p.total

    async def wait(self, h: ProcHandle, timeout: float) -> int | None:
        return await self._get(h).wait(timeout)

    async def kill(self, h: ProcHandle) -> None:
        await self._get(h).kill()

    def network(self, h: ProcHandle) -> dict[str, Any]:
        """What this process reached through FERB's proxy and what FERB refused it (proxy.py), so far."""
        self._get(h)
        return self._facts(h.id)

    def _facts(self, proc_id: str) -> dict[str, Any]:
        return self.proxy.facts(proc_id) if self.proxy is not None else {"destinations": [], "refused": []}

    def list(self) -> list[ProcInfo]:
        out = []
        for p, info in self._procs.values():
            out.append(dataclasses.replace(info, running=p.exit_code is None, exit_code=p.exit_code,
                                           capture_error=p.capture_error, sandbox_ms=p.ready_ms if info.sandboxed else None,
                                           network=bool(self._facts(info.id)["destinations"])))
        return out

    def info(self, h: ProcHandle) -> ProcInfo:
        self._get(h)
        return next(p for p in self.list() if p.id == h.id)

    def handle(self, proc_id: str, owner_task: str) -> ProcHandle:
        return ProcHandle(proc_id, owner_task)

    def owner_of(self, proc_id: str) -> str | None:
        """The task that started this process (the gateway's per-task sink decisions, S4); None if it's unknown."""
        hit = self._procs.get(proc_id)
        return hit[1].owner_task if hit is not None else None

    def command_of(self, proc_id: str) -> str | None:
        """The command text this process ran (FERB names it to the owner when a destination needs approval)."""
        hit = self._procs.get(proc_id)
        return hit[1].command if hit is not None else None

    def network_approved(self, proc_id: str) -> bool:
        """Only the live process with the approved command's proxy credentials carries its grant."""
        hit = self._procs.get(proc_id)
        return hit is not None and hit[0].exit_code is None and proc_id in self._command_grants

    async def kill_id(self, proc_id: str) -> bool:
        hit = self._procs.get(proc_id)
        if hit is None:
            return False
        await hit[0].kill()
        return True

    async def end_task(self, task_id: str) -> int:
        """Task finished: kill its processes, except long-lived ones; report and forget the finished ones' network."""
        n = 0
        for p, info in list(self._procs.values()):
            if info.owner_task == task_id and not info.long_lived and p.exit_code is None:
                await p.kill()
                n += 1
        self._settle(lambda info: info.owner_task == task_id)
        return n

    async def kill_all(self) -> int:
        n = 0
        for p, _ in list(self._procs.values()):
            if p.exit_code is None:
                await p.kill()
                n += 1
        self._settle(lambda info: True)
        return n

    def _settle(self, which: Callable[[ProcInfo], bool]) -> None:
        """Finished processes: forget their credential (it stops working now). The proxy keeps the record and reports
        it through `_report` once the process's last connection closes, so a setsid child's late bytes still count.
        A running process is left alone, so later reads still show it."""
        if self.proxy is None:
            return
        for info in self.list():
            if which(info) and not info.running:
                self.proxy.forget(info.id)

    def _report(self, proc_id: str, facts: dict[str, Any]) -> None:
        """The proxy's final record for a forgotten process (from the daemon loop, or a tunnel's thread at close)."""
        hit = self._procs.get(proc_id)
        if hit is not None and self.on_network is not None and (facts["destinations"] or facts["refused"]):
            self.on_network(hit[1], facts)
