import asyncio
import shutil
import tempfile
import threading
from pathlib import Path
from typing import Any, Callable, Iterator

import pytest

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.daemon.main import Daemon


class RunningDaemon:
    """A real Daemon on its own event loop in a background thread (the tests talk to it over the socket)."""

    def __init__(self, home: Path, **kwargs: Any) -> None:
        self.loop = asyncio.new_event_loop()
        self.thread = threading.Thread(target=self.loop.run_forever, daemon=True)
        self.thread.start()
        self.daemon: Daemon = self.run(self._make(home, kwargs))
        self.run(self.daemon.start())

    @staticmethod
    async def _make(home: Path, kwargs: dict[str, Any]) -> Daemon:
        return Daemon(home=home, **kwargs)

    def run(self, coro: Any) -> Any:
        return asyncio.run_coroutine_threadsafe(coro, self.loop).result(timeout=10)

    def call_soon(self, fn: Callable[..., Any], *args: Any) -> None:
        """Run fn on the daemon's loop (the bus and the clock belong to it)."""
        done = threading.Event()
        self.loop.call_soon_threadsafe(lambda: (fn(*args), done.set()))
        done.wait(5)

    @property
    def sock(self) -> Path:
        return self.daemon.server.path

    def stop(self) -> None:
        self.run(self.daemon.shutdown())
        self.loop.call_soon_threadsafe(self.loop.stop)
        self.thread.join(5)


@pytest.fixture
def home() -> Iterator[Path]:
    # Short path: AF_UNIX socket paths are limited to 104 bytes on macOS (pytest's tmp_path is too long).
    d = Path(tempfile.mkdtemp(prefix="ferbt-"))
    yield d / "home"
    shutil.rmtree(d, ignore_errors=True)


@pytest.fixture
def daemon(home: Path) -> Iterator[RunningDaemon]:
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"))
    yield d
    d.stop()
