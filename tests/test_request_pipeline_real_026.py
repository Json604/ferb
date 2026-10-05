"""Real model/RPC proof of full pasted source, artifact verification and task sentinel on disposable files."""

import json
import tempfile
from pathlib import Path

import pytest

from ferb.act.fs import files
from ferb.cli.client import Client
from ferb.clock.clock import SystemClock
from tests.conftest import RunningDaemon, only_tools

pytestmark = pytest.mark.real


def test_full_pasted_request_reaches_real_worker_and_checker(monkeypatch):
    with tempfile.TemporaryDirectory(prefix='ferb-request-026-') as directory:
        base = Path(directory).resolve()
        original = files.resolve

        def disposable_only(path, ctx, must_exist):
            candidate = files.owner_path(path, ctx.workspace).resolve()
            if not candidate.is_relative_to(base):
                raise files.PathRefused('Disposable test scope: this path is outside the fixture')
            return original(path, ctx, must_exist)

        monkeypatch.setattr(files, 'resolve', disposable_only)
        daemon = RunningDaemon(base / 'home', clock=SystemClock(), watch_owner_input=True)
        only_tools(daemon, 'fs.', 'task.evidence', 'actions.search')
        source = '\n'.join(f'Lesson {i:03d}: Preserve this exact Unicode source — विषय {i}.' for i in range(140))
        source += '\nFINAL_DISTINGUISHING_REQUIREMENT: content and source, not just the filename.\n'
        request = ('Create pipeline/source.txt in your workspace containing exactly the full text inside this fence, '
                   'including the final newline. Inspect the saved file to verify every line and the final requirement. '
                   'Only use the supplied disposable filesystem tools; no browser, account, personal file or native app. '
                   'Work entirely in the background.\n```text\n' + source + '```')
        client = Client(daemon.sock, timeout=900)
        try:
            events = list(client.stream('chat.send', {'session_id': 'disposable-026', 'text': request}))
            result = next(e for e in events if e['type'] == 'outcome')
            assert result['status'] == 'done' and result['verified'], result
            artifact = daemon.daemon.home / 'workspace/pipeline/source.txt'
            assert artifact.read_text() == source
            trace_client = Client(daemon.sock)
            trace = trace_client.call('trace.get', {'task_id': result['task_id']})
            trace_client.close()
            assert 'FINAL_DISTINGUISHING_REQUIREMENT' in trace['outcome']['request']['text']
            assert trace['outcome']['verification']
            assert trace['outcome']['coverage']['sentinel_recorded']
            receipt = {'task_id': result['task_id'], 'verified': result['verified'], 'source_bytes': len(source.encode()),
                       'exact_saved_source': True, 'planner_recorded': bool(trace['outcome']['planner_decisions']),
                       'sentinel': [dict(kind=s['kind'], **(s.get('output') or {}))
                                    for s in trace['outcome']['disturbances']],
                       'scope': 'Isolated daemon and filesystem-fenced fixture; real Codex planner/worker/checker',
                       'cleanup': trace['outcome']['cleanup']}
            Path('/private/tmp/ferb-026-request-live.json').write_text(json.dumps(receipt, indent=2))
        finally:
            client.close()
            daemon.stop()


def test_large_deferred_registry_is_discovered_and_stale_source_is_not_current():
    """Real provider, synthetic read-only sources: exercise discovery plus honest stopping, no account access."""
    from tests.conftest import FakeCap
    from ferb.core.types import RiskLevel
    from ferb.policy.rules import Kind
    with tempfile.TemporaryDirectory(prefix='ferb-discovery-0261-') as directory:
        daemon = RunningDaemon(Path(directory)/'home',clock=SystemClock(),watch_owner_input=True)
        only_tools(daemon,'actions.search','task.evidence')
        # A large MCP inventory exercises deferred discovery without exposing any real action surface.
        caps = daemon.daemon.registry.capabilities
        for i in range(220):
            cap=FakeCap(f'fixture.unrelated_{i:03d}',Kind.READ,{'fixture:read'},content={'fixture':True})
            cap.risk,cap.reversible=RiskLevel.LOW,True
            caps[cap.name]=cap
        source=FakeCap('fixture.primary_feed',Kind.READ,{'fixture:read'},content={
            'primary_feed':{'available':False,'error':'Fixture source disconnected; no current inventory read'},
            'archive':{'title':'Old item','observed_at':'2026-10-02T10:00:00+05:30','due_date':'2026-10-06'},
            'linked_page':{'read_now':True,'title':'Old item','due_date':'2026-10-06'},
            'coverage':'Archive and current old-item page only; newer feed entries are unknown'})
        source.risk,source.reversible=RiskLevel.LOW,True
        caps[source.name]=source
        client=Client(daemon.sock,timeout=900)
        try:
            result=next(e for e in client.stream('chat.send',{'session_id':'fixture-discovery',
                'text':'Find the latest item in the primary feed using fixture.primary_feed and report its name and due date. All available tools are synthetic read-only fixtures.'}) if e['type']=='outcome')
            assert source.calls>0, result
            assert not result['verified'] and result['status']!='done', result
            assert result['stop_reason'], result
            trace_client=Client(daemon.sock)
            try:
                trace=trace_client.call('trace.get',{'task_id':result['task_id']})['outcome']
            finally:
                trace_client.close()
            assert trace['runtime_tools']
            Path('/private/tmp/ferb-0261-discovery-live.json').write_text(json.dumps({
                'task_id':result['task_id'],'status':result['status'],'verified':result['verified'],
                'stop_reason':result['stop_reason'],'source_calls':source.calls,'synthetic_tools':221,
                'runtime_tools_recorded':len(trace['runtime_tools']),
                'disturbances':[r['output'] for r in trace['disturbances'] if r['kind']=='disturbance.summary'],
                'disturbance_records':[{'at':r['started_at'],'kind':r['kind'],'facts':r['output']}
                                       for r in trace['disturbances']] },indent=2))
        finally:
            client.close();daemon.stop()
