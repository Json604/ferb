"""Target recordings: real encoding/decoding, isolated driver routes, and lifecycle ownership."""
import asyncio
import errno
import json
import os
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


def test_window_recording_keeps_a_frame_whose_new_password_field_has_no_box_and_says_so(monkeypatch):
    """L13: a field that appears while capturing is painted over from the walk after it; one with no box to paint
    leaves that frame to the F5 text recognition, with the note (owner: no screenshot is refused)."""
    from tests.mackit import vision
    d, ax, shots, *_ = vision(monkeypatch)
    async def check():
        _, shot, _ = await d.screenshot('Notes')
        _, capture = d.recording_source('Notes', shot.id)
        ax.secure = [False, True]
        frame, note = await capture()
        assert frame == shots.jpeg and 'has no place' in note
    asyncio.run(check())


@pytest.mark.parametrize('unknown_after_capture', [False, True])
def test_window_recording_keeps_a_frame_whose_secret_inventory_is_unknown_and_says_so(monkeypatch,
                                                                                      unknown_after_capture):
    from tests.mackit import vision
    d, ax, shots, *_ = vision(monkeypatch)
    async def check():
        _, shot, _ = await d.screenshot('Notes')
        _, capture = d.recording_source('Notes', shot.id)
        ax.secure = [False, None] if unknown_after_capture else [None]
        frame, note = await capture()                  # the other look read it: what it found stays painted (I1)
        assert frame == shots.jpeg and "in one look around the capture" in note and len(shots.captured) == 2
    asyncio.run(check())


def test_extension_record_frame_preserves_action_references_and_never_activates():
    out = worker('''(async () => {
      attach = async id => tabState(id);
      maskState = async id => ({top: 'doc', viewport: [0, 0, 100, 50], items: []});
      let calls = []; cdp = async (id, method, args) => {calls.push({id,method,args}); return {data:'jpeg'};};
      const st = tabState(19); st.visual = {snap:7}; st.shot = 'other screenshot';
      const frame = await commands.record_frame({tab_id:19});
      return {frame,calls,visual:st.visual,shot:st.shot};
    })()''')
    page = {'top': 'doc', 'viewport': [0, 0, 100, 50], 'items': []}
    assert out['frame'] == {'data': 'jpeg', 'mask': {'before': page, 'after': page}}
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
    page = {'top': 'doc', 'viewport': [0, 0, 100, 50], 'items': []}
    bridge = SimpleNamespace(gen=1, conn=object(), call=AsyncMock(return_value={
        'data': base64.b64encode(b'target pixels').decode(), 'mask': {'before': page, 'after': page}}))
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
      maskState=async()=>({top:'doc',viewport:[0,0,100,50],items:[]});
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
    """An embedded frame whose host heard no announcement (nothing loaded yet, or out of the extension's reach) is
    painted whole: its content is never in a frame unmasked (L13; formerly the whole frame was dropped)."""
    out=worker('''(async()=>{
      inject=async()=>{};
      let rows=[{frameId:0,result:{documentId:'top',own:null,viewport:[0,0,100,100],rects:[],
        frames:[{id:'e',heard:[],rect:[10,10,20,20],redrawn:false}]}}];
      chrome.scripting={executeScript:async args=>args.func.toString().includes('captureState')?rows:[]};
      const unread=await maskState(9,Date.now()+5000);
      rows[0].result.frames[0].heard=['t'];
      rows.push({frameId:2,result:{documentId:'child',own:'t',viewport:[0,0,20,20],rects:[],frames:[]}});
      return {unread,read:await maskState(9,Date.now()+5000)};
    })()''')
    assert [i['rect'] for i in out['unread']['items'] if i['paint']] == [[10,10,20,20]]
    assert out['read']['items'] == [{'key':'frame|top|e','rect':[10,10,20,20],'paint':False,'child':'child'}]


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


def test_a_recording_checks_the_folder_it_really_writes_into(tmp_path, monkeypatch):
    """The output folder is checked as macOS opened it (F_GETPATH) before anything is made there, so another spelling
    of a protected folder (volfs, the Data volume) or a link swapped in after the gateway's check can't take the video
    into FERB's own files (rule 10)."""
    import os
    import ferb.act.recording as recording
    monkeypatch.setattr(recording, 'executable', lambda name: name)
    protected = tmp_path / 'protected'
    protected.mkdir()
    seen = []

    def closed(path):
        seen.append(path)
        return 'closed' if Path(os.path.realpath(protected)) in (path, *path.parents) else None
    st = os.stat(protected)

    async def capture(): return b'frame'

    async def check():
        m = recording.RecordingManager(sanitize=lambda image: image)
        for alias in (Path(f'/.vol/{st.st_dev}/{st.st_ino}/v.mp4'),
                      Path('/System/Volumes/Data' + os.path.realpath(protected)) / 'v.mp4'):
            with pytest.raises(Refused, match='closed'):
                await m.start('task', {'tab': 1}, alias, capture, closed=closed)
        assert list(protected.iterdir()) == [] and m.records == {}
    asyncio.run(check())


def _protected(tmp_path):
    """A stand-in for one of FERB's own files, which a swap must never reach."""
    protected = tmp_path / 'ferb-home'
    protected.mkdir()
    (protected / 'config.toml').write_text('owner settings')
    return protected


def test_a_receipt_swapped_for_a_link_after_the_check_is_replaced_not_written_through(tmp_path, colours):
    """Everything a recording writes goes through the output folder it checked and keeps open: a receipt turned into
    a link to FERB's own file mid-recording is replaced by the finished receipt; the linked file is untouched."""
    from ferb.act.recording import RecordingManager
    protected = _protected(tmp_path)
    out = tmp_path / 'out'
    out.mkdir()

    async def check():
        async def capture(): return colours[0]
        m = RecordingManager(sanitize=lambda image: image, staging=tmp_path / 'staging')
        r = await m.start('task', {'tab': 1}, out / 'v.mp4', capture, fps=1)
        receipt = out / 'v.mp4.json'
        receipt.unlink()
        receipt.symlink_to(protected / 'config.toml')
        result = await m.stop(r['id'], 'task')
        assert result['capture_complete'], result
        assert (protected / 'config.toml').read_text() == 'owner settings'
        assert not receipt.is_symlink() and json.loads(receipt.read_text())['id'] == r['id']
    asyncio.run(check())


def test_an_output_folder_swapped_after_the_check_never_takes_frames_or_the_video_elsewhere(tmp_path, colours):
    """Frames are staged in FERB's own folder, never beside the output; the video and receipt go into the folder that
    was checked even if a link now stands at its old path."""
    from ferb.act.recording import RecordingManager
    protected = _protected(tmp_path)
    out = tmp_path / 'out'
    out.mkdir()

    async def check():
        async def capture(): return colours[1]
        m = RecordingManager(sanitize=lambda image: image, staging=tmp_path / 'staging')
        r = await m.start('task', {'tab': 2}, out / 'v.mp4', capture, fps=1)
        assert sorted(p.name for p in out.iterdir()) == ['v.mp4.json']       # no frames beside the output
        await m.sample(r['id'], 'task')
        out.rename(tmp_path / 'moved')
        out.symlink_to(protected)
        result = await m.end_task('task')                                    # the task ends: same checked folder
        assert result[0]['bytes'] > 0 and 'task ended' in result[0]['error'], result
        assert sorted(p.name for p in protected.iterdir()) == ['config.toml']
        assert (tmp_path / 'moved' / 'v.mp4').stat().st_size > 0 and (tmp_path / 'moved' / 'v.mp4.json').exists()
        assert not list((tmp_path / 'staging').iterdir())                    # staging cleaned up
        landed = os.path.realpath(tmp_path / 'moved' / 'v.mp4')              # where the video really is, not the link
        assert result[0]['path'] == landed and result[0]['receipt'] == landed + '.json', result
        assert json.loads((tmp_path / 'moved' / 'v.mp4.json').read_text())['path'] == landed
    asyncio.run(check())


def test_an_output_file_planted_as_a_link_is_never_written_through(tmp_path, colours):
    from ferb.act.recording import RecordingManager
    protected = _protected(tmp_path)
    out = tmp_path / 'out'
    out.mkdir()

    async def check():
        async def capture(): return colours[0]
        m = RecordingManager(sanitize=lambda image: image, staging=tmp_path / 'staging')
        r = await m.start('task', {'tab': 3}, out / 'v.mp4', capture, fps=1)
        (out / 'v.mp4').symlink_to(protected / 'config.toml')
        result = await m.stop(r['id'], 'task')
        assert not result['capture_complete'] and 'exists' in result['error'].lower()
        assert (protected / 'config.toml').read_text() == 'owner settings'
    asyncio.run(check())


def test_the_daemon_stages_recording_frames_in_ferbs_own_folder(home):
    from ferb.daemon.main import Daemon
    d = Daemon(home)
    assert d.recordings.staging == home / 'run' / 'recordings'


def _fds() -> set[int]:
    return {int(f) for f in os.listdir('/dev/fd')}


def test_a_start_that_fails_after_the_receipt_closes_the_receipt_and_removes_it(tmp_path, monkeypatch):
    """Reviewer's round-3 reproduction (rec_leak.py): staging can't be made after the receipt was created; the start
    fails, and neither the receipt's descriptor nor the receipt is left behind."""
    from ferb.act import recording
    monkeypatch.setattr(recording, 'executable', lambda name: '/usr/bin/true')
    (tmp_path / 'staging').write_text('not a folder')
    out = tmp_path / 'out'
    out.mkdir()

    async def check():
        m = recording.RecordingManager(sanitize=lambda b: b, staging=tmp_path / 'staging')
        async def capture(): return b'frame'
        before = _fds()
        with pytest.raises(OSError):
            await m.start('task', {'tab': 1}, out / 'v.mp4', capture)
        assert _fds() - before == set() and list(out.iterdir()) == [] and m.records == {}
    asyncio.run(check())


def test_publishing_across_volumes_closes_the_new_file_when_the_video_cant_be_read(tmp_path, monkeypatch):
    from ferb.act import recording

    def other_volume(*a, **k):
        raise OSError(errno.EXDEV, os.strerror(errno.EXDEV))
    monkeypatch.setattr(recording.os, 'link', other_volume)
    out_dir = tmp_path / 'out'
    out_dir.mkdir()
    out = os.open(out_dir, os.O_RDONLY | os.O_DIRECTORY)
    try:
        before = _fds()
        with pytest.raises(FileNotFoundError):
            recording._publish(tmp_path / 'missing.mp4', out, 'v.mp4')
        assert _fds() - before == set() and list(out_dir.iterdir()) == []
    finally:
        os.close(out)


def test_the_daemon_clears_staging_a_restart_left_behind(home):
    """No recording survives a restart: frames staged by the last run are removed when the daemon starts."""
    from ferb.clock.clock import FakeClock
    from ferb.core.ids import SequentialIds
    from tests.conftest import RunningDaemon
    stale = home / 'run' / 'recordings' / 'rec-abc123'
    stale.mkdir(parents=True)
    (stale / '00000000.jpg').write_bytes(b'frame from the last run')
    (stale / 'frames.txt').write_text('ffconcat version 1.0')
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds('id'), watch_owner_input=False)
    try:
        assert list((home / 'run' / 'recordings').iterdir()) == []
    finally:
        d.stop()
