"""Fixes from the independent audit (2026-10-02, branch rework/human-control): each test pins one verified defect."""

import asyncio
import json
import os
from pathlib import Path
from typing import Any

import pytest

from ferb.act.mac import applescript
from ferb.act.shell.manager import ShellManager
from ferb.core.errors import Refused


def test_do_script_cannot_reach_terminal() -> None:
    """`do script` ran any shell command through Terminal, bypassing the shell guard (Terminal is scriptable)."""
    with pytest.raises(Refused):
        applescript.wrap("com.apple.Terminal", 'do script "rm -rf ~"')
    with pytest.raises(Refused):
        applescript.wrap("com.apple.Terminal", 'tell window 1\n  do  script "ls"\nend')


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
        h = await m.start(["/bin/sh", "-c", "i=0; while [ $i -lt 3000 ]; do echo line$i; i=$((i+1)); done"],
                          tmp_path, "t1")
        await m.wait(h, 10)
        text, off, total = await m.read(h, 0, 100)
        assert text.startswith("line0") and off == 100 and total > 20000
        text2, off2, _ = await m.read(h, off, 100)
        assert off2 == 200 and not text2.startswith(text[:5])
        whole, end, total = await m.read(h)
        assert end == total and whole.rstrip().endswith("line2999")
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


def test_ledger_done_ignores_the_planners_free_text_expected(tmp_path: Path) -> None:
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
    checks = Checks(ledger, lambda t: [], _no_account)
    cond = Condition(text="liked", check=Check(kind="ledger_done", path=None, text=None, url_contains=None,
                                                 site=None, expected="Spotify like current track"))
    ctx = ToolContext("task1", "verifier", "r", Identity.OWNER, frozenset(), "dev", None, str(tmp_path), (), ())
    r = asyncio.run(checks.run(0, cond, "task1", ctx))
    assert r.met is True and "done" in r.evidence


async def _no_account(task_id: str, site: str) -> str | None:
    return None


def test_extension_wait_and_backoff_are_bounded() -> None:
    """background.js: `wait` looped forever after the daemon gave up, and the reconnect backoff reset itself."""
    js = Path("extensions/chromium/background.js").read_text()
    assert "while (Date.now() < deadline)" in js and 'return { met: false, detail: "timeout" }' in js
    assert "retryMs = 1000; return;" in js                       # only once the daemon answered (welcome)
    assert js.count("retryMs = 1000") == 2                       # the declaration and the welcome reset
    assert json.loads(Path("extensions/chromium/manifest.json").read_text())["version"] >= "0.6.7"


def test_worker_prompt_matches_the_gateway_on_approvals() -> None:
    md = Path("ferb/orchestrator/prompts/worker.md").read_text()
    assert "call the exact same tool again" in md and "wait for nothing" not in md


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
