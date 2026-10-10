"""S10 code-built daily security facts, persisted with the existing daily summary. Never claims intent."""
import json


def build(db, day: str) -> dict:
    c = db.conn
    destinations = [dict(r) for r in c.execute('SELECT kind, value, how FROM known_destinations WHERE substr(at,1,10)=?', (day,))]
    confirmations = [dict(r) for r in c.execute('SELECT rule, granted, used FROM confirmations WHERE substr(at,1,10)=?', (day,))]
    blocked = []
    for row in c.execute("SELECT id, kind, input_json, output_json, error FROM spans WHERE substr(started_at,1,10)=? "
                         "AND kind IN ('tool.refused','tool.call')", (day,)):
        inp, out = json.loads(row['input_json'] or '{}'), json.loads(row['output_json'] or '{}')
        for refusal in (out.get('network') or {}).get('refused', []):
            blocked.append({'span': row['id'], 'network': refusal})
        if row['kind'] == 'tool.refused':
            blocked.append({'span': row['id'], 'tool': inp.get('tool'), 'reason': row['error']})
    memory = [dict(r) for r in c.execute("SELECT id, origin, status FROM memory_items WHERE substr(created_at,1,10)=? "
                                        "AND (origin='third_party' OR id IN (SELECT ms.item_id FROM memory_sources ms "
                                        "LEFT JOIN trail t ON ms.kind='trail' AND ms.source_id=t.id "
                                        "LEFT JOIN journal j ON ms.kind='journal' AND ms.source_id=j.id "
                                        "WHERE t.source IN ('window','browser_tab','email','message','notification') OR COALESCE(j.origin,'') LIKE '%external:%'))", (day,))]
    return {'new_destinations': destinations, 'confirmations': confirmations, 'blocked_flows': blocked,
            'outside_origin_memory': memory, 'meaning': 'recorded facts; missing observations are not proof of no activity'}
