"""Owns reshaping FERB's provider-neutral output schemas into what one provider accepts, and the answer back (D32). Must
not change what an answer means: the core schema stays the one every answer is checked against
(ferb/core/structured.py), after the adapter maps it back.

FERB's core schemas are in OpenAI strict form: every property required, `null` where a value may be absent. Codex and the
OpenAI API take them as they are. Other providers limit that form:

- Anthropic (the API's `output_config.format`, and Claude Code's typed output, which uses the same feature): at most 16
  parameters with union types (`anyOf`, or a type list such as ["string", "null"]) and at most 24 optional parameters,
  across the request's strict schemas (https://platform.claude.com/docs/en/build-with-claude/structured-outputs,
  "Schema complexity limits", checked 2026-10-30). SUPERVISOR_SCHEMA has 42 nullable properties, which the API refused
  ("too many parameters with union types", 4 spans). `for_anthropic` turns each nullable object into a list of at
  most one item (no union, nothing optional), each nested nullable scalar into an optional non-null property while the
  optional budget lasts, and leaves the rest as unions; it refuses (SchemaTooComplex) a schema still over a limit.
- Gemini (generateContent `responseJsonSchema`): nullability is a type list; `enum` is for strings and numbers, so an
  enum holding null is written as anyOf [the enum, null] (the same values). Nothing to map back.

Answers come back through `Shape.answer_text` (a missing nullable property becomes null; a list standing for a
nullable object becomes its one item, or null when empty) before FERB's own check, which then sees the core shape.

The counted limits are not the only ones. Anthropic also refuses a typed format whose compiled grammar is too large
(undocumented size; the owner's planner turns 2026-11-08 05:20, 06:00, 07:10: "HTTP 400: The compiled grammar is too
large, which would cause performance issues", on SUPERVISOR_SCHEMA after reshaping, 14 unions and 24 optionals), and
Gemini a schema whose constraint "has too many states" (the shape FERB's runtime notes expected, LIVE-411). So every
adapter that sends a typed format has one fallback (RUN-427): when the provider refuses a request that carried it
(HTTP 400, or Claude Code's `api_error_status` 400) and the same request without the format is accepted, the format
didn't fit. The answer form is then stated in the instructions (`stated`), each answer is checked against the core
schema, one that doesn't fit is told back once (`repair_note`), and `Refusals` remembers the fit for STATED_FOR_S per
runtime, model and wire schema, so later calls go straight to the stated form. A new FERB build starts with none
remembered (the memory is the daemon's), so it tries the strict form again."""

import copy
import hashlib
import json
import time
from dataclasses import dataclass
from typing import Any, Callable

# https://platform.claude.com/docs/en/build-with-claude/structured-outputs, "Schema complexity limits" (2026-10-30)
ANTHROPIC_MAX_UNIONS = 16
ANTHROPIC_MAX_OPTIONAL = 24
WRAPPED_NOTE = "A list standing for one optional object: empty for none, otherwise exactly one item."
# The JSON Schema keywords Gemini's structured output documents (https://ai.google.dev/gemini-api/docs/structured-output,
# checked 2026-10-30), plus $ref/$defs/description/title, which FERB's schemas may carry
GEMINI_KEYWORDS = frozenset({"type", "properties", "required", "additionalProperties", "items", "prefixItems",
                             "minItems", "maxItems", "enum", "format", "minimum", "maximum", "anyOf", "$ref", "$defs",
                             "description", "title"})


class SchemaTooComplex(ValueError):
    """A core schema that stays over a provider's documented limit after reshaping: refused before anything is sent."""


@dataclass(frozen=True)
class Shape:
    """A core output schema and the form one provider receives (`wire`); `mapped` when answers need mapping back."""
    core: dict[str, Any]
    wire: dict[str, Any]
    mapped: bool = False

    def value(self, answer: Any) -> Any:
        return _back(self.core, answer) if self.mapped else answer

    def field(self, key: str, value: Any) -> Any:
        """One top-level property's answer in the core shape (the planner's streamed reply reads them one by one)."""
        prop = (self.core.get("properties") or {}).get(key)
        return _back(prop, value) if self.mapped and isinstance(prop, dict) else value

    def answer_text(self, text: str) -> str:
        """The final answer's text in the core shape; text that isn't JSON is left for FERB's check to name."""
        if not self.mapped:
            return text
        try:
            answer = json.loads(text)
        except ValueError:
            return text
        return json.dumps(_back(self.core, answer), ensure_ascii=False)


def same(schema: dict[str, Any]) -> Shape:
    return Shape(schema, schema)


# ---------- reading a schema ----------

def _branches(node: dict[str, Any]) -> list[dict[str, Any]]:
    return [b for b in node.get("anyOf", []) if isinstance(b, dict) and b.get("type") != "null"]


def nullable(node: Any) -> bool:
    if not isinstance(node, dict):
        return False
    kind = node.get("type")
    return ((isinstance(kind, list) and "null" in kind)
            or any(isinstance(b, dict) and b.get("type") == "null" for b in node.get("anyOf", []))
            or (isinstance(node.get("enum"), list) and None in node["enum"]))


def is_union(node: Any) -> bool:
    return isinstance(node, dict) and ("anyOf" in node or isinstance(node.get("type"), list))


def _non_null(node: dict[str, Any]) -> dict[str, Any] | None:
    """The one non-null form of a nullable property; None when it has several."""
    if "anyOf" in node:
        rest = _branches(node)
        if len(rest) != 1:
            return None
        out = copy.deepcopy(rest[0])
        if "description" in node and "description" not in out:
            out["description"] = node["description"]
        return out
    kind = node.get("type")
    if isinstance(kind, list):
        kinds = [k for k in kind if k != "null"]
        if len(kinds) != 1:
            return None
        out = {k: copy.deepcopy(v) for k, v in node.items() if k != "type"}
        out["type"] = kinds[0]
        if isinstance(out.get("enum"), list):
            out["enum"] = [v for v in out["enum"] if v is not None]
        return out
    return None


def _children(node: dict[str, Any]) -> list[Any]:
    out: list[Any] = list((node.get("properties") or {}).values())
    if isinstance(node.get("items"), dict):
        out.append(node["items"])
    out += node.get("anyOf", []) + node.get("allOf", []) + list((node.get("$defs") or {}).values())
    return out


def complexity(schema: dict[str, Any]) -> tuple[int, int]:
    """(parameters with union types, optional parameters): every property at any depth, as Anthropic counts them."""
    unions = optional = 0
    stack: list[Any] = [schema]
    while stack:
        node = stack.pop()
        if not isinstance(node, dict):
            continue
        required = set(node.get("required") or [])
        for name, prop in (node.get("properties") or {}).items():
            unions += is_union(prop)
            optional += name not in required
        stack += _children(node)
    return unions, optional


def keywords(schema: Any) -> set[str]:
    """Every schema keyword used (property names excluded)."""
    found: set[str] = set()
    stack: list[Any] = [schema]
    while stack:
        node = stack.pop()
        if isinstance(node, dict):
            found |= set(node)
            stack += _children(node)
    return found


def enum_with_null(schema: Any) -> bool:
    stack: list[Any] = [schema]
    while stack:
        node = stack.pop()
        if isinstance(node, dict):
            if isinstance(node.get("enum"), list) and None in node["enum"]:
                return True
            stack += _children(node)
    return False


# ---------- Anthropic ----------

def for_anthropic(schema: dict[str, Any]) -> Shape:
    """The schema within Anthropic's union and optional limits (module docstring); SchemaTooComplex otherwise."""
    _, optional = complexity(schema)
    budget = [ANTHROPIC_MAX_OPTIONAL - optional]

    def prop(node: Any, top: bool) -> tuple[Any, bool]:
        """A property's wire form, and whether it stops being required."""
        if not isinstance(node, dict):
            return node, False
        inner = _non_null(node) if nullable(node) else None
        if inner is not None and inner.get("type") == "object":
            note = " ".join(filter(None, [node.get("description"), WRAPPED_NOTE]))
            return {"type": "array", "items": walk(inner), "description": note}, False
        if inner is not None and not top and budget[0] > 0:
            budget[0] -= 1
            return walk(inner), True
        return walk(node), False

    def walk(node: Any, top: bool = False) -> Any:
        if not isinstance(node, dict):
            return node
        out = {k: v for k, v in node.items() if k not in ("properties", "required", "items", "anyOf", "$defs")}
        if "properties" in node:
            required = list(node.get("required") or [])
            props: dict[str, Any] = {}
            for name, sub in node["properties"].items():
                props[name], dropped = prop(sub, top)
                if dropped and name in required:
                    required.remove(name)
            out["properties"], out["required"] = props, required
        elif "required" in node:
            out["required"] = list(node["required"])
        if "items" in node:
            out["items"] = walk(node["items"])
        if "anyOf" in node:
            out["anyOf"] = [walk(b) for b in node["anyOf"]]
        if "$defs" in node:
            out["$defs"] = {k: walk(v) for k, v in node["$defs"].items()}
        return out

    wire = walk(copy.deepcopy(schema), top=True)
    unions, optional = complexity(wire)
    if unions > ANTHROPIC_MAX_UNIONS or optional > ANTHROPIC_MAX_OPTIONAL:
        raise SchemaTooComplex(f"this answer form has {unions} union-typed and {optional} optional parameters after "
                               f"reshaping; Anthropic allows {ANTHROPIC_MAX_UNIONS} and {ANTHROPIC_MAX_OPTIONAL}")
    return Shape(schema, wire, mapped=wire != schema)


# ---------- Gemini ----------

def for_gemini(schema: dict[str, Any]) -> Shape:
    """An enum holding null becomes anyOf [the enum without null, null]: the same values in Gemini's documented form."""
    def walk(node: Any) -> Any:
        if isinstance(node, list):
            return [walk(v) for v in node]
        if not isinstance(node, dict):
            return node
        out = dict(node)
        if "properties" in node:
            out["properties"] = {k: walk(v) for k, v in node["properties"].items()}
        if "$defs" in node:
            out["$defs"] = {k: walk(v) for k, v in node["$defs"].items()}
        for key in ("items", "anyOf", "allOf"):
            if key in node:
                out[key] = walk(node[key])
        enum = out.get("enum")
        if isinstance(enum, list) and None in enum:
            declared = out.get("type")
            kinds = [k for k in (declared if isinstance(declared, list) else [declared]) if k and k != "null"]
            if len(kinds) != 1:
                raise SchemaTooComplex("an enum holding null needs one other type for Gemini")
            rest = {k: v for k, v in out.items() if k not in ("type", "enum", "description")}
            branch = {"type": kinds[0], "enum": [v for v in enum if v is not None], **rest}
            out = {"anyOf": [branch, {"type": "null"}],
                   **({"description": out["description"]} if "description" in out else {})}
        return out
    return Shape(schema, walk(copy.deepcopy(schema)))


# ---------- the answer form stated in the instructions (the fallback, RUN-427) ----------

# How long a refused typed format is remembered before the strict form is tried again: the 24 h Anthropic documents for
# keeping a compiled grammar (https://platform.claude.com/docs/en/build-with-claude/structured-outputs; not re-read for
# this change). A technical retry interval, not a limit on the owner's work
STATED_FOR_S = 24 * 3600.0


class Refusals:
    """Which typed formats a provider refused (runtime family, model, wire schema), each for STATED_FOR_S on an
    injected monotonic clock. In memory only: a daemon start or a new FERB build tries the strict form again."""

    def __init__(self, monotonic: Callable[[], float] = time.monotonic) -> None:
        self.monotonic = monotonic
        self._at: dict[str, float] = {}

    @staticmethod
    def key(provider: str, model: str | None, wire: dict[str, Any]) -> str:
        text = json.dumps([provider, model or "", wire], sort_keys=True, separators=(",", ":"))
        return hashlib.sha256(text.encode()).hexdigest()

    def refused(self, key: str) -> bool:
        at = self._at.get(key)
        if at is not None and self.monotonic() - at >= STATED_FOR_S:
            del self._at[key]
            return False
        return at is not None

    def remember(self, key: str) -> None:
        self._at[key] = self.monotonic()


REFUSALS = Refusals()            # the daemon's one memory, shared by the API loop and the Claude Agent SDK runtime


def stated(schema: dict[str, Any]) -> str:
    """The answer form as instructions: the core schema, compact. Adapters put it after FERB's own instructions, in the
    cached prefix, so it repeats across calls."""
    return ("FERB's answer format (required): your final answer is one JSON object that matches this JSON Schema, "
            "with nothing before or after it and no code fence. Include every property; use null where the schema "
            "allows it for a value that doesn't apply.\n"
            + json.dumps(schema, ensure_ascii=False, separators=(",", ":")))


def repair_note(problem: str) -> str:
    return (f"That answer doesn't fit FERB's answer format ({problem}). Reply again with only the corrected JSON "
            "object.")


def unfenced(text: str) -> str:
    """The text inside one Markdown code fence wrapping the whole answer (```json ... ```); other text as it is."""
    inner = text.strip()
    if not (inner.startswith("```") and inner.endswith("```") and len(inner) >= 6):
        return text
    first, _, rest = inner[3:-3].partition("\n")
    return rest.strip() if first.strip() in ("", "json", "JSON") else text


# ---------- answers back to the core shape ----------

def _type_of(value: Any) -> str:
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, dict):
        return "object"
    if isinstance(value, list):
        return "array"
    if isinstance(value, str):
        return "string"
    return "number"


def _kinds(node: dict[str, Any]) -> set[str]:
    kind = node.get("type")
    kinds = set(kind) if isinstance(kind, list) else {kind} if isinstance(kind, str) else set()
    return kinds | ({"number"} if "integer" in kinds else set())


def _back(node: Any, value: Any) -> Any:
    """`value` (a provider's answer at `node` of the core schema) in the core shape."""
    if not isinstance(node, dict) or value is None:
        return value
    inner = _non_null(node) if nullable(node) else None
    if inner is not None and inner.get("type") == "object" and isinstance(value, list):
        if not value:                                  # a list standing for a nullable object (`for_anthropic`)
            return None
        return _back(inner, value[0]) if len(value) == 1 else value
    if "anyOf" in node:
        rest = _branches(node)
        match = next((b for b in rest if _type_of(value) in _kinds(b)), None)
        return _back(match, value) if match is not None else value
    if isinstance(value, dict) and "properties" in node:
        out: dict[str, Any] = {}
        for name, sub in node["properties"].items():
            if name in value:
                out[name] = _back(sub, value[name])
            elif nullable(sub):
                out[name] = None
        out.update({k: v for k, v in value.items() if k not in out})   # left for FERB's check to name
        return out
    if isinstance(value, list) and isinstance(node.get("items"), dict):
        return [_back(node["items"], v) for v in value]
    return value
