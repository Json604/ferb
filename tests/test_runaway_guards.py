"""The suite's runaway guards (tests/support/runaway.py): a hung test and a memory-growing test each end their pytest
run with every thread's traceback, instead of running for an hour or taking the Mac's memory (2026-10-25)."""

import json
import os
import subprocess
import sys
import textwrap
from pathlib import Path

from tests.conftest import SCRIPTED_DISPATCH_REPEATS, RunningDaemon, chat, install, outcome, sup, task
from tests.support.runaway import MEMORY_EXIT_CODE, memory_bytes

ROOT = Path(__file__).resolve().parents[1]


def run_pytest(tmp_path: Path, body: str, **env: str) -> subprocess.CompletedProcess[str]:
    """A separate pytest run of one test file with only the guard plugin (not the suite's conftest)."""
    (tmp_path / "pytest.ini").write_text("[pytest]\n")
    (tmp_path / "test_case.py").write_text(textwrap.dedent(body))
    environ = {**os.environ, "PYTHONPATH": str(ROOT), **env}
    environ.pop("PYTEST_ADDOPTS", None)
    return subprocess.run([sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", "-p", "tests.support.runaway",
                           "test_case.py"], cwd=tmp_path, env=environ, capture_output=True, text=True, timeout=60)


def test_a_hung_test_ends_the_run_with_every_threads_traceback(tmp_path: Path) -> None:
    r = run_pytest(tmp_path, """
        import time

        def test_hangs():
            time.sleep(60)
        """, FERB_TEST_TIMEOUT_S="1")
    assert r.returncode == 1, (r.returncode, r.stdout, r.stderr)
    assert "Timeout (0:00:01)!" in r.stderr
    assert "test_case.py" in r.stderr and "in test_hangs" in r.stderr     # the traceback names the test
    assert "passed" not in r.stdout


def test_a_long_test_keeps_its_own_limit(tmp_path: Path) -> None:
    r = run_pytest(tmp_path, """
        import time
        import pytest

        @pytest.mark.ferb_timeout(30)
        def test_takes_two_seconds():
            time.sleep(2)

        def test_quick():
            pass
        """, FERB_TEST_TIMEOUT_S="1")
    assert r.returncode == 0, (r.returncode, r.stdout, r.stderr)
    assert "2 passed" in r.stdout


def test_a_growing_test_is_ended_by_the_memory_guard(tmp_path: Path) -> None:
    r = run_pytest(tmp_path, """
        import time

        def test_grows():
            chunks = []
            for _ in range(40):                      # bounded: at most 1 GB if the guard never fires
                chunks.append(b"x" * (25 << 20))     # written, so resident
                time.sleep(0.05)
            time.sleep(10)
        """, FERB_TEST_MAX_RSS_MB="250")
    assert r.returncode == MEMORY_EXIT_CODE, (r.returncode, r.stdout, r.stderr)
    assert "FERB test memory guard" in r.stderr and "250 MB limit" in r.stderr
    assert "test_case.py::test_grows" in r.stderr                          # the guard names the running test
    assert "in test_grows" in r.stderr                                      # and the traceback shows where


def test_memory_reads_this_process() -> None:
    used = memory_bytes()
    assert 10 << 20 < used < 64 << 30


def test_a_scripted_retry_that_goes_off_its_path_ends_the_task(daemon: RunningDaemon) -> None:
    """The attempt loop has no retry cap (the orchestrator decides), so a script whose one choice says "retry" for a
    worker that keeps failing would run forever (2026-10-25, test_supervisor: a 1.7 GB test database). The scripted
    choice runs out instead, and the task ends as a failed choice."""
    failed = json.dumps({"status": "failed", "summary": "It broke.", "artifacts": [], "evidence": []})
    retry = json.dumps({"action": "retry", "worker_type": "default", "instructions": "Again.", "question": ""})
    script = install(daemon, [sup("task", "On it.", task("write x.md", paths=("x.md",)))],
                     {"worker": failed, "worker.dispatch": retry})
    out = outcome(chat(daemon, "write x.md in your workspace", timeout=120))
    assert out["status"] == "failed"
    assert sum(1 for c in script.contexts if c[1] == "worker") == SCRIPTED_DISPATCH_REPEATS + 1
