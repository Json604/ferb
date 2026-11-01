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

from ferb.core.errors import Refused

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


def _ask_to_hide(pid: int, bundle_id: str) -> bool:
    """Hide exactly that process (NSRunningApplication hide: the app's own hide, as ⌘H)."""
    from AppKit import NSRunningApplication
    app = NSRunningApplication.runningApplicationWithProcessIdentifier_(pid)
    if app is None or str(app.bundleIdentifier() or "") != bundle_id:
        return False
    return bool(app.hide())


def _tree(task_id: str) -> str:
    return task_id.partition(".")[0]


class LaunchedApps:
    """The apps each task tree's calls launched. `running()`: the apps running now (apps.running's RunningApp list);
    `owner_used(task_id, app)`: the owner used it during the task; `unsaved(pid)` and `quit(pid, bundle_id)` as above
    (tests give fakes); `now()`: FERB's clock, as ISO text, for when a launch happened (the desktop records' clock).
    An app belongs to its tree, not to the plan step that launched it: it is released with the tree's own task
    (`<tree>.worker`, released when the task ends, whatever happened), so a later step doesn't launch it again."""

    def __init__(self, running: Callable[[], list[Any]], owner_used: Callable[[str, Launched], bool],
                 unsaved: Callable[[int], bool | None] = unsaved,
                 quit: Callable[[int, str], bool] = _ask_to_quit, now: Callable[[], str] | None = None,
                 hide: Callable[[int, str], bool] = _ask_to_hide) -> None:
        self.running, self.owner_used, self.unsaved, self.quit, self.hide = running, owner_used, unsaved, quit, hide
        self.now = now or (lambda: __import__("datetime").datetime.now().astimezone().isoformat())
        self._by_tree: dict[str, list[Launched]] = {}

    def launched(self, task_id: str, bundle_id: str, name: str, pid: int) -> Launched:
        """A call of this task launched the app (it wasn't running): stamped with FERB's clock."""
        app = Launched(bundle_id, name, pid, self.now())
        self.add(task_id, app)
        return app

    def add(self, task_id: str, app: Launched) -> None:
        self._by_tree.setdefault(_tree(task_id), []).append(app)

    def of(self, task_id: str) -> list[Launched]:
        return list(self._by_tree.get(_tree(task_id), []))

    def _own(self, task_id: str, app: Any, verb: str) -> Launched:
        """The task tree's launch record for this exact process (bundle id and pid), else Refused: FERB quits or
        hides only an app it opened for this task (HAND-200, kb area hands)."""
        mine = [x for x in self.of(task_id) if x.pid == app.pid and x.bundle_id == app.bundle_id]
        if not mine:
            raise Refused(f"{app.name} isn't an app FERB opened for this task, so FERB doesn't {verb} it: the "
                          "owner's apps stay as they are (HAND-200)")
        return mine[0]

    async def quit_now(self, task_id: str, app: Any) -> str:
        """⌘Q, the safe way (HAND-200): only an app this task tree launched, and only when it reads as having no
        unsaved changes (as at release); then the app is asked to quit, as ⌘Q does, and is no longer the task's. Else
        Refused, saying why (nothing was sent)."""
        mine = self._own(task_id, app, "quit")
        state = await asyncio.to_thread(self.unsaved, mine.pid)
        if state is not False:
            raise Refused(f"FERB didn't quit {mine.name}: {KEPT_UNSAVED if state else KEPT_UNKNOWN}")
        if not await asyncio.to_thread(self.quit, mine.pid, mine.bundle_id):
            raise Refused(f"{mine.name} didn't accept FERB's request to quit")
        tree = self._by_tree.get(_tree(task_id), [])
        if mine in tree:
            tree.remove(mine)
        return f"asked {mine.name} to quit (FERB opened it for this task; it shows no unsaved changes)"

    async def hide_now(self, task_id: str, app: Any) -> str:
        """⌘H, the safe way (HAND-200): only an app this task tree launched is hidden (nothing is lost by hiding)."""
        mine = self._own(task_id, app, "hide")
        if not await asyncio.to_thread(self.hide, mine.pid, mine.bundle_id):
            raise Refused(f"{mine.name} didn't hide when FERB asked")
        return f"hid {mine.name} (FERB opened it for this task)"

    async def release(self, task_id: str) -> list[dict[str, Any]]:
        """At the tree's own task's release (`<tree>.worker`): each app the tree launched, quit or left running with
        why; one resource record each. Any other task's release (a plan step, a check) leaves them to the tree."""
        tree, _, rest = task_id.partition(".")
        if rest != "worker":
            return []
        out = []
        for app in self._by_tree.pop(tree, []):
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
