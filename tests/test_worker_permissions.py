"""W1/W4/D36 shared tools keep identity and checker boundaries."""
import asyncio
from types import SimpleNamespace

from ferb.act.mac.clipboard import MacClipboard
from ferb.act.shell.tools import ShellRun
from ferb.core.types import Identity


def test_shared_shell_and_clipboard_allow_both_identities() -> None:
    assert ShellRun.identities == MacClipboard.identities == {Identity.OWNER, Identity.FERB}
    assert not MacClipboard.on_screen


def test_clipboard_reports_actual_change_count_and_attributes_task() -> None:
    seen = []
    cap = MacClipboard(write=lambda value: 41, on_write=lambda count, task: seen.append((count, task)))
    out = asyncio.run(cap.run(cap.Input(text="owner-directed clipboard"), SimpleNamespace(task_id="task")))
    assert out.content == {"characters": 24, "change_count": 41}
    assert seen == [(41, "task")]


def test_clipboard_failure_does_not_report_a_successful_write() -> None:
    def failed(value: str) -> int:
        raise RuntimeError("the pasteboard did not accept the text")
    import pytest
    with pytest.raises(RuntimeError):
        asyncio.run(MacClipboard(write=failed).run(MacClipboard.Input(text="x"), SimpleNamespace(task_id="task")))


import pytest
from tests.conftest import FakeCap, gateway_request, gateway_setup, gateway_tool_call

@pytest.mark.parametrize('identity', [Identity.OWNER, Identity.FERB])
def test_gateway_refuses_other_identity_even_when_scopes_match(daemon, identity):
    cap = FakeCap('probe.other_identity', 'read', {'probe'})
    cap.identities = {Identity.FERB if identity is Identity.OWNER else Identity.OWNER}
    grant, _ = gateway_setup(daemon, {'probe'}, gateway_request('Read this.'), [cap], identity=identity)
    assert cap.name not in {tool['name'] for tool in daemon.daemon.gateway.tools_for(grant)}
    out = gateway_tool_call(daemon, grant, cap.name)
    assert out['is_error'] and 'identity' in out['content']['error'] and cap.calls == 0
