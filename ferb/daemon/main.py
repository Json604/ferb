"""Daemon startup / shutdown lifecycle (section 2) and its methods.

Build step 1: ping, trace.get, logs.subscribe. Build step 2: the redaction pipeline on every writer and on
`ferb logs`, the policy engine + ledger + undo-window releases, `ferb stop` / `resume-tasks` / `undo` /
`autonomy`. The emergency stop survives a daemon restart (STOPPED marker) until the owner lifts it.
"""

import asyncio
import os
import signal
import sys
from dataclasses import asdict
from pathlib import Path
from typing import Any, AsyncIterator, Callable

from ferb.bus import DROPPED, Bus, Overflow
from ferb.clock.clock import SystemClock
from ferb.config import Config
from ferb.daemon.gateway import FsRoots, Gateway
from ferb.orchestrator.supervisor import Supervisor
from ferb.orchestrator.tree import TaskTree
from ferb.registry import build as build_registry
from ferb.runtimes.proxy import ToolListProxy
from ferb.core.errors import FerbError
from ferb.core.ids import RandomIds
from ferb.core.interfaces import Clock, IdSource
from ferb.daemon.auth import RunTokens
from ferb.daemon.server import Server, Session, ensure_socket_free
from ferb.memory.journal import Journal
from ferb.orchestrator.ledger import Ledger
from ferb.orchestrator.stop import StopControl
from ferb.paths import ferb_home, socket_path
from ferb.policy.autonomy import Level
from ferb.policy.engine import PolicyEngine, Verdict
from ferb.sanitize.redact import Redactor
from ferb.store.db import Database
from ferb.store.spans import SpanNode, SpanStore

Redact = Callable[[Any], Any]

# `ferb logs` filter flags -> bus topic patterns
LOG_FILTERS = {"tasks": "task.*", "trail": "trail.event", "tools": "tool.called", "browsers": "browser.*",
               "policy": "policy.*", "actions": "action.*"}
RELEASE_TICK_S = 0.2


class Daemon:
    def __init__(self, home: Path | None = None, clock: Clock | None = None, ids: IdSource | None = None,
                 redact: Redact | None = None, owner_uid: int | None = None, undo_window_s: float = 10.0) -> None:
        self.home = home or ferb_home()
        self.clock = clock or SystemClock()
        self.ids = ids or RandomIds()
        self.config = Config(self.home)
        self._redact_override = redact
        self.redact: Redact = redact or Redactor()
        self.bus = Bus(self.clock)
        self.tokens = RunTokens(self.clock)
        self.server = Server(socket_path(self.home), self.tokens, owner_uid)
        self.engine = PolicyEngine(self.clock, str(self.home / "workspace"), level=self._level,
                                   trusted_kinds=self._trusted_kinds, undo_window_s=undo_window_s)
        self.stop_control = StopControl(self.tokens.revoke_all, self._set_blocked, self.engine.holds.undo_all)
        self.db: Database | None = None
        self.journal: Journal | None = None
        self.spans: SpanStore | None = None
        self.ledger: Ledger | None = None
        self._release_task: asyncio.Task[None] | None = None
        self._autonomy = Level.BALANCED
        self.registry = build_registry()
        self.tasks = TaskTree()
        self.gateway: Gateway | None = None
        self.supervisor: Supervisor | None = None
        self._proxy: ToolListProxy | None = None

    @property
    def stop_marker(self) -> Path:
        return self.home / "STOPPED"

    async def start(self) -> None:
        self.home.mkdir(parents=True, exist_ok=True)
        os.chmod(self.home, 0o700)
        ensure_socket_free(self.server.path)   # refuse before touching the database if another daemon runs
        self._load_config()
        self.db = Database(self.home / "ferb.db")
        self.db.migrate()
        self.journal = Journal(self.db, self.clock, self.ids, self.redact)
        self.spans = SpanStore(self.db, self.clock, self.ids, self.redact)
        self.ledger = Ledger(self.db, self.clock, self.ids)
        self._build_agents()
        for name, fn in (("ping", self._ping), ("trace.get", self._trace), ("stop.engage", self._stop_engage),
                         ("stop.lift", self._stop_lift), ("undo", self._undo), ("autonomy.get", self._autonomy_get),
                         ("autonomy.set", self._autonomy_set), ("doctor.check", self._doctor_check),
                         ("automation.request", self._automation_request), ("tools.list", self._tools_list),
                         ("tools.call", self._tools_call), ("chat.approve", self._chat_approve)):
            self.server.unary[name] = fn
        self.server.streams["logs.subscribe"] = self._logs
        self.server.streams["chat.send"] = self._chat_send
        await self.server.start()          # refuses to start if another daemon answers on the socket
        self.journal.append("daemon.started", {"pid": os.getpid()})
        unknown = self.ledger.mark_unknown_after_restart()
        if unknown:
            self.journal.append("ledger.unknown_after_restart", {"intent_ids": unknown})
        if self.stop_marker.exists():      # a stop outlives a crash/restart until the owner lifts it
            self.engine.stopped = True
            self.stop_control.engaged = True
            self.journal.append("stop.still_engaged", {})
        self._release_task = asyncio.create_task(self._release_loop())

    async def shutdown(self) -> None:
        if self._release_task:
            self._release_task.cancel()
        if self.supervisor:
            self.supervisor.cancel_all()
        if self._proxy:
            self._proxy.close()
        await self.server.close()
        if self.journal:
            self.journal.append("daemon.stopped", {"pid": os.getpid()})
        if self.db:
            self.db.close()

    async def run(self) -> None:
        """Run until SIGTERM / SIGINT, then shut down cleanly."""
        await self.start()
        loop = asyncio.get_running_loop()
        done = asyncio.Event()
        for sig in (signal.SIGTERM, signal.SIGINT):
            loop.add_signal_handler(sig, done.set)
        await done.wait()
        await self.shutdown()

    # ---------- policy plumbing ----------

    def publish_verdict(self, v: Verdict) -> None:
        for topic, data in v.events:
            self.bus.publish(topic, self.redact(data))

    def _set_blocked(self, blocked: bool) -> None:
        self.engine.stopped = blocked
        if blocked:
            self.stop_marker.touch(mode=0o600)
        else:
            self.stop_marker.unlink(missing_ok=True)

    def _level(self) -> Level:
        return self._autonomy

    def _trusted_kinds(self) -> frozenset[str]:
        return frozenset(self.config.get("policy", "trusted_kinds", []))

    def _load_config(self) -> None:
        self._autonomy = Level(self.config.get("policy", "autonomy", Level.BALANCED.value))
        if self._redact_override is None:
            self.redact = Redactor(set(self.config.get("privacy", "exclude_domains", [])),
                                   set(self.config.get("privacy", "exclude_apps", [])))

    async def _release_loop(self) -> None:
        """Undo windows: publish releases as holds come due (the gateway executes them from build step 4)."""
        while True:
            for _hold, verdict in self.engine.release_due():
                self.publish_verdict(verdict)
            await asyncio.sleep(RELEASE_TICK_S)

    # ---------- agents: registry, gateway, runtimes, supervisor (build step 4) ----------

    def _build_agents(self) -> None:
        device = str(self.config.get("device", "id", "unknown-device"))
        workspace = self.home / "workspace"
        workspace.mkdir(parents=True, exist_ok=True)
        roots = FsRoots(str(workspace), tuple(self.config.get("fs", "owner_roots", [])),
                        tuple(self.config.get("fs", "untrusted_roots", ["~/Downloads"])))
        assert self.ledger and self.spans and self.journal
        self.gateway = Gateway(self.registry.capabilities, self.engine, self.ledger, self.spans, self.tasks,
                               self.clock, self.redact, self._publish, roots, device)
        self.supervisor = Supervisor(make_runtime=self._make_runtime, gateway=self.gateway, engine=self.engine,
                                     tokens=self.tokens, tasks=self.tasks, spans=self.spans, journal=self.journal,
                                     publish=self._publish, clock=self.clock, ids=self.ids, device_id=device,
                                     workspace=str(workspace), on_violation=self._runtime_violation)
        self.stop_control.register("runtimes", self.supervisor.cancel_all)

    def _publish(self, topic: str, data: dict[str, Any]) -> None:
        self.bus.publish(topic, self.redact(data))

    def _runtime_violation(self, runtime: str, detail: str) -> None:
        """Section 7: a run that shows a tool outside the allowlist unregisters its runtime."""
        self.registry.unregistered[runtime] = detail
        self._journal().append("runtime.unregistered", {"runtime": runtime, "detail": detail})
        self._publish("policy.blocked", {"step": "runtime", "reason": f"{runtime} unregistered: {detail}"})

    def _make_runtime(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any]) -> Any:
        name = str(self.config.get("runtimes", "orchestrator" if role == "orchestrator" else "worker", "codex"))
        if name not in self.registry.runtimes:
            raise FerbError(f"unknown runtime {name!r} in config.toml [runtimes]")
        if name in self.registry.unregistered:
            raise FerbError(f"runtime {name} is unregistered: {self.registry.unregistered[name]}")
        shim = [sys.executable, "-m", "ferb.cli.mcp"]
        if name == "codex":
            from ferb.device.install import repo_root
            from ferb.runtimes.codex_exec import CodexRuntime, codex_binary
            workdir = self.home / "workspace" if mode == "worker" else self.home / "run" / "supervisor"
            return CodexRuntime(codex_binary(self.home, repo_root()), mode, workdir, self.home,  # type: ignore[arg-type]
                                shim_cmd=shim, tool_names=tool_names, output_schema=schema)
        from ferb.runtimes.claude_sdk import ClaudeRuntime
        if self._proxy is None:
            self._proxy = ToolListProxy()
        return ClaudeRuntime(mode, self._proxy, str(self.home), shim_cmd=shim, tool_names=tool_names,  # type: ignore[arg-type]
                             output_schema=schema, model=self.config.get("runtimes", "claude_model"))

    async def _tools_list(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if session.grant is None or self.gateway is None:
            raise FerbError("no run token")
        return {"tools": self.gateway.tools_for(session.grant)}

    async def _tools_call(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        if session.grant is None or self.gateway is None:
            raise FerbError("no run token")
        return await self.gateway.call(session.grant, str(params.get("name")), dict(params.get("arguments") or {}))

    async def _chat_approve(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        ap = self.engine.approvals.resolve(str(params["approval_id"]), bool(params["granted"]))
        self._journal().append("approval.resolved", {"approval_id": ap.id, "state": ap.state,
                                                     "capability": ap.capability})
        self._publish("approval.resolved", {"approval_id": ap.id, "state": ap.state})
        return {"approval_id": ap.id, "state": ap.state}

    async def _chat_send(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        """One chat turn: the supervisor's events, merged with bus events about the tasks this turn started
        (tool calls, approval requests, undo countdowns, blocks) so `ferb chat` can show them inline."""
        if self.supervisor is None:
            raise FerbError("daemon not started")
        queue: asyncio.Queue[dict[str, Any] | None] = asyncio.Queue()
        my_tasks: set[str] = set()
        sub = self.bus.subscribe({"tool.called", "approval.requested", "action.*", "policy.blocked",
                                  "worker.exposed"})

        async def turn() -> None:
            try:
                async for ev in self.supervisor.send(str(params.get("session_id", "default")),  # type: ignore[union-attr]
                                                     str(params.get("text", ""))):
                    if ev.get("task_id"):
                        my_tasks.add(ev["task_id"])
                    await queue.put(ev)
            except FerbError as e:
                await queue.put({"type": "error", "text": str(e)})
            except Exception as e:                       # never a silent chat: the owner sees what broke
                await queue.put({"type": "error", "text": f"internal error: {type(e).__name__}: {self.redact(str(e))}"})
            finally:
                await queue.put(None)

        async def bus_events() -> None:
            async for ev in sub:
                tid = ev.data.get("task_id")
                if tid in my_tasks or (ev.topic.startswith("action.") and not tid):
                    await queue.put({"type": "event", "topic": ev.topic, "data": self.redact(ev.data)})

        t1, t2 = asyncio.create_task(turn()), asyncio.create_task(bus_events())
        try:
            while (item := await queue.get()) is not None:
                yield item
        finally:
            t2.cancel()
            sub.close()
            if not t1.done():
                t1.cancel()

    def _journal(self) -> Journal:
        if self.journal is None:
            raise FerbError("daemon not started")
        return self.journal

    # ---------- methods ----------

    async def _ping(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"pid": os.getpid(), "role": session.role.value, "now": self.clock.now().isoformat(),
                "stopped": self.engine.stopped}

    async def _trace(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        task_id = str(params.get("task_id", ""))
        if self.spans is None:
            raise FerbError("daemon not started")
        return {"task_id": task_id, "spans": [_node_json(n) for n in self.spans.tree(task_id)]}

    async def _stop_engage(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        summary = self.stop_control.engage()
        self._journal().append("stop.engaged", summary)
        self.bus.publish("stop.engaged", summary)
        return summary

    async def _stop_lift(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        self.stop_control.lift()
        self._journal().append("stop.lifted", {})
        self.bus.publish("stop.lifted", {})
        return {"stopped": False}

    async def _undo(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """`ferb undo [id]`: with no id, undo the only pending send (or list them if several)."""
        hold_id = params.get("hold_id")
        pending = self.engine.holds.pending()
        if hold_id is None:
            if len(pending) != 1:
                return {"undone": None, "pending": [{"hold_id": h.id, "capability": h.action.capability,
                                                     "kind": h.action.kind} for h in pending]}
            hold_id = pending[0].id
        if not self.engine.undo(str(hold_id)):
            raise FerbError(f"nothing to undo for {hold_id} (already sent, undone, or unknown)")
        self._journal().append("action.undone", {"hold_id": hold_id})
        self.bus.publish("action.undone", {"hold_id": hold_id})
        return {"undone": hold_id}

    async def _autonomy_get(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        return {"level": self._autonomy.value}

    async def _autonomy_set(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        level = Level(str(params.get("level")))
        self.config.set("policy", "autonomy", level.value)       # atomic write, stays 0600
        self._autonomy = level
        self._journal().append("autonomy.set", {"level": level.value})
        return {"level": level.value}

    async def _doctor_check(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Permissions and self-test as seen by *this* process — the one that will really act (section 8)."""
        import ferb
        from ferb.device import permissions, selftest
        from ferb.device.install import code_hash

        def run() -> dict[str, Any]:
            results = selftest.run_all() if params.get("selftest", True) else []
            return {"permissions": permissions.check_all(),
                    "selftest": [{"name": r.name, "ok": r.ok, "detail": r.detail} for r in results]}
        out = await asyncio.to_thread(run)
        out["runtimes"] = await self._runtime_status()
        out.update({"pid": os.getpid(), "executable": os.path.realpath(sys.executable),
                    "version": ferb.__version__, "code_hash": code_hash(Path(ferb.__file__).parent),
                    "home": str(self.home), "stopped": self.engine.stopped})
        return out

    async def _runtime_status(self) -> dict[str, Any]:
        import importlib.metadata
        from ferb.device.install import repo_root
        from ferb.runtimes.claude_sdk import SDK_VERSION
        from ferb.runtimes.codex_exec import CODEX_VERSION, check_version, codex_binary
        status: dict[str, Any] = {
            "orchestrator": self.config.get("runtimes", "orchestrator", "codex"),
            "worker": self.config.get("runtimes", "worker", "codex"),
            "unregistered": dict(self.registry.unregistered)}
        try:
            binary = codex_binary(self.home, repo_root())
            await check_version(binary)
            status["codex"] = f"{CODEX_VERSION} ({binary})"
        except FerbError as e:
            status["codex"] = f"problem: {e}"
        installed = importlib.metadata.version("claude-agent-sdk")
        status["claude"] = installed if installed == SDK_VERSION else f"problem: {installed} != pinned {SDK_VERSION}"
        return status

    async def _automation_request(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Ask macOS for Apple Events permission to each app (shows the system prompt; must run in the daemon so
        the grant attaches to FERB's interpreter). Apps that aren't running are asked on first use instead."""
        from ferb.device import permissions
        bundle_ids = [str(b) for b in params.get("bundle_ids", []) if b != "com.brave.Browser"]  # never Brave
        results = await asyncio.to_thread(lambda: {b: permissions.automation(b, ask=True).value for b in bundle_ids})
        self._journal().append("automation.requested", results)
        return {"results": results}

    async def _logs(self, session: Session, params: dict[str, Any]) -> AsyncIterator[dict[str, Any]]:
        wanted = {LOG_FILTERS[f] for f in params.get("filters", []) if f in LOG_FILTERS} or None
        sub = self.bus.subscribe(wanted, maxsize=int(params.get("maxsize", 1000)))
        try:
            async for ev in sub:
                yield {"topic": ev.topic, "at": ev.at.isoformat(),
                       "class": "notice" if ev.topic == DROPPED else ev.cls.value, "data": self.redact(ev.data)}
        except Overflow:
            yield {"topic": "bus.overflow", "at": self.clock.now().isoformat(), "class": "notice",
                   "data": {"message": "fell behind on critical events; restart `ferb logs`"}}
        finally:
            sub.close()


def _node_json(node: SpanNode) -> dict[str, Any]:
    s = asdict(node.span)
    for k in ("started_at", "ended_at"):
        s[k] = s[k].isoformat() if s[k] else None
    s["children"] = [_node_json(c) for c in node.children]
    return s


def main() -> None:
    asyncio.run(Daemon().run())
