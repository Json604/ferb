"""Offline source retention and restart contracts, using documented WhatsApp/self-chat and clipboard scenarios."""
import asyncio
import json
from pathlib import Path
from types import SimpleNamespace

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.sanitize.redact import Redactor
from ferb.sense.batches import EventBatcher
from ferb.sense.daily import DailyActivity
from ferb.sense.exclusions import Exclusions
from ferb.sense.notifications import NotificationPolicy
from ferb.sense.perception import Perception
from ferb.sense.trail import Observation, Trail
from ferb.store.db import Database


def fixture(tmp_path: Path):
    db = Database(tmp_path / 'ferb.db'); db.migrate()
    clock, ids, redact = FakeClock(), SequentialIds('p'), Redactor()
    activity = DailyActivity(tmp_path / 'activity.jsonl', clock.now)
    trail = Trail(db, clock, ids, redact, Exclusions, lambda _, data: activity.record(data), lambda: 'dev', lambda: False)
    daemon = SimpleNamespace(db=db, clock=clock, ids=ids, redact=redact, daily_activity=activity, trail=trail,
                             sense=None, jev=None, _publish=lambda *args: None,
                             config=SimpleNamespace(get=lambda *args: args[-1]))
    return daemon, Perception(daemon)


def test_seven_day_content_retention_preserves_source_id_and_clears_search_and_copy(tmp_path: Path) -> None:
    d, perception = fixture(tmp_path)
    event = d.trail.record(Observation('clipboard', {'text': 'uniqueclipboardcontent', 'change_count': 4}))
    d.clock.advance(7 * 86400 + 1)
    assert perception.purge_text() == 1
    row = d.trail.query(since='2099-01-01', event_id=event.id)['events'][0]
    assert row['id'] == event.id and row['data'] == {'change_count': 4, 'content_expired': True}
    assert d.trail.query(since='2000-01-01', text='uniqueclipboardcontent')['count'] == 0
    assert 'uniqueclipboardcontent' not in d.daily_activity.path.read_text()
    assert perception.purge_text() == 0
    d.db.close()


def test_notification_receipt_survives_reconstruction_and_excluded_capture_has_no_receipt(tmp_path: Path) -> None:
    d, perception = fixture(tmp_path)
    event = Observation('notification', {'external_id': 'self-message-1', 'bundle_id': 'com.whatsapp.WhatsApp', 'body': 'hi'})
    perception.notification(event)
    Perception(d).notification(event)
    assert d.db.conn.execute('SELECT COUNT(*) FROM trail').fetchone()[0] == 1
    d.trail.exclusions = lambda: Exclusions.build(['com.whatsapp.WhatsApp'])
    perception.notification(Observation('notification', event.data | {'external_id': 'self-message-2'}))
    assert not perception.seen('self-message-2')
    d.db.close()


def test_notification_context_uses_only_earlier_same_conversation_and_told_receipts(tmp_path: Path) -> None:
    d, perception = fixture(tmp_path)
    data = {'bundle_id': 'com.whatsapp.WhatsApp', 'conversation': 'self'}
    first = d.trail.record(Observation('notification', data | {'body': 'old'}))
    current = d.trail.record(Observation('notification', data | {'body': 'current'}))
    d.trail.record(Observation('notification', data | {'body': 'future'}))
    d.trail.record(Observation('notification', data | {'conversation': 'other', 'body': 'different chat'}))
    perception.policy.told(data['bundle_id'], 'self', first.id, 'Already mentioned')
    state = perception.policy.context({'id': current.id, 'data': current.data})
    assert [event['id'] for event in state['conversation']] == [first.id]
    assert state['already_told'][0]['event_id'] == first.id
    d.db.close()


def test_restart_releases_only_in_flight_receipts(tmp_path: Path) -> None:
    d, perception = fixture(tmp_path)
    d._jev_active = lambda: object()                   # Jev switched on (the batcher does nothing while it's off)
    event = d.trail.record(Observation('window', {'bundle_id': 'com.microsoft.VSCode', 'title': 'FERB'}))
    d.db.conn.execute("INSERT INTO perception_batches VALUES('batch',?,'age','pending',?,NULL)",
                      (d.clock.now().isoformat(), json.dumps([event.id])))
    d.db.conn.execute("INSERT INTO perception_receipts VALUES(?,'batch')", (event.id,))
    assert not perception.batches.pending()
    stop = asyncio.Event(); stop.set()
    asyncio.run(perception.batches.run(stop))
    assert [row['id'] for row in perception.batches.pending()] == [event.id]
    d.db.close()
