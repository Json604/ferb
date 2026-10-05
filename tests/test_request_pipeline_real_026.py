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
