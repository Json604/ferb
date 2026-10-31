"""Own task process lifetimes, reads, signals and cleanup. Must not rewrite commands or decide their meaning.

ShellManager: start / read / send / wait / kill / list, with task ownership and cleanup.

Processes belong to the task that started them; a worker only sees its own task's. When the task ends its
processes' whole groups are killed unless started as long-lived (servers), even when the command's own process has
already exited (children it left keep its group); then their entries go once their network record is out. `ferb stop`
kills them all; `ferb ps` / `ferb kill` list and end them. Output logs are purged after the evidence window (7 days). Commands run through /bin/sh in a clean environment (no FERB tokens, no API keys), under FERB's
sandbox (sandbox.py): the daemon never starts a command without it, and a child process can't shed the profile
(macOS applies it to the whole tree). The sandbox bounds files, the Keychain, app/URL launches, launchd jobs and
outbound network (what each rule covers, and its measured residuals, is in sandbox.py and the F5a Task 2 and 3
reports). Each process gets its own credentials for FERB's egress proxy (proxy.py), the only way off the Mac, so what
it reached is a recorded fact (`network`); when the task ends, each finished process's record is reported
(`on_network`, journaled by the daemon) and forgotten, and a process still running then is reported when it exits.
An owner-pinned local port whose listener runs its pinned executable when the command starts is reachable directly
(L24, local.py); the live `network` view lists each such port under `local_direct` (`local: true`) with the
connections the kernel reported for this command (`DirectReports`), and the record keeps the ones it connected to, or
all of them when the kernel's reports couldn't be read.

Downloads (hands decision 7): while any FERB command runs, every WATCH_S the bytes each one received through the proxy
since the last look go to `on_received` (the daemon counts them against the task tree's download budget, shared with
the browser, and checks the free-disk floor); when it answers why, that command's whole group is stopped and the
command's results say so (`stopped`, FERB's words), a fact, never a judgment of the command. Only a command receiving
bytes is ever stopped; one that isn't downloading never is."""

import asyncio
import dataclasses
import os
import secrets
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable

from ferb.act.shell.listeners import contained
from ferb.act.shell.local import DirectReports, report_tag
from ferb.act.shell.proxy import EgressProxy
from ferb.act.shell.pty_proc import PtyProc
from ferb.act.shell.sandbox import Sandbox
from ferb.sanitize.redact import RedactionStream, Redactor
from ferb.core.types import EscapeGrant
from ferb.policy.protection import ADMIN, LocalPort
from ferb.core.errors import Refused

CLEAN_ENV_KEYS = ("PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "SHELL", "TMPDIR")
SEND_WAIT_S = 2.0                         # how long shell.send keeps typing into a command that reads slowly
# Ruling (step 12e): how often the download budget and the free-disk floor are checked while a FERB command runs. At a
# fast 1 GB/s link a command can pass either by about this much before it is stopped (the floor is 10 GB).
WATCH_S = 1.0


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
    # owner-pinned local ports it may reach directly, as checked when it started: ({"port", "executable"}, ...) (L24)
    direct: tuple[dict[str, Any], ...] = ()
    stopped: str | None = None                # FERB stopped it: why (the download budget or the free-disk floor)


class ShellManager:
    def __init__(self, log_dir: Path, redact_text: Callable[[str], str],
                 stream_factory: Callable[[], RedactionStream] | None = None, sandbox: Sandbox | None = None,
                 proxy: EgressProxy | None = None,
                 on_network: Callable[[ProcInfo, dict[str, Any]], None] | None = None,
                 redactor: Redactor | None = None,
                 on_escape: Callable[[str, EscapeGrant], None] | None = None,
                 reports: DirectReports | None = None,
                 local_refused: Callable[[str], bool] | None = None) -> None:
        self.log_dir, self.redact_text = log_dir, redact_text
        self.stream_factory = stream_factory
        self.sandbox = sandbox                    # None only in unit tests of the PTY plumbing; the daemon always sets it
        self.proxy = proxy                        # FERB's egress proxy (None: no network beyond this Mac)
        self.on_network = on_network              # a finished process's final network record (reported when its last
        self.redactor, self.on_escape = redactor, on_escape
        self.reports = reports                    # the kernel's reports of direct local connections (None: unread)
        self.local_refused = local_refused        # whether a task's tree said No to one of this Mac's addresses
        self._procs: dict[str, tuple[PtyProc, ProcInfo]] = {}   # connection closes, so a setsid child's late bytes count)
        self._owner_input: dict[str, str] = {}
        self._report_on_exit: set[str] = set()   # running when their task ended: retired when they exit
        self._retiring: set[str] = set()         # finished, task ended: dropped once the proxy reports their record
        # Downloads (hands decision 7, daemon supplied): (task, bytes received since the last look) -> why the command
        # must stop now, or None. Called on the event loop only.
        self.on_received: Callable[[str, int], str | None] | None = None
        self._received: dict[str, int] = {}      # process -> bytes received through the proxy, as last counted
        self._stopped: dict[str, str] = {}       # process -> why FERB stopped it
        self._watcher: asyncio.Task[None] | None = None
        self._loop: asyncio.AbstractEventLoop | None = None
        if proxy is not None:
            proxy.on_report = self._report

    def _env(self, proc_id: str | None = None) -> dict[str, str]:
        env = clean_env(self.sandbox)
        if self.proxy is not None and proc_id is not None:
            env.update(self.proxy.env(proc_id))       # this process's own proxy credentials
        return env

    async def start(self, command: str, cwd: Path, owner_task: str, long_lived: bool = False,
                    *, lift: str | None = None, escape: EscapeGrant | None = None,
                    folders: tuple[str, ...] = ()) -> ProcHandle:
        """Run the command text through /bin/sh, under the sandbox profile (with `lift`'s one rule lifted, and the
        task's `folders` the only visible folders it may change)."""
        pid = "p-" + secrets.token_hex(4)
        direct: tuple[LocalPort, ...] = ()
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
            # Building the profile reads the sockets and runs sandbox_check (listeners.py): off the event loop. Pinned
            # ports are direct only for a command whose record goes out with the proxy's, and never after its tree's
            # No to one of this Mac's addresses: that No stays final, checked per connection by the proxy
            rule = None if lift == ADMIN else lift
            if self.proxy is not None and not (self.local_refused is not None and self.local_refused(owner_task)):
                tag = report_tag(pid)                 # in its profile only: the command can't read it there
                argv, direct = await asyncio.to_thread(self.sandbox.contain, command, tag, rule, folders)
                if direct and self.reports is not None:
                    self.reports.expect(pid, tag)
            else:
                argv = await asyncio.to_thread(self.sandbox.argv, command, rule, folders)
            profile = self.sandbox.profile_path(argv)
        secret = escape.secret if escape is not None else None
        if secret and self.redactor is not None:
            self.redactor.retain_secret(secret)
        def finished() -> None:
            Sandbox.cleanup(profile)
            if pid in self._report_on_exit:          # outlived its task: its record is due now, then it goes
                self._report_on_exit.discard(pid)
                self._retire(pid)
            if secret and self.redactor is not None:
                self.redactor.release_secret(secret)
        p = PtyProc(argv, cwd, self._env(pid), self.log_dir / f"{pid}.log", self.redact_text, self.stream_factory,
                    on_exit=finished, admin_secret=secret if lift == ADMIN else None)
        entry = self.sandbox.protection.entry(lift) if self.sandbox is not None and lift else None
        self._procs[pid] = (p, ProcInfo(pid, owner_task, command, str(cwd), time.time(), True, None, long_lived,
                                      sandboxed=self.sandbox is not None, lifted=lift,
                                      read_path=str(entry.path) if entry and entry.id.startswith("read:") else None,
                                      direct=tuple({"port": pin.port, "executable": str(pin.executable)}
                                                   for pin in direct)))
        try:
            await p.start()
        except BaseException:
            finished()
            self._procs.pop(pid, None)
            if self.proxy is not None:
                self.proxy.forget(pid)
            if direct and self.reports is not None:   # it never ran: no reports to expect under its tag
                self.reports.forget(pid)
            raise
        if escape is not None and self.on_escape is not None:
            self.on_escape(owner_task, escape)
        self._watch_downloads()
        return ProcHandle(pid, owner_task)

    # ---------- downloads: the task tree's budget and the free-disk floor (hands decision 7) ----------

    def _watch_downloads(self) -> None:
        """Start the download watch while commands run (it ends by itself when none is left)."""
        if self.on_received is None or self.proxy is None or (self._watcher is not None and not self._watcher.done()):
            return
        self._loop = asyncio.get_running_loop()
        self._watcher = self._loop.create_task(self._watch())

    async def _watch(self) -> None:
        while self._procs:
            await asyncio.sleep(WATCH_S)
            for pid, (p, info) in list(self._procs.items()):
                why = self._count_received(pid, info.owner_task)
                if why is not None and pid not in self._stopped and p.exit_code is None:
                    self._stopped[pid] = why
                    try:
                        await p.kill(self._ours(pid))
                    except Exception:
                        pass                      # it ended meanwhile; `stopped` still says why FERB acted

    def _count_received(self, pid: str, task: str, facts: dict[str, Any] | None = None) -> str | None:
        """The bytes this process received through the proxy since the last look go to `on_received`; its answer (why
        it must stop) only when it received some. Never raises: a broken count stops nothing."""
        if self.on_received is None or self.proxy is None:
            return None
        got = sum(int(d.get("bytes_in") or 0) for d in (facts or self.proxy.facts(pid))["destinations"])
        new = got - self._received.get(pid, 0)
        if new <= 0:
            return None
        self._received[pid] = got
        try:
            return self.on_received(task, new)
        except Exception:
            return None

    def stopped(self, proc_id: str) -> str | None:
        """Why FERB stopped this command (the download budget or the free-disk floor); None: it didn't."""
        return self._stopped.get(proc_id)

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

    async def send(self, h: ProcHandle, data: str, wait_s: float = SEND_WAIT_S) -> tuple[int, int]:
        """Type `data` into the command's terminal: (bytes that went in, bytes given). A command that reads slowly gets
        the rest for up to `wait_s`; whatever didn't go in by then is reported, never claimed. Refused, before
        anything is typed: a command the owner took over, one started with an owner-approved lift (D31: the Yes covered
        that exact command, not what is typed into it later), and a line longer than its terminal takes while it reads
        lines (the terminal would drop it whole and report it sent)."""
        if h.id in self._owner_input:
            raise Refused('You have taken over this process. Worker input is paused until you detach.')
        p = self._get(h)
        lifted = self._procs[h.id][1].lifted
        if lifted is not None:
            raise Refused(f"This command runs with the owner's one-time lift of {lifted}; FERB doesn't type into it, "
                          "because the owner's Yes covered that exact command, not later input. Put the input in the "
                          "approved command itself (a here-document or a file), and ask again if it needs the lift.")
        raw = data.encode()
        limit = p.line_limit()
        if limit is not None:
            *full, last = raw.split(b"\n")
            if any(len(line) + 1 > limit for line in full) or len(last) > limit - 1:
                raise Refused(f"A line longer than {limit - 1} bytes can't go to this command: its terminal reads "
                              "line by line and drops a longer line whole. Nothing was sent. Write the text to a file "
                              "in the workspace and pass the file, or send shorter lines.")
        sent, deadline = 0, time.monotonic() + wait_s
        while True:
            sent += p.write(raw[sent:])
            if sent >= len(raw) or p.exit_code is not None or time.monotonic() >= deadline:
                return sent, len(raw)
            await asyncio.sleep(0.02)

    def take_input(self, proc_id: str) -> str:
        if proc_id not in self._procs or self._procs[proc_id][0].exit_code is not None:
            raise Refused('That process is not running.')
        if proc_id in self._owner_input:
            raise Refused('Another attachment already owns input to this process.')
        lease = secrets.token_urlsafe(32)
        self._owner_input[proc_id] = lease
        return lease

    def owner_send(self, proc_id: str, lease: str, data: str) -> int:
        """The owner's own typing into an attached process: what the terminal took (bytes)."""
        if not lease or not secrets.compare_digest(self._owner_input.get(proc_id, ''), lease):
            raise Refused('This attachment does not own process input.')
        return self._procs[proc_id][0].send(data)

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
        await self._get(h).kill(self._ours(h.id))

    def _ours(self, proc_id: str) -> Callable[[list[int]], bool]:
        """For killing a group whose first process has ended (PtyProc.kill): every process now in it runs inside
        FERB's sandbox, and its id isn't another running command's own (a reused id would name that command's group).
        Without a sandbox (unit tests of the plumbing) nothing can be checked, and that is accepted there."""
        hit = self._procs.get(proc_id)
        leader = hit[0].proc.pid if hit is not None and hit[0].proc is not None else None
        others = {p.proc.pid for pid, (p, _) in self._procs.items()
                  if pid != proc_id and p.proc is not None and p.exit_code is None}
        sandbox = self.sandbox

        def check(members: list[int]) -> bool:
            if leader is None or leader in others:
                return False
            if sandbox is None:
                return True
            prot = sandbox.protection
            return all(contained(m, prot.ferb_home, prot.workspace) for m in members)
        return check

    def network(self, h: ProcHandle) -> dict[str, Any]:
        """What this process reached through FERB's proxy and what FERB refused it (proxy.py), so far."""
        self._get(h)
        return self._facts(h.id)

    def _facts(self, proc_id: str) -> dict[str, Any]:
        facts = self.proxy.facts(proc_id) if self.proxy is not None else {"destinations": [], "refused": []}
        hit = self._procs.get(proc_id)
        if hit is not None and hit[1].direct:
            facts["local_direct"] = self._direct(hit[1])
        return facts

    def _direct(self, info: ProcInfo) -> list[dict[str, Any]]:
        """Each pinned local port this process may reach directly, with the connections the kernel reported for it
        so far and the programs that made them; None for both when the kernel's reports couldn't be read."""
        seen = self.reports.connections(info.id, info.started) if self.reports is not None else None
        return [{"host": "localhost", "port": d["port"], "executable": d["executable"], "local": True, "direct": True,
                 **({"reported_connections": None, "clients": None} if seen is None else
                    seen.get(d["port"], {"reported_connections": 0, "clients": []}))} for d in info.direct]

    def list(self) -> list[ProcInfo]:
        out = []
        for p, info in self._procs.values():
            out.append(dataclasses.replace(info, running=p.exit_code is None, exit_code=p.exit_code,
                                           capture_error=p.capture_error, sandbox_ms=p.ready_ms if info.sandboxed else None,
                                           network=bool(self._facts(info.id)["destinations"]),
                                           stopped=self._stopped.get(info.id)))
        return out

    def info(self, h: ProcHandle) -> ProcInfo:
        self._get(h)
        return next(p for p in self.list() if p.id == h.id)

    def handle(self, proc_id: str, owner_task: str) -> ProcHandle:
        return ProcHandle(proc_id, owner_task)

    def owner_of(self, proc_id: str) -> str | None:
        """The task that started this process; None if it's unknown."""
        hit = self._procs.get(proc_id)
        return hit[1].owner_task if hit is not None else None

    def live_owner(self, proc_id: str) -> str | None:
        """The task of a process that is still running (the gateway's network gate, L2); None once it has ended, so
        a descendant that outlives it with its proxy credentials reaches nothing new, and None if it's unknown."""
        hit = self._procs.get(proc_id)
        return hit[1].owner_task if hit is not None and hit[0].exit_code is None else None

    async def kill_id(self, proc_id: str) -> bool:
        hit = self._procs.get(proc_id)
        if hit is None:
            return False
        await hit[0].kill(self._ours(proc_id))
        return True

    async def end_task(self, task_id: str) -> int:
        """Task finished: kill its processes' whole groups, except long-lived ones, whether or not the command's own
        process has ended (`cmd &` children live on in its group); report and forget the finished ones' network, and
        drop them. Returns how many were still running."""
        n = 0
        for pid, (p, info) in list(self._procs.items()):
            if info.owner_task == task_id and not info.long_lived:
                n += p.exit_code is None
                await p.kill(self._ours(pid))
        self._settle(lambda info: info.owner_task == task_id)
        return n

    async def stop_running(self, owned: Callable[[str], bool]) -> int:
        """A task paused (orchestration decision 13): the still-running commands of the tasks `owned` names stop, their
        whole groups, except long-lived ones (which outlive the task anyway); their records stay, so a resumed attempt
        can still read what they printed. Returns how many were stopped."""
        n = 0
        for pid, (p, info) in list(self._procs.items()):
            if owned(info.owner_task) and not info.long_lived and p.exit_code is None:
                n += 1
                await p.kill(self._ours(pid))
        return n

    async def kill_all(self) -> int:
        if self._watcher is not None:
            self._watcher.cancel()
        n = 0
        for pid, (p, _) in list(self._procs.items()):
            n += p.exit_code is None
            await p.kill(self._ours(pid))
        self._settle(lambda info: True)
        return n

    def _settle(self, which: Callable[[ProcInfo], bool]) -> None:
        """Processes whose task ended: a finished one is retired now (`_retire`); a running one (long-lived) is left
        alone, so later reads still show it, and retired when it exits."""
        for info in self.list():
            if which(info) and info.running:
                self._report_on_exit.add(info.id)
            elif which(info):
                self._retire(info.id)

    def _retire(self, proc_id: str) -> None:
        """A finished process whose task ended: its proxy credential is forgotten (it stops working now) and its entry
        goes once its network record is out. The proxy keeps the record and reports it through `_report` once the
        process's last connection closes, so a setsid child's late bytes still count; the entry waits for that."""
        if self.proxy is not None:
            self.proxy.forget(proc_id)
            if self.proxy.pending(proc_id):
                self._retiring.add(proc_id)
                return
        self._drop(proc_id)

    def _drop(self, proc_id: str) -> None:
        self._retiring.discard(proc_id)
        self._received.pop(proc_id, None)
        self._stopped.pop(proc_id, None)
        self._owner_input.pop(proc_id, None)
        self._procs.pop(proc_id, None)

    def purge_logs(self, before: float) -> int:
        """Output logs last written before `before` (a Unix time), of processes FERB no longer tracks (retention: the
        evidence window, 7 days). Returns how many went."""
        tracked = set(self._procs)
        n = 0
        try:
            entries = list(os.scandir(self.log_dir))
        except FileNotFoundError:
            return 0
        for entry in entries:
            if not entry.name.endswith(".log") or entry.name[:-4] in tracked:
                continue
            try:
                if entry.is_file(follow_symlinks=False) and entry.stat(follow_symlinks=False).st_mtime < before:
                    os.unlink(entry.path)
                    n += 1
            except OSError:
                continue
        return n

    def _count_final(self, proc_id: str, task: str, facts: dict[str, Any]) -> None:
        """A process's final record: the bytes it received since the watch last looked still count (on the loop: the
        proxy may report from a tunnel's thread)."""
        loop, count = self._loop, self.on_received
        if count is None or loop is None or loop.is_closed():
            return
        got = sum(int(d.get("bytes_in") or 0) for d in facts["destinations"])
        new = got - self._received.get(proc_id, 0)
        if new <= 0:
            return
        self._received[proc_id] = got

        def counted() -> None:
            try:
                count(task, new)
            except Exception:
                pass
        try:
            on_loop = asyncio.get_running_loop() is loop
        except RuntimeError:
            on_loop = False
        if on_loop:
            counted()
        else:
            loop.call_soon_threadsafe(counted)

    def _report(self, proc_id: str, facts: dict[str, Any]) -> None:
        """The proxy's final record for a forgotten process (from the daemon loop, or a tunnel's thread at close)."""
        hit = self._procs.get(proc_id)
        if hit is None or self.on_network is None:
            if hit is not None and proc_id in self._retiring:
                self._drop(proc_id)
            return
        info = hit[1]
        self._count_final(proc_id, info.owner_task, facts)
        try:
            # The direct local ports it connected to, as the kernel reported them, or all of them when unknown (L24)
            used = [d for d in self._direct(info) if d["reported_connections"] is None or d["reported_connections"] > 0]
            if info.direct and self.reports is not None:
                self.reports.forget(proc_id)
            if used:
                facts = {**facts, "local_direct": used}
            if facts["destinations"] or facts["refused"] or used:
                self.on_network(info, facts)
        finally:
            if proc_id in self._retiring:           # its record was what its entry waited for
                self._drop(proc_id)
