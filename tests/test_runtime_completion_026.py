"""A provider process's partial report cannot masquerade as a completed model turn."""

import asyncio
import json
from pathlib import Path
import sys

import pytest

from ferb.core.types import RunHandle
from ferb.runtimes.codex_exec import CodexRuntime


@pytest.mark.anyio
async def test_provider_nonzero_exit_after_report_is_an_error(tmp_path: Path):
    event = json.dumps({'type': 'item.completed', 'item': {'type': 'agent_message', 'text': 'success claim'}})
    proc = await asyncio.create_subprocess_exec(sys.executable, '-c',
            f'import sys; print({event!r}, flush=True); sys.exit(7)',
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    runtime = CodexRuntime(Path('/unused'), 'worker', tmp_path, tmp_path, timeout_s=None)
    handle = RunHandle('test', runtime.name, 'test.worker', {'proc': proc, 'violation': None, 'seen': set()})
    events = [event async for event in runtime.events(handle)]
    assert any(event.kind == 'error' and '7' in event.data['message'] for event in events)
    assert not any(event.kind == 'final' for event in events)
