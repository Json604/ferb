"""Owns the apps FERB launches as task resources (L39). An app that wasn't running when a FERB call launched it belongs
to that call's task, and is quit when the task's resources are released. It stays running, named in the release
record so the task's answer names it, when the owner used it during the task, when it may hold unsaved changes, or
when FERB can't tell. Must not force anything: quitting asks the app to quit, as ⌘Q does.

- "The owner used it" (ruling, L39): it is the owner's frontmost app at release, or the desktop observer recorded it
  coming forward with no FERB call that can change the screen in flight or just ended (`brought_forward`). A FERB
  call near the moment it came forward (the live case: Word came forward during FERB's click in Pages) makes it
  FERB's, not the owner's. Residual: the owner bringing it forward at the same moment as a FERB on-screen call reads
  as FERB's; the unsaved-changes rule still keeps their work.
- "Unsaved changes": a window Accessibility marks edited (AXEdited). A document window (AXDocument) whose edited state
  Accessibility can't read, or windows FERB can't read at all, count as "can't tell": the app stays.
- An app is matched by its process id and bundle id, so a relaunch by the owner is never quit."""

import asyncio
import json
from dataclasses import dataclass
from typing import Any, Callable

KEPT_OWNER = "you used it during the task"
KEPT_UNSAVED = "it has unsaved changes"
KEPT_UNKNOWN = "FERB couldn't tell whether it has unsaved changes"
KEPT_REFUSED = "it didn't accept FERB's request to quit"


@dataclass(frozen=True)
class Launched:
    bundle_id: str
    name: str
    pid: int
    at: str                                   # when FERB launched it (ISO), for the desktop records after it


def brought_forward(db: Any, tree_id: str, pid: int, since: str) -> bool:
    """Whether the desktop observer recorded the app (by pid) coming forward in this task tree after `since` with no
    FERB call that can change the screen in flight or just ended (P26's facts, never who did it)."""
    rows = db.conn.execute(
        "SELECT output_json FROM spans WHERE task_id = ? AND kind = 'disturbance.change' AND started_at >= ? AND "
        "json_extract(output_json, '$.changes.frontmost_app.after.pid') = ?", (tree_id, since, pid)).fetchall()
    for row in rows:
        data = json.loads(row[0] or "{}")
        if not any(isinstance(a, dict) and a.get("on_screen") for a in data.get("actions") or []):
            return True
    return False


def unsaved(pid: int) -> bool | None:
    """True when a window of the process is marked edited; False when every window reads as not edited or isn't a
    document; None when FERB can't tell (no Accessibility, or a document window whose edited state can't be read)."""
    try:
        import ApplicationServices as AX
    except ImportError:
        return None
    if not AX.AXIsProcessTrusted():
        return None
    app = AX.AXUIElementCreateApplication(pid)
    err, windows = AX.AXUIElementCopyAttributeValue(app, "AXWindows", None)
    if err == AX.kAXErrorNoValue or (not err and not windows):
        return False
    if err:
        return None
    unknown = False
    for window in windows:
        err, edited = AX.AXUIElementCopyAttributeValue(window, "AXEdited", None)
        if not err and edited:
            return True
        if err:
            doc_err, doc = AX.AXUIElementCopyAttributeValue(window, "AXDocument", None)
            if not doc_err and doc:
                unknown = True
    return None if unknown else False


def _ask_to_quit(pid: int, bundle_id: str) -> bool:
    """Ask exactly that process to quit (NSRunningApplication terminate: the app's own quit, as ⌘Q)."""
    from AppKit import NSRunningApplication
    app = NSRunningApplication.runningApplicationWithProcessIdentifier_(pid)
    if app is None or str(app.bundleIdentifier() or "") != bundle_id:
        return False
    return bool(app.terminate())


class LaunchedApps:
    """The apps each task's calls launched. `running()`: the apps running now (apps.running's RunningApp list);
    `owner_used(task_id, app)`: the owner used it during the task; `unsaved(pid)` and `quit(pid, bundle_id)` as above
    (tests give fakes)."""

    def __init__(self, running: Callable[[], list[Any]], owner_used: Callable[[str, Launched], bool],
                 unsaved: Callable[[int], bool | None] = unsaved,
                 quit: Callable[[int, str], bool] = _ask_to_quit) -> None:
        self.running, self.owner_used, self.unsaved, self.quit = running, owner_used, unsaved, quit
        self._by_task: dict[str, list[Launched]] = {}

    def add(self, task_id: str, app: Launched) -> None:
        self._by_task.setdefault(task_id, []).append(app)

    def of(self, task_id: str) -> list[Launched]:
        return list(self._by_task.get(task_id, []))

    async def release(self, task_id: str) -> list[dict[str, Any]]:
        """Each app the task launched: quit, or left running with why; one resource record each."""
        out = []
        for app in self._by_task.pop(task_id, []):
            record: dict[str, Any] = {"resource": "app", "app": app.name, "bundle_id": app.bundle_id}
            try:
                now = [a for a in await asyncio.to_thread(self.running)
                       if a.pid == app.pid and a.bundle_id == app.bundle_id]
                if not now:
                    out.append(record | {"released": True, "closed": False, "note": "it had already quit"})
                    continue
                if now[0].active or self.owner_used(task_id, app):     # the daemon's records: its thread
                    out.append(record | {"released": True, "closed": False, "left_running": KEPT_OWNER})
                    continue
                state = await asyncio.to_thread(self.unsaved, app.pid)
                if state is not False:
                    out.append(record | {"released": True, "closed": False,
                                         "left_running": KEPT_UNSAVED if state else KEPT_UNKNOWN})
                    continue
                asked = await asyncio.to_thread(self.quit, app.pid, app.bundle_id)
                out.append(record | ({"released": True, "closed": True} if asked else
                                     {"released": False, "closed": False, "left_running": KEPT_REFUSED,
                                      "error": f"{app.name} didn't accept FERB's request to quit"}))
            except asyncio.CancelledError:
                raise
            except Exception as e:
                out.append(record | {"released": False, "closed": False, "left_running": KEPT_UNKNOWN,
                                     "error": f"{type(e).__name__}: {e}"})
        return out


def left_running(resources: list[dict[str, Any]]) -> list[str]:
    """The owner's line for each app FERB launched and left running, from the release records (the answer's last
    lines, like the sites contacted)."""
    return [f"{r['app']} is still open: FERB opened it for this task and left it running because {r['left_running']}."
            for r in resources if isinstance(r, dict) and r.get("resource") == "app" and r.get("left_running")]
