"""Owns checking a model's typed answer against the schema FERB asked for. Must not repair, guess or fill fields.

Every runtime's final answer passes here once, whatever the provider promised about enforcing the schema (D32)."""

import json
from typing import Any

import jsonschema


def check_final(text: str, schema: dict[str, Any] | None) -> str:
    """The answer unchanged when it is JSON matching schema; ValueError naming the first problem otherwise."""
    if not schema:
        return text
    try:
        value = json.loads(text)
    except ValueError as e:
        raise ValueError(f"the answer is not JSON ({getattr(e, 'msg', e)})") from None
    error = jsonschema.exceptions.best_match(jsonschema.Draft202012Validator(schema).iter_errors(value))
    if error is not None:
        where = "/".join(str(p) for p in error.absolute_path) or "the top level"
        raise ValueError(f"the answer doesn't match the schema at {where}: {error.message[:200]}")
    return text
