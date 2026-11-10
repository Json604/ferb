"""D31 regressions: a single exact grant and separated read/write lifts."""
from pathlib import Path

from ferb.act.shell.sandbox import Sandbox
from ferb.core.types import EscapeGrant
from ferb.policy.escape import escape_key
from ferb.policy.protection import Protection


def test_exact_escape_key_preserves_command_and_resolved_cwd(tmp_path: Path) -> None:
    assert escape_key("trustd", "echo a", str(tmp_path)) != escape_key("trustd", "echo b", str(tmp_path))
    assert escape_key("trustd", "echo a", str(tmp_path)) != escape_key("admin", "echo a", str(tmp_path))
    assert "secret" not in repr(EscapeGrant("admin", "c", "echo a", str(tmp_path), "secret"))


def test_path_lifts_do_not_lift_the_other_access_or_ferb_state(tmp_path: Path) -> None:
    p = Protection(tmp_path, tmp_path / ".ferb", ["~/.ssh"], ["~/.ssh"], [])
    sb = Sandbox(p, tmp_path / "profiles", listeners=lambda: set())
    read = sb.profile("read:~/.ssh")
    assert f'(deny file-write* (subpath "{tmp_path}/.ssh"))' in read
    write = sb.profile("write:~/.ssh")
    assert f'(deny file-read* (subpath "{tmp_path}/.ssh"))' in write
    assert f'(allow file-write* file-read-metadata (subpath "{tmp_path}/.ssh"))' in write
    assert "require-not" in write


def test_admin_nopasswd_discards_password_pipe_and_keeps_proxy_env(tmp_path, monkeypatch) -> None:
    import asyncio
    import subprocess
    from ferb.act.shell.pty_proc import PtyProc
    original = subprocess.Popen
    seen = []
    def fake_sudo(argv, **kw):
        seen.append(argv)
        assert argv[:9] == ['/usr/bin/sudo', '-S', '-k', '-p', '', '--', '/usr/bin/sandbox-exec', '-f', 'profile']
        # A NOPASSWD launcher deliberately reads no bytes from stdin. Execute the fixed contained supervisor.
        return original(argv[9:], **kw)
    monkeypatch.setattr(subprocess, 'Popen', fake_sudo)
    async def run():
        p = PtyProc(['/usr/bin/sandbox-exec', '-f', 'profile', '/bin/sh', '-c',
                     'read -r answer; printf "%s:%s" "$answer" "$HTTPS_PROXY"'], tmp_path,
                    {'PATH': '/usr/bin:/bin', 'HTTPS_PROXY': 'http://proxy'}, tmp_path/'admin.log', str,
                    admin_secret=' secret with spaces ')
        await p.start()
        p.send('owner input\n')
        assert await p.wait(5) == 0
        text, _, _ = p.read()
        assert 'owner input:http://proxy' in text and 'secret with spaces' not in text
        assert all('secret with spaces' not in arg for arg in seen[0])
    asyncio.run(run())


def test_admin_supervisor_stops_term_ignoring_command(tmp_path, monkeypatch) -> None:
    import asyncio
    import subprocess
    from ferb.act.shell.pty_proc import PtyProc
    original = subprocess.Popen
    monkeypatch.setattr(subprocess, 'Popen', lambda argv, **kw: original(argv[9:], **kw))
    async def run():
        p = PtyProc(['/usr/bin/sandbox-exec', '-f', 'profile', '/bin/sh', '-c',
                     "trap '' TERM; printf ready; sleep 30"], tmp_path, {'PATH': '/usr/bin:/bin'},
                    tmp_path/'term.log', str, admin_secret='private')
        await p.start()
        await asyncio.sleep(.1)
        await p.kill()
        assert p.exit_code is not None
    asyncio.run(run())


def test_read_lift_does_not_reopen_immutable_ferb_directory(tmp_path):
    p = Protection(tmp_path, tmp_path/'.ferb', ['~'], [], [])
    text = Sandbox(p, tmp_path/'profiles', listeners=lambda: set()).profile('read:~')
    assert '(require-all' in text and '(require-not' in text
    assert str(tmp_path/'.ferb/workspace/uploads') in text


def test_escape_no_is_final_for_exact_command_and_cwd(daemon, tmp_path):
    """Through the gateway's one authorization step (step 6b): the owner's No to an exact escape command is final
    for that command and folder in the task; asked once."""
    from tests.conftest import Answers, gateway_request, gateway_setup, gateway_tool_call
    grant, _ = gateway_setup(daemon, {'shell'}, gateway_request('Run my command.'), [])
    answers = Answers(daemon, choice=2)
    try:
        r = gateway_tool_call(daemon, grant, 'shell.run', command='echo approved', cwd=str(tmp_path), escape='trustd',
                              why='needed for the exact command')
        again = gateway_tool_call(daemon, grant, 'shell.run', command='echo approved', cwd=str(tmp_path),
                                  escape='trustd', why='same command')
    finally:
        answers.stop()
    assert r['is_error'] and 'said no' in r['content']['error']
    assert again['is_error'] and 'already said no' in again['content']['error']
    assert len(answers.asked) == 1


def test_a_never_lifted_escape_is_refused_without_a_question(daemon, tmp_path):
    from tests.conftest import Answers, gateway_request, gateway_setup, gateway_tool_call
    grant, _ = gateway_setup(daemon, {'shell'}, gateway_request('Run my command.'), [])
    answers = Answers(daemon, choice=1)
    try:
        r = gateway_tool_call(daemon, grant, 'shell.run', command='ls', cwd=str(tmp_path), escape='ferb_state')
        missing = gateway_tool_call(daemon, grant, 'shell.run', command='ls', cwd=str(tmp_path / 'nope'),
                                    escape='trustd')
    finally:
        answers.stop()
    assert r['is_error'] and missing['is_error'] and 'does not exist' in missing['content']['error']
    assert answers.asked == []
