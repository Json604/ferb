"""The suite's runaway guards (tests/support/runaway.py): a hung test and a memory-growing test each end their pytest
run with every thread's traceback, instead of running for an hour or taking the Mac's memory (2026-10-25)."""

import os
import subprocess
import sys
import textwrap
from pathlib import Path

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
