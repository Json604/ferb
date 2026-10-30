"""Provider-neutral output schemas (D32, step 4): FERB's core schemas stay in OpenAI strict form; each adapter reshapes
them for its provider and maps the answer back before FERB's own check (ferb/runtimes/schema_shape.py)."""
import asyncio
import json
from typing import Any

import jsonschema
import pytest

from ferb.core.structured import check_final
from ferb.core.types import Identity, ModelRequest, RunHandle, Task
from ferb.memory.distill import DISTILL_SCHEMA
from ferb.orchestrator import schemas as core
from ferb.runtimes.schema_shape import (ANTHROPIC_MAX_OPTIONAL, ANTHROPIC_MAX_UNIONS, GEMINI_KEYWORDS, SchemaTooComplex,
                                        complexity, enum_with_null, for_anthropic, for_gemini, keywords, nullable)
from tests.support.runtime_fakes import Scripted

TASK = Task("t.planner", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
CORE = {name: getattr(core, name) for name in dir(core) if name.endswith("_SCHEMA")} | {"DISTILL_SCHEMA": DISTILL_SCHEMA}


# ---------- answers generated from a schema, and the form a provider's model writes them in ----------

def _non_null(node: dict[str, Any]) -> dict[str, Any]:
    if "anyOf" in node:
        return next(b for b in node["anyOf"] if b.get("type") != "null")
    kinds = [k for k in node["type"] if k != "null"]
    return {**node, "type": kinds[0], **({"enum": [v for v in node["enum"] if v is not None]} if "enum" in node else {})}


def sample(node: dict[str, Any], fill: bool) -> Any:
    """An answer valid for the core schema: every nullable value null (`fill` False) or filled in."""
    if nullable(node):
        return sample(_non_null(node), fill) if fill else None
    if "anyOf" in node:
        return sample(node["anyOf"][-1], fill)
    kind = node.get("type")
    if "enum" in node:
        return node["enum"][0]
    if kind == "object":
        return {name: sample(sub, fill) for name, sub in node.get("properties", {}).items()}
    if kind == "array":
        return [sample(node["items"], fill)] * max(1, node.get("minItems", 0))
    return {"string": "x", "integer": 1, "number": 1.5, "boolean": True}[kind]


def as_wire(core_node: dict[str, Any], wire_node: dict[str, Any], value: Any) -> Any:
    """`value` as a model answering the wire schema writes it: a nullable object as a list of at most one item, an
    optional null property left out."""
    if value is None:
        return [] if wire_node.get("type") == "array" and nullable(core_node) else None
    if nullable(core_node) and _non_null(core_node).get("type") == "object" and wire_node.get("type") == "array":
        return [as_wire(_non_null(core_node), wire_node["items"], value)]
    if nullable(core_node) and not nullable(wire_node) and "anyOf" not in wire_node:
        return as_wire(_non_null(core_node), wire_node, value)
    if isinstance(value, dict):
        required = set(wire_node.get("required", []))
        out = {}
        for name, sub in value.items():
            if sub is None and name not in required:
                continue
            out[name] = as_wire(core_node["properties"][name], wire_node["properties"][name], sub)
        return out
    if isinstance(value, list) and core_node.get("type") == "array":
        return [as_wire(core_node["items"], wire_node["items"], v) for v in value]
    if "anyOf" in core_node and isinstance(value, list):            # e.g. control.standing_sends: the array branch
        c = next(b for b in core_node["anyOf"] if b.get("type") == "array")
        w = next(b for b in wire_node["anyOf"] if b.get("type") == "array")
        return [as_wire(c["items"], w["items"], v) for v in value]
    return value


# ---------- Anthropic ----------

@pytest.mark.parametrize("name", sorted(CORE))
def test_every_core_schema_fits_anthropics_limits_and_round_trips(name: str) -> None:
    schema = CORE[name]
    shape = for_anthropic(schema)
    unions, optional = complexity(shape.wire)
    assert unions <= ANTHROPIC_MAX_UNIONS and optional <= ANTHROPIC_MAX_OPTIONAL, (name, unions, optional)
    assert shape.core is schema                                      # the core schema itself is never changed
    for fill in (False, True):
        answer = sample(schema, fill)
        jsonschema.validate(answer, schema)
        wire = as_wire(schema, shape.wire, answer)
        jsonschema.validate(wire, shape.wire)                        # what the provider would accept from its model
        back = json.loads(shape.answer_text(json.dumps(wire)))
        assert back == answer, name
        check_final(json.dumps(back), schema)                        # FERB's one check sees the core shape


def test_the_planners_schema_was_over_the_union_limit_and_now_fits() -> None:
    unions, _ = complexity(core.SUPERVISOR_SCHEMA)
    assert unions > ANTHROPIC_MAX_UNIONS                              # 42: the API refused it ("too many ... union")
    wire = for_anthropic(core.SUPERVISOR_SCHEMA).wire
    assert set(wire["required"]) == set(core.SUPERVISOR_SCHEMA["required"])   # every top-level key stays required
    assert wire["properties"]["task"]["type"] == "array"             # a nullable object: a list of at most one


def test_a_schema_still_over_a_limit_is_refused_before_sending() -> None:
    props = {f"f{i}": {"type": "object", "additionalProperties": False, "required": ["v"],
                       "properties": {"v": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}} for i in range(20)}
    schema = {"type": "object", "additionalProperties": False, "required": list(props), "properties": props}
    with pytest.raises(SchemaTooComplex, match="Anthropic allows"):
        for_anthropic(schema)


def test_a_missing_required_key_still_fails_ferbs_check() -> None:
    shape = for_anthropic(core.CHECKER_SCHEMA)
    back = shape.answer_text(json.dumps({"next": "retry"}))
    with pytest.raises(ValueError):
        check_final(back, core.CHECKER_SCHEMA)


def test_a_list_of_two_for_one_optional_object_is_left_for_ferbs_check() -> None:
    shape = for_anthropic(core.SUPERVISOR_SCHEMA)
    answer = sample(core.SUPERVISOR_SCHEMA, False) | {"kind": "reply", "text": "hi"}
    wire = as_wire(core.SUPERVISOR_SCHEMA, shape.wire, answer)
    wire["schedule"] = [{}, {}]
    with pytest.raises(ValueError):
        check_final(shape.answer_text(json.dumps(wire)), core.SUPERVISOR_SCHEMA)


@pytest.mark.anyio
async def test_the_anthropic_api_gets_the_reshaped_schema_and_ferb_gets_its_own_answer_back() -> None:
    from ferb.runtimes.anthropic_api import AnthropicProvider
    from ferb.runtimes.api import ApiRuntime
    shape = for_anthropic(core.SUPERVISOR_SCHEMA)
    answer = sample(core.SUPERVISOR_SCHEMA, False) | {"kind": "reply", "text": "Hello"}
    wire = as_wire(core.SUPERVISOR_SCHEMA, shape.wire, answer)
    script = Scripted([(200, {"content": [{"type": "text", "text": json.dumps(wire)}], "stop_reason": "end_turn",
                              "usage": {"input_tokens": 1, "output_tokens": 1}})])

    async def key(provider: str) -> str:
        return "sk-test"
    rt = ApiRuntime(AnthropicProvider("claude-sonnet-5-5"), mode="supervisor", role="orchestrator", tool_names=[],
                    output_schema=core.SUPERVISOR_SCHEMA, bridge=None, key=key, transport=script.transport())
    run = await rt.start(TASK, ModelRequest("system", ("context",)), "")
    events = [e async for e in rt.events(run)]
    assert script.requests[0][1]["output_config"]["format"]["schema"] == shape.wire
    final = next(e for e in events if e.kind == "final").data["text"]
    assert json.loads(final) == answer and check_final(final, core.SUPERVISOR_SCHEMA)


@pytest.mark.anyio
async def test_claude_codes_typed_answer_is_mapped_back_to_ferbs_form() -> None:
    from types import SimpleNamespace

    from claude_agent_sdk import ResultMessage
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    from tests.support.claude_home import signed_in_home
    shape = for_anthropic(core.SUPERVISOR_SCHEMA)
    answer = sample(core.SUPERVISOR_SCHEMA, False) | {"kind": "reply", "text": "Hello"}
    wire = as_wire(core.SUPERVISOR_SCHEMA, shape.wire, answer)

    class Client:
        async def receive_messages(self) -> Any:
            yield ResultMessage("success", 1, 1, False, 1, "s", structured_output=wire)

        async def disconnect(self) -> None:
            pass
    proxy = SimpleNamespace(seen=lambda ident: set(), forget=lambda ident: None,
                            base_url=lambda ident: "http://unused")
    rt = ClaudeRuntime("supervisor", proxy, signed_in_home(), output_schema=core.SUPERVISOR_SCHEMA,  # type: ignore[arg-type]
                       model="opus")
    assert rt._options("run", None).output_format["schema"] == shape.wire
    run = RunHandle("run", "claude", "t", {"client": Client(), "checked_auth": True})
    final = next(e for e in [e async for e in rt.events(run)] if e.kind == "final").data["text"]
    assert json.loads(final) == answer


def test_the_planners_streamed_reply_reads_reshaped_fields() -> None:
    """The reply streams as it is written; with the schema reshaped, its metadata arrive in the provider's form and are
    read back field by field (a task's empty list for no schedule is null)."""
    from ferb.orchestrator.reply_stream import ReplyDraft
    from ferb.sanitize.redact import Redactor
    shape = for_anthropic(core.SUPERVISOR_SCHEMA)
    answer = {k: ([] if isinstance(v, list) else v) for k, v in sample(core.SUPERVISOR_SCHEMA, False).items()}
    answer |= {"kind": "reply", "text": "Hello there"}
    wire = as_wire(core.SUPERVISOR_SCHEMA, shape.wire, answer)
    assert wire["task"] == [] and wire["schedule"] == []             # the provider's form, not null
    text = json.dumps({k: wire[k] for k in [*[k for k in wire if k != "text"], "text"]})
    seen: list[dict[str, Any]] = []
    draft = ReplyDraft(Redactor(), seen.append, shape.field)
    for i in range(0, len(text), 7):
        draft.feed(text[i:i + 7])
    assert "".join(e["text"] for e in seen if e["type"] == "reply_delta").startswith("Hello")
    # a task's metadata are validated as FERB's own outcome before its text streams: only after mapping back
    answer = answer | {"kind": "task", "task": sample(core.TASK_SCHEMA, False), "text": "Working on it"}
    wire = as_wire(core.SUPERVISOR_SCHEMA, shape.wire, answer)
    text = json.dumps({k: wire[k] for k in [*[k for k in wire if k != "text"], "text"]})
    for field, streams in ((shape.field, True), (None, False)):
        seen.clear()
        draft = ReplyDraft(Redactor(), seen.append, field)
        draft.feed(text)
        assert any(e["type"] == "plan_delta" for e in seen) is streams


# ---------- Gemini ----------

@pytest.mark.parametrize("name", sorted(CORE))
def test_every_core_schema_uses_only_geminis_documented_keywords(name: str) -> None:
    schema = CORE[name]
    shape = for_gemini(schema)
    assert keywords(shape.wire) <= GEMINI_KEYWORDS, keywords(shape.wire) - GEMINI_KEYWORDS
    assert not enum_with_null(shape.wire)                            # Gemini's enum holds strings and numbers
    for fill in (False, True):
        answer = sample(schema, fill)
        jsonschema.validate(answer, shape.wire)                      # the same answers, nothing to map back
        assert shape.answer_text(json.dumps(answer)) == json.dumps(answer)


def test_a_nullable_enum_becomes_an_any_of_for_gemini() -> None:
    schema = core._obj({"loop_action": {"type": ["string", "null"], "enum": ["continue", "finished", "blocked", None]}})
    wire = for_gemini(schema).wire["properties"]["loop_action"]
    assert wire["anyOf"] == [{"type": "string", "enum": ["continue", "finished", "blocked"]}, {"type": "null"}]


def test_gemini_parallel_calls_to_one_tool_without_ids_stay_apart() -> None:
    from ferb.runtimes.api import ToolOutcome
    from ferb.runtimes.google_api import GoogleProvider
    provider = GoogleProvider("gemini-3.8-flash")
    state = provider.begin(ModelRequest("", ("go",)), [], None, None, "")
    data = {"candidates": [{"finishReason": "STOP", "content": {"role": "model", "parts": [
        {"functionCall": {"name": "fs_list", "args": {"path": "/a"}}},
        {"functionCall": {"name": "fs_list", "args": {"path": "/b"}}},
        {"functionCall": {"name": "fs_read", "id": "g-7", "args": {"path": "/c"}}}]}}]}
    turn = provider.parse(state, data)
    assert len({c.id for c in turn.calls}) == 3
    provider.add_results(state, turn, [ToolOutcome(c, "ok", False) for c in turn.calls])
    responses = [p["functionResponse"] for p in state["body"]["contents"][-1]["parts"]]
    assert [r.get("id") for r in responses] == [None, None, "g-7"]   # only Gemini's own ids go back; order pairs the rest
    assert [r["name"] for r in responses] == ["fs_list", "fs_list", "fs_read"]


def test_haiku_5_5_has_a_price_with_its_long_prompt_rate() -> None:
    from ferb.runtimes.usage import cost_usd
    assert cost_usd("claude-haiku-5-5", 50_000, 0, 0, 50_000) == pytest.approx(0.03)
    assert cost_usd("claude-haiku-5-5", 200_000, 0, 0, 0) == pytest.approx(0.10)       # over 100k: 5x input


