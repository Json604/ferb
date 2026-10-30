"""Own PTY transport, output storage and process-group termination. Must not truncate the retained output or choose commands.

A task's process group and owner-only redacted output log. Offsets address the full log, not a lossy ring."""

import asyncio
import ctypes
import errno
import os
import pty
import signal
import subprocess
import termios
import time
from pathlib import Path
from typing import Any, Callable

from ferb.sanitize.redact import RedactionStream, Redactor

# Fixed code only: the approved command and environment are positional argv. Discard sudo's password pipe even
# for NOPASSWD before the task gets stdin. A TERM forwarded by sudo kills the elevated command's process group.
ADMIN_SUPERVISOR = ('exec 0<"$1"; shift; trap \'kill -KILL 0\' TERM HUP INT; '
                    '/usr/bin/env -i "$@" <&0 & child=$!; wait "$child"; status=$?; exit "$status"')


class PtyProc:
    def __init__(self, argv: list[str], cwd: Path, env: dict[str, str], log: Path,
                 redact_text: Callable[[str], str],
                 stream_factory: Callable[[], RedactionStream] | None = None,
                 on_exit: Callable[[], None] | None = None, *, admin_secret: str | None = None) -> None:
        self.argv, self.cwd, self.env, self.log, self.redact_text = argv, cwd, env, log, redact_text
        self.on_exit = on_exit                   # after the process ended and its output was drained
        self._admin_secret = admin_secret
        self._redaction = (stream_factory or Redactor().stream)()
        self.total = 0
        self.started_at: float | None = None
        self.ready_ms: float | None = None
        self.proc: subprocess.Popen[bytes] | None = None
        self.exit_code: int | None = None
        self.capture_error: str | None = None
        self._master = self._log_fd = -1
        self._output_closed = False
        self._done = asyncio.Event()

    async def start(self) -> None:
        self.started_at = time.monotonic()
        master, slave = pty.openpty()
        self._master = master
        try:
            settings = termios.tcgetattr(slave)
            settings[1] &= ~termios.ONLCR  # preserve registered multiline values byte-for-byte
            if self._admin_secret is not None:
                settings[3] &= ~termios.ECHO
            termios.tcsetattr(slave, termios.TCSANOW, settings)
            self.log.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            self._log_fd = os.open(self.log, os.O_CREAT | os.O_TRUNC | os.O_WRONLY, 0o600)
            os.fchmod(self._log_fd, 0o600)
            argv = self.argv
            secret, self._admin_secret = self._admin_secret, None
            if secret is not None:
                if argv[:2] != ["/usr/bin/sandbox-exec", "-f"]:
                    raise ValueError("an admin command must keep FERB's sandbox")
                argv = ["/usr/bin/sudo", "-S", "-k", "-p", "", "--", *argv[:3], "/bin/sh", "-c",
                        ADMIN_SUPERVISOR, "ferb-admin", os.ttyname(slave),
                        *[f"{key}={value}" for key, value in self.env.items()], *argv[3:]]
            self.proc = subprocess.Popen(argv, cwd=self.cwd, env=self.env,
                                         stdin=subprocess.PIPE if secret is not None else slave, stdout=slave,
                                         stderr=slave, start_new_session=True, close_fds=True)
            if secret is not None:
                assert self.proc.stdin is not None
                try:
                    self.proc.stdin.write((secret + "\n").encode())
                    self.proc.stdin.flush()
                except BrokenPipeError:
                    pass
                finally:
                    self.proc.stdin.close()
                    secret = None
            os.set_blocking(master, False)
            asyncio.get_running_loop().add_reader(master, self._on_output)
            asyncio.create_task(self._reap())
        except BaseException:
            if self.proc is not None:
                try:
                    os.killpg(self.proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                await asyncio.to_thread(self.proc.wait)
            self._close_output()
            raise
        finally:
            os.close(slave)

    def _append(self, data: bytes, *, final: bool = False) -> None:
        # The production stream includes registered secrets. The extra callback keeps callers' contextual
        # redaction, but it never sees half a UTF-8 character or a partial record from the raw PTY.
        safe = self.redact_text(self._redaction.feed(data, final=final).decode('utf-8')).encode('utf-8')
        while safe:
            written = os.write(self._log_fd, safe)
            if written == 0:
                raise OSError(errno.EIO, 'zero-length log write')
            self.total += written
            safe = safe[written:]

    def _on_output(self) -> None:
        if self._output_closed:
            return
        try:
            data = os.read(self._master, 65536)
        except BlockingIOError:
            return
        except OSError as exc:
            if exc.errno != errno.EIO:
                self._capture_failed(exc)
            data = b''                         # a PTY commonly signals EOF with EIO
        if data:
            self._ready()
            try:
                self._append(data)
            except Exception as exc:
                self._capture_failed(exc)
                self._close_output()
        else:
            self._close_output()

    def _ready(self) -> None:
        if self.ready_ms is None and self.started_at is not None:
            self.ready_ms = (time.monotonic() - self.started_at) * 1000

    def _capture_failed(self, exc: Exception) -> None:
        # Redactor exceptions may contain raw output: report type/errno without replaying its arguments.
        code = f' (errno {exc.errno})' if isinstance(exc, OSError) else ''
        self.capture_error = f'{type(exc).__name__}{code}: shell output capture failed; output is incomplete'

    def _close_output(self) -> None:
        if self._output_closed:
            return
        self._output_closed = True
        if self._master >= 0:
            asyncio.get_running_loop().remove_reader(self._master)
            os.close(self._master)
            self._master = -1
        if self._log_fd >= 0:
            try:
                if self.capture_error is None:
                    self._append(b'', final=True)
            except Exception as exc:
                self._capture_failed(exc)
            finally:
                os.close(self._log_fd)
                self._log_fd = -1

    async def _reap(self) -> None:
        assert self.proc is not None
        self.exit_code = await asyncio.to_thread(self.proc.wait)
        self._ready()
        try:
            # Drain bytes already written before exit; do not rely on a fixed sleep or leave a descendant's
            # inherited PTY descriptor holding up the result of the tracked command.
            while not self._output_closed:
                try:
                    chunk = os.read(self._master, 65536)
                except (BlockingIOError, OSError):
                    break
                if not chunk:
                    break
                self._append(chunk)
        except Exception as exc:
            self._capture_failed(exc)
        finally:
            try:
                self._close_output()
            finally:
                self._done.set()
                if self.on_exit is not None:
                    callback, self.on_exit = self.on_exit, None
                    callback()

    def read(self, since: int = 0, limit: int | None = None) -> tuple[str, int, bool]:
        """Read full redacted history by byte offset. Returned offsets never split a UTF-8 character."""
        start = min(max(0, since), self.total)
        with self.log.open('rb') as f:
            f.seek(start)
            if start > 0:
                # An explicitly requested offset (or tail) can start inside a character.
                while start < self.total:
                    byte = f.read(1)
                    if not byte or byte[0] & 0xC0 != 0x80:
                        break
                    start += 1
                f.seek(start)
            chunk = f.read(min(limit, self.total - start) if limit is not None else self.total - start)
            # Include the remaining bytes of the final character, at most three extra bytes per page.
            if chunk and start + len(chunk) < self.total:
                for _ in range(3):
                    try:
                        chunk.decode('utf-8')
                        break
                    except UnicodeDecodeError as error:
                        if error.reason != 'unexpected end of data':
                            break
                        chunk += f.read(1)
        return chunk.decode('utf-8', errors='replace'), start + len(chunk), False

    def line_limit(self) -> int | None:
        """The longest line, in bytes with its newline, the command's terminal takes while it reads lines (canonical
        mode): a longer line is dropped whole by the terminal driver while the write still reports every byte
        (measured: 1,023 characters and a newline arrive, 1,024 don't; MAX_CANON is 1,024). None when the command reads
        raw input (no such limit) or isn't running."""
        if self.exit_code is not None or self._master < 0:
            return None
        try:
            if not termios.tcgetattr(self._master)[3] & termios.ICANON:
                return None
            return int(os.fpathconf(self._master, "PC_MAX_CANON"))
        except (OSError, termios.error, ValueError):
            return None

    def write(self, data: bytes) -> int:
        """Writes what the terminal takes now and returns how many bytes went in (0 once the command ended); never
        blocks: a full input buffer takes fewer bytes, or none."""
        if self.exit_code is not None or self._master < 0 or not data:
            return 0
        try:
            return os.write(self._master, data)
        except BlockingIOError:
            return 0
        except OSError as e:
            if e.errno in (errno.EIO, errno.EBADF):          # the terminal closed: the command ended
                return 0
            raise

    def send(self, text: str) -> int:
        return self.write(text.encode())

    async def wait(self, timeout: float) -> int | None:
        try:
            await asyncio.wait_for(self._done.wait(), timeout)
        except asyncio.TimeoutError:
            return None
        return self.exit_code

    async def kill(self, members_ours: Callable[[list[int]], bool] | None = None) -> None:
        """End the command's whole process group: TERM, then KILL. While the command's own process runs, its group id
        can't belong to anyone else. Once it has ended, children it left (`cmd &`, a daemonized server) keep the group
        alive, but if they're gone too the id could since name an unrelated process's group (a reused pid): then the
        group is signalled only after `members_ours` confirms every process now in it is the command's (FERB's
        sandbox); without that check, or if it can't tell, nothing is signalled."""
        if self.proc is None:
            return
        pgid = self.proc.pid
        for sig, grace in ((signal.SIGTERM, 2.0), (signal.SIGKILL, 2.0)):
            if self.exit_code is not None:
                members = group_members(pgid)
                if not members or members_ours is None or not members_ours(members):
                    return
            try:
                os.killpg(pgid, sig)
            except (ProcessLookupError, PermissionError):
                return
            if self.exit_code is None and await self.wait(grace) is None:
                continue
            for _ in range(int(grace / 0.05)):               # the leader is gone: wait for the rest of its group
                if not group_members(pgid):
                    return
                await asyncio.sleep(0.05)


_PROC_PGRP_ONLY = 2                               # libproc's proc_listpids type: the processes of one group
try:
    _listpids: Any = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True).proc_listpids
    _listpids.argtypes = (ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_int)
    _listpids.restype = ctypes.c_int
except (OSError, AttributeError):                 # not macOS: no group listing, so no signal after the command ended
    _listpids = None


def group_members(pgid: int) -> list[int]:
    """The processes now in process group `pgid` (libproc proc_listpids); empty when none, or when it can't be read."""
    if _listpids is None or pgid <= 0:
        return []
    size = 256
    while True:
        buf = (ctypes.c_int * size)()
        n = _listpids(_PROC_PGRP_ONLY, pgid, buf, ctypes.sizeof(buf))
        if n <= 0:
            return []
        count = n // ctypes.sizeof(ctypes.c_int)
        if count < size:
            return [pid for pid in buf[:count] if pid > 0]
        size *= 4
