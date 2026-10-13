"""Read-only comparison of recorded baseline snapshots. Aggregates never imply owner acceptance."""
from typing import Any


METRICS = ('done_pct', 'verified_pct', 'unverified_pct', 'failed_pct', 'owner_pass_pct', 'avg_task_s',
           'tool_calls', 'tool_errors', 'owner_questions', 'cache_pct', 'avg_trace_bytes')
RATES = {'done_pct': ('done', 'requests'), 'verified_pct': ('verified', 'requests'),
         'unverified_pct': ('unverified', 'requests'), 'failed_pct': ('failed', 'requests'),
         'owner_pass_pct': ('owner_pass', 'owner_verdicts')}


def _value(snapshot: dict[str, Any], metric: str) -> float | int | None:
    if metric in RATES:
        numerator, denominator = RATES[metric]
        if numerator in snapshot and snapshot.get(denominator):
            return round(100 * snapshot[numerator] / snapshot[denominator], 2)
        return None
    value = snapshot.get(metric)
    return value if isinstance(value, (int, float)) and not isinstance(value, bool) else None


def compare_baselines(before: dict[str, Any], after: dict[str, Any]) -> dict[str, Any]:
    metrics = {}
    for metric in METRICS:
        a, b = _value(before, metric), _value(after, metric)
        metrics[metric] = {'before': a, 'after': b, 'delta': round(b - a, 2) if a is not None and b is not None else None}
    return {'schema': 1, 'acceptance': 'not inferred from aggregate metrics',
            'caveat': 'Snapshots may cover different task mixes and retained traces. All-time counts overlap; '
                      'their difference is not a matched release experiment. Percent deltas are percentage points.',
            'captured_at': {'before': before.get('captured_at'), 'after': after.get('captured_at')},
            'denominators': {name: {key: source.get(key) for key in ('requests', 'owner_verdicts')}
                             for name, source in (('before', before), ('after', after))},
            'model_runs': {'before': before.get('model_runs'), 'after': after.get('model_runs')},
            'latency_s': {'before': before.get('latency_s'), 'after': after.get('latency_s')}, 'metrics': metrics}
