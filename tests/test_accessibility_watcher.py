"""AX observer bridge contracts through injected modules; never subscribe to native apps."""
import asyncio
import sys
import threading
from types import SimpleNamespace

import pytest

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
