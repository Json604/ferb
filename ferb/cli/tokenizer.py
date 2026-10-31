"""FERB's own pinned tokenizer copy (fix wave 3, 8a): `ferb setup` puts it under FERB's home, `ferb doctor` checks it.

Token counts (the planner's budget, the consolidator's split) use o200k_base. tiktoken keeps its ranks in the temporary
folder, which macOS cleans: then every count silently fell back to a byte ceiling. FERB keeps its own copy, checked
against the SHA-256 pinned in ferb/core/context.py. A verified copy already on this Mac is copied; otherwise the public
file is downloaded once and kept only if it matches the pin."""
import hashlib
import os
from pathlib import Path
from typing import Any, Callable
from urllib.request import urlopen

from ferb.core.context import TOKENIZER_SHA256, TOKENIZER_URL, tokenizer_path, verified_tokenizer


def _fetch(url: str) -> bytes:
    with urlopen(url, timeout=60) as response:
        data: bytes = response.read()
    return data


def ensure(home: Path, fetch: Callable[[str], Any] | None = None) -> str:
    """Put FERB's pinned copy under `home`, and say what happened in one line. Never raises: without it, counts keep a
    byte ceiling and `ferb doctor` says so."""
    target = tokenizer_path(home)
    try:
        if target.is_file() and hashlib.sha256(target.read_bytes()).hexdigest() == TOKENIZER_SHA256:
            return "Tokenizer: FERB's pinned copy is in place."
        data = verified_tokenizer()                       # a verified copy on this Mac (tiktoken's cache)
        how = "copied from this Mac's tiktoken cache"
        if data is None:
            data = (fetch or _fetch)(TOKENIZER_URL)
            how = "downloaded"
        if hashlib.sha256(data).hexdigest() != TOKENIZER_SHA256:
            return "Tokenizer: the downloaded file doesn't match FERB's pinned hash; nothing was kept."
        target.parent.mkdir(parents=True, exist_ok=True)
        part = target.with_suffix(".part")
        part.write_bytes(data)
        os.replace(part, target)
        return f"Tokenizer: FERB's pinned copy {how} and verified."
    except Exception as exc:
        return (f"Tokenizer: not available ({type(exc).__name__}: {exc}); token counts use a byte ceiling until "
                "FERB's next update can fetch it.")
