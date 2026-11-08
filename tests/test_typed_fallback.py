"""Adapter contract for typed answers a provider won't take as a typed format (RUN-427): every schema FERB sends fits
Anthropic's counted limits after reshaping; a request without the format is still a valid request with the answer form
in the cached instructions; a refusal (the 2026-11-08 "compiled grammar is too large" 400) falls back once, is
remembered for later calls, and an answer outside the form is told back once. Offline: scripted HTTP and a fake Claude
Code client."""
import json
from types import SimpleNamespace
from typing import Any

import pytest

from ferb.core.structured import check_final
from ferb.core.types import Identity, ModelRequest, Task
from ferb.daemon.areas.intro import SUGGEST_SCHEMA
from ferb.memory import import_extract, mac_profile, playbooks, profile
from ferb.memory.distill import DISTILL_SCHEMA
from ferb.orchestrator import schemas as core
from ferb.runtimes.anthropic_api import AnthropicProvider
from ferb.runtimes.api import ApiRuntime
from ferb.runtimes.google_api import GoogleProvider
from ferb.runtimes.openai_api import OpenAIProvider
from ferb.runtimes.schema_shape import (ANTHROPIC_MAX_OPTIONAL, ANTHROPIC_MAX_UNIONS, STATED_FOR_S, Refusals,
                                        SchemaTooComplex, complexity, for_anthropic, for_gemini)
from tests.support.claude_home import signed_in_home
from tests.support.runtime_fakes import Scripted
from tests.test_schema_shape import sample

from ferb.runtimes.claude_sdk import ClaudeRuntime as _Claude  # noqa: E402
CLAUDE_START = _Claude.start             # saved before the offline fixture replaces it (module code runs first)

TASK = Task("t.typed", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
REQ = ModelRequest("FERB instructions", ("stable facts", "the message"), 1)
# Every typed answer FERB asks a model for: the roles' schemas, the memory passes, the import and the intro
SENT = ({name: getattr(core, name) for name in dir(core) if name.endswith("_SCHEMA")}
        | {"DISTILL_SCHEMA": DISTILL_SCHEMA, "MAC_SCHEMA": mac_profile.MAC_SCHEMA, "PLAYBOOK_SCHEMA": playbooks.SCHEMA,
           "PROFILE_SCHEMA": profile.PROFILE_SCHEMA, "RECALL_SCHEMA": import_extract.RECALL_SCHEMA,
           "VERIFY_SCHEMA": import_extract.VERIFY_SCHEMA, "SUGGEST_SCHEMA": SUGGEST_SCHEMA})
GRAMMAR = {"type": "error", "error": {"type": "invalid_request_error", "message": (
    "The compiled grammar is too large, which would cause performance issues. Simplify your tool schemas or reduce "
    "the number of strict tools.")}}          # the owner's planner turns, 2026-11-08 (journal chat.failed)
PROVIDERS = {"anthropic": lambda: AnthropicProvider("claude-sonnet-5-5"),
             "openai": lambda: OpenAIProvider("gpt-6-astra"), "google": lambda: GoogleProvider("gemini-3.8-flash")}


def anthropic_answer(value: Any, fence: bool = False) -> tuple[int, dict[str, Any]]:
    text = json.dumps(value)
    return 200, {"content": [{"type": "text", "text": f"```json\n{text}\n```" if fence else text}],
                 "stop_reason": "end_turn", "usage": {"input_tokens": 1, "output_tokens": 1}}


def make(script: Scripted, refusals: Refusals, schema: dict[str, Any] = core.SUPERVISOR_SCHEMA,
         provider: str = "anthropic") -> ApiRuntime:
    async def key(name: str) -> str:
        return "sk-test"
    return ApiRuntime(PROVIDERS[provider](), mode="supervisor", role="orchestrator", tool_names=[],
                      output_schema=schema, bridge=None, key=key, transport=script.transport(), refusals=refusals)


async def run(rt: ApiRuntime) -> list[Any]:
    handle = await rt.start(TASK, REQ, "")
    return [e async for e in rt.events(handle)]


def stated_schema(text: str) -> Any:
    """The schema at the end of the instructions (the answer form is its last line)."""
    return json.loads(text.rsplit("\n", 1)[-1])


def check_stated(provider: str, body: dict[str, Any], schema: dict[str, Any]) -> None:
    """A request with no typed format, the answer form after FERB's own instructions, caching kept."""
    if provider == "anthropic":
        assert "format" not in body.get("output_config", {}) and body["output_config"]["effort"] == "low"
        system = body["system"][-1]
        assert system["cache_control"] == {"type": "ephemeral", "ttl": "1h"} and system["text"].startswith("FERB instructions")
        assert stated_schema(system["text"]) == schema
        assert json.dumps(body).count('"cache_control"') <= 4
    elif provider == "openai":
        assert "text" not in body
        developer = body["input"][0]["content"][0]
        assert developer["text"].startswith("FERB instructions") and stated_schema(developer["text"]) == schema
        assert developer["prompt_cache_breakpoint"] == {"mode": "explicit"}          # FERB's instructions stay cached
    else:
        config = body.get("generationConfig", {})
        assert "responseJsonSchema" not in config and "responseMimeType" not in config
        parts = body["systemInstruction"]["parts"]
        assert parts[0]["text"] == "FERB instructions" and stated_schema(parts[-1]["text"]) == schema


# decision: RUN-427, RUN-408
@pytest.mark.parametrize("name", sorted(SENT))
def test_every_schema_ferb_sends_fits_anthropics_counted_limits(name: str) -> None:
    wire = for_anthropic(SENT[name]).wire
    unions, optional = complexity(wire)
    assert unions <= ANTHROPIC_MAX_UNIONS and optional <= ANTHROPIC_MAX_OPTIONAL, (name, unions, optional)
    for_gemini(SENT[name])


# decision: RUN-427
@pytest.mark.anyio
@pytest.mark.parametrize("provider", sorted(PROVIDERS))
@pytest.mark.parametrize("name", sorted(SENT))
async def test_the_stated_form_is_a_valid_request_for_every_schema_and_provider(name: str, provider: str) -> None:
    refusals = Refusals()
    rt = make(Scripted([]), refusals, SENT[name], provider)
    assert rt.shape is not None
    refusals.remember(rt.format_key)
    handle = await rt.start(TASK, REQ, "")
    check_stated(provider, handle.state["run"].state["body"], SENT[name])
    await rt.cancel(handle)


# decision: RUN-427
@pytest.mark.anyio
async def test_a_grammar_too_large_refusal_falls_back_once_and_later_calls_skip_the_strict_form() -> None:
    refusals = Refusals()
    answer = sample(core.SUPERVISOR_SCHEMA, False) | {"kind": "reply", "text": "Hello"}
    script = Scripted([(400, GRAMMAR), anthropic_answer(answer, fence=True)])
    events = await run(make(script, refusals))
    assert "format" in script.requests[0][1]["output_config"]                      # strict first
    check_stated("anthropic", script.requests[1][1], core.SUPERVISOR_SCHEMA)       # then the same request, stated
    final = next(e for e in events if e.kind == "final").data["text"]
    assert json.loads(check_final(final, core.SUPERVISOR_SCHEMA)) == answer
    assert [e.data["typed"] for e in events if e.kind == "usage"] == ["stated_after_refusal"]
    assert not any(e.kind == "error" for e in events)

    later = Scripted([anthropic_answer(answer)])                                    # the next call: stated at once
    events = await run(make(later, refusals))
    assert len(later.requests) == 1
    check_stated("anthropic", later.requests[0][1], core.SUPERVISOR_SCHEMA)
    assert [e.data["typed"] for e in events if e.kind == "usage"] == ["stated"]


# decision: RUN-427
@pytest.mark.anyio
async def test_the_strict_form_is_tried_again_after_a_day() -> None:
    now = [0.0]
    refusals = Refusals(lambda: now[0])
    answer = {"ok": True}
    schema = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"],
              "additionalProperties": False}
    await run(make(Scripted([(400, GRAMMAR), anthropic_answer(answer)]), refusals, schema))
    now[0] = STATED_FOR_S - 1
    early = Scripted([anthropic_answer(answer)])
    await run(make(early, refusals, schema))
    assert "output_config" not in early.requests[0][1] or "format" not in early.requests[0][1]["output_config"]
    now[0] = STATED_FOR_S + 1
    later = Scripted([anthropic_answer(answer)])
    events = await run(make(later, refusals, schema))
    assert "format" in later.requests[0][1]["output_config"]
    assert [e.data["typed"] for e in events if e.kind == "usage"] == ["strict"]


# decision: RUN-427, RUN-1
@pytest.mark.anyio
async def test_a_400_the_stated_form_gets_too_stays_its_own_error_and_is_not_remembered() -> None:
    refusals = Refusals()
    other = {"type": "error", "error": {"type": "invalid_request_error", "message": "messages: something else"}}
    script = Scripted([(400, other), (400, other)])
    events = await run(make(script, refusals))
    assert len(script.requests) == 2
    assert events[-1].kind == "error" and "something else" in events[-1].data["message"]
    rt = make(Scripted([]), refusals)
    assert not refusals.refused(rt.format_key)


# decision: RUN-427, RUN-440
@pytest.mark.anyio
async def test_a_stated_answer_outside_the_form_is_told_back_once() -> None:
    refusals = Refusals()
    good = sample(core.SUPERVISOR_SCHEMA, False) | {"kind": "reply", "text": "Hello"}
    bad = {"kind": "reply"}
    rt = make(Scripted([]), refusals)
    refusals.remember(rt.format_key)
    script = Scripted([anthropic_answer(bad), anthropic_answer(good)])
    events = await run(make(script, refusals))
    assert len(script.requests) == 2
    messages = script.requests[1][1]["messages"]
    assert [m["role"] for m in messages] == ["user", "assistant", "user"]           # the problem, told back once
    assert json.loads(next(e for e in events if e.kind == "final").data["text"]) == good
    # RUN-440: each usage report says how the repair went
    assert [e.data.get("repair") for e in events if e.kind == "usage"] == ["asked", "fixed"]

    twice = Scripted([anthropic_answer(bad), anthropic_answer(bad)])                # a second miss: the caller's check
    events = await run(make(twice, refusals))
    assert len(twice.requests) == 2
    assert [e.data.get("repair") for e in events if e.kind == "usage"] == ["asked", "failed"]
    with pytest.raises(ValueError):
        check_final(next(e for e in events if e.kind == "final").data["text"], core.SUPERVISOR_SCHEMA)


# decision: RUN-427
@pytest.mark.anyio
async def test_a_schema_over_the_counted_limits_is_stated_from_the_start() -> None:
    props = {f"f{i}": {"type": "object", "additionalProperties": False, "required": ["v"],
                       "properties": {"v": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}} for i in range(20)}
    schema = {"type": "object", "additionalProperties": False, "required": list(props), "properties": props}
    with pytest.raises(SchemaTooComplex):
        for_anthropic(schema)
    answer = {f"f{i}": {"v": i} for i in range(20)}
    script = Scripted([anthropic_answer(answer)])
    events = await run(make(script, Refusals(), schema))
    check_stated("anthropic", script.requests[0][1], schema)
    assert json.loads(next(e for e in events if e.kind == "final").data["text"]) == answer


# ---------- Claude Agent SDK ----------

class Client:
    """Claude Code as the SDK client: the first one's API call is refused, the next one answers."""
    made: list["Client"] = []
    results: list[Any] = []

    def __init__(self, options: Any) -> None:
        self.options = options
        self.result = Client.results.pop(0)
        Client.made.append(self)

    async def connect(self) -> None:
        pass

    async def query(self, frames: Any) -> None:
        self.frames = [f async for f in frames]

    async def receive_messages(self) -> Any:
        yield self.result

    async def disconnect(self) -> None:
        pass


# decision: RUN-427
@pytest.mark.anyio
async def test_claude_code_refused_on_the_typed_format_starts_again_stated(monkeypatch) -> None:
    import claude_agent_sdk
    from claude_agent_sdk import ResultMessage
    monkeypatch.setattr(claude_agent_sdk, "ClaudeSDKClient", Client)
    answer = sample(core.SUPERVISOR_SCHEMA, False) | {"kind": "reply", "text": "Hello"}
    Client.made, Client.results = [], [
        ResultMessage("success", 1, 1, True, 1, "s", result="API Error: 400 " + json.dumps(GRAMMAR),
                      api_error_status=400),
        ResultMessage("success", 1, 1, False, 1, "s", result=json.dumps(answer)),
        ResultMessage("success", 1, 1, False, 1, "s", result=json.dumps(answer))]
    proxy = SimpleNamespace(seen=lambda ident: set(), forget=lambda ident: None, base_url=lambda ident: "http://unused")
    refusals = Refusals()

    def runtime() -> _Claude:
        return _Claude("supervisor", proxy, signed_in_home(),  # type: ignore[arg-type]
                       output_schema=core.SUPERVISOR_SCHEMA, model="sonnet", refusals=refusals)
    rt = runtime()
    handle = await CLAUDE_START(rt, TASK, REQ, "")
    events = [e async for e in rt.events(handle)]
    first, second = Client.made
    assert first.options.output_format is not None and second.options.output_format is None
    system = second.frames[0]["message"]["content"][0]["text"]
    assert system.startswith("FERB instructions") and stated_schema(system) == core.SUPERVISOR_SCHEMA
    assert json.loads(next(e for e in events if e.kind == "final").data["text"]) == answer
    assert not any(e.kind == "error" for e in events)
    assert not any("repair" in e.data for e in events if e.kind == "usage")       # RUN-440: it fit: no repair
    # the Anthropic API runtime and Claude Code share the fact (one feature, the same model)
    api = make(Scripted([]), refusals)
    assert api.provider.model == "claude-sonnet-5-5" and rt.format_key == api.format_key
    assert refusals.refused(api.format_key)
    again = runtime()
    handle = await CLAUDE_START(again, TASK, REQ, "")
    assert Client.made[-1].options.output_format is None
    await again.cancel(handle)
