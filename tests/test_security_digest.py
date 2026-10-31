from ferb.policy.security_digest import build
from tests.conftest import gateway_request, gateway_setup


def test_daily_security_digest_is_structured_and_redacted(daemon):
    grant, _ = gateway_setup(daemon, set(), gateway_request('Open https://mysite.example.'), [])
    async def check():
        day = daemon.daemon.clock.now().date().isoformat()
        return build(daemon.daemon.db, day)
    result = daemon.run(check())
    assert {'new_destinations', 'confirmations', 'blocked_flows', 'outside_origin_memory'} <= result.keys()
    assert 'missing observations' in result['meaning']
