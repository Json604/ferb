"""Empty native run loops must idle while workspace notifications remain available."""
import asyncio
import sys
import threading
from types import SimpleNamespace

from ferb.sense.watchers.workspace import WorkspaceWatcher


def test_empty_runloop_waits_delivers_notifications_and_cleans_up(monkeypatch):
    callbacks, removed = {}, []
    entered = threading.Event()
    pumps = 0

    def pump(*args):
        nonlocal pumps
        pumps += 1
        entered.set()
        return 1  # kCFRunLoopRunFinished: no native sources, returns immediately.

    def register(name, obj, queue, callback):
        callbacks[name] = callback
        return name

    center = SimpleNamespace(addObserverForName_object_queue_usingBlock_=register,
                             removeObserver_=removed.append)
    appkit = SimpleNamespace(NSWorkspace=SimpleNamespace(sharedWorkspace=lambda: SimpleNamespace(
        notificationCenter=lambda: center)), NSWorkspaceApplicationKey='app')
    for event in ('DidLaunchApplication', 'DidTerminateApplication', 'DidActivateApplication',
                  'DidDeactivateApplication', 'WillSleep', 'DidWake', 'DidMount', 'DidUnmount'):
        setattr(appkit, f'NSWorkspace{event}Notification', event)
    monkeypatch.setitem(sys.modules, 'AppKit', appkit)
    monkeypatch.setitem(sys.modules, 'Quartz', SimpleNamespace(
        kCFRunLoopDefaultMode='default', CFRunLoopRunInMode=pump))
    events = []

    async def run():
        stop = asyncio.Event()
        task = asyncio.create_task(WorkspaceWatcher(lambda data: {**data, 'redacted': True},
            lambda bundle, name: bundle == 'excluded').run(events.append, stop))
        try:
            assert await asyncio.to_thread(entered.wait, 1)
            for bundle in ('excluded', 'included'):
                app = SimpleNamespace(bundleIdentifier=lambda: bundle, localizedName=lambda: 'App',
                                      processIdentifier=lambda: 7)
                callbacks['DidActivateApplication'](SimpleNamespace(
                    userInfo=lambda: {'app': app}, name=lambda: 'DidActivateApplication'))
            await asyncio.sleep(.05)
            assert pumps == 1
        finally:
            stop.set()
            # The remaining half-second native slice must be interruptible.
            await asyncio.wait_for(task, .3)

    asyncio.run(run())
    assert len(events) == 1 and events[0].data['redacted'] is True
    assert events[0].data['bundle_id'] == 'included'
    assert sorted(removed) == sorted(callbacks)
    assert not any(t.name == 'ferb-workspace' for t in threading.enumerate())
