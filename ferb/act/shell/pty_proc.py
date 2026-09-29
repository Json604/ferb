"""One process in a PTY (section 3 act/shell): output kept in a bounded ring buffer (byte offsets, so readers can
ask "since N") and appended to a log file after redaction. The process runs in its own session/process group so
kill reaches its children."""

import asyncio
import os
import pty
import signal
import subprocess
from pathlib import Path
from typing import Callable

RING = 256 * 1024


class PtyProc:
    def __init__(self, argv: list[str], cwd: Path, env: dict[str, str], log: Path,
                 redact_text: Callable[[str], str]) -> None:
        self.argv, self.cwd, self.env, self.log, self.redact_text = argv, cwd, env, log, redact_text
        self.buf = bytearray()
        self.start_offset = 0                  # offset of buf[0] in the whole output
        self.total = 0
        self.proc: subprocess.Popen[bytes] | None = None
        self.exit_code: int | None = None
        self._master = -1
        self._done = asyncio.Event()

    async def start(self) -> None:
        master, slave = pty.openpty()
        self.proc = subprocess.Popen(self.argv, cwd=self.cwd, env=self.env, stdin=slave, stdout=slave, stderr=slave,
                                     start_new_session=True, close_fds=True)
        os.close(slave)
        os.set_blocking(master, False)
        self._master = master
        self.log.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        loop = asyncio.get_running_loop()
        loop.add_reader(master, self._on_output)
        asyncio.create_task(self._reap())

    def _on_output(self) -> None:
        try:
            data = os.read(self._master, 65536)
        except OSError:
            data = b""
        if not data:
            asyncio.get_running_loop().remove_reader(self._master)
            return
        self.buf += data
        self.total += len(data)
        if len(self.buf) > RING:
            drop = len(self.buf) - RING
            del self.buf[:drop]
            self.start_offset += drop
        with open(self.log, "a", encoding="utf-8") as f:
            f.write(self.redact_text(data.decode(errors="replace")))   # logs are redacted too (10.6)

    async def _reap(self) -> None:
        assert self.proc
        self.exit_code = await asyncio.to_thread(self.proc.wait)
        await asyncio.sleep(0.1)                                        # let the last output drain
        try:
            asyncio.get_running_loop().remove_reader(self._master)
            os.close(self._master)
        except (OSError, ValueError):
            pass
        self._done.set()

    def read(self, since: int = 0) -> tuple[str, int, bool]:
        """(text since offset, new offset, whether older output was dropped)."""
        start = max(since, self.start_offset)
        chunk = bytes(self.buf[start - self.start_offset:])
        return chunk.decode(errors="replace"), self.total, since < self.start_offset

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
