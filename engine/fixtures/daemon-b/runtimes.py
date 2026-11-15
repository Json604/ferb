"""Records ferb/daemon/areas/runtimes.py (with ferb/runtimes/keys.py, check.py, claude_login.py's sign-in and
ferb/cli/runtimes.py's status/assign) past the signed-out paths rust/fixtures/daemon's `runtimes` scenario reaches:
signed in, a key stored, a model and an effort saved, a role assigned, a sign-in started, given a code, cancelled and
failed, checks passing and failing, a switch_task answered.

Run from the repo root:  uv run python rust/fixtures/daemon-b/runtimes.py      (writes runtimes.json beside it)

What is real: the daemon, its handlers, the runtime table's factories (CATALOG[...].build: the adapters are built for
real), keys.py's logic, the Claude sign-in's process handling. What is scripted, and recorded so a Rust test can play the
same: the adapters' start/events/cancel (SCRIPT, one entry per call FERB starts), the Keychain (tests/test_api_keys.py
FakeKeychain behind keys.helper and `security`), a provider's model list (LISTS), the supervisor's switch offers
(OFFERS), the claude program (a /bin/sh stand-in, CLAUDE_SH). Every seam call FERB made is in each scenario's `log`
(builds, starts, cancels, tokens, task holds, spans, gateway forgets, model lists, offers), task ids as `check-#<n>`.
"""

import dataclasses
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402

H = common.load("runtimes")

from common import BScenario, run  # noqa: E402
from ferb.core.errors import Unsupported  # noqa: E402
from ferb.core.types import RunHandle, RuntimeEvent  # noqa: E402
import ferb.runtimes.catalog as catalog  # noqa: E402
import ferb.runtimes.claude_login as claude_login  # noqa: E402
import ferb.runtimes.keys as keys  # noqa: E402
import ferb.runtimes.models as models_  # noqa: E402
from ferb.runtimes.api import ApiRuntime  # noqa: E402
from ferb.runtimes.claude_sdk import ClaudeRuntime  # noqa: E402
from ferb.runtimes.codex_exec import CodexRuntime  # noqa: E402
from tests.test_api_keys import FakeKeychain  # noqa: E402

MP = H.MP
ENTRY = ["ferb/daemon/areas/runtimes.py", "ferb/runtimes/keys.py", "ferb/runtimes/check.py",
         "ferb/runtimes/claude_login.py", "ferb/cli/runtimes.py"]


def sha(data: bytes | str) -> str:
    return hashlib.sha256(data.encode() if isinstance(data, str) else data).hexdigest()


# ---------------------------------------------------------------------------------------------------------------
# The scripted adapters: every runtime FERB builds really is built (its factory runs); only start/events/cancel are
# scripted. Each start takes the next SCRIPT entry: {"raise": [class, words]} at start, or {"tools": [[name, args]],
# "events": [[kind, data]], "raise_in_events": [class, words]} for its events.
# ---------------------------------------------------------------------------------------------------------------

SCRIPT: list[dict[str, Any]] = []
LOG: list[list[Any]] = []
CUR: dict[str, Any] = {}
EXC = {"Unsupported": Unsupported, "ConnectionError": ConnectionError}


async def _start(self: Any, task: Any, request: Any, run_token: str) -> Any:
    entry = SCRIPT.pop(0)
    LOG.append(["start", {"task_id": task.id, "parent_id": task.parent_id, "goal": task.goal,
                          "device_id": task.device_id, "identity": task.identity.value, "scopes": sorted(task.scopes),
                          "role": task.role, "request_id": task.request_id, "system": request.system,
                          "parts": list(request.parts), "cacheable": request.cacheable,
                          "images": [{"sha256": sha(i.data), "png_hex": i.data.hex(), "reference": i.ref,
                                      "mime": i.mime} for i in request.images],
                          "token": bool(run_token)}])
    if "raise" in entry:
        cls, words = entry["raise"]
        raise EXC[cls](words)
    return RunHandle("run-" + task.id, "scripted", task.id, {"entry": entry})


async def _events(self: Any, run_: Any) -> Any:
    entry = run_.state["entry"]
    for name, args in entry.get("tools", []):
        cap = CUR["d"].registry.capabilities[name]
        await cap.run(cap.Input(**args), None)
    for kind, data in entry.get("events", []):
        yield RuntimeEvent(kind, dict(data))
    if "raise_in_events" in entry:
        cls, words = entry["raise_in_events"]
        raise EXC[cls](words)


async def _cancel(self: Any, run_: Any) -> None:
    LOG.append(["cancel"])


for _cls in (CodexRuntime, ClaudeRuntime, ApiRuntime):
    MP.setattr(_cls, "start", _start)
    MP.setattr(_cls, "events", _events)
    MP.setattr(_cls, "cancel", _cancel)

for _name, _entry in list(catalog.CATALOG.items()):
    def _build(b: Any, real: Any = _entry.build, name: str = _name) -> Any:
        rt = real(b)
        LOG.append(["build", {"name": name, "role": b.role, "mode": b.mode, "tools": list(b.tool_names),
                              "schema_sha256": sha(json.dumps(b.schema)), "model": b.model, "effort": b.effort,
                              "planner": b.planner, "limit_s": b.limit_s}
                    | ({"cache_role": rt.cache_role, "cache_scope": rt.cache_scope} if name == "codex" else {})])
        return rt
    MP.setitem(catalog.CATALOG, _name, dataclasses.replace(_entry, build=_build))

# a provider's model list (models.provider_models, which reaches the provider over HTTP)
LISTS: dict[str, Any] = {}


async def _provider_models(provider: str, key: str) -> list[str]:
    LOG.append(["models.list", {"provider": provider, "key": key}])
    found = LISTS.get(provider)
    if isinstance(found, str):
        raise models_.ListFailed(found)
    return list(found or [])


MP.setattr(models_, "provider_models", _provider_models)

# the stand-in claude program: prints the page, takes a code on stdin, "saves" a login only for the right code
CLAUDE_SH = """#!/bin/sh
saved="$CLAUDE_CONFIG_DIR/invented-login"
if [ "$1 $2" = "auth status" ]; then
  if [ -f "$saved" ]; then
    echo '{"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty"}'
  else
    echo '{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}'
  fi
  exit 0
fi
if [ "$1 $2 $3" = "auth login --claudeai" ]; then
  if [ -n "$ANTHROPIC_API_KEY" ] || [ -n "$CLAUDE_CODE_OAUTH_TOKEN" ]; then
    echo "a key from the environment would take over the login"
    exit 3
  fi
  echo "Opening browser to sign in... If the browser didn't open, visit: https://claude.example.test/oauth?state=invented."
  printf 'Paste code here if prompted > '
  read code
  if [ "$code" != "invented-code-42" ]; then
    echo "Invalid code."
    exit 1
  fi
  echo "saved" > "$saved"
  echo "Login successful. Credential invented-credential-words"
  exit 0
fi
exit 2
"""

CODEX_CATALOG = {"models": [
    {"slug": "gpt-6.1-sol", "context_window": 400000, "effective_context_window_percent": 95,
     "default_reasoning_level": "medium",
     "supported_reasoning_levels": [{"effort": "low"}, {"effort": "medium"}, {"effort": "high"}, {"effort": "ultra"}]},
    {"slug": "gpt-5.6-sol", "context_window": 272000, "effective_context_window_percent": 95,
     "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}]},
    {"slug": "gpt-invented-mini", "context_window": 128000, "effective_context_window_percent": 90}]}

PLANNER_OK = json.dumps({"kind": "reply", "answers_question": False, "task": None, "control": None,
                         "side_entries": None, "text": "ok"})
WORKER_OK = json.dumps({"status": "done", "reason": "", "question": None, "answer": "ok", "artifacts": [], "how": ""})
CHECKER_OK = json.dumps({"results": [{"index": 0, "met": True, "applies": True, "evidence": "the answer says ok", "how": None}],
                         "next": "accept", "findings": [], "missing": "", "instructions": "", "worker_type": None,
                         "question": "", "reason": ""})


def usage(model: str, typed: str, i: int, o: int, cost: float | None) -> list[Any]:
    return ["usage", {"model": model, "typed": typed, "input_tokens": i, "output_tokens": o, "cost_usd": cost}]


def final(text: str) -> list[Any]:
    return ["final", {"text": text}]


# ---------------------------------------------------------------------------------------------------------------
# A scenario: its daemon with the seams wrapped (every call logged), the files and Keychain it starts from
# ---------------------------------------------------------------------------------------------------------------

def scenario(name: str, what: str, *, files: dict[str, str] | None = None, keychain: dict[str, Any] | None = None,
             lists: dict[str, Any] | None = None, offers: dict[str, Any] | None = None,
             script: list[dict[str, Any]] | None = None, claude: bool = False) -> BScenario:
    m = H.Model()
    m.real_build = True
    files = dict(files or {})

    def before(d: Any) -> None:
        for rel, text in files.items():
            path = d.home / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        d.config.set("device", "id", "invented-device")
    keys._cache.clear()
    fake = FakeKeychain()
    fake.helper_items.update((keychain or {}).get("helper", {}))
    fake.security_items.update((keychain or {}).get("security", {}))
    fake.delete_fails = bool((keychain or {}).get("delete_fails"))
    MP.setattr(keys, "helper", fake.helper)
    MP.setattr(keys, "_security", lambda *a: fake.security(["security", *a]))
    LISTS.clear()
    LISTS.update(lists or {})
    SCRIPT[:] = list(script or [])
    LOG.clear()
    s = BScenario(name, what, ENTRY, model=m, before_start=[before],
                  setup={"files": files, "keychain": keychain or {}, "lists": lists or {}, "offers": offers or {},
                         "script": list(script or []), "claude_sh": CLAUDE_SH if claude else None})
    d = s.d
    CUR["d"], CUR["keychain"], CUR["offers"] = d, fake, dict(offers or {})
    s.setup["config"] = (d.home / "config.toml").read_text()
    del d._runtime_available                       # conftest's stand-in: the real one (a model set and signed in)
    wrap(d)
    if claude:
        cli = s.s.root / "bin" / "claude"
        cli.parent.mkdir(parents=True, exist_ok=True)
        cli.write_text(CLAUDE_SH)
        cli.chmod(0o755)
        d.claude_sign_in.cli = lambda: cli
    return s


def wrap(d: Any) -> None:
    tokens, gateway, tasks, spans, sup = d.tokens, d.gateway, d.tasks, d.spans, d.supervisor
    issue, revoke, forget, hold, remove = tokens.issue, tokens.revoke_worker, gateway.forget_calls, tasks.hold, \
        tasks.remove
    start, finish = spans.start, spans.finish
    mine: set[str] = set()

    def issue_w(task_id: str, holder: str, scopes: Any, identity: Any, **kw: Any) -> Any:
        LOG.append(["token.issue", {"task_id": task_id, "holder": holder, "scopes": sorted(scopes),
                                    "identity": identity.value, "ttl_s": kw.get("ttl_s"),
                                    "observe_only": kw.get("observe_only"), "tools": sorted(kw.get("tools") or []),
                                    "max_calls": kw.get("max_calls")}])
        return issue(task_id, holder, scopes, identity, **kw)

    def revoke_w(holder: str) -> Any:
        LOG.append(["token.revoke", holder])
        return revoke(holder)

    def forget_w(holder: str) -> Any:
        LOG.append(["gateway.forget", holder])
        return forget(holder)

    def hold_w(record: Any) -> Any:
        t = record.task
        LOG.append(["tasks.hold", {"task_id": t.id, "goal": t.goal, "scopes": sorted(t.scopes),
                                   "at": record.started_at.isoformat() if hasattr(record, "started_at") else None}])
        return hold(record)

    def remove_w(task_id: str) -> Any:
        LOG.append(["tasks.remove", task_id])
        return remove(task_id)

    def start_w(kind: str, *a: Any, **kw: Any) -> Any:
        span = start(kind, *a, **kw)
        if kind == "runtime.check":
            mine.add(span.id)
            LOG.append(["span.start", {"kind": kind, "task_id": kw.get("task_id"), "input": kw.get("input")}])
        return span

    def finish_w(span: Any, *a: Any, **kw: Any) -> Any:
        if span.id in mine:
            LOG.append(["span.finish", {"output": kw.get("output")}])
        return finish(span, *a, **kw)

    def offer(task_id: str) -> Any:
        LOG.append(["offer", task_id])
        return CUR["offers"].get(task_id)

    def answer(task_id: str, runtime: str) -> Any:
        LOG.append(["answer", task_id, runtime])
        return {"task_id": task_id, "runtime": runtime, "role": CUR["offers"][task_id]["role"]}
    MP.setattr(tokens, "issue", issue_w)
    MP.setattr(tokens, "revoke_worker", revoke_w)
    MP.setattr(gateway, "forget_calls", forget_w)
    MP.setattr(tasks, "hold", hold_w)
    MP.setattr(tasks, "remove", remove_w)
    MP.setattr(spans, "start", start_w)
    MP.setattr(spans, "finish", finish_w)
    MP.setattr(sup, "runtime_offer", offer)
    MP.setattr(sup, "answer_runtime_switch", answer)


def phase(s: BScenario, *calls: tuple[str, Any]) -> None:
    """One connection for a run of calls: a Rust test replays each phase whole, doing the steps between phases."""
    c = s.conn("app")
    s.calls(c, *calls)
    s.s.close(c)


def act(s: BScenario, what: str, *args: Any) -> None:
    """Something the daemon's other parts do between calls (a worker reporting an account fact), as a step."""
    d = s.d

    def go() -> Any:
        if what == "runtime_account":
            return d._runtime_account(*args)
        if what == "forget_refusals":
            d._refusal_facts = None
            return None
        raise AssertionError(what)
    H.on_loop(s.host, go)
    H.settle(s.host)
    s.s.steps.append({"do": "act", "what": what, "args": list(args)})


def wait_sign_in(s: BScenario, state: str, url: bool) -> None:
    d = s.d
    s.s.wait(f"the Claude sign-in is {state}" + (" with its page" if url else ""),
             lambda: H.on_loop(s.host, lambda: d.claude_sign_in.state == state and bool(d.claude_sign_in.url) == url))
    H.settle(s.host)
    s.s.steps.append({"do": "wait_sign_in", "state": state, "url": url})


def finish(s: BScenario) -> dict[str, Any]:
    d = s.d
    config_after = (d.home / "config.toml").read_text()
    fake = CUR["keychain"]
    keychain_after = {"helper": dict(sorted(fake.helper_items.items())),
                      "security": dict(sorted(fake.security_items.items())), "reads": list(fake.reads)}
    signed = (d.home / "claude-home" / claude_login.SIGNED_IN).is_file()
    log = json.loads(json.dumps(LOG, default=str))
    out = s.finish()
    seen: dict[str, str] = {}

    def norm(m: re.Match[str]) -> str:
        return seen.setdefault(m.group(0), f"check-#{len(seen) + 1}")
    text = re.sub(r"check-(?:id|r\d+)-\d{6}", norm, json.dumps(log))
    out["log"] = H.intern(json.loads(H.fx(text)))
    out["after"] = H.intern(json.loads(H.fx(json.dumps({"config": config_after, "keychain": keychain_after,
                                                         "claude_signed_in": signed}))))
    assert not SCRIPT, f"{s.s.name}: unused script entries {SCRIPT}"
    return out


# ---------------------------------------------------------------------------------------------------------------
# Scenarios
# ---------------------------------------------------------------------------------------------------------------

def sc_status_and_choice() -> dict[str, Any]:
    s = scenario("status_and_choice", "Settings → AI read with Claude and Codex signed in, an effort and a role's own "
                 "model set; choosing a runtime per role with and without a model; a usage limit Claude reported shown "
                 "until a later call answers, read again from the journal as after a restart.",
                 files={"claude-home/.ferb-signed-in": "invented marker\n", "codex-home/auth.json": "{}\n",
                        "codex-home/models_cache.json": json.dumps(CODEX_CATALOG)})
    H.on_loop(s.host, lambda: (s.d.config.set("efforts", "codex", "medium"),
                               s.d.config.set("role_models", "worker", {"runtime": "claude",
                                                                        "model": "claude-sonnet-5-5",
                                                                        "effort": "high"})))
    s.setup["config"] = (s.d.home / "config.toml").read_text()
    phase(s, ("runtimes.status", {}))
    act(s, "runtime_account", "claude", "usage_limit")
    act(s, "runtime_account", "codex", "signed_out")
    phase(s, ("runtimes.status", {}),
          ("runtimes.use", {"runtime": "claude", "roles": ["orchestrator"], "model": "claude-sonnet-5-5"}),
          ("runtimes.use", {"runtime": "claude", "model": ""}), ("runtimes.use", {"runtime": "claude", "model": 5}),
          ("runtimes.use", {"runtime": "claude", "roles": []}),
          ("runtimes.use", {"runtime": "claude", "roles": ["nope", "worker"]}),
          ("runtimes.use", {"runtime": "claude", "roles": [1]}),
          ("runtimes.use", {"runtime": "codex", "roles": ["checker"], "model": "gpt-unknown-1"}),
          ("runtimes.use", {"runtime": "codex", "roles": ["checker"], "model": "gpt-invented-mini"}),
          ("runtimes.use", {"runtime": "openai", "roles": ["worker"]}),
          ("runtimes.models", {"runtime": "codex"}), ("runtimes.models", {"runtime": "claude"}),
          ("runtimes.models", {"runtime": "google"}))
    act(s, "forget_refusals")
    phase(s, ("runtimes.status", {}))
    act(s, "runtime_account", "claude", "")
    act(s, "runtime_account", "claude", "")
    act(s, "runtime_account", "codex", "usage_limit")
    phase(s, ("runtimes.status", {}), ("runtimes.use", {"runtime": "claude"}), ("runtimes.status", {}))
    return finish(s)


def sc_keys() -> dict[str, Any]:
    s = scenario("keys", "API keys through the helper (a fake Keychain): stored and read back, an older `security` "
                 "item moved and one that won't delete, removed; the models a key can use from the provider's list "
                 "(answered, failed with words naming the key, no key).",
                 keychain={"security": {"api/openai": "invented-older-openai-words"}},
                 lists={"anthropic": ["claude-sonnet-5-5", "claude-opus-5-5", "claude-invented-7"],
                        "openai": "HTTP 401: the key invented-openai-words was refused"})
    phase(s, ("keys.store", {"provider": "anthropic", "key": "  invented-anthropic-words  "}),
          ("keys.store", {"provider": "anthropic", "key": "two words"}),
          ("keys.store", {"provider": "anthropic", "key": 5}), ("keys.store", {"provider": "anthropic"}),
          ("keys.store", {"provider": "anthropic", "key": "   "}),
          ("runtimes.status", {}), ("runtimes.models", {"runtime": "anthropic"}),
          ("runtimes.models", {"runtime": "openai"}),
          ("keys.store", {"provider": "openai", "key": "invented-openai-words"}),
          ("runtimes.models", {"runtime": "openai"}), ("runtimes.models", {"runtime": "google"}),
          ("keys.delete", {"provider": "anthropic"}), ("runtimes.models", {"runtime": "anthropic"}),
          ("runtimes.status", {}), ("keys.delete", {}))
    CUR["keychain"].delete_fails = True
    CUR["keychain"].security_items["api/google"] = "invented-older-google-words"
    s.s.steps.append({"do": "keychain", "delete_fails": True,
                      "security": {"api/google": "invented-older-google-words"}})
    phase(s, ("keys.store", {"provider": "google", "key": "invented-google-words"}),
          ("keys.delete", {"provider": "google"}), ("keys.delete", {"provider": "openai"}))
    return finish(s)


def sc_checks() -> dict[str, Any]:
    script = [
        {"events": [usage("claude-opus-5-5", "json_schema", 12, 3, 0.0004), final('{"ok": true}')]},
        {"events": [final('{"ok": false}')]},
        {"events": [final("not json")]},
        {"events": [["error", {"message": "invented provider words", "code": 7}]]},
        {"raise": ["Unsupported", "invented start refusal"]},
        {"events": []},
        {"events": [["violation", {"tool": "invented"}], final("[1]")]},
        {"raise_in_events": ["ConnectionError", "invented network drop"]},
        {"events": [final('{"ok": true}')]},
        {"events": [final('{"ok": true}')]},
        # check_calls claude: every line passes
        {"events": [usage("claude-opus-5-5", "json_schema", 100, 20, 0.0015), final(PLANNER_OK)]},
        {"tools": [["check.ping", {}], ["check.echo", {"text": "hello"}]],
         "events": [usage("claude-opus-5-5", "json_schema", 120, 30, 0.002), usage("", "", 5, 1, 0.0001),
                    final(WORKER_OK)]},
        {"events": [usage("claude-opus-5-5", "stated", 90, 25, None), final(CHECKER_OK)]},
        {"events": [usage("claude-opus-5-5", "json_schema", 300, 4, 0.0031), final('{"colour": "red"}')]},
        # check_calls claude with another model: every line fails a different way
        {"events": [["error", {"message": "invented overload"}]]},
        {"tools": [["check.echo", {"text": "not hello"}]],
         "events": [["retrying", {"message": "invented retry"}], final('{"status": "done"}')]},
        {"raise": ["Unsupported", "invented start refusal"]},
        {"raise_in_events": ["ConnectionError", "invented network drop"]},
        # check_calls codex: the planner answers, the rest give no answer
        {"events": [usage("gpt-6.1-sol", "json_schema", 50, 5, None), final(PLANNER_OK)]},
        {"events": [["error", {"code": 3}]]},
        {"events": []},
        {"events": [final("")]},
    ]
    s = scenario("checks", "runtimes.check and runtimes.check_calls through the real factories with scripted "
                 "adapters: an answer, a refusal, words that aren't JSON, an error event, a refused start, no answer, "
                 "a stream that raises (Python answers an internal error), a model without documented limits; every "
                 "check line passing, then each failing differently.",
                 files={"claude-home/.ferb-signed-in": "invented marker\n", "codex-home/auth.json": "{}\n",
                        "codex-home/models_cache.json": json.dumps(CODEX_CATALOG)},
                 keychain={"helper": {"api.sealed/anthropic": "invented-anthropic-words"}}, script=script)
    act(s, "runtime_account", "claude", "signed_out")
    phase(s, ("runtimes.check", {"runtime": "claude"}),
          ("runtimes.check", {"runtime": "claude", "model": "claude-sonnet-5-5"}),
          ("runtimes.check", {"runtime": "claude"}), ("runtimes.check", {"runtime": "claude"}),
          ("runtimes.check", {"runtime": "claude"}), ("runtimes.check", {"runtime": "claude"}),
          ("runtimes.check", {"runtime": "claude"}))
    phase(s, ("runtimes.check", {"runtime": "claude"}))
    phase(s, ("runtimes.check", {"runtime": "codex"}),
          ("runtimes.check", {"runtime": "codex", "model": "gpt-unknown-1"}),
          ("runtimes.check", {"runtime": "anthropic"}), ("runtimes.check", {}),
          ("runtimes.check_calls", {"runtime": "claude"}),
          ("runtimes.check_calls", {"runtime": "claude", "model": " claude-sonnet-5-5 "}),
          ("runtimes.check_calls", {"runtime": "codex", "model": "gpt-6.1-sol"}),
          ("runtimes.check_calls", {}))
    return finish(s)


def sc_model_choice() -> dict[str, Any]:
    script = [
        {"events": [final('{"ok": true}')]},           # claude-sonnet-5-5 at high
        {"events": [final('{"ok": false}')]},          # turbo refused
        {"events": [final('{"ok": true}')]},           # worker's own effort, the connection's model
        {"events": [final('{"ok": true}')]},           # orchestrator's own model
        {"events": [["error", {"message": "invented: model not found"}]]},   # openai: no list, the call refuses
        {"events": [final('{"ok": true}')]},           # openai: no list, the call answers
        {"events": [final('{"ok": true}')]},           # a spaced model id
        {"events": [final('{"ok": true}')]},           # an effort given as a number: str(7)
    ]
    s = scenario("model_choice", "runtimes.model: a model and an effort checked before they are saved (in the "
                 "provider's list, else one tiny call), refused ones kept out, a role's own model and effort set and "
                 "cleared, the shape checks.",
                 files={"claude-home/.ferb-signed-in": "invented marker\n"},
                 keychain={"helper": {"api.sealed/anthropic": "invented-anthropic-words",
                                      "api.sealed/openai": "invented-openai-words"}},
                 lists={"anthropic": ["claude-opus-5-5", "claude-sonnet-5-5"],
                        "openai": "HTTP 503: invented overload"}, script=script)
    phase(s, ("runtimes.model", {"runtime": "claude", "model": "claude-sonnet-5-5", "effort": "high"}),
          ("runtimes.model", {"runtime": "claude", "model": "claude-sonnet-5-5", "effort": "turbo"}),
          ("runtimes.model", {"runtime": "claude", "model": "claude-sonnet-5-5", "effort": "very high!"}),
          ("runtimes.model", {"runtime": "codex", "model": "gpt-5.6-sol", "effort": "ultra"}),
          ("runtimes.model", {"runtime": "claude", "model": "", "role": "worker", "effort": "medium"}),
          ("runtimes.model", {"runtime": "claude", "role": "worker", "effort": ""}),
          ("runtimes.model", {"runtime": "claude", "role": "nope", "model": "x"}),
          ("runtimes.model", {"runtime": "claude", "role": "orchestrator", "model": "claude-opus-5-5"}),
          ("runtimes.models", {"runtime": "claude"}),
          ("runtimes.model", {"runtime": "anthropic", "model": "claude-opus-5-5"}),
          ("runtimes.model", {"runtime": "anthropic", "model": "claude-opus-9"}),
          ("runtimes.model", {"runtime": "openai", "model": "gpt-invented-x"}),
          ("runtimes.model", {"runtime": "openai", "model": "gpt-invented-y", "effort": "high"}),
          ("runtimes.model", {"runtime": "claude", "model": "  claude-opus-5-5   extra "}),
          ("runtimes.model", {"runtime": "claude", "model": "claude-opus-5-5", "effort": 7}),
          ("runtimes.models", {"runtime": "anthropic"}), ("runtimes.status", {}))
    return finish(s)


def sc_switch_task() -> dict[str, Any]:
    s = scenario("switch_task", "The app's one tap: a task waiting with FERB's offer of other runtimes, switched for "
                 "the task, or its role pointed at the runtime first; refusals.",
                 offers={"task-a": {"role": "worker", "choices": ["claude", "anthropic"]},
                         "task-b": {"role": "orchestrator", "choices": ["claude"]}})
    phase(s, ("runtimes.switch_task", {"task_id": "task-a", "runtime": "claude"}),
          ("runtimes.switch_task", {"task_id": "task-a", "runtime": "openai"}),
          ("runtimes.switch_task", {"task_id": "task-b", "runtime": "claude", "scope": "role"}),
          ("runtimes.switch_task", {"task_id": "task-a", "runtime": "anthropic", "scope": "task"}),
          ("runtimes.switch_task", {"runtime": "claude"}),
          ("runtimes.switch_task", {"task_id": "task-a", "runtime": "claude", "scope": ["role"]}),
          ("runtimes.status", {}))
    return finish(s)


def sc_sign_in() -> dict[str, Any]:
    s = scenario("sign_in", "FERB's own Claude sign-in with a stand-in claude program: started, its page read, a "
                 "pasted code refused for its shape then sent, finished; started again and cancelled; started again "
                 "and failed (a wrong code).", claude=True)
    phase(s, ("runtimes.sign_in", {"runtime": "claude"}))
    wait_sign_in(s, "waiting", True)
    phase(s, ("runtimes.status", {}), ("runtimes.sign_in", {"runtime": "claude"}),
          ("runtimes.sign_in", {"runtime": "claude", "code": "two words"}),
          ("runtimes.sign_in", {"runtime": "claude", "code": "  "}),
          ("runtimes.sign_in", {"runtime": "claude", "code": " invented-code-42 "}))
    wait_sign_in(s, "signed_in", False)
    phase(s, ("runtimes.status", {}), ("runtimes.sign_in", {"runtime": "claude", "code": "invented-code-42"}),
          ("runtimes.sign_in", {"runtime": "claude"}))
    wait_sign_in(s, "waiting", True)
    phase(s, ("runtimes.sign_in", {"runtime": "claude", "cancel": True}), ("runtimes.status", {}),
          ("runtimes.sign_in", {"runtime": "claude"}))
    wait_sign_in(s, "waiting", True)
    phase(s, ("runtimes.sign_in", {"runtime": "claude", "code": "wrong-code"}))
    wait_sign_in(s, "failed", False)
    phase(s, ("runtimes.status", {}), ("runtimes.sign_in", {"runtime": "claude", "cancel": "yes", "code": 42}))
    wait_sign_in(s, "failed", False)
    return finish(s)


if __name__ == "__main__":
    run("runtimes", "runtimes.json", [sc_status_and_choice, sc_keys, sc_checks, sc_model_choice, sc_switch_task,
                                      sc_sign_in])
