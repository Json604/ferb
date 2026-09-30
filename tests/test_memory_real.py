"""Build step 11, real models (pytest -m real), both runtimes:
- a chat turn where the owner states something: the supervisor proposes it with the owner's exact words, code checks
  the quote, and it's remembered;
- one day of distillation over a fixture day (the owner's activity, FERB's own tasks, a pasted note): the summary is
  written first, the model writes a narrative and proposals that pass code's checks. Token usage is measured and
  written to tests/.distill_usage.json for the report.
"""

import json
import shutil
import tempfile
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Iterator

import pytest

from ferb.clock.clock import SystemClock
from ferb.memory import summary
from tests.conftest import RunningDaemon
from tests.test_supervisor import chat

pytestmark = pytest.mark.real
USAGE = Path(__file__).with_name(".distill_usage.json")


@pytest.fixture(params=["codex", pytest.param("claude", marks=pytest.mark.real_claude)])
def rt(request: pytest.FixtureRequest) -> Iterator[tuple[str, RunningDaemon]]:
    base = Path(tempfile.mkdtemp(prefix="ferbm-"))
    d = RunningDaemon(base / "home", clock=SystemClock())
    d.daemon.config.set("runtimes", "orchestrator", request.param)
    d.daemon.config.set("runtimes", "worker", request.param)
    yield request.param, d
    d.stop()
    shutil.rmtree(base, ignore_errors=True)


def test_a_statement_in_chat_is_remembered_with_the_owners_words(rt: tuple[str, RunningDaemon]) -> None:
    _, d = rt
    evs = chat(d, "By the way, my dentist is Dr. Anika Rao at Smile Clinic. Please remember that.", timeout=300)
    assert any(e["type"] == "memory" and "Rao" in e["text"] for e in evs), evs

    async def items() -> list[Any]:
        return d.daemon.memory.items("active")
    got = d.run(items())
    assert any("Rao" in i.content and i.origin == "owner_stated" for i in got), got


def _seed(d: RunningDaemon, day: str) -> None:
    rows: list[tuple[str, str, str, dict[str, Any]]] = []
    for h in range(9, 18):
        rows.append((f"{h:02d}:00", "owner", "app_focus", {"app": "Visual Studio Code", "bundle_id": "com.microsoft.VSCode"}))
        rows.append((f"{h:02d}:20", "owner", "browser_tab", {"browser": "chrome", "profile": "Default", "host": "github.com",
                                                              "url": "https://github.com/ferb/ferb/pulls", "title": "Pull requests"}))
        rows.append((f"{h:02d}:40", "owner", "app_focus", {"app": "Slack", "bundle_id": "com.tinyspeck.slackmacgap"}))
    rows.append(("13:00", "owner", "idle", {"state": "idle"}))
    rows.append(("13:45", "owner", "idle", {"state": "active"}))
    rows.append(("15:00", "ferb", "browser_tab", {"browser": "chrome", "profile": "Profile 22", "host": "mail.google.com",
                                                   "url": "https://mail.google.com/", "title": "Inbox"}))
    rows.append(("16:00", "owner", "file", {"path": "/Users/o/notes/standup.md", "change": "modified", "is_dir": False}))

    async def go() -> None:
        for i, (t, actor, source, data) in enumerate(rows):
            at = datetime.fromisoformat(f"{day}T{t}:00").astimezone()
            d.daemon.db.conn.execute(  # type: ignore[union-attr]
                "INSERT INTO trail (id, at, day, device_id, actor, source, data_json) VALUES (?, ?, ?, 'dev', ?, ?, ?)",
                (f"seed-{i}", at.isoformat(), day, actor, source, json.dumps(data)))
    d.run(go())


def test_one_day_of_distillation_on_a_real_model(rt: tuple[str, RunningDaemon]) -> None:
    runtime, d = rt
    day = (datetime.now().astimezone().date() - timedelta(days=1)).isoformat()
    _seed(d, day)
    usage: list[dict[str, Any]] = []
    make = d.daemon._make_runtime

    def measured(*a: Any, **k: Any) -> Any:
        inner = make(*a, **k)
        events = inner.events

        async def tap(run: Any) -> Any:
            async for e in events(run):
                if e.kind == "usage":
                    usage.append(dict(e.data))
                yield e
        inner.events = tap
        return inner
    d.daemon.distiller.make_runtime = measured
    r = d.run(d.daemon.distiller.run_day(day), timeout=600)
    assert r["status"] == "distilled", r

    async def check() -> tuple[bool, list[Any], list[Any]]:
        j = [e for e in d.daemon._journal().entries() if e.kind == summary.NARRATIVE_KIND]
        return summary.summarized(d.daemon.db, day), j, d.daemon.memory.items(None, valid_now=False)
    summarized, narrative, items = d.run(check())
    assert summarized and narrative and narrative[0].data["text"], (r, narrative)
    assert all(i.origin != "owner_stated" for i in items)
    assert not any("mail.google" in i.content for i in items if i.subject == "owner"), items   # FERB's tab isn't a habit
    tokens = {k: sum(int(u.get(k) or 0) for u in usage)
              for k in ("input_tokens", "cached_input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens",
                        "output_tokens")}
    prev = json.loads(USAGE.read_text()) if USAGE.exists() else {}
    prev[runtime] = {"tokens": tokens, "proposed": r.get("proposed"), "refused": r.get("refused"),
                     "narrative": narrative[0].data["text"][:400], "items": [f"{i.origin}: {i.content}" for i in items]}
    USAGE.write_text(json.dumps(prev, indent=2))
