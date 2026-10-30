"""Daemon step 2 (notes/daemon.md): composition with typed ports, one events object that redacts once, the loop's
exception handler, background work kept and reported, read-only chat.status, live privacy settings, redacted ps."""

import asyncio
import json
from typing import Any

import pytest

from ferb.cli.client import Client
from ferb.core.errors import FerbError
from tests.conftest import RunningDaemon


async def _on_loop(fn: Any) -> Any:
    return fn()


def _journal(d: RunningDaemon, kind: str) -> list[dict[str, Any]]:
    async def read() -> list[dict[str, Any]]:
        return [e.data for e in d.daemon.journal.of_kind(kind)]                     # type: ignore[union-attr]
    return d.run(read())


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


def test_a_published_event_is_redacted_once_for_every_consumer(daemon: RunningDaemon) -> None:
    """`_publish` used to redact the same data for each consumer (plans, today's activity, conversations, bus)."""
    d = daemon.daemon
    real = d.redact
    calls: list[Any] = []

    def counting(value: Any) -> Any:
        calls.append(value)
        return real(value)
    daemon.run(_on_loop(d._activity_device))                         # this Mac's id, redacted once and kept
    sub = d.bus.subscribe({"trail.event"})
    d.redact = counting                                              # type: ignore[assignment]
    try:
        daemon.call_soon(d._publish, "trail.event", {"source": "app_focus", "data": {"app": "Notes"}})
    finally:
        d.redact = real
    assert len(calls) == 1
    queued = list(sub._queue)
    sub.close()
    assert [e.data["data"] for e in queued] == [{"app": "Notes"}]    # the bus's shape is unchanged


def test_a_plan_waiting_for_an_event_is_not_loaded_per_event(daemon: RunningDaemon) -> None:
    """notify_plan_event used to load and parse every registered plan on every published topic."""
    sup = daemon.daemon.supervisor
    assert sup is not None
    loads: list[str] = []
    woke: list[str] = []
    real = sup.plans.load

    class Plan:
        steps = [type("S", (), {"event": "timer.fired", "wakes_on": "timer.fired"})()]

    def load(key: str) -> dict[str, Any]:
        loads.append(key)
        return {"plan": Plan()}
    run = type("Run", (), {"wake": lambda self: woke.append("woke")})()
    sup.plans.load = load                                             # type: ignore[method-assign]
    sup._plan_runs["tree-1"] = run                                    # type: ignore[assignment]
    sup.wake_loops = lambda key: None                                 # type: ignore[method-assign]
    sup.plans.event = lambda topic, data: None                        # type: ignore[method-assign]
    try:
        for topic in ("trail.event", "tool.called", "timer.fired", "trail.event", "timer.fired"):
            daemon.call_soon(sup.notify_plan_event, topic, {})
        assert loads == ["tree-1"] and woke == ["woke", "woke"]
        sup.plans.revisions["tree-1"] = 1                              # a steer rewrote the plan: read it again
        daemon.call_soon(sup.notify_plan_event, "trail.event", {})
        assert loads == ["tree-1", "tree-1"]
    finally:
        sup._plan_runs.pop("tree-1", None)
        sup.plans.load = real                                         # type: ignore[method-assign]


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


def test_background_work_is_kept_and_its_failure_journaled(daemon: RunningDaemon) -> None:
    d = daemon.daemon

    async def fails() -> None:
        raise ValueError("background broke")

    async def spawn() -> Any:
        job = d._spawn(fails(), "a test job")
        assert job in d._background
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        return job
    job = daemon.run(spawn())
    assert job not in d._background
    assert _journal(daemon, "daemon.background_failed") == [{"what": "a test job",
                                                            "error": "ValueError: background broke"}]


def test_a_background_loop_that_raises_is_journaled_and_started_again(daemon: RunningDaemon,
                                                                       monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.daemon import main
    monkeypatch.setattr(main, "LOOP_RESTART_S", 0.0)
    runs: list[int] = []

    async def flaky() -> None:
        runs.append(1)
        if len(runs) < 3:
            raise OSError("loop broke")
    daemon.run(daemon.daemon._supervise("test loop", flaky))
    assert len(runs) == 3
    assert [r["failures"] for r in _journal(daemon, "daemon.loop_failed")] == [1, 2]


def test_chat_status_only_reads_and_results_ack_marks(daemon: RunningDaemon) -> None:
    """chat.status without `peek` used to mark every unshown result shown, so one client consumed another's."""
    d = daemon.daemon
    daemon.call_soon(lambda: d.task_results.record("t-1", "2026-10-30T09:00:00+00:00", "done", "g", "Done.",
                                                   False))                              # type: ignore[union-attr]
    c = Client(daemon.sock)
    try:
        first = c.call("chat.status")["results"]
        assert [r["task_id"] for r in first] == ["t-1"]
        assert c.call("chat.status")["results"] == first == c.call("chat.status", {"peek": True})["results"]
        assert "memory_digest" not in c.call("chat.status")
        c.call("results.ack", {"task_ids": ["t-1"]})
        assert c.call("chat.status")["results"] == []
    finally:
        c.close()


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


def test_logs_policy_filter_streams_only_policy_topics(daemon: RunningDaemon) -> None:
    from ferb.daemon.main import LOG_FILTERS

    async def first(params: dict[str, Any]) -> Any:
        stream = daemon.daemon._logs(None, params)                     # type: ignore[arg-type]
        try:
            return await stream.__anext__()
        finally:
            await stream.aclose()
    assert set(LOG_FILTERS["policy"]) == {"approval.*", "stop.*", "security.*"}
    with pytest.raises(FerbError, match="unknown log filter"):
        daemon.run(first({"filters": ["nope"]}))

    async def policy_only() -> list[str]:
        stream = daemon.daemon._logs(None, {"filters": ["policy"]})   # type: ignore[arg-type]
        task = asyncio.ensure_future(stream.__anext__())
        await asyncio.sleep(0)
        daemon.daemon._publish("trail.event", {"source": "x"})
        daemon.daemon._publish("approval.resolved", {"approval_id": "a1", "state": "granted"})
        ev = await asyncio.wait_for(task, 2)
        await stream.aclose()
        return [ev["topic"]]
    assert daemon.run(policy_only()) == ["approval.resolved"]


def test_a_failed_start_shuts_down_what_it_built(home: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.clock.clock import FakeClock
    from ferb.core.ids import SequentialIds
    from ferb.daemon.main import Daemon
    stopped: list[bool] = []
    real_shutdown = Daemon.shutdown

    async def shutdown(self: Daemon) -> None:
        stopped.append(True)
        await real_shutdown(self)

    def broken(self: Daemon) -> None:
        raise RuntimeError("timers broke")
    monkeypatch.setattr(Daemon, "shutdown", shutdown)
    monkeypatch.setattr(Daemon, "_build_timers", broken)
    d = RunningDaemon.__new__(RunningDaemon)
    import threading
    d.loop = asyncio.new_event_loop()
    d.thread = threading.Thread(target=d.loop.run_forever, daemon=True)
    d.thread.start()
    try:
        d.daemon = d.run(RunningDaemon._make(home, {"clock": FakeClock(), "ids": SequentialIds("id"),
                                                    "watch_owner_input": False}))
        with pytest.raises(RuntimeError, match="timers broke"):
            d.run(d.daemon.start(), timeout=30)
        assert stopped == [True]
        assert not d.daemon.server.path.exists()                      # nothing left serving
        from ferb.store.db import Database
        db = Database(home / "ferb.db")
        kinds = [r[0] for r in db.conn.execute("SELECT kind FROM journal WHERE kind LIKE 'daemon.%' ORDER BY seq")]
        db.close()
        assert kinds[-2:] == ["daemon.start_failed", "daemon.stopped"]
    finally:
        d.loop.call_soon_threadsafe(d.loop.stop)
        d.thread.join(5)
