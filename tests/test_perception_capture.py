"""F7 offline replays; native privacy/AX behavior still needs the owner's two-app prototype acceptance."""
from dataclasses import replace

from ferb.sanitize.redact import Redactor
from ferb.sense.capture import Field, TypedFields, clipboard_observation
from ferb.sense.watchers.accessibility import accessibility_observation


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


def test_field_switch_and_pause_never_relabel_old_text_as_new_field() -> None:
    events = []
    a = Field("com.whatsapp.WhatsApp", "WhatsApp", 7, "chat", "compose", "AXTextArea")
    b = replace(a, field_id="search")
    fields = TypedFields(events.append, Redactor())
    fields.feed(a, "draft", actor="owner", now=0)
    fields.feed(b, "find", actor="owner", now=0.1)      # step 9b: the burst in `a` is complete and kept, as `a`'s
    fields.clear()
    fields.flush(b, secure=False, now=1)
    assert [(e.data["field_id"], e.data["text"]) for e in events] == [("compose", "draft")]   # never relabelled


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


def test_two_app_ax_replays_preserve_app_window_parent_and_never_read_values() -> None:
    # Synthetic callback metadata representing the owner's WhatsApp and VS Code surfaces, not live captures.
    for app, window in (("com.whatsapp.WhatsApp", "chat"), ("com.microsoft.VSCode", "editor")):
        event = accessibility_observation(app, app, 7, "AXValueChanged", {"window_id": window,
            "field_id": "compose", "role": "AXTextArea", "title": "sk-ant-abcdefghijklmnopqrstuvwx"}, Redactor())
        assert event.data["bundle_id"] == app and event.data["window_id"] == window
        assert "sk-ant-" not in str(event.data) and "value" not in event.data


def test_native_subscription_failure_does_not_stop_other_sources(monkeypatch) -> None:
    import asyncio
    import sys
    import pytest
    from ferb.sense.watchers.accessibility import AccessibilityWatcher
    from ferb.sense.watchers.workspace import WorkspaceWatcher
    # Deliberately fail imports before any native app call; the watcher must surface failure, not set shared stop.
    monkeypatch.setitem(sys.modules, 'ApplicationServices', None)
    monkeypatch.setitem(sys.modules, 'AppKit', None)
    async def run():
        for watcher in (AccessibilityWatcher(Redactor(), lambda *args: False),
                        WorkspaceWatcher(Redactor(), lambda *args: False)):
            stop = asyncio.Event()
            with pytest.raises(RuntimeError, match='failed'):
                await watcher.run(lambda event: None, stop)
            assert not stop.is_set()
    asyncio.run(run())
