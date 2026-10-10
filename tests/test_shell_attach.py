"""P22 the owner watches the same PTY and holds exclusive input while attached."""
import asyncio
from pathlib import Path
import pytest
from ferb.act.shell.manager import ShellManager, ProcHandle, ProcInfo
from ferb.core.errors import Refused


def test_owner_input_lease_excludes_worker_and_stale_owner(tmp_path):
    class Pty:
        exit_code = None
        sent = []
        def send(self, data): self.sent.append(data)
    manager = ShellManager(tmp_path, lambda text: text)
    pty = Pty()
    manager._procs['p-1'] = (pty, ProcInfo('p-1', 'task', 'agent', str(tmp_path), 0, True, None, False))
    lease = manager.take_input('p-1')
    with pytest.raises(Refused): manager.take_input('p-1')
    with pytest.raises(Refused): asyncio.run(manager.send(ProcHandle('p-1', 'task'), 'worker'))
    with pytest.raises(Refused): manager.owner_send('p-1', 'wrong-lease', 'stale')
    manager.owner_send('p-1', lease, 'owner')
    manager.release_input('p-1', lease)
    asyncio.run(manager.send(ProcHandle('p-1', 'task'), 'worker'))
    assert pty.sent == ['owner', 'worker']
