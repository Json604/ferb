"""Listen-only text capture. Secure Input, terminal apps and the focused AX role are checked before Unicode is
extracted."""
import asyncio
import ctypes
import time
from typing import Any, Callable

from ferb.policy.protection import DEFAULT_TERMINAL_APPS
from ferb.sense.capture import Field, TypedFields, TEXT_ROLES
from ferb.sense.watchers.base import Emit, focused_app, poll

# L17: typed text is never captured in a terminal app. A `sudo` password typed there has no secure field and Secure
# Input may be off, so it would otherwise reach Jev. A where-boundary on the app (bundle ids, any case), never a
# guess about what is typed; the one list of terminal bundle ids is protection's (also D31's: FERB never types there).
TERMINAL_APPS = frozenset(b.casefold() for b in DEFAULT_TERMINAL_APPS)


def secure_input() -> bool:
    try:
        lib = ctypes.CDLL("/System/Library/Frameworks/Carbon.framework/Carbon")
        lib.IsSecureEventInputEnabled.restype = ctypes.c_bool
        return bool(lib.IsSecureEventInputEnabled())
    except (OSError, AttributeError):
        return True                         # no primary defense: capture nothing


def focused_field(excluded: Callable[[str, str], bool]) -> Field | None:
    from ferb.act.mac.accessibility import _attr
    import ApplicationServices as AX
    app = focused_app()
    if app is None or app.bundle_id.casefold() in TERMINAL_APPS or excluded(app.bundle_id, app.name):
        return None
    root = AX.AXUIElementCreateApplication(app.pid)
    field = _attr(root, "AXFocusedUIElement")
    role = str(_attr(field, "AXRole") or "") if field is not None else ""
    subrole = str(_attr(field, "AXSubrole") or "") if field is not None else ""
    if role not in TEXT_ROLES or "Secure" in role or "Secure" in subrole:
        return None
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
        except UnicodeDecodeError:          # the buffer cut a surrogate pair in two: more text follows
            n, chars = size, None
        if n < size:
            if isinstance(chars, str):
                return chars
            return bytes().join(int(c).to_bytes(2, "little") for c in chars or ()).decode("utf-16-le", errors="replace")
        size *= 2


class TextInputWatcher:
    source = "typed_text"

    def __init__(self, owner_input: Any, redact: Callable[[Any], Any], excluded: Callable[[str, str], bool]) -> None:
        self.owner_input, self.redact, self.excluded = owner_input, redact, excluded

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
            field = focused_field(self.excluded)
            if field is None or not field.safe:
                fields.clear()
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
            if secure_input():
                fields.clear()             # do not query AXValue while Secure Input is active
                return
            fields.flush(focused_field(self.excluded), secure=secure_input(), now=time.monotonic())

        self.owner_input.on_text = guarded_key
        try:
            await poll(0.1, stop, flush)
        finally:
            self.owner_input.on_text = None
            fields.clear()
