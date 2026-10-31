"""System-wide AX event subscriptions. No screenshot, input, activation or per-app behavior inference."""
import asyncio
import threading
from typing import Any, Callable

from ferb.sense.capture import password_element
from ferb.sense.front_window import FERB_APPS
from ferb.sense.trail import Observation
from ferb.sense.watchers.base import Emit, run_loop_slice

APP_EVENTS = ("AXWindowCreated", "AXFocusedWindowChanged", "AXFocusedUIElementChanged",
              "AXApplicationActivated", "AXApplicationDeactivated", "AXMainWindowChanged")
ELEMENT_EVENTS = ("AXValueChanged", "AXTitleChanged", "AXUIElementDestroyed", "AXSelectedChildrenChanged")


def accessibility_observation(bundle_id: str, app: str, pid: int, notification: str, facts: dict[str, Any],
                              redact: Callable[[Any], Any]) -> Observation:
    """One AX event as recorded. In a browser no title is kept (fix list, step 9b): a page or window title can't be
    checked against incognito or the excluded domains (windows.py skips browser titles for the same reason)."""
    from ferb.act.mac.browsers import is_browser
    keys = ("window_id", "field_id", "role") if is_browser(bundle_id) else ("window_id", "field_id", "role", "title")
    allowed = {k: facts[k] for k in keys if k in facts}
    return Observation("accessibility", redact({"bundle_id": bundle_id, "app": app, "pid": pid,
                       "notification": notification} | allowed), {"actor": "unknown"})


def element_facts(kind: str, element: Any, attr: Callable[[Any, str], Any]) -> dict[str, Any] | None:
    """What an AX event about `element` records, or None when nothing is: a value change of a secure or password
    field (a shown password is a plain AXTextField; its metadata still names it). Such a field never gives its title."""
    role = str(attr(element, "AXRole") or "")
    subrole = str(attr(element, "AXSubrole") or "")
    hidden = "Secure" in role + subrole or password_element(role, element, attr)
    if hidden and kind == "AXValueChanged":
        return None
    window = attr(element, "AXWindow")
    facts = {"role": role, "field_id": str(attr(element, "AXIdentifier") or hash(element)),
             "window_id": str(attr(window, "AXWindowNumber") or hash(window)) if window is not None else ""}
    if not hidden:
        facts["title"] = str(attr(element, "AXTitle") or "")
    return facts


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
                # Secure and password controls never expose their titles or value changes to the event queue.
                facts = element_facts(kind, element, _attr)
                if facts is None:
                    return
                observation = accessibility_observation(bundle, name, pid, kind, facts, self.redact)
                loop.call_soon_threadsafe(emit, observation)

            def attach(app: Any) -> None:
                pid, bundle, name = int(app.processIdentifier()), str(app.bundleIdentifier() or ""), str(app.localizedName() or "")
                if not bundle or self.excluded(bundle, name):
                    return
                if bundle.casefold() in FERB_APPS:
                    return      # rule 17 (owner, 2026-10-25): no observer in FERB's own apps; each key typed there read the UI
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
                    run_loop_slice(done, 1.0)
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
