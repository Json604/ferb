"""P3 compact recent activity without changing its source, time, or trust."""
import json
from collections import defaultdict
from datetime import datetime, timedelta
from typing import Any
from ferb.core import origin
from ferb.sense.tools import trail_sources
from ferb.store.db import Database


def recent_activity(db: Database, now: datetime) -> tuple[str, str]:
    since = (now - timedelta(minutes=10)).isoformat()
    groups: dict[str, list[dict[str, Any]]] = defaultdict(list)
    sources: set[str] = set()
    source_refs: list[str] = []
    for row in db.conn.execute('SELECT id,at,source,actor,data_json FROM trail WHERE at>=? ORDER BY at', (since,)):
        source_refs.append('trail:' + row['id'])
        data = json.loads(row['data_json'])
        app = str(data.get('app') or data.get('bundle_id') or data.get('bundle') or row['source'])
        groups[app].append({'id': row['id'], 'at': row['at'], 'source': row['source'], 'actor': row['actor'], 'data': data})
        sources.add(row['source'])
    compact = []
    for app, events in groups.items():
        # Repeated identical state transitions are counted, not mistaken for distinct actions.
        states: dict[str, dict[str, Any]] = {}
        for event in events:
            key = json.dumps({k: event[k] for k in ('source', 'actor', 'data')}, sort_keys=True)
            if key not in states:
                states[key] = {**event, 'count': 0, 'last_at': event['at']}
            states[key]['count'] += 1
            states[key]['last_at'] = event['at']
        compact.append({'app': app, 'events': list(states.values())})
    labels = {origin.from_read_source(kind, ident) for kind, ident in trail_sources(sources)} if sources else {origin.FERB}
    return json.dumps({'since': since, 'through': now.isoformat(), 'source': 'trail.query',
                       'trust': origin.join(labels), 'apps': compact, 'source_refs': source_refs}, ensure_ascii=False), origin.join(labels)


def ranked_memory_blocks(candidates: list[dict[str, Any]]) -> list[tuple[Any, set[str]]]:
    """Keep complete ranked items. Higher ranks allocate first; every omitted item retains a fetch route."""
    from ferb.core.context import ContextBlock
    return [(ContextBlock('Layer 4 — retrieved memory (data):\n' + item['text'], item['reference'],
                          priority=2 + len(candidates) - rank, origin=item['origin'],
                          source_refs=tuple(item.get('source_refs', ())), payload=item['text']), origin.split(item['origin']))
            for rank, item in enumerate(candidates)]


def recent_activity_refs(text: str) -> list[str]:
    """Read the structured IDs emitted above; no prose or content is interpreted as lineage."""
    try:
        value = json.loads(text)
    except ValueError:
        return []
    refs = value.get('source_refs', []) if isinstance(value, dict) else []
    return [ref for ref in refs if isinstance(ref, str) and ref.startswith('trail:')]


def recent_activity_payload(text: str) -> str:
    """Extract event data from our recent-activity schema, retaining unknown legacy hook text conservatively."""
    try:
        value = json.loads(text)
    except ValueError:
        return text
    if not isinstance(value, dict) or value.get('source') != 'trail.query' or not isinstance(value.get('apps'), list):
        return text
    return json.dumps([event['data'] for app in value['apps'] for event in app.get('events', [])
                       if 'data' in event], ensure_ascii=False)
