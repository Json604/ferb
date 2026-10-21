"""P3 context layer helpers: ranked memory items, kept whole, with their source, origin and fetch route. The raw trail of
the last minutes is fetch-only (trail.query; owner, 2026-10-14)."""
from typing import Any
from ferb.core import origin

# L29: the fold route of the planner's learned-procedure lines (an item's `fold`): what lists them all
PROCEDURES_ROUTE = 'memory.learned kind=procedure'
# L29: the fold route of the verified earlier runs a worker gets with its procedure (each read in full by its card's id)
PROCEDURE_RUNS_ROUTE = "memory.learned kind=episode, the verified earlier runs of this procedure's kind of task"


def ranked_memory_blocks(candidates: list[dict[str, Any]]) -> list[tuple[Any, set[str]]]:
    """Keep complete ranked items. Higher ranks allocate first; every omitted item retains a fetch route. An item's
    `fold` (a retrieval route) groups it with the others of that route: markers that can't all fit fold into one
    counted note."""
    from ferb.core.context import ContextBlock
    return [(ContextBlock('Layer 4 — retrieved memory (data):\n' + item['text'], item['reference'],
                          priority=2 + len(candidates) - rank, origin=item['origin'],
                          source_refs=tuple(item.get('source_refs', ())), payload=item['text'],
                          fold=item.get('fold', '')), origin.split(item['origin']))
            for rank, item in enumerate(candidates)]
