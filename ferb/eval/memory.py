"""Owner-history evaluation data and paired answer reports. Never invent relevance labels or owner verdicts."""
from __future__ import annotations

from typing import Any

from ferb.memory.learning import LearningRefused, digest, encoded


def history_queries(learning: Any) -> dict[str, Any]:
    cases = [{'task_id': r['task_id'], 'query': r['request_text'], 'relevant': [],
              'expected': '', 'origin': 'owner_history'} for r in learning.db.conn.execute(
        'SELECT task_id,request_text FROM episodes ORDER BY started_at,task_id')]
    return {'cases': cases, 'label_status': 'needs owner relevance and expected-result labels',
            'recommended_queries': 'approximately 50–100, including Hinglish, people, temporal and app queries'}


def paired_results(cases: list[dict[str, Any]], without: list[dict[str, Any]],
                   with_memory: list[dict[str, Any]]) -> dict[str, Any]:
    """Compare actual supervised runs. Each result cites task_id, verified, owner_verdict, seconds and cost_usd.
    Missing verdicts remain pending; success is never inferred from an answer's prose.
    """
    by_mode = [{r['case_id']: r for r in rows} for rows in (without, with_memory)]
    rows = []
    for case in cases:
        ident = case.get('id', case.get('task_id'))
        if ident not in by_mode[0] or ident not in by_mode[1]:
            raise LearningRefused('paired evaluation requires both modes for every case')
        pair = [m[ident] for m in by_mode]
        if any(not r.get('task_id') for r in pair):
            raise LearningRefused('every result needs its actual task trace ID')
        rows.append({'case_id': ident, 'without': pair[0], 'with': pair[1]})
    def stats(mode: str) -> dict[str, Any]:
        r = [p[mode] for p in rows]
        judged = [x for x in r if x.get('owner_verdict') in ('pass','fail')]
        return {'runs': len(r), 'owner_judged': len(judged),
                'owner_passes': sum(x.get('owner_verdict') == 'pass' for x in judged),
                'verified': sum(x.get('verified') is True for x in r),
                'seconds': sum(float(x.get('seconds') or 0) for x in r),
                'cost_usd': sum(float(x.get('cost_usd') or 0) for x in r)}
    return {'dataset_hash': digest(cases), 'paired': rows, 'without': stats('without'), 'with': stats('with'),
            'acceptance': 'pending' if any(r[m].get('owner_verdict') not in ('pass','fail')
                                         for r in rows for m in ('without','with')) else 'owner judged'}
