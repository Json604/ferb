"""One owner-supervised native pointer click in a disposable Chrome profile.

Run alone: pytest -m real tests/test_mac_pointer_real.py -s. Coordinate truth comes from the receiving canvas,
not a successful injection call. Front-app/cursor samples supplement the owner's keyboard-focus/Space verdict.
Never resolves the owner's Chrome by name; only the spawned PID is targeted and closed.
"""
import asyncio
import json
import time
from urllib.parse import quote

import pytest

from ferb.act.gui_lock import GuiLock
from ferb.act.mac import apps, screenshot
from ferb.act.mac.driver import MacComputerDriver
from ferb.act.mac.input import RealInput
from tests.chromelab import ChromeLab, Frontmost, chrome_available

pytestmark = pytest.mark.real

PAGE = '''<!doctype html><title>FERB disposable pointer check - 0 clicks</title>
<style>body{margin:0}canvas{position:absolute;left:40px;top:40px;background:#cce5ff}</style>
<canvas width="300" height="200" aria-label="FERB test canvas"></canvas><script>
window.hits=[];document.querySelector('canvas').onclick=e=>{
 hits.push({x:e.offsetX,y:e.offsetY,trusted:e.isTrusted});
 document.title='FERB disposable pointer check - '+hits.length+' clicks';
};</script>'''


def test_one_native_canvas_click_at_the_exact_pixel_without_activating(tmp_path):
    if not chrome_available():
        pytest.skip('Google Chrome is needed for the isolated native pointer check')
    import Quartz
    from ferb.act.mac.window_input import load_route
    assert load_route() is not None, 'window-targeted native transport unavailable on this Mac'
    profile = tmp_path / 'chrome'
    profile.mkdir()
    chrome, front = ChromeLab(profile), Frontmost()
    try:
        target = chrome.send('Target.createTarget', {'url': 'data:text/html,' + quote(PAGE),
                             'newWindow': True, 'background': True, 'width': 800, 'height': 600})['targetId']
        session = chrome.send('Target.attachToTarget', {'targetId': target, 'flatten': True})['sessionId']
        for _ in range(50):
            if chrome.evaluate(session, 'document.readyState') == 'complete':
                break
            time.sleep(.1)
        d = MacComputerDriver('pointer-check', RealInput(lambda: False, GuiLock()))
        labapp = apps.RunningApp('com.google.Chrome', 'FERB disposable Chrome', chrome.pid, False)
        d.app = lambda _: labapp  # exact test-owned PID, even when the owner's Chrome is also running

        async def check():
            await d.snapshot('lab')
            _, shot, jpeg = await d.screenshot('lab')
            (tmp_path / 'before.jpg').write_bytes(jpeg)
            geometry = chrome.evaluate(session, '({top:outerHeight-innerHeight,left:(outerWidth-innerWidth)/2})')
            x = (60 + geometry['left']) * shot.size[0] / shot.bounds[2]
            y = (70 + geometry['top']) * shot.size[1] / shot.bounds[3]
            before_cursor = tuple(Quartz.CGEventGetLocation(Quartz.CGEventCreate(None)))
            before_front = apps.frontmost()
            on_screen = screenshot.on_screen(shot.window)
            result = await d.click_at('lab', shot.id, x, y, allow_foreground=False)
            await asyncio.sleep(.4)
            after_front = apps.frontmost()
            report = dict(ok=result.ok, detail=result.detail, hits=chrome.evaluate(session, 'window.hits'),
                          cursor_before=before_cursor,
                          cursor_after=tuple(Quartz.CGEventGetLocation(Quartz.CGEventCreate(None))),
                          front_pid_before=before_front.pid if before_front else None,
                          front_pid_after=after_front.pid if after_front else None,
                          focus_changes=front.changes, lab_pid=chrome.pid, window_on_current_space=on_screen)
            (tmp_path / 'result.json').write_text(json.dumps(report, indent=2))
            print(json.dumps(report), flush=True)
            assert result.ok, result.detail
            assert report['hits'] == [{'x': 20, 'y': 30, 'trusted': True}], report
            assert not front.took_focus(chrome.pid), report
        asyncio.run(check())
    finally:
        chrome.close()
        front.stop()
