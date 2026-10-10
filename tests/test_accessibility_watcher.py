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
