"""Strict structured-output helpers for learning calls (Codex strict output allows no open object): an open map is a
list of {name, value}, read back here. Shared by the consolidation and intent-label schemas."""
import json
from typing import Any

from ferb.memory.learning import LearningRefused

JSON_TEXT = {'type': 'string', 'description': 'the value as JSON text: a string in double quotes, a number, '
                                              'true/false, a list or an object'}


def pairs(value: dict[str, Any]) -> dict[str, Any]:
    """An open map in strict form: a list of {name, value}."""
    return {'type': 'array', 'items': {'type': 'object', 'additionalProperties': False, 'required': ['name', 'value'],
                                       'properties': {'name': {'type': 'string'}, 'value': value}}}


def to_map(items: list[dict[str, Any]], *, json_text: bool = False) -> dict[str, Any]:
    """The map a strict {name, value} list stands for. A repeated name is refused; a value that isn't JSON text is
    the literal string (a model often writes a bare string where JSON text was asked for)."""
    out: dict[str, Any] = {}
    for pair in items:
        name, value = pair['name'], pair['value']
        if name in out:
            raise LearningRefused(f'{name} is given twice')
        if json_text:
            try:
                value = json.loads(value)
            except ValueError:
                pass                        # not JSON text: the string as written
        out[name] = value
    return out
