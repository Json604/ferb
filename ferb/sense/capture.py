"""Capture guards and per-field text aggregation. Never persist raw keys or infer authority from observed text."""
from dataclasses import dataclass
import threading
from typing import Any, Callable

from ferb.sanitize.redact import password_field
from ferb.sense.trail import Observation

CONCEALED = {"org.nspasteboard.ConcealedType", "org.nspasteboard.TransientType"}
TEXT_ROLES = {"AXTextField", "AXTextArea", "AXComboBox"}
# AX attributes that can name a password field, also while "Show password" makes it a plain AXTextField.
FIELD_METADATA_ATTRS = ("AXTitle", "AXDescription", "AXPlaceholderValue", "AXIdentifier", "AXDOMIdentifier")
_clipboard_write: tuple[int, str | None] | None = None


def password_element(role: str, element: Any, attr: Callable[[Any, str], Any]) -> bool:
    """A text element whose own AX metadata names it a password field: its value is a secret (rule 14)."""
    return role in TEXT_ROLES and password_field(role, *(attr(element, a) for a in FIELD_METADATA_ATTRS))


def mark_clipboard_write(change_count: int, task_id: str | None = None) -> None:
    """Called only after FERB's native pasteboard write succeeds; unrelated later changes never inherit it."""
    global _clipboard_write
    _clipboard_write = (change_count, task_id)


def clipboard_write() -> tuple[int, str | None] | None:
    return _clipboard_write


@dataclass(frozen=True)
class Field:
    bundle_id: str
    app: str
    pid: int
    window_id: str
    field_id: str
    role: str
    label: str = ""
    masked: bool = False

    @property
    def safe(self) -> bool:
        return bool(self.field_id) and self.role in TEXT_ROLES and not self.masked

    def place(self) -> tuple[str, int, str, str]:
        """Which element this is, whatever its state (masked, its role or label changed)."""
        return self.bundle_id, self.pid, self.window_id, self.field_id

    def data(self) -> dict[str, Any]:
        return {"bundle_id": self.bundle_id, "app": self.app, "pid": self.pid,
                "window_id": self.window_id, "field_id": self.field_id, "role": self.role, "field": self.label}


@dataclass
class _Pending:
    field: Field
    actor: str
    text: str
    at: float


class TypedFields:
    """Hold text in volatile memory for the decided P18 window; only redacted complete field bursts leave here."""
    def __init__(self, emit: Callable[[Observation], None], redact: Callable[[Any], Any], hold_s: float = 0.5) -> None:
        self.emit, self.redact, self.hold_s = emit, redact, hold_s
        self._pending: _Pending | None = None
        self._lock = threading.RLock()

    def clear(self) -> None:
        with self._lock:
            self._pending = None

    def leave(self, current: "Field | None") -> None:
        """A key arrived where nothing is captured (a secure field, a terminal, no text field): a burst held for
        another field is complete and kept; one for this same field, now not safe, is dropped."""
        with self._lock:
            p, self._pending = self._pending, None
            if p is not None and (current is None or current.place() != p.field.place()):
                self._emit(p)

    @property
    def holding(self) -> bool:
        """Typed text is waiting for its field's flush."""
        with self._lock:
            return self._pending is not None

    def feed(self, field: Field | None, text: str, *, actor: str, now: float, secure: bool = False) -> None:
        with self._lock:
            if secure or field is None or not field.safe:
                self._pending = None
                return
            p = self._pending
            if p is None or p.field != field or p.actor != actor:
                # Focus moved to another safe field (or another actor typed): the former burst is complete. Its field
                # was checked safe as each of its keys arrived, so it is kept (fix list: text typed within 0.5 s of
                # moving on was dropped).
                if p is not None:
                    self._emit(p)
                p = self._pending = _Pending(field, actor, "", now)
            for char in text:
                if char in ("\x08", "\x7f"):
                    p.text = p.text[:-1]
                elif char not in ("\r", "\x1b", "\t"):
                    p.text += char
            p.at = now

    def flush(self, current: Field | None, *, secure: bool, now: float) -> None:
        with self._lock:
            p = self._pending
            if p is None:
                return
            if secure:
                self._pending = None              # Secure Input or a secure field now: nothing held is kept
                return
            if current is not None and current == p.field and current.safe and now - p.at < self.hold_s:
                return
            # The hold ran out, or focus moved on: the burst is complete. A field that turned secret meanwhile
            # (`current` the same element, no longer safe) keeps nothing.
            self._pending = None
            if current is None or current.place() != p.field.place() or current.safe:
                self._emit(p)

    def _emit(self, p: _Pending) -> None:
        if p.text:
            # F5's shared capture redactor understands native OTP/secure-field metadata.
            text = self.redact.field(p.text, role=p.field.role, label=p.field.label)
            data = self.redact(p.field.data() | {"text": text})
            self.emit(Observation("typed_text", data, {"actor": p.actor}))


def clipboard_observation(change_count: int, types: set[str], read: Callable[[], str | None],
                          redact: Callable[[Any], Any], *, ferb_count: int | None = None,
                          task_id: str | None = None, context: dict[str, Any] | None = None) -> Observation | None:
    if types & CONCEALED:
        return None
    text = read()
    if text is None:
        return None
    actor = "ferb" if change_count == ferb_count else "unknown"
    return Observation("clipboard", redact((context or {}) | {"text": text, "change_count": change_count}),
                       {"actor": actor, "task_id": task_id if actor == "ferb" else None})
