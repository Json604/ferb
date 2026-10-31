from datetime import datetime, timedelta, timezone
import json

from ferb.sense.daily import DailyActivity


def test_daily_log_search_is_relevant_bounded_and_rolls_over(tmp_path):
    now = [datetime(2026, 10, 3, 10, tzinfo=timezone(timedelta(hours=5, minutes=30)))]
    log = DailyActivity(tmp_path / 'activity.jsonl', lambda: now[0])
    log.record({'what': 'browser.read', 'where': 'scaler Gmail FMI assignment 2', 'device_id': 'mac'})
    log.record({'what': 'mac.scroll', 'where': 'WhatsApp', 'device_id': 'mac'})
    hits = log.search('FMI assignment', limit=1)
    assert len(hits) == 1 and hits[0]['where'] == 'scaler Gmail FMI assignment 2'
    assert hits[0]['when'] == now[0].astimezone().isoformat()
    assert log.search('unrelated physics') == []
    now[0] += timedelta(days=1)
    assert log.search('FMI') == []
    log.record({'what': 'browser.read', 'where': 'new work', 'device_id': 'mac'})
    rows = [json.loads(line) for line in log.path.read_text().splitlines()]
    assert len(rows) == 1 and rows[0]['where'] == 'new work'


def test_reopening_daily_log_keeps_today_and_ignores_partial_tail(tmp_path):
    now = datetime(2026, 10, 3, tzinfo=timezone.utc)
    log = DailyActivity(tmp_path / 'activity.jsonl', lambda: now)
    log.record({'what': 'file.read', 'where': 'assignment'})
    with log.path.open('a') as out:
        out.write('{broken')
    reopened = DailyActivity(log.path, lambda: now)
    assert len(reopened.search('assignment')) == 1


def test_daemon_redacts_owner_activity_before_daily_storage(daemon):
    token = 'sk-ant-abcdefghijklmnopqrstuvwx'
    daemon.call_soon(daemon.daemon._publish, 'trail.event',
                     {'source':'browser_tab', 'actor':'owner', 'data':{'title':'FMI ' + token, 'host':'mail.google.com'}})
    rows = daemon.daemon.daily_activity.search('FMI')
    assert rows and rows[0]['what'] == 'browser_tab'
    assert rows[0]['data']['host'] == 'mail.google.com' and 'where' not in rows[0]      # stored once (fix list)
    assert token not in daemon.daemon.daily_activity.path.read_text()
    assert rows[0]['actor'] == 'owner' and rows[0]['device_id']


def test_source_correction_and_forget_rewrite_every_mirror_without_other_rows(tmp_path):
    log = DailyActivity(tmp_path / 'activity.jsonl', lambda: datetime.now(timezone.utc))
    for source in ('sensitive', 'keep'):
        log.record({'id': source, 'source': 'window', 'data': {'title': source}, 'where': {'title': source}})
    log.redact_source('sensitive', {'title': '[sensitive]'})
    rows = list(log._rows())
    assert rows[0]['source_id'] == 'sensitive'
    assert rows[0]['data'] == {'title': '[sensitive]'} and 'where' not in rows[0]      # one copy, corrected
    log.forget_sources({'sensitive'})
    assert [r['source_id'] for r in log._rows()] == ['keep']
    assert log.path.stat().st_mode & 0o777 == 0o600


def test_forget_removes_daily_source_and_keep_sources_retains_it(daemon):
    from ferb.sense.trail import Observation
    async def check():
        d = daemon.daemon
        for keep in (True, False):
            event = d.trail.record(Observation('window', {'title': 'forget-source', 'app': 'Example'}))
            item = d.memory.propose('fact', 'owner', f'source-{keep}', 'forget-source', 'observed', [('trail', event.id)])
            d.memory.forget(item.id, keep_sources=keep)
            hits = [r for r in d.daily_activity.search('forget-source') if r.get('source_id') == event.id]
            assert bool(hits) is keep
    daemon.run(check())


def test_indexed_search_matches_a_full_scan_and_follows_appends_and_rewrites(tmp_path, monkeypatch):
    """L28: one indexed lookup per turn; the index reads each appended row once and a rewrite rebuilds it."""
    import ferb.sense.daily as daily
    now = datetime(2026, 10, 3, 10, tzinfo=timezone.utc)
    log = DailyActivity(tmp_path / 'activity.jsonl', lambda: now)
    for i in range(30):
        log.record({'id': f'r{i}', 'source': 'window', 'what': 'window', 'where': {'title': f'tea shop {i % 7} page'}})

    def scan(query, limit=8):
        wanted = daily.terms(query)
        best = []
        for seq, row in enumerate(log._rows()):
            score = len(wanted & daily.terms(json.dumps(row, ensure_ascii=False)))
            if row.get('day') == now.date().isoformat() and score:
                best.append((score, seq, row))
        best.sort(key=lambda hit: hit[:2], reverse=True)
        return [row for _, _, row in best[:max(1, limit)]]
    for query in ('tea shop 3 page', 'window', 'shop 6', 'nothing here'):
        assert log.search(query) == scan(query)
    decoded = []
    real = daily.json.loads
    monkeypatch.setattr(daily.json, 'loads', lambda text, *a, **k: decoded.append(1) or real(text, *a, **k))
    log.search('tea')
    assert len(decoded) <= 8                       # only the hits are decoded again, never every row
    log.record({'id': 'late', 'source': 'window', 'what': 'window', 'where': {'title': 'brand new kettle'}})
    assert [r['source_id'] for r in log.search('kettle')] == ['late']
    log.forget_sources({'late'})
    assert log.search('kettle') == []
