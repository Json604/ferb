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
    assert openai_api.limits_for("gpt-5.6-sol").context_budget == 1_050_000 - 128_000
    home = tmp_path / "ferb"
    (home / "codex-home").mkdir(parents=True)
    (home / "codex-home" / "auth.json").write_text("{}")
    (home / "codex-home" / "models_cache.json").write_text(json.dumps(
        {"models": [{"slug": "gpt-5.6-sol", "context_window": 272000, "effective_context_window_percent": 95}]}))
    codex = CodexRuntime(tmp_path / "codex", "supervisor", tmp_path, home).limits       # the default model
    # Codex's docs don't say whether its window holds the reply: FERB uses the input window Codex itself reports
    # (catalog window x effective percent), never subtracting the reply a second time
    assert (codex.model, codex.context_budget, codex.max_output) == ("gpt-5.6-sol", 258_400, 128_000)
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
    daemon.run(daemon.daemon.supervisor.finish_episodes())       # cards are written after the outcome (L28)
    calls: list[str] = []

    def counted(task_id: str) -> list[str]:
        calls.append(task_id)
        return original(task_id)
    learning.task_sources = counted
    install(daemon, [sup("task", "Doing.", task("step 3"))], {"worker": done("done")})
    result = outcome(chat(daemon, "do step 3"))
    daemon.run(daemon.daemon.supervisor.finish_episodes())
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


def test_the_recent_raw_trail_is_fetch_only_never_in_the_planner_prompt(daemon):
    """Owner decision 2026-10-14: today's matching activity stays in the prompt; the raw trail of the last minutes
    (measured at 119k-433k tokens on the owner's data) is never in it, even when it would fit. The planner is told a
    task's worker can fetch it with trail.query. Real trail rows, through the daemon's own wiring."""
    from ferb.sense.trail import Observation

    async def seed():
        trail = daemon.daemon.trail
        trail.record(Observation("window", {"app": "Brave", "title": "ZEBRA-XYZZY crossing"}, {"actor": "owner"}))
        trail.record(Observation("window", {"app": "Brave", "title": "kettle shopping list"}, {"actor": "owner"}))
    daemon.run(seed())
    script = install(daemon, [sup("reply", "ok")])
    chat(daemon, "kettle shopping again?")             # (no word shared with the other row or its field names)
    request = _planner_requests(script)[-1]
    assert "kettle shopping list" in request.context.split("Layer 3 — today's activity", 1)[1]   # today's match: in
    assert "ZEBRA-XYZZY" not in request.context                           # the last minutes' raw trail: fetch-only
    assert "ZEBRA-XYZZY" not in request.system
    assert "raw activity of the last minutes" in request.system and "trail.query" in request.system


def test_the_outcome_is_sent_before_episode_capture_and_capture_still_completes(daemon):
    """Episode capture (its lineage resolution) runs after the owner has the outcome, as a tracked job: awaited at
    shutdown, a failure journaled."""
    import time
    from ferb.cli.client import Client
    sv = daemon.daemon.supervisor
    captured: list[str] = []

    def slow(task_id: str) -> None:
        time.sleep(1.0)
        captured.append(task_id)
    sv.on_episode = slow
    install(daemon, [sup("task", "Doing.", task("job")), sup("task", "Doing.", task("job two"))], {"worker": done("done")})
    c = Client(daemon.sock, timeout=30)
    seen_at_outcome = None
    try:
        for ev in c.stream("chat.send", {"session_id": SESSION, "text": "do the job"}):
            if ev["type"] == "outcome":
                seen_at_outcome = list(captured)
                task_id = ev["task_id"]
    finally:
        c.close()
    assert seen_at_outcome == []                                       # the outcome came first
    daemon.run(sv.finish_episodes(), timeout=10)
    assert captured == [task_id]

    def failing(task_id: str) -> None:
        raise RuntimeError("capture broke")
    sv.on_episode = failing
    outcome(chat(daemon, "do job two"))
    daemon.run(sv.finish_episodes(), timeout=10)

    async def failed():
        return [e.data for e in daemon.daemon.journal.of_kind("memory.episode_hook_failed")]
    assert any("capture broke" in row["error"] for row in daemon.run(failed()))


def test_gpt_5_6_sol_has_a_published_price():
    from ferb.runtimes.usage import cost_usd
    assert cost_usd("gpt-5.6-sol", 100_000, 0, 0, 1_000_000) == pytest.approx(0.4 + 20.0)   # $4 in, $20 out per 1M
    assert cost_usd("gpt-5.6-sol", 1_000_000, 1_000_000, 0, 0) == pytest.approx(0.8)        # >272K: cached $0.40 x 2


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


def test_codex_default_is_gpt_5_6_sol():
    """Owner decision 2026-10-14: GPT-5.5 retires from Codex for ChatGPT sign-ins."""
    import ferb.runtimes.codex_exec as cx
    from ferb.runtimes.catalog import CATALOG
    assert cx.MODEL == "gpt-5.6-sol" and CATALOG["codex"].default_model == "gpt-5.6-sol"


def _fake_codex(tmp_path, monkeypatch, catalog: dict):
    """A stand-in for the pinned binary: only `debug models --bundled`, run with an empty Codex home (no login)."""
    import ferb.runtimes.codex_exec as cx
    fake = tmp_path / "codex"
    fake.write_text('#!/bin/sh\n[ "$1 $2 $3" = "debug models --bundled" ] || exit 2\n'
                    '[ -z "$(ls -A "$CODEX_HOME")" ] || exit 3\n'
                    f"cat <<'EOF'\n{json.dumps(catalog)}\nEOF\n")
    fake.chmod(0o755)
    monkeypatch.setattr(cx, "codex_binary", lambda ferb_home, repo=None: fake)
    return fake


def test_a_fresh_codex_login_reads_its_window_from_codexs_own_catalog(tmp_path, monkeypatch):
    """A fresh install has FERB's Codex login but no cached catalog yet (Codex writes one only after a run): the window
    comes from the catalog shipped with the pinned binary, dumped once through Codex itself, offline. Codex runs with
    that same catalog."""
    import ferb.runtimes.codex_exec as cx
    from ferb.core.errors import Unsupported
    home = tmp_path / "ferb"
    (home / "codex-home").mkdir(parents=True)
    (home / "codex-home" / "auth.json").write_text("{}")
    fake = _fake_codex(tmp_path, monkeypatch, {"models": [
        {"slug": "gpt-5.6-sol", "context_window": 272000, "effective_context_window_percent": 95, "tool_mode": "x"}]})
    assert cx.CodexRuntime(fake, "supervisor", tmp_path, home).limits.context_budget == 258_400
    locked = cx.locked_catalog(home)
    assert locked is not None and json.loads(locked.read_text())["models"][0]["tool_mode"] is None
    assert not (home / "codex-home" / "models_cache.json").exists()      # FERB's login home is left as it was

    def missing(ferb_home, repo=None):
        raise Unsupported("pinned Codex not installed")
    monkeypatch.setattr(cx, "codex_binary", missing)
    for dumped in (home / "run").glob("codex-bundled-*.json"):
        dumped.unlink()
    with pytest.raises(ModelLimitsUnknown, match="ferb setup --update"):  # the error names the real fix
        cx.CodexRuntime(fake, "supervisor", tmp_path, home).limits


def test_the_conversation_seam_is_the_timeline_only():
    """`_recent` is gone: code that would still send the whole chat as one protected block breaks loudly (L28)."""
    from ferb.orchestrator.supervisor import Supervisor
    assert not hasattr(Supervisor, "_recent") and not hasattr(Supervisor, "recent")


def test_running_summary_refreshes_only_when_uncovered_entries_fall_out(daemon, monkeypatch):
    """Owner decision 2026-10-14: no fixed step and no new number. A refresh runs only when entries that fell out of
    the window aren't covered by the latest summary."""
    sv = daemon.daemon.supervisor
    started: list[tuple[str, int]] = []

    async def counted(conversation: str, count: int) -> None:
        started.append((conversation, count))
    monkeypatch.setattr(sv, "_summarize", counted)
    install(daemon, [sup("reply", "detail " * 600) for _ in range(8)] + [sup("reply", "ok")] * 3)
    for i in range(8):
        chat(daemon, f"message {i} " + "filler " * 600)
    assert started == []                                               # everything fit: nothing to fold
    _window(monkeypatch, 22_000, 1_000)
    chat(daemon, "now the window is small")
    assert len(started) == 1 and started[0][1] > 0                    # uncovered entries fell out: one refresh
    monkeypatch.setattr(FakeRuntime, "limits", ModelLimits("fake-model", 1_000_000, 100_000, "test double"))
    chat(daemon, "a large window again")
    assert len(started) == 1                                           # nothing fell out: no refresh


def test_daily_log_rewrites_wait_for_a_search_in_progress(tmp_path):
    import threading
    from datetime import datetime, timezone
    from ferb.sense.daily import DailyActivity
    log = DailyActivity(tmp_path / "activity.jsonl", lambda: datetime(2026, 10, 3, tzinfo=timezone.utc))
    log.record({"id": "a", "source": "window", "where": {"title": "tea"}})
    log._lock.acquire()
    done = threading.Event()
    worker = threading.Thread(target=lambda: (log.forget_sources({"a"}), done.set()))
    worker.start()
    try:
        assert not done.wait(0.3)                                      # held off while a search holds the log
    finally:
        log._lock.release()
    worker.join(5)
    assert done.is_set() and log.search("tea") == []


def test_every_activity_search_runs_off_the_event_loop(daemon):
    import threading
    activity = daemon.daemon.daily_activity
    original = activity.search
    seen: list[str] = []

    def counted(words: str, *args, **kwargs):
        seen.append(threading.current_thread().name)
        return original(words, *args, **kwargs)
    activity.search = counted
    install(daemon, [sup("task", "Doing.", task("find the tea shop page"))], {"worker": done("found")})
    outcome(chat(daemon, "find the tea shop page"))
    assert len(seen) >= 2 and daemon.thread.name not in seen          # the planner's and each worker context's


def test_since_last_talk_payload_holds_only_the_answer_texts(daemon):
    def worker(token: str) -> str:                       # the answer lands after the owner's message (frozen clock)
        daemon.call_soon(daemon.daemon.clock.advance, 1)
        return done("FIRST-JOB-ANSWER")
    install(daemon, [sup("task", "Doing.", task("first job")), sup("task", "Doing.", task("second job"))],
            {"worker": worker})
    outcome(chat(daemon, "do the first job"))
    result = outcome(chat(daemon, "do the second job"))

    async def payloads():
        return [json.loads(r[0]) for r in daemon.daemon.db.conn.execute(
            "SELECT output_json FROM spans WHERE kind='context.exposure' AND task_id=? "
            "AND json_extract(output_json, '$.text') LIKE 'Layer 2 — situation:%'", (result["task_id"],))]
    found = daemon.run(payloads())
    assert found and "FIRST-JOB-ANSWER" in found[0]["payload"]
    assert "a task finished" not in found[0]["payload"] and "ago" not in found[0]["payload"]


def test_a_failed_query_embedding_is_not_retried_on_the_loop(daemon):
    learning = daemon.daemon.learning
    calls: list[str] = []

    class Failing:
        name = "failing"

        def encode(self, text, *, query=False):
            calls.append(text)
            raise RuntimeError("model unavailable")
    learning.embedder = Failing()
    daemon.run(daemon.daemon._learning_candidates("tea shop"))

    async def unavailable():
        return daemon.daemon.journal.of_kind("memory.embedding_unavailable")
    assert calls == ["tea shop"] and len(daemon.run(unavailable())) == 1


BUSY_SECOND = """
import json, time
big = json.dumps(list(range(300000)))
end = time.perf_counter() + 1.0
while time.perf_counter() < end:                 # CPU-bound like the real resolution (long JSON decodes)
    json.loads(big)
"""


def test_episode_capture_runs_off_the_loop_so_a_new_message_is_not_held(daemon, monkeypatch):
    """The learning capture's reads (the lineage resolution: 1.3-1.6 s of CPU on the owner's data) run in a process of
    their own: in a thread they would still hold the interpreter lock against the event loop. A message sent meanwhile
    gets its acknowledgement and first line at once; the card is still written."""
    import sys
    import time
    from ferb.cli.client import Client
    from ferb.memory.learning import Learning
    real = Learning.prepare_episode

    def busy(self, task_id, apps=None):                                # if the reads ran in-process
        exec(BUSY_SECOND, {})
        return real(self, task_id, apps)
    monkeypatch.setattr(Learning, "prepare_episode", busy)
    run_reader = "\nimport runpy, sys\nsys.argv[0] = 'card_reader'\nrunpy.run_module('ferb.memory.card_reader', run_name='__main__')"
    daemon.daemon.card_reader = [sys.executable, "-I", "-c", BUSY_SECOND + run_reader]
    install(daemon, [sup("task", "Doing.", task("job")), sup("reply", "quick answer")], {"worker": done("done")})
    outcome(chat(daemon, "do the job"))
    c = Client(daemon.sock, timeout=30)
    sent, first = time.monotonic(), {}
    try:
        for ev in c.stream("chat.send", {"session_id": SESSION, "text": "a quick question"}):
            first.setdefault(ev["type"], time.monotonic() - sent)
            if ev["type"] == "reply":
                break
    finally:
        c.close()
    capturing = bool(daemon.daemon.supervisor._episode_jobs)
    daemon.run(daemon.daemon.supervisor.finish_episodes(), timeout=20)
    assert capturing, first                                            # the answer came while the capture still ran
    assert first["ack"] < 0.2 and first["reply"] < 0.5, first

    async def card():
        return [n for n in daemon.daemon.learning.items("episode")]
    assert daemon.run(card())                                          # and the card was still written


def test_reply_rows_keep_their_source_ids_intact(daemon):
    """A registered secret value can occur inside an id; journal rows, like span inputs, keep FERB's ids whole by key,
    or an altered ref breaks lineage (production: 44 episode captures failed on 'unknown source m-8e95…[redacted]…')."""
    sv = daemon.daemon.supervisor
    node = "m-8e953d2b8ca8fa1fce4363854ff888cff4b8e7875d600c2682390412a8cf79ab"
    daemon.call_soon(daemon.daemon._add_secret_value, "4363854ff888")
    sv.memory_candidates = lambda words: [{'text': 'A remembered fact.', 'reference': 'memory.learned id=x',
                                           'origin': 'private', 'source_refs': [node]}]
    install(daemon, [sup("reply", "ok")])
    chat(daemon, "hello")

    async def reply():
        return daemon.daemon.journal.of_kind("chat.reply")[-1].data["source_refs"]
    assert node in daemon.run(reply())


def test_the_card_reader_is_started_as_its_exact_entry_point_with_no_arguments(daemon, monkeypatch):
    """Controller decision (packaged app): the stub accepts only exact entry points, so the reader is started as exactly
    `python -I -m ferb.memory.card_reader`; the task id and app versions go on stdin, and the reader finds the database
    in the daemon's FERB home itself."""
    import asyncio
    import sys
    from ferb.daemon import main
    assert daemon.daemon.card_reader == [sys.executable, "-I", "-m", "ferb.memory.card_reader"]
    started = []
    real = asyncio.create_subprocess_exec

    async def spawn(*args, **kwargs):
        if "ferb.memory.card_reader" in args:
            started.append((args, kwargs.get("env", {}).get("FERB_HOME")))
        return await real(*args, **kwargs)
    monkeypatch.setattr(main.asyncio, "create_subprocess_exec", spawn)
    install(daemon, [sup("task", "Doing.", task("job"))], {"worker": done("done")})
    outcome(chat(daemon, "do the job"))
    daemon.run(daemon.daemon.supervisor.finish_episodes(), timeout=60)
    assert started == [((sys.executable, "-I", "-m", "ferb.memory.card_reader"), str(daemon.daemon.home))], started

    async def cards():
        return daemon.daemon.learning.items("episode"), daemon.daemon.journal.of_kind("memory.episode_failed")
    written, failed = daemon.run(cards())
    assert written and not failed, failed


def test_an_owner_verdict_given_during_a_capture_stays_on_the_card(daemon):
    """Review round 3 (reproduced): a card the reader read before the owner's verdict was written after it, so the
    feedback was lost and `verified` flipped back. A change to a card's inputs makes an in-flight read read again."""
    import time
    from ferb.cli.client import Client
    from tests.support.cards import slow_reader
    daemon.daemon.card_reader = slow_reader(1.5)
    install(daemon, [sup("task", "Doing.", task("job"))], {"worker": done("done")})
    outcome(chat(daemon, "do the job"))

    async def tid():
        return daemon.daemon.db.conn.execute("SELECT task_id FROM episodes").fetchone()[0]
    task_id = daemon.run(tid())
    time.sleep(0.8)                                   # the reader has read; it answers 1.5 s after it started
    c = Client(daemon.sock, timeout=60)
    try:
        assert c.call("verdict.set", {"task_id": task_id, "verdict": "fail"})["verdict"] == "fail"
    finally:
        c.close()

    async def card():
        [node] = daemon.daemon.learning.items("episode")
        return node["body"]["feedback"], node["body"]["verified"]
    feedback, verified = daemon.run(card())
    assert [f["verdict"] for f in feedback] == ["fail"] and verified is False    # when verdict.set answers
    daemon.run(daemon.daemon.supervisor.finish_episodes(), timeout=30)
    feedback, verified = daemon.run(card())
    assert [f["verdict"] for f in feedback] == ["fail"] and verified is False    # and after every capture finished


def _reader_pids(daemon) -> list[int]:
    async def pids():
        import asyncio
        for _ in range(500):
            if daemon.daemon._card_readers:
                return [p.pid for p in daemon.daemon._card_readers]
            await asyncio.sleep(0.02)
        return []
    return daemon.run(pids(), timeout=30)


def _gone(pid: int) -> bool:
    import os
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    return False


def test_a_cancelled_capture_kills_its_card_reader(daemon):
    """A capture that is cancelled (or fails) never leaves its reader running: the reader is killed and reaped."""
    import asyncio
    from tests.support.cards import HANGING_READER
    daemon.daemon.card_reader = list(HANGING_READER)
    install(daemon, [sup("task", "Doing.", task("job"))], {"worker": done("done")})
    outcome(chat(daemon, "do the job"))
    pids = _reader_pids(daemon)
    assert pids

    async def cancel():
        jobs = list(daemon.daemon.supervisor._episode_jobs)
        for job in jobs:
            job.cancel()
        await asyncio.gather(*jobs, return_exceptions=True)
        return [e.data for e in daemon.daemon.journal.of_kind("memory.episode_deferred")]
    deferred = daemon.run(cancel(), timeout=10)
    assert all(_gone(pid) for pid in pids) and not daemon.daemon._card_readers
    assert deferred and deferred[-1]["task_id"]


def test_shutdown_kills_an_in_flight_card_reader_instead_of_waiting(home):
    """At shutdown an in-flight reader is killed rather than awaited; the capture is journaled as deferred, and the
    hourly consolidator captures that card again."""
    import sqlite3
    import time
    from ferb.core.ids import SequentialIds
    from tests.conftest import FakeClock
    from tests.support.cards import HANGING_READER
    d = RunningDaemon(home, clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False)
    d.daemon._permissions = lambda: {"accessibility": True, "screen": True}
    d.daemon.supervisor.availability = d.daemon.supervisor._default_availability
    d.daemon.card_reader = list(HANGING_READER)
    install(d, [sup("task", "Doing.", task("job"))], {"worker": done("done")})
    outcome(chat(d, "do the job"))
    pids = _reader_pids(d)
    assert pids
    began = time.monotonic()
    d.stop()                                          # its own wait gives up after 10 s
    assert time.monotonic() - began < 8
    assert all(_gone(pid) for pid in pids)
    db = sqlite3.connect(home / "ferb.db")
    try:
        assert db.execute("SELECT count(*) FROM journal WHERE kind='memory.episode_deferred'").fetchone()[0] == 1
    finally:
        db.close()


def test_verdicts_and_consolidation_read_cards_in_the_card_reader(daemon, monkeypatch):
    """`verdict.set` and the consolidator's hourly re-capture used to resolve lineage on the event loop. Both read in
    the card reader now; the consolidator yields between episodes and stops when a task starts."""
    import asyncio
    from ferb.cli.client import Client
    from ferb.daemon import main
    from ferb.memory.learning import Learning
    install(daemon, [sup("task", "Doing.", task("first job")), sup("task", "Doing.", task("second job"))],
            {"worker": done("done")})
    outcome(chat(daemon, "do the first job"))
    outcome(chat(daemon, "do the second job"))
    daemon.run(daemon.daemon.supervisor.finish_episodes(), timeout=60)

    def on_the_loop(self, task_id, apps=None):
        raise AssertionError("an episode card was read on the daemon's event loop")
    monkeypatch.setattr(Learning, "prepare_episode", on_the_loop)
    started = []
    real = asyncio.create_subprocess_exec

    async def spawn(*args, **kwargs):
        if "ferb.memory.card_reader" in args:
            started.append(args)
        return await real(*args, **kwargs)
    monkeypatch.setattr(main.asyncio, "create_subprocess_exec", spawn)

    async def tasks():
        return [r[0] for r in daemon.daemon.db.conn.execute("SELECT task_id FROM episodes ORDER BY ended_at")]
    first, second = daemon.run(tasks())
    c = Client(daemon.sock, timeout=60)
    try:
        c.call("verdict.set", {"task_id": first, "verdict": "pass"})
    finally:
        c.close()
    assert len(started) == 1

    async def card(task_id):
        return next(n for n in daemon.daemon.learning.items("episode") if n["key"] == task_id)["body"]["feedback"]
    assert [f["verdict"] for f in daemon.run(card(first))] == ["pass"]

    consolidator = daemon.daemon.consolidator
    checks = iter([True, True, False])                # idle at the start and before the first card, then a task starts
    consolidator.idle = lambda: next(checks, False)
    started.clear()
    assert daemon.run(consolidator.catch_up(), timeout=60) == {"status": "busy"}
    assert len(started) == 1                          # the second card waits for the next pass


def test_a_capture_reaching_a_ref_damaged_by_redaction_still_writes_the_card_and_says_so(daemon):
    """Controller decision B: the owner's journal holds 60 rows whose ids an earlier redaction altered. A capture whose
    lineage reaches one skips that ref (it holds the redaction marker, so it can never resolve) and journals it."""
    damaged = "m-8e953d2b8ca8f[redacted]b46e986ee416a81e4cc270dfe91fe0f6a23784"
    fact = {'text': 'A remembered fact.', 'reference': 'memory.learned id=x', 'origin': 'private', 'source_refs': [damaged]}
    daemon.daemon.supervisor.memory_candidates = lambda words: [fact]
    install(daemon, [sup("task", "Doing.", task("job"))], {"worker": done("done")})
    outcome(chat(daemon, "do the job"))
    daemon.run(daemon.daemon.supervisor.finish_episodes(), timeout=60)

    async def result():
        j = daemon.daemon.journal
        return (daemon.daemon.learning.items("episode"), [e.data for e in j.of_kind("memory.episode_refs_damaged")],
                j.of_kind("memory.episode_failed"))
    cards, reported, failed = daemon.run(result())
    assert cards and not failed, failed
    assert damaged not in cards[0]["sources"]
    assert reported and reported[-1]["refs"] == [damaged]
