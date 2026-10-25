"""AX observer bridge contracts through injected modules; never subscribe to native apps."""
import asyncio
import sys
import threading
from types import SimpleNamespace

import pytest

from ferb.sanitize.redact import Redactor
from ferb.sense.watchers.accessibility import AccessibilityWatcher


@pytest.fixture
def bridge(monkeypatch):
    calls = []
    observers = []
    application = SimpleNamespace(processIdentifier=lambda: 7, bundleIdentifier=lambda: 'com.microsoft.VSCode',
                                  localizedName=lambda: 'Code')
    def create(pid, callback, output):
        if not hasattr(callback, 'pyobjc_closure'):
            raise TypeError('callback argument must be decorated with objc.callbackFor')
        calls.append(('create', pid, callback))
        observer = object()
        observers.append(observer)
        return 0, observer
    def callback_for(api):
        assert api is ax.AXObserverCreate
        def decorate(callback):
            callback.pyobjc_closure = object()
            return callback
        return decorate
    ax = SimpleNamespace(AXUIElementCreateApplication=lambda pid: object(), AXObserverCreate=create,
                         AXObserverAddNotification=lambda *args: 0, AXObserverGetRunLoopSource=lambda observer: observer)
    quartz = SimpleNamespace(CFRunLoopGetCurrent=lambda: 'loop', kCFRunLoopDefaultMode='default',
        CFRunLoopAddSource=lambda *args: calls.append(('add', args)),
        CFRunLoopRemoveSource=lambda *args: calls.append(('remove', args)),
        CFRunLoopRunInMode=lambda *args: threading.Event().wait(.001))
    monkeypatch.setitem(sys.modules, 'ApplicationServices', ax)
    monkeypatch.setitem(sys.modules, 'Quartz', quartz)
    monkeypatch.setitem(sys.modules, 'objc', SimpleNamespace(callbackFor=callback_for))
    monkeypatch.setitem(sys.modules, 'AppKit', SimpleNamespace(NSWorkspace=SimpleNamespace(
        sharedWorkspace=lambda: SimpleNamespace(runningApplications=lambda: [application]))))
    from ferb.act.mac import accessibility
    monkeypatch.setattr(accessibility, '_attr', lambda *args: None)
    return ax, calls, observers


def test_retained_observer_callback_is_bridged_and_runloop_source_is_removed(bridge):
    _, calls, observers = bridge
    events = []
    async def run():
        stop = asyncio.Event()
        def emit(event):
            events.append(event)
            if event.data.get('subscription') == 'attached':
                stop.set()
        await asyncio.wait_for(AccessibilityWatcher(Redactor(), lambda *args: False).run(emit, stop), 2)
    asyncio.run(run())
    assert events[0].data['subscription'] == 'attached'
    assert len(observers) == 1
    assert [call[0] for call in calls] == ['create', 'add', 'remove']
    assert hasattr(calls[0][2], 'pyobjc_closure')


@pytest.mark.parametrize('bundle', ['dev.ferb.notch', 'DEV.Ferb.Notch', 'com.ferb.ferb', 'com.ferb.app', 'com.ferb.daemon'])
def test_ferb_own_apps_never_get_an_observer(bridge, monkeypatch, bundle):
    """Rule 17 (owner, 2026-10-25): no subscription, callback or read in FERB's own apps."""
    _, calls, observers = bridge
    own = SimpleNamespace(processIdentifier=lambda: 9, bundleIdentifier=lambda: bundle, localizedName=lambda: 'FERB')
    workspace = sys.modules['AppKit'].NSWorkspace.sharedWorkspace()
    monkeypatch.setattr(workspace, 'runningApplications', lambda: [own], raising=False)
    monkeypatch.setattr(sys.modules['AppKit'].NSWorkspace, 'sharedWorkspace', lambda: workspace)
    scanned = threading.Event()
    monkeypatch.setattr(sys.modules['Quartz'], 'CFRunLoopRunInMode', lambda *args: scanned.set() or 1)

    async def run():
        stop = asyncio.Event()
        task = asyncio.create_task(AccessibilityWatcher(lambda data: data, lambda *args: False).run(
            lambda event: None, stop))
        try:
            assert await asyncio.to_thread(scanned.wait, 1)
        finally:
            stop.set()
            await asyncio.wait_for(task, .5)

    asyncio.run(run())
    assert observers == [] and not any(call[0] == 'create' for call in calls)


def test_subscription_failure_keeps_sanitized_bridge_diagnostic(bridge):
    ax, _, _ = bridge
    redactor = Redactor()
    redactor.add_secret_value('private-error-canary')
    def fail(*args):
        raise TypeError('callback signature is invalid; private-error-canary')
    ax.AXObserverCreate = fail
    async def run():
        with pytest.raises(RuntimeError) as caught:
            await AccessibilityWatcher(redactor, lambda *args: False).run(lambda event: None, asyncio.Event())
        message = str(caught.value)
        assert 'TypeError: callback signature is invalid' in message
        assert 'private-error-canary' not in message
    asyncio.run(run())


@pytest.mark.parametrize('empty_apps', [True, False], ids=['no-apps', 'subscriptions-denied'])
def test_no_native_sources_do_not_busy_poll_apps(bridge, monkeypatch, empty_apps):
    ax, _, _ = bridge
    entered = threading.Event()
    scans = 0
    workspace = sys.modules['AppKit'].NSWorkspace.sharedWorkspace()
    original_apps = workspace.runningApplications

    def applications():
        nonlocal scans
        scans += 1
        return [] if empty_apps else original_apps()

    def pump(*args):
        entered.set()
        return 1

    workspace.runningApplications = applications
    monkeypatch.setattr(sys.modules['AppKit'].NSWorkspace, 'sharedWorkspace', lambda: workspace)
    monkeypatch.setattr(sys.modules['Quartz'], 'CFRunLoopRunInMode', pump)
    monkeypatch.setattr(ax, 'AXObserverCreate', lambda *args: (-25211, None))

    async def run():
        stop = asyncio.Event()
        task = asyncio.create_task(AccessibilityWatcher(lambda data: data, lambda *args: False).run(
            lambda event: None, stop))
        try:
            assert await asyncio.to_thread(entered.wait, 1)
            await asyncio.sleep(.05)
            assert scans == 1
        finally:
            stop.set()
            await asyncio.wait_for(task, .3)

    asyncio.run(run())
    assert not any(t.name == 'ferb-ax-observers' for t in threading.enumerate())
