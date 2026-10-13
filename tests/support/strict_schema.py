"""The strict structured-output rules FERB's output schemas must meet (Codex/OpenAI strict mode, Anthropic
output_config): every object sets additionalProperties false and lists every property as required."""
from typing import Any


def strict_problems(node: Any, path: str = "$") -> list[str]:
    """Each place in the schema that breaks the strict rules; empty when the schema is strict-compatible."""
    out = []
    if isinstance(node, dict):
        if node.get("type") == "object" or "properties" in node:
            if node.get("additionalProperties") is not False:
                out.append(f"{path}: additionalProperties")
            if set(node.get("properties") or {}) - set(node.get("required") or []):
                out.append(f"{path}: required")
        for k, v in node.items():
            out += strict_problems(v, f"{path}.{k}")
    elif isinstance(node, list):
        for i, v in enumerate(node):
            out += strict_problems(v, f"{path}[{i}]")
    return out
