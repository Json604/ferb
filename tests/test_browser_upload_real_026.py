"""Disposable Chrome upload/paging/dialog checks with the full daemon disturbance probe recording.

Only a temporary profile, local HTTP fixture and temporary upload bytes are touched. The sentinel starts before
Chrome launches and finishes after its cleanup. Observed changes are evidence, never attributed automatically.
"""
import asyncio
import hashlib
import http.server
import json
import tempfile
import threading
from pathlib import Path

import pytest

from ferb.core.types import BrowserAction
from ferb.daemon.disturbance import DesktopProbe, DisturbanceSentinel
from tests.chromelab import browser_lab

pytestmark = pytest.mark.real


def test_direct_upload_top_same_origin_and_oopif_with_full_receipts():
    events=[]
    task_id='disposable-browser-upload-026'
    report=Path('/private/tmp/ferb-026-browser-live.json')

    async def run():
        loop=asyncio.get_running_loop()
        sentinel=DisturbanceSentinel(DesktopProbe(),lambda task,event,facts:events.append({'task':task,'event':event,**facts}))
        await sentinel.start(task_id)
        def action(name,fn):
            asyncio.run_coroutine_threadsafe(sentinel.before_action(task_id,name,name,{}),loop).result()
            try: return fn()
            finally: asyncio.run_coroutine_threadsafe(sentinel.after_action(name),loop).result()
        def exercise():
            class Handler(http.server.BaseHTTPRequestHandler):
                def do_GET(self):
                    self.send_response(200);self.send_header('Content-Type','text/html');self.end_headers()
                    if self.path == '/':
                        html=f'''<title>Disposable upload lab</title><input type=file aria-label="Top upload">
                        <iframe src="/child"></iframe><iframe src="http://localhost:{self.server.server_port}/child"></iframe>
                        <button onclick="document.getElementById('answer').textContent='answer='+confirm('Fixture choice?')">Fixture dialog</button><p id=answer></p>
                        <p>{'full source paragraph ' * 1000}TAIL_EVIDENCE</p>'''
                    else:
                        html='<input type=file aria-label="Child upload">'
                    self.wfile.write(html.encode())
                def log_message(self,*args): pass
            server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
            threading.Thread(target=server.serve_forever,daemon=True).start()
            try:
                with tempfile.TemporaryDirectory(prefix='ferb-upload-026-') as temp:
                    source=Path(temp)/'assignment.pdf';source.write_bytes(b'Capital budgeting: disposable fixture only')
                    with browser_lab(headless=True) as (daemon,chrome,front):
                        # Provision only this headless fixture; ordinary cold opens must return choices.
                        cold=chrome.evaluate(chrome.sw, "commands.open({url:'about:blank'},'cold-probe')")
                        assert cold['opened'] is False and cold['reason']=='window_selection_required'
                        window=chrome.evaluate(chrome.sw, "ferbWindow(true)")
                        driver=daemon.daemon.router
                        async def uploads():
                            tab=await driver.open_for(f'http://127.0.0.1:{server.server_port}/',task_id,'Lab',window_id=int(window))
                            try:
                                snap=await driver.snapshot(tab)
                                targets=[e for e in snap.elements if e.input_type == 'file']
                                assert len(targets) == 3, [(e.label,e.frame_id,e.input_type) for e in snap.elements]
                                seen=[]
                                for target in targets:
                                    count=len((await driver.dialog(tab)).get('file_choosers', []))
                                    clicked=await driver.act(driver.element_ref(tab,target.id),BrowserAction('click'),'')
                                    assert clicked.ok,clicked
                                    for _ in range(40):
                                        choosers=(await driver.dialog(tab)).get('file_choosers', [])
                                        if len(choosers)>count: break
                                        await asyncio.sleep(.1)
                                    assert len(choosers)==count+1,choosers
                                    chooser=choosers[-1]
                                    assert chooser['native_picker_shown'] is False and chooser['transferred'] is False
                                    result=await driver.act(driver.element_ref(tab,target.id),BrowserAction('upload',files=(str(source),)),'')
                                    assert result.ok,result
                                    assert result.facts['bytes_match'] is True,result.facts
                                    assert result.facts['files'][0]['sha256'] == hashlib.sha256(source.read_bytes()).hexdigest()
                                    seen.append({'frame':target.frame_id,'bytes_match':result.facts['bytes_match'],
                                                 'native_chooser_intercepted':True,'chooser_mode':chooser['mode']})
                                full='';offset=0
                                while offset is not None:
                                    page=await driver.read(tab,offset,7000);full+=page['text'];offset=page['next_offset']
                                assert 'TAIL_EVIDENCE' in json.loads(full)['text']
                                snap=await driver.snapshot(tab)
                                buttons=[e for e in snap.elements if e.label == 'Fixture dialog']
                                assert len(buttons) == 1
                                click=asyncio.create_task(driver.act(driver.element_ref(tab,buttons[0].id),BrowserAction('click'),''))
                                for _ in range(40):
                                    pending=await driver.dialog(tab)
                                    if pending.get('pending'): break
                                    await asyncio.sleep(.1)
                                assert pending['pending']['message'] == 'Fixture choice?'
                                assert (await driver.dialog(tab,'dismiss'))['handled'] == 'dismissed'
                                assert (await click).ok
                                assert 'answer=false' in (await driver.snapshot(tab)).text
                                return {'uploads':seen,'full_snapshot_tail':True,'unexpected_dialog':'pending until explicit dismissal'}
                            finally:
                                await driver.close(tab)
                        return action('browser.upload.three_frames',lambda:daemon.run(uploads(),timeout=90))
            finally:
                server.shutdown()
        try:
            result=await asyncio.to_thread(lambda:action('browser.disposable_lab',exercise))
            return result
        finally:
            await sentinel.finish(task_id)
            report.write_text(json.dumps({'events':events},indent=2))
    result=asyncio.run(run())
    data=json.loads(report.read_text());data['result']=result;report.write_text(json.dumps(data,indent=2))
