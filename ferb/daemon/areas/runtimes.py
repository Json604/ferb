"""Own building model runtimes per role (D32) and the runtime RPCs. Must not pick a model the owner's config doesn't
name."""

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


def _provider(name: str, model: str) -> Any:
    if name == "anthropic":
        from ferb.runtimes.anthropic_api import AnthropicProvider
        return AnthropicProvider(model)
    if name == "openai":
        from ferb.runtimes.openai_api import OpenAIProvider
        return OpenAIProvider(model)
    from ferb.runtimes.google_api import GoogleProvider
    return GoogleProvider(model)


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
        model = self.config.get("models", name) or (self.config.get("runtimes", "claude_model") if name == "claude" else None)
        return str(model) if model else None

    def _make_runtime(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any],
                      effort: str | None = None) -> Any:
        return self._build_runtime(self._runtime_for(role), role, mode=mode, tool_names=tool_names, schema=schema,
                                   effort=effort)

    def _build_runtime(self, name: str, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any],
                       effort: str | None = None) -> Any:
        """One runtime for a role (D32). `effort`: the reasoning effort a worker-mode run asks for (L39: "high" for the
        checker of a step that changed files), each adapter's own setting; None keeps the runtime's default."""
        if name not in self.registry.runtimes:
            raise FerbError(f"unknown runtime {name!r} in config.toml [runtimes]")
        model = self._model_for(name)
        shim = mcp_shim_command()
        if name == "codex":
            from ferb.device.install import repo_root
            from ferb.core.errors import Unsupported
            from ferb.runtimes.codex_exec import MODEL, CodexRuntime, codex_binary, codex_home
            if codex_home(self.home) is None:
                raise Unsupported("Codex isn't signed in for FERB: choose Sign in to Codex… in FERB's menu bar, "
                                  "or run `ferb setup --codex-login`")
            workdir = self.home / "workspace" if mode == "worker" else self.home / "run" / "supervisor"
            return CodexRuntime(codex_binary(self.home, repo_root()), mode, workdir, self.home,  # type: ignore[arg-type]
                                shim_cmd=shim, tool_names=tool_names, output_schema=schema, model=model or MODEL,
                                timeout_s=None if mode == 'worker' else 900, on_abandoned=self.server.abandon,
                                redact=self.redact, cache_role=self._codex_cache_role(role, mode, schema),
                                cache_scope=str(self.config.get("device", "id", "unknown-device")),
                                cache_allowed=self._codex_cache_turn, on_cache_fallback=self._codex_cache_fallback,
                                effort=effort if mode == "worker" else None)
        if name in ("anthropic", "openai", "google"):
            from ferb.runtimes.api import ApiRuntime, GatewayBridge
            from ferb.runtimes.catalog import CATALOG
            provider = _provider(name, model or CATALOG[name].default_model)
            return ApiRuntime(provider, mode=mode, role=role, tool_names=tool_names,  # type: ignore[arg-type]
                              output_schema=schema, bridge=GatewayBridge(self.gateway, self.tokens), key=self._api_key,
                              effort=effort if mode == "worker" else None)
        from ferb.runtimes.claude_sdk import ClaudeRuntime
        if self._proxy is None:
            self._proxy = ToolListProxy()
        return ClaudeRuntime(mode, self._proxy, str(self.home), shim_cmd=shim, tool_names=tool_names,  # type: ignore[arg-type]
                             output_schema=schema, model=model, effort=effort if mode == "worker" else None)

    def _codex_cache_role(self, role: str, mode: str, schema: dict[str, Any]) -> str | None:
        """P20 (owner, 2026-10-25): "planner" for the planner's chat turn (the orchestrator's SUPERVISOR_SCHEMA call) when
        `[runtimes] codex_planner_cache = true`, so its stable context goes in Codex's developer role under a fixed cache
        key; None otherwise, today's path. Off by default until the owner passes the live injection eval
        (tests/security/test_planner_injection_real.py); only the boolean true turns it on."""
        from ferb.orchestrator.schemas import SUPERVISOR_SCHEMA
        planner = role == "orchestrator" and mode == "supervisor" and schema is SUPERVISOR_SCHEMA
        return "planner" if planner and self.config.get("runtimes", "codex_planner_cache", False) is True else None

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
        from ferb.runtimes.catalog import CATALOG
        now = time.monotonic()
        hit = self._available_cache.get(name)
        if hit is None or now - hit[0] > 60:
            hit = (now, name in CATALOG and CATALOG[name].available(self.home)[0])
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
        """L34 C: the owner's choice of AI service for the roles named (all three by default), from its next call;
        journaled. One that can't run here yet is set anyway and says so, as `ferb runtimes use` does."""
        from ferb.cli import runtimes as runtimes_cli
        from ferb.runtimes.catalog import CATALOG, ROLES
        runtime = str(params.get("runtime", ""))
        chosen = params.get("roles", list(ROLES))
        if not isinstance(chosen, list) or not all(isinstance(role, str) for role in chosen):
            raise FerbError("roles must be a list of role names")
        try:
            assigned = runtimes_cli.assign(self.home, runtime, chosen)
        except ValueError as e:
            raise FerbError(str(e)) from None
        self._available_cache.pop(runtime, None)
        self._journal().append("runtimes.changed", {"runtime": runtime, "roles": chosen})
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

    async def _dump_codex_catalog(self) -> None:
        """L28, final review M8: when a role runs on Codex, the catalog shipped with the pinned binary is dumped (once
        per Codex version; `codex debug models --bundled` is a process) in a worker thread before the daemon serves,
        so no limits read on the event loop ever runs it. A failure leaves the window unknown, which is said."""
        from ferb.runtimes.catalog import ROLES
        from ferb.runtimes.codex_exec import bundled_catalog
        if any(self._runtime_for(role) == "codex" for role in ROLES):
            await asyncio.to_thread(bundled_catalog, self.home)

    def _runtime_limits(self) -> dict[str, dict[str, Any]]:
        """Each role's runtime and model with its documented context window, reply limit and per-input character cap
        (L28), or the problem."""
        from ferb.core.errors import ModelLimitsUnknown
        from ferb.runtimes.catalog import CATALOG, ROLES
        out: dict[str, dict[str, Any]] = {}
        for role in ROLES:
            name = self._runtime_for(role)
            model = self._model_for(name)
            entry: dict[str, Any] = {"runtime": name, "model": model or (CATALOG[name].default_model if name in CATALOG else "")}
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
        status["limits"] = self._runtime_limits()
        return status
