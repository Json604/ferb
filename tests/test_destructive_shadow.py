import asyncio
from types import SimpleNamespace
from ferb.decide.decider import Ask, ShadowDecider
from ferb.eval.destructive import measure


def test_destructive_question_cannot_be_enabled_by_config():
    jobs = []
    d = ShadowDecider(None, None, None, None, lambda x: x, enabled=lambda: {'action.destructive'})
    d._enqueue = jobs.append
    assert asyncio.run(d.ask({'kind': 'pay'}, [Ask('action.destructive', 'destructive')])) == {}
    assert len(jobs) == 1 and jobs[0].mode == 'shadow'


def test_labelled_accuracy_reports_missing_answers_honestly():
    result = measure({'delete': 'destructive', 'read': 'safe'}, {})
    assert result['answered'] == 0 and result['caught'] == 0
