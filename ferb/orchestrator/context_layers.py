"""P3 context layer helpers: ranked memory items, kept whole, with their source, origin and fetch route. The raw trail of
the last minutes is fetch-only (trail.query; owner, 2026-10-14)."""
from typing import Any
from ferb.core import origin


def ranked_memory_blocks(candidates: list[dict[str, Any]]) -> list[tuple[Any, set[str]]]:
    """Keep complete ranked items. Higher ranks allocate first; every omitted item retains a fetch route."""
    from ferb.core.context import ContextBlock
    return [(ContextBlock('Layer 4 — retrieved memory (data):\n' + item['text'], item['reference'],
                          priority=2 + len(candidates) - rank, origin=item['origin'],
                          source_refs=tuple(item.get('source_refs', ())), payload=item['text']), origin.split(item['origin']))
            for rank, item in enumerate(candidates)]
