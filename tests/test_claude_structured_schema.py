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
    sent = AnthropicProvider("claude-sonnet-5-5").begin(_request(), [], SUPERVISOR_SCHEMA, None, "k")
    schema = sent["body"]["output_config"]["format"]["schema"]
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
