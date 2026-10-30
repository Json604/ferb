"""Own building model runtimes per role (D32) from FERB's runtime table (ferb/runtimes/catalog.py) and the runtime RPCs.
Must not pick a model the owner's config doesn't name (a runtime's own default aside)."""

import asyncio
import json
import sys
import time
from pathlib import Path
from typing import Any

from ferb.core.errors import FerbError
from ferb.core.types import Identity
from ferb.daemon.server import Session
from ferb.runtimes.proxy import ToolListProxy


def mcp_shim_command() -> list[str]:
    """How runtimes start FERB's MCP shim. Isolated (-I): they start it in FERB's workspace, which workers write, and
    `python -m` would otherwise put that folder first on sys.path, ahead of FERB's own package."""
    return [sys.executable, "-I", "-m", "ferb.cli.mcp"]


class Runtimes:
    """A mixin of the Daemon (ferb/daemon/main.py): its methods use the daemon's state."""

    def _runtime_violation(self, runtime: str, detail: str) -> None:
        """Record this run’s tool-source violation; the orchestrator can select another attempt."""
        self._journal().append("runtime.failed", {"runtime": runtime, "detail": detail})

    def _runtime_for(self, role: str) -> str:
        """The runtime a role uses: a planner-chosen worker type, else [runtimes] orchestrator / worker / checker."""
        if role.startswith("worker:") and role != "worker:default":
            return role.partition(":")[2]
        if role == "orchestrator":
            return str(self.config.get("runtimes", "orchestrator", "codex"))
        worker = str(self.config.get("runtimes", "worker", "codex"))
        return str(self.config.get("runtimes", "checker", worker)) if role == "checker" else worker

    def _model_for(self, name: str) -> str | None:
        """The model the owner set for a runtime; None when they set none (the runtime's default applies)."""
        from ferb.runtimes.catalog import configured_model
        return configured_model(self.config, name)

    def _worker_types(self) -> list[str]:
        """What the planner may name as a worker type: the role's own runtime ("default"), then every runtime that can
        run here (a model set and signed in), in the table's order."""
        from ferb.runtimes.catalog import CATALOG
        return ["default", *[name for name in CATALOG if self._runtime_available(name)]]

    def _make_runtime(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any],
                      effort: str | None = None) -> Any:
        return self._build_runtime(self._runtime_for(role), role, mode=mode, tool_names=tool_names, schema=schema,
                                   effort=effort)

    def _build_runtime(self, name: str, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any],
                       effort: str | None = None) -> Any:
        """One runtime for a role (D32), through its factory in the runtime table. `effort`: the reasoning effort a
        worker-mode run asks for (L39: "high" for the checker of a step that changed files), each adapter's own
        setting; None keeps the runtime's default. The call's time limit: the planner's chat turn 90 s, another
        non-worker call CALL_LIMIT_S, a worker none (catalog.call_limit)."""
        from ferb.orchestrator.schemas import SUPERVISOR_SCHEMA
        from ferb.runtimes.catalog import CATALOG, Build, call_limit, model_for
        entry = CATALOG.get(name)
        if entry is None:
            if role.startswith("worker:"):          # the planner's choice of worker type, not the owner's config
                raise FerbError(f"there is no worker type {name!r}; the worker types are "
                                f"{', '.join(self._worker_types())}")
            raise FerbError(f"unknown runtime {name!r} in config.toml [runtimes]; runtimes: {', '.join(CATALOG)}")
        planner = role == "orchestrator" and mode == "supervisor" and schema is SUPERVISOR_SCHEMA
        return entry.build(Build(self.home, role, mode, tool_names, schema,  # type: ignore[arg-type, attr-defined]
                                 model_for(self.config, name), self._runtime_deps(), effort, planner,
                                 call_limit(mode, planner)))

    def _runtime_deps(self) -> Any:
        from ferb.runtimes.api import GatewayBridge
        from ferb.runtimes.catalog import RuntimeDeps
        return RuntimeDeps(config=self.config, shim_cmd=mcp_shim_command(), redact=self.redact,
                           bridge=lambda: GatewayBridge(self.gateway, self.tokens), api_key=self._api_key,
                           proxy=self._claude_proxy, on_abandoned=self.server.abandon,
                           cache_allowed=self._codex_cache_turn, cache_fallback=self._codex_cache_fallback,
                           device_id=str(self.config.get("device", "id", "unknown-device")))

    def _claude_proxy(self) -> ToolListProxy:
        if self._proxy is None:
            self._proxy = ToolListProxy()
        return self._proxy

    async def _sweep_stale_runs(self) -> None:
        """Review M1: Codex run folders a crash left in the temporary directory (each holds a copy of FERB's Codex
        login), removed at start (`codex_stream.sweep_stale_runs`); how many, journaled."""
        import tempfile
        from ferb.runtimes.codex_stream import sweep_stale_runs
        swept = await asyncio.to_thread(sweep_stale_runs, Path(tempfile.gettempdir()), time.time())
        if swept:
            self._journal().append("runtime.stale_runs_swept", {"count": len(swept)})

    def _codex_cache_turn(self, task: Any) -> bool:
        """Whether this planner turn may hold its stable context in the developer role (P20): only the owner's chat turn
        (its `chat.message` is journaled under the request's id, outside a scheduled run's session; a scheduled or
        watch run keeps the user role, review M4), only while pasted text is data (`[policy] pasted_text_is_data`), and
        only in a conversation where no message's pasted parts were counted as the owner's own words: the developer
        role never holds pasted text unmarked (review M5)."""
        from ferb.orchestrator.continuity import SCHEDULE_SESSION
        if not bool(self.config.get("policy", "pasted_text_is_data", True)):
            return False
        journal = self._journal()
        asked = journal.of_kind("chat.message", "request_id", str(task.request_id))
        if not asked:
            return False
        session = str(asked[-1].data.get("session_id", "owner"))
        if session.startswith(SCHEDULE_SESSION):
            return False
        return self.db.conn.execute(                        # one indexed lookup, not every message of the session
            "SELECT 1 FROM journal WHERE kind = 'chat.message' AND json_extract(data_json, '$.pasted_as_owner_words') > 0 "
            "AND coalesce(json_extract(data_json, '$.session_id'), 'owner') = ? LIMIT 1", (session,)).fetchone() is None

    def _codex_cache_fallback(self, request_id: str, why: str) -> None:
        """One journal line when a cached planner turn took today's path instead: for which request, and why (P20)."""
        self._journal().append("runtime.cache_fallback", {"runtime": "codex", "role": "planner",
                                                          "request_id": request_id, "why": why})

    async def _api_key(self, provider: str) -> str | None:
        """From the Keychain at call time; added to the redactor so an echo never reaches a trace or the journal."""
        from ferb.runtimes import keys
        key = await keys.api_key(provider)
        if key:
            self._add_secret_value(key)
        return key

    def _runtime_available(self, name: str) -> bool:
        """The runtime table's availability (a model set and signed in), read at most once a minute per runtime."""
        from ferb.runtimes.catalog import CATALOG, model_for
        now = time.monotonic()
        hit = self._available_cache.get(name)
        if hit is None or now - hit[0] > 60:
            entry = CATALOG.get(name)
            hit = (now, entry is not None and entry.available(self.home, model_for(self.config, name))[0])
            self._available_cache[name] = hit
        return hit[1]

    async def _runtimes_status(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: Choose your AI's live status (and Settings → AI): each runtime's availability, each role's runtime."""
        from ferb.cli import runtimes as runtimes_cli
        out = await asyncio.to_thread(runtimes_cli.status, self.home)
        for row in out["runtimes"]:
            if row["name"] == "claude":
                row["sign_in"] = self.claude_sign_in.status()      # L34 C: a sign-in under way, or how it ended
        return out

    async def _runtimes_sign_in(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C (owner 2026-10-25: "Build a Claude sign-in like Codex's"): FERB's own Claude sign-in for FERB.app.
        `{runtime: "claude"}` starts it (or reports the one waiting); `code` passes the code the sign-in page shows to
        the waiting sign-in; `cancel: true` stops it. Journaled without the code or anything Claude Code printed."""
        if params.get("runtime") != "claude":
            raise FerbError("Only Claude signs in here; ChatGPT signs in with Sign in with ChatGPT "
                            "(`ferb setup --codex-login`)")
        if params.get("cancel") is True:
            return await self.claude_sign_in.cancel()
        if "code" in params:
            return await self.claude_sign_in.send_code(str(params["code"]))
        status = await self.claude_sign_in.start()
        self._journal().append("runtimes.sign_in", {"runtime": "claude", "state": status["state"]})
        return status

    def _claude_signed_in(self, ok: bool) -> None:
        """FERB's own Claude sign-in ended: journaled, never with the credential or the code."""
        self._available_cache.pop("claude", None)
        self._journal().append("runtimes.sign_in_finished", {"runtime": "claude", "signed_in": ok})

    async def _runtimes_use(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: the owner's choice of AI service for the roles named (all three by default), and optionally its
        model (`model`, checked against the runtime's documented models), from its next call; journaled. One that
        can't run here yet is set anyway and says so, as `ferb runtimes use` does."""
        from ferb.cli import runtimes as runtimes_cli
        from ferb.runtimes.catalog import CATALOG, ROLES
        runtime = str(params.get("runtime", ""))
        chosen = params.get("roles", list(ROLES))
        model = params.get("model")
        if not isinstance(chosen, list) or not all(isinstance(role, str) for role in chosen):
            raise FerbError("roles must be a list of role names")
        if model is not None and (not isinstance(model, str) or not model.strip()):
            raise FerbError("model must be a model name")
        try:
            assigned = await asyncio.to_thread(runtimes_cli.assign, self.home, runtime, chosen, model)
        except ValueError as e:
            raise FerbError(str(e)) from None
        self._available_cache.pop(runtime, None)
        self._journal().append("runtimes.changed", {"runtime": runtime, "roles": chosen,
                                                    **({"model": model} if model else {})})
        usable, fix = await asyncio.to_thread(CATALOG[runtime].available, self.home)
        return {"roles": assigned, "usable": usable, "fix": fix}

    async def _runtimes_check(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """One tiny typed call through a runtime: proves the key, model and network work (costs a fraction of a cent)."""
        from ferb.core.types import ModelRequest, Task
        name = str(params.get("runtime"))
        self._available_cache.pop(name, None)
        schema = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"],
                  "additionalProperties": False}
        try:
            rt = self._build_runtime(name, "orchestrator", mode="supervisor", tool_names=[], schema=schema)
            from ferb.orchestrator.worker import model_limits
            model_limits(rt)                # a model without documented limits fails here, before any call (L28)
            device = str(self.config.get("device", "id", "unknown-device"))
            task = Task("runtime-check", None, "chat", device, Identity.OWNER, set(), "mixed", "runtime-check", [], None)
            run = await rt.start(task, ModelRequest('Answer {"ok": true}.', ("Check.",), 0), "")
        except Exception as e:
            return {"ok": False, "detail": self._redact_text(f"{type(e).__name__}: {e}")}
        detail = ""
        try:
            async for ev in rt.events(run):
                if ev.kind == "final":
                    try:
                        answer = json.loads(ev.data["text"])
                    except (TypeError, ValueError):
                        answer = None
                    if not isinstance(answer, dict):
                        return {"ok": False, "detail": 'the answer was not {"ok": ...}'}
                    return {"ok": answer.get("ok") is True, "detail": "answered"}
                if ev.kind in ("error", "violation"):
                    detail = self._redact_text(str(ev.data))
        finally:
            await rt.cancel(run)
        return {"ok": False, "detail": detail or "no answer"}

    async def _prepare_codex(self) -> None:
        """Before the daemon serves, when a role runs on Codex or Codex is signed in for FERB (a worker type): the catalog
        shipped with the pinned binary is dumped (L28, final review M8: once per Codex version, in a worker thread, so
        no limits read on the event loop runs a process), then the pinned version is checked and FERB's locked model
        catalog written, once for the daemon's life instead of on every call. A failure is journaled; a run checks again
        and fails loudly."""
        from ferb.core.errors import Unsupported
        from ferb.device.install import repo_root
        from ferb.runtimes.catalog import ROLES
        from ferb.runtimes.codex_exec import bundled_catalog, codex_binary, codex_home, prepare
        if not (any(self._runtime_for(role) == "codex" for role in ROLES) or codex_home(self.home) is not None):
            return
        await asyncio.to_thread(bundled_catalog, self.home)
        try:
            await prepare(self.home, codex_binary(self.home, repo_root()))
        except (Unsupported, OSError) as e:
            self._journal().append("runtime.codex_problem", {"detail": self._redact_text(f"{type(e).__name__}: {e}")})

    def _runtime_limits(self) -> dict[str, dict[str, Any]]:
        """Each role's runtime and model with its documented context window, reply limit and per-input character cap
        (L28), or the problem."""
        from ferb.core.errors import ModelLimitsUnknown
        from ferb.runtimes.catalog import CATALOG, ROLES
        out: dict[str, dict[str, Any]] = {}
        for role in ROLES:
            name = self._runtime_for(role)
            model = self._model_for(name)
            entry: dict[str, Any] = {"runtime": name,
                                     "model": model or (CATALOG[name].default_model if name in CATALOG else "")}
            try:
                if name not in CATALOG:
                    raise ModelLimitsUnknown(f"unknown runtime {name!r}")
                limits = CATALOG[name].limits(model, self.home)
                entry.update(model=limits.model, context_window=limits.context_window, max_output=limits.max_output,
                             context_budget=limits.context_budget, max_input_chars=limits.max_input_chars,
                             source=limits.source)
            except ModelLimitsUnknown as exc:
                entry["problem"] = str(exc)
            out[role] = entry
        return out

    async def _runtime_status(self) -> dict[str, Any]:
        import importlib.metadata
        from ferb.device.install import repo_root
        from ferb.runtimes.claude_sdk import SDK_VERSION
        from ferb.runtimes.codex_exec import CODEX_VERSION, check_version, codex_binary
        status: dict[str, Any] = {
            "orchestrator": self.config.get("runtimes", "orchestrator", "codex"),
            "worker": self.config.get("runtimes", "worker", "codex")}
        try:
            binary = codex_binary(self.home, repo_root())
            await check_version(binary)
            status["codex"] = f"{CODEX_VERSION} ({binary})"
        except FerbError as e:
            status["codex"] = f"problem: {e}"
        installed = importlib.metadata.version("claude-agent-sdk")
        status["claude"] = installed if installed == SDK_VERSION else f"problem: {installed} != pinned {SDK_VERSION}"
        status["limits"] = self._runtime_limits()
        return status
