"""The offline safety suite as the push check runs it: the parallel pass, then the `serial` files alone
(tests/conftest.py _SERIAL). A test that fails is rerun once on its own; when it passes alone it is reported as
"flaky under load" and doesn't fail the run (TEST decision: tests are deterministic; the timer-dependent ones are
being moved to the fake clock, and this retry goes when they are). A test that fails alone fails the run.

    uv run python scripts/run_suite.py            (from the repo root; stdlib only)"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OFFLINE = "not real and not real_claude and not real_api"
CACHE = ROOT / ".pytest_cache" / "v" / "cache" / "lastfailed"


def pytest(*args: str) -> int:
    return subprocess.run([sys.executable, "-m", "pytest", "-q", *args], cwd=ROOT).returncode


def failed() -> list[str]:
    try:
        return sorted(k for k, v in json.loads(CACHE.read_text()).items() if v)
    except (OSError, ValueError):
        return []


def run_pass(name: str, *args: str) -> tuple[bool, list[str]]:
    """(passed, flaky): a failing pass reruns only its failures, serially, once."""
    CACHE.unlink(missing_ok=True)
    if pytest(*args) == 0:
        return True, []
    first = failed()
    if not first:                       # a collection error or an interrupted run: nothing to retry
        print(f"run_suite: the {name} pass failed before any test result", file=sys.stderr)
        return False, []
    print(f"run_suite: rerunning {len(first)} failed test(s) from the {name} pass, alone", file=sys.stderr)
    CACHE.unlink(missing_ok=True)
    if pytest("-p", "no:xdist", *first) == 0:
        return True, first
    return False, []


def main() -> int:
    ok1, flaky1 = run_pass("parallel", "-n", "auto", "-m", f"{OFFLINE} and not serial")
    ok2, flaky2 = run_pass("serial", "-p", "no:xdist", "-m", f"{OFFLINE} and serial") if ok1 else (False, [])
    for t in flaky1 + flaky2:
        print(f"run_suite: flaky under load (failed, then passed alone): {t}", file=sys.stderr)
    return 0 if ok1 and ok2 else 1


if __name__ == "__main__":
    sys.exit(main())
