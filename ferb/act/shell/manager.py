"""Own task process lifetimes, reads, signals and cleanup. Must not rewrite commands or decide their meaning.

ShellManager: start / read / send / wait / kill / list, with task ownership and cleanup.

Processes belong to the task that started them; a worker only sees its own task's. When the task ends its
processes are killed unless started as long-lived (servers); `ferb stop` kills them all; `ferb ps` / `ferb kill`
list and end them. Commands run through /bin/sh in a clean environment (no FERB tokens, no API keys), under FERB's
sandbox (sandbox.py): the daemon never starts a command without it, and a child process can't shed the profile
(macOS applies it to the whole tree). The sandbox bounds files, the Keychain, app/URL launches, launchd jobs and
outbound network (what each rule covers, and its measured residuals, is in sandbox.py and the F5a Task 2 report)."""

import dataclasses
import os
import secrets
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from ferb.act.shell.pty_proc import PtyProc
from ferb.act.shell.sandbox import Sandbox
from ferb.sanitize.redact import RedactionStream
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
    network: bool = False                     # it fetches from the network: its output is untrusted
    capture_error: str | None = None
    sandboxed: bool = False                   # it runs under FERB's sandbox profile
    lifted: str | None = None                 # the one rule an approved escape lifted for this run (D31)


class ShellManager:
    def __init__(self, log_dir: Path, redact_text: Callable[[str], str],
                 stream_factory: Callable[[], RedactionStream] | None = None, sandbox: Sandbox | None = None) -> None:
        self.log_dir, self.redact_text = log_dir, redact_text
        self.stream_factory = stream_factory
        self.sandbox = sandbox                    # None only in unit tests of the PTY plumbing; the daemon always sets it
        self._procs: dict[str, tuple[PtyProc, ProcInfo]] = {}

    def _env(self) -> dict[str, str]:
        return clean_env(self.sandbox)

    async def start(self, command: str, cwd: Path, owner_task: str, long_lived: bool = False,
                    network: bool = False, *, lift: str | None = None) -> ProcHandle:
        """Run the command text through /bin/sh, under the sandbox profile (with `lift`'s one rule lifted)."""
        pid = "p-" + secrets.token_hex(4)
        if self.sandbox is None:
            if lift is not None:
                raise Refused("no sandbox here to lift a rule from")
            argv, profile = ["/bin/sh", "-c", command], None
        else:
            argv = self.sandbox.argv(command, lift)
            profile = self.sandbox.profile_path(argv)
        p = PtyProc(argv, cwd, self._env(), self.log_dir / f"{pid}.log", self.redact_text, self.stream_factory,
                    on_exit=lambda: Sandbox.cleanup(profile))
        try:
            await p.start()
        except BaseException:
            Sandbox.cleanup(profile)
            raise
        self._procs[pid] = (p, ProcInfo(pid, owner_task, command, str(cwd), time.time(), True, None, long_lived,
                                        network, sandboxed=self.sandbox is not None, lifted=lift))
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
        self._get(h).send(data)

    async def tail(self, h: ProcHandle, limit: int) -> tuple[str, int, int]:
        p = self._get(h)
        text, offset, _ = p.read(max(0, p.total - limit), limit)
        return text, offset, p.total

    async def wait(self, h: ProcHandle, timeout: float) -> int | None:
        return await self._get(h).wait(timeout)

    async def kill(self, h: ProcHandle) -> None:
        await self._get(h).kill()

    def list(self) -> list[ProcInfo]:
        out = []
        for p, info in self._procs.values():
            out.append(dataclasses.replace(info, running=p.exit_code is None, exit_code=p.exit_code,
                                           capture_error=p.capture_error))
        return out

    def info(self, h: ProcHandle) -> ProcInfo:
        self._get(h)
        return next(p for p in self.list() if p.id == h.id)

    def handle(self, proc_id: str, owner_task: str) -> ProcHandle:
        return ProcHandle(proc_id, owner_task)

    async def kill_id(self, proc_id: str) -> bool:
        hit = self._procs.get(proc_id)
        if hit is None:
            return False
        await hit[0].kill()
        return True

    async def end_task(self, task_id: str) -> int:
        """Task finished: kill its processes, except long-lived ones."""
        n = 0
        for p, info in list(self._procs.values()):
            if info.owner_task == task_id and not info.long_lived and p.exit_code is None:
                await p.kill()
                n += 1
        return n

    async def kill_all(self) -> int:
        n = 0
        for p, _ in list(self._procs.values()):
            if p.exit_code is None:
                await p.kill()
                n += 1
        return n
