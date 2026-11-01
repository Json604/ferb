"""Memory import from other AI assistants (MEM-401-6): each reader on a fixture (the
three-month window, roles), redaction before any model call, recall then verification through a scripted model,
imported facts saved as FERB's understanding (never the owner's words, never known destinations), newer words win,
resume after a restart, cancel, nothing raw kept, and the quality score on the labelled fixtures."""
import asyncio
import json
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

import pytest

from ferb.daemon.main import Daemon
from ferb.memory.import_sources import read, window_start
from ferb.sanitize.redact import Redactor
from tests.conftest import RunningDaemon
from tests.support.import_fixtures import (
    RAW,
    SECRET,
    ScriptedImportModel,
    codex_home,
    gemini_export,
    make_exports,
    score,
    verify_answer,
)
from tests.support.memory_items import owner_fact, run

NOW = datetime(2026, 1, 1, 9, tzinfo=UTC)          # FakeClock's start: the window opens 2025-10-01


def texts(convs: list[Any], role: str) -> list[str]:
    return [m.text for c in convs for m in c.messages if m.role == role]


def read_fixture(tmp_path: Path, source: str) -> list[Any]:
    exports = make_exports(tmp_path)
    return read(source, Path(exports.path(source)), now=NOW, redact=Redactor().text)


# ---------- readers ----------

# decision: MEM-401
def test_the_window_is_three_calendar_months() -> None:
    assert window_start(NOW) == datetime(2025, 10, 1, 9, tzinfo=UTC)
    assert window_start(datetime(2026, 5, 31, tzinfo=UTC)).date().isoformat() == "2026-02-28"


# decision: MEM-401, MEM-403, MEM-405, MEM-407
def test_chatgpt_export_roles_window_and_redaction(tmp_path: Path) -> None:
    convs = read_fixture(tmp_path, "chatgpt")
    owner = texts(convs, "owner")
    assert any("IIT Delhi" in t for t in owner) and any("Bangalore" in t for t in owner)
    assert "I prefer short, plain answers without jargon." in owner          # custom instructions are the owner's
    assert not any("Infosys" in t for t in owner)                             # older than the window: never read
    assert not any("system prompt" in t for c in convs for t in [m.text for m in c.messages])
    assert any("Mumbai" in t for t in texts(convs, "assistant"))              # an assistant's words stay its own
    assert not any(SECRET in m.text for c in convs for m in c.messages) and any("[redacted]" in t for t in owner)


# decision: MEM-403, MEM-407, MEM-408
def test_claude_export_projects_and_saved_memory_as_claims(tmp_path: Path) -> None:
    convs = read_fixture(tmp_path, "claude")
    assert "Always write code with type hints." in texts(convs, "owner")
    [claims] = [c for c in convs if c.claims]
    assert [m.role for m in claims.messages] == ["assistant"] and "guitar" in claims.messages[0].text
    assert any("Great project!" == t for t in texts(convs, "assistant"))


# decision: MEM-403, MEM-407, MEM-414
@pytest.mark.parametrize("html", [False, True])
def test_gemini_takeout_json_and_html(tmp_path: Path, html: bool) -> None:
    convs = read("gemini", gemini_export(tmp_path, html=html), now=NOW, redact=Redactor().text)
    assert texts(convs, "owner") == ["what's a good 5k plan? I run 3 times a week"]   # "Used …" isn't a prompt; old out
    assert texts(convs, "assistant") == ["Try intervals on Tuesdays."]


# decision: MEM-403, MEM-407
def test_claude_code_only_the_owners_typed_turns(tmp_path: Path) -> None:
    convs = read_fixture(tmp_path, "claude_code")
    chats = [c for c in convs if not c.claims]
    assert [c.id for c in chats] == ["s-main"]                                 # no subagent, no SDK session
    assert texts(chats, "owner") == ["use uv for python projects, never pip",
                                     "[pasted text, not read] what does this mean?",
                                     "run pytest with nice -n 10 always"]     # no command output, meta, tool result
    assert [c.claims for c in convs if c.claims] == [True]                    # the saved memory note: a claim


# decision: MEM-403, MEM-407
def test_codex_only_interactive_sessions_and_typed_turns(tmp_path: Path) -> None:
    convs = read_fixture(tmp_path, "codex")
    assert len(convs) == 1                                                     # no `codex exec`, no subagent thread
    assert texts(convs, "owner") == ["I deploy my site on Vercel, why is the build failing?"]
    assert texts(convs, "assistant") == ["Check the build logs."]


# decision: MEM-403
def test_codex_reads_a_zstd_compressed_rollout(tmp_path: Path) -> None:
    """A rollout Codex compressed (`rollout-*.jsonl.zst`) is read like a plain one, and its id has no suffix."""
    import zstandard
    root = codex_home(tmp_path)
    plain = next(root.glob("sessions/**/rollout-2025-12-12*.jsonl"))
    packed = plain.with_name(plain.name + ".zst")
    packed.write_bytes(zstandard.ZstdCompressor().compress(plain.read_bytes()))
    plain.unlink()
    convs = read("codex", root, now=NOW, redact=Redactor().text)
    assert texts(convs, "owner") == ["I deploy my site on Vercel, why is the build failing?"]
    assert texts(convs, "assistant") == ["Check the build logs."]
    assert [c.id for c in convs] == ["sess-2025-12-12-cli"]


# ---------- runs through the daemon ----------

async def _wait(task: Any) -> None:
    if task is not None:
        await asyncio.wait({task})


def start(d: RunningDaemon, source: str, path: str, model: ScriptedImportModel) -> dict[str, Any]:
    d.daemon._make_runtime = model                                       # type: ignore[method-assign,assignment]
    out = d.run(d.daemon._memory_import_start(None, {"source": source, "path": path}))
    d.run(_wait(d.daemon._import_task), timeout=30)
    return dict(out)


def imported(d: RunningDaemon, source: str | None = None) -> list[dict[str, Any]]:
    params = {"imported_from": source or "any"}
    return list(d.run(d.daemon._memory_list(None, params))["items"])


def status(d: RunningDaemon) -> dict[str, Any]:
    return dict(d.run(d.daemon._memory_import_status(None, {}))["current"])


def notices(d: RunningDaemon) -> list[str]:
    return [e.data["text"] for e in run(d, lambda: d.daemon._journal().of_kind("chat.notice"))]


# decision: MEM-400, MEM-402, MEM-405, MEM-406, MEM-412
def test_an_import_saves_verified_facts_as_ferbs_understanding(daemon: RunningDaemon, tmp_path: Path) -> None:
    """Estimate first (no model call), then the run: redacted before any call, recall then verify, facts saved as
    FERB's understanding labelled with their source, no known destinations, not in the core profile yet, one notice."""
    d, exports, model = daemon.daemon, make_exports(tmp_path / "x"), ScriptedImportModel()
    destinations = run(daemon, lambda: d.db.conn.execute("SELECT COUNT(*) FROM known_destinations").fetchone()[0])
    estimate = daemon.run(d._memory_import_estimate(None, {"source": "chatgpt", "path": exports.path("chatgpt")}))
    assert estimate["conversations"] == 6 and estimate["model_calls_estimate"] >= 2 and not model.contexts
    start(daemon, "chatgpt", exports.path("chatgpt"), model)
    assert {kind for kind, _ in model.contexts} == {"recall", "verify"}
    assert not any(SECRET in text for _, text in model.contexts)              # redacted before any model call
    assert not any("Infosys" in text for _, text in model.contexts)           # nothing older than the window
    items = imported(daemon, "chatgpt")
    assert items and all(i["origin"] == "inferred" and i["label"] == "FERB's understanding" for i in items)
    assert all(i["about"] == "imported from ChatGPT" and i["imported_from"] == ["chatgpt"] for i in items)
    assert not any("Mumbai" in i["content"] for i in items)                   # an assistant's claim never a fact
    assert not any(i["in_profile"] for i in items)                            # joins Tier 1 only once it holds up
    assert run(daemon, lambda: d.db.conn.execute("SELECT COUNT(*) FROM known_destinations").fetchone()[0]) \
        == destinations
    row = status(daemon)
    assert row["status"] == "done" and row["counts"]["model_calls"] == len(model.contexts)
    assert row["counts"]["facts_saved"] == len(items)
    [told] = notices(daemon)
    assert told.startswith(f"Imported {len(items)} facts from ChatGPT") and "Memory page" in told
    assert not imported(daemon, "gemini")                                     # the filter


# decision: MEM-409
def test_newer_words_win_and_the_older_stay_as_history(daemon: RunningDaemon, tmp_path: Path) -> None:
    d, exports = daemon.daemon, make_exports(tmp_path / "x")
    start(daemon, "chatgpt", exports.path("chatgpt"), ScriptedImportModel())
    homes = [i for i in run(daemon, lambda: d.memory.items(None, valid_now=False)) if i.key == "home_city"]
    current = [i for i in homes if i.valid_to is None]
    assert [i.content for i in current] == ["The owner lives in Bangalore."]
    [old] = [i for i in homes if "Pune" in i.content]
    assert old.valid_to is not None and current[0].supersedes == old.id


# decision: MEM-409
def test_the_owners_own_words_are_never_replaced_by_an_import(daemon: RunningDaemon, tmp_path: Path) -> None:
    mine = owner_fact(daemon, "home_city", "I live in Jaipur")
    start(daemon, "chatgpt", make_exports(tmp_path / "x").path("chatgpt"), ScriptedImportModel())
    homes = [i for i in run(daemon, lambda: daemon.daemon.memory.items("active")) if i.key == "home_city"]
    assert [i.id for i in homes] == [mine.id] and status(daemon)["counts"]["kept_owner"] >= 1


# decision: MEM-408
def test_a_saved_memory_counts_only_with_the_owners_words(daemon: RunningDaemon, tmp_path: Path) -> None:
    start(daemon, "claude", make_exports(tmp_path / "x").path("claude"), ScriptedImportModel())
    contents = [i["content"] for i in imported(daemon, "claude")]
    assert any("vegetarian" in c for c in contents) and not any("guitar" in c for c in contents)


# decision: MEM-408
def test_a_verifier_quote_not_in_the_owners_message_is_dropped(daemon: RunningDaemon, tmp_path: Path) -> None:
    model = ScriptedImportModel()
    model.verify_override = lambda text: {"verdicts": [v | {"quote": "words nobody typed"}
                                                       for v in verify_answer(text)["verdicts"]]}
    start(daemon, "codex", make_exports(tmp_path / "x").path("codex"), model)
    assert not imported(daemon) and status(daemon)["counts"]["dropped"] >= 1


# decision: MEM-404
def test_nothing_raw_is_kept(daemon: RunningDaemon, tmp_path: Path) -> None:
    """MEM-404: facts only. After importing every source, no table, the database file or its WAL holds the
    conversations' raw text (assistant replies, titles, injected output, pasted text, the secret)."""
    d, exports = daemon.daemon, make_exports(tmp_path / "x")
    for source in ("chatgpt", "claude", "gemini", "claude_code", "codex"):
        start(daemon, source, exports.path(source), ScriptedImportModel())

    def dump() -> str:
        out = []
        for (table,) in d.db.conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'").fetchall():
            out += [json.dumps([str(v) for v in row]) for row in d.db.conn.execute(f'SELECT * FROM "{table}"')]
        return "\n".join(out)
    stored = run(daemon, dump)
    raw_files = b"".join(p.read_bytes() for p in d.home.glob("ferb.db*"))
    for words in RAW:
        assert words not in stored and words.encode() not in raw_files, words


# decision: MEM-411
def test_an_import_waits_while_the_owner_has_a_turn(daemon: RunningDaemon, tmp_path: Path) -> None:
    d, model, busy = daemon.daemon, ScriptedImportModel(), [True]
    d._import_busy = lambda: busy[0]                                     # type: ignore[method-assign]
    run(daemon, lambda: setattr(d._import_job(), "idle_poll", 0.01))
    d._make_runtime = model                                              # type: ignore[method-assign,assignment]
    daemon.run(d._memory_import_start(None, {"source": "codex", "path": make_exports(tmp_path / "x").path("codex")}))
    daemon.run(asyncio.sleep(0.2))
    assert not model.contexts and status(daemon)["status"] == "running"
    busy[0] = False
    daemon.run(_wait(d._import_task), timeout=30)
    assert model.contexts and status(daemon)["status"] == "done"


# decision: MEM-411
def test_every_import_call_waits_for_the_owners_turn_not_only_the_first(daemon: RunningDaemon, tmp_path: Path) -> None:
    """memory-import rulings: each model call waits while the owner has a chat turn. A turn that starts after a chunk's
    recall call holds its verify calls too."""
    d, model, released = daemon.daemon, ScriptedImportModel(), [False]
    d._import_busy = lambda: bool(model.contexts) and not released[0]  # type: ignore[method-assign]
    run(daemon, lambda: setattr(d._import_job(), "idle_poll", 0.01))
    d._make_runtime = model                                              # type: ignore[method-assign,assignment]
    daemon.run(d._memory_import_start(None, {"source": "codex", "path": make_exports(tmp_path / "x").path("codex")}))
    daemon.run(asyncio.sleep(0.3))
    assert [kind for kind, _ in model.contexts] == ["recall"] and status(daemon)["status"] == "running"
    released[0] = True
    daemon.run(_wait(d._import_task), timeout=30)
    assert "verify" in {kind for kind, _ in model.contexts} and status(daemon)["status"] == "done"


# decision: MEM-410
def test_a_restart_resumes_from_the_last_finished_chunk(home: Path, tmp_path: Path,
                                                         monkeypatch: pytest.MonkeyPatch) -> None:
    """Progress is journaled per conversation part (hashes only): a restart mid-run goes on by itself, and what was
    read before is not read again."""
    from ferb.clock.clock import FakeClock
    from ferb.core.ids import SequentialIds
    exports = make_exports(tmp_path / "x")
    first = ScriptedImportModel()
    first.block_after = 2                         # the first chunk's recall and verification, then it hangs
    monkeypatch.setattr(Daemon, "_make_runtime", lambda self, *a, **k: first(*a, **k))
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("a"), watch_owner_input=False)
    try:
        run(d, lambda: setattr(d.daemon._import_job(), "chunk_chars", 1))     # one conversation a chunk
        d.run(d.daemon._memory_import_start(None, {"source": "chatgpt", "path": exports.path("chatgpt")}))
        for _ in range(300):
            if len(first.contexts) >= 3:
                break
            d.run(asyncio.sleep(0.02))
        assert status(d)["counts"]["chunks_done"] == 1
    finally:
        d.stop()
    read_before = [text for kind, text in first.contexts[:1]]
    second = ScriptedImportModel()
    monkeypatch.setattr(Daemon, "_make_runtime", lambda self, *a, **k: second(*a, **k))
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("b"), watch_owner_input=False)
    try:
        for _ in range(500):
            if status(d)["status"] != "running":
                break
            d.run(asyncio.sleep(0.02))
        assert status(d)["status"] == "done"
        assert not any(text == read_before[0] for kind, text in second.contexts if kind == "recall")
        homes = [i for i in run(d, lambda: d.daemon.memory.items("active")) if i.key == "home_city"]
        assert [i.content for i in homes] == ["The owner lives in Bangalore."]
    finally:
        d.stop()


# ---------- measured quality ----------

def import_all(d: RunningDaemon, exports: Any, model_for: Any) -> dict[str, Any]:
    for source in ("chatgpt", "claude", "gemini", "claude_code", "codex"):
        d.daemon._make_runtime = model_for()                             # type: ignore[method-assign]
        d.run(d.daemon._memory_import_start(None, {"source": source, "path": exports.path(source)}))
        d.run(_wait(d.daemon._import_task), timeout=900)
    return score([i["content"] for i in imported(d)])


# decision: MEM-400
def test_quality_on_the_labelled_fixtures(daemon: RunningDaemon, tmp_path: Path) -> None:
    """The offline score: what code alone guarantees with a credulous model. Every true fact survives the code's
    checks; every trap code can see (assistant claims, old messages, injected output, subagents and programs, the
    secret, an unsupported saved memory, the superseded city) is gone; unmarked pasted text needs the model (the live
    eval below)."""
    got = import_all(daemon, make_exports(tmp_path / "x"), ScriptedImportModel)
    print("memory import quality (scripted model):", json.dumps(got))
    assert got["recall"] == 1.0 and got["traps_kept"] == ["pasted text"]


# decision: MEM-400
@pytest.mark.real
def test_quality_live(daemon: RunningDaemon, tmp_path: Path) -> None:
    """The live eval (owner-run, `-m real`, step 13): the same fixtures through the configured Codex runtime."""
    d = daemon.daemon
    real = Daemon._make_runtime.__get__(d)
    got = import_all(daemon, make_exports(tmp_path / "x"), lambda: real)
    print("memory import quality (live model):", json.dumps(got))
    assert "secret" not in got["traps_kept"] and got["recall"] >= 0.7


# decision: MEM-403, MEM-408
def test_chatgpt_split_files_and_memory_traces_as_claims(tmp_path: Path) -> None:
    """Larger exports split conversations-000.json, -001…; the saved memory shows only as the memory snapshot and the
    assistant's memory writes (recipient "bio"), read as claims."""
    import zipfile

    from tests.support.import_fixtures import _gpt_conv, ts
    conv = _gpt_conv("g-m", "Notes", [("user", "remember that I'm vegetarian", ts("2025-11-01"))], extra=[
        {"author": {"role": "assistant"}, "recipient": "bio", "create_time": ts("2025-11-01", 11), "metadata": {},
         "content": {"content_type": "text", "parts": ["Is vegetarian."]}},
        {"author": {"role": "user"}, "create_time": ts("2025-11-01", 9), "metadata": {},
         "content": {"content_type": "model_editable_context", "model_set_context": "1. Is vegetarian."}}])
    out = tmp_path / "split.zip"
    with zipfile.ZipFile(out, "w") as z:
        z.writestr("conversations-000.json", json.dumps([conv]))
        z.writestr("conversations-001.json", json.dumps([_gpt_conv("g-n", "More", [("user", "I live in Pune",
                                                                                        ts("2025-11-02"))])]))
    convs = read("chatgpt", out, now=NOW, redact=Redactor().text)
    [claims] = [c for c in convs if c.claims]
    assert [m.text for m in claims.messages] == ["1. Is vegetarian.", "Is vegetarian."]
    assert texts([c for c in convs if not c.claims], "owner") == ["remember that I'm vegetarian", "I live in Pune"]


# decision: MEM-403
def test_claude_export_in_its_2026_per_kind_zips(tmp_path: Path) -> None:
    import zipfile
    root = tmp_path / "claude-2026"
    root.mkdir()
    with zipfile.ZipFile(root / "conversations-000.zip", "w") as z:
        z.writestr("conversations.json", json.dumps([{"uuid": "c", "name": "n", "chat_messages": [
            {"sender": "human", "text": "", "content": [{"type": "text", "text": "I'm building FERB"}],
             "created_at": "2025-11-15T10:00:00.187414Z"}]}]))
    with zipfile.ZipFile(root / "projects-000.zip", "w") as z:
        z.writestr("projects/p1.json", json.dumps({"uuid": "p1", "name": "Code", "prompt_template": "Use type hints.",
                                                   "updated_at": "2025-11-01T10:00:00Z"}))
    with zipfile.ZipFile(root / "memories-000.zip", "w") as z:
        z.writestr("memories/m1.json", json.dumps({"account_uuid": "a", "conversations_memory": "Builds FERB.",
                                                   "memory_files": [{"path": "/x.md", "content": "Likes tea.",
                                                                     "updated_at": "2025-12-01T10:00:00Z"}]}))
    convs = read("claude", root, now=NOW, redact=Redactor().text)
    assert sorted(texts(convs, "owner")) == ["I'm building FERB", "Use type hints."]
    [claims] = [c for c in convs if c.claims]
    assert [m.text for m in claims.messages] == ["Builds FERB.", "Likes tea."]          # never the account id
