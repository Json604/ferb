"""Owns token estimates and recoverable context assembly. Must not infer intent, choose a target or claim semantic success from transport facts.

Budget complete context blocks and expose omissions; never shorten requests or generated artifacts.

Counts use the cached o200k_base tokenizer as an estimate, not a claim about every provider's tokenizer.
Loading is local only. A missing tokenizer/cache uses a conservative UTF-8 byte ceiling and says so."""

import base64
import hashlib
import json
import os
import tempfile
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import Any

from ferb.core.errors import ModelLimitsUnknown
from ferb.core.types import ModelLimits

_ENCODING_URL = "https://openaipublic.blob.core.windows.net/encodings/o200k_base.tiktoken"
_ENCODING_SHA256 = "446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d"
# o200k_base's public tokenizer pattern (OpenAI tiktoken, MIT); ranks remain in its verified local cache.
_PATTERN = "|".join([
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]*[\p{Ll}\p{Lm}\p{Lo}\p{M}]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}]+[\p{Ll}\p{Lm}\p{Lo}\p{M}]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?",
    r"\p{N}{1,3}", r" ?[^\s\p{L}\p{N}]+[\r\n/]*", r"\s*[\r\n]+", r"\s+(?!\S)", r"\s+",
])


@lru_cache(maxsize=1)
def _encoding() -> Any:
    try:
        import tiktoken
        directory = os.environ.get("TIKTOKEN_CACHE_DIR", os.environ.get("DATA_GYM_CACHE_DIR",
                                    str(Path(tempfile.gettempdir()) / "data-gym-cache")))
        path = Path(directory) / hashlib.sha1(_ENCODING_URL.encode()).hexdigest()
        data = path.read_bytes()
        if hashlib.sha256(data).hexdigest() != _ENCODING_SHA256:
            return None
        ranks = {base64.b64decode(token): int(rank) for token, rank in
                 (line.split() for line in data.splitlines() if line)}
        return tiktoken.Encoding(name="o200k_base", pat_str=_PATTERN, mergeable_ranks=ranks,
                                 special_tokens={"<|endoftext|>": 199999, "<|endofprompt|>": 200018})
    except (ImportError, OSError, ValueError):
        return None


def token_method() -> str:
    return "o200k_base_chunked8192_estimate_provider_may_differ" if _encoding() is not None else "utf8_byte_ceiling_tokenizer_unavailable"


def count_tokens(text: str) -> int:
    encoding = _encoding()
    if encoding is None:
        return len(text.encode("utf-8"))
    # Bound tokenizer work on huge unbroken source strings without dropping a character.
    # Chunk boundaries make this an explicit estimate even for a provider using o200k_base.
    return sum(len(encoding.encode_ordinary(text[start:start + 8192]))
               for start in range(0, len(text), 8192))


def runtime_limits(runtime: Any) -> ModelLimits:
    """A runtime's documented model limits (L28): the context budget is their window minus room for the reply. A runtime
    that reports none raises ModelLimitsUnknown; FERB never substitutes a number of its own."""
    limits = getattr(runtime, "limits", None)
    if not isinstance(limits, ModelLimits):
        raise ModelLimitsUnknown(f"the {getattr(runtime, 'name', 'model')} runtime doesn't report its model's "
                                 "documented context window")
    return limits


@dataclass(frozen=True)
class ContextBlock:
    text: str
    reference: str
    protected: bool = False  # always included, whatever the budget (the situation, the request)
    cache: bool = False      # repeats across this role's calls; the cacheable prefix ends after the last leading one (P20)
    priority: int = 0        # optional layer allocation; original presentation order is preserved
    origin: str = ""          # supplied by the source, never inferred from model-written text
    source_refs: tuple[str, ...] = ()
    payload: str | None = None  # source data without presentation/catalogue metadata; None preserves legacy text
    history: bool = False    # one conversation entry: filled newest-first after the protected blocks, never with gaps


@dataclass(frozen=True)
class Assembly:
    parts: list[str]                     # the context in order, the budget report last
    cacheable: int                       # leading parts that repeat across this role's calls
    shown: tuple[bool, ...]              # per non-empty block, in order: handed to the model in full
    omitted_history: int                 # older conversation entries that did not fit


def assemble(blocks: list[ContextBlock], *, budget_tokens: int, reserved_tokens: int = 0) -> Assembly:
    """Reserve prompts/tools first; then every protected block; then the conversation entries newest-first while they
    fit, the older ones reported in one omission note; then optional blocks by priority (omitted whole, their retrieval
    references kept visible). Each block is counted once (L28); the report counts the final body once.

Protected source text can exceed the budget. Reporting that fact is preferable to changing it.
The budget is a context allocation (the model's window minus room for its reply), not a task/output/token-spend limit.
"""
    if budget_tokens <= 0 or reserved_tokens < 0:
        raise ValueError("context budget must be positive and reserve nonnegative")
    active = [block for block in blocks if block.text]
    omission = [f"[Context omitted; fetch full text: {block.reference}]" for block in active]
    history = [index for index, block in enumerate(active) if block.history and not block.protected]
    kept = {index for index, block in enumerate(active) if block.protected}
    separator = count_tokens("\n\n")
    cost = {index: count_tokens(active[index].text) for index in range(len(active))
            if index in kept or index in history}
    gap = {index: count_tokens(omission[index]) for index in range(len(active))
           if index not in kept and index not in history}
    # the report and a history note are small; they are counted into the decision as their longest form
    note_cost = count_tokens(_note(len(history), "chat.history")) + separator if history else 0
    report_cost = count_tokens(_report(budget_tokens, reserved_tokens, 10 ** 9, True, len(active))) + separator
    used = (reserved_tokens + report_cost + note_cost + sum(cost[index] + separator for index in kept) +
            sum(gap[index] + separator for index in gap))
    for index in reversed(history):
        if used + cost[index] + separator > budget_tokens:
            break
        kept.add(index)
        used += cost[index] + separator
    for index, block in sorted(((i, active[i]) for i in gap), key=lambda pair: -pair[1].priority):
        extra = count_tokens(block.text) - gap[index]
        if used + extra <= budget_tokens:
            kept.add(index)
            used += extra
    omitted_history = sum(1 for index in history if index not in kept)
    parts: list[str] = []
    repeats: list[bool] = []             # per part: repeats across this role's calls
    noted = False
    for index, block in enumerate(active):
        if index in kept:
            parts.append(block.text)
            repeats.append(block.cache)
        elif index in history:
            if not noted:                # the older entries collapse into one note where the first one stood
                parts.append(_note(omitted_history, block.reference.split(" ", 1)[0]))
                repeats.append(block.cache)
                noted = True
        else:
            parts.append(omission[index])
            repeats.append(False)        # an omitted block ends the cacheable prefix
    parts = parts or [""]
    body = "\n\n".join(parts)
    content = count_tokens(body)
    over = content + reserved_tokens > budget_tokens
    report = _report(budget_tokens, reserved_tokens, content, over, len(active) - len(kept))
    # Header/omission references also consume context. Report their unavoidable overhead truthfully.
    if not over and content + count_tokens(report) + separator + reserved_tokens > budget_tokens:
        report = _report(budget_tokens, reserved_tokens, content, True, len(active) - len(kept))
    parts.append(report)
    cacheable = 0
    for repeat in repeats:
        if not repeat:
            break
        cacheable += 1
    return Assembly(parts, cacheable, tuple(index in kept for index in range(len(active))), omitted_history)


def _note(count: int, reference: str) -> str:
    return f"[{count} older entries not shown here; fetch full text: {reference}]"


def _report(budget: int, reserved: int, content: int, over: bool, omitted: int) -> str:
    return "Context budget: " + json.dumps({"budget_tokens": budget, "reserved_tokens": reserved,
                                            "content_tokens_estimate": content, "method": token_method(),
                                            "over_budget": over, "omitted_blocks": omitted})


def overhead_tokens(budget_tokens: int, reserved_tokens: int = 0) -> int:
    """What `assemble` adds around one protected block at most (its budget report and separator): what a caller that
    splits work to fit a window keeps free beside its own text."""
    return count_tokens(_report(budget_tokens, reserved_tokens, 10 ** 9, True, 1)) + 2 * count_tokens("\n\n")


def assemble_parts(blocks: list[ContextBlock], *, budget_tokens: int,
                   reserved_tokens: int = 0) -> tuple[list[str], int]:
    """`assemble` as (ordered parts with the budget report last, how many leading parts repeat across calls)."""
    out = assemble(blocks, budget_tokens=budget_tokens, reserved_tokens=reserved_tokens)
    return out.parts, out.cacheable


def assemble_context(blocks: list[ContextBlock], *, budget_tokens: int, reserved_tokens: int = 0) -> str:
    """The assembled context as one text (`assemble_parts` joined)."""
    return "\n\n".join(assemble_parts(blocks, budget_tokens=budget_tokens, reserved_tokens=reserved_tokens)[0])
