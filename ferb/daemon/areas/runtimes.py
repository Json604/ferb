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
        """The runtime a role uses: a planner-chosen worker type, or the runtime the owner chose for one task's checker
        ("checker:<runtime>", ORCH-315), else [runtimes] orchestrator / worker / checker."""
        if role.startswith(("worker:", "checker:")) and role != "worker:default":
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
                       effort: str | None = None, model: str = "") -> Any:
        """One runtime for a role (D32), through its factory in the runtime table. `effort`: the reasoning effort a
        worker-mode run asks for (L39: "high" for the checker of a step that changed files), each adapter's own
        setting; None keeps the runtime's default. The call's time limit: none for the planner's chat turn (ORCH-528)
        or a worker, CALL_LIMIT_S for another non-worker call (catalog.call_limit)."""
        from ferb.orchestrator.schemas import SUPERVISOR_SCHEMA
        from ferb.runtimes.catalog import CATALOG, Build, call_limit, role_model
        entry = CATALOG.get(name)
        if entry is None:
            if role.startswith("worker:"):          # the planner's choice of worker type, not the owner's config
                raise FerbError(f"there is no worker type {name!r}; the worker types are "
                                f"{', '.join(self._worker_types())}")
            raise FerbError(f"unknown runtime {name!r} in config.toml [runtimes]; runtimes: {', '.join(CATALOG)}")
        planner = role == "orchestrator" and mode == "supervisor" and schema is SUPERVISOR_SCHEMA
        return entry.build(Build(self.home, role, mode, tool_names, schema,  # type: ignore[arg-type, attr-defined]
                                 model or role_model(self.config, name, role), self._runtime_deps(), effort, planner,
                                 call_limit(mode, planner)))

    def _runtime_deps(self) -> Any:
        from ferb.runtimes.api import GatewayBridge
        from ferb.runtimes.catalog import RuntimeDeps
        return RuntimeDeps(config=self.config, shim_cmd=mcp_shim_command(), redact=self.redact,
                           bridge=lambda: GatewayBridge(self.gateway, self.tokens), api_key=self._api_key,
                           proxy=self._claude_proxy, on_abandoned=self.server.abandon,
                           cache_refusal=self._codex_cache_refusal, cache_skipped=self._codex_cache_skipped,
                           cache_fallback=self._codex_cache_fallback,
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

    def _codex_cache_refusal(self, task: Any) -> str:
        """Why this planner turn may not hold its stable context in the developer role (P20); "" when it may. Only the
        owner's chat turn may: its `chat.message` is journaled under the request's id, outside a scheduled run's
        session (a scheduled or watch run keeps the user role, review M4). A refused turn still resumes the planner's
        fixed thread with its whole context in the user turn (RUN-416). Pasted text no longer refuses it: the owner's
        pastes into FERB's chat count as their words (owner, 2026-11-04: "(b) pastes count as your words")."""
        from ferb.orchestrator.continuity import SCHEDULE_SESSION
        asked = self._journal().of_kind("chat.message", "request_id", str(task.request_id))
        if not asked:
            return "not an owner's chat message"
        if str(asked[-1].data.get("session_id", "owner")).startswith(SCHEDULE_SESSION):
            return "a scheduled run's turn"
        return ""

    def _codex_cache_skipped(self, request_id: str, why: str) -> None:
        """One journal line when a planner turn kept its context in the user role (RUN-416): for which request, and why."""
        self._journal().append("runtime.cache_skipped", {"runtime": "codex", "role": "planner",
                                                         "request_id": request_id, "why": why})

    def _codex_cache_fallback(self, request_id: str, why: str) -> None:
        """One journal line when a cached planner turn took today's path instead: for which request, and why (P20)."""
        self._journal().append("runtime.cache_fallback", {"runtime": "codex", "role": "planner",
                                                          "request_id": request_id, "why": why})

    async def _api_key(self, provider: str) -> str | None:
        """From the Keychain at call time; added to the redactor so an echo never reaches a trace or the journal."""
        from ferb.runtimes import keys
        problems: list[dict[str, Any]] = []
        key = await keys.api_key(provider, lambda kind, data: problems.append(data))
        self._keys_problems(problems)
        if key:
            self._add_secret_value(key)
        return key

    def _keys_problems(self, problems: list[dict[str, Any]]) -> None:
        """GATE-547: an older key item FERB couldn't delete, journaled on the loop (provider only)."""
        for problem in problems:
            self._journal().append("keys.old_item_not_deleted", problem)

    def _runtime_available(self, name: str) -> bool:
        """The runtime table's availability (a model set and signed in), as last known (RUN-424): read once (the
        daemon's start reads every runtime), then again only after an event that can change it: the owner's runtime
        or model choice (runtimes.use, a one-tap switch), a sign-in (Claude's finishing; the app's AI status read,
        runtimes.status, which reads every runtime afresh after a ChatGPT sign-in or a saved key), a runtime check,
        or a call the runtime refused for its sign-in (`_runtime_signed_out`). No per-turn sign-in or Keychain probe."""
        from ferb.runtimes.catalog import CATALOG, model_for
        model = model_for(self.config, name)
        hit = self._available_cache.get(name)
        if hit is None or hit[0] != model:              # a model changed in config is a change too
            entry = CATALOG.get(name)
            hit = (model, entry is not None and entry.available(self.home, model)[0])
            self._available_cache[name] = hit
        return hit[1]

    def _runtime_signed_out(self, name: str) -> None:
        """A call `name` ran said its sign-in failed (ORCH-315's account fact): it counts as unavailable until the next
        sign-in, runtime change, status read or start reads it again (RUN-424)."""
        from ferb.runtimes.catalog import model_for
        self._available_cache[name] = (model_for(self.config, name), False)
        self._journal().append("runtime.signed_out", {"runtime": name})

    async def _runtimes_status(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: Choose your AI's live status (and Settings → AI): each runtime's availability, each role's runtime.
        Read afresh, so it also refreshes the daemon's last known availability (RUN-424)."""
        from ferb.cli import runtimes as runtimes_cli
        out = await asyncio.to_thread(runtimes_cli.status, self.home)
        for row in out["runtimes"]:
            self._available_cache[row["name"]] = (row["model"], bool(row["usable"]))
            if row["name"] == "claude":
                row["sign_in"] = self.claude_sign_in.status()      # L34 C: a sign-in under way, or how it ended
        return out

    async def _runtimes_sign_in(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C (owner 2026-10-25: "Build a Claude sign-in like Codex's"): FERB's own Claude sign-in for FERB.app.
        `{runtime: "claude"}` starts it (or reports the one waiting); `code` passes the code the sign-in page shows to
        the waiting sign-in; `cancel: true` stops it. Journaled without the code or anything Claude Code printed."""
        if params.get("runtime") != "claude":
            raise FerbError("Only Claude signs in here; ChatGPT signs in with Settings → AI → Sign in with ChatGPT")
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

    async def _runtimes_switch_task(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """ORCH-315, the app's one tap: a task that stopped on a usage limit or a sign-in failure waits with
        FERB's offer of each other signed-in runtime; this answers it with `runtime` for that task (`scope` "task", the
        default), or first points the role that failed at it from its next call ("role", as `runtimes.use` does), then
        answers. Owner-only (auth.py); journaled. FERB never calls it itself."""
        from ferb.cli import runtimes as runtimes_cli
        task_id, runtime = str(params.get("task_id", "")), str(params.get("runtime", ""))
        scope = params.get("scope", "task")
        if scope not in ("task", "role"):
            raise FerbError('scope must be "task" or "role"')
        offer = self.supervisor.runtime_offer(task_id)
        if offer is None or runtime not in offer["choices"]:
            raise FerbError(f"task {task_id} isn't waiting to switch to {runtime}")
        if scope == "role":
            try:
                await asyncio.to_thread(runtimes_cli.assign, self.home, runtime, [offer["role"]])
            except ValueError as e:
                raise FerbError(str(e)) from None
            self._available_cache.pop(runtime, None)
            self._journal().append("runtimes.changed", {"runtime": runtime, "roles": [offer["role"]]})
        answered = self.supervisor.answer_runtime_switch(task_id, runtime)
        return {"task_id": task_id, "runtime": runtime, "scope": scope, "role": answered["role"]}

    async def _runtimes_check(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """One tiny typed call through a runtime: proves the key, model and network work (costs a fraction of a cent)."""
        return await self._check_runtime(str(params.get("runtime")), str(params.get("model") or ""))

    async def _check_runtime(self, name: str, model: str = "") -> dict[str, Any]:
        """`runtimes.check`'s call, with `model` instead of the runtime's own when given (RUN-429: a model is checked
        before it is saved)."""
        from ferb.core.types import ModelRequest, Task
        self._available_cache.pop(name, None)
        schema = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"],
                  "additionalProperties": False}
        try:
            rt = self._build_runtime(name, "orchestrator", mode="supervisor", tool_names=[], schema=schema, model=model)
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

    async def _runtimes_models(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """RUN-429 (owner 2026-11-08): the models a connection can use, for Settings → AI's picker: an API key's live
        list from its provider (read with the stored key), Codex's and Claude's from what their pinned runtime knows;
        the model it runs now and each role's own model. A list the provider didn't give says why (`problem`); the
        owner can still type any model id."""
        from ferb.runtimes import models as models_
        from ferb.runtimes.catalog import CATALOG, ROLES, model_for
        name = str(params.get("runtime", ""))
        entry = CATALOG.get(name)
        if entry is None:
            raise FerbError(f"unknown AI service {name!r}")
        out: dict[str, Any] = {"runtime": name, "model": model_for(self.config, name), "models": [],
                               "source": "provider" if entry.sign_in == "key" else "known",
                               "roles": {role: self.config.get("role_models", role) for role in ROLES
                                         if isinstance(self.config.get("role_models", role), dict)}}
        if entry.sign_in != "key":
            out["models"] = await asyncio.to_thread(models_.known_models, name, self.home)
            return out
        key = await self._api_key(name)
        if not key:
            out["problem"] = f"no {name} API key is saved"
            return out
        try:
            out["models"] = await models_.provider_models(name, key)
        except models_.ListFailed as e:
            out["problem"] = self._redact_text(str(e))
        return out

    async def _runtimes_model(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """RUN-429: the owner's model for a connection (`runtime`, `model`), or for one role on it (`role`; an empty
        model clears the role's own). Checked before it is saved: in the provider's own list for this key, else (no
        list, or a sign-in) one tiny call. A model that fails isn't saved: the previous one keeps running and the
        provider's words come back. Saved to config.toml's [models] / [role_models] through this owner RPC only."""
        from ferb.runtimes import models as models_
        from ferb.runtimes.catalog import CATALOG, ROLES
        name, model = str(params.get("runtime", "")), " ".join(str(params.get("model") or "").split())
        role = params.get("role")
        entry = CATALOG.get(name)
        if entry is None:
            raise FerbError(f"unknown AI service {name!r}")
        if role is not None and role not in ROLES:
            raise FerbError(f"role must be one of {', '.join(ROLES)}")
        if role is not None and not model:
            await asyncio.to_thread(self.config.remove, "role_models", str(role))
            self._journal().append("runtimes.model", {"runtime": name, "role": role, "model": None})
            return {"ok": True, "model": "", "detail": "uses the connection's model"}
        if not model:
            raise FerbError("model must be a model id")
        checked = ""
        if entry.sign_in == "key":
            key = await self._api_key(name)
            if key:
                try:
                    offered = await models_.provider_models(name, key)
                    if model not in offered:
                        return {"ok": False, "detail": f"{model} isn't in the models your {name} key can use"}
                    checked = "in the provider's list"
                except models_.ListFailed:
                    checked = ""
        if not checked:
            result = await self._check_runtime(name, model)
            if not result.get("ok"):
                return {"ok": False, "detail": result.get("detail") or "no answer"}
            checked = "answered"
        if role is not None:
            await asyncio.to_thread(self.config.set, "role_models", str(role), {"runtime": name, "model": model})
        else:
            await asyncio.to_thread(self.config.set, "models", name, model)
        self._available_cache.pop(name, None)
        self._journal().append("runtimes.model", {"runtime": name, "model": model, "checked": checked}
                               | ({"role": role} if role is not None else {}))
        return {"ok": True, "model": model, "detail": checked}

    async def _keys_store(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """GATE-543: FERB.app's API key for a provider (APP_ONLY), stored through FERB's signed secret helper and read
        back through it; registered with the redactor first. The key is in no reply, span or journal entry."""
        from ferb.runtimes import keys
        from ferb.runtimes.catalog import api_providers
        provider, key = str(params.get("provider", "")), params.get("key")
        if provider not in api_providers():
            raise FerbError(f"unknown provider {provider!r}")
        if not isinstance(key, str) or not key.strip() or any(c.isspace() for c in key.strip()):
            raise FerbError("that doesn't look like an API key")
        self._add_secret_value(key.strip())
        problems: list[dict[str, Any]] = []
        stored = await asyncio.to_thread(keys.store_key, provider, key.strip(), lambda k, d: problems.append(d))
        self._keys_problems(problems)
        self._available_cache.pop(provider, None)
        self._journal().append("keys.stored", {"provider": provider, "stored": stored})
        return {"stored": stored}

    async def _keys_delete(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """GATE-543: the owner's Remove key (APP_ONLY): the provider's item goes through the helper (an older one too)."""
        from ferb.runtimes import keys
        from ferb.runtimes.catalog import api_providers
        provider = str(params.get("provider", ""))
        if provider not in api_providers():
            raise FerbError(f"unknown provider {provider!r}")
        problems: list[dict[str, Any]] = []
        removed = await asyncio.to_thread(keys.delete_key, provider, lambda k, d: problems.append(d))
        self._keys_problems(problems)
        self._available_cache.pop(provider, None)
        self._journal().append("keys.deleted", {"provider": provider, "removed": removed})
        return {"removed": removed}

    async def _runtimes_check_calls(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """Settings → AI's Check (T-1069, owner 2026-11-08): the call shapes FERB really makes, through the connection's
        real adapter and model (`model` to try another): a planner call with the planner's schema and its lookup tools
        offered, a worker call that calls a tool with no arguments and one with arguments and answers in the worker's
        schema, a checker call, and a call with a small generated picture. Each is a line with pass or fail, the
        provider's words (redacted), the model, the typed form, time, tokens and cost; and the total cost. The two
        check tools exist only while it runs and touch nothing."""
        from ferb.core.types import ModelImage, ModelRequest
        from ferb.orchestrator.schemas import CHECKER_SCHEMA, SUPERVISOR_SCHEMA, WORKER_SCHEMA
        from ferb.orchestrator.supervisor import PLANNER_SCOPES, PLANNER_TOOLS
        from ferb.runtimes import check
        from ferb.runtimes.catalog import CATALOG, model_for
        name, model = str(params.get("runtime", "")), str(params.get("model") or "").strip()
        if name not in CATALOG:
            raise FerbError(f"unknown AI service {name!r}")
        self._available_cache.pop(name, None)
        ping, echo = check.CheckPing(), check.CheckEcho()
        caps = self.registry.capabilities
        caps[ping.name], caps[echo.name] = ping, echo        # type: ignore[assignment]
        lines: list[Any] = []
        try:
            lines.append(await self._check_call(check.Line("Planner call (its real schema, lookups offered)"), name,
                                                model, "orchestrator", "supervisor", SUPERVISOR_SCHEMA,
                                                sorted(PLANNER_TOOLS), set(PLANNER_SCOPES),
                                                ModelRequest(check.PROMPT_PLANNER, ("Check.",), 0)))
            worker = await self._check_call(check.Line("Worker call (tools, its typed report)"), name, model, "worker",
                                            "worker", WORKER_SCHEMA, [check.PING, check.ECHO], set(),
                                            ModelRequest(check.PROMPT_WORKER, ("Check.",), 0))
            # RUN-439: the two tool calls ran inside the worker call: its time, tokens and cost are on its line
            lines += [check.Line("Tool call with no arguments", ok=ping.calls > 0,
                                 detail="" if ping.calls else "the model didn't call check.ping",
                                 model=worker.model, typed=worker.typed, part_of="worker call"),
                      check.Line("Tool call with arguments", ok="hello" in echo.texts,
                                 detail="" if "hello" in echo.texts else "the model didn't call check.echo with hello",
                                 model=worker.model, typed=worker.typed, part_of="worker call"),
                      worker]
            lines.append(await self._check_call(check.Line("Checker call (its schema)"), name, model, "checker",
                                                "worker", CHECKER_SCHEMA, [], set(),
                                                ModelRequest(check.PROMPT_CHECKER, ("Check.",), 0)))
            lines.append(await self._check_call(check.Line("A picture in the request"), name, model, "orchestrator",
                                                "supervisor", check.IMAGE_SCHEMA, [], set(),
                                                ModelRequest(check.PROMPT_IMAGE, ("Check.",), 0,
                                                             (ModelImage(check.red_png(), "connection-check",
                                                                         "image/png"),))))
        finally:
            caps.pop(ping.name, None)
            caps.pop(echo.name, None)
        costs = [line.cost_usd for line in lines if line.cost_usd is not None]
        self._journal().append("runtimes.checked", {"runtime": name, "passed": sum(line.ok for line in lines),
                                                    "lines": len(lines)})
        return {"runtime": name, "model": model or model_for(self.config, name), "ok": all(l.ok for l in lines),
                "lines": [line.row() for line in lines], "total_cost_usd": round(sum(costs), 6) if costs else None}

    async def _check_call(self, line: Any, name: str, model: str, role: str, mode: str, schema: dict[str, Any],
                          tools: list[str], scopes: set[str], request: Any) -> Any:
        """One check call: the real runtime for `role`, offered `tools` through a run token for a live record held
        only for this call (as the planner's lookups are), its events read to the end. The line passes when the final
        answer fits `schema`; an error or a misfit says why, redacted."""
        from ferb.core.structured import check_final
        from ferb.core.types import Task
        from ferb.orchestrator.tree import TaskRecord
        from ferb.runtimes import check
        started = time.monotonic()
        task_id, holder = "check-" + self.ids.new_id(), ""
        device = str(self.config.get("device", "id", "unknown-device"))
        task = Task(task_id, None, "FERB's connection check", device, Identity.OWNER, set(scopes), "mixed", task_id,
                    [], None)
        span = self.spans.start("runtime.check", task_id=None, input={"runtime": name, "role": role,
                                                                       "check": line.name})
        try:
            rt = self._build_runtime(name, role, mode=mode, tool_names=tools, schema=schema, model=model)
            token = ""
            if tools:
                holder = "check:" + task_id
                self.tasks.hold(TaskRecord(task, self.clock.now(), span.id, task_id))
                token = self.tokens.issue(task_id, holder, set(scopes), Identity.OWNER, ttl_s=600.0,
                                          observe_only=True, tools=frozenset(tools), max_calls=10)
            runs: list[Any] = []

            async def answer() -> None:
                runs.append(await rt.start(task, request, token))
                async for ev in rt.events(runs[0]):
                    if ev.kind == "usage":
                        line.add_usage(ev.data)
                    elif ev.kind == "final":
                        try:
                            check_final(str(ev.data.get("text", "")), schema)
                            line.ok = True
                        except ValueError as e:
                            line.detail = f"the answer didn't fit its form: {e}"
                        return
                    elif ev.kind in ("error", "violation"):
                        line.detail = str(ev.data.get("message") or ev.data)
                        return
                    elif ev.kind == "retrying":           # Codex retrying after an error: why, if it never answers
                        line.extra["retrying"] = str(ev.data.get("message") or "")
            try:
                await asyncio.wait_for(answer(), check.LINE_LIMIT_S)   # RUN-439: a check line never hangs
            except TimeoutError:
                line.detail = check.no_answer(check.LINE_LIMIT_S, getattr(rt, "retried", []),
                                              line.extra.get("retrying", ""))
            finally:
                if runs:
                    await rt.cancel(runs[0])
        except Exception as e:                       # the provider's or FERB's words, never a crash of the check
            line.detail = f"{type(e).__name__}: {e}"
        finally:
            if holder:
                self.tokens.revoke_worker(holder)
                self.gateway.forget_calls(holder)
                self.tasks.remove(task_id)
            line.seconds = time.monotonic() - started
            line.detail = self._redact_text(line.detail) if line.detail else ("" if line.ok else "no answer")
            line.model = line.model or model or self._model_label(name)
            self.spans.finish(span, output=line.row())
        return line

    def _model_label(self, name: str) -> str:
        from ferb.runtimes.catalog import model_for
        return model_for(self.config, name)

    async def _prepare_codex(self) -> None:
        """Before the daemon answers any call but ping (DMN-510), when a role runs on Codex or Codex is signed in for
        FERB (a worker type): the catalog shipped with the pinned binary is dumped (L28, final review M8: once per Codex version, in a worker thread, so
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
        from ferb.runtimes.catalog import role_model
        for role in ROLES:
            name = self._runtime_for(role)
            model = role_model(self.config, name, role) or None
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
        from ferb.runtimes.claude_sdk import claude_pin_status
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
        status["claude"] = claude_pin_status(importlib.metadata.version("claude-agent-sdk"))
        status["limits"] = self._runtime_limits()
        return status
