"""Own PTY transport, output storage and process-group termination. Must not truncate the retained output or choose commands.

A task's process group and owner-only redacted output log. Offsets address the full log, not a lossy ring."""

import asyncio
import errno
import os
import pty
import signal
import subprocess
import termios
import time
from pathlib import Path
from typing import Callable

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

    def send(self, text: str) -> None:
        if self.exit_code is None and self._master >= 0:
            os.write(self._master, text.encode())

    async def wait(self, timeout: float) -> int | None:
        try:
            await asyncio.wait_for(self._done.wait(), timeout)
        except asyncio.TimeoutError:
            return None
        return self.exit_code

    async def kill(self) -> None:
        if self.proc is None or self.exit_code is not None:
            return
        for sig, grace in ((signal.SIGTERM, 2.0), (signal.SIGKILL, 2.0)):
            try:
                os.killpg(self.proc.pid, sig)
            except ProcessLookupError:
                return
            if await self.wait(grace) is not None:
                return
