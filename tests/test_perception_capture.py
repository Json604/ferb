"""F7 offline replays; native privacy/AX behavior still needs the owner's two-app prototype acceptance."""
from dataclasses import replace

from ferb.sanitize.redact import Redactor
from ferb.sense.capture import Field, TypedFields, clipboard_observation
from ferb.sense.watchers.accessibility import accessibility_observation


# decision: SENSE-401
def test_typed_text_is_grouped_per_field_redacted_and_attributed() -> None:
    events = []
    field = Field("com.whatsapp.WhatsApp", "WhatsApp", 7, "chat", "message", "AXTextArea")
    fields = TypedFields(events.append, Redactor(), hold_s=0.5)
    fields.feed(field, "hello ", actor="owner", now=0)
    fields.feed(field, "world", actor="owner", now=0.1)
    fields.flush(field, secure=False, now=0.5)
    assert not events
    fields.flush(field, secure=False, now=0.7)
    assert events[0].data["text"] == "hello world" and events[0].hints["actor"] == "owner"
    fields.feed(field, "sk-ant-abcdefghijklmnopqrstuvwx", actor="ferb", now=1)
    fields.flush(field, secure=False, now=2)
    assert "sk-ant-" not in str(events[-1].data) and events[-1].hints["actor"] == "ferb"


# decision: SENSE-401
def test_secure_input_during_hold_and_unknown_or_masked_fields_drop_text() -> None:
    events = []
    normal = Field("com.microsoft.VSCode", "Code", 9, "editor", "search", "AXTextField")
    fields = TypedFields(events.append, Redactor())
    fields.feed(normal, "secret", actor="owner", now=0)
    fields.flush(normal, secure=True, now=0.1)
    fields.flush(normal, secure=False, now=1)
    for field in (replace(normal, role="AXSecureTextField"), replace(normal, role="AXWindow"),
                  replace(normal, masked=True)):
        fields.feed(field, "never capture", actor="owner", now=2)
        fields.flush(field, secure=False, now=3)
    assert not events


# decision: SENSE-403
def test_clipboard_concealed_markers_checked_before_read_and_exact_ferb_count() -> None:
    reads = []
    read = lambda: reads.append(True) or "hello"
    assert clipboard_observation(4, {"org.nspasteboard.ConcealedType"}, read, Redactor()) is None
    assert clipboard_observation(5, {"org.nspasteboard.TransientType"}, read, Redactor()) is None
    assert not reads
    event = clipboard_observation(6, {"public.utf8-plain-text"}, read, Redactor(), ferb_count=6)
    assert event.hints["actor"] == "ferb" and event.data["text"] == "hello"
    event = clipboard_observation(7, set(), read, Redactor(), ferb_count=6)
    assert event.hints["actor"] == "unknown"


# decision: SENSE-403
def test_two_app_ax_replays_preserve_app_window_parent_and_never_read_values() -> None:
    # Synthetic callback metadata representing the owner's WhatsApp and VS Code surfaces, not live captures.
    for app, window in (("com.whatsapp.WhatsApp", "chat"), ("com.microsoft.VSCode", "editor")):
        event = accessibility_observation(app, app, 7, "AXValueChanged", {"window_id": window,
            "field_id": "compose", "role": "AXTextArea", "title": "sk-ant-abcdefghijklmnopqrstuvwx"}, Redactor())
        assert event.data["bundle_id"] == app and event.data["window_id"] == window
        assert "sk-ant-" not in str(event.data) and "value" not in event.data
