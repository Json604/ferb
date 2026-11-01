from datetime import datetime, timezone

from ferb.sense.daily import DailyActivity


def test_daemon_redacts_owner_activity_before_daily_storage(daemon):
    token = 'sk-ant-abcdefghijklmnopqrstuvwx'
    daemon.call_soon(daemon.daemon._publish, 'trail.event',
                     {'source':'browser_tab', 'actor':'owner', 'data':{'title':'FMI ' + token, 'host':'mail.google.com'}})
    rows = daemon.daemon.daily_activity.search('FMI')
    assert rows and rows[0]['what'] == 'browser_tab'
    assert rows[0]['data']['host'] == 'mail.google.com' and 'where' not in rows[0]      # stored once (fix list)
    assert token not in daemon.daemon.daily_activity.path.read_text()
    assert rows[0]['actor'] == 'owner' and rows[0]['device_id']


# decision: MEM-2
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


# decision: MEM-2
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
