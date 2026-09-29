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

pytest_plugins = ["tests.browserkit"]           # browser fixtures: local pages, temporary keychain


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
        kwargs.setdefault("watchers", False)            # the owner's real activity stays out of test trails
        kwargs.setdefault("distill", False)             # the daily distillation calls a model
        kwargs.setdefault("notifications", False)       # no macOS notifications from tests
        return Daemon(home=home, **kwargs)

    def run(self, coro: Any, timeout: float = 10) -> Any:
        return asyncio.run_coroutine_threadsafe(coro, self.loop).result(timeout=timeout)

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
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    yield d
    d.stop()


@pytest.fixture(autouse=True)
def no_real_models(request: pytest.FixtureRequest, monkeypatch: pytest.MonkeyPatch) -> None:
    """Offline tests must never reach a real model (cost, flakiness): only `-m real` tests may."""
    if request.node.get_closest_marker("real"):
        return

    async def refuse(*a: Any, **k: Any) -> None:
        raise AssertionError("an offline test tried to start a real model runtime; mark it `real` or use a fake")
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    from ferb.runtimes.codex_exec import CodexRuntime
    monkeypatch.setattr(CodexRuntime, "start", refuse)
    monkeypatch.setattr(ClaudeRuntime, "start", refuse)
