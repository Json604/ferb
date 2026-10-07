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

DEFAULT_BUDGET_TOKENS = 32768
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


@dataclass(frozen=True)
class ContextBlock:
    text: str
    reference: str
    protected: bool = False


def assemble_context(blocks: list[ContextBlock], *, budget_tokens: int = DEFAULT_BUDGET_TOKENS,
                     reserved_tokens: int = 0) -> str:
    """Reserve prompts/tools first; omit whole optional blocks, keeping retrieval references visible.

Protected source text can exceed the configured budget. Reporting that fact is preferable to changing it.
The budget is a context allocation, not a task/output/token-spend limit.
"""
    if budget_tokens <= 0 or reserved_tokens < 0:
        raise ValueError("context budget must be positive and reserve nonnegative")
    active = [block for block in blocks if block.text]
    kept = {index for index, block in enumerate(active) if block.protected}

    def render() -> str:
        parts = [block.text if index in kept else f"[Context omitted; fetch full text: {block.reference}]"
                 for index, block in enumerate(active)]
        body = "\n\n".join(parts)
        used = count_tokens(body) + reserved_tokens
        report = {"budget_tokens": budget_tokens, "reserved_tokens": reserved_tokens,
                  "content_tokens_estimate": count_tokens(body), "method": token_method(),
                  "over_budget": used > budget_tokens,
                  "omitted_blocks": len(active) - len(kept)}
        return body + "\n\nContext budget: " + json.dumps(report)

    for index, block in enumerate(active):
        if block.protected:
            continue
        kept.add(index)
        if count_tokens(render()) + reserved_tokens > budget_tokens:
            kept.remove(index)
    result = render()
    # Header/omission references also consume context. Report their unavoidable overhead truthfully.
    if count_tokens(result) + reserved_tokens > budget_tokens and '"over_budget": false' in result:
        result = result.replace('"over_budget": false', '"over_budget": true', 1)
    return result
