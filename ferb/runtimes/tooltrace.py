"""Model-visible tool names from a Codex request trace (spike 1): each request's `"tools":[...]` and every
`tool_search` result (logged as `output=[...]`, which loads MCP namespaces). Nameless hosted tools are kept by
type — a tool the check can't see is exactly what it must never miss."""

import json
from typing import Any

MARKERS = ('"tools":', " output=")


def tools_in(text: str) -> set[str]:
    dec = json.JSONDecoder()
    names: set[str] = set()
    i = 0
    while True:
        hits = [(p, m) for m in MARKERS if (p := text.find(m + "[", i)) >= 0]
        if not hits:
            return names
        pos, marker = min(hits)
        try:
            arr, end = dec.raw_decode(text, pos + len(marker))
        except json.JSONDecodeError:
            i = pos + 1
            continue
        if marker == '"tools":' or _looks_like_tools(arr):
            names |= _flatten(arr)
            i = end
        else:
            i = pos + 1


def _looks_like_tools(arr: Any) -> bool:
    return isinstance(arr, list) and bool(arr) and all(
        isinstance(t, dict) and t.get("type") in ("function", "namespace", "custom") for t in arr)


def _flatten(tools: list[Any]) -> set[str]:
    out: set[str] = set()
    for t in tools:
        if not isinstance(t, dict):
            continue
        if t.get("tools"):
            out |= {f"{t['name']}.{n}" for n in _flatten(t["tools"])}
        else:
            out.add(t.get("name") or t.get("type") or json.dumps(t)[:80])
    return out
