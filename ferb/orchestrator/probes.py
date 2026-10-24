"""L31: a loop's probe. The round's worker chose one observation tool, its arguments, a field of its result and an
exact comparison; code validates it, reads the field and compares JSON values. Code never judges what a value means
(R2): the model decided which value means the job needs doing again, as `event_match` does for waits."""
import json
from collections.abc import Iterable
from typing import Any, Literal

from pydantic import BaseModel, ValidationError

MISSING: Any = object()

PROBE_SCHEMA: dict[str, Any] = {
    "type": "object", "additionalProperties": False,
    "required": ["tool", "args_json", "field", "op", "value_json"],
    "properties": {"tool": {"type": "string"}, "args_json": {"type": "string"}, "field": {"type": "string"},
                   "op": {"type": "string", "enum": ["equals", "not_equals", "changed"]},
                   "value_json": {"type": "string"}}}


class Probe(BaseModel):
    tool: str
    args_json: str = '{}'
    field: str
    op: Literal['equals', 'not_equals', 'changed']
    value_json: str = 'null'

    def args(self) -> dict[str, Any]:
        value = json.loads(self.args_json or '{}')
        if not isinstance(value, dict):
            raise ValueError('probe arguments must be a JSON object')
        return value

    def value(self) -> Any:
        return json.loads(self.value_json or 'null')


# What a runtime puts before a FERB tool's name in the name its model sees: the Claude Agent SDK and Codex name an MCP
# server's tool `mcp__<server>__<tool>` (claude_sdk.py's allow-list; Codex's own convention, `ferb__<tool>` in older
# Codex), and FERB's Codex adapter allow-lists `mcp__ferb.<tool>` (codex_exec.py, codex_stream.py).
RUNTIME_PREFIXES = ('mcp__ferb__', 'mcp__ferb.', 'ferb__')


def resolve_tool(written: str, registered: Iterable[str]) -> tuple[str, str | None]:
    """The registered tool a probe names, from the name the model wrote: a registered name itself, or the form a model
    sees (`_` for `.`, cli/mcp.py), after at most one exact runtime prefix (RUNTIME_PREFIXES). Returns (name, None);
    a name that is no tool comes back as written, for `refusal` to refuse. When two registered tools share that form,
    neither is guessed: (written, why)."""
    names = list(registered)
    for name in (written, *(written[len(p):] for p in RUNTIME_PREFIXES if written.startswith(p))):
        found = sorted(n for n in names if n == name or n.replace('.', '_') == name)
        if len(found) == 1:
            return found[0], None
        if found:
            return written, f"the probe's tool {written} could be any of {', '.join(found)}: name one of them exactly"
    return written, None


def refusal(probe: Probe, cap: Any, scopes: Iterable[str], identity: Any) -> str | None:
    """Why this probe can't be armed, as a plain sentence for the next round; None when it can."""
    if cap is None:
        return f'{probe.tool} is not a FERB tool'
    if not set(cap.scopes) <= set(scopes) or identity not in cap.identities:
        return f'{probe.tool} is outside this task'
    if not getattr(cap, 'observation', False) or not getattr(cap, 'probe_safe', False):
        return f"{probe.tool} can't be a probe: a probe must only read, move nothing and return at once"
    if not probe.field.strip():
        return 'a probe needs a field of the result to compare'
    try:
        cap.Input.model_validate(probe.args())
        if probe.op != 'changed':
            probe.value()
    except (ValueError, ValidationError) as e:
        return f'invalid probe: {e}'
    return None


def field_value(content: Any, path: str) -> Any:
    """The value at a dot path (`media.0.playing`): dict keys, and list indexes as digits; MISSING if absent."""
    node = content
    for part in path.split('.'):
        if isinstance(node, dict) and part in node:
            node = node[part]
        elif isinstance(node, list) and part.isdigit() and int(part) < len(node):
            node = node[int(part)]
        else:
            return MISSING
    return node


def same(a: Any, b: Any) -> bool:
    """Exact JSON equality: `true` is not `1`, `"false"` is not `false`, `1` is not `1.0`.
    The MISSING sentinel compares by identity only."""
    if a is MISSING or b is MISSING:
        return a is b
    return json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True)


def tripped(probe: Probe, observed: Any, baseline: Any) -> bool:
    """Whether this read shows the job needs doing again; a missing field always does."""
    if observed is MISSING:
        return True
    if probe.op == 'changed':
        return not same(observed, baseline)
    return same(observed, probe.value()) == (probe.op == 'equals')
