"""L28 (owner, 2026-10-14): the planner's prompt uses each model's real context window minus room for its reply, the
situation always goes in first, then chat newest-first with older chat summarized, ordered so it can be cached."""
import json
import re

import pytest

from ferb.core.errors import ModelLimitsUnknown
from ferb.core.types import ModelLimits
from tests.conftest import FakeRuntime, RunningDaemon, chat, done, install, outcome, sup, task

SESSION = "s1"


def _window(monkeypatch: pytest.MonkeyPatch, window: int, output: int) -> ModelLimits:
    limits = ModelLimits("fake-model", window, output, "test double")
    monkeypatch.setattr(FakeRuntime, "limits", limits)
    return limits


def _planner_requests(script):
    return [r for _, key, r in script.requests if key == "supervisor"]


def _turn_spans(daemon: RunningDaemon) -> list[dict]:
    async def go():
        return [json.loads(r[0]) for r in daemon.daemon.db.conn.execute(
            "SELECT input_json FROM spans WHERE kind='model.turn' ORDER BY rowid")]
    return daemon.run(go())


def _situation_hooks(daemon: RunningDaemon) -> None:
    sv = daemon.daemon.supervisor
    sv.memory_candidates = lambda words: [{'text': 'REMEMBERED-FACT the owner prefers tea', 'reference': 'memory.learned id=m1',
                                           'origin': 'private', 'source_refs': ['m1']}]
    sv.relevant_context = lambda words: ('TODAY-ACTIVITY opened the tea shop page', 'private', 'opened the tea shop page',
                                         ['trail:t1'])
    sv.recent_activity = lambda: ('', 'ferb')


def test_long_history_keeps_situation_activity_and_memory_and_summarizes_the_oldest_chat(daemon, monkeypatch):
    turns = 24
    script = install(daemon, [sup("reply", f"answer {i} " + "detail " * 600) for i in range(turns)] +
                     [sup("reply", "latest answer"), sup("reply", "after the summary")],
                     {"summary": json.dumps({"summary": "EARLIER-SUMMARY the owner asked about tea twenty times."})})
    for i in range(turns):
        chat(daemon, f"message number {i} " + "filler " * 600)
    _situation_hooks(daemon)
    limits = _window(monkeypatch, 26_000, 1_000)          # the history (about 29k tokens) no longer fits
    chat(daemon, "what next?")
    request = _planner_requests(script)[-1]
    context = request.context
    for layer in ("Layer 2 — situation:", "Layer 3 — today's activity", "TODAY-ACTIVITY", "REMEMBERED-FACT",
                  "Layer 6 — running tasks and open questions"):
        assert layer in context, layer
    assert "message number 0 " not in context                     # the oldest chat did not fit
    assert f"message number {turns - 1} " in context              # the newest always does
    assert "older entries not shown" in context
    assert _turn_spans(daemon)[-1]["context_budget_tokens"] == limits.context_window - limits.max_output

    async def summarized():
        import asyncio
        for _ in range(200):
            rows = daemon.daemon.journal.of_kind("chat.summary", "session_id", SESSION)
            if rows:
                return rows[-1]
            await asyncio.sleep(0.02)
        return None
    row = daemon.run(summarized(), timeout=10)
    assert row is not None and "EARLIER-SUMMARY" in row.data["text"]
    assert "inferred" in row.origin                               # model-written (MP2)
    chat(daemon, "and now?")
    later = _planner_requests(script)[-1].context
    assert "EARLIER-SUMMARY" in later and "message number 0 " not in later
    assert later.index("EARLIER-SUMMARY") < later.index(f"message number {turns - 1} ")


def test_budget_follows_the_runtime_window(daemon, monkeypatch):
    big = "A long earlier owner note. " * 400
    script = install(daemon, [sup("reply", "ok"), sup("reply", "ok"), sup("reply", "ok")])
    chat(daemon, big)
    _window(monkeypatch, 3000, 1000)
    chat(daemon, "small window turn")
    assert big.strip() not in _planner_requests(script)[-1].context
    _window(monkeypatch, 400_000, 100_000)
    chat(daemon, "large window turn")
    assert big.strip() in _planner_requests(script)[-1].context
    budgets = [span["context_budget_tokens"] for span in _turn_spans(daemon)[-2:]]
    assert budgets == [2000, 300_000]


def test_a_runtime_without_a_documented_window_is_an_explicit_error(daemon, monkeypatch):
    install(daemon, [sup("reply", "never used")])
    monkeypatch.setattr(FakeRuntime, "limits", None)
    events = chat(daemon, "hello")
    reply = next(e for e in events if e["type"] == "reply")
    assert "context window" in reply["text"]

    async def failed():
        return [e.data for e in daemon.daemon.journal.of_kind("chat.failed")]
    assert any("context window" in str(row.get("detail")) for row in daemon.run(failed()))


def test_adapters_report_documented_limits_and_refuse_unknown_models(tmp_path):
    from ferb.runtimes import anthropic_api, google_api, openai_api
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    from ferb.runtimes.codex_exec import CodexRuntime
    assert openai_api.limits_for("gpt-5.5").context_budget == 1_050_000 - 128_000
    assert anthropic_api.limits_for("claude-sonnet-5-5").context_budget == 1_000_000 - 128_000
    gemini = google_api.limits_for("gemini-3.8-flash")         # Google documents the input limit itself
    assert (gemini.context_window, gemini.max_output, gemini.context_budget) == (1_048_576, 65_536, 1_048_576)
    for lookup in (openai_api.limits_for, anthropic_api.limits_for, google_api.limits_for):
        with pytest.raises(ModelLimitsUnknown):
            lookup("a-model-nobody-documented")
    assert ClaudeRuntime("supervisor", None, str(tmp_path), model="opus").limits.context_window == 1_000_000
    with pytest.raises(ModelLimitsUnknown):
        ClaudeRuntime("supervisor", None, str(tmp_path), model=None).limits          # the SDK default is plan-dependent
    home = tmp_path / "ferb"
    (home / "codex-home").mkdir(parents=True)
    (home / "codex-home" / "auth.json").write_text("{}")
    (home / "codex-home" / "models_cache.json").write_text(json.dumps(
        {"models": [{"slug": "gpt-5.5", "context_window": 272000}]}))
    codex = CodexRuntime(tmp_path / "codex", "supervisor", tmp_path, home, model="gpt-5.5").limits
    assert (codex.context_window, codex.max_output) == (272_000, 128_000)
    with pytest.raises(ModelLimitsUnknown):
        CodexRuntime(tmp_path / "codex", "supervisor", tmp_path, home, model="gpt-6-astra").limits   # not in its catalog


def test_doctor_names_a_role_whose_runtime_has_no_documented_window(tmp_path):
    from ferb.runtimes.catalog import CATALOG
    assert CATALOG["anthropic"].limits("claude-opus-5-5", tmp_path).context_window == 1_000_000
    with pytest.raises(ModelLimitsUnknown):
        CATALOG["claude"].limits(None, tmp_path)


def test_stable_prefix_is_byte_identical_across_consecutive_turns(daemon):
    script = install(daemon, [sup("reply", f"answer {i}") for i in range(4)])
    for words in ("first message", "second message", "third message", "fourth message"):
        chat(daemon, words)
    third, fourth = _planner_requests(script)[-2:]
    assert third.system == fourth.system
    assert third.cacheable >= 3                                    # Layer 1, the conversation header, its entries
    assert fourth.parts[:third.cacheable] == third.parts[:third.cacheable]
    assert fourth.cacheable > third.cacheable
    assert "Layer 5 — owner's complete new message:\nfourth message" in fourth.parts[fourth.cacheable:][-1] or \
        any(p.startswith("Layer 5 — owner's complete new message:\nfourth message") for p in fourth.parts[fourth.cacheable:])


def test_task_sources_runs_once_per_lineage_per_turn(daemon):
    learning = daemon.daemon.learning
    original = learning.task_sources
    install(daemon, [sup("task", "Doing.", task(f"step {i}")) for i in range(3)], {"worker": done("done")})
    earlier = [outcome(chat(daemon, f"do step {i}"))["task_id"] for i in range(3)]
    calls: list[str] = []

    def counted(task_id: str) -> list[str]:
        calls.append(task_id)
        return original(task_id)
    learning.task_sources = counted
    install(daemon, [sup("task", "Doing.", task("step 3"))], {"worker": done("done")})
    result = outcome(chat(daemon, "do step 3"))
    assert len(calls) == len(set(calls)), calls                     # each task's lineage read once in the turn

    async def card():
        node = next(n for n in learning.items("episode") if n["key"] == result["task_id"])
        return node["sources"]
    sources = daemon.run(card())
    # the planner was shown the earlier tasks: their lineage reaches the new card, resolved (no handles stored)
    assert {"episode:" + t for t in earlier} <= set(sources)
    assert not [s for s in sources if s.startswith(("entry:", "task:"))]


def test_one_activity_search_per_planner_turn_off_the_event_loop(daemon):
    import threading
    activity = daemon.daemon.daily_activity
    original = activity.search
    seen: list[str] = []

    def counted(words: str, *args, **kwargs):
        seen.append(threading.current_thread().name)
        return original(words, *args, **kwargs)
    activity.search = counted
    install(daemon, [sup("reply", "ok")])
    chat(daemon, "find the tea shop page")
    assert len(seen) == 1
    assert seen[0] != daemon.thread.name                          # not on the daemon's event loop thread


def test_history_payload_holds_only_entry_texts(daemon):
    install(daemon, [sup("reply", "First answer text."), sup("task", "Doing.", task("inspect"))],
            {"worker": done("done")})
    chat(daemon, "First owner words.")
    result = outcome(chat(daemon, "Inspect now."))

    async def payloads():
        return [json.loads(r[0]) for r in daemon.daemon.db.conn.execute(
            "SELECT s.output_json FROM spans s WHERE s.kind='context.exposure' AND s.task_id=? "
            "AND json_extract(s.input_json, '$.reference') LIKE 'chat.history%'", (result["task_id"],))]
    found = daemon.run(payloads())
    assert found
    payload = "\n".join(p["payload"] for p in found)
    assert "First owner words." in payload and "First answer text." in payload
    assert not re.search(r"\[c\d+\]", payload)
    assert not re.search(r"\(\w{3} \d{2} \w{3} \d{2}:\d{2}\)", payload)
    assert "entry_id=" not in payload and "Owner:" not in payload


def test_reply_rows_record_their_history_by_handle_not_by_copy(daemon):
    sv = daemon.daemon.supervisor
    sv.recent_activity = lambda: ('', 'ferb')
    sv.memory_candidates = lambda words: []
    sv.relevant_context = lambda words: ('Matching activity', 'private', 'Matching activity', ['trail:first-turn'])
    install(daemon, [sup("reply", "answer 0"), sup("reply", "answer 1")])
    chat(daemon, "message 0")
    sv.relevant_context = lambda words: ('', 'ferb', '', [])
    chat(daemon, "message 1")

    async def replies():
        return daemon.daemon.journal.of_kind("chat.reply")
    first, second = daemon.run(replies())
    assert "trail:first-turn" in first.data["source_refs"]
    # the second reply cites the first reply by its entry, never by copying the refs that reply already cites
    assert "entry:" + first.id in second.data["source_refs"]
    assert "trail:first-turn" not in second.data["source_refs"]


def test_assembly_counts_each_block_once(monkeypatch):
    import ferb.core.context as module
    from ferb.core.context import ContextBlock, assemble_parts
    counted: list[int] = []
    real = module.count_tokens

    def tracking(text: str) -> int:
        counted.append(len(text))
        return real(text)
    monkeypatch.setattr(module, "count_tokens", tracking)
    blocks = [ContextBlock(f"optional block {i} " * 50, f"ref {i}") for i in range(60)]
    blocks.insert(0, ContextBlock("required", "r", protected=True))
    assemble_parts(blocks, budget_tokens=2000)
    total = sum(len(b.text) for b in blocks)
    assert sum(counted) < 4 * total                                # never the whole body per block


def test_todays_activity_is_always_in_and_the_recent_trail_dump_only_when_it_fits(daemon, monkeypatch):
    """Today's matching activity is always in the prompt. Every trail row of the last minutes (measured at 119k-433k
    tokens on the owner's data) goes in whole only when the window has room after the conversation; otherwise its
    retrieval reference stands and the planner snapshot keeps it for the task."""
    sv = daemon.daemon.supervisor
    _situation_hooks(daemon)
    dump = json.dumps({"apps": [{"app": "Brave", "events": [{"data": {"title": f"page {i}"}} for i in range(3000)]}],
                       "source_refs": ["trail:recent"]})
    sv.recent_activity = lambda: (dump, "external:web")
    script = install(daemon, [sup("reply", "ok"), sup("reply", "ok")])
    chat(daemon, "a turn with a large window")
    assert "page 2999" in _planner_requests(script)[-1].context
    _window(monkeypatch, 30_000, 1_000)
    chat(daemon, "a turn with a small window")
    context = _planner_requests(script)[-1].context
    assert "TODAY-ACTIVITY" in context and "page 2999" not in context
    assert "[Context omitted; fetch full text: trail.query" in context


def test_running_summary_never_folds_notice_text_and_cites_what_it_folded(daemon, monkeypatch):
    """A notice's text (observed, typed or clipboard-derived) has its own purge and forget paths: the running summary
    never copies it. The summary cites each entry it folded, so an explicit forget reaches it (and summaries on it)."""
    import ferb.orchestrator.supervisor as module
    monkeypatch.setattr(module, "CONVERSATION_KINDS", (*module.CONVERSATION_KINDS, "chat.notice"))
    sv = daemon.daemon.supervisor
    script = install(daemon, [sup("reply", "first answer")],
                     {"summary": json.dumps({"summary": "SUMMARY-ONE"})})
    chat(daemon, "first owner words")

    async def notice():
        return daemon.daemon.journal.append("chat.notice", {"id": "n1", "session_id": SESSION, "text": "NOTICED-TYPED-TEXT", "quotes": []},
                                            origin="external:observation")
    noticed = daemon.run(notice())

    async def fold():
        await sv._summarize(SESSION, 3)
        return sv._summary(SESSION)
    first = daemon.run(fold())
    folded_context = next(c for _, k, c, _ in script.contexts if k == "summary")
    assert "first owner words" in folded_context and "NOTICED-TYPED-TEXT" not in folded_context
    assert "entry:" + noticed.id in first.data["covered"] and "entry:" + noticed.id not in first.data["source_refs"]
    assert "external:observation" not in first.origin

    async def second_and_forget():
        sv.journal.append(module.SUMMARY_KIND, {"session_id": SESSION, "text": "SUMMARY-TWO", "covered": [],
                                               "source_refs": ["entry:" + first.id]}, origin="inferred")
        folded_message = next(r.partition(":")[2] for r in first.data["source_refs"] if r.startswith("entry:"))
        gone = sv.forget_summaries({folded_message})
        return gone, sv._summary(SESSION)
    gone, left = daemon.run(second_and_forget())
    assert gone == 2 and left is None
