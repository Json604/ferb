"""Profile discovery facts and full installed browser access; the agents decide profile authorization."""
import json

from ferb.orchestrator.availability import Availability
from tests.conftest import chat, install, sup, task

def test_browser_capabilities_do_not_depend_on_profile_phrases(daemon, unattended):
    s = daemon.daemon.supervisor
    s.availability = lambda: Availability({'browser:read', 'browser:act', 'mac:read', 'fs:read'}, '')
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
