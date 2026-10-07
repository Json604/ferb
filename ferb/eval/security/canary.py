"""Fixed-format, non-secret canary tokens. Stable per tag within one run so tests can assert on them."""

import hashlib
import secrets

_RUN_SEED = secrets.token_hex(8)


def make(tag: str) -> str:
    return f"FERB-CANARY-{tag}-{hashlib.sha256(f'{_RUN_SEED}:{tag}'.encode()).hexdigest()[:8]}"
