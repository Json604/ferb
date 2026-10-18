"""Source IDs from FERB's structured readers, including paged evidence replay. Never parses arbitrary prose.

L28 lineage handles: a planner turn cites a conversation entry it was shown as `entry:<journal id>` and a task as
`task:<task id>`, instead of copying everything those already cite into every reply and exposure.

An episode card cites its task's direct sources only (memory review C1): the owner's request entry, the task's own
tool results and evidence, and the procedure it used. A handle among them stands for the entry or task itself
(`direct`), never for everything that entry or task cites: a chat reply citing old activity doesn't drag that activity
into every later card. A forget reaches the rest at forget time: nodes built on what it deletes
(`Learning.descendants`), and the tasks that were given the item, recorded on each task as its exposures cite memory
(`task_memory_refs`): their answers, records and cards are scrubbed or forgotten there (`memory/echoes.py`)."""
import json
from typing import Any

ENTRY, TASK = "entry:", "task:"
# The planner block that carries the owner's message (Layer 5), and the worker block that carries the selected
# procedure: the two context blocks a card counts as direct sources.
REQUEST_REFERENCE = "current request"
PROCEDURE_LABEL = "Selected procedure (data; check applicability and verify each step)"


def direct(ref: str) -> str:
    """A ref as a card's own source edge: a lineage handle names the entry (`journal:`) or task (`episode:`) itself."""
    if ref.startswith(ENTRY):
        return "journal:" + ref[len(ENTRY):]
    if ref.startswith(TASK):
        return "episode:" + ref[len(TASK):]
    return ref


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
