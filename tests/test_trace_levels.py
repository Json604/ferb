"""`ferb trace` shows the story by default; --steps one line per tool call; --full/--json raw (P26). The story
outlives the 30-day span purge because it comes from the permanent episode (D21)."""
import io
import json

from ferb.cli import admin
from ferb.cli.client import Client
from tests.conftest import RunningDaemon, chat, done, install, sup, task


def _trace(daemon: RunningDaemon, *flags: str) -> str:
    out, c = io.StringIO(), Client(daemon.sock)
    try:
        admin.trace(c, "last", out, full="--full" in flags, raw="--json" in flags, steps="--steps" in flags)
    finally:
        c.close()
    return out.getvalue()


def _say_hi(daemon: RunningDaemon) -> None:
    install(daemon, [sup("task", "Doing it.", task("say hi"))], {"worker": done("said hi")})
    chat(daemon, "say hi")


def test_default_is_the_story(daemon: RunningDaemon) -> None:
    _say_hi(daemon)
    text = _trace(daemon).splitlines()
    assert text[0].startswith("#") and '"say hi"' in text[0] and "model calls" in text[0]
    assert any(ln.startswith("Tokens:") for ln in text) and any(ln.startswith("Desktop:") for ln in text)


def test_steps_full_and_json(daemon: RunningDaemon) -> None:
    _say_hi(daemon)
    raw = json.loads(_trace(daemon, "--json"))
    assert raw["episode"]["request_text"] == "say hi" and raw["spans"]
    assert _trace(daemon, "--full").strip()
    assert _trace(daemon, "--steps").strip() == "(no tool calls)"           # the scripted worker made none


def test_story_survives_span_purge(daemon: RunningDaemon) -> None:
    _say_hi(daemon)

    async def drop():
        root = daemon.daemon.db.conn.execute(
            "SELECT id FROM tasks WHERE parent_id IS NULL ORDER BY created_at DESC").fetchone()["id"]
        daemon.daemon.db.conn.execute("DELETE FROM spans")
        return root
    daemon.run(drop())
    story = _trace(daemon).splitlines()
    assert story[0].startswith("#") and '"say hi"' in story[0]
    steps = _trace(daemon, "--steps")
    assert "steps aren't kept" in steps and '"say hi"' in steps
