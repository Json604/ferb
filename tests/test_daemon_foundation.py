"""Daemon step 2 (kb area daemon): composition with typed ports, one events object that redacts once, the loop's
exception handler, background work kept and reported, read-only chat.status, live privacy settings, redacted ps."""

import asyncio
import json
import threading
from pathlib import Path
from typing import Any, Literal

import pytest

from ferb.cli.client import Client
from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from tests.conftest import RunningDaemon


async def _on_loop(fn: Any) -> Any:
    return fn()


def _journal(d: RunningDaemon, kind: str) -> list[dict[str, Any]]:
    async def read() -> list[dict[str, Any]]:
        return [e.data for e in d.daemon.journal.of_kind(kind)]                     # type: ignore[union-attr]
    return d.run(read())


# decision: DMN-11
def test_the_supervisor_ports_and_gateway_deps_name_every_dependency() -> None:
    """Missing wiring fails where it is built (no silent lambda default for the daemon)."""
    from ferb.daemon.gateway import GatewayDeps
    from ferb.orchestrator import ports
    for port in (ports.MemoryPort, ports.SchedulePort, ports.EnvironmentPort, ports.DesktopPort, ports.BrowserPort,
                 ports.NotifyPort, GatewayDeps):
        with pytest.raises(TypeError):
            port()                                                                   # type: ignore[call-arg]
    with pytest.raises(TypeError):                                                   # the daemon left one out
        ports.BrowserPort(browser_context=lambda t: "", task_tabs=lambda t: [])      # type: ignore[call-arg]


# decision: DMN-11
def test_the_daemon_hands_the_supervisor_its_ports_and_the_gateway_its_router(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    sup, gw = d.supervisor, d.gateway
    assert sup is not None and gw is not None
    assert sup.timers is d.timers and sup.changes is d.changes and sup.disturbance is d.disturbance
    assert gw.confirmations is d.confirmations and gw.destinations is d.destinations and gw.recovery is not None
    assert not hasattr(sup, "recall") and not hasattr(sup, "relevant_source_refs")    # hooks the daemon never set
    from ferb.orchestrator.ports import QuestionRouter
    assert isinstance(gw.route_question, QuestionRouter)
    asked: list[tuple[str, str, list[str]]] = []

    async def route(task_id: str, question: str, options: list[str]) -> dict[str, Any]:
        asked.append((task_id, question, options))
        return {"ask_owner": False, "answer": "yes"}
    sup.route_question = route                                       # type: ignore[method-assign]
    gw.route_question.bind(sup.route_question)
    assert daemon.run(gw.route_question("t1", "Which?", ["a"])) == {"ask_owner": False, "answer": "yes"}
    assert asked == [("t1", "Which?", ["a"])]


def test_an_unretrieved_exception_is_journaled_redacted_never_printed(daemon: RunningDaemon,
                                                                      capsys: pytest.CaptureFixture[str]) -> None:
    secret = "hunter2-sentinel-value"
    d = daemon.daemon
    daemon.call_soon(d.redact.add_secret_value, secret)                # type: ignore[attr-defined]

    def boom() -> None:
        raise RuntimeError(f"the token was {secret}")
    daemon.loop.call_soon_threadsafe(boom)                           # a callback that raised: the loop's handler
    for _ in range(100):
        if _journal(daemon, "daemon.unhandled"):
            break
        daemon.run(asyncio.sleep(0.02))
    rows = _journal(daemon, "daemon.unhandled")
    assert rows and rows[0]["error"] == "RuntimeError" and secret not in json.dumps(rows)
    assert any("test_daemon_foundation.py" in f for f in rows[0]["frames"])
    assert secret not in capsys.readouterr().err


# decision: DMN-12
def test_privacy_exclusions_set_in_settings_apply_at_once(daemon: RunningDaemon) -> None:
    """They used to reach the Redactor (and through it the egress proxy and hearing) only at the next start."""
    d = daemon.daemon
    assert not d.redact.excluded_host("www.bank.example")             # type: ignore[attr-defined]
    c = Client(daemon.sock)
    try:
        c.call("settings.set", {"key": "privacy.exclude_domains", "value": ["bank.example"]})
        c.call("settings.set", {"key": "privacy.exclude_apps", "value": ["com.example.Vault"]})
    finally:
        c.close()
    assert d.redact.excluded_host("www.bank.example")                  # type: ignore[attr-defined]
    assert d.hearing.excluded_host("www.bank.example")
    if d.proxy is not None:
        assert d.proxy.excluded("www.bank.example")
    assert "com.example.vault" in d.redact.exclude_apps                # type: ignore[attr-defined]


def test_ps_list_redacts_commands(daemon: RunningDaemon) -> None:
    from types import SimpleNamespace
    d = daemon.daemon
    secret = "sk-live-sentinel-0123456789abcdef"
    daemon.call_soon(d.redact.add_secret_value, secret)                # type: ignore[attr-defined]
    real = d.shells.list                                               # type: ignore[union-attr]
    d.shells.list = lambda: [SimpleNamespace(id="p1", command=f"curl -H 'Authorization: {secret}' x",  # type: ignore
                                             running=True)]
    try:
        out = daemon.run(d._ps_list(None, {}))                         # type: ignore[arg-type]
    finally:
        d.shells.list = real                                           # type: ignore[union-attr,method-assign]
    assert out["processes"][0]["id"] == "p1" and secret not in json.dumps(out)


_FREED: list[int] = []
_PROBE: list[Any] = []


def _autoreleasing_job() -> None:
    """Cocoa work that leaves an autoreleased object behind: an array holding a probe until its pool drains."""
    import objc
    from Foundation import NSArray, NSObject
    if not _PROBE:
        class FerbAutoreleaseProbe(NSObject):                 # type: ignore[misc]
            def dealloc(self) -> None:
                _FREED.append(1)
                objc.super(FerbAutoreleaseProbe, self).dealloc()
        _PROBE.append(FerbAutoreleaseProbe)
    probe = _PROBE[0].new()
    NSArray.arrayWithObject_(probe)
    del probe


# decision: DMN-26
def test_work_on_the_daemons_threads_drains_its_autorelease_pool(daemon: RunningDaemon) -> None:
    """Step 12d (footprint): an object a worker-thread job autoreleases is freed when the job ends, never kept by the
    long-lived thread: asyncio.to_thread jobs and a watcher's poll step alike."""
    import gc

    from ferb.sense.watchers.base import poll
    _FREED.clear()
    daemon.run(asyncio.to_thread(_autoreleasing_job))
    gc.collect()
    assert _FREED == [1]

    async def one_poll() -> None:
        stop = asyncio.Event()
        await poll(0.01, stop, lambda: (_autoreleasing_job(), stop.set()))
    _FREED.clear()
    daemon.run(one_poll())
    gc.collect()
    assert _FREED == [1]


class _SeenWait(asyncio.Event):
    """The server's start gate, telling the test (from the daemon's thread) once a call is waiting on it."""

    def __init__(self, waiting: threading.Event) -> None:
        super().__init__()
        self.waiting = waiting

    async def wait(self) -> Literal[True]:
        self.waiting.set()
        return await super().wait()


# decision: DMN-510
def test_the_daemon_answers_ping_while_its_slow_start_up_work_runs(home: Path) -> None:
    """Audit 2026-11-02 (item 5): the daemon dumped Codex's catalog (the 270 MB binary's first run on a new Mac) before
    it served, so FERB.app's 30 s wait for `ping` could run out on a slow Mac. It serves first: `ping` is answered while
    that work runs, and every other call waits until the start is done."""
    loop = asyncio.new_event_loop()
    thread = threading.Thread(target=loop.run_forever, daemon=True)
    thread.start()
    entered, release, waiting = threading.Event(), asyncio.Event(), threading.Event()
    d = asyncio.run_coroutine_threadsafe(RunningDaemon._make(home, {"clock": FakeClock(), "ids": SequentialIds("id"),
                                                                     "watch_owner_input": False}), loop).result(10)
    d._runtime_available = lambda name: name in ("codex", "claude")                  # type: ignore[method-assign]
    d.server.ready = _SeenWait(waiting)

    async def slow_codex() -> None:
        entered.set()
        await release.wait()
    d._prepare_codex = slow_codex                                                   # type: ignore[method-assign]
    started = asyncio.run_coroutine_threadsafe(d.start(), loop)
    try:
        assert entered.wait(10)
        first = Client(d.server.path)
        assert first.call("ping")["pid"] > 0                                       # answered mid-start
        first.close()
        answers: list[dict[str, Any]] = []

        def other() -> None:
            second = Client(d.server.path)
            answers.append(second.call("schedules.list"))
            second.close()
        caller = threading.Thread(target=other)
        caller.start()
        assert waiting.wait(10)                                                     # it reached the gate and waits
        assert answers == [] and not started.done()
        loop.call_soon_threadsafe(release.set)
        started.result(10)
        caller.join(10)
        assert len(answers) == 1                                                    # answered once the start is done
    finally:
        loop.call_soon_threadsafe(release.set)
        asyncio.run_coroutine_threadsafe(d.shutdown(), loop).result(10)
        loop.call_soon_threadsafe(loop.stop)
        thread.join(5)
