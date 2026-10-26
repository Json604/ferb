"""Fixes from the independent audit (2026-10-02, branch rework/human-control): each test pins one verified defect."""

import asyncio
import json
import os
import shlex
from pathlib import Path
from typing import Any

import pytest

from ferb.act.mac import applescript
from ferb.act.shell.manager import ShellManager
from ferb.core.errors import Refused


def test_applescript_script_content_is_not_classified_by_keywords():
    body = 'tell window 1\n  do script "ls"\nend'
    assert body in applescript.wrap("com.apple.Terminal", body)


def test_codex_run_token_never_appears_on_the_command_line(tmp_path: Path) -> None:
    from ferb.runtimes.codex_exec import CodexRuntime
    rt = CodexRuntime(tmp_path / "codex", "worker", tmp_path / "wd", tmp_path, shim_cmd=["ferb", "mcp"],
                      tool_names=["mac.act"])
    token_file = tmp_path / "run" / "token-x"
    cmd = rt._cmd(None, token_file)
    joined = " ".join(cmd)
    assert "FERB_RUN_TOKEN_FILE" in joined and str(token_file) in joined
    assert 'FERB_RUN_TOKEN="' not in joined
    with pytest.raises(ValueError):
        rt._cmd(None, None)


def test_the_mcp_shim_reads_the_token_from_a_file(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.cli.mcp import Shim
    f = tmp_path / "tok"
    f.write_text("secret-token\n")
    monkeypatch.delenv("FERB_RUN_TOKEN", raising=False)
    monkeypatch.setenv("FERB_RUN_TOKEN_FILE", str(f))
    assert Shim()._token == "secret-token"
    monkeypatch.setenv("FERB_RUN_TOKEN_FILE", str(tmp_path / "missing"))
    with pytest.raises(SystemExit):
        Shim()


def test_shell_output_pages_from_the_start(tmp_path: Path) -> None:
    """`shell.read` always returned the LAST 8000 chars: the start of a long output was unreachable."""
    from ferb.act.shell import tools as st

    async def run() -> None:
        m = ShellManager(tmp_path, lambda s: s)
        h = await m.start("i=0; while [ $i -lt 50000 ]; do echo line$i; i=$((i+1)); done", tmp_path, "t1")
        await m.wait(h, 10)
        text, off, total = await m.read(h, 0, 100)
        assert text.startswith("line0") and off == 100 and total > 400000
        text2, off2, _ = await m.read(h, off, 100)
        assert off2 == 200 and not text2.startswith(text[:5])
        whole, end, total = await m.read(h)
        assert end == total and whole.rstrip().endswith("line49999")
    asyncio.run(run())
    assert st.TAIL == 8000


def test_shell_path_includes_the_owners_tool_folders(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Under launchd PATH is bare (/usr/bin:/bin:...); brew, node, uv, gh were never found."""
    monkeypatch.setenv("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
    env = ShellManager(tmp_path, lambda s: s)._env()
    parts = env["PATH"].split(":")
    assert parts[-4:] == ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
    for extra in ("/opt/homebrew/bin", "/usr/local/bin"):
        if os.path.isdir(extra):
            assert extra in parts[:-4]


def test_completed_ledger_rows_do_not_prove_the_requested_goal(tmp_path: Path) -> None:
    """The planner wrote "Spotify like current track" in `expected`; the code compared it to the ledger's capability
    name and reported a succeeded task as failed (run 34ca95b3, 2 Oct 08:41)."""
    from ferb.clock.clock import FakeClock
    from ferb.core.ids import SequentialIds
    from ferb.core.types import Identity, ToolContext
    from ferb.orchestrator.ledger import Ledger
    from ferb.orchestrator.schemas import Check, Condition
    from ferb.orchestrator.verify import Checks
    from ferb.store.db import Database
    db = Database(tmp_path / "t.db")
    db.migrate()
    ledger = Ledger(db, FakeClock(), SequentialIds("a"))
    row = ledger.intend("task1", "mac.click", {"site": "com.spotify.client"}, "h")
    ledger.mark(row.id, "executing")
    ledger.mark(row.id, "done", ["span"])
    checks = Checks(lambda t: [])                       # a done ledger row exists; the checks never read it
    cond = Condition(text="liked", check=Check(kind="none", path=None, text=None, url_contains=None,
                                                 site=None, expected="Spotify like current track"))
    ctx = ToolContext("task1", "verifier", "r", Identity.OWNER, frozenset(), "dev", None, str(tmp_path), (), ())
    r = asyncio.run(checks.run(0, cond, "task1", ctx))
    assert not checks.code_checkable(cond, "task1")     # ledger rows never decide; the checker reads the outcome
    assert r.met is None


def test_extension_wait_and_backoff_are_bounded() -> None:
    """background.js: `wait` looped forever after the daemon gave up, and the reconnect backoff reset itself."""
    js = Path("extensions/chromium/background.js").read_text()
    assert "while (Date.now() < deadline)" in js and 'return { met: false, detail: "timeout" }' in js
    from tests.chromelab import worker
    result = worker('''(async () => {
      retryMs = 60000; refused = 'old version';
      await onDaemon({type: 'welcome', gen: 4});
      return {retryMs, refused, gen};
    })()''')
    assert result == {'retryMs': 1000, 'refused': None, 'gen': 4}
    from ferb.act.browser.ext_protocol import version_tuple
    assert version_tuple(json.loads(Path("extensions/chromium/manifest.json").read_text())["version"]) >= (0, 6, 7)


def test_worker_prompt_matches_the_gateway_on_approvals() -> None:
    md = Path("ferb/orchestrator/prompts/worker.md").read_text()
    assert "owner_ask goes to the orchestrator first" in md and "concrete reviewable" in md


@pytest.mark.parametrize('role', ['supervisor.md', 'worker.md', 'dispatch.md', 'question.md', 'verifier.md', None])
def test_every_task_system_prompt_carries_the_same_interaction_contract(role):
    from types import SimpleNamespace
    from ferb.orchestrator.supervisor import Supervisor
    host = SimpleNamespace(_approach=lambda: 'orchestrator/prompts', facts=lambda: 'FERB profile is available.',
                           availability=lambda: SimpleNamespace(text='Installed actions'),
                           identity_context=lambda: 'Owner identity and rules.', skills_catalog=lambda: [],
                           abilities_catalog=lambda: [])
    system = ' '.join(Supervisor._system(host, role).split())
    assert 'Shared interaction contract' in system
    assert 'shell' in system and 'AppleScript' in system
    assert 'Only an owner-directed foreground interaction' in system
    assert 'Never guess a profile directory' in system
    assert 'that direction covers its necessary actions and retries' in system
    assert 'do not ask again for each step or worker switch' in system
    assert 'FERB profile is available.' in system
    assert 'Owner identity and rules.' in system


def test_stats_reads_outcomes_and_errors_from_the_database(tmp_path: Path) -> None:
    import io
    from ferb.cli.admin import stats
    from ferb.store.db import Database
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    now = "2099-01-01T00:00:00+00:00"
    db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, "
                    "updated_at) VALUES ('t1','t1','r','g','owner','mixed','[]','done',?,?)", (now, now))
    db.conn.execute("INSERT INTO tasks (id, tree_id, request_id, goal, identity, role, scopes_json, status, created_at, "
                    "updated_at) VALUES ('t2','t2','r','g','owner','mixed','[]','failed',?,?)", (now, now))
    db.conn.execute("INSERT INTO spans (id, kind, started_at, input_json, error) VALUES ('s1','tool.call',?,'{}',"
                    "'blocked by FERB policy: x')", (now,))
    db.conn.commit()
    out = io.StringIO()
    assert stats(tmp_path, out, days=36500) == 0
    text = out.getvalue()
    assert "done 1 (50%)" in text and "blocked by FERB policy: x" in text
    assert stats(tmp_path / "nowhere", io.StringIO()) == 1


def test_watch_harness_records_owner_verdict_next_to_the_detector(tmp_path: Path) -> None:
    """`ferb watch`: the owner's answer is the verdict; the detector's observations sit beside it; a failing step is
    a finding, not a crash. (The real steps act on TextEdit / Calculator / a throwaway Chrome: the owner runs them.)"""
    import io
    from ferb.device import watch
    answers = iter(["", "s", "o", "the dock bounced"])
    det = watch.Detector(tmp_path / "events.jsonl")
    try:
        sess = watch.Session(io.StringIO(), lambda prompt: next(answers), det, "com.example.owner")
        ok = sess.do(watch.Step("X1", "a quiet step", lambda: {"landed": True}))
        moved = sess.do(watch.Step("X2", "a step the owner saw", lambda: {"landed": False}))
        boom = sess.do(watch.Step("X3", "a step that errors", lambda: (_ for _ in ()).throw(RuntimeError("nope"))))
    finally:
        det.stop()
    assert ok["owner_saw"] == "nothing" and ok["result"] == {"landed": True} and "detector" in ok
    assert moved["owner_saw"] == "desktop switched"
    assert boom["owner_saw"] == "the dock bounced" and boom["error"].startswith("RuntimeError")
    assert [r["id"] for r in sess.results] == ["X1", "X2", "X3"]


def test_watch_dry_run_lists_steps_without_touching_anything(tmp_path: Path) -> None:
    import io
    from ferb.device import watch
    out = io.StringIO()
    assert watch.run(tmp_path, out, ask=lambda p: "", only="M4,B5", dry_run=True) == 0
    text = out.getvalue()
    assert "M4" in text and "B5" in text and "M1" in text and "B12" in text      # setup / teardown added
    assert "M9" not in text


def test_shell_log_and_pages_redact_split_secrets_before_persistence(tmp_path: Path) -> None:
    import stat
    import sys
    from ferb.sanitize.redact import Redactor

    async def run() -> None:
        redactor = Redactor()
        redactor.add_secret_value('test-private-secret')
        m = ShellManager(tmp_path, redactor.text, stream_factory=redactor.stream)
        code = "import os,time; os.write(1,b'first\\ntest-private-'); time.sleep(.1); os.write(1,'secret\\nlast café\\n'.encode())"
        h = await m.start(shlex.join([sys.executable, '-c', code]), tmp_path, 't')
        assert await m.wait(h, 10) == 0
        text, end, total = await m.read(h)
        assert 'test-private-' not in text and '[redacted]' in text and 'café' in text
        assert end == total
        log = tmp_path / f'{h.id}.log'
        assert log.read_bytes().decode() == text
        assert stat.S_IMODE(log.stat().st_mode) == 0o600
        pages, offset = [], 0
        while offset < total:
            chunk, offset, _ = await m.read(h, offset, 1)
            pages.append(chunk)
        assert ''.join(pages) == text
    asyncio.run(run())


def test_failed_shell_start_closes_pty_descriptors(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.shell import pty_proc
    opened = []
    real_open = pty_proc.pty.openpty

    def openpty():
        pair = real_open()
        opened.extend(pair)
        return pair

    monkeypatch.setattr(pty_proc.pty, 'openpty', openpty)
    # The process that fails to start is the sandbox's launcher; its profile file goes too (F5a Task 2)
    from ferb.act.shell import sandbox
    from ferb.policy.protection import Protection
    monkeypatch.setattr(sandbox, 'SANDBOX_EXEC', '/no/such/ferb-test-executable')
    (tmp_path / 'h' / '.ferb' / 'workspace').mkdir(parents=True)
    sb = sandbox.Sandbox(Protection(tmp_path / 'h', tmp_path / 'h' / '.ferb', [], [], []), tmp_path / 'profiles')
    m = ShellManager(tmp_path, lambda text: text, sandbox=sb)
    with pytest.raises(FileNotFoundError):
        asyncio.run(m.start('true', tmp_path, 't'))
    assert not m.list() and not list((tmp_path / 'profiles').glob('*.sb'))
    for fd in opened:
        with pytest.raises(OSError):
            os.fstat(fd)


def test_shell_stream_shows_an_interactive_prompt_before_exit(tmp_path: Path) -> None:
    import sys
    from ferb.sanitize.redact import Redactor

    async def run() -> None:
        redactor = Redactor()
        manager = ShellManager(tmp_path, redactor.text, redactor.stream)
        handle = await manager.start(shlex.join([sys.executable, '-c',
                                                 "answer = input('Continue? '); print('received', answer)"]),
                                     tmp_path, 'prompt')
        try:
            async with asyncio.timeout(3):
                while True:
                    output, _, _ = await manager.read(handle)
                    if 'Continue? ' in output:
                        break
                    await asyncio.sleep(.01)
            assert manager.info(handle).running
            await manager.send(handle, 'yes\n')
            assert await manager.wait(handle, 3) == 0
            output, _, _ = await manager.read(handle)
            assert 'received yes' in output
        finally:
            await manager.kill(handle)
    asyncio.run(run())


def test_shell_redacts_registered_multiline_values_after_pty_transport(tmp_path: Path) -> None:
    import sys
    from ferb.sanitize.redact import Redactor
    secret = 'synthetic-first\nsynthetic-second'

    async def run() -> None:
        redactor = Redactor()
        redactor.add_secret_value(secret)
        manager = ShellManager(tmp_path, redactor.text, redactor.stream)
        handle = await manager.start(shlex.join([sys.executable, '-c',
                                                 "import os; os.write(1,b'synthetic-first\\nsynthetic-second\\n')"]),
                                     tmp_path, 'multiline')
        assert await manager.wait(handle, 3) == 0
        output, _, _ = await manager.read(handle)
        assert '[redacted]' in output and 'synthetic-first' not in output and 'synthetic-second' not in output
    asyncio.run(run())


def test_shell_capture_failure_reports_error_and_finishes(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from ferb.act.shell import pty_proc

    async def run() -> None:
        manager = ShellManager(tmp_path, lambda text: text)
        handle = await manager.start('echo captured', tmp_path, 'capture')
        process = manager._get(handle)
        real_write = pty_proc.os.write

        def broken(fd, data):
            if fd == process._log_fd:
                raise OSError(28, 'synthetic disk full')
            return real_write(fd, data)

        monkeypatch.setattr(pty_proc.os, 'write', broken)
        try:
            assert await manager.wait(handle, 3) is not None
            assert 'capture' in manager.info(handle).capture_error
            assert process._log_fd == process._master == -1
        finally:
            await manager.kill(handle)
    asyncio.run(run())
