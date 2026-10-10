"""D31 regressions: a single exact grant, separated read/write lifts and hidden secret lifetime."""
from pathlib import Path

from ferb.act.shell.sandbox import Sandbox
from ferb.core.types import EscapeGrant
from ferb.policy.escape import escape_key
from ferb.policy.protection import Protection
from ferb.sanitize.redact import Redactor


def test_exact_escape_key_preserves_command_and_resolved_cwd(tmp_path: Path) -> None:
    assert escape_key("trustd", "echo a", str(tmp_path)) != escape_key("trustd", "echo b", str(tmp_path))
    assert escape_key("trustd", "echo a", str(tmp_path)) != escape_key("admin", "echo a", str(tmp_path))
    assert "secret" not in repr(EscapeGrant("admin", "c", "echo a", str(tmp_path), "secret"))


def test_temporary_short_secrets_preserve_spaces_and_concurrent_leases() -> None:
    r = Redactor()
    r.retain_secret(" a ")
    r.retain_secret(" a ")
    r.release_secret(" a ")
    stream = r.stream()
    assert b" a " not in stream.feed(b"prefix a", final=False) + stream.feed(b" suffix", final=True)
    assert r.text("prefix a suffix") == "prefix[redacted]suffix"
    r.release_secret(" a ")
    assert r.text(" a ") == " a "


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
    from types import SimpleNamespace
    import pytest
    from ferb.core.errors import Refused
    from tests.conftest import Answers, gateway_request, gateway_setup
    grant, _ = gateway_setup(daemon, {'shell:run'}, gateway_request('Run my command.'), [])
    args = SimpleNamespace(escape='trustd', command='echo approved', cwd=str(tmp_path))
    gateway = daemon.daemon.gateway
    rec = daemon.daemon.tasks.get(grant.task_id)
    answers = Answers(daemon, choice=2)
    try:
        with pytest.raises(Refused):
            daemon.run(gateway._escape(grant, rec, args, 'needed for the exact command'))
        with pytest.raises(Refused, match='already said no'):
            daemon.run(gateway._escape(grant, rec, args, 'same command'))
    finally:
        answers.stop()
    assert len(answers.asked) == 1
