"""L28, D32: a runtime's documented per-input character cap (Codex 0.157.1's app-server refuses a turn whose text input
holds more than 1,048,576 characters) is a model limit like its token window. Context assembly fits both; a protected
block that can't fit is refused by name before anything is sent, never sent to be rejected; the learner's calls are
split to fit it. Live 2026-10-11 to 10-20: every learning job failed at Codex's cap with no visible reason."""
import asyncio
import json
from pathlib import Path
from typing import Any

import pytest

from ferb.core.context import ContextBlock, assemble
from ferb.core.errors import InputTooLarge
from ferb.core.types import Identity, ModelLimits, ModelRequest, Task
from ferb.memory.consolidate import Consolidator
from ferb.memory.learning import Learning
from ferb.runtimes.codex_exec import CodexRuntime
from ferb.store.db import Database
from tests.conftest import FakeDistillRuntime, FakeRuntime, chat, install, sup, task

WIDE = 1_000_000          # a token window that holds everything these tests send
CODEX_START = CodexRuntime.start          # the real one: offline tests replace it with a refusal (conftest)


def capped(chars: int) -> ModelLimits:
    return ModelLimits("fake-model", WIDE, 100_000, "test double", max_input_chars=chars)


# ---------- assembly ----------

def test_an_assembly_that_fits_tokens_but_not_characters_drops_optional_blocks_and_older_chat() -> None:
    blocks = [ContextBlock("OWNER REQUEST", "request", protected=True),
              *[ContextBlock(f"chat entry {i} " + "x" * 900, f"chat.history id={i}", history=True) for i in range(10)],
              ContextBlock("optional observation " * 200, "task.evidence span_id=s1")]
    out = assemble(blocks, budget_tokens=WIDE, max_chars=4_000)
    text = "\n\n".join(out.parts)
    assert len(text) <= 4_000 and out.chars == len(text)
    assert "OWNER REQUEST" in text and "chat entry 9 " in text          # protected, then the newest chat
    assert "chat entry 0 " not in text and "older entries not shown" in text
    assert "optional observation" not in text and "task.evidence span_id=s1" in text
    everything = assemble(blocks, budget_tokens=WIDE)                   # no cap documented: tokens alone decide
    assert "chat entry 0 " in "\n\n".join(everything.parts)


def test_protected_text_over_the_character_cap_is_refused_by_name() -> None:
    with pytest.raises(InputTooLarge) as caught:
        assemble([ContextBlock("y" * 5_000, "request", protected=True)], budget_tokens=WIDE, max_chars=4_000)
    assert caught.value.max_chars == 4_000 and caught.value.chars > 5_000
    assert "4000" in str(caught.value) and "characters" in str(caught.value)


def test_characters_are_counted_as_codex_counts_them_not_bytes() -> None:
    """Codex counts Rust `chars()` (Unicode scalar values): 3,000 CJK characters are 9,000 UTF-8 bytes but fit 4,000."""
    out = assemble([ContextBlock("世" * 3_000, "request", protected=True)], budget_tokens=WIDE, max_chars=4_000)
    assert "世" * 3_000 in out.parts[0]


# ---------- the Codex runtime ----------

def test_codex_reports_its_documented_input_cap_and_other_runtimes_none(tmp_path: Path) -> None:
    from ferb.runtimes import anthropic_api, google_api, openai_api
    from ferb.runtimes.codex_exec import MAX_INPUT_CHARS, CodexRuntime
    home = tmp_path / "ferb"
    (home / "codex-home").mkdir(parents=True)
    (home / "codex-home" / "auth.json").write_text("{}")
    (home / "codex-home" / "models_cache.json").write_text(json.dumps(
        {"models": [{"slug": "gpt-5.6-sol", "context_window": 272000, "effective_context_window_percent": 95}]}))
    limits = CodexRuntime(tmp_path / "codex", "supervisor", tmp_path, home).limits
    assert MAX_INPUT_CHARS == limits.max_input_chars == 1_048_576
    assert "rust-v0.157.1" in limits.source and "MAX_USER_INPUT_TEXT_CHARS" in limits.source
    from ferb.runtimes.codex_exec import CODEX_VERSION, INPUT_CHARS_SOURCE
    assert f"rust-v{CODEX_VERSION}" in INPUT_CHARS_SOURCE              # a new pin needs the cap read again
    for other in (openai_api.limits_for("gpt-5.6-sol"), anthropic_api.limits_for("claude-sonnet-5-5"),
                  google_api.limits_for("gemini-3.8-flash")):
        assert other.max_input_chars is None


def test_status_and_doctor_name_the_cap(daemon) -> None:
    d = daemon.daemon
    (d.home / "codex-home").mkdir(exist_ok=True)
    (d.home / "codex-home" / "auth.json").write_text("{}")
    (d.home / "codex-home" / "models_cache.json").write_text(json.dumps(
        {"models": [{"slug": "gpt-5.6-sol", "context_window": 272000, "effective_context_window_percent": 95}]}))
    limits = d._runtime_limits()["orchestrator"]
    assert limits["max_input_chars"] == 1_048_576
    from ferb.cli.setup import context_window_check
    check = context_window_check("orchestrator", limits)                 # the `ferb doctor` line
    assert check.status == "ok" and "at most 1048576 characters in one input" in check.detail


@pytest.mark.anyio
@pytest.mark.parametrize("schema", [{"type": "object"}, None])     # app-server, and plain `codex exec`
async def test_the_codex_runtime_refuses_an_input_over_its_cap_before_starting_codex(tmp_path, monkeypatch, schema):
    import ferb.runtimes.codex_exec as codex
    spawned: list[Any] = []

    async def spawn(*cmd: Any, **kw: Any) -> Any:
        spawned.append(cmd)
        raise AssertionError("Codex must not be started")

    async def version(binary: Any) -> None:
        return None
    monkeypatch.setattr(asyncio, "create_subprocess_exec", spawn)
    monkeypatch.setattr(codex, "check_version", version)
    (tmp_path / "codex-home").mkdir()
    (tmp_path / "codex-home" / "auth.json").write_text("{}")
    rt = codex.CodexRuntime(Path("/fake/codex"), "supervisor", tmp_path / "work", tmp_path, output_schema=schema)
    t = Task("t", None, "reply", "d", Identity.OWNER, set(), "mixed", "r", [], None)
    with pytest.raises(InputTooLarge) as caught:
        await CODEX_START(rt, t, ModelRequest("FERB instructions", ("z" * (codex.MAX_INPUT_CHARS + 1),)), "")
    assert caught.value.max_chars == codex.MAX_INPUT_CHARS and not spawned


# ---------- the planner and the worker ----------

def test_the_planner_fits_chat_to_the_character_cap(daemon, monkeypatch) -> None:
    turns = 12
    script = install(daemon, [sup("reply", f"answer {i}") for i in range(turns)] + [sup("reply", "latest")])
    for i in range(turns):
        chat(daemon, f"message number {i} " + "filler " * 300)
    monkeypatch.setattr(FakeRuntime, "limits", capped(30_000))
    chat(daemon, "what next?")
    request = [r for _, key, r in script.requests if key == "supervisor"][-1]
    assert len(request.context) <= 30_000
    assert f"message number {turns - 1} " in request.context and "message number 0 " not in request.context
    assert "older entries not shown" in request.context


def _refusals(daemon) -> list[dict[str, Any]]:
    async def go() -> list[dict[str, Any]]:
        return [{"input": json.loads(r[0]), "error": r[1], "parent": r[2]} for r in daemon.daemon.db.conn.execute(
            "SELECT s.input_json, s.error, p.kind FROM spans s LEFT JOIN spans p ON p.id = s.parent_id "
            "WHERE s.kind = 'context.too_large' ORDER BY s.rowid")]
    return daemon.run(go())


def test_a_planner_context_over_the_character_cap_is_refused_by_name_never_sent(daemon, monkeypatch) -> None:
    """Refused by FERB, not reported by the service; the screenshot (0 characters) is not blamed and not retried; the
    trace names each block's size."""
    from ferb.sense import front_window as fw

    async def front(request_id: str, client_pid: int | None = None) -> Any:     # a screenshot goes with the turn
        return fw.FrontWindow("Brave Browser", "com.brave.Browser", "notes", "https://example.com/", "notes",
                              "ev-" + request_id, "external:web:example.com", image=b"\xff\xd8\xff redacted")
    daemon.daemon.front_window = front
    script = install(daemon, [sup("reply", "never used")])
    monkeypatch.setattr(FakeRuntime, "limits", capped(2_000))
    message = "please read this: " + "q" * 3_000
    events = chat(daemon, message)
    reply = next(e for e in events if e["type"] == "reply")["text"]
    assert "didn't send" in reply and "2,000" in reply and "reported an error" not in reply
    assert not script.requests                                          # nothing was sent to be rejected

    async def journal(kind: str) -> list[Any]:
        return [e.data for e in daemon.daemon.journal.of_kind(kind)]
    assert any("characters" in str(row.get("detail")) for row in daemon.run(journal("chat.failed")))
    assert not daemon.run(journal("screen.image_refused"))
    [refusal] = _refusals(daemon)                                        # one assembly, no image-less retry
    blocks = refusal["input"]["blocks"]
    assert refusal["parent"] == "model.turn" and "2000" in refusal["error"]
    assert all(set(b) == {"reference", "chars"} for b in blocks)
    assert max(blocks, key=lambda b: b["chars"])["chars"] >= len(message)


def test_a_worker_context_over_the_character_cap_is_refused_by_name_never_sent(daemon, monkeypatch) -> None:
    wide, small = capped(WIDE), capped(500)
    monkeypatch.setattr(FakeRuntime, "limits", property(lambda self: small if self.mode == "worker" else wide))
    script = install(daemon, [sup("task", "on it", task("tidy the notes"))], {"worker": json.dumps({
        "status": "done", "summary": "never used", "artifacts": [], "evidence": []})})
    chat(daemon, "tidy the notes", timeout=60)
    assert not [key for _, key, _ in script.requests if key == "worker"]

    async def worker_runs() -> list[tuple[str, str]]:
        return [(r[0], r[1]) for r in daemon.daemon.db.conn.execute(
            "SELECT error, output_json FROM spans WHERE kind = 'worker' AND error IS NOT NULL")]
    runs = daemon.run(worker_runs())
    assert any("InputTooLarge" in e and "500" in e and "didn't send" in out for e, out in runs)
    assert any(r["parent"] == "worker" for r in _refusals(daemon))


# ---------- the learner and the distiller ----------

class _Clock:
    def now(self) -> Any:
        from datetime import datetime, timezone
        return datetime(2026, 10, 11, tzinfo=timezone.utc)


class _NoMemory:
    def items(self, **_: Any) -> list[Any]:
        return []

    def get(self, _ident: str) -> None:
        return None


@pytest.fixture
def learning(tmp_path):
    db = Database(tmp_path / "memory.db")
    db.migrate()
    yield Learning(db, _Clock(), lambda *args: None, lambda s: s)
    db.close()


EMPTY = {"procedures": [], "lessons": [], "identity": [], "episodes": []}


def _card(store: Learning, ident: str, request: str) -> Any:
    return store.put("episode", ident, {"task_id": ident, "verified": True, "request": request, "apps": {},
                                        "slots": {}, "status": "done",
                                        "actions": [{"id": ident + "-a", "tool": "browser.open", "args": {"n": ident}}]},
                     [], origin_label="ferb", made_by="code:evidence-card/2", state="active")


def test_a_group_that_fits_the_window_but_not_one_input_is_split_by_characters(learning, monkeypatch) -> None:
    cards = [_card(learning, f"t{i}", "open the page " + "word " * 300) for i in range(8)]
    rt = FakeDistillRuntime(EMPTY)
    monkeypatch.setattr(FakeDistillRuntime, "limits", capped(6_000))
    out = asyncio.run(Consolidator(learning, _NoMemory(), lambda *a, **k: rt).catch_up())
    assert len(rt.contexts) > 1 and all(len(context) <= 6_000 for context in rt.contexts)
    assert all(sum(c["id"] in context for context in rt.contexts) == 1 for c in cards)
    assert not [r for r in out["sources"] if "error" in r]
    assert {r[0] for r in learning.db.conn.execute("SELECT status FROM learning_jobs")} == {"done"}


def test_a_card_longer_than_one_input_is_never_sent_and_its_job_names_the_cap(learning, monkeypatch) -> None:
    small, big = _card(learning, "s", "open the page"), _card(learning, "b", "open the page " + "word " * 2_000)
    rt = FakeDistillRuntime(EMPTY)
    monkeypatch.setattr(FakeDistillRuntime, "limits", capped(6_000))
    asyncio.run(Consolidator(learning, _NoMemory(), lambda *a, **k: rt).catch_up())
    assert not any(big["id"] in context for context in rt.contexts)
    assert any(small["id"] in context for context in rt.contexts)
    rows = dict(learning.db.conn.execute("SELECT source_ref, error FROM learning_jobs WHERE status = 'failed'"))
    assert set(rows) == {big["id"]} and "characters" in rows[big["id"]] and "6000" in rows[big["id"]]


def test_a_day_over_the_character_cap_is_refused_by_name_never_sent(daemon, monkeypatch) -> None:
    from tests.conftest import trail_row
    d = daemon.daemon
    trail_row(daemon, "2025-12-30")
    rt = FakeDistillRuntime({"narrative": "", "items": []})
    monkeypatch.setattr(FakeDistillRuntime, "limits", capped(100))
    d.distiller.make_runtime = lambda *a, **k: rt
    result = daemon.run(d.distiller.run_day("2025-12-30"), timeout=30)
    assert result["status"] == "failed" and not rt.contexts

    async def row() -> Any:
        return d.db.conn.execute("SELECT detail FROM distill_runs WHERE day = '2025-12-30'").fetchone()
    detail = daemon.run(row())["detail"]
    assert "InputTooLarge" in detail and "characters" in detail and "100" in detail
