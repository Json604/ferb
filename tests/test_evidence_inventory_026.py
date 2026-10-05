"""Inventory and history reads must disclose every candidate through explicit pages."""

from ferb.cli.client import Client
from ferb.policy import services
from ferb.sense.trail import Observation


def test_environment_does_not_filter_installed_apps_by_request_words():
    env = services.Environment([('example.strange', 'अनाम'), ('com.apple.Notes', 'Notes')], [])
    stable, live = services.summary(env, '26', {'example.strange'})
    assert 'example.strange' in stable and 'अनाम' in stable and 'Notes' in stable
    assert 'example.strange' in live


def test_trail_filters_before_paging_and_exposes_cursor(daemon):
    async def check():
        trail = daemon.daemon.trail
        trail.record(Observation('browser_tab', {'host': 'wanted.test', 'title': 'old match'}))
        for _ in range(501):
            trail.record(Observation('browser_tab', {'host': 'other.test', 'title': 'other'}))
        trail.record(Observation('browser_tab', {'host': 'wanted.test', 'title': 'new match'}))
        first = trail.query(since='2000-01-01', host='wanted.test', limit=1)
        assert first['events'][0]['data']['title'] == 'new match'
        second = trail.query(since='2000-01-01', host='wanted.test', limit=1, before_seq=first['next_before_seq'])
        assert second['events'][0]['data']['title'] == 'old match'
        assert second['next_before_seq'] is None
    daemon.run(check())


def test_timeline_never_silently_drops_old_rows(daemon):
    async def seed():
        for i in range(520):
            daemon.daemon._journal().append('test.evidence', {'i': i})
    daemon.run(seed())
    client = Client(daemon.sock)
    entries = client.call('timeline.get')['entries']
    assert len([e for e in entries if e['kind'] == 'test.evidence']) == 520
    client.close()
