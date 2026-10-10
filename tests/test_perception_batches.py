import asyncio
import json
from datetime import datetime, timezone
from pathlib import Path

from ferb.clock.clock import FakeClock
from ferb.core.ids import SequentialIds
from ferb.sanitize.redact import Redactor
from ferb.sense.batches import batch_events, EventBatcher
from ferb.sense.notifications import NotificationPolicy
from ferb.store.db import Database


def test_bursts_split_by_app_and_oversize_event_has_retrievable_excerpt() -> None:
    events = [{"id": str(i), "source": "clipboard", "data": {"bundle_id": app, "text": text}}
              for i, (app, text) in enumerate((("a", "x " * 300), ("b", "b"), ("a", "c")))]
    batches = batch_events(events, budget=500, count=lambda text: len(text))
    assert [e["id"] for batch in batches for e in batch] == ["0", "2", "1"]
    assert batches[0][0]["excerpt"] and batches[0][0]["source_ref"] == "trail:0"
    assert events[0]["data"]["text"] == "x " * 300
    assert all(len(json.dumps(batch, ensure_ascii=False)) <= 500 for batch in batches)


def test_notification_scope_mute_requires_owner_quote_and_keeps_topic_rules_for_jev(tmp_path: Path) -> None:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    policy = NotificationPolicy(db, FakeClock(), SequentialIds("n"), Redactor())
    import pytest
    with pytest.raises(ValueError, match="owner"):
        policy.add("com.whatsapp.WhatsApp", "self", None, "ignore them", "different words")
    policy.add("com.whatsapp.WhatsApp", "self", None, "mute this chat", "please mute this chat")
    assert policy.muted("com.whatsapp.WhatsApp", "self")
    assert not policy.muted("com.whatsapp.WhatsApp", "other")
    policy.add("com.whatsapp.WhatsApp", "other", "football", "not football", "not football")
    assert not policy.muted("com.whatsapp.WhatsApp", "other")
    assert policy.context({"id": "e", "data": {"bundle_id": "com.whatsapp.WhatsApp", "conversation": "other"}})["rules"]
    db.close()


def test_batch_receipts_prevent_replay_and_notification_stays_shadow(tmp_path: Path) -> None:
    from ferb.sense.trail import Observation, Trail
    from ferb.sense.exclusions import Exclusions
    db = Database(tmp_path / "ferb.db"); db.migrate()
    clock, ids = FakeClock(), SequentialIds("b")
    trail = Trail(db, clock, ids, Redactor(), Exclusions, lambda *args: None, lambda: "dev", lambda: False)
    event = trail.record(Observation("notification", {"bundle_id": "com.whatsapp.WhatsApp", "conversation": "self", "body": "hi"}))
    calls = []
    class Jev:
        async def shadow(self, state, asks):
            calls.append((state, asks)); return {asks[0].key: "yes"}
    batcher = EventBatcher(db, clock, ids, lambda: Jev(), NotificationPolicy(db, clock, ids, Redactor()), lambda *args: None)
    asyncio.run(batcher.flush("interrupt"))
    asyncio.run(batcher.flush("interrupt"))
    assert len(calls) == 1 and calls[0][1][0].key == "notification.attention"
    assert db.conn.execute("SELECT COUNT(*) FROM perception_receipts").fetchone()[0] == 1
    assert db.conn.execute("SELECT COUNT(*) FROM notification_told").fetchone()[0] == 0
    db.close()


def test_notification_budget_omits_only_old_history_with_retrieval_refs(tmp_path: Path, monkeypatch) -> None:
    import ferb.sense.batches as module
    db = Database(tmp_path / 'ferb.db'); db.migrate()
    clock, ids = FakeClock(), SequentialIds('history')
    current = {'id': 'current', 'source': 'notification', 'data': {'body': 'latest'}}
    rules = [{'owner_quote': 'Always mention changed dates'}]
    class Policy:
        def muted(self, *args): return False
        def context(self, event):
            return {'notification': event, 'rules': rules, 'already_told': [{'text': 'previous report'}],
                    'conversation': [{'id': 'old', 'data': 'x' * 10000}, {'id': 'newer', 'data': 'recent'}]}
    batcher = EventBatcher(db, clock, ids, lambda: None, Policy(), lambda *args: None)
    monkeypatch.setattr(module, 'count_tokens', len)
    state, asks = batcher._request([current], 'batch')
    assert state['notification'] == current and state['rules'] == rules
    assert state['omitted_oldest_messages'] == {'count': 1, 'source_refs': ['trail:old']}
    assert state['conversation'][0]['id'] == 'newer' and state['already_told']
    db.close()
