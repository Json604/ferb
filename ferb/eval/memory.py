"""Owner-history evaluation data and paired answer reports. Never invent relevance labels or owner verdicts."""
from __future__ import annotations

from typing import Any



def history_queries(learning: Any) -> dict[str, Any]:
    cases = [{'task_id': r['task_id'], 'query': r['request_text'], 'relevant': [],
              'expected': '', 'origin': 'owner_history'} for r in learning.db.conn.execute(
        'SELECT task_id,request_text FROM episodes ORDER BY started_at,task_id')]
    return {'cases': cases, 'label_status': 'needs owner relevance and expected-result labels',
            'recommended_queries': 'approximately 50–100, including Hinglish, people, temporal and app queries'}
