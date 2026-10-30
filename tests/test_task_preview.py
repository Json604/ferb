import asyncio


from ferb.daemon.task_preview import TaskPreviews


def test_preview_never_returns_frame_after_task_ends():
    asyncio.run(preview_never_returns_frame_after_task_ends())


async def preview_never_returns_frame_after_task_ends():
    previews = TaskPreviews()
    entered, release = asyncio.Event(), asyncio.Event()

    async def capture():
        entered.set()
        await release.wait()
        return b'captured'

    previews.bind('task', capture)
    pending = asyncio.create_task(previews.frame('task'))
    await entered.wait()
    previews.remove({'task'})
    release.set()
    assert await pending is None


def test_switching_targets_discards_old_inflight_frame():
    asyncio.run(switching_targets_discards_old_inflight_frame())


async def switching_targets_discards_old_inflight_frame():
    previews = TaskPreviews()
    entered, release = asyncio.Event(), asyncio.Event()

    async def old():
        entered.set()
        await release.wait()
        return b'old'

    async def new():
        return b'new'

    previews.bind('task', old)
    pending = asyncio.create_task(previews.frame('task'))
    await entered.wait()
    previews.bind('task', new)
    release.set()
    assert await pending is None
    assert await previews.frame('task') == b'new'
    assert await previews.frame('other') is None


def test_preview_is_owner_only():
    from ferb.daemon.auth import Role, allowed
    assert allowed(Role.CLI, 'tasks.preview')
    assert not allowed(Role.MCP_SHIM, 'tasks.preview')
    assert not allowed(Role.NATIVE_HOST, 'tasks.preview')


def test_gateway_preview_uses_task_owned_tab_and_sanitizes(monkeypatch):
    from types import SimpleNamespace
    from ferb.act.locks import ResourceLocks
    from ferb.daemon.gateway import Gateway
    from ferb.sanitize.redact import Redactor
    import ferb.sanitize.images

    calls = []

    class Driver:
        def owned_by(self, tab, task):
            calls.append((tab, task))
            return 'owned-tab'

        def recording_source(self, tab, owner):
            assert tab == 'owned-tab'
            assert owner == 'root.child'
            async def capture():
                return b'masked-capture'
            return {'tab': tab}, capture

        async def screenshot(self, tab):
            raise AssertionError('preview must not replace worker screenshot references')

    class Sanitizer:
        def __init__(self, redactor):
            pass

        def __call__(self, image):
            assert image == b'masked-capture'
            return b'sanitized'

    monkeypatch.setattr(ferb.sanitize.images, 'ImageRedactor', Sanitizer)
    gateway = object.__new__(Gateway)
    gateway.previews = TaskPreviews()
    gateway.locks = ResourceLocks()
    gateway.redact = Redactor()
    asyncio.run(gateway._preview_target(SimpleNamespace(driver=Driver()), 'browser.click',
                            SimpleNamespace(tab='tab-7'), SimpleNamespace(task_id='root.child'),
                            SimpleNamespace(tree_id='root')))
    assert asyncio.run(gateway.previews.frame('root')) == b'sanitized'
    assert calls == [('tab-7', 'root.child')]
    asyncio.run(gateway._preview_target(SimpleNamespace(), 'fs.read', SimpleNamespace(),
                            SimpleNamespace(task_id='root.child'), SimpleNamespace(tree_id='root')))
    assert asyncio.run(gateway.previews.frame('root')) == b'sanitized'


def test_mac_preview_uses_shot_window_and_never_app_front_window(monkeypatch):
    from types import SimpleNamespace
    from ferb.act.locks import ResourceLocks
    from ferb.daemon.gateway import Gateway
    from ferb.sanitize.redact import Redactor
    import ferb.sanitize.images

    class Driver:
        def recording_source(self, app, shot):
            assert (app, shot) == ('Notes', 'background-shot')
            async def capture():
                return b'background-window', 'masked'
            return {'window_id': 17}, capture

        async def window_image(self, app, window=None):
            raise AssertionError('must not capture the front window')

    monkeypatch.setattr(ferb.sanitize.images, 'ImageRedactor', lambda redact: lambda image: image)
    gateway = object.__new__(Gateway)
    gateway.previews = TaskPreviews()
    gateway.locks = ResourceLocks()
    gateway.redact = Redactor()
    cap = SimpleNamespace(driver=Driver())
    grant, record = SimpleNamespace(task_id='root.child'), SimpleNamespace(tree_id='root')
    asyncio.run(gateway._preview_target(cap, 'mac.click', SimpleNamespace(app='Notes', shot='background-shot'), grant, record))
    assert asyncio.run(gateway.previews.frame('root')) == b'background-window'
    asyncio.run(gateway._preview_target(cap, 'mac.act', SimpleNamespace(app='Notes', element='s1e2'), grant, record))
    assert asyncio.run(gateway.previews.frame('root')) == b'background-window'
    asyncio.run(gateway._preview_target(cap, 'mac.act', SimpleNamespace(app='Mail', element='s1e2'), grant, record))
    assert asyncio.run(gateway.previews.frame('root')) is None


def test_mac_exact_element_preview_preserves_worker_references(monkeypatch):
    from tests.mackit import vision
    d, ax, shots, *_ = vision(monkeypatch)
    async def check():
        await d.snapshot('Notes')
        _, shot, _ = await d.screenshot('Notes')
        state = d._state['com.apple.Notes']
        element = f's{state.gen}e1'
        from types import SimpleNamespace
        state.nodes[element] = SimpleNamespace(ref='observed-control')
        ax.element_window = lambda ref: 'exact-window'
        ax.frame = lambda ref: shots.bounds
        target, capture = d.preview_source('Notes', element=element)
        assert target['window_id'] == 110
        assert await capture() == shots.jpeg
        assert await capture() == shots.jpeg
        assert state.shot is shot
        assert element in state.nodes
    asyncio.run(check())


def test_mac_preview_without_exact_element_window_never_uses_front_window(monkeypatch):
    import pytest
    from tests.mackit import vision
    from ferb.core.errors import Refused
    d, ax, shots, *_ = vision(monkeypatch)
    asyncio.run(d.snapshot('Notes'))
    state = d._state['com.apple.Notes']
    element = f's{state.gen}e1'
    from types import SimpleNamespace
    state.nodes[element] = SimpleNamespace(ref='observed-control')
    ax.element_window = lambda ref: None
    shots.front_window = lambda *args: (_ for _ in ()).throw(AssertionError('no front fallback'))
    with pytest.raises(Refused):
        d.preview_source('Notes', element=element)


def test_task_ending_while_window_resolves_cannot_rebind_preview(monkeypatch):
    import threading
    from types import SimpleNamespace
    from ferb.daemon.gateway import Gateway
    from ferb.act.locks import ResourceLocks
    from ferb.sanitize.redact import Redactor
    entered, release = threading.Event(), threading.Event()
    class Driver:
        def recording_source(self, app, shot):
            entered.set()
            release.wait(2)
            async def capture():
                return b'ended-task'
            return {'window_id': 110}, capture
    async def check():
        gateway = object.__new__(Gateway)
        gateway.previews, gateway.locks, gateway.redact = TaskPreviews(), ResourceLocks(), Redactor()
        pending = asyncio.create_task(gateway._preview_target(SimpleNamespace(driver=Driver()), 'mac.click',
            SimpleNamespace(app='Notes', shot='s1'), SimpleNamespace(task_id='task'), SimpleNamespace(tree_id='task')))
        await asyncio.to_thread(entered.wait, 2)
        gateway.previews.remove({'task'})
        release.set()
        await pending
        assert await gateway.previews.frame('task') is None
    asyncio.run(check())


def test_snapshot_preview_uses_returned_window_not_mutable_driver_tree(monkeypatch):
    from tests.mackit import vision
    from types import SimpleNamespace
    from ferb.daemon.gateway import Gateway
    from ferb.act.locks import ResourceLocks
    from ferb.sanitize.redact import Redactor
    import ferb.sanitize.images
    d, ax, shots, *_ = vision(monkeypatch)
    asyncio.run(d.snapshot('Notes'))
    d._state['com.apple.Notes'].tree.window = 'old-window-from-concurrent-preview'
    ax.frame = lambda ref: (_ for _ in ()).throw(AssertionError('must use returned window id'))
    monkeypatch.setattr(ferb.sanitize.images, 'ImageRedactor', lambda redact: lambda image: image)
    async def check():
        gateway = object.__new__(Gateway)
        gateway.previews, gateway.locks, gateway.redact = TaskPreviews(), ResourceLocks(), Redactor()
        await gateway._preview_target(SimpleNamespace(driver=d), 'mac.snapshot', SimpleNamespace(app='Notes'),
            SimpleNamespace(task_id='task'), SimpleNamespace(tree_id='task'),
            {'windows': [{'id': 110, 'current': True}, {'id': 999, 'current': False}]})
        assert await gateway.previews.frame('task') == shots.jpeg
        assert shots.captured == [110]
    asyncio.run(check())


def test_repeated_same_surface_binding_does_not_discard_inflight_frame():
    async def check():
        previews = TaskPreviews()
        entered, release = asyncio.Event(), asyncio.Event()
        async def capture():
            entered.set()
            await release.wait()
            return b'frame'
        async def replacement():
            raise AssertionError('same exact surface must retain capture closure')
        previews.bind('task', capture, 'app:Notes:11', identity='window:11:110')
        pending = asyncio.create_task(previews.frame('task'))
        await entered.wait()
        previews.bind('task', replacement, 'app:Notes:11', identity='window:11:110')
        release.set()
        assert await pending == b'frame'
        assert await previews.frame('task') == b'frame'
        async def changed():
            return b'other-window'
        previews.bind('task', changed, 'app:Notes:11', identity='window:11:120')
        assert await previews.frame('task') == b'other-window'
    asyncio.run(check())


def test_gateway_switches_exact_windows_and_drops_invalid_explicit_target(monkeypatch):
    from types import SimpleNamespace
    from ferb.daemon.gateway import Gateway
    from ferb.act.locks import ResourceLocks
    from ferb.sanitize.redact import Redactor
    import ferb.sanitize.images
    monkeypatch.setattr(ferb.sanitize.images, 'ImageRedactor', lambda redact: lambda image: image)
    class Driver:
        def preview_source(self, app, *, window, element):
            assert app == 'Notes'
            if window == 'gone':
                raise LookupError('closed')
            async def capture():
                return (window or element).encode()
            return {'window_id': window or element}, capture
    async def check():
        gateway = object.__new__(Gateway)
        gateway.previews, gateway.locks, gateway.redact = TaskPreviews(), ResourceLocks(), Redactor()
        cap, grant, rec = SimpleNamespace(driver=Driver()), SimpleNamespace(task_id='task'), SimpleNamespace(tree_id='task')
        await gateway._preview_target(cap, 'mac.act', SimpleNamespace(app='Notes', element='s1e1'), grant, rec)
        assert await gateway.previews.frame('task') == b's1e1'
        await gateway._preview_target(cap, 'mac.screenshot', SimpleNamespace(app='Notes', window='72'), grant, rec)
        assert await gateway.previews.frame('task') == b'72'
        await gateway._preview_target(cap, 'mac.act',
            SimpleNamespace(app='Notes', action='key', element='unrelated-element'), grant, rec)
        assert await gateway.previews.frame('task') == b'72'
        await gateway._preview_target(cap, 'mac.screenshot', SimpleNamespace(app='Notes', window='gone'), grant, rec)
        assert await gateway.previews.frame('task') is None
    asyncio.run(check())


def test_gateway_same_window_action_keeps_frame_during_capture(monkeypatch):
    from types import SimpleNamespace
    from ferb.daemon.gateway import Gateway
    from ferb.act.locks import ResourceLocks
    from ferb.sanitize.redact import Redactor
    import ferb.sanitize.images
    monkeypatch.setattr(ferb.sanitize.images, 'ImageRedactor', lambda redact: lambda image: image)
    async def check():
        entered, release = asyncio.Event(), asyncio.Event()
        class Driver:
            def preview_source(self, app, *, window, element):
                async def capture():
                    entered.set()
                    await release.wait()
                    return b'continuous'
                return {'kind': 'window', 'app': 'Notes', 'pid': 11, 'window_id': 110}, capture
        gateway = object.__new__(Gateway)
        gateway.previews, gateway.locks, gateway.redact = TaskPreviews(), ResourceLocks(), Redactor()
        cap, grant, rec = SimpleNamespace(driver=Driver()), SimpleNamespace(task_id='task'), SimpleNamespace(tree_id='task')
        await gateway._preview_target(cap, 'mac.act', SimpleNamespace(app='Notes', element='s1e1'), grant, rec)
        frame = asyncio.create_task(gateway.previews.frame('task'))
        await entered.wait()
        await gateway._preview_target(cap, 'mac.act', SimpleNamespace(app='Notes', element='s2e2'), grant, rec)
        release.set()
        assert await frame == b'continuous'
    asyncio.run(check())
