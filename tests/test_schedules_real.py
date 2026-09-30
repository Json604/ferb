"""Build step 15, real (pytest -m real; Codex only): schedules made from chat by the real model and run by the real
daemon clock.
- "in a minute, write done.md containing ok" -> a one-off; it runs unattended, writes the file, and its answer waits
  for the next chat;
- a repeat (test-only: every minute; the owner's minimum is 15) runs twice, then "cancel that schedule" cancels it.
"""

import shutil
import tempfile
import time
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.cli.client import Client
from ferb.clock.clock import SystemClock
from tests.conftest import RunningDaemon

pytestmark = pytest.mark.real


@pytest.fixture
def codex(monkeypatch: pytest.MonkeyPatch) -> Iterator[RunningDaemon]:
    from ferb.orchestrator import schedules
    monkeypatch.setattr(schedules, "MIN_EVERY_MIN", 1)                  # test only (the repeat is shortened below)
    base = Path(tempfile.mkdtemp(prefix="fsc-", dir="/tmp"))
    d = RunningDaemon(base / "home", clock=SystemClock())
    d.daemon.config.set("runtimes", "orchestrator", "codex")
    d.daemon.config.set("runtimes", "worker", "codex")
    yield d
    d.stop()
    shutil.rmtree(base, ignore_errors=True)


def chat(d: RunningDaemon, text: str) -> list[dict[str, Any]]:
    c = Client(d.sock, timeout=900)
    try:
        return list(c.stream("chat.send", {"session_id": "s", "text": text}))
    finally:
        c.close()


def call(d: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> Any:
    c = Client(d.sock)
    try:
        return c.call(method, params or {})
    finally:
        c.close()


def wait_for(fn: Any, timeout: float) -> Any:
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        v = fn()
        if v:
            return v
        time.sleep(3)
    return fn()


def test_a_one_off_runs_unattended_and_its_answer_waits(codex: RunningDaemon) -> None:
    evs = chat(codex, "in 1 minute, write a file done.md in your workspace containing just the word ok")
    assert any(e["type"] == "timer" and "scheduled" in e["text"] for e in evs), evs
    done = codex.daemon.home / "workspace" / "done.md"
    assert wait_for(lambda: done.exists() and "ok" in done.read_text().lower(), 360), "the run didn't happen"
    res = wait_for(lambda: call(codex, "chat.status")["results"], 60)
    assert res and res[0]["status"] == "done"


def test_a_repeat_runs_twice_then_is_cancelled_from_chat(codex: RunningDaemon) -> None:
    evs = chat(codex, "every 15 minutes, starting in a minute, append a line 'tick' to ticks.md in your workspace")
    assert any(e["type"] == "timer" and "scheduled" in e["text"] for e in evs), evs
    codex.run(_every_minute(codex))                                    # test only: don't wait 15 minutes
    ticks = codex.daemon.home / "workspace" / "ticks.md"
    assert wait_for(lambda: ticks.exists() and ticks.read_text().lower().count("tick") >= 2, 480), \
        ticks.read_text() if ticks.exists() else "no runs"
    sid = call(codex, "schedules.list")["schedules"][0]["id"]
    chat(codex, "please cancel the ticks schedule")
    assert all(s["id"] != sid for s in call(codex, "schedules.list")["schedules"])


async def _every_minute(d: RunningDaemon) -> None:
    import json
    db = d.daemon.db
    r = db.conn.execute("SELECT id, rule_json FROM schedules").fetchone()          # type: ignore[union-attr]
    rule = json.loads(r["rule_json"])
    rule["every_minutes"] = 1
    db.conn.execute("UPDATE schedules SET rule_json = ? WHERE id = ?", (json.dumps(rule), r["id"]))   # type: ignore[union-attr]


def test_the_model_says_when_and_code_computes_the_date(codex: RunningDaemon) -> None:
    """Structured times: "next Friday at 5 pm" -> the date FERB computed, shown in the confirmation."""
    from ferb.clock.when import When, resolve
    evs = chat(codex, "remind me next friday at 5 pm to call mom")
    expected = resolve(When(day="next_friday", time="17:00"), codex.daemon.clock.now())
    notes = [e["text"] for e in evs if e["type"] == "timer"]
    assert len(notes) == 1 and f"{expected.astimezone():%a %d %b %H:%M}" in notes[0], (notes, evs)
