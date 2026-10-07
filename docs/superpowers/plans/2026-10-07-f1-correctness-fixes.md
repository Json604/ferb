# F1 Correctness Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the five correctness problems that distort every later measurement: the orchestrator losing chat history, memory recall matching everything (and breaking Hindi), observed memory never activating, workers/checkers running without FERB's tools, and prompts ordered so caching can't work.

**Architecture:** All five are changes in place (the plan's "replace only where the code can't express the design"). History and resumable attempts become capped, newest-kept, never-dropped blocks; recall drops query terms that match most memories (data-driven, no word list) and tokenizes by Unicode category, with an FTS migration so Devanagari words index whole; observed proposals accumulate evidence by key instead of exact wording; a run whose runtime shows FERB's tools were never loaded is re-run once with that fact; the context budget report moves to the end and the owner's message becomes the last block.

**Tech Stack:** Python 3.12, SQLite FTS5 (`unicode61 categories 'L* N* Co M*'`), the orchestrator (`ferb/orchestrator/`), memory (`ferb/memory/`), context assembly (`ferb/core/context.py`), pytest.

**Spec:** `docs/0.27_PLAN.md` — "Final build order", milestone F1 ("chat history compacted, never silently dropped; recall (stop words, ranking, Devanagari tokenizer); observed memory activation; workers and checkers seeing FERB's tools") and P20 (stable-first prompt order, decided; registry milestone F2, the orchestrator ordering part lands here). Evidence for each bug is in the plan's "Memory audit", "Verification audit" and "R3 traces" sections.

## Global Constraints

- Python 3.12; `uv sync --group dev`; tests via `uv run pytest`; before each commit `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`.
- Branch `astra/0.27-upgrade`; commit messages cite what they fix and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not push from a task (the controller pushes after the final review).
- New schema only in a new migration file `ferb/store/migrations/0014_*.sql`; never edit an existing migration.
- Code never judges intent: no keyword or label lists deciding meaning (CLAUDE.md rule 6). Term filtering must be data-driven.
- `ferb/core/context.py` rule: never shorten a request or a generated artifact; budgeting omits or caps whole entries and says so.
- Runtime Markdown under `ferb/orchestrator/prompts/` and `ferb/memory/distill.md` is executable input; edit it as carefully as code.
- Do not start, restart or install the running daemon; no live model calls from tasks (the controller runs the live checks).
- Never read or search `vibed/`.

## Review Focus

- **A conversation whose newest single entry is larger than the history cap** (e.g. a pasted 20k-token document): the newest entry must still appear whole and older ones be omitted with a note — never an empty or clipped history. Test in Task 1.
- **A query made only of words that occur in most memories** ("the", "my", "a"): recall must return nothing rather than every memory. Test in Task 2.
- **A Hindi or Hinglish query** ("दिल्ली", "bijli ka bill"): Devanagari words must match whole words, both in the query and the index. Test in Task 2.
- **The same observed pattern proposed on three days with different wording**: exactly one item, activated once — no duplicates, no missed activation. Test in Task 3.
- **A run that legitimately loaded or never needed FERB's tools** (Claude runtime, fake runtime, or Codex run that did call tool_search): must never be re-run. Test in Task 4.

---

### Task 1: Chat history and resumable attempts are capped, never dropped

Today `Supervisor.recent()` (`ferb/orchestrator/supervisor.py:403-439`) returns the whole conversation plus every task as one optional block, and `_supervise` (`:366-384`) adds every resumable attempt (`ContinuityStore.candidates`, `ferb/orchestrator/continuity.py:76-81`) as another; `assemble_context` drops an optional block whole when it doesn't fit, which happened in 11 of 12 live orchestrator turns ("do it" → "Which one should I do?").

**Files:**
- Modify: `ferb/orchestrator/supervisor.py` (constants near the top, `recent()`, the candidates block in `_supervise`)
- Test: `tests/test_history_cap.py` (new)

**Interfaces:**
- Produces: `HISTORY_TOKENS = 12000`, `ATTEMPTS_TOKENS = 2000` (module constants in supervisor.py); `Supervisor.recent(current_request: str = "", conversation: str | None = None, max_tokens: int = HISTORY_TOKENS) -> tuple[str, dict[str, datetime]]` returning oldest-first text whose newest entries fit in `max_tokens`, with an omission note when older entries are left out; `Supervisor._attempts_text(candidates: list[dict[str, Any]], max_tokens: int = ATTEMPTS_TOKENS) -> str`. In `_supervise` both blocks are `protected=True` (they are capped, so they cannot blow the budget).
- Task 5 relies on: the history block being first and protected in `_supervise`.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_history_cap.py
"""Chat history and resumable attempts are capped to their newest entries, never dropped whole."""
from ferb.core.context import count_tokens
from tests.conftest import RunningDaemon, chat, install, sup


def _recent(daemon: RunningDaemon, **kw):
    async def go():
        return daemon.daemon.supervisor.recent(conversation="s1", **kw)
    return daemon.run(go())


def test_long_history_keeps_newest_entries_within_cap(daemon: RunningDaemon) -> None:
    install(daemon, [sup("reply", f"answer {i}") for i in range(40)])
    for i in range(40):
        chat(daemon, f"message number {i} " + "filler " * 120)
    text, _ = _recent(daemon, max_tokens=3000)
    assert count_tokens(text) <= 3000 + 200          # cap plus the header/omission lines
    assert "message number 39" in text                # the newest turn is always there
    assert "message number 0 " not in text            # the oldest was left out
    assert "older entries not shown" in text and "chat.history" in text
    assert text.index("message number 38") < text.index("message number 39")   # oldest first


def test_newest_entry_larger_than_cap_is_kept_whole(daemon: RunningDaemon) -> None:
    install(daemon, [sup("reply", "ok"), sup("reply", "ok")])
    chat(daemon, "a small first message")
    big = "pasted document line. " * 3000
    chat(daemon, big)
    text, _ = _recent(daemon, max_tokens=500)
    assert big.strip() in text                        # never clipped
    assert "a small first message" not in text and "older entries not shown" in text


def test_attempts_text_keeps_newest_within_cap(daemon: RunningDaemon) -> None:
    rows = [{"task_id": f"t{i}", "prior_task": None, "status": "failed", "goal": f"goal {i} " + "x " * 80}
            for i in range(50)]                        # candidates() returns newest first
    text = daemon.daemon.supervisor._attempts_text(rows, max_tokens=800)
    assert "task_id=t0 " in text and "task_id=t49 " not in text
    assert count_tokens(text) <= 800 + 100 and "older resumable attempts not shown" in text
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest tests/test_history_cap.py -v`
Expected: FAIL — `recent()` has no `max_tokens` parameter; `_attempts_text` does not exist.

- [ ] **Step 3: Implement**

At module level in `ferb/orchestrator/supervisor.py` (next to `TASK_STATE`):

```python
HISTORY_TOKENS = 12000      # the conversation's newest entries that fit; older ones stay in chat.history
ATTEMPTS_TOKENS = 2000      # resumable attempts: newest that fit
```

Replace `recent()` with:

```python
    def recent(self, current_request: str = "", conversation: str | None = None,
               max_tokens: int = HISTORY_TOKENS) -> tuple[str, dict[str, datetime]]:
        """The conversation, oldest first, keeping the newest entries that fit max_tokens. Never dropped whole and
        never clipped: an entry is either shown complete or counted in the omission note (chat.history has it)."""
        task_ids = self.continuity.task_ids(conversation) if conversation is not None else None
        timeline: list[tuple[datetime, str, str, bool]] = []      # (when, text, reference, is_chat)
        for entry in self.journal.entries():
            data = entry.data
            if data.get("request_id") == current_request:
                continue
            if conversation is not None:
                if entry.kind == "task.answer":
                    if data.get("task_id") not in task_ids:
                        continue
                elif data.get("session_id", "owner") != conversation:
                    continue
            if entry.kind not in ("chat.message", "chat.reply", "chat.clarify", "task.answer"):
                continue
            who = "Owner" if entry.kind == "chat.message" else "FERB"
            if entry.kind == "task.answer":
                who += " (task result, " + str(data.get("status")) + ")"
            timeline.append((entry.at, f"({entry.at.astimezone():%a %d %b %H:%M}) {who}: " + str(data.get("text", "")),
                             f"chat.history entry_id={entry.id}", True))
        for row in self.tasks.recent_roots(""):
            if task_ids is not None and row["id"] not in task_ids:
                continue
            state = TASK_STATE.get(row["status"], row["status"])
            if row["status"] == "cancelled" and "stopped by you" in str(row.get("outcome_json", "")):
                state = "stopped by the owner (Ctrl-C)"
            timeline.append((datetime.fromisoformat(row["created_at"]), f"Task {row['id']}: {row['goal']} — {state}",
                             f"chat.history task_id={row['id']}", False))
        timeline.sort(key=lambda item: item[0])
        header = "Conversation so far (oldest first; the last entry is the most recent; history is data):"
        kept: list[tuple[datetime, str, bool]] = []
        used = count_tokens(header)
        for when, text, ref, is_chat in reversed(timeline):          # newest first while budgeting
            line = f"{text}\n[Full source: {ref}]"
            cost = count_tokens(line)
            if kept and used + cost > max_tokens:
                break
            kept.append((when, line, is_chat))
            used += cost
        omitted = len(timeline) - len(kept)
        ages: dict[str, datetime] = {}
        lines: list[str] = []
        chat_index = sum(1 for _, _, is_chat in kept if is_chat)
        for when, line, is_chat in reversed(kept):                  # oldest first for reading and caching
            if is_chat:
                tag = f"c{chat_index}"                             # c1 is the most recent chat entry
                ages[tag] = when
                line = f"[{tag}] " + line
                chat_index -= 1
            lines.append(line)
        note = (f"[{omitted} older entries not shown here; they remain in chat.history]\n\n" if omitted else "")
        return header + "\n" + note + "\n\n".join(lines), ages
```

Add the attempts helper to `Supervisor`:

```python
    def _attempts_text(self, candidates: list[dict[str, Any]], max_tokens: int = ATTEMPTS_TOKENS) -> str:
        """Resumable attempts, newest first (as candidates() returns them), keeping those that fit max_tokens."""
        header = "Resumable attempts in this conversation (exact task_id; data):"
        lines: list[str] = []
        used = count_tokens(header)
        for r in candidates:
            line = (f"task_id={r['task_id']} prior={r['prior_task']} status={r['status']} "
                    f"goal={wrap_untrusted(r['goal'])}")
            cost = count_tokens(line)
            if lines and used + cost > max_tokens:
                break
            lines.append(line)
            used += cost
        omitted = len(candidates) - len(lines)
        note = f"\n[{omitted} older resumable attempts not shown; ask chat.history include_tasks=true]" if omitted else ""
        return header + "\n" + "\n".join(lines) + note
```

In `_supervise`, replace the candidates block construction with:

```python
        candidates = self.continuity.candidates(session.id)
        if candidates:
            blocks.append(ContextBlock(self._attempts_text(candidates), "chat.history include_tasks=true",
                                       protected=True))
```

and make the history block protected: `ContextBlock(recent, "chat.history", protected=True)`. Import `count_tokens` from `ferb.core.context` at the top of supervisor.py if it is not already imported. Keep every other block as it is (Task 5 reorders them).

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_history_cap.py tests/test_hardening_context.py tests/test_persistent_chat.py -v`
Expected: PASS (the existing `test_recent_history_does_not_silently_cut_owner_requirement` and the persistent-chat tests still pass).

- [ ] **Step 5: Full suite and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add ferb/orchestrator/supervisor.py tests/test_history_cap.py
git commit -m "F1: cap chat history and resumable attempts to their newest entries, never drop them

The orchestrator's conversation history and resumable-attempts blocks were
optional and dropped whole when over budget (11 of 12 live turns), so
follow-ups like \"do it\" lost their referent. Both are now capped, newest
entries kept whole, oldest first, with an omission note pointing to
chat.history, and marked protected.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Recall matches meaningfully, including Hindi

`Memory.recall` (`ferb/memory/items.py:128-146`) ORs every query word, so "the" matches all items; Python's `\w` and FTS5's default `unicode61` both split Devanagari words into fragments (`दिल्ली` → `द`, `ल`; verified 2026-10-07).

**Files:**
- Create: `ferb/store/migrations/0014_memory_fts_unicode.sql`
- Modify: `ferb/memory/items.py` (add `query_terms()`, rewrite the term handling in `recall`)
- Test: `tests/test_recall.py` (new)

**Interfaces:**
- Produces: `ferb.memory.items.query_terms(text: str) -> list[str]` — casefolded runs of Unicode letters, numbers, marks and private-use characters (matches the FTS tokenizer categories), in order, deduplicated. `Memory.recall(query, k=8, at=None)` keeps its signature; it drops any query term that matches more than half of the active, currently valid items (only when there are at least 2 such items) and returns `[]` when no term is left.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_recall.py
"""Recall: data-driven term filtering (no word list) and whole-word matching for Devanagari."""
from ferb.memory.items import query_terms


def test_query_terms_keep_devanagari_words_whole() -> None:
    assert query_terms("दिल्ली जाऊँगा, bijli ka bill!") == ["दिल्ली", "जाऊँगा", "bijli", "ka", "bill"]
    assert query_terms("The THE the") == ["the"]


def _memory(daemon):
    return daemon.daemon.memory


def _add(daemon, key, content):
    async def go():
        return _memory(daemon).propose("fact", "owner", key, content, "owner_stated", [("journal", "j-" + key)])
    return daemon.run(go())


def _recall(daemon, query):
    async def go():
        return [r.item.key for r in _memory(daemon).recall(query)]
    return daemon.run(go())


def test_common_words_do_not_match_every_memory(daemon) -> None:
    _add(daemon, "diet", "The owner eats the vegetarian diet")
    _add(daemon, "editor", "The owner uses the VS Code editor")
    _add(daemon, "college", "The owner studies at the Scaler school")
    assert _recall(daemon, "delete the hf cache completely") == []
    assert _recall(daemon, "which editor do I use") == ["editor"]


def test_hindi_word_recalls_its_memory(daemon) -> None:
    _add(daemon, "travel", "मैं अगले हफ्ते दिल्ली जाऊँगा")
    _add(daemon, "bills", "The electricity bill (bijli ka bill) is due on the 5th")
    assert _recall(daemon, "दिल्ली कब") == ["travel"]
    assert "bills" in _recall(daemon, "bijli bill")
```

Before running, open `tests/conftest.py` and `ferb/daemon/main.py` to confirm the daemon exposes its `Memory` instance as `daemon.daemon.memory` and that `propose(...)` accepts these arguments and source tuple shape (`ferb/memory/items.py:162`); `propose` requires at least one source and checks journal/trail sources in `_check` — if a journal source must exist in the DB, append a journal row first (see how `tests/test_memory.py` creates owner-stated items) and adjust the helper. Do not weaken `_check`.

- [ ] **Step 2: Run to verify failure**

Run: `uv run pytest tests/test_recall.py -v`
Expected: FAIL — `query_terms` does not exist; with the current code the common-words test returns all three keys and the Hindi query fragments.

- [ ] **Step 3: Write the migration**

```sql
-- ferb/store/migrations/0014_memory_fts_unicode.sql
-- Index Devanagari (and other mark-bearing scripts) as whole words: unicode61's default token categories exclude
-- combining marks, which split words like दिल्ली into fragments. Rebuild from memory_items (external content).
DROP TABLE memory_fts;
CREATE VIRTUAL TABLE memory_fts USING fts5(subject, key, content, content='memory_items', content_rowid='seq',
    tokenize="unicode61 categories 'L* N* Co M*'");
INSERT INTO memory_fts(memory_fts) VALUES ('rebuild');
```

The insert/update/delete triggers on `memory_items` (migration 0006) reference `memory_fts` by name and keep working against the recreated table.

- [ ] **Step 4: Implement `query_terms` and the recall change**

In `ferb/memory/items.py`, add near the top (after imports; add `import unicodedata`):

```python
def query_terms(text: str) -> list[str]:
    """Casefolded word runs by Unicode category (letters, numbers, marks, private use), matching the FTS tokenizer
    (migration 0014). Python's \\w drops combining marks and would split Devanagari words."""
    terms: list[str] = []
    current = ""
    for ch in text.casefold():
        cat = unicodedata.category(ch)
        if cat[0] in "LNM" or cat == "Co":
            current += ch
        elif current:
            terms.append(current)
            current = ""
    if current:
        terms.append(current)
    return list(dict.fromkeys(terms))
```

In `recall`, replace the first lines (`words = ...` through `match = ...`) with:

```python
        when = (at or self.clock.now()).isoformat()
        terms = query_terms(query)
        if not terms:
            return []
        valid = ("m.status = 'active' AND valid_from <= ? AND (valid_to IS NULL OR valid_to > ?)")
        total = int(self.db.conn.execute(f"SELECT COUNT(*) FROM memory_items m WHERE {valid}", (when, when)).fetchone()[0])
        if total == 0:
            return []

        def matches(term: str) -> int:
            return int(self.db.conn.execute(
                f"SELECT COUNT(*) FROM memory_fts f JOIN memory_items m ON m.seq = f.rowid "
                f"WHERE memory_fts MATCH ? AND {valid}", (f'"{term}"', when, when)).fetchone()[0])

        # A term that matches most memories says nothing about any one of them (data-driven, no word list).
        terms = [t for t in terms if total < 2 or matches(t) * 2 <= total]
        if not terms:
            return []
        match = " OR ".join(f'"{t}"' for t in terms)
```

and keep the rest of the method, using the existing `when` (remove the now-duplicate `when = ...` line below). Terms are double-quoted as today; a term containing `"` cannot occur because `query_terms` keeps only letters, numbers, marks and private-use characters.

- [ ] **Step 5: Run the tests**

Run: `uv run pytest tests/test_recall.py tests/test_memory.py tests/test_store.py -v`
Expected: PASS (`test_migrations_apply_once_and_rerun_is_a_no_op` picks up 0014 automatically).

- [ ] **Step 6: Full suite and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add ferb/store/migrations/0014_memory_fts_unicode.sql ferb/memory/items.py tests/test_recall.py
git commit -m "F1: recall drops terms that match most memories and keeps Hindi words whole

Recall ORed every query word, so \"the\" returned every memory to every task.
Terms that match more than half of the active memories are now dropped
(data-driven, no stop-word list). Query terms and the FTS index (migration
0014) tokenize by Unicode category so Devanagari words stay whole.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Observed patterns accumulate evidence by key and can activate

`Memory.propose` (`ferb/memory/items.py:170-175`) adds evidence to an existing proposal only when the content matches exactly (lowercased), and the distiller is shown only active items (`ferb/memory/distill.py:142`), so it can never repeat a pending observation's key: 2 observed proposals exist, none ever activated.

**Files:**
- Modify: `ferb/memory/items.py` (`propose`'s same-item lookup for observed items)
- Modify: `ferb/memory/distill.py` (`_model` context)
- Modify: `ferb/memory/distill.md` (one instruction)
- Test: `tests/test_memory_activation.py` (new)

**Interfaces:**
- Produces: for `origin == "observed"`, `propose` treats an existing non-retracted, current proposal with the same `kind`, `subject` and `key` and origin `observed` as the same item (regardless of wording) and adds the new sources to it; the item activates when its owner-trail evidence spans `OBSERVED_DAYS` (3) distinct days, as today. Other origins keep the exact-content rule. `Memory.items("proposed")` (existing) is used by the distiller.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_memory_activation.py
"""Observed patterns accumulate evidence by key across days and activate on the third day."""


def test_same_key_different_wording_on_three_days_activates_once(daemon) -> None:
    mem = daemon.daemon.memory

    async def go():
        ids = []
        for day, wording in (("2099-01-01", "Codes in VS Code most evenings"),
                             ("2099-01-02", "Uses VS Code for coding in the evening"),
                             ("2099-01-03", "Evening coding happens in VS Code")):
            src = _trail_row(daemon, day)
            ids.append(mem.propose("preference", "owner", "editor", wording, "observed", [("trail", src)]).id)
        return ids, mem.get(ids[0])
    ids, item = daemon.run(go())
    assert len(set(ids)) == 1
    assert item.status == "active"
```

`_trail_row(daemon, day)` must insert one owner trail row for that local day and return its id. Write it at the top of the test file by following how `tests/test_memory.py` (or `tests/test_sense.py`) creates owner trail rows for observed-memory tests — reuse the same helper if one exists in `tests/support/` or `tests/conftest.py`; if none exists, add a small helper to `tests/support/memory_rows.py` and import it (shared test support lives in `tests/support/`, CLAUDE.md).

```python
def test_distiller_sees_pending_observed_proposals(daemon) -> None:
    mem = daemon.daemon.memory
    daemon.run(_propose_one(mem, daemon))           # one observed proposal, not yet active
    seen = {}

    class Capture:
        async def start(self, task, system, context, token):
            seen["context"] = context
            raise RuntimeError("stop after capturing the context")

    distiller = daemon.daemon.distiller          # confirm the attribute name in ferb/daemon/main.py
    distiller.make_runtime = lambda *a, **k: Capture()
    try:
        daemon.run(distiller._model("2099-01-04", _empty_evidence()))
    except RuntimeError:
        pass
    assert "Patterns seen before, not yet confirmed" in seen["context"]
    assert "editor" in seen["context"]
```

Implement `_propose_one` (one observed proposal with one trail source) and `_empty_evidence()` (a `summary.DayEvidence` with no lines — build it the way `ferb/memory/summary.py:57` `build()` does for an empty day, or call `summary.build(db, day)` on a day with no rows) in the same file. Confirm the distiller attribute name on the daemon before writing the test; if `_model` is not reachable that way, construct the distiller object the way `tests/test_memory.py` does.

- [ ] **Step 2: Run to verify failure**

Run: `uv run pytest tests/test_memory_activation.py -v`
Expected: FAIL — three different ids (exact-content match) and the item stays "proposed"; the distiller context lacks the pending section.

- [ ] **Step 3: Implement the key-based match for observed proposals**

In `propose`, replace the `same = ...` query with:

```python
        if origin == "observed":                                   # a pattern: same key = same pattern, any wording
            same = self.db.conn.execute(
                "SELECT id FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND origin = 'observed' "
                "AND status != 'retracted' AND valid_to IS NULL ORDER BY seq LIMIT 1",
                (kind, subject, key)).fetchone()
        else:
            same = self.db.conn.execute(
                "SELECT id FROM memory_items WHERE kind = ? AND subject = ? AND key = ? AND lower(content) = lower(?) "
                "AND origin = ? AND status != 'retracted' AND valid_to IS NULL",
                (kind, subject, key, content, origin)).fetchone()
```

In `ferb/memory/distill.py` `_model`, after building `known`, add the pending observed proposals and include them in `ctx`:

```python
        pending = [f"- {i.kind} / {i.subject} / {i.key}: {i.content}" for i in self.memory.items("proposed")
                   if i.origin == "observed"]
        ctx = (f"Day: {day}\n\nEvidence (cite handles):\n" + "\n".join(ev.lines) +
               "\n\nWhat FERB already remembers (reuse a key to update a fact):\n" + ("\n".join(known) or "(nothing)") +
               "\n\nPatterns seen before, not yet confirmed (if today's evidence shows one again, propose it with the "
               "same key):\n" + ("\n".join(pending) or "(none)"))
```

(Confirm `Memory.items(status)` returns items with `.origin`; it is used with `"active"` on the line above.)

In `ferb/memory/distill.md`, in the `origin "observed"` bullet, after "so propose recurring patterns, not one-offs.", add: `When a pattern listed under "Patterns seen before" shows up again today, propose it with exactly the same kind, subject and key.`

If a test pins prompt files by hash (search `tests/` for `distill.md` or a prompt-hash test), update the expected hash as that test's own instructions say.

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_memory_activation.py tests/test_memory.py -v`
Expected: PASS.

- [ ] **Step 5: Full suite and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add ferb/memory/items.py ferb/memory/distill.py ferb/memory/distill.md tests/test_memory_activation.py tests/support
git commit -m "F1: observed patterns accumulate evidence by key and can activate

Evidence only accumulated on a byte-identical sentence and the distiller never
saw pending proposals, so no observed pattern could reach 3 days. Observed
proposals now match by kind/subject/key, and the distiller sees pending
patterns so it can repeat their key.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: A run that never loaded FERB's tools is re-run once with that fact

> **Amendment (controller ruling F1-R1, 2026-10-07):** research found the root fix, verified live on Codex 0.157.1:
> Codex defers MCP tools when the model's catalog entry has `supports_search_tool: true`
> (codex-rs `spec_plan.rs`: `search_tool_enabled = model_info.supports_search_tool && ...`; github.com/openai/codex
> issue 36382). With `supports_search_tool: false` the dummy server's tools were directly visible (`mcp__<server>.<tool>`)
> and `tool_search` disappeared; the side effects (ChatGPT connector tools, sub-agent tools) are removed by
> `--disable apps --disable plugins --disable multi_agent`, which FERB already passes (`ferb/runtimes/codex_exec.py:37`).
> So this task ALSO adds `"supports_search_tool": False` to `CATALOG_LOCKDOWN` (`ferb/runtimes/codex_exec.py:59-60`),
> with a test that `locked_catalog` sets it on every model and that `DISABLED` still contains `apps`, `plugins` and
> `multi_agent`, and updates `ferb/orchestrator/prompts/runtime_tools.md` so it says FERB's tools are normally
> directly visible and `tool_search` is only the fallback. The re-run guard below stays as the fallback for when
> `locked_catalog` returns None (no `models_cache.json`).

Codex 0.157.1 always hides MCP tools behind `tool_search` (probe 2026-10-07: a dummy MCP server's tools were not directly visible with default flags, with `--disable tool_search_always_defer_mcp_tools`, or with `--enable deferred_tool_world_state`; the flag is listed as "removed"). A worker or checker that skips `tool_search` works blind — the live HF-cache checker returned all-unknown ("Tool discovery was not attempted"), costing a full retry. The runtime reports `tools_seen` in its final event (`ferb/orchestrator/worker.py:173-178`); FERB tools appear there with "ferb" in the name (e.g. `mcp__ferb.browser_open`) once loaded.

**Files:**
- Modify: `ferb/orchestrator/worker.py` (`run_worker`, `_run_worker_once`, `_run`)
- Modify: `ferb/orchestrator/supervisor.py` (initialize two dicts in `__init__` next to `self._stalled`)
- Test: `tests/test_tools_loaded.py` (new)

**Interfaces:**
- Produces: `NO_TOOLS_LOADED` (module constant in worker.py) — the fact text; `Supervisor._tools_seen: dict[str, list[str]]` and `Supervisor._tools_offered: dict[str, bool]` keyed by worker id; a `runtime.tools_not_loaded` span when a re-run happens. `run_worker` keeps its signature and return type.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_tools_loaded.py
"""A worker/checker whose runtime shows FERB's tools were never loaded is re-run once with that fact."""
import json

from ferb.core.types import RuntimeEvent
from tests.conftest import chat, done, install, outcome, sup, task, tool


def _report(summary="ok"):
    return json.dumps({"status": "done", "summary": summary, "artifacts": [], "evidence": []})


def test_rerun_when_tools_never_loaded(daemon) -> None:
    def second(token):
        tool(daemon.sock, token, "actions.search", query="files")      # a real FERB tool call
        return _report("did it with tools")
    script = install(daemon, [sup("task", "Working", task())], {
        "worker": [RuntimeEvent("final", {"text": _report("blind"), "tools_seen": ["apply_patch"]}), second]})
    chat(daemon, "do the thing")
    worker_contexts = [c for _, k, c, _ in script.contexts if k == "worker"]
    assert len(worker_contexts) == 2
    assert "never loaded FERB's tools" in worker_contexts[1]
    kinds = [r[0] for r in daemon.daemon.db.conn.execute("SELECT kind FROM spans")]
    assert "runtime.tools_not_loaded" in kinds


def test_no_rerun_when_ferb_tools_were_seen(daemon) -> None:
    script = install(daemon, [sup("task", "Working", task())], {
        "worker": [RuntimeEvent("final", {"text": _report(), "tools_seen": ["tool_search", "mcp__ferb.fs_read"]})]})
    chat(daemon, "do the thing")
    assert len([1 for _, k, _, _ in script.contexts if k == "worker"]) == 1


def test_no_rerun_when_runtime_reports_nothing(daemon) -> None:
    script = install(daemon, [sup("task", "Working", task())], {"worker": done("ok")})
    chat(daemon, "do the thing")
    assert len([1 for _, k, _, _ in script.contexts if k == "worker"]) == 1
```

Check `actions.search` is a registered FERB tool available to the default task scopes in tests (search `ferb/registry.py` / `tests/` for `actions.search`); if not, use another always-available read tool such as the one used by neighbouring supervisor tests. The `FakeRuntime` in `tests/conftest.py` yields a `RuntimeEvent` step as-is, which is how the first step reports `tools_seen`.

- [ ] **Step 2: Run to verify failure**

Run: `uv run pytest tests/test_tools_loaded.py -v`
Expected: the first test FAILS (only one worker context; no re-run); the two negative tests pass already.

- [ ] **Step 3: Implement**

In `ferb/orchestrator/supervisor.py` `__init__`, next to `self._stalled: set[str] = set()`:

```python
        self._tools_seen: dict[str, list[str]] = {}       # worker id -> tool names its runtime reported
        self._tools_offered: dict[str, bool] = {}         # worker id -> whether FERB offered it any tools
```

and declare them on the worker mixin class in `ferb/orchestrator/worker.py` next to `_stalled: set[str]`:

```python
    _tools_seen: dict[str, list[str]]
    _tools_offered: dict[str, bool]
```

Add the module constant in worker.py:

```python
NO_TOOLS_LOADED = ("Runtime fact: your previous attempt at this exact step never loaded FERB's tools, so it could not "
                   "act or inspect anything. FERB's tools are deferred: first call tool_search with the query \"ferb\" "
                   "(and with an exact tool name if one you need is still missing), then do the step.")
```

In `_run_worker_once`, right after `tools = [...]` is computed: `self._tools_offered[worker_id] = bool(tools)`.

In `_run`, where the final event's `tools_seen` is recorded, also store it: `if worker_id: self._tools_seen[worker_id] = list(ev.data["tools_seen"])`.

Replace `run_worker` with:

```python
    async def run_worker(self, tree: "_Tree", task: Task, prompt: str, ctx: list[ContextBlock], schema: dict[str, Any],
                         span: str) -> tuple[dict[str, Any] | Outcome, str]:
        """One attempt. The orchestrator, rather than a report-word classifier, chooses recovery. A run whose runtime
        shows FERB's tools were never loaded is a runtime fault, not a judgment: it is re-run once with that fact."""
        result, worker_id = await self._run_worker_once(tree, task, prompt, ctx, schema, span)
        seen = self._tools_seen.pop(worker_id, None)
        offered = self._tools_offered.pop(worker_id, False)
        if (seen is not None and offered and not any("ferb" in name for name in seen)
                and not isinstance(result, Outcome) and not tree.stop):
            s = self.spans.start("runtime.tools_not_loaded", task_id=task.id, parent_id=span,
                                 input={"tools_seen": seen})
            self.spans.finish(s, output={"rerun": True})
            fact = ContextBlock(NO_TOOLS_LOADED, "runtime fact: FERB tools were not loaded", protected=True)
            result, worker_id = await self._run_worker_once(tree, task, prompt, ctx + [fact], schema, span)
            self._tools_seen.pop(worker_id, None)
            self._tools_offered.pop(worker_id, False)
        return result, worker_id
```

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_tools_loaded.py tests/test_supervisor.py -v`
Expected: PASS.

- [ ] **Step 5: Full suite and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add ferb/orchestrator/worker.py ferb/orchestrator/supervisor.py tests/test_tools_loaded.py
git commit -m "F1: re-run once a worker or checker whose runtime never loaded FERB's tools

Codex 0.157.1 always defers MCP tools behind tool_search (probe 2026-10-07; the
disable flag is removed). A run that skipped tool_search worked blind, e.g.
the HF-cache checker returned all-unknown and cost a full retry. When the
runtime's tools_seen shows no FERB tool although tools were offered, the step
is re-run once with that fact; traced as runtime.tools_not_loaded.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Stable-first prompt order (P20, orchestrator part)

`assemble_context` (`ferb/core/context.py:68-101`) starts the context with a "Context budget: {…}" report whose numbers change every turn, and `_supervise` puts the owner's message first (`ferb/orchestrator/supervisor.py:366`). Caching needs an identical prefix (verified live: 95% cached with a stable prefix, ~0 when the first line changes); orchestrator turns cache only 37%, dispatch 8%, resources 5%.

**Files:**
- Modify: `ferb/core/context.py` (`assemble_context`: report at the end)
- Modify: `ferb/orchestrator/supervisor.py` (`_supervise` block order)
- Modify: `tests/test_hardening_context.py:343-346` (the header is now the last paragraph)
- Test: `tests/test_prompt_order.py` (new)

**Interfaces:**
- Consumes: Task 1's protected, oldest-first history block.
- Produces: `assemble_context` returns `body + "\n\nContext budget: " + json` (same JSON fields); `_supervise` orders blocks: history, recalled memory, environment, relevant context, resumable attempts, repair, time block, live facts, and the owner's message last.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_prompt_order.py
"""Stable content first, changing content last, so prompt caching can reuse the prefix (P20)."""
from ferb.core.context import ContextBlock, assemble_context
from tests.conftest import chat, install, sup


def test_budget_report_comes_last() -> None:
    out = assemble_context([ContextBlock("stable block", "a", protected=True),
                            ContextBlock("changing block", "b", protected=True)])
    assert out.startswith("stable block")
    assert out.rsplit("\n\n", 1)[1].startswith("Context budget: ")


def test_supervisor_puts_history_first_and_the_owner_message_last(daemon) -> None:
    script = install(daemon, [sup("reply", "first"), sup("reply", "second")])
    chat(daemon, "an earlier message")
    chat(daemon, "the new message")
    ctx = [c for _, k, c, _ in script.contexts if k == "supervisor"][-1]
    body = ctx.rsplit("\n\nContext budget: ", 1)[0]
    assert body.startswith("Conversation so far")
    assert body.rstrip().endswith("Owner's new message:\nthe new message")
```

- [ ] **Step 2: Run to verify failure**

Run: `uv run pytest tests/test_prompt_order.py -v`
Expected: FAIL — the context starts with "Context budget:" and with the owner's message.

- [ ] **Step 3: Implement**

In `assemble_context`'s inner `render()`, return the report after the body:

```python
        return body + "\n\nContext budget: " + json.dumps(report)
```

(Keep the over-budget correction below it: it replaces the first `"over_budget": false`, which still exists exactly once.)

In `_supervise`, build `blocks` in this order (keep each block's text and reference as today; only the order and Task 1's `protected=True` change):

```python
        blocks = [ContextBlock(recent, "chat.history", protected=True),
                  ContextBlock(mem_text, "memory.read"),
                  ContextBlock(_env_text(env), "actions.search and current device facts"),
                  ContextBlock(self.relevant_context(message), "task.evidence query=context.source; memory.read; trail.query")]
        candidates = self.continuity.candidates(session.id)
        if candidates:
            blocks.append(ContextBlock(self._attempts_text(candidates), "chat.history include_tasks=true",
                                       protected=True))
        if repair:
            blocks.append(ContextBlock(load_prompt(self._approach() + "/repair.md") + "\n" +
                                       json.dumps(repair), "plan validation", protected=True))
        owner_active, last_event = self.signals()
        since = await self.since_last_talk(request_id)
        blocks.append(ContextBlock(context.time_block(self.clock.now(), None, None, owner_active, last_event,
                                     ages=ages | mem_ages, since_last_talk=since), "chat.history"))
        if env.get("live"):
            blocks.append(ContextBlock(env["live"], "current device facts"))
        blocks.append(ContextBlock("Owner's new message:\n" + message, "current request", protected=True))
```

Leave the `planner_sources` snapshot and the reference rewriting that follow unchanged.

Update `tests/test_hardening_context.py` lines 343-346 so they read the report from the end:

```python
        assert context.count('Context budget: ') == 1
        body, header = context.rsplit('\n\nContext budget: ', 1)
        accounting = json.loads(header)
        assert accounting['content_tokens_estimate'] == count_tokens(body)
```

Search `tests/` for other assertions that assume the report is first (`startswith("Context budget")`, `split('\n\n', 1)` on contexts) and update them the same way; `tests/test_unit1_speed_voice.py:94` only checks presence and needs no change.

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_prompt_order.py tests/test_context_budget.py tests/test_hardening_context.py tests/test_supervisor.py -v`
Expected: PASS.

- [ ] **Step 5: Full suite and commit**

Run: `uv run pytest -q && uv run mypy ferb/core && git diff --check`

```bash
git add ferb/core/context.py ferb/orchestrator/supervisor.py tests/test_prompt_order.py tests/test_hardening_context.py
git commit -m "F1: stable-first prompt order (P20): budget report last, owner message last

The per-turn budget report opened every context and the owner's message came
first, so no orchestrator prompt shared a prefix with the previous turn
(37% cached; dispatch 8%). The report now closes the context and the
supervisor orders blocks from stable (history, oldest first) to changing
(time, live facts, the new message).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Docs, registry and F2 carry-forward notes

**Files:**
- Modify: `CHANGELOG.md` (new entry above `## 0.27.0-dev — 2026-10-07 — F0 process and measurement`)
- Modify: `ARCHITECTURE.md` (as-built notes)
- Modify: `docs/0.27_DECISIONS.toml` (P20 tests; D32 note)

**Interfaces:** none (documentation).

- [ ] **Step 1: CHANGELOG**

Add, in the file's format:

```markdown
## 0.27.0-dev — 2026-10-07 — F1 correctness fixes

Commits from the F1 plan commit through the F1 review fixes.

- Chat history and resumable attempts are capped to their newest entries (kept whole, oldest first, with an omission
  note) instead of being dropped whole when over budget.
- Recall drops query terms that match most memories (data-driven) and tokenizes by Unicode category; migration 0014
  rebuilds the memory index so Devanagari words index whole.
- Observed patterns accumulate evidence by key; the distiller sees pending patterns, so they can activate after
  3 days.
- A worker or checker whose runtime shows FERB's tools were never loaded is re-run once with that fact (Codex
  0.157.1 always defers MCP tools behind tool_search).
- The context budget report closes the context and the owner's message is the last block (P20).
```

- [ ] **Step 2: ARCHITECTURE**

In the orchestrator/context section, add short as-built lines: history cap (`HISTORY_TOKENS`, `ATTEMPTS_TOKENS`), block order, budget report position, the tools-not-loaded re-run, recall term filtering and the FTS tokenizer. Keep each to one or two lines in the file's style.

- [ ] **Step 3: Registry**

In `docs/0.27_DECISIONS.toml`: set P20's `tests = ["tests/test_prompt_order.py"]` (milestone stays F2; F2 adds runtime cache markers) and add to D32:

```toml
note = "Probe 2026-10-07: Codex 0.157.1 always defers MCP tools behind tool_search (flag removed); the Codex adapter must handle deferral or use app-server dynamic tools without deferLoading. During the probe Codex also tried to load the owner's personal skills from ~/.agents/skills; the adapter must keep them out."
```

Run `uv run pytest tests/test_decision_coverage.py -q`.

- [ ] **Step 4: Commit**

```bash
git add CHANGELOG.md ARCHITECTURE.md docs/0.27_DECISIONS.toml
git commit -m "F1: changelog, architecture notes, registry (P20 tests, D32 adapter notes)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Live acceptance (controller with the owner, after the final review and `ferb setup --update`)

1. Long conversation follow-up: in an existing long chat, ask something, then reply "do it"; FERB acts on the right thing. Trace: the orchestrator context contains the history block (no "omitted_blocks" for history).
2. Recall: ask a disk/storage question; the trace's recalled-memory block does not contain unrelated personal items.
3. Tools: run a task that needs inspection; no `runtime.tools_not_loaded` loop, and the checker used tools (no all-unknown verdict for lack of tools).
4. Caching: after a few chat turns, `ferb baseline` shows a higher cache rate for orchestrator turns than the F0 baseline (`~/.ferb/baselines/2026-10-07T09:02:08+00:00.json`).
5. The owner records verdicts with `ferb verdict`.

## Self-Review

- Spec coverage: history compacted, never dropped → Task 1; recall stop words/ranking/Devanagari → Task 2; observed activation → Task 3; workers and checkers seeing FERB's tools → Task 4 (the setting-based fix was ruled out by the probe; the adapter-level fix is carried to F2 in Task 6); P20 orchestrator ordering → Task 5; docs → Task 6.
- Placeholder scan: code steps carry real code. Three steps tell the implementer to confirm an attribute or helper name against the code before using it (`daemon.daemon.memory`, the distiller attribute, an owner-trail-row helper); each names where to look and what to do if it differs.
- Type consistency: `recent(..., max_tokens)`, `_attempts_text(candidates, max_tokens)`, `query_terms(text)`, `NO_TOOLS_LOADED`, `_tools_seen`, `_tools_offered` are used with the same names and types across tasks.
- Review Focus: oversized newest entry (Task 1 test 2), common-word query (Task 2), Hindi query (Task 2), same-key activation (Task 3), no re-run when tools loaded or not reported (Task 4 tests 2-3).
