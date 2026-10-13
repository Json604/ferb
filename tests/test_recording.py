"""Target recordings: real encoding/decoding, isolated driver routes, and lifecycle ownership."""
import asyncio
import json
import shutil
import subprocess
from pathlib import Path
from types import SimpleNamespace

import pytest

from ferb.core.errors import Refused
from tests.chromelab import worker


@pytest.fixture
def colours():
    ffmpeg = shutil.which('ffmpeg')
    if not ffmpeg:
        pytest.skip('recording requires ffmpeg')
    return [subprocess.check_output([ffmpeg, '-v', 'error', '-f', 'lavfi', '-i',
            f'color={colour}:s=320x180', '-frames:v', '1', '-f', 'image2pipe', '-c:v', 'mjpeg', 'pipe:1'])
            for colour in ('red', 'blue')]


def test_saved_video_contains_target_frames_and_real_timing(tmp_path, colours):
    from ferb.act.recording import RecordingManager, inspect_video
    async def check():
        clock = [0.0]
        current = [colours[0]]
        async def capture(): return current[0]
        m = RecordingManager(now=lambda: clock[0], sanitize=lambda image: image)
        path = tmp_path / 'target.mp4'
        started = await m.start('task', {'kind':'tab', 'tab_id':7, 'profile':'FERB'}, path, capture, fps=1)
        clock[0] = 1; current[0] = colours[1]
        await m.sample(started['id'], 'task')
        clock[0] = 2
        stopped = await m.stop(started['id'], 'task')
        assert stopped['capture_complete'] and stopped['frames'] == 2
        assert stopped['content_reviewed'] is False
        assert not list(tmp_path.glob('.ferb-recording-*'))
        info, image = await inspect_video(path, [0.2, 1.2])
        assert 1.9 <= info['duration_s'] <= 2.2
        assert info['sample_times_s'] == [0.2, 1.2] and image.startswith(b'\xff\xd8')
        # Decode the actual review image: first panel is red, second is blue. No source-frame shortcut.
        rgb = subprocess.check_output([shutil.which('ffmpeg'), '-v', 'error', '-f', 'image2pipe', '-i', 'pipe:0',
              '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1'], input=image)
        width = info['review_width']
        def pixel(x, y): return rgb[(y * width + x)*3:(y * width + x)*3+3]
        red, blue = pixel(100,100), pixel(width//2+100,100)
        assert red[0] > red[2] + 100 and blue[2] > blue[0] + 100
        receipt = json.loads(Path(stopped['receipt']).read_text())
        assert receipt['target']['tab_id'] == 7 and receipt['capture_complete']
        assert receipt['sample_times_s'] == [0,1]
    asyncio.run(check())


def test_recording_failure_is_partial_and_cleanup_is_owned(tmp_path, colours):
    from ferb.act.recording import RecordingManager
    async def check():
        failed = [False]
        async def capture():
            if failed[0]: raise Refused('target closed')
            return colours[0]
        m = RecordingManager(sanitize=lambda image: image)
        r = await m.start('first', {'kind':'window','window_id':110}, tmp_path/'partial.mp4', capture, fps=1)
        with pytest.raises(Refused, match='another task'):
            await m.stop(r['id'], 'second')
        assert await m.end_task('second') == []
        failed[0] = True
        await m.sample(r['id'], 'first')
        result = (await m.end_task('first'))[0]
        assert result['released'] and not result['capture_complete']
        assert 'target closed' in result['error'] and result['frames'] == 1
        assert Path(result['path']).exists()
        assert await m.end_task('first') == []
    asyncio.run(check())


def test_recording_never_overwrites_and_first_failure_leaves_no_capture(tmp_path, colours):
    from ferb.act.recording import RecordingManager
    async def check():
        p = tmp_path/'existing.mp4'; p.write_bytes(b'owner video')
        async def capture(): raise AssertionError('should not capture')
        m = RecordingManager(sanitize=lambda image: image)
        with pytest.raises(Refused, match='exists'):
            await m.start('task', {}, p, capture)
        assert p.read_bytes() == b'owner video'
        async def unavailable(): raise Refused('background capture unavailable')
        with pytest.raises(Refused, match='unavailable'):
            await m.start('task', {}, tmp_path/'new.mp4', unavailable)
        assert sorted(x.name for x in tmp_path.iterdir()) == ['existing.mp4']
    asyncio.run(check())


def test_window_recording_stays_on_observed_window_without_invalidating_pointer(monkeypatch):
    from tests.mackit import vision
    from ferb.act.mac.screenshot import Window
    d, ax, shots, apps, posted = vision(monkeypatch)
    async def check():
        _, shot, _ = await d.screenshot('Notes')
        target, capture = d.recording_source('Notes', shot.id)
        shots.front_window = lambda *a: Window(999, 11, shots.bounds, 'other')
        assert await capture() == shots.jpeg
        assert shots.captured == [110, 110] and target['window_id'] == 110
        assert d._state['com.apple.Notes'].shot is shot
        assert not apps.activated and not posted
        shots.window = lambda wid: Window(wid, 12, shots.bounds, 'reassigned')
        with pytest.raises(Refused, match='window'):
            await capture()
        assert shots.captured == [110, 110]
    asyncio.run(check())


def test_window_recording_withholds_a_frame_whose_new_password_field_has_no_box(monkeypatch):
    """L13: a field that appears while capturing is painted over from the walk after it; one with no box to paint
    withholds that frame (tests/test_pixels_to_model.py: frames are masked, and the recording goes on)."""
    from ferb.core.errors import Withheld
    from tests.mackit import vision
    d, ax, shots, *_ = vision(monkeypatch)
    async def check():
        _, shot, _ = await d.screenshot('Notes')
        _, capture = d.recording_source('Notes', shot.id)
        ax.secure = [False, True]
        with pytest.raises(Withheld, match='inventory'):
            await capture()
    asyncio.run(check())


@pytest.mark.parametrize('unknown_after_capture', [False, True])
def test_window_recording_withholds_unknown_secret_inventory(monkeypatch, unknown_after_capture):
    from tests.mackit import vision
    d, ax, shots, *_ = vision(monkeypatch)
    async def check():
        _, shot, _ = await d.screenshot('Notes')
        _, capture = d.recording_source('Notes', shot.id)
        ax.secure = [False, None] if unknown_after_capture else [None]
        with pytest.raises(Refused, match='inventory'):
            await capture()
        assert len(shots.captured) == (2 if unknown_after_capture else 1)
    asyncio.run(check())


def test_extension_record_frame_preserves_action_references_and_never_activates():
    out = worker('''(async () => {
      attach = async id => tabState(id);
      maskState = async id => ({documents: [[0, 'doc']], rects: [], viewport: [100, 50]});
      let calls = []; cdp = async (id, method, args) => {calls.push({id,method,args}); return {data:'jpeg'};};
      const st = tabState(19); st.visual = {snap:7}; st.shot = 'other screenshot';
      const frame = await commands.record_frame({tab_id:19});
      return {frame,calls,visual:st.visual,shot:st.shot};
    })()''')
    assert out['frame'] == {'data': 'jpeg', 'mask': {'rects': [], 'viewport': [100, 50]}}
    assert out['visual'] == {'snap':7} and out['shot'] == 'other screenshot'
    assert out['calls'] == [{'id':19, 'method':'Page.captureScreenshot',
                            'args':{'format':'jpeg','quality':70,'fromSurface':True,'captureBeyondViewport':False}}]


def test_cleanup_stops_recording_before_releasing_browser():
    from ferb.daemon.main import Daemon
    order=[]
    async def finish(task): order.append('recording'); return [{'resource':'recording','released':True}]
    async def release(task): order.append('browser'); return []
    async def unheard(task): return []                 # no listen in progress (L26 listens end first)
    host=SimpleNamespace(recordings=SimpleNamespace(end_task=finish),identity_browser=SimpleNamespace(release=release),
                         router=None,shells=None,hearing=SimpleNamespace(end_task=unheard))
    assert asyncio.run(Daemon._task_end(host,'task')) == [{'resource':'recording','released':True}]
    assert order == ['recording','browser']


def test_recording_finds_installed_encoder_under_launchd_path(monkeypatch):
    from ferb.act.recording import executable
    installed = next((p for p in (Path('/opt/homebrew/bin/ffmpeg'),Path('/usr/local/bin/ffmpeg')) if p.is_file()),None)
    if installed is None: pytest.skip('Homebrew ffmpeg not installed')
    monkeypatch.setenv('PATH','/usr/bin:/bin:/usr/sbin:/sbin')
    assert Path(executable('ffmpeg')) == installed


def test_browser_recording_source_preserves_refs_and_checks_ownership():
    from ferb.act.browser.ext_bridge import _TabDriver, _Tab
    from ferb.core.types import TabRef
    from unittest.mock import AsyncMock
    import base64
    ref = TabRef('device','brave','Default',1,'t1')
    bridge = SimpleNamespace(gen=1, conn=object(), call=AsyncMock(return_value={
        'data': base64.b64encode(b'target pixels').decode(), 'mask': {'rects': [], 'viewport': [100, 50]}}))
    tab = _Tab(ref,bridge,19,'task','https://example.test',snap_gen=3,visual=('keep',))
    d = _TabDriver(None,lambda:[]); d._tabs['t1'] = tab
    target,capture=d.recording_source(ref,'task')
    assert target['tab_id'] == 19 and asyncio.run(capture()) == b'target pixels'
    assert tab.snap_gen == 3 and tab.visual == ('keep',)
    tab.owner='other'
    with pytest.raises(Refused): asyncio.run(capture())
    assert bridge.call.await_count == 1


def test_extension_recording_chunks_do_not_consume_click_screenshot():
    out=worker('''(async () => {
      attach=async id=>tabState(id);
      maskState=async()=>({documents:[[0,'doc']],rects:[],viewport:[100,50]});
      cdp=async()=>({data:'x'.repeat(SHOT_PART+10)});
      const st=tabState(8); st.shot='click image';
      const start=await commands.record_frame({tab_id:8});
      let wrong=false; try {await commands.record_frame({tab_id:8,part:0,capture_id:'wrong'});} catch(e){wrong=true;}
      const a=await commands.record_frame({tab_id:8,part:0,capture_id:start.capture_id});
      const b=await commands.record_frame({tab_id:8,part:1,capture_id:start.capture_id});
      return {parts:start.parts,size:a.data.length+b.data.length,expected:SHOT_PART+10,wrong,shot:st.shot,cleared:st.recordFrame===null};
    })()''')
    assert out == {'parts':2,'size':out['expected'],'expected':out['expected'],'wrong':True,'shot':'click image','cleared':True}


def test_recording_frame_masks_cover_every_embedded_frame():
    """An embedded frame that can't be read (not loaded yet, or out of the extension's reach) is painted whole: its
    content is never in a frame unmasked (L13; formerly the whole frame was dropped)."""
    out=worker('''(async()=>{
      inject=async()=>{};
      let rows=[{frameId:0,result:{documentId:'top',viewport:[100,100],rects:[],
        frames:[{frame_id:2,rect:[10,10,20,20],inner:[10,10],scaled:false}]}}];
      chrome.scripting={executeScript:async()=>rows};
      const unread=await maskState(9);
      rows.push({frameId:2,result:{documentId:'child',viewport:[20,20],rects:[],frames:[]}});
      return {unread,read:await maskState(9)};
    })()''')
    assert out['unread']['rects'] == [[10,10,20,20]] and out['read']['rects'] == []
    assert [d[0] for d in out['read']['documents']] == [0,2]


def test_restarted_app_cannot_reuse_old_recording_selection(monkeypatch):
    from tests.mackit import vision
    from ferb.act.mac.apps import RunningApp
    d, _, _, apps, _ = vision(monkeypatch)
    async def check():
        _, shot, _ = await d.screenshot('Notes')
        apps.apps[0] = RunningApp('com.apple.Notes','Notes',31,False)
        with pytest.raises(Refused, match='fresh'):
            d.recording_source('Notes',shot.id)
    asyncio.run(check())


def test_concurrent_recording_starts_cannot_claim_the_same_output(tmp_path,colours):
    from ferb.act.recording import RecordingManager
    async def check():
        m=RecordingManager(sanitize=lambda image: image)
        async def capture():
            await asyncio.sleep(0)
            return colours[0]
        starts=await asyncio.gather(*(m.start('task',{'tab':i},tmp_path/'one.mp4',capture) for i in (1,2)),
                                     return_exceptions=True)
        assert sum(isinstance(r,Refused) for r in starts)==1
        assert sum(isinstance(r,dict) for r in starts)==1
        await m.end_task('task')
        assert not m.records and not list(tmp_path.glob('.ferb-recording-*'))
    asyncio.run(check())
