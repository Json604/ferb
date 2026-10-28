"""Live 2026-10-28: on the Anthropic API runtime every planner call failed with "HTTP 400: output_config.format.schema:
For 'integer' type, property 'minimum' is not supported" (0dd5185 added bounds to CONTEXT_READ_SCHEMA). Anthropic's
structured outputs reject numerical and string constraints, complex array constraints (minItems above 1, maxItems)
and the like; FERB's own Pydantic models check the answer afterwards, so the Claude adapters drop those keywords from
the schema they send, never from FERB's schema itself."""

from typing import Any

import pytest
from pydantic import ValidationError

from ferb.orchestrator.schemas import SUPERVISOR_SCHEMA, ContextRead
from ferb.runtimes.anthropic_api import AnthropicProvider, structured_schema

UNSUPPORTED = {"minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf", "minLength", "maxLength",
               "maxItems"}


def _keywords(node: Any, out: set[str]) -> set[str]:
    if isinstance(node, dict):
        for key, value in node.items():
            if key == "properties" and isinstance(value, dict):
                for sub in value.values():
                    _keywords(sub, out)
            else:
                out.add(key)
                _keywords(value, out)
    elif isinstance(node, list):
        for item in node:
            _keywords(item, out)
    return out


def test_the_schema_sent_to_claude_carries_no_keyword_it_rejects() -> None:
    assert UNSUPPORTED & _keywords(SUPERVISOR_SCHEMA, set())           # FERB's own schema keeps its bounds
    body = AnthropicProvider("claude-sonnet-5-5").begin(_request(), [], SUPERVISOR_SCHEMA, None, "k")["body"]
    schema = next(t["input_schema"] for t in body["tools"] if t["name"] == "final_answer")    # carried: see below
    assert not UNSUPPORTED & _keywords(schema, set())
    with pytest.raises(ValidationError):                               # and FERB still checks them on the answer
        ContextRead(source="history", limit=500)


def test_only_keywords_are_dropped_never_a_field_named_like_one() -> None:
    schema = {"type": "object", "additionalProperties": False, "required": ["minimum", "items"],
              "properties": {"minimum": {"type": "integer", "minimum": 0},
                             "items": {"type": "array", "items": {"type": "string", "maxLength": 9},
                                       "minItems": 1, "maxItems": 3}}}
    assert structured_schema(schema) == {
        "type": "object", "additionalProperties": False, "required": ["minimum", "items"],
        "properties": {"minimum": {"type": "integer"},
                       "items": {"type": "array", "items": {"type": "string"}, "minItems": 1}}}
    assert "minItems" not in structured_schema({"type": "array", "minItems": 2})


def _request() -> Any:
    from ferb.core.types import ModelRequest
    return ModelRequest(system="", parts=("plan this",))


def test_a_schema_past_anthropics_limits_travels_in_a_final_answer_tool() -> None:
    """Live 2026-10-28, right after the keyword fix: "anthropic HTTP 400: Schemas contains too many parameters with union
    types (48 parameters with type arrays or anyOf)... limit: 16 parameters with unions". Anthropic's documented limits
    for strict schemas are 16 union-typed and 24 optional parameters per request; non-strict tools don't count. The
    planner's schema (48 unions, also 46 before 0dd5185) goes in a non-strict final_answer tool whose call is the
    answer; a schema within the limits keeps strict structured output."""
    import json
    from ferb.orchestrator.schemas import WORKER_SCHEMA
    provider = AnthropicProvider("claude-sonnet-5-5")
    state = provider.begin(_request(), [], SUPERVISOR_SCHEMA, None, "k")
    body = state["body"]
    assert "format" not in body.get("output_config", {})
    tool = next(t for t in body["tools"] if t["name"] == "final_answer")
    assert tool["input_schema"] == structured_schema(SUPERVISOR_SCHEMA) and "strict" not in tool
    assert body["tool_choice"] == {"type": "auto"} and "final_answer" in body["system"][0]["text"]
    answer = {"kind": "reply", "text": "Hi"}
    turn = provider.parse(state, {"stop_reason": "tool_use", "usage": {},
                                  "content": [{"type": "tool_use", "id": "t1", "name": "final_answer", "input": answer}]})
    assert turn.calls == [] and json.loads(turn.final) == answer
    small = provider.begin(_request(), [], WORKER_SCHEMA, None, "k")["body"]
    assert small["output_config"]["format"]["schema"] == structured_schema(WORKER_SCHEMA) and "tools" not in small
