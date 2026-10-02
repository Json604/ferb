"""Personal profiles require owner direction, never a model-selected owner identity."""
import json

from ferb.act.browser.profiles import BrowserProfile, named_in
from ferb.core.types import Identity
from ferb.orchestrator.availability import Availability
from tests.test_supervisor import chat, install, sup, task

KNOWN = [BrowserProfile('brave', 'Default', 'Work', ''),
         BrowserProfile('chrome', 'Profile 2', 'scaler', 'person@sst.scaler.com')]


def test_account_or_site_name_is_not_explicit_profile_permission():
    assert named_in('read sst.scaler.com', KNOWN, explicit_only=True) is None
    assert named_in('research scaler', KNOWN, explicit_only=True) is None
    assert named_in('use my Work profile', KNOWN, explicit_only=True).allows('brave', 'Default')
    assert named_in('use Chrome', KNOWN, explicit_only=True).browser == 'chrome'


def test_owner_task_does_not_get_personal_browser_scopes_by_default(daemon):
    s = daemon.daemon.supervisor
    s.availability = lambda: Availability({'browser:read', 'browser:act', 'mac:read', 'fs:read'}, '')
    s.environment = lambda words: {'named_browser': named_in(words, KNOWN, explicit_only=True)}
    seen = []
    def worker(token):
        seen.append(daemon.daemon.tokens.validate(token).scopes)
        return json.dumps({'status':'needs_user', 'summary':'Ready', 'artifacts':[], 'evidence':[]})
    install(daemon, [sup('task', 'Reading', task('read my documents', 'owner', paths=('notes.md',)))], {'worker':worker})
    chat(daemon, 'read my documents')
    assert not ({'browser:read', 'browser:act'} & seen[0])
    install(daemon, [sup('task', 'Reading', task('read my Work profile', 'owner'))], {'worker':worker})
    chat(daemon, 'use my Work profile')
    assert {'browser:read', 'browser:act'} <= seen[1]


def test_web_request_misclassified_as_owner_is_repaired_before_work(daemon):
    s = daemon.daemon.supervisor
    s.availability = lambda: Availability({'browser:read', 'identity:browse', 'fs:read'}, '')
    s.environment = lambda words: {'named_browser': None}
    seen = []
    def worker(token):
        seen.append(daemon.daemon.tokens.validate(token).identity)
        return json.dumps({'status':'needs_user', 'summary':'Ready', 'artifacts':[], 'evidence':[]})
    script = install(daemon, [sup('task','Opening',task('order lunch','owner',sites=('shop.example',))),
                             sup('task','Opening',task('order lunch','ferb',sites=('shop.example',)))], {'worker':worker})
    chat(daemon, 'order lunch on shop.example')
    assert seen == [Identity.FERB]
    assert len([x for x in script.contexts if x[1] == 'supervisor']) == 2


def test_mac_browser_fallback_cannot_bypass_removed_browser_scopes(daemon):
    from tests.test_gateway import FakeCap, request, setup
    cap = FakeCap('mac.open', 'open_tab', {'mac:read'}, site='com.google.chrome')
    grant, _ = setup(daemon, {'mac:read'}, request('order lunch'), [cap])
    result = daemon.run(daemon.daemon.gateway.call(grant, 'mac.open', {'effect':'navigate'}))
    assert result['is_error'] and cap.calls == 0
    assert 'personal browser' in result['content']['error'].lower()


def test_bad_prior_profile_choice_is_not_inherited_as_permission(daemon):
    s = daemon.daemon.supervisor
    s.availability = lambda: Availability({'identity:browse','browser:read','fs:read'}, '')
    s.environment = lambda words: {'named_browser': None}
    from ferb.orchestrator.schemas import TaskSpec
    from ferb.policy.targets import Request
    async def legacy():
        s.engine.requests.record(Request('old', 'order lunch on shop.example'))
        spec = TaskSpec(goal="order lunch in owner's Work profile", identity='owner', targets={'sites':['shop.example']})
        events = []
        async for event in s.run_tree(spec,'Old attempt','old',conversation_id='s1'):
            events.append(event)
        return next(e['task_id'] for e in events if e['type']=='plan')
    report = json.dumps({'status':'needs_user','summary':'Disconnected','artifacts':[],'evidence':[]})
    install(daemon, [], {'worker':report})
    prior = daemon.run(legacy())
    follow = json.loads(sup('task','Retrying',task('retry lunch','ferb',sites=('shop.example',))))
    follow['control'] = {'task_id':prior,'action':'retry'}
    seen = []
    def worker(token):
        seen.append(daemon.daemon.tokens.validate(token))
        return report
    install(daemon,[json.dumps(follow)],{'worker':worker})
    chat(daemon,'try again')
    assert seen[0].identity == Identity.FERB
    assert not ({'browser:read','browser:act'} & seen[0].scopes)
