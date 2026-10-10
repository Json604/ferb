"""System-wide AX event subscriptions. No screenshot, input, activation or per-app behavior inference."""
import asyncio
import threading
from typing import Any, Callable

from ferb.sense.trail import Observation
from ferb.sense.watchers.base import Emit

APP_EVENTS = ("AXWindowCreated", "AXFocusedWindowChanged", "AXFocusedUIElementChanged",
              "AXApplicationActivated", "AXApplicationDeactivated", "AXMainWindowChanged")
ELEMENT_EVENTS = ("AXValueChanged", "AXTitleChanged", "AXUIElementDestroyed", "AXSelectedChildrenChanged")


def accessibility_observation(bundle_id: str, app: str, pid: int, notification: str, facts: dict[str, Any],
                              redact: Callable[[Any], Any]) -> Observation:
    allowed = {k: facts[k] for k in ("window_id", "field_id", "role", "title") if k in facts}
    return Observation("accessibility", redact({"bundle_id": bundle_id, "app": app, "pid": pid,
                       "notification": notification} | allowed), {"actor": "unknown"})


class AccessibilityWatcher:
    source = "accessibility"

    def __init__(self, redact: Callable[[Any], Any], excluded: Callable[[str, str], bool]) -> None:
        self.redact, self.excluded = redact, excluded

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        loop = asyncio.get_running_loop()
        done = threading.Event()
        failure: list[Exception] = []

        def subscription_failure() -> RuntimeError:
            error = failure[0]
            detail = self.redact({'error': f'{type(error).__name__}: {error}'})['error']
            return RuntimeError(f'Accessibility subscription failed: {detail}')

        def native_body() -> None:
            import ApplicationServices as AX
            import Quartz
            import objc
            from AppKit import NSWorkspace
            from ferb.act.mac.accessibility import _attr
            observers: dict[int, tuple[Any, Any, list[Any]]] = {}
            failed_pids: set[int] = set()
            runloop = Quartz.CFRunLoopGetCurrent()

            def deliver(bundle: str, name: str, pid: int, kind: str, element: Any) -> None:
                role = str(_attr(element, "AXRole") or "")
                subrole = str(_attr(element, "AXSubrole") or "")
                window = _attr(element, "AXWindow")
                facts = {"role": role, "field_id": str(_attr(element, "AXIdentifier") or hash(element)),
                         "window_id": str(_attr(window, "AXWindowNumber") or hash(window)) if window is not None else ""}
                # Secure controls never expose their titles/values to the callback or event queue.
                if "Secure" not in role + subrole:
                    facts["title"] = str(_attr(element, "AXTitle") or "")
                observation = accessibility_observation(bundle, name, pid, kind, facts, self.redact)
                loop.call_soon_threadsafe(emit, observation)

            def attach(app: Any) -> None:
                pid, bundle, name = int(app.processIdentifier()), str(app.bundleIdentifier() or ""), str(app.localizedName() or "")
                if not bundle or self.excluded(bundle, name):
                    return
                root = AX.AXUIElementCreateApplication(pid)
                registered: list[Any] = []

                def watch_element(observer: Any, element: Any) -> None:
                    if element is None or any(element == old for old in registered):
                        return
                    registered.append(element)
                    for event in ELEMENT_EVENTS:
                        AX.AXObserverAddNotification(observer, element, event, None)

                # AXObserverCreate retains its C callback. PyObjC requires an explicit bridge closure;
                # keep the decorated function in observers for the entire native observer lifetime.
                @objc.callbackFor(AX.AXObserverCreate)
                def callback(observer: Any, element: Any, notification: str, refcon: Any) -> None:
                    if done.is_set() or self.excluded(bundle, name):
                        return
                    try:
                        deliver(bundle, name, pid, str(notification), element)
                        if str(notification) == "AXUIElementDestroyed":
                            registered[:] = [old for old in registered if old != element]
                            return
                        watch_element(observer, _attr(root, "AXFocusedUIElement"))
                        for window in _attr(root, "AXWindows") or []:
                            watch_element(observer, window)
                    except Exception:
                        # A disappearing element is normal; never log its possibly private exception value.
                        return

                code, observer = AX.AXObserverCreate(pid, callback, None)
                if code or observer is None:
                    failed_pids.add(pid)
                    loop.call_soon_threadsafe(emit, Observation("accessibility", self.redact({"bundle_id": bundle, "pid": pid,
                        "subscription": "unavailable", "ax_error": int(code)}), {"actor": "unknown"}))
                    return
                unsupported = []
                for event in APP_EVENTS:
                    status = AX.AXObserverAddNotification(observer, root, event, None)
                    if status:
                        unsupported.append({"event": event, "ax_error": int(status)})
                watch_element(observer, _attr(root, "AXFocusedUIElement"))
                for window in _attr(root, "AXWindows") or []:
                    watch_element(observer, window)
                Quartz.CFRunLoopAddSource(runloop, AX.AXObserverGetRunLoopSource(observer), Quartz.kCFRunLoopDefaultMode)
                observers[pid] = (observer, callback, registered)
                loop.call_soon_threadsafe(emit, Observation("accessibility", self.redact({"bundle_id": bundle, "pid": pid,
                    "subscription": "attached", "unsupported": unsupported}), {"actor": "unknown"}))

            try:
                while not done.is_set():
                    apps = list(NSWorkspace.sharedWorkspace().runningApplications())
                    pids = {int(a.processIdentifier()) for a in apps}
                    for pid in set(observers) - pids:
                        observer, _, _ = observers.pop(pid)
                        Quartz.CFRunLoopRemoveSource(runloop, AX.AXObserverGetRunLoopSource(observer), Quartz.kCFRunLoopDefaultMode)
                    failed_pids.intersection_update(pids)
                    for app in apps:
                        if int(app.processIdentifier()) not in observers and int(app.processIdentifier()) not in failed_pids:
                            attach(app)
                    Quartz.CFRunLoopRunInMode(Quartz.kCFRunLoopDefaultMode, 1.0, False)
            except Exception as error:
                failure.append(error)
            finally:
                for observer, _, _ in observers.values():
                    Quartz.CFRunLoopRemoveSource(runloop, AX.AXObserverGetRunLoopSource(observer), Quartz.kCFRunLoopDefaultMode)

        def native() -> None:
            try:
                native_body()
            except Exception as error:
                failure.append(error)

        thread = threading.Thread(target=native, name="ferb-ax-observers", daemon=True)
        thread.start()
        try:
            while not stop.is_set():
                await asyncio.sleep(0.1)
                if failure:
                    raise subscription_failure()
        finally:
            done.set()
            await asyncio.to_thread(thread.join, 2)
        if failure:
            raise subscription_failure()
