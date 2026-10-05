"""One owner-observed recording check in a disposable Chrome profile, through the real gateway/extension.

Run from a normal terminal: uv run --no-sync pytest -q -s -m real tests/test_recording_real.py
No installed daemon, personal browser profile, global input, camera or microphone is used.
"""
import base64
import http.server
import json
from pathlib import Path
import threading
import time

import pytest

from tests.browserkit import OWNER, task, call
from tests.chromelab import browser_lab

pytestmark = pytest.mark.real


def test_record_two_background_targets_while_owner_keeps_working(tmp_path):
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            name = 'ALPHA' if self.path.startswith('/alpha') else 'BETA'
            page = (f'<!doctype html><title>FERB recording check {name}</title>'
                    '<style>body{background:#ffdccc;font:40px system-ui;padding:40px}button{font:30px system-ui}</style>'
                    f'<h1>{name}</h1><p id="state">READY</p>'
                    '<button onclick="document.getElementById(\'state\').textContent=\'COMPLETE\';'
                    'document.body.style.background=\'#c6f8d0\'">Complete</button>').encode()
            self.send_response(200); self.send_header('Content-Type','text/html; charset=utf-8')
            self.send_header('Content-Length',str(len(page))); self.end_headers(); self.wfile.write(page)
        def log_message(self,*args): pass
    server = http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
    thread = threading.Thread(target=server.serve_forever,daemon=True); thread.start()
    url=f'http://127.0.0.1:{server.server_port}'
    input('Keep using another app during this check. Press Enter when ready; the check starts in 3 seconds. ')
    time.sleep(3)
    try:
        with browser_lab() as (d,chrome,front):
            grant=task(d,'recording-check',OWNER|{'fs:read','fs:write'},'Record the two disposable background pages.')
            def invoke(name,**args):
                result=call(d,grant,name,**args)
                assert not result['is_error'],result
                return result
            recordings=[]
            try:
                for name in ('alpha','beta'):
                    tab=invoke('browser.open',url=url+'/'+name,profile='Lab')['content']['tab']
                    invoke('browser.wait',tab=tab,until='load')
                    r=invoke('browser.record',tab=tab,path=str(tmp_path/f'{name}.mp4'),fps=2)['content']
                    recordings.append((name,tab,r['id']))
                time.sleep(2)
                for name,tab,_ in recordings:
                    snapshot=invoke('browser.snapshot',tab=tab)['content']
                    button=next(e['id'] for e in snapshot['elements'] if e['role']=='button' and e['label']=='Complete')
                    invoke('browser.act',tab=tab,element=button,action='click')
                    assert 'COMPLETE' in invoke('browser.snapshot',tab=tab)['content']['text']
                time.sleep(2)
                reports=[]
                for name,_,rid in recordings:
                    stopped=invoke('recording.stop',id=rid)['content']
                    review=invoke('recording.inspect',path=stopped['path'])
                    (tmp_path/f'{name}-review.jpg').write_bytes(base64.b64decode(review['image']['data']))
                    reports.append({'recording':stopped,'review':review['content']})
                report={'targets':reports,'browser_took_focus':front.took_focus(chrome.pid),
                        'focus_changes':front.changes,'owner_verdict':'pending'}
                (tmp_path/'result.json').write_text(json.dumps(report,indent=2))
                print(f'\nReview both contact sheets and MP4s in {tmp_path}.\n'
                      'Expected: each correct target changes READY → COMPLETE. '
                      'Report any cursor, typing-focus or desktop disturbance; this is separate from the automated result.')
                assert not report['browser_took_focus'],report
            finally:
                d.run(d.daemon._task_end('recording-check'),timeout=60)
    finally:
        server.shutdown();server.server_close();thread.join(2)
