"""P3 context layer helpers: ranked memory items, kept whole, with their source, origin and fetch route. The raw trail of
the last minutes is fetch-only (trail.query; owner, 2026-10-14)."""
from typing import Any
from ferb.core import origin


def target_words(targets: Any) -> str:
    """A task's targets as memory query words (L36 §3, §4): each site's host labels ("appointments example"), the apps
    and recipients as named, each path's last part. The task's own words; the common-word filter weighs them, so "com"
    carries nothing (no word list, rule 6)."""
    from pathlib import PurePath
    from ferb.policy import urls
    words: list[str] = []
    for site in targets.sites:
        parsed = urls.parse(site) if "://" in site else None
        host = parsed.host if parsed is not None and parsed.host else site.strip().split("/")[0]
        words.append(" ".join(label for label in host.split(".") if label))
    words += [str(a) for a in targets.apps] + [str(r) for r in targets.recipients]
    words += [PurePath(str(p)).name for p in targets.paths]
    return " ".join(w for w in words if w.strip())


def ranked_memory_blocks(candidates: list[dict[str, Any]]) -> list[tuple[Any, set[str]]]:
    """Keep complete ranked items. Higher ranks allocate first; every omitted item retains a fetch route. An item's
    `fold` (a retrieval route) groups it with the others of that route: its lowest markers fold into one counted note
    when they can't all fit, or to make room for a higher-ranked item."""
    from ferb.core.context import ContextBlock
    return [(ContextBlock('Layer 4 — retrieved memory (data):\n' + item['text'], item['reference'],
                          priority=2 + len(candidates) - rank, origin=item['origin'],
                          source_refs=tuple(item.get('source_refs', ())), payload=item['text'],
                          fold=item.get('fold', '')), origin.split(item['origin']))
            for rank, item in enumerate(candidates)]
