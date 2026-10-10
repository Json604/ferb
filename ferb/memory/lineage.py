"""Source IDs from FERB's structured readers, including paged evidence replay. Never parses arbitrary prose."""
import json
from typing import Any


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
