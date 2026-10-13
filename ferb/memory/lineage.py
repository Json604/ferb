"""Source IDs from FERB's structured readers, including paged evidence replay. Never parses arbitrary prose.

L28 lineage handles: a planner turn cites a conversation entry it was shown as `entry:<journal id>` and a task as
`task:<task id>`, instead of copying everything those already cite into every reply and exposure. Where lineage becomes
a source edge (an episode card), `resolve` expands the handles transitively, so forgetting a source still reaches every
card derived from it."""
import json
from typing import Any, Callable, Iterable

ENTRY, TASK = "entry:", "task:"


def resolve(refs: Iterable[str], entry: Callable[[str], dict[str, Any] | None],
            task: Callable[[str], Iterable[str]]) -> set[str]:
    """The complete refs behind `refs`: an entry handle becomes the entry itself (`journal:<id>`) plus what it cites (a
    task answer also cites its task); a task handle becomes the task's recorded lineage. Each handle is expanded once;
    other refs are kept as they are."""
    out: set[str] = set()
    expanded: set[str] = set()
    stack = list(refs)
    while stack:
        ref = stack.pop()
        if not ref.startswith((ENTRY, TASK)):
            out.add(ref)
            continue
        if ref in expanded:
            continue
        expanded.add(ref)
        ident = ref.partition(":")[2]
        if ref.startswith(TASK):
            stack.extend(r for r in task(ident) if r not in out)
            continue
        out.add("journal:" + ident)
        data = entry(ident)
        if data is None:
            continue                     # a deleted entry keeps its own ref: a card never drops that edge
        stack.extend(str(r) for r in data.get("source_refs", []) if r not in out)
        if data.get("task_id"):
            stack.append(TASK + str(data["task_id"]))
    return out


def record_sources(record: dict[str, Any]) -> set[str]:
    inp = record.get('input') or {}
    if record.get('kind') in ('context.source', 'context.exposure'):
        return {str(ref) for ref in inp.get('source_refs', [])}
    if record.get('kind') == 'tool.call':
        return call_sources(inp, record.get('output'))
    return set()


def call_sources(inp: dict[str, Any], result: Any) -> set[str]:
    tool, args = inp.get('tool'), inp.get('args') or {}
    result = result if isinstance(result, dict) else {}
    refs: set[str] = set()
    if tool in ('memory.read', 'memory.learned'):
        prefix = 'memory:' if tool == 'memory.read' else ''
        if args.get('item_id'):
            refs.add(prefix + str(args['item_id']))
        refs.update(str(ref) for ref in result.get('source_refs', []))
        pages = [result]
        try:
            full = json.loads(result.get('text', ''))
            if isinstance(full, dict):
                pages.append(full)
        except (ValueError, TypeError):
            pass  # An incomplete JSON page retains the exact requested item ID.
        for page in pages:
            refs.update(prefix + str(e['id']) for e in page.get('items', page.get('candidates', []))
                        if isinstance(e, dict) and e.get('id'))
    elif tool == 'trail.query':
        if args.get('event_id'):
            refs.add('trail:' + str(args['event_id']))
        refs.update('trail:' + str(e['id']) for e in result.get('events', []) if e.get('id'))
    elif tool == 'trail.days':
        refs.update('journal:' + str(e['id']) for e in result.get('days', []) if e.get('id'))
        refs.update(str(ref) for ref in result.get('source_refs', []))
    elif tool == 'task.evidence':
        # This sidecar is built by TaskEvidence from authorized stored records, independent of text pagination.
        refs.update(str(ref) for ref in result.get('source_refs', []))
        for record in result.get('records', []):
            refs.update(record_sources(record))
        try:
            full = json.loads(result.get('text', ''))
            if isinstance(full, dict):
                refs.update(record_sources(full))
        except (ValueError, TypeError):
            pass
    return refs
