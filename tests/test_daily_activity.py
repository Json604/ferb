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
    assert rows[0]['where']['host'] == 'mail.google.com'
    assert token not in daemon.daemon.daily_activity.path.read_text()
    assert rows[0]['actor'] == 'owner' and rows[0]['device_id']
