"""Live (owner-run, billed): two calls with the same long stable prefix; the second reads the provider's cache (P20)."""
import asyncio
import shutil
import tempfile
from pathlib import Path

import pytest

from ferb.clock.clock import SystemClock
from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.keys import has_api_key
from tests.conftest import RunningDaemon

pytestmark = pytest.mark.real_api
TASK = Task("cache-check", None, "chat", "dev", Identity.OWNER, set(), "mixed", "cache-check", [], None)
STABLE = "Reference notes for a cache check. " * 900                  # ~6k tokens: above every provider's minimum
SCHEMA = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"], "additionalProperties": False}


@pytest.fixture
def rt_daemon_for():
    """A daemon in a temp home on the real clock; the Keychain is the owner's login keychain."""
    made: list[tuple[RunningDaemon, Path]] = []

    def make(name: str) -> RunningDaemon:
        base = Path(tempfile.mkdtemp(prefix="ferbc-"))
        d = RunningDaemon(base / "home", clock=SystemClock())
        d.daemon.config.set("runtimes", "orchestrator", name)
        made.append((d, base))
        return d
    yield make
    for d, base in made:
        d.stop()
        shutil.rmtree(base, ignore_errors=True)


@pytest.mark.parametrize("name", ["anthropic", "openai", "google"])
def test_second_call_reads_the_cache(rt_daemon_for, name: str) -> None:
    if not has_api_key(name):
        pytest.skip(f"no {name} key")
    d = rt_daemon_for(name)
    cached = []
    for i in range(2):
        rt = d.daemon._build_runtime(name, "orchestrator", mode="supervisor", tool_names=[], schema=SCHEMA)

        async def once():
            run = await rt.start(TASK, ModelRequest('Answer {"ok": true}.', (STABLE, f"call {i}"), 1), "")
            return [e async for e in rt.events(run)]
        events = d.run(once(), timeout=120)
        cached.append(next(e.data["cached_input_tokens"] for e in events if e.kind == "usage"))
        d.run(asyncio.sleep(3))
    assert cached[1] > 0, cached
