"""Listen-only text capture. Secure Input, terminals (terminal apps and terminals inside other apps) and the focused
field's AX role and password metadata are checked before Unicode is extracted."""
import asyncio
import ctypes
import time
from typing import Any, Callable, Collection

from ferb.policy import terminal as terminals
from ferb.policy.protection import DEFAULT_TERMINAL_APPS
from ferb.sense.front_window import FERB_APPS
from ferb.sense.capture import Field, TypedFields, TEXT_ROLES, password_element
from ferb.sense.watchers.base import Emit, Focused, focused_app, poll

# L17, D28, rule 14: typed text is never captured in a terminal. A `sudo` password typed there has no secure field and
# Secure Input may be off, so it would otherwise reach Jev. A where-boundary, never a guess about what is typed, by the
# same facts as D31's floor before FERB types into a terminal (policy/terminal.py): a terminal app or an app whose
# Info.plist declares the Shell role or whose terminal Accessibility can't show (Zed) is never even read; in any other
# app a focused text field inside a terminal (VS Code's xterm.js input, a web terminal, JetBrains' Terminal tool
# window; a label naming one counts, errs toward not capturing) is skipped.
TERMINAL_APPS = frozenset(b.casefold() for b in DEFAULT_TERMINAL_APPS)
_last: list[tuple[int, Any, bool]] = []      # the last focused field's answer: one CFEqual per keystroke, not a walk
# The owner's [protection] terminal_apps (D31 round 2, N5: the typing floor counts them, so capture does too); the
# watcher is handed the daemon's list, this default serves a watcher made without one
owner_terminal_apps: Callable[[], Collection[str]] = lambda: ()


def terminal_app(app: Focused, owner_apps: Collection[str] = ()) -> bool:
    """A terminal by the app alone: its bundle id (built-in or the owner's list), its Info.plist's Shell role, or a
    terminal it can't show."""
    from ferb.act.mac import apps
    return app.bundle_id.casefold() in TERMINAL_APPS or \
        terminals.surface(app.bundle_id, [], owner_apps, shell_app=apps.declares_shell(app.bundle_id)) is not None


def in_terminal(app: Focused, field: Any, owner_apps: Collection[str] = ()) -> bool:
    """Whether the focused field is inside a terminal by its own and its ancestors' facts (never its value). The answer
    for the same element is reused until the focus moves."""
    from ferb.act.mac import accessibility as ax
    if _last and _last[0][0] == app.pid and _same(_last[0][1], field):
        return _last[0][2]
    found = terminals.surface(app.bundle_id, ax.surface_facts(field), owner_apps) is not None
    _last[:] = [(app.pid, field, found)]
    return found


def _same(a: Any, b: Any) -> bool:
    if a is b:
        return True
    from ferb.act.mac import accessibility as ax
    try:
        return ax.same(a, b)
    except Exception:                        # not an AX element (or CF can't compare them): not the same
        return False


def secure_input() -> bool:
    try:
        lib = ctypes.CDLL("/System/Library/Frameworks/Carbon.framework/Carbon")
        lib.IsSecureEventInputEnabled.restype = ctypes.c_bool
        return bool(lib.IsSecureEventInputEnabled())
    except (OSError, AttributeError):
        return True                         # no primary defense: capture nothing


def focused_field(excluded: Callable[[str, str], bool],
                  terminal_apps: Callable[[], Collection[str]] | None = None) -> Field | None:
    from ferb.act.mac.accessibility import _attr
    import ApplicationServices as AX
    app = focused_app()
    owner_apps = (terminal_apps or owner_terminal_apps)()
    if app is None or terminal_app(app, owner_apps) or excluded(app.bundle_id, app.name):
        return None
    if app.bundle_id.casefold() in FERB_APPS:
        return None                         # rule 17 (owner, 2026-10-25): FERB's own apps are never read per keystroke
    root = AX.AXUIElementCreateApplication(app.pid)
    field = _attr(root, "AXFocusedUIElement")
    role = str(_attr(field, "AXRole") or "") if field is not None else ""
    subrole = str(_attr(field, "AXSubrole") or "") if field is not None else ""
    if role not in TEXT_ROLES or "Secure" in role or "Secure" in subrole or password_element(role, field, _attr):
        return None                         # a shown password is a plain AXTextField: its metadata still says so
    if in_terminal(app, field, owner_apps):
        return None                         # a terminal inside another app (D31's facts): never captured
    value = _attr(field, "AXValue")
    masked = isinstance(value, str) and bool(value) and all(c in "•●*·" for c in value)
    window = _attr(field, "AXWindow")
    # Object identity is local to this AX element, never a label interpreted as authorization.
    fid = str(_attr(field, "AXIdentifier") or hash(field))
    wid = str(_attr(window, "AXWindowNumber") or hash(window)) if window is not None else ""
    return Field(app.bundle_id, app.name, app.pid, wid, fid, role,
                 str(_attr(field, "AXTitle") or _attr(field, "AXDescription") or ""), masked)


def key_text(event: Any) -> str:
    """The text a key event carries. PyObjC fills and reports only as much as the buffer it's given holds (a
    zero-length buffer gives (0, ''): C1), so ask with a buffer and grow it until the whole text fits."""
    import Quartz
    size = 4
    while True:
        try:
            n, chars = Quartz.CGEventKeyboardGetUnicodeString(event, size, None, None)
        except UnicodeDecodeError:          # the buffer cut a surrogate pair in two: one more unit completes it
            size += 1
            try:
                n, chars = Quartz.CGEventKeyboardGetUnicodeString(event, size, None, None)
            except UnicodeDecodeError:      # a lone surrogate never decodes: this event's text is dropped
                return ""
        if n < size:
            if isinstance(chars, str):
                return chars
            return bytes().join(int(c).to_bytes(2, "little") for c in chars or ()).decode("utf-16-le", errors="replace")
        size *= 2


class TextInputWatcher:
    source = "typed_text"

    def __init__(self, owner_input: Any, redact: Callable[[Any], Any], excluded: Callable[[str, str], bool],
                 terminal_apps: Callable[[], Collection[str]] | None = None) -> None:
        self.owner_input, self.redact, self.excluded = owner_input, redact, excluded
        self.terminal_apps = terminal_apps          # the owner's [protection] terminal_apps (N5)

    async def run(self, emit: Emit, stop: asyncio.Event) -> None:
        if not self.owner_input.running:
            raise RuntimeError("listen-only input tap is unavailable")
        import Quartz
        loop = asyncio.get_running_loop()
        fields = TypedFields(lambda o: loop.call_soon_threadsafe(emit, o), self.redact)

        failure: list[str] = []

        def key(event: Any, marked: bool) -> None:
            if stop.is_set() or secure_input():
                fields.clear()
                return
            field = focused_field(self.excluded, self.terminal_apps)
            if field is None or not field.safe:
                fields.leave(field)        # an earlier field's complete burst is kept; this key never is
                return
            flags = Quartz.CGEventGetFlags(event)
            if flags & (Quartz.kCGEventFlagMaskCommand | Quartz.kCGEventFlagMaskControl):
                return
            text = key_text(event)
            if text:
                fields.feed(field, text, actor="ferb" if marked else "owner", now=time.monotonic())

        def guarded_key(event: Any, marked: bool) -> None:
            try:
                key(event, marked)
            except Exception as error:
                fields.clear()             # an AX/API failure cannot validate previously held text
                failure[:] = [type(error).__name__]

        def flush() -> None:
            if failure:
                raise RuntimeError(f"text capture adapter failed: {failure[0]}")
            if not fields.holding:
                return                     # nothing typed since the last flush: no Accessibility reads
            if secure_input():
                fields.clear()             # do not query AXValue while Secure Input is active
                return
            fields.flush(focused_field(self.excluded, self.terminal_apps), secure=secure_input(), now=time.monotonic())

        self.owner_input.on_text = guarded_key
        try:
            await poll(0.1, stop, flush)
        finally:
            self.owner_input.on_text = None
            fields.clear()
