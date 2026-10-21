"""Shared bus test support: a FERB client holding a `logs.subscribe` stream, as the notch UI and `ferb chat` do, and
waiting for something to become true."""
import threading
import time
from typing import Any, Callable

from ferb.cli.client import Client
from tests.conftest import RunningDaemon


def until(check: Callable[[], Any], what: str) -> Any:
    for _ in range(200):
        value = check()
        if value:
            return value
        time.sleep(0.02)
    raise AssertionError("timed out waiting for " + what)


class Subscriber:
    """An authenticated FERB client holding a bus subscription: unfiltered, as the notch UI and `ferb chat` do, or
    filtered, as `ferb logs --filter` and `ferb voice` do."""

    def __init__(self, d: RunningDaemon, filters: list[str] | None = None) -> None:
        self.d, self.client, self.events = d, Client(d.sock), []          # type: list[dict[str, Any]]
        self.filters = filters
        before = len(d.daemon._log_subscriptions)
        self.thread = threading.Thread(target=self._read, daemon=True)
        self.thread.start()
        until(lambda: len(d.daemon._log_subscriptions) > before, "the subscription to open")

    def _read(self) -> None:
        try:
            for event in self.client.stream("logs.subscribe", {"filters": self.filters} if self.filters else {}):
                self.events.append(event)
        except Exception:
            return

    def topic(self, topic: str) -> list[dict[str, Any]]:
        return [e["data"] for e in self.events if e["topic"] == topic]

    def close(self) -> None:
        before = len(self.d.daemon._log_subscriptions)
        self.client.interrupt()
        self.client.close()
        self.thread.join(5)
        until(lambda: len(self.d.daemon._log_subscriptions) < before, "the subscription to close")
