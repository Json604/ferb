"""Profile discovery facts and full installed browser access; the agents decide profile authorization."""
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


def test_browser_capabilities_do_not_depend_on_profile_phrases(daemon):
    s = daemon.daemon.supervisor
    s.availability = lambda: Availability({'browser:read', 'browser:act', 'mac:read', 'fs:read'}, '')
    s.environment = lambda words: {'named_browser': named_in(words, KNOWN, explicit_only=True)}
    seen = []
    def worker(token):
        seen.append(daemon.daemon.tokens.validate(token).scopes)
        return json.dumps({'status':'needs_user', 'summary':'Ready', 'artifacts':[], 'evidence':[]})
    install(daemon, [sup('task', 'Reading', task('read my documents', 'owner', paths=('notes.md',)))], {'worker':worker})
    chat(daemon, 'read my documents')
    assert {'browser:read', 'browser:act'} <= seen[0]
    install(daemon, [sup('task', 'Reading', task('read my Work profile', 'owner'))], {'worker':worker})
    chat(daemon, 'use my Work profile')
    assert {'browser:read', 'browser:act'} <= seen[1]
