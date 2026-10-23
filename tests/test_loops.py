"""L31: loops — a plan step that repeats a checked worker round, with a code-run probe between rounds."""
import json

import pytest

from ferb.orchestrator.plans import Plan, Step, problems
from ferb.orchestrator.schemas import STEP_SCHEMA


def test_a_loop_step_is_plan_vocabulary():
    step = Step(id='keep', kind='loop', goal='Keep the tab playing', end_when='')
    assert step.kind == 'loop' and step.end_when == ''
    assert 'loop' in STEP_SCHEMA['properties']['kind']['enum']
    assert STEP_SCHEMA['properties']['end_when'] == {'type': 'string'}
    assert 'end_when' in STEP_SCHEMA['required']


def test_a_loop_needs_a_goal_and_keeps_until_for_waits():
    errors = problems(Plan(steps=[Step(id='keep', kind='loop', goal=' ')]), [], context_tokens=1000)
    assert 'keep: a loop needs a goal' in errors
    errors = problems(Plan(steps=[Step(id='keep', kind='loop', goal='Keep it', until='2026-10-25T09:00:00+05:30')]),
                      [], context_tokens=1000)
    assert "keep: a loop's end goes in end_when; until is a wait's time" in errors


def test_a_step_cannot_wait_on_a_loop_that_only_the_owner_ends():
    plan = Plan(steps=[Step(id='keep', kind='loop', goal='Keep it playing'),
                       Step(id='after', kind='do', goal='Report', depends_on=['keep'])])
    assert ("after: depends on loop keep, which runs until the owner says stop, so it could never start"
            in problems(plan, [], context_tokens=1000))
    plan.steps[0].end_when = 'the checks pass'
    assert problems(plan, [], context_tokens=1000) == []


from pydantic import BaseModel

from ferb.orchestrator.probes import MISSING, PROBE_SCHEMA, Probe, field_value, refusal, same, tripped


class _Cap:
    def __init__(self, observation=True, probe_safe=True, scopes=frozenset(), identities=('ferb',)):
        self.observation, self.probe_safe, self.scopes, self.identities = observation, probe_safe, set(scopes), set(identities)

    class Input(BaseModel):
        tab: int = 0


def _probe(**k):
    return Probe(**({'tool': 'browser.snapshot', 'args_json': '{"tab": 3}', 'field': 'media.0.playing',
                     'op': 'equals', 'value_json': 'false'} | k))


def test_a_probe_must_be_a_probe_safe_observation_in_this_task():
    assert refusal(_probe(), _Cap(), {'browser'}, 'ferb') is None
    assert 'not a FERB tool' in refusal(_probe(), None, set(), 'ferb')
    assert "can't be a probe" in refusal(_probe(), _Cap(observation=False), set(), 'ferb')
    assert "can't be a probe" in refusal(_probe(), _Cap(probe_safe=False), set(), 'ferb')
    assert 'outside this task' in refusal(_probe(), _Cap(scopes={'mac'}), {'browser'}, 'ferb')
    assert 'outside this task' in refusal(_probe(), _Cap(identities=('owner',)), set(), 'ferb')
    assert 'invalid probe' in refusal(_probe(args_json='{"tab": "x"}'), _Cap(), set(), 'ferb')
    assert 'invalid probe' in refusal(_probe(args_json='[1]'), _Cap(), set(), 'ferb')
    assert 'invalid probe' in refusal(_probe(value_json='nope'), _Cap(), set(), 'ferb')
    assert 'needs a field' in refusal(_probe(field=' '), _Cap(), set(), 'ferb')


def test_field_value_walks_keys_and_list_indexes():
    content = {'media': [{'playing': False}], 'n': 0}
    assert field_value(content, 'media.0.playing') is False
    assert field_value(content, 'n') == 0
    assert field_value(content, 'media.1.playing') is MISSING
    assert field_value(content, 'media.x') is MISSING
    assert field_value(content, 'nope') is MISSING


def test_comparison_is_exact_json():
    assert same(True, True) and not same(True, 1) and not same('false', False) and same({'a': 1, 'b': 2}, {'b': 2, 'a': 1})
    assert tripped(_probe(), False, None)
    assert not tripped(_probe(), True, None)
    assert not tripped(_probe(), 'false', None)                  # a string is not the JSON false
    assert tripped(_probe(op='not_equals', value_json='true'), False, None)
    assert tripped(_probe(op='changed'), 5, 4) and not tripped(_probe(op='changed'), 4, 4)
    assert tripped(_probe(), MISSING, None)                      # a missing field always trips


def test_probe_schema_is_strict():
    assert set(PROBE_SCHEMA['required']) == {'tool', 'args_json', 'field', 'op', 'value_json'}
    assert PROBE_SCHEMA['additionalProperties'] is False
    assert PROBE_SCHEMA['properties']['op']['enum'] == ['equals', 'not_equals', 'changed']


def test_only_reading_tools_that_move_nothing_are_probe_safe():
    from ferb.act.browser import tools as browser
    from ferb.act.fs import files
    from ferb.act.mac import tools as mac
    from ferb.act.shell import tools as shell
    safe = [browser.BrowserFind, browser.BrowserRead, browser.BrowserTabs, browser.BrowserSnapshot,
            shell.ShellRead, shell.ShellList, files.FsList, files.FsRead, mac.MacSnapshot]
    for cls in safe:
        assert getattr(cls, 'observation', False) and getattr(cls, 'probe_safe', False), cls.__name__
    for cls in [browser.BrowserScroll, browser.BrowserScreenshot, browser.BrowserPoint, browser.BrowserWait,
                browser.BrowserOpen, shell.ShellWait, mac.MacScreenshot]:
        assert not getattr(cls, 'probe_safe', False), cls.__name__
