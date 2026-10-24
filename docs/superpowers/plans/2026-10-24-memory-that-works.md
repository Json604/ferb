# Memory and Learning That Work (L36) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make what the owner tells FERB reach every role that needs it: the owner's words are saved even when the
planner skips them, every guess is asked about at once, one ranked search feeds the planner (60 items), every worker
and plan step (20 items, with its parent goal) and both memory tools, logins are one record per site, stale facts
retire, sealed values stop damaging ordinary text, and procedure learning stops refusing every real run.

**Architecture:** Memory items stay the source of truth (`ferb/memory/items.py`), indexed as learned fact nodes
(`ferb/memory/learning.py`). This plan changes what is indexed (content only), how it is ranked (document-frequency
filter, tied bm25 ranks, a small recency boost, owner first) and how much each role gets (the owner's budgets). The
daemon (`ferb/daemon/main.py`) owns questions, the notch list and login records; the supervisor
(`ferb/orchestrator/supervisor.py`) pushes memory into worker contexts. Code checks every quote against the owner's
typed words; models only propose.

**Tech Stack:** Python 3.12, SQLite FTS5 (`unicode61` and trigram), pydantic, jsonschema, pytest; Swift 6 / SwiftUI
(`ui/mac/FERBNotch`) for the notch list.

**Spec:** `docs/superpowers/specs/2026-10-24-memory-that-works-design.md` (binding). Section 1 is done: commits
`1c22dcb` (every turn path applies the planner's entries once) and `f747d37` (the planner remembers a login name).
The audit behind it (live database and code at f747d37) is summarized in the spec's "Why".

## Global Constraints

- Python 3.12; `uv sync --group dev`. Commands: `uv run pytest -q tests/<file>::<test>`, full `uv run pytest -q`,
  `uv run mypy ferb/core`, `git diff --check`; Swift: `cd ui/mac/FERBNotch && swift test`.
- Owner numbers, verbatim (2026-10-24): **"Planner 60, worker 20"**. Each planner turn gets the 60 most relevant
  items; each worker and plan step the 20 most relevant, ranked. The identity layer (owner-stated preferences about the
  owner, in every system prompt) does not count toward them.
- Owner decisions, verbatim: guesses **"Ask right away"**; usernames **"Yes, plain"**; timing **"BEFORE LAUNCH"**.
- The backlog of existing inferred proposals appears once as a single "Review N older guesses" list (Keep / Edit /
  Drop each); every new guess is asked at once.
- Quote checks run against the owner's typed words, never pasted text (MP2, rule 16).
- Secrets never reach memory, a model, a log, a trace or the database (rule 14); `secrets.list` never returns a value
  or a Keychain account; the place a secret is bound to is always code-derived (D12).
- No keyword list decides intent (rule 6). The macOS system word list used to tell the owner a sealed value is an
  ordinary word is a fact, not an intent judgement.
- A question never expires while it waits (rule 9); questions survive a restart through `memory_questions`.
- Every commit message cites `L36` (plus other decision ids it touches) and ends with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Work on `main`. Before each task run `git status` and `git log --oneline -3`: another agent commits to `main`
  (gateway secret summary, content.js, loops.py, supervisor control paths, steering.py, supervisor.md). Find code by
  function name, not line number; stage only the files the task lists (`git add <paths>`, never `git add -A`); push
  with `git push origin main`, and if it is rejected, `git pull --rebase origin main`, rerun the task's focused tests,
  push again. Never force-push.
- Docs change in the task that changes the behavior: `ARCHITECTURE.md` (new §16.12 "Memory that works (L36)", plus
  the memory and secrets paragraphs named in each task), `README.md` ("Memory and learning"), `CHANGELOG.md` (a bullet
  at the end of the "After the owner's update (2026-10-24):" list, just above `Licensed under Apache-2.0.`). No new
  Markdown file except this plan.
- Never read or search `vibed/`. Do not install or restart the daemon; live checks are the owner's
  (`docs/OWNER_ACCEPTANCE.md`, added in the last task). Real tests (`-m real`) only with the owner watching.
- Runtime prompts (`ferb/orchestrator/prompts/*.md`, `ferb/memory/*.md`) are executable input: every new sentence is
  pinned by a test.

## Rulings at a glance

Each is marked `Ruling:` where it is built. The owner can correct any of them.

1. Distill quotes cite a C handle; code maps it to the message's journal id (the model sees handles, not ids).
2. A quote counts only on an `inferred` item that cites that C handle, from a message journaled with
   `untrusted_parts` (older rows can't prove what was typed).
3. Edit is a second, free-text question; text typed into the Keep / Edit / Drop question that is no option is
   refused (a stray "yes" never becomes a memory).
4. Drop deletes that guess alone and keeps its fingerprint; it is not `forget` (spec gap: `forget` purges every version
   of the key and the owner's own messages).
5. Declining a guess question is Drop (skipping FERB's own check is a No, L3).
6. The backlog list reuses the notch's learning-digest notice (it already shows per-item buttons), offered once.
7. The content index per kind is listed in Task 3; a fact's key and subject slugs are indexed as their words
   (`usvisascheduling_username` -> "usvisascheduling username", `project:us-b1b2-visa` -> "us b1b2 visa"); day
   digests and rollups aren't indexed (code counts, navigation).
8. Recency adds at most 0.05, halving every 30 days; equal bm25 scores share a rank, so owner-first can apply.
9. Memory blocks stay protected inside the 60/20 budgets (spec gap: unprotected blocks are filled after the
   conversation, and the planner has no tool to fetch an omitted one).
10. An episode's "outcome" is its status and whether it was verified; its answer stays in the card.
11. Attention gets the planner's 60.
12. Workers get memory through a new async `memory_push` hook, not the synchronous `recall` (the query embedding
    stays off the event loop, L28); the daemon's `recall` stub goes. Their query is the owner's request, the parent
    goal (a step's), the task or step goal and the task's targets (`target_words`: sites' host labels, apps,
    recipients, paths' last parts).
12a. The planner's memory query is the owner's message plus the goals and targets of the tasks its task layers show:
    this chat's running tasks and the resumable attempts it is shown (recently ended ones among them). An owner message
    that shares no word with a fact still recalls it when the work it continues names the site.
13. Plan steps keep an empty activity layer; do/reduce, converse and loop steps all get the parent goal.
14. For a login name, `content` is the username alone; the record keeps the owner's own spelling; HTTPS origins only.
15. A new Keychain account is `"<place> · <name>"`; migrated entries keep their old account.
16. Entries bound to a whole browser are listed as unusable in every `secrets.list` answer.
17. The hidden-input line names "<username> at <site>".
17a. On a login page the username comes from `secrets.list(origin)` (the record), independent of memory ranking;
    worker.md tells the worker to call it before typing into a login form.
18. `until` is `{quote, day | date | in_days}`; code works out the day's end (the next local midnight).
19. Distill retires an item (named by kind, subject, key) only on a passing quote; otherwise a Retire / Keep question.
20. Whole-token matching applies to word-like sealed values (letters and spaces); any other sealed value still matches
    anywhere outside FERB's ids (spec gap: a password glued to other text would leak, rule 14).
21. "Ordinary word" comes from `/usr/share/dict/words`; the line ends the task's answer, once per place.
22. Per-run handles are recognized by the shapes FERB's tools give them; `~` expands to the daemon user's home.
23. Version ranges replace "an update sends a learned procedure back to candidate": out of range, it stays learned
    and the worker is told to verify each step.
24. A procedure receipt is written only when the run followed its required steps.
25. The recall tests use the audit's real texts (the 12:17 and 12:22 messages, the visa task's goal and targets, the
    research_official step goal); only the owner's email address is replaced (`owner@example.com`, `owner-account`).
    The surrounding store is invented.

## Review Focus

The input classes the spec implies that are most likely to bite the owner, each pinned by a test in its task:

- An owner message that shares no word with a fact ("hmm idk if thats s agood strategy…", 12:17) while the task it
  continues names the site: the fact is still among the planner's 60, and the task's login step gets it among its 20
  through the task's targets (Task 3, Task 5).
- A memory query made only of words most items share ("is the for and owner") returns nothing, never the newest
  60 or 20 items (Task 3).
- The owner types "keep", "Drop" or "2" into a guess question's field: it picks that option; any other text is refused
  with "Pick Keep, Edit or Drop." and the question stays open, never saved as a memory (Task 1).
- A sealed password with digits or symbols glued to other text (`?p=hunter2!x`, `userhunter2!`) is still redacted;
  only a word-like answer is matched as a whole word (Task 9).
- An `owner-secret-handles.json` in the old flat format, holding a browser-bound entry, or unreadable: old handles keep
  working, the browser one is listed unusable, an unreadable file is never overwritten and the tools say why (Task 6).

## Tasks

1. Safety net: quoted owner words, and every guess asked at once (§2)
2. The older guesses, reviewed once as one list (§2 ruling)
3. One ranked search with the owner's budgets (§3)
4. `memory.read` and `memory.learned` through the one search (§3)
5. Every worker and plan step gets memory and its parent goal (§4)
6. One login record per site, `secrets.list`, migration (§5)
7. The username from the owner's own words (§5)
8. Out-of-date facts retire (§6)
9. Redaction matches sealed values safely (§7)
10. Learning unblocked, use recorded, live checks (§8)

---

### Task 1: Safety net — the owner's quoted words, and every guess asked at once (spec §2)

**Files:**
- Modify: `ferb/memory/items.py` (add `Capture`, `capture`, `CAPTURE_TOLD`, `owner_span`; `Memory.drop`)
- Modify: `ferb/sanitize/untrusted.py` (add `typed_lines_of`)
- Modify: `ferb/memory/summary.py` (`DayEvidence.add` returns its handle; `DayEvidence.owner_words`)
- Modify: `ferb/memory/distill.py` (`QUOTE_SCHEMA`, `owner_said`, `KEEP_GUESS`, `Distiller.ask_keep`, `run_day`)
- Modify: `ferb/memory/distill.md`
- Modify: `ferb/orchestrator/steering.py` (`EDIT_MEMORY_ASK`, `MEMORY_QUESTIONS`, `OPTION_ONLY`, `choices`)
- Modify: `ferb/orchestrator/supervisor.py` (`ROUTABLE_QUESTIONS`, `PLAIN_CHOICE`, `_plain_choice`, in `_answer`)
- Modify: `ferb/daemon/main.py` (`_remember`, `_ask_owner`, new `_ask_keep` and `_settle_guess`,
  `_question_answered`, `_restore_memory_questions`, `_build_memory` wiring, `_resolve_approval`)
- Modify: `ferb/cli/live_chat.py` (the reply line names a check's options)
- Create: `tests/support/memory_items.py`, `tests/test_memory_capture.py`
- Modify: `tests/test_memory.py`, `tests/test_decide.py` (answers carry `quote`)
- Modify: `docs/0.27_DECISIONS.toml`, `ARCHITECTURE.md`, `README.md`, `CHANGELOG.md`

**Interfaces:**
- Consumes: `owner_sentences`, `contains_words`, `query_terms` (items.py); `summary.build`; `Daemon._ask_owner`.
- Produces:
  - `items.capture(own_words: str, content: str, quote: str) -> Capture` with `Capture.origin` in
    `{"owner_stated", "inferred", ""}`, `.content: str`, `.refused: str`; `CAPTURE_TOLD: dict[str, str]`.
  - `items.owner_span(text: str, part: str) -> str | None` (the owner's own spelling of `part`).
  - `Memory.drop(item_id: str) -> None`.
  - `untrusted.typed_lines_of(for_model: str) -> str`.
  - `DayEvidence.owner_words: dict[str, tuple[str, str]]` (C handle -> (journal id, typed lines)).
  - `distill.owner_said(item: dict, ev: DayEvidence) -> tuple[str, str] | None` ((sentences, journal id)).
  - `Distiller.ask_keep: Callable[[MemoryItem, str], None]`.
  - `steering.EDIT_MEMORY_ASK = "memory.edit"`, `steering.MEMORY_QUESTIONS: dict[str, tuple[str, tuple[str, ...]]]`,
    `steering.OPTION_ONLY: frozenset[str]`, `steering.choices(options) -> str`.
  - `Daemon._ask_keep(item, reason: str) -> None`, `Daemon._settle_guess(item_id: str, action: str, text: str = "") -> str`
    (`action` in `keep | edit | drop`; raises `FerbError` for anything else or an empty edit).
  - Test helpers in `tests/support/memory_items.py`: `run`, `journal`, `owner_fact`, `today`, `said`, `c_handle`,
    `distill_item`, `distill_day`, `pending`.

- [ ] **Step 1: Write the shared test helpers**

Create `tests/support/memory_items.py`:

```python
"""Shared helpers for memory tests on a running test daemon (L36). The daemon's loop owns SQLite, so every read and
write goes through it."""
from typing import Any

from ferb.memory import summary
from tests.conftest import FakeDistillRuntime


def run(d: Any, fn: Any) -> Any:
    async def go() -> Any:
        return fn()
    return d.run(go())


def journal(d: Any, kind: str, data: dict[str, Any]) -> str:
    return str(run(d, lambda: d.daemon._journal().append(kind, data).id))


def owner_fact(d: Any, key: str, content: str, kind: str = "fact", subject: str = "owner") -> Any:
    """A memory item in the owner's own words, as `ferb memory add` saves it."""
    src = journal(d, "memory.added", {"content": content})
    return run(d, lambda: d.daemon.memory.propose(kind, subject, key, content, "owner_stated", [("journal", src)], 1.0))


def today(d: Any) -> str:
    return str(d.daemon.clock.now().astimezone().date().isoformat())


def said(d: Any, text: str, pasted: int | None = 0) -> str:
    """A chat message as intake journals it. `pasted` None: recorded before FERB counted pasted parts."""
    data: dict[str, Any] = {"session_id": "s1", "request_id": "r1", "text": text}
    if pasted is not None:
        data["untrusted_parts"] = pasted
    return journal(d, "chat.message", data)


def c_handle(d: Any, journal_id: str) -> str:
    """The C handle today's distill evidence gives that message."""
    ev = run(d, lambda: summary.build(d.daemon.db, today(d)))
    return next(h for h, refs in ev.handles.items() if h.startswith("C") and refs == [("journal", journal_id)])


def distill_item(handle: str, content: str, quote: str | None = None, **fields: Any) -> dict[str, Any]:
    """One distill answer item about the owner's dentist, citing `handle`, with a quote of that message or none."""
    return {"kind": "person", "subject": "owner", "key": "dentist", "content": content, "origin": "inferred",
            "confidence": 0.6, "handles": [handle],
            "quote": None if quote is None else {"handle": handle, "text": quote}} | fields


def distill_day(d: Any, *items: dict[str, Any]) -> dict[str, Any]:
    rt = FakeDistillRuntime({"narrative": "A day.", "items": list(items)})
    d.daemon.distiller.make_runtime = lambda *a, **k: rt
    return dict(d.run(d.daemon.distiller.run_day(today(d))))


def pending(d: Any, capability: str = "memory.confirm") -> list[Any]:
    return list(run(d, lambda: [q for q in d.daemon.state.approvals.pending() if q.capability == capability]))
```

- [ ] **Step 2: Write the failing tests**

Create `tests/test_memory_capture.py`:

```python
"""L36 §2: a safety net that saves the owner's words, and every guess FERB makes asked at once (Keep / Edit / Drop).

Live 2026-10-24 (memory audit): of the owner's durable statements in 14 days, 14 were missed and 9 only proposed; the
nightly distill made 27 inferred proposals that nobody was ever asked about."""
from typing import Any

import pytest

from ferb.core.errors import FerbError
from ferb.memory.items import MemoryRefused, capture
from ferb.sanitize.untrusted import mark, typed_lines_of
from tests.support.memory_items import (c_handle, distill_day, distill_item, journal, pending, run, said)


@pytest.mark.parametrize("message", [
    "plain words\nsecond line",
    "before \x1b[200~pasted text\x1b[201~ after",
    "look:\n```\ncode here\n```\nthen this",
    "> quoted line\nmy own line",
])
def test_typed_lines_read_back_from_a_stored_message_are_what_capture_saw(message: str) -> None:
    m = mark(message)
    assert typed_lines_of(m.for_model) == m.typed_lines


def test_capture_saves_the_owners_sentence_their_guess_or_nothing() -> None:
    own = "btw my dentist is Dr. Rao now"
    stated = capture(own, "my dentist is Dr. Rao", "my dentist is Dr. Rao")
    assert (stated.origin, stated.content, stated.refused) == ("owner_stated", own, "")
    assert capture(own, "Your dentist is Dr. Rao", "my dentist is Dr. Rao").origin == "inferred"
    assert capture(own, "Your dentist is Dr. Rao", "my dentist is Dr. Mehta").refused == "quote not in the owner's words"


def test_a_quote_of_the_owners_typed_words_saves_their_own_words(daemon: Any) -> None:
    jid = said(daemon, "btw my dentist is Dr. Rao now")
    r = distill_day(daemon, distill_item(c_handle(daemon, jid), "my dentist is Dr. Rao", "my dentist is Dr. Rao"))
    assert r["proposed"] == 1, r
    [it] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert (it.origin, it.content, it.source_ids) == ("owner_stated", "btw my dentist is Dr. Rao now",
                                                      ["journal:" + jid])
    assert pending(daemon) == []


@pytest.mark.parametrize("message, pasted, content, quote", [
    ("my dentist is Dr. Rao", 0, "Your dentist is Dr. Rao", "my dentist is Dr. Mehta"),     # not what they typed
    ("my dentist is Dr. Rao", 0, "Your dentist is Dr. Rao", None),                          # no quote
    ('note <untrusted-data id="abc123">\nmy dentist is Dr. Rao\n</untrusted-data id="abc123">', 1,
     "my dentist is Dr. Rao", "my dentist is Dr. Rao"),                                     # only in the pasted part
    ("my dentist is Dr. Rao", None, "my dentist is Dr. Rao", "my dentist is Dr. Rao"),      # pastes weren't counted
])
def test_without_the_owners_typed_words_a_guess_is_asked_at_once(daemon: Any, message: str, pasted: int | None,
                                                                 content: str, quote: str | None) -> None:
    jid = said(daemon, message, pasted)
    distill_day(daemon, distill_item(c_handle(daemon, jid), content, quote))
    [it] = run(daemon, lambda: daemon.daemon.memory.items("proposed"))
    assert (it.origin, it.content) == ("inferred", content)
    [q] = pending(daemon)
    assert q.options == ("Keep", "Edit", "Drop") and content in q.reason


def guess(daemon: Any) -> tuple[Any, Any]:
    jid = said(daemon, "my dentist is Dr. Rao")
    distill_day(daemon, distill_item(c_handle(daemon, jid), "Your dentist is Dr. Rao"))
    [q] = pending(daemon)
    [it] = run(daemon, lambda: daemon.daemon.memory.items("proposed"))
    return q, it


def test_a_guess_is_asked_about_once(daemon: Any) -> None:
    _, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._ask_keep(it, "again"))
    assert len(pending(daemon)) == 1


def test_keep_makes_a_guess_active(daemon: Any) -> None:
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 1))
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)).status == "active"


def test_drop_deletes_the_guess_alone_and_it_is_never_proposed_again(daemon: Any) -> None:
    added = journal(daemon, "memory.added", {"content": "My dentist is Dr. Mehta"})
    mine = run(daemon, lambda: daemon.daemon.memory.propose("person", "owner", "dentist", "My dentist is Dr. Mehta",
                                                            "owner_stated", [("journal", added)], 1.0))
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 3))
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)) is None
    assert run(daemon, lambda: daemon.daemon.memory.get(mine.id)).status == "active"      # the owner's version stays
    message = it.source_ids[0].split(":", 1)[1]
    assert run(daemon, lambda: [e.id for e in daemon.daemon._journal().entries_of("chat.message")]) == [message]
    with pytest.raises(MemoryRefused, match="forget"):
        run(daemon, lambda: daemon.daemon.memory.propose(it.kind, it.subject, it.key, it.content, "inferred",
                                                         [("journal", message)], 0.6))


def test_declining_a_guess_question_drops_it(daemon: Any) -> None:
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, False))
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)) is None


def test_edit_saves_the_owners_text_as_their_own_words(daemon: Any) -> None:
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 2))
    [edit] = pending(daemon, "memory.edit")
    assert edit.options == () and "Your dentist is Dr. Rao" in edit.reason
    run(daemon, lambda: daemon.daemon._resolve_approval(edit.id, True, None, "My dentist is Dr. Rao at Smile Clinic"))
    active = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert [(a.origin, a.content) for a in active] == [("owner_stated", "My dentist is Dr. Rao at Smile Clinic")]
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)).status == "retracted"


@pytest.mark.parametrize("typed, kept", [("keep", True), ("Drop", False), ("1", True), ("3", False)])
def test_an_option_typed_into_the_answer_field_picks_it(daemon: Any, typed: str, kept: bool) -> None:
    q, it = guess(daemon)
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, None, typed))
    item = run(daemon, lambda: daemon.daemon.memory.get(it.id))
    assert (item is not None and item.status == "active") is kept


def test_other_text_typed_into_a_guess_question_is_refused_and_it_stays_open(daemon: Any) -> None:
    """Review Focus: a stray "yes keep it" is never saved as a memory."""
    q, it = guess(daemon)
    with pytest.raises(FerbError, match="Pick Keep, Edit or Drop"):
        run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, None, "yes keep it"))
    assert run(daemon, lambda: daemon.daemon.state.approvals.get(q.id)).state == "pending"
    assert run(daemon, lambda: daemon.daemon.memory.get(it.id)).status == "proposed"
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `uv run pytest -q tests/test_memory_capture.py`
Expected: FAIL at import: `ImportError: cannot import name 'capture' from 'ferb.memory.items'`.

- [ ] **Step 4: Add `capture`, `owner_span` and `Memory.drop` to `ferb/memory/items.py`**

After `whole_clauses`, add:

```python
@dataclass(frozen=True)
class Capture:
    """What chat capture saves from the owner's typed words (`capture`)."""
    origin: str          # "owner_stated", "inferred", or "" when nothing is saved
    content: str
    refused: str = ""    # why nothing is saved, in code's words (CAPTURE_TOLD has the owner's)


# Each refusal as the owner reads it in FERB's line (owner 2026-10-24: the reason, never "a problem on my side")
CAPTURE_TOLD = {"quote not in the owner's words": "the words I quoted from you aren't in what you typed",
                "the quote holds nothing of what would be saved": "the words I quoted from you don't say it"}


def capture(own_words: str, content: str, quote: str) -> Capture:
    """Chat capture's check (memory review I1, MP2), shared by the planner's remember entries and distillation's quotes
    (L36 §2). `own_words`: what the owner typed (pasted parts left out), with its line breaks. The quote must be whole
    words of it. Then the whole sentence(s) holding `content` are saved as the owner's own words (`owner_sentences`);
    when the content isn't there, `content` is FERB's wording (inferred), provided the quote shares a word with it."""
    content = content.strip()
    if not contains_words(own_words, quote):
        return Capture("", content, "quote not in the owner's words")
    clause = owner_sentences(own_words, content)
    if clause is None and not set(query_terms(quote)) & set(query_terms(content)):
        return Capture("", content, "the quote holds nothing of what would be saved")
    return Capture("owner_stated", clause) if clause else Capture("inferred", content)


def owner_span(text: str, part: str) -> str | None:
    """`part` as the owner spelled it in `text` (whole words, case and spacing aside), or None."""
    found = _found(text, part)
    return text[found[0]:found[1]] if found else None
```

In `class Memory`, after `retract`, add:

```python
    def drop(self, item_id: str) -> None:
        """The owner's Drop on one of FERB's guesses (L36 §2): this proposal alone is deleted and its fingerprint kept,
        so distillation never proposes it again. The owner's messages it came from and every other version of the fact
        stay: a guess never reached a prompt, and `forget` would purge every version of its key with their sources."""
        it = self.get(item_id)
        if it is None or it.status != "proposed":
            raise MemoryRefused(f"no proposed item {item_id}")
        self.db.conn.execute("DELETE FROM memory_sources WHERE item_id = ?", (item_id,))
        self.db.conn.execute("DELETE FROM memory_items WHERE id = ?", (item_id,))
        self.db.conn.execute("INSERT OR IGNORE INTO memory_forgotten (fingerprint, at) VALUES (?, ?)",
                             (fingerprint(it.kind, it.subject, it.key, it.content), self.clock.now().isoformat()))
        self.on_settled([item_id], True)
        self.on_change()
```

- [ ] **Step 5: Add `typed_lines_of` to `ferb/sanitize/untrusted.py`**

After `_lines`, add:

```python
def typed_lines_of(for_model: str) -> str:
    """The owner's typed words with their line breaks (`Marked.typed_lines`) read back from a message as stored
    (`Marked.for_model`): each wrapped pasted or quoted part is left out and ends a line, so a saved sentence ends
    there (L36 §2: distillation checks its quotes against these, never pasted text)."""
    parts, pos = [], 0
    for m in WRAPPED.finditer(for_model):
        parts.append(for_model[pos:m.start()])
        pos = m.end()
    parts.append(for_model[pos:])
    return _lines(parts)
```

- [ ] **Step 6: Give distillation the owner's typed words per C handle (`ferb/memory/summary.py`)**

In `DayEvidence`, add the field and make `add` return its handle:

```python
    handles: dict[str, list[Source]] = field(default_factory=dict)
    # C handle -> (its chat message's journal id, the owner's typed lines): what a distilled quote is checked against
    # (L36 §2); only messages journaled with their pasted parts counted
    owner_words: dict[str, tuple[str, str]] = field(default_factory=dict)

    def add(self, prefix: str, text: str, sources: list[Source]) -> str:
        h = f"{prefix}{sum(1 for k in self.handles if k.startswith(prefix)) + 1}"
        self.handles[h] = sources
        self.lines.append(f"{h}: {text}")
        return h
```

In `build`, replace the `if own:` line in the chat loop:

```python
            if own:
                handle = ev.add("C", f"the owner said: {own}", [("journal", j["id"])])
                if d.get("untrusted_parts") is not None:      # Ruling: only rows that counted their pasted parts
                    from ferb.sanitize.untrusted import typed_lines_of
                    ev.owner_words[handle] = (j["id"], typed_lines_of(text))
```

- [ ] **Step 7: Let distillation cite the owner's words and ask about every guess (`ferb/memory/distill.py`)**

Add the schema piece and extend the item schema (strict form: every property required):

```python
# L36 §2: the owner's own words an inferred item rests on. Ruling: the model cites one of the item's C handles (it sees
# handles, not journal ids); code maps the handle to its message.
QUOTE_SCHEMA: dict[str, Any] = {"anyOf": [{"type": "null"}, {
    "type": "object", "additionalProperties": False, "required": ["handle", "text"],
    "properties": {"handle": {"type": "string"}, "text": {"type": "string"}}}]}
KEEP_GUESS = "Keep this? “{text}” FERB guessed it from your day on {day}."
```

In `DISTILL_SCHEMA`, the item's `"required"` becomes
`["kind", "subject", "key", "content", "origin", "confidence", "handles", "quote"]` and its `"properties"` gain
`"quote": QUOTE_SCHEMA`.

After `check`, add:

```python
def owner_said(item: dict[str, Any], ev: summary.DayEvidence) -> tuple[str, str] | None:
    """The owner's own words an inferred item rests on (L36 §2): its quote cites one of the item's own C handles, and
    chat capture's check (`capture`) finds the quote and the content in that message's typed lines. Ruling: a quote
    counts only on an inferred item. Their whole sentence(s) and the message's journal id, else None."""
    quote = item.get("quote")
    if item.get("origin") != "inferred" or not isinstance(quote, dict):
        return None
    handle = str(quote.get("handle", ""))
    if handle not in [str(h) for h in item.get("handles", [])] or handle not in ev.owner_words:
        return None
    journal_id, typed = ev.owner_words[handle]
    got = capture(typed, str(item.get("content", "")), str(quote.get("text", "")))
    return (got.content, journal_id) if got.origin == "owner_stated" else None
```

Import `capture` with the other items imports: `from ferb.memory.items import KINDS, Memory, MemoryRefused, capture`.

In `Distiller.__init__`, after `self.jev`, add:

```python
        # L36 §2 ("Ask right away"): each new inferred proposal becomes the owner's Keep / Edit / Drop question
        self.ask_keep: Callable[[Any, str], None] = lambda item, reason: None
```

In `run_day`, replace the body of the `try:` in the item loop:

```python
            try:
                srcs = check(item, ev)
                origin_, content = item["origin"], item["content"]
                confidence = float(item.get("confidence", 0.6))
                stated = owner_said(item, ev)
                if stated is not None:                  # exactly as if the planner had captured it in that turn
                    origin_, content, confidence, srcs = "owner_stated", stated[0], 1.0, [("journal", stated[1])]
                m = self.memory.propose(item["kind"], item["subject"], item["key"], content, origin_,  # type: ignore[arg-type]
                                        srcs, confidence)
                proposed += 1
                if m.origin == "inferred" and m.status == "proposed":
                    self.ask_keep(m, KEEP_GUESS.format(text=" ".join(m.content.split()), day=day))
            except (MemoryRefused, KeyError, ValueError) as e:
                refused.append(str(e))
```

- [ ] **Step 8: Tell the distiller about quotes (`ferb/memory/distill.md`)**

Replace the `origin "inferred"` bullet with:

```markdown
   - origin "inferred": a reasonable conclusion from the owner's activity or words (E / C handles); FERB asks the owner
     about it at once. When the owner said it in so many words, add "quote": the C handle it rests on and the owner's
     exact words copied from that line (never from pasted text); FERB checks them against what the owner typed and
     then saves the owner's own sentence as theirs. Otherwise "quote" is null. Every other origin has "quote": null.
```

- [ ] **Step 9: Questions and their options (`ferb/orchestrator/steering.py`)**

After `CONFIRM_MEMORY_ASK`, add:

```python
EDIT_MEMORY_ASK = 'memory.edit'               # L36 §2: the owner's own words for one of FERB's guesses (free text)
# FERB's questions about memory, by `memory_questions.kind` (before any ':'): their capability and options
MEMORY_QUESTIONS: dict[str, tuple[str, tuple[str, ...]]] = {
    'confirm': (CONFIRM_MEMORY_ASK, ('Keep', 'Edit', 'Drop')), 'edit': (EDIT_MEMORY_ASK, ()),
    'forget': (FORGET_ASK, ('Yes', 'No'))}
# Checks whose typed answer must be one of their options (Ruling: a stray "yes keep it" is never saved as a memory)
OPTION_ONLY = frozenset({CONFIRM_MEMORY_ASK})


def choices(options: Any) -> str:
    """Options as the owner reads them: "Keep, Edit or Drop"."""
    opts = [str(o) for o in options]
    return opts[0] if len(opts) == 1 else ", ".join(opts[:-1]) + " or " + opts[-1] if opts else ""
```

(`Any` is imported from `typing` at the top of steering.py; add it if missing.)

- [ ] **Step 10: Chat routing (`ferb/orchestrator/supervisor.py`)**

Import `EDIT_MEMORY_ASK` and `choices` from `ferb.orchestrator.steering` with the existing steering import. Change:

```python
ROUTABLE_QUESTIONS = frozenset({"owner.ask", "plan.ask", EDIT_MEMORY_ASK}) | FERB_CHECKS
```

Below `PLAIN_YES_NO`, add:

```python
PLAIN_CHOICE = "Is that {choices} for this? {question}"


def _plain_choice(ap: Any) -> str:
    """FERB's "which is it?" about its own check (L3): Yes or No, or that check's own options ("Keep, Edit or Drop")."""
    if tuple(ap.options) in ((), ("Yes", "No")):
        return PLAIN_YES_NO.format(question=ap.reason)
    return PLAIN_CHOICE.format(choices=choices(ap.options), question=ap.reason)
```

In `_answer`, in the `if ap.capability in FERB_CHECKS:` branch, replace `PLAIN_YES_NO.format(question=ap.reason)` with
`_plain_choice(ap)`.

- [ ] **Step 11: The daemon's questions (`ferb/daemon/main.py`)**

Module constants, near the other memory text:

```python
KEEP_WORDING = "Keep this, in FERB's words (you didn't say it this way)? “{text}”"
EDIT_QUESTION = ("How should FERB remember it? Type it in your own words; FERB saves exactly what you type. "
                 "(Its guess: “{text}”)")
```

`_remember`: replace the quote check and propose with `capture` (behavior unchanged for the planner's entries):

```python
        from ferb.memory.items import CAPTURE_TOLD, capture
        saved: list[dict[str, str]] = []
        for it in items:
            got = capture(own_words, str(it.content), str(it.quote))
            if got.refused:
                self._journal().append("memory.capture_refused", {"key": it.key, "why": got.refused})
                saved.append({"refused": CAPTURE_TOLD[got.refused]})
                continue
            try:
                m = self.memory.propose(it.kind, it.subject, it.key, got.content, got.origin,
                                        [("journal", source_id)], 1.0 if got.origin == "owner_stated" else 0.7)
            except FerbError as e:
                self._journal().append("memory.capture_refused", {"key": it.key, "why": str(e)})
                saved.append({"refused": reason(e, self.redact)})
                continue
            text = " ".join(m.content.split())
            if m.origin == "owner_stated":
                saved.append({"receipt": f"remembered ({m.id}): {m.content}", "line": f"Remembered: {text}"})
            else:
                self._ask_keep(m, KEEP_WORDING.format(text=text))
                saved.append({"receipt": f"proposed ({m.id}): {m.content} (FERB's wording; waits for the owner's Keep)",
                              "line": f"Saved in my words, not yours, so I won't use it until you answer Keep: “{text}”."})
        return saved
```

`_ask_owner`: take capability and options from the kind:

```python
        from ferb.core.types import Identity
        from ferb.orchestrator.steering import MEMORY_QUESTIONS
        from ferb.policy.rules import ActionRequest
        capability, options = MEMORY_QUESTIONS[kind.split(":", 1)[0]]
        action = ActionRequest(capability, "owner_question", Identity.OWNER, frozenset(), "", "", request_id, frozenset())
        ap = self.state.approvals.question(action, reason, list(options), approval_id)
```

(the rest of `_ask_owner` is unchanged; it already publishes `list(ap.options)`).

Add after `_ask_owner`:

```python
    def _ask_keep(self, item: Any, reason: str) -> None:
        """L36 §2 ("Ask right away"): one of FERB's guesses about the owner, asked at once with Keep / Edit / Drop;
        never twice for one item (an open Keep or Edit question about it is enough)."""
        if self.db.conn.execute("SELECT 1 FROM memory_questions WHERE target = ? AND kind IN ('confirm', 'edit')",
                                (item.id,)).fetchone():
            return
        self._ask_owner("confirm", item.id, reason, "memory-confirm-" + item.id)

    def _settle_guess(self, item_id: str, action: str, text: str = "") -> str:
        """The owner's answer to one of FERB's guesses (L36 §2). Keep makes it active, owner-confirmed. Edit saves the
        owner's text as their own words (a new owner-stated version of the same fact) and retracts the guess. Drop
        deletes the guess alone (`Memory.drop`; Ruling: never `forget`, which purges every version and the owner's
        messages). A line for the owner; FerbError for an unknown action or an empty edit."""
        item = self.memory.get(item_id)
        if item is None or item.status != "proposed" or item.origin != "inferred":
            return "That guess isn't waiting any more."
        if action == "keep":
            self.memory.confirm(item_id)
            return "Kept."
        if action == "drop":
            self.memory.drop(item_id)
            self._forget_nodes(["memory:" + item_id], named=False)      # its index node goes with it
            return "Dropped. FERB won't suggest it again."
        if action == "edit" and text.strip():
            entry = self._journal().append("memory.edited", {"id": item_id, "content": text.strip()})
            mine = self.memory.edit(item_id, text.strip(), ("journal", entry.id))
            self.memory.retract(item_id, "the owner wrote it in their own words")
            return "Saved in your words: " + " ".join(mine.content.split())
        raise FerbError("Pick Keep, Edit or Drop; Edit needs the words to save.")
```

`_question_answered`: replace the `if what == "confirm":` block with:

```python
        if what == "confirm":
            # Ruling: a declined question is Drop (skipping FERB's own check is a No, L3)
            choice = ap.choice if ap.state == "granted" else "Drop"
            if choice == "Edit":
                item = self.memory.get(ident)
                if item is None or item.status != "proposed":
                    return "That guess isn't waiting any more."
                self._ask_owner("edit", ident, EDIT_QUESTION.format(text=" ".join(item.content.split())),
                                "memory-edit-" + ident)
                return ""
            return self._settle_guess(ident, "keep" if choice == "Keep" else "drop")
        if what == "edit":
            if ap.state == "granted" and ap.answer:
                return self._settle_guess(ident, "edit", ap.answer)
            return self._settle_guess(ident, "drop")
```

`_restore_memory_questions`: the `gone` test for items becomes

```python
                gone = item is None or (row["kind"] in ("confirm", "edit") and item.status != "proposed")
```

`_build_memory`: after the `Distiller(...)` line add `self.distiller.ask_keep = self._ask_keep`, and change the
settled hook to close Edit questions too:

```python
        self.memory.on_settled = lambda ids, deleted: self._close_memory_questions(ids, None if deleted else
                                                                                  ("confirm", "edit"))
```

`_resolve_approval`: in the branch that strips ordinary typed text (`text = text.strip()` / `granted = bool(text)`),
add right after those two lines:

```python
                from ferb.orchestrator.steering import OPTION_ONLY, choices
                if ap is not None and ap.capability in OPTION_ONLY and text and ap.option_for(text) is None:
                    raise FerbError(f"Pick {choices(ap.options)}.")
```

and, further down, publish `memory.forgot` only for a forget question:

```python
            if followed and ap.capability == "memory.forget":
                self._publish("memory.forgot", {"approval_id": ap.id, "text": followed})
```

- [ ] **Step 12: The CLI names a check's options (`ferb/cli/live_chat.py`)**

Replace the `'   Reply Yes or No.'` line:

```python
        from ferb.orchestrator.steering import choices
        self.say('   Reply ' + choices(data.get('options') or ['Yes', 'No']) + '.'
                 if data.get('capability') in FERB_CHECKS else '   Reply here to answer.')
```

- [ ] **Step 13: Update the tests whose expectations change**

- `tests/test_memory.py::test_a_paraphrase_waits_for_the_owners_confirm_before_it_shapes_prompts`: replace
  `"Reply Yes" in saved[0]["line"]` with `"answer Keep" in saved[0]["line"]`.
- `tests/test_memory.py::test_a_paraphrase_can_be_confirmed_from_chat_with_a_plain_yes`: rename to
  `..._with_a_plain_keep`; expect `question.options == ("Keep", "Edit", "Drop")`; the routed answer and the chat text
  are `"Keep"`.
- `tests/test_memory.py::test_distillation_proposals_are_checked_in_code`: its `item()` helper's dict gains
  `"quote": None`.
- `tests/test_decide.py::test_memory_proposals_jev_rejects_are_dropped_when_enabled`: the item gains `"quote": None`.

- [ ] **Step 14: Run the focused tests**

Run: `uv run pytest -q tests/test_memory_capture.py tests/test_memory.py tests/test_decide.py tests/test_context_sources_026.py tests/test_orchestrated_control.py`
Expected: PASS.

- [ ] **Step 15: Decision registry and docs**

Append to `docs/0.27_DECISIONS.toml`:

```toml
[L36]
milestone = "L"
tests = ["tests/test_memory_capture.py"]
note = "Owner 2026-10-24 approved the direction of \"memory and learning that work\" and answered its questions: \"BEFORE LAUNCH\"; guesses \"Ask right away\"; memory size \"Planner 60, worker 20\"; usernames \"Yes, plain\". Spec docs/superpowers/specs/2026-10-24-memory-that-works-design.md; plan docs/superpowers/plans/2026-10-24-memory-that-works.md. Section 1 (capture on every turn path) landed in 1c22dcb and f747d37."
summary = "Memory that works: the owner's quoted words saved by distillation, every new guess asked at once (Keep / Edit / Drop) and the older ones reviewed once as one list; one ranked content search for the planner (60), workers and plan steps (20, with the parent goal), memory.read and memory.learned; one login record per site with a plain username and sealed entries, listed by secrets.list without values; dated facts retire; sealed values redacted as whole tokens and never inside FERB's ids; procedures compared modulo per-run handles with app-version ranges, and their use recorded."
```

Append to the end of `ARCHITECTURE.md` (after §16.11):

```markdown
### 16.12 Memory that works (L36)

Spec `docs/superpowers/specs/2026-10-24-memory-that-works-design.md`; plan
`docs/superpowers/plans/2026-10-24-memory-that-works.md`. Capture on every turn path landed first (`1c22dcb`,
`f747d37`).
- **Safety net (§2).** Distillation may give an inferred item a `quote` {handle, text} citing one of its own C handles;
  `distill.owner_said` runs chat capture's check (`items.capture`: the quote is whole words of the message's typed lines,
  read back by `untrusted.typed_lines_of`, and the content is found there) and, when it passes, saves the owner's whole
  sentence(s) as `owner_stated` with the message's journal entry as its source. Only a message journaled with
  `untrusted_parts` counts (`DayEvidence.owner_words`). Every new inferred item, from chat capture or distillation, is
  asked at once (`Daemon._ask_keep`; kind `confirm`, capability `memory.confirm`, options Keep / Edit / Drop, kept in
  `memory_questions` until answered). Keep confirms. Edit asks a free-text `memory.edit` question and saves the answer
  as a new owner-stated version, retracting the guess. Drop, and a declined question, delete that guess alone and keep
  its fingerprint (`Memory.drop`), so it is never proposed again; `forget` is not used, since it purges every version of
  the key and the owner's messages. Text typed into the Keep / Edit / Drop question that is no option is refused
  (`steering.OPTION_ONLY`).
```

In `README.md` "Memory and learning", after the sentence about "forget my …", add:

```markdown
When FERB guesses something about you that you didn't say in those words (from chat, or from its nightly look at your
day), it asks at once: "Keep this? …" with Keep, Edit and Drop. Edit asks for your own words and saves them as yours;
Drop deletes the guess (your messages stay) and FERB never suggests it again. The nightly look saves what you said in so
many words as your own, after checking the words against what you typed (never pasted text).
```

In `CHANGELOG.md`, add at the end of the "After the owner's update (2026-10-24):" list:

```markdown
- What you say is saved even when the planner skips it, and every guess is asked about (L36, MP2). Live 2026-10-24
  (memory audit): 14 of your durable statements in two weeks were never saved, and the nightly distill had made 27
  guesses about you that nobody asked you about. Distillation can now quote your own words for an item; FERB checks the
  quote against what you typed (never pasted text) and saves your sentence as yours. Any guess, from chat or the nightly
  look, becomes a question at once: Keep, Edit (your own words are saved instead) or Drop (deleted; never suggested
  again). Text typed into that question that isn't one of the three is refused, so a stray "yes" is never saved.
```

- [ ] **Step 16: Full checks**

Run: `uv run pytest -q` then `uv run mypy ferb/core` then `git diff --check`
Expected: all pass; no whitespace errors.

- [ ] **Step 17: Commit and push**

```bash
git add ferb/memory/items.py ferb/sanitize/untrusted.py ferb/memory/summary.py ferb/memory/distill.py \
  ferb/memory/distill.md ferb/orchestrator/steering.py ferb/orchestrator/supervisor.py ferb/daemon/main.py \
  ferb/cli/live_chat.py tests/support/memory_items.py tests/test_memory_capture.py tests/test_memory.py \
  tests/test_decide.py docs/0.27_DECISIONS.toml ARCHITECTURE.md README.md CHANGELOG.md
git commit -m "feat(L36, MP2): distill saves the owner's quoted words; every guess is asked at once with Keep, Edit or Drop

The nightly distill may quote the owner's own words for an inferred item; code checks the quote against the
message's typed lines (never pasted text) and saves the owner's sentence as owner_stated. Every new guess, from
chat capture or distill, becomes a Keep / Edit / Drop question at once; Drop deletes the guess alone and keeps
its fingerprint, Edit saves the owner's words as a new version.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 2: The older guesses, reviewed once as one list (spec §2, ruling "The backlog")

**Files:**
- Modify: `ferb/daemon/main.py` (`BACKLOG_OFFERED`, `_guess_backlog`; `_memory_learning` ops `backlog`, `guess`)
- Modify: `ui/mac/FERBNotch/Sources/NotchCore/ConversationArchive.swift` (`LearningProposal.kind`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/DaemonConnection.swift` (`fetchGuessBacklog`, `decideGuess`,
  `refreshDaemonStatus`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/ConversationTranscript.swift` (`GuessRow`, used by `NoticeView`)
- Test: `tests/test_memory_capture.py`, `ui/mac/FERBNotch/Tests/NotchCoreTests/QuestionsAndNoticesTests.swift`
- Docs: `ui/mac/FERBNotch/README.md`, `README.md`, `ARCHITECTURE.md` §16.12, `CHANGELOG.md`

**Interfaces:**
- Consumes: `Daemon._settle_guess(item_id, action, text)` (Task 1).
- Produces: RPC `memory.learning {op: "backlog"}` -> `{"backlog": {"id", "text", "proposals": [{"id", "text"}]} | None}`
  (offered once); RPC `memory.learning {op: "guess", id, action: keep|edit|drop, text?}` -> `{"line": str}`;
  Swift `LearningProposal(id:text:decision:kind:)`, `NotchModel.fetchGuessBacklog()`,
  `NotchModel.decideGuess(noticeID:proposalID:action:text:)`.

Ruling: the list reuses the notch's learning-digest notice, which already shows a list with per-item buttons in
today's chat; it is offered once (a journal marker), and only the notch fetches it, so `ferb chat` can't use it up.

- [ ] **Step 1: Write the failing Python test** (append to `tests/test_memory_capture.py`)

```python
def test_the_older_guesses_are_offered_once_as_one_list(daemon: Any) -> None:
    src = said(daemon, "x")
    old = [run(daemon, lambda c=c: daemon.daemon.memory.propose("fact", "owner", f"k{c}", f"guess number {c}",
                                                                "inferred", [("journal", src)], 0.6)) for c in range(3)]
    run(daemon, lambda: daemon.daemon._ask_keep(old[2], "Keep this?"))        # asked already: not in the list
    first = daemon.run(daemon.daemon._memory_learning(None, {"op": "backlog"}))["backlog"]
    assert first["text"] == "Review 2 older guesses"
    assert [p["id"] for p in first["proposals"]] == [old[0].id, old[1].id]
    assert daemon.run(daemon.daemon._memory_learning(None, {"op": "backlog"}))["backlog"] is None
    kept = daemon.run(daemon.daemon._memory_learning(None, {"op": "guess", "id": old[0].id, "action": "keep"}))
    assert kept["line"] == "Kept." and run(daemon, lambda: daemon.daemon.memory.get(old[0].id)).status == "active"
    with pytest.raises(FerbError):
        daemon.run(daemon.daemon._memory_learning(None, {"op": "guess", "id": old[1].id, "action": "edit", "text": " "}))
    edited = daemon.run(daemon.daemon._memory_learning(None, {"op": "guess", "id": old[1].id, "action": "edit",
                                                              "text": "my own words"}))
    assert edited["line"] == "Saved in your words: my own words"
```

- [ ] **Step 2: Run it to verify it fails**

Run: `uv run pytest -q tests/test_memory_capture.py::test_the_older_guesses_are_offered_once_as_one_list`
Expected: FAIL (`KeyError: 'backlog'` or an unknown-op error from `_memory_learning`).

- [ ] **Step 3: Implement the ops (`ferb/daemon/main.py`)**

Constant: `BACKLOG_OFFERED = "memory.backlog_offered"`. In `_memory_learning`, right after
`op, ident = params.get("op", "inventory"), str(params.get("id", ""))`:

```python
        if op == "backlog":
            return {"backlog": self._guess_backlog()}
        if op == "guess":
            return {"line": self._settle_guess(ident, str(params.get("action", "")), str(params.get("text") or ""))}
```

Add the method next to `_settle_guess`:

```python
    def _guess_backlog(self) -> dict[str, Any] | None:
        """L36 ruling ("The backlog"): FERB's guesses that were waiting before it asked about each at once, offered
        once as one "Review N older guesses" list (each Keep / Edit / Drop, through op "guess"); a guess already asked
        about is not in it. Marked offered whenever it is read, so it never comes back; None when there is nothing."""
        if self._journal().of_kind(BACKLOG_OFFERED):
            return None
        asked = {r["target"] for r in self.db.conn.execute("SELECT target FROM memory_questions")}
        waiting = [i for i in self.memory.items("proposed") if i.origin == "inferred" and i.id not in asked]
        entry = self._journal().append(BACKLOG_OFFERED, {"count": len(waiting)})
        if not waiting:
            return None
        return {"id": entry.id, "text": f"Review {len(waiting)} older guess{'' if len(waiting) == 1 else 'es'}",
                "proposals": [{"id": i.id, "text": " ".join(i.content.split())} for i in waiting]}
```

- [ ] **Step 4: Run the Python test**

Run: `uv run pytest -q tests/test_memory_capture.py`
Expected: PASS.

- [ ] **Step 5: Write the failing Swift test** (add to `QuestionsAndNoticesTests` next to
  `testTheLearningDigestIsShownWithConfirmAndNotRightWhileTheOwnerIsLooking`)

```swift
    func testOlderGuessesAreShownOnceWithKeepEditAndDrop() async throws {
        let rpc = StatusRPC()
        rpc.responses["memory.learning"] = ["backlog": ["id": "b-1", "text": "Review 2 older guesses", "proposals": [
            ["id": "g-1", "text": "You use Brave for work"], ["id": "g-2", "text": "You study at Scaler"]
        ]]]
        let model = makeModel(rpc)
        model.expand()
        await model.refreshDaemonStatus()
        XCTAssertTrue(rpc.calls.contains { $0.0 == "memory.learning" && $0.1["op"] as? String == "backlog" })
        let backlog = try XCTUnwrap(model.conversation.notices.first { $0.id == "guesses:b-1" })
        XCTAssertEqual(backlog.text, "Review 2 older guesses")
        XCTAssertEqual(backlog.proposals?.map(\.kind), ["guess", "guess"])

        let decided = expectation(description: "Both decisions sent")
        decided.expectedFulfillmentCount = 2
        rpc.onCall = { method, params in
            if method == "memory.learning", params["op"] as? String == "guess" { decided.fulfill() }
        }
        model.decideGuess(noticeID: backlog.id, proposalID: "g-1", action: "keep", text: nil)
        model.decideGuess(noticeID: backlog.id, proposalID: "g-2", action: "edit", text: "I studied at Scaler")
        await fulfillment(of: [decided], timeout: 2)
        rpc.onCall = nil
        await settle()
        let sent = rpc.calls.filter { $0.0 == "memory.learning" && $0.1["op"] as? String == "guess" }
            .map { "\($0.1["id"] as? String ?? "?")=\($0.1["action"] as? String ?? "?"):\($0.1["text"] as? String ?? "")" }
        XCTAssertEqual(sent.sorted(), ["g-1=keep:", "g-2=edit:I studied at Scaler"])
        XCTAssertEqual(model.conversation.notices.first { $0.id == backlog.id }?.proposals?.map(\.decision),
                       ["kept", "edited"])
        await model.refreshDaemonStatus()
        XCTAssertEqual(model.conversation.notices.filter { $0.id.hasPrefix("guesses:") }.count, 1)
    }
```

- [ ] **Step 6: Run it to verify it fails**

Run: `cd ui/mac/FERBNotch && swift test --filter QuestionsAndNoticesTests/testOlderGuessesAreShownOnceWithKeepEditAndDrop`
Expected: FAIL to compile (`value of type 'LearningProposal' has no member 'kind'`).

- [ ] **Step 7: Implement the notch side**

`ConversationArchive.swift`, `LearningProposal`:

```swift
public struct LearningProposal: Codable, Equatable {
    public let id: String
    public let text: String
    /// "confirmed" or "not right" for a learning digest item; "kept", "dropped" or "edited" for a guess; nil while
    /// undecided.
    public var decision: String?
    /// "guess" for one of FERB's older guesses about the owner (L36: Keep, Edit or Drop); nil for a digest item.
    public var kind: String?

    public init(id: String, text: String, decision: String? = nil, kind: String? = nil) {
        self.id = id
        self.text = text
        self.decision = decision
        self.kind = kind
    }
}
```

`DaemonConnection.swift`: in `refreshDaemonStatus`, the digest line becomes

```swift
        if bodyVisible && viewingToday { await fetchLearningDigest(); await fetchGuessBacklog() }
```

and after `decideLearning` add:

```swift
    /// FERB's guesses that were waiting before it asked about each at once (L36): offered once as one list
    /// (`memory.learning {op: backlog}`, which marks it offered), shown at once in today's chat.
    func fetchGuessBacklog() async {
        guard let reply = try? await rpc.call("memory.learning", params: ["op": "backlog"]),
              let backlog = reply["backlog"] as? [String: Any], let id = backlog["id"] as? String,
              let text = backlog["text"] as? String else { return }
        let proposals = (backlog["proposals"] as? [[String: Any]] ?? []).compactMap { item -> LearningProposal? in
            guard let pid = item["id"] as? String, !pid.isEmpty, let ptext = item["text"] as? String else { return nil }
            return LearningProposal(id: pid, text: ptext, kind: "guess")
        }
        guard !proposals.isEmpty,
              conversation.addNotice(id: "guesses:" + id, text: text, tag: nil, proposals: proposals) else { return }
        saveConversation()
        onBackgroundChange?(true)
    }

    /// The owner's answer to one older guess: Keep, Drop, or their own words saved instead (`edit`).
    func decideGuess(noticeID: String, proposalID: String, action: String, text: String?) {
        Task {
            do {
                var params: [String: Any] = ["op": "guess", "id": proposalID, "action": action]
                if let text { params["text"] = text }
                _ = try await rpc.call("memory.learning", params: params)
                let decision = ["keep": "kept", "drop": "dropped", "edit": "edited"][action] ?? action
                conversation.decide(noticeID: noticeID, proposalID: proposalID, decision: decision)
                saveConversation()
            } catch { promptError = "That answer about FERB's guess wasn't recorded. Try again." }
        }
    }
```

`ConversationTranscript.swift`: in `NoticeView`, the proposals loop shows a guess with its own row:

```swift
                ForEach(notice.proposals ?? [], id: \.id) { proposal in
                    if proposal.kind == "guess" {
                        GuessRow(notice: notice, proposal: proposal, model: model)
                    } else {
                        // the existing digest item VStack, unchanged
                    }
                }
```

(move the existing `VStack(alignment: .leading, spacing: 5) { … }.padding(.top, 2)` into the `else` branch as it is),
and add below `NoticeView`:

```swift
/// One of FERB's older guesses (L36): Keep, Drop, or the owner's own words saved instead.
private struct GuessRow: View {
    let notice: ConversationNotice
    let proposal: LearningProposal
    @ObservedObject var model: NotchModel
    @State private var words = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(proposal.text).textSelection(.enabled)
            if let decision = proposal.decision {
                Text(["kept": "Kept", "dropped": "Dropped", "edited": "Saved in your words"][decision] ?? decision)
                    .font(.system(size: 10)).foregroundStyle(Palette.secondary)
            } else {
                HStack(spacing: 8) {
                    Button("Keep") {
                        model.decideGuess(noticeID: notice.id, proposalID: proposal.id, action: "keep", text: nil)
                    }.buttonStyle(FerbButtonStyle())
                    Button("Drop") {
                        model.decideGuess(noticeID: notice.id, proposalID: proposal.id, action: "drop", text: nil)
                    }.buttonStyle(FerbButtonStyle(quiet: true))
                }
                TextField("Or type it in your own words…", text: $words)
                    .textFieldStyle(.plain).padding(6)
                    .background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 8))
                    .onSubmit {
                        let typed = words.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !typed.isEmpty else { return }
                        words = ""
                        model.decideGuess(noticeID: notice.id, proposalID: proposal.id, action: "edit", text: typed)
                    }
            }
        }
        .padding(.top, 2)
    }
}
```

- [ ] **Step 8: Run the Swift tests and build**

Run: `cd ui/mac/FERBNotch && swift test && ./build.sh`
Expected: PASS; the build succeeds. (Do not relaunch or inspect the running app; the owner evaluates visuals.)

- [ ] **Step 9: Docs**

`ui/mac/FERBNotch/README.md`, after the sentence about the learning digest: "Once, while today's chat is open, FERB's
older guesses about you (`memory.learning {op:"backlog"}`, which marks them offered) appear as one "Review N older
guesses" list: Keep, Drop, or type it in your own words, each sent as `memory.learning {op:"guess"}`."

`README.md` "Memory and learning", after Task 1's paragraph: "The guesses FERB made before it started asking appear
once in the notch as one list, "Review N older guesses", each with Keep, Drop or a field for your own words."

`ARCHITECTURE.md` §16.12, new bullet: "- **The older guesses (§2 ruling).** `memory.learning {op: backlog}` returns the
inferred proposals with no open question once (journal `memory.backlog_offered`); the notch shows them as one list in
today's chat (a learning-digest notice whose items have `kind: "guess"`), and each answer goes through
`memory.learning {op: guess}` to `Daemon._settle_guess`."

`CHANGELOG.md` bullet: "- The 27 guesses FERB had made about you without asking appear once in the notch as one list,
"Review 27 older guesses", each with Keep, Drop or your own words (L36)."

- [ ] **Step 10: Full checks, commit and push**

Run: `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`.

```bash
git add ferb/daemon/main.py ui/mac/FERBNotch/Sources/NotchCore/ConversationArchive.swift \
  ui/mac/FERBNotch/Sources/FERBNotch/DaemonConnection.swift ui/mac/FERBNotch/Sources/FERBNotch/ConversationTranscript.swift \
  ui/mac/FERBNotch/Tests/NotchCoreTests/QuestionsAndNoticesTests.swift ui/mac/FERBNotch/README.md \
  tests/test_memory_capture.py README.md ARCHITECTURE.md CHANGELOG.md
git commit -m "feat(L36): the older guesses appear once in the notch as one Keep, Edit or Drop list

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 3: One ranked search with the owner's budgets (spec §3)

**Files:**
- Create: `ferb/store/migrations/0048_learning_search_version.sql` (use the next free number if 0048 is taken)
- Modify: `ferb/memory/learning.py` (constants, `search_text`, `shown_text`, `_tied`, `reindex_search`,
  `Learning.weighted_terms`, `Learning.search`, `Learning.put`)
- Modify: `ferb/memory/embeddings.py` (`index`, `index_async` embed `search_text`)
- Modify: `ferb/daemon/main.py` (`start`: re-index; `_query_vector`, `_search`, `_learning_candidates`)
- Modify: `ferb/orchestrator/context_layers.py` (`target_words`)
- Modify: `ferb/orchestrator/supervisor.py` (`_chat_task_words`; `_supervise` recalls for the message plus those words)
- Create: `tests/support/memory_corpus.py`, `tests/test_memory_search.py`, `tests/support/live_steer.py` (moved from
  `tests/test_control_turn_entries.py`, which then imports it)
- Docs: `ARCHITECTURE.md` (§8's retrieval sentence and §16.12), `README.md`, `CHANGELOG.md`; add
  `"tests/test_memory_search.py"` to `[L36].tests`

**Interfaces:**
- Consumes: `query_terms` (items.py); the existing `learning_fts` / `learning_titles` tables; `Continuity.candidates`,
  `Continuity.load`, `Continuity.task_ids`, `Supervisor._attempts` (what the planner's task layers show).
- Produces (learning.py): `PLANNER_MEMORY_ITEMS = 60`, `WORKER_MEMORY_ITEMS = 20`,
  `PROMPT_LEAVES_OUT = frozenset({'identity', 'episode_intent', 'procedure'})`, `SEARCH_TEXT_VERSION = 'content/1'`,
  `slug_words(slug: str) -> str`, `search_text(kind: str, key: str, body: dict) -> str`, `shown_text(node: dict) -> str`,
  `reindex_search(db) -> bool`, `Learning.weighted_terms(words: list[str]) -> list[str]`,
  `Learning.search(query, *, kind=None, app=None, person=None, since=None, until=None, query_vector=None, embed=True,
  limit: int | None = None, leave_out: frozenset[str] = frozenset()) -> list[dict]`.
  Daemon: `_query_vector(words) -> tuple[list[int] | None, bool]`,
  `_search(query, *, limit=None, leave_out=frozenset(), **filters) -> list[dict]` (async),
  `_learning_candidates(words, budget=PLANNER_MEMORY_ITEMS, procedures=True)`.
  context_layers: `target_words(targets: TargetsSpec) -> str`. Supervisor: `_chat_task_words(session) -> str`.

Ruling (index): fact = content plus the words of its subject and key slugs (`slug_words`: `usvisascheduling_username`
-> "usvisascheduling username", `project:us-b1b2-visa` -> "us b1b2 visa"; the item's own words, not JSON field names);
entity, identity, identity proposal = key and text; episode = goal and
outcome (narrative, status); procedure = intent and UI hints; lesson = trigger and text; task reflection = reflection
and narrative; a day narrative = its text. Day digests, rollups and intent labels index nothing: they are code counts,
navigation and bookkeeping, found through what they cite.
Ruling (ranking): recency adds at most 0.05 (the gap between ranks 4 and 5), halving every 30 days; equal bm25 scores
share a rank, so "owner first at equal relevance" can apply.
Ruling (protection): the memory blocks stay protected, bounded by the owner's budgets. Unprotected blocks are filled
only after the conversation, which fills the planner's window, and the planner has no tool to fetch one it lost.
Ruling (episodes): an episode's outcome is its status and whether it was verified; its answer stays in the card.
Ruling (attention): attention keeps `memory_candidates`' default, the planner's 60.
Ruling (planner query): a planner turn recalls for the owner's message plus the goals and `target_words` of the tasks
its task layers show: this chat's running tasks and the resumable attempts `_attempts` shows it (recently ended ones
among them). An owner message that shares no word with a fact ("hmm idk if thats s agood strategy…") still recalls it
when the work it continues names the site.

- [ ] **Step 0: Move the live-steer helpers to shared support**

Create `tests/support/live_steer.py` with `_Stalled` renamed `StalledWorker` and `_control` renamed `control_turn`, their
code unchanged (copy them from `tests/test_control_turn_entries.py`, with the imports they use: `asyncio`, `json`,
`threading`, `Any`, `pytest`, `RuntimeEvent` from `ferb.core.types`, and `FakeRuntime`, `done`, `sup`, `task` from
`tests.conftest`). In `tests/test_control_turn_entries.py`, delete both definitions, import
`from tests.support.live_steer import StalledWorker, control_turn`, and rename their uses. Run
`uv run pytest -q tests/test_control_turn_entries.py`: PASS.

- [ ] **Step 1: Write the corpus fixture** (`tests/support/memory_corpus.py`)

```python
"""A made-up store shaped like the owner's on 2026-10-24 (memory audit: 102 past tasks, 45 day narratives, 25 task
reflections, 21 facts, 14 entity summaries), for ranking tests (L36 §3). Invented text only (Ruling: the owner's chat
lines never enter the repository); every text holds the words nearly all of them share."""
from typing import Any

from ferb.memory.learning import Learning

TOPICS = ("gmail inbox", "kaggle leaderboard", "zomato order", "github pull request", "whatsapp group",
          "spotify playlist", "instagram reel", "scaler assignment", "resume pdf", "discord server", "brave profile",
          "chrome tab", "calendar invite", "flight booking", "notes folder", "desktop files", "terminal command",
          "youtube video", "linkedin post", "rsna competition")
USERNAME = "the username is Asha Rao"


def topic(n: int) -> str:
    return TOPICS[n % len(TOPICS)]


def fact(store: Learning, key: str, content: str, *, owner: bool = True, subject: str = "owner") -> dict[str, Any]:
    """A fact node as `Learning.sync_facts` indexes a memory item."""
    body = {"id": "item-" + key, "kind": "fact", "subject": subject, "key": key, "content": content,
            "origin": "owner_stated" if owner else "inferred", "status": "active", "unconfirmed_hint": False,
            "observed_at": "2026-10-21T00:00:00+00:00", "valid_from": "2026-10-21T00:00:00+00:00", "valid_to": None,
            "supersedes": None, "confidence": 1.0, "source_ids": []}
    return store.put("fact", f"{subject}/{key}", body, [], origin_label="owner" if owner else "inferred",
                     made_by="code:fact-index/1", state="active", owner=owner)


def owner_like_store(store: Learning) -> None:
    for n in range(102):
        store.put("episode", f"task{n:03d}", {
            "task_id": f"task{n:03d}", "intent": f"Check the {topic(n)} for the owner and report what is new",
            "request": f"check my {topic(n)}", "status": "done", "verified": n % 3 == 0,
            "narrative": f"The {topic(n)} is up to date and there is nothing new for the owner.",
            "ended_at": f"2026-10-{1 + n % 24:02d}T10:00:00+00:00", "actions": [], "apps": {}, "slots": {}},
            [], origin_label="ferb", made_by="code:evidence-card/4", state="active")
    for n in range(45):
        store.put("activity_summary", f"trail.day_narrative:2026-09-{1 + n % 30:02d}:{n}", {
            "day": f"2026-09-{1 + n % 30:02d}",
            "text": f"The owner spent the day on the {topic(n)} and the {topic(n + 7)}."},
            [], origin_label="inferred", made_by="model:test", state="active")
    for n in range(25):
        store.put("episode_annotation", f"task{n:03d}", {
            "reflection": f"The {topic(n)} is easier for the owner in the morning.",
            "narrative": "It went fine and the owner is happy."},
            [], origin_label="inferred", made_by="model:test", state="active")
    for n in range(21):
        fact(store, f"note_{n}", f"the {topic(n)} is kept in the {topic(n + 3)} for the owner")
    for n in range(14):
        store.put("entity", f"project:{topic(n).replace(' ', '-')}", {
            "text": f"The {topic(n)} is a project of the owner and it is active."},
            [], origin_label="inferred", made_by="code:entity-summary/1", state="active")
```

- [ ] **Step 2: Write the failing tests** (`tests/test_memory_search.py`)

```python
"""L36 §3: one ranked search over content, with the owner's budgets ("Planner 60, worker 20")."""
import threading
from pathlib import Path
from typing import Any

import pytest

from ferb.memory.learning import (PLANNER_MEMORY_ITEMS, SEARCH_TEXT_VERSION, WORKER_MEMORY_ITEMS, encoded,
                                  reindex_search, search_text, shown_text, slug_words)
from ferb.orchestrator.context_layers import target_words
from ferb.orchestrator.schemas import TargetsSpec
from tests.conftest import chat, install, sup, task
from tests.support.learning_fixtures import new_store
from tests.support.live_steer import StalledWorker
from tests.support.memory_corpus import USERNAME, fact, owner_like_store
from tests.support.memory_items import owner_fact

# The audit's real texts (scratchpad memaudit/repro.py and the live task 0a5fbe31, 2026-10-24); only the owner's email
# address is replaced (Ruling 25). The owner's 12:17 message created the task; 12:22 corrected it.
MSG_1217 = ("hmm idk if thats s agood strategy for this, cause slots can pen anytime. search online extensilvely for "
            "this and then propose what is a good strategy")
MSG_1222 = ("also, wherever its available, book it. dont just wait for chennai. first ofc then counselar. search what "
            "info you’d require at booking the slot stpe. if nothting is req from me then just booook it. dont wait for "
            "me. book and then tell me")
VISA_GOAL = ("Update the ongoing US B1/B2 appointment strategy work for your `owner@example.com` account in Chrome "
             "`New_Personal`: optimize for actually securing an appointment without paying an agent. Do not restrict "
             "the search to Chennai—look for the earliest available valid OFC appointment first and corresponding "
             "consular interview across any Indian locations the official system permits, with any date acceptable. "
             "Research exactly what information, documents, selections, or decisions the official booking flow "
             "requires before the final booking step. Use your securely stored login and security answers where "
             "applicable. If no new information or decision is required from you when a valid appointment pair "
             "appears, book it immediately without waiting for you, then report the exact OFC and consular locations, "
             "dates, and confirmation details. If the booking flow requires a genuinely missing personal choice or "
             "information, ask only for that. Also determine and implement a monitoring frequency that materially "
             "improves your chance of getting a slot while respecting official limits and avoiding account blocks, "
             "bots, paid agents, or unsafe extensions.")
VISA_TARGETS = TargetsSpec(apps=["Chrome New_Personal"], sites=["usvisascheduling.com"])
RESEARCH_OFFICIAL = ("Collect current official guidance on India B1/B2 appointment availability, scheduling rules, "
                     "rescheduling limits, account access, and automation or abuse restrictions from official US "
                     "government and scheduling sources.")
PLANNER_QUERIES = ["log in to usvisascheduling.com",
                   "Check usvisascheduling.com in Chrome New_Personal for Chennai OFC and interview availability",
                   "usvisascheduling B1 B2 India OFC consular booking fields New_Personal owner-account"]


@pytest.fixture
def store(tmp_path: Path) -> Any:
    db, learning = new_store(tmp_path / "memory.db")
    owner_like_store(learning)
    yield learning
    db.close()


def ranked(store: Any, query: str, node_id: str, limit: int) -> bool:
    return node_id in [n["id"] for n in store.search(query, limit=limit)]


def test_target_and_slug_words_are_the_items_and_tasks_own_words() -> None:
    assert target_words(TargetsSpec(sites=["https://www.usvisascheduling.com/en-US/login", "usvisascheduling.com"],
                                    apps=["Chrome New_Personal"], recipients=["Asha"],
                                    paths=["/Users/owner/Desktop/proofOfWork/CV/Resume_2026.pdf"])) == \
        "www usvisascheduling com usvisascheduling com Chrome New_Personal Asha Resume_2026.pdf"
    assert slug_words("usvisascheduling_username") == "usvisascheduling username"
    assert slug_words("project:us-b1b2-visa") == "us b1b2 visa"


def test_the_owners_visa_messages_recall_the_username_through_the_task_they_continue(store: Any) -> None:
    """The audit's reproduction as a fixture: the fact sat at rank 167-201 of 207 unranked lines. The owner's 12:17 and
    12:22 messages share no word with it; the running visa task they continue names the site (Ruling 12a), so the
    planner's query (message, then that task's goal and targets) recalls it within the 60."""
    username = fact(store, "usvisascheduling_username", USERNAME)
    continued = VISA_GOAL + "\n" + target_words(VISA_TARGETS)
    for message in (MSG_1217, MSG_1222):
        assert ranked(store, message + "\n" + continued, username["id"], PLANNER_MEMORY_ITEMS), message[:20]
    for query in PLANNER_QUERIES:
        assert ranked(store, query, username["id"], PLANNER_MEMORY_ITEMS), query


def test_the_research_official_step_recalls_the_username_within_its_20(store: Any) -> None:
    """The step that typed the owner's email (12:34): its query is the request, the parent goal, its step goal and the
    task's targets (Ruling 12)."""
    username = fact(store, "usvisascheduling_username", USERNAME)
    query = "\n".join((MSG_1217, VISA_GOAL, RESEARCH_OFFICIAL, target_words(VISA_TARGETS)))
    assert ranked(store, query, username["id"], WORKER_MEMORY_ITEMS)


def test_the_planner_recalls_for_the_work_its_message_continues(daemon: Any, monkeypatch: pytest.MonkeyPatch) -> None:
    """A planner turn's memory query adds the goals and targets of the tasks its task layers show: the owner's 12:17
    message, sent while the visa task runs in the same chat, brings the username into the planner's memory."""
    for i in range(6):
        owner_fact(daemon, f"hose_{i}", f"garden hose note {i}")
    owner_fact(daemon, "usvisascheduling_username", USERNAME)
    worker = StalledWorker(monkeypatch)
    script = install(daemon, [sup("task", "Looking.", task(VISA_GOAL, apps=("Chrome New_Personal",),
                                                           sites=("usvisascheduling.com",)))])
    first: list[dict[str, Any]] = []
    thread = threading.Thread(target=lambda: first.extend(chat(daemon, "look for visa slots", session="visa")))
    thread.start()
    try:
        assert worker.started.wait(5)
        script.sup.append(sup("reply", "Here is a strategy."))
        chat(daemon, MSG_1217, session="visa", timeout=5)
        planner = [ctx for _, key, ctx, _ in script.contexts if key == "supervisor"][-1]
        assert USERNAME in planner
    finally:
        worker.release.set()
        thread.join(5)


def test_words_most_items_share_find_nothing_instead_of_the_newest_items(store: Any) -> None:
    """Review Focus: never the newest 60 or 20 items for a query of common words."""
    assert store.search("is the for and owner") == []
    assert store.search("") != []                              # no words at all: a listing, newest first


def test_json_field_names_and_scaffolding_are_not_searched(store: Any) -> None:
    fact(store, "dentist", "my dentist is Dr. Rao")
    assert store.search("unconfirmed_hint observed_at confidence") == []
    assert search_text("fact", "owner/dentist", {"content": "my dentist is Dr. Rao", "subject": "owner",
                                                 "key": "dentist", "observed_at": "x"}) == \
        "my dentist is Dr. Rao owner dentist"


def test_an_episode_shows_its_goal_outcome_and_date_and_one_without_a_goal_is_left_out(store: Any) -> None:
    episode = next(n for n in store.search("kaggle leaderboard") if n["kind"] == "episode")
    line = shown_text(episode)
    assert line.startswith("past task task") and "Check the kaggle leaderboard for the owner" in line
    assert "(2026-10-" in line and ("— done, verified" in line or line.endswith("— done"))
    store.put("episode", "nogoal", {"task_id": "nogoal", "intent": "", "narrative": "zebra crossing found",
                                    "status": "done"}, [], origin_label="ferb", made_by="code:evidence-card/4",
              state="active")
    assert [n for n in store.search("zebra crossing") if n["key"] == "nogoal"] == []


def test_the_owners_words_rank_before_a_confirmed_guess_at_equal_relevance(tmp_path: Path) -> None:
    db, learning = new_store(tmp_path / "m.db")
    for i in range(4):
        fact(learning, f"other_{i}", f"bicycle chain note {i}")
    mine = fact(learning, "editor", "the editor is Zed")
    guess = fact(learning, "editor", "the editor is Zed", owner=False)     # can't supersede the owner's version
    assert [n["id"] for n in learning.search("which editor")][:2] == [mine["id"], guess["id"]]
    db.close()


def test_a_store_indexed_by_json_is_reindexed_once_by_content(tmp_path: Path) -> None:
    db, learning = new_store(tmp_path / "m.db")
    node = fact(learning, "dentist", "my dentist is Dr. Rao")
    db.conn.execute("UPDATE learning_fts SET text = ? WHERE id = ?",
                    (node["key"] + " " + encoded(node["body"]), node["id"]))
    db.conn.execute("DELETE FROM learning_search_version")
    assert reindex_search(db) is True
    assert db.conn.execute("SELECT text FROM learning_fts WHERE id = ?", (node["id"],)).fetchone()[0] == \
        "my dentist is Dr. Rao owner dentist"
    assert db.conn.execute("SELECT version FROM learning_search_version").fetchone()[0] == SEARCH_TEXT_VERSION
    assert reindex_search(db) is False
    db.close()


def test_the_planner_gets_at_most_its_60_items(daemon: Any) -> None:
    for i in range(80):
        owner_fact(daemon, f"tomato_{i}", f"tomato tip {i}: water them in the morning")
    for i in range(100):
        owner_fact(daemon, f"bicycle_{i}", f"bicycle note {i}: oil the chain")
    found = daemon.run(daemon.daemon._learning_candidates("how should I grow tomato plants"))
    assert len(found) == PLANNER_MEMORY_ITEMS and all("tomato" in c["text"] for c in found)
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `uv run pytest -q tests/test_memory_search.py`
Expected: FAIL at import (`cannot import name 'PLANNER_MEMORY_ITEMS'`).

- [ ] **Step 4: Migration** (`ferb/store/migrations/0048_learning_search_version.sql`)

```sql
-- L36 §3: which version of the content-only search text `learning_fts` holds (`learning.SEARCH_TEXT_VERSION`). Empty
-- here, so the daemon's next start re-indexes every node by its content (`learning.reindex_search`), in one
-- transaction before anything reads or forgets.
CREATE TABLE IF NOT EXISTS learning_search_version (version TEXT NOT NULL);
```

- [ ] **Step 5: Content index, ranking and budgets (`ferb/memory/learning.py`)**

Add `import math` with the imports. After `LESSONS_LABEL` / `RUN_LABEL`, add:

```python
# The owner's budgets (2026-10-24: "Planner 60, worker 20"): the most relevant items a planner turn, and a worker or
# plan step, get after ranking. The identity layer (the owner's own preferences about themselves) is outside them.
PLANNER_MEMORY_ITEMS = 60
WORKER_MEMORY_ITEMS = 20
# What a prompt's memory never lists: identity is in every system prompt, an intent label is bookkeeping, and the
# procedures the planner may choose are listed apart (L29)
PROMPT_LEAVES_OUT = frozenset({'identity', 'episode_intent', 'procedure'})
# Ruling: recency's share of a node's score, at most RECENCY_WEIGHT (the gap between ranks 4 and 5), halving every
# RECENCY_HALF_LIFE_DAYS, so it reorders near-equal matches only
RECENCY_WEIGHT = 0.05
RECENCY_HALF_LIFE_DAYS = 30
# The version of `search_text` the index holds (migration 0048, `reindex_search`)
SEARCH_TEXT_VERSION = 'content/1'
```

After `_one_line`, add:

```python
def slug_words(slug: str) -> str:
    """A key or subject slug as the words it is made of (L36 §3): `usvisascheduling_username` -> "usvisascheduling
    username", `project:us-b1b2-visa` -> "us b1b2 visa" (the label before ':' names its kind, not the item)."""
    return ' '.join(query_terms(slug.split(':', 1)[1] if ':' in slug else slug))


def search_text(kind: str, key: str, body: dict[str, Any]) -> str:
    """What search matches a node by (L36 §3): its content (for a fact, with the words of its subject and key slugs, the
    item's own words), never JSON field names or FERB's scaffolding."""
    def words(*values: Any) -> str:
        return ' '.join(_one_line(v) for v in values if isinstance(v, str) and v.strip())
    if kind == 'fact':
        return words(body.get('content'), slug_words(str(body.get('subject') or '')),
                     slug_words(str(body.get('key') or '')))
    if kind in ('entity', 'identity', 'identity_proposal'):
        return words(key, body.get('text'))
    if kind == 'episode':
        return words(body.get('intent'), body.get('narrative'), body.get('status'))
    if kind == 'procedure':
        return words(body.get('intent'), body.get('ui_hints'))
    if kind == 'lesson':
        return words(body.get('trigger'), body.get('text'))
    if kind == 'episode_annotation':
        return words(body.get('reflection'), body.get('narrative'))
    if kind == 'activity_summary':
        return words(body.get('text'))         # a day's narrative; a day digest (code counts) has no text
    return ''                                  # rollups and intent labels: found through what they cite


def shown_text(node: dict[str, Any]) -> str:
    """A node as one line of a prompt or a memory.learned candidate (L36 §3): what it says, never a bare id."""
    kind, key, body = node['kind'], str(node['key']), node['body']
    if kind == 'fact':
        return f"{key} — {_one_line(body.get('content'))}"
    if kind == 'episode':
        when = str(body.get('ended_at') or body.get('started_at') or node['recorded_at'])[:10]
        outcome = str(body.get('status') or 'unknown') + (', verified' if body.get('verified') is True else '')
        return f"past task {body.get('task_id') or key} ({when}): {_one_line(body.get('intent'))} — {outcome}"
    if kind == 'procedure':
        return _one_line(body.get('intent') or key)
    if kind == 'lesson':
        return f"when {_one_line(body.get('trigger'))}: {_one_line(body.get('text'))}"
    if kind == 'episode_annotation':
        return f"about task {key}: {_one_line(body.get('reflection'))}"
    text = _one_line(body.get('text'))
    return f"{key}: {text}" if text else key


def _tied(rows: Iterable[Any]) -> dict[str, int]:
    """Ranks from rows (id, score) ordered best first; equal scores share a rank, so two equally good matches score
    alike and the owner's statement can come first (L36 §3)."""
    out: dict[str, int] = {}
    rank, last = -1, None
    for ident, score in rows:
        if score != last:
            rank, last = rank + 1, score
        out.setdefault(ident, rank)
    return out


def reindex_search(db: Any) -> bool:
    """Index every node by its content (`search_text`) once per SEARCH_TEXT_VERSION (L36 §3): a store indexed by an
    earlier version (JSON bodies, field names and all) is re-indexed in one transaction at the daemon's start, before
    anything reads or forgets. True when it re-indexed."""
    row = db.conn.execute('SELECT version FROM learning_search_version').fetchone()
    if row is not None and row[0] == SEARCH_TEXT_VERSION:
        return False
    nodes = db.conn.execute('SELECT id, kind, key, body_json FROM learning_nodes').fetchall()
    db.conn.execute('BEGIN')
    try:
        db.conn.execute('DELETE FROM learning_fts')
        db.conn.executemany('INSERT INTO learning_fts VALUES (?,?)',
                            [(r[0], search_text(r[1], r[2], json.loads(r[3]))) for r in nodes])
        db.conn.execute('DELETE FROM learning_search_version')
        db.conn.execute('INSERT INTO learning_search_version VALUES (?)', (SEARCH_TEXT_VERSION,))
        db.conn.execute('COMMIT')
    except BaseException:
        db.conn.execute('ROLLBACK')
        raise
    return True
```

In `Learning.put`, replace `text = key + ' ' + encoded(body)` with `text = search_text(kind, key, body)`.

Add `weighted_terms` to `Learning` and replace `search`:

```python
    def weighted_terms(self, words: list[str]) -> list[str]:
        """The query words that carry weight (L36 §3, as `Memory.recall` does): a word found in more than half of the
        indexed nodes says nothing about any one of them (data-driven, no word list); the floor of 2 keeps a shared
        topic useful in a small store."""
        total = int(self.db.conn.execute("SELECT COUNT(*) FROM learning_fts WHERE text != ''").fetchone()[0])

        def matches(term: str) -> int:
            return int(self.db.conn.execute('SELECT COUNT(*) FROM learning_fts WHERE learning_fts MATCH ?',
                                            ('"' + term.replace('"', '""') + '"',)).fetchone()[0])
        return [t for t in words if total < 2 or matches(t) <= max(2, total / 2)]

    def search(self, query: str, *, kind: str | None = None, app: str | None = None,
               person: str | None = None, since: str | None = None, until: str | None = None,
               query_vector: list[int] | None = None, embed: bool = True, limit: int | None = None,
               leave_out: frozenset[str] = frozenset()) -> list[dict[str, Any]]:
        """The one ranked search (L36 §3): bm25 over each node's content (`search_text`) for the words that carry
        weight (`weighted_terms`), the trigram title match and, when the owner adopted one, embedding similarity,
        fused by reciprocal rank (equal scores share a rank: `_tied`), plus a small recency boost; at equal score the
        owner's own statement first. A query whose every word most nodes share finds nothing; a query with no words
        lists the newest. `leave_out`: kinds dropped before `limit` (the budget) applies. An episode without a stored
        goal is left out. `query_vector`: the query's embedding, computed off the event loop (L28); `embed` False:
        keyword ranking only (the caller's encode failed and was reported)."""
        words = query_terms(query)
        terms = self.weighted_terms(words) if words else []
        rankings: list[dict[str, int]] = []
        if terms:
            match = ' OR '.join('"' + t.replace('"', '""') + '"' for t in terms)
            rankings.append(_tied(self.db.conn.execute(
                'SELECT id, bm25(learning_fts) FROM learning_fts WHERE learning_fts MATCH ? ORDER BY 2', (match,))))
            if len(query) >= 3:
                rankings.append(_tied(self.db.conn.execute(
                    'SELECT id, bm25(learning_titles) FROM learning_titles WHERE learning_titles MATCH ? ORDER BY 2',
                    ('"' + query.replace('"', '""') + '"',))))
        elif not words:
            rankings.append({p['id']: n for n, p in enumerate(reversed(self.items(kind)))})
        if self.embedder is not None and query and embed:
            from ferb.memory.embeddings import ranked
            try:
                rankings.append({ident: n for n, ident in enumerate(ranked(self.db, self.embedder, query,
                                                                             vector=query_vector))})
            except Exception as e:
                self.publish('memory.embedding_unavailable', {'error': self.redact(str(e))})
        scores: dict[str, float] = {}
        for ranking in rankings:
            for ident, n in ranking.items():
                scores[ident] = scores.get(ident, 0.0) + 1 / (n + 1)
        now = self.clock.now()
        out = []
        for ident, score in scores.items():
            p = self.get(ident)
            if not p or p['valid_to'] or p['state'] in ('retired', 'rolled_back', 'proposed') or \
                    (kind and p['kind'] != kind) or p['kind'] in leave_out:
                continue                          # 'proposed': a memory item waiting for the owner's Keep
            if p['kind'] == 'episode_intent':     # a card's intent label (L29): bookkeeping, never recalled
                continue
            body = p['body']
            if p['kind'] == 'episode' and not str(body.get('intent') or '').strip():
                continue
            if app and app not in body.get('apps', {}) and body.get('scope', {}).get('app') != app and \
                    app not in body.get('preconditions', {}).get('apps', {}):
                continue
            if person and person.casefold() not in encoded(body).casefold():
                continue
            if since and p['valid_from'] < since or until and p['valid_from'] > until:
                continue
            age = max(0.0, (now - datetime.fromisoformat(p['recorded_at'])).total_seconds() / 86400)
            p['score'] = score + RECENCY_WEIGHT * math.pow(0.5, age / RECENCY_HALF_LIFE_DAYS)
            out.append(p)
        out.sort(key=lambda p: (round(p['score'], 9), p['origin'] == origin.OWNER, p['recorded_at'], p['id']),
                 reverse=True)
        return out if limit is None else out[:limit]
```

- [ ] **Step 6: Embeddings use the same text (`ferb/memory/embeddings.py`)**

In `index` and `index_async`, replace `text = node['key'] + ' ' + encoded(node['body'])` with

```python
        text = search_text(node['kind'], node['key'], node['body'])
        if not text:
            continue                      # nothing to match (a rollup, an intent label): never embedded
```

and import `search_text` from `ferb.memory.learning`. A changed text changes its hash, so the next index re-embeds it.

- [ ] **Step 7: The daemon (`ferb/daemon/main.py`)**

Extend the learning import at the top:
`from ferb.memory.learning import PLANNER_MEMORY_ITEMS, PROMPT_LEAVES_OUT, SEARCH_TEXT_VERSION, SOURCE_REF_INDEX, reindex_search`.

In `start()`, after `self.db.conn.execute(SOURCE_REF_INDEX)`:

```python
        reindexed = reindex_search(self.db)   # L36 §3: content-only search text, once per version, before any read
```

and right after `self.journal = Journal(...)`:

```python
        if reindexed:
            self.journal.append("memory.search_reindexed", {"version": SEARCH_TEXT_VERSION})
```

Replace `_learning_candidates` with these three methods:

```python
    async def _query_vector(self, words: str) -> tuple[list[int] | None, bool]:
        """The query's embedding when the owner adopted a model, computed off the event loop (L28); (None, False)
        after a failed encode, reported once (keyword ranking only)."""
        embedder = self.learning.embedder if self.learning else None
        if embedder is None or not words:
            return None, True
        try:
            return await asyncio.to_thread(embedder.encode, words, query=True), True
        except Exception as e:                       # reported once; keyword ranking only, no second encode
            self._journal().append("memory.embedding_unavailable", {"error": self._redact_text(str(e))})
            return None, False

    async def _search(self, query: str, *, limit: int | None = None, leave_out: frozenset[str] = frozenset(),
                      **filters: Any) -> list[dict[str, Any]]:
        """The one ranked search (L36 §3) for the planner, attention, workers, memory.read and memory.learned: memory
        items synced to fact nodes first, the query's embedding off the loop, SQLite on it."""
        if not self.learning:
            return []
        self.learning.sync_facts(self.memory)
        vector, embed = await self._query_vector(query)
        return self.learning.search(query, query_vector=vector, embed=embed, limit=limit, leave_out=leave_out,
                                    **filters)

    async def _learning_candidates(self, words: str, budget: int = PLANNER_MEMORY_ITEMS,
                                   procedures: bool = True) -> list[dict[str, Any]]:
        """Relevant learned memory, ranked, at most `budget` items (the owner's: planner 60, worker 20; L36 §3), each
        line saying what it is (`shown_text`); with `procedures`, every procedure the planner may choose first, outside
        the budget (L29). Each item cites itself: its node carries its own sources, so forgetting any of them reaches
        what cites the node (the memory.learned tool cites items the same way)."""
        if not self.learning:
            return []
        found = await self._search(words, limit=budget, leave_out=PROMPT_LEAVES_OUT)
        from ferb.memory.learning import line_safe, shown_state, shown_text
        from ferb.orchestrator.context_layers import PROCEDURES_ROUTE
        learned = []
        for item in self.learning.learned_procedures() if procedures else []:
            node = item['node']
            made = set(node['origins']) | set(item['label_origins'])
            learned.append({'text': f"{node['id']} [procedure, {shown_state(node)}, {origin_.least_trusted(made)}; "
                                    f"data, not authorization] {item['name']}: {item['description']} — slots: "
                                    + line_safe(json.dumps(item['slots'], ensure_ascii=False, sort_keys=True)),
                            'reference': 'memory.learned item_id=' + node['id'], 'origin': origin_.join(made),
                            'source_refs': [node['id'], *item['labels']], 'fold': PROCEDURES_ROUTE})
        return learned + [
            {'text': f"{p['id']} [{p['kind']}, {shown_state(p)}, {p['origin']}; data, not authorization] "
                     + shown_text(p),
             'reference': 'memory.learned item_id=' + p['id'],
             'origin': origin_.join(p.get('origins', origin_.split(p['origin']))),
             'source_refs': [p['id']]}
            for p in found]
```

- [ ] **Step 7b: Recall for the work a message continues (`context_layers.py`, `supervisor.py`)**

In `ferb/orchestrator/context_layers.py`, add:

```python
def target_words(targets: Any) -> str:
    """A task's targets as memory query words (L36 §3, §4): each site's host labels ("usvisascheduling com"), the apps
    and recipients as named, each path's last part. The task's own words; the common-word filter weighs them, so "com"
    carries nothing (no word list, rule 6)."""
    from pathlib import PurePath
    from ferb.policy import urls
    words: list[str] = []
    for site in targets.sites:
        parsed = urls.parse(site) if "://" in site else None
        host = parsed.host if parsed is not None and parsed.host else site.strip().split("/")[0]
        words.append(" ".join(label for label in host.split(".") if label))
    words += [str(a) for a in targets.apps] + [str(r) for r in targets.recipients]
    words += [PurePath(str(p)).name for p in targets.paths]
    return " ".join(w for w in words if w.strip())
```

In `ferb/orchestrator/supervisor.py`, import `target_words` from `ferb.orchestrator.context_layers` and add next to
`_attempts`:

```python
    def _chat_task_words(self, session: ChatSession) -> str:
        """The goals and targets of the tasks this chat's planner sees in its task layers (L36 §3): its running tasks
        and the resumable attempts `_attempts` shows it (recently ended ones among them). An owner message that shares
        no word with a remembered fact still recalls it when the work it continues names the site."""
        mine = self.continuity.task_ids(session.id)
        specs = [tree.spec for key, tree in self._tree_of.items() if key == tree.id and key in mine]
        if not session.id.startswith(SCHEDULE_SESSION):         # a scheduled run's planner is shown no attempts
            for task_id in self._attempts(self.continuity.candidates(session.id))[1]:
                saved = None if task_id in self._tree_of else self.continuity.load(task_id, session.id)
                if saved is not None:
                    specs.append(saved[0])
        return "\n".join(part for spec in specs for part in (spec.goal, target_words(spec.targets)) if part.strip())
```

In `_supervise`, replace `situation.extend(await self._memory_blocks(message, remembered, mem_text, procedures))` with:

```python
        recall_for = "\n".join(part for part in (message, self._chat_task_words(session)) if part.strip())
        situation.extend(await self._memory_blocks(recall_for, remembered, mem_text, procedures))
```

- [ ] **Step 8: Run the focused tests**

Run: `uv run pytest -q tests/test_memory_search.py`
Expected: PASS.

- [ ] **Step 9: Fix only the fallout this change explains**

Run: `uv run pytest -q tests/test_planner_context.py tests/test_f6_context.py tests/test_memory.py tests/test_learning.py tests/test_learning_alive.py tests/test_learning_after_forget.py tests/test_learning_by_intent_e2e.py tests/test_memory_embeddings.py tests/test_consolidation_cost.py tests/test_intent_labels.py tests/test_forget_complete.py tests/test_attention_understanding.py tests/test_context_exposure.py tests/test_orchestrated_control.py`
Expected: a failure here may only be one of these, each fixed as said: a test whose planner now also recalls through an
earlier task of its chat (expect the extra items, or give the test's earlier task no shared words); a query whose words appear only in a node's JSON
field names (change the query to the node's content words); an assertion on an episode line equal to its bare key
(assert the goal and outcome instead); an embedding test that pinned the old `key + JSON` text (pin `search_text`).
Never weaken an assertion about origins, provenance, proposed items or forgetting. List each changed test in the
commit body.

- [ ] **Step 10: Docs**

`ARCHITECTURE.md` §8: replace "`query_terms` (`ferb/memory/items.py`) tokenizes a query and `Memory.recall` drops terms
that match most memories;" with "`query_terms` (`ferb/memory/items.py`) tokenizes a query, and the one ranked search
(`Learning.search`, L36) drops terms that match most indexed nodes;".

`ARCHITECTURE.md` §16.12 bullet: "- **One ranked search (§3).** `Learning.search` serves the planner turn, attention,
worker and plan-step pushes, `memory.read` and `memory.learned` (the daemon's `_search`: facts synced, the query's
embedding off the loop). `learning_fts` holds each node's content only (`search_text`; a fact with the words of its
subject and key slugs, `slug_words`; re-indexed once per `SEARCH_TEXT_VERSION` at start, journal
`memory.search_reindexed`). A planner turn recalls for the owner's message plus the goals and targets
(`target_words`: sites' host labels, apps, recipients, paths' last parts) of the tasks its task layers show
(`Supervisor._chat_task_words`: this chat's running tasks and the resumable attempts it is shown), so a message that
shares no word with a fact still recalls it through the work it continues. A word found in more than half of the nodes carries
no weight (`weighted_terms`); bm25 ranks with ties sharing a rank, the trigram title match and an adopted embedding
are fused by reciprocal rank, plus at most 0.05 for recency (half-life 30 days); at equal score an owner-stated node
comes first. Budgets after ranking: `PLANNER_MEMORY_ITEMS` 60 and `WORKER_MEMORY_ITEMS` 20 (owner, 2026-10-24); the
identity layer and the planner's procedure list are outside them. The memory blocks stay protected inside the budgets
(the planner's conversation fills its window before optional blocks, and it has no tool to fetch one). Episodes show
`past task <id> (<date>): <goal> — <status>[, verified]` (`shown_text`); one without a goal is left out."

`README.md` "Memory and learning", after the `ferb memory search` sentence: "Search ranks by what items say (never JSON
field names), skips words most memories share, and favors what you said yourself; each planner turn gets the 60 most
relevant memories and each worker or plan step the 20 most relevant."

`CHANGELOG.md` bullet: "- Memory is ranked and bounded (L36). Live 2026-10-24: every planner turn got about 207 memory
lines (43 KB), 102 of them bare task ids, all kept whatever their relevance, because words like "is", "the" and "for"
matched the JSON of every item. Search now reads only what items say, ignores words most items share, ranks by
relevance with a small boost for recent items and your own statements first, and gives each planner turn the 60 most
relevant (owner: "Planner 60, worker 20"). A message is also matched through the task it continues: at 12:17 your
"hmm idk if thats s agood strategy…" shared no word with the saved username, but the running visa task named
usvisascheduling.com, and now that brings the username in. Past tasks show their goal, outcome and date. The index is
rebuilt once at the next start."

Add `"tests/test_memory_search.py"` to `[L36].tests` in `docs/0.27_DECISIONS.toml`.

- [ ] **Step 11: Full checks, commit and push**

Run: `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`.

```bash
git add ferb/store/migrations/0048_learning_search_version.sql ferb/memory/learning.py ferb/memory/embeddings.py \
  ferb/daemon/main.py ferb/orchestrator/context_layers.py ferb/orchestrator/supervisor.py \
  tests/support/memory_corpus.py tests/support/live_steer.py tests/test_control_turn_entries.py \
  tests/test_memory_search.py docs/0.27_DECISIONS.toml \
  ARCHITECTURE.md README.md CHANGELOG.md   # plus any fallout test files from Step 9
git commit -m "feat(L36): one ranked content search with the owner's budgets, planner 60

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 4: `memory.read` and `memory.learned` through the one search (spec §3, "The tools")

**Files:**
- Modify: `ferb/orchestrator/evidence.py` (`MemoryRead`)
- Modify: `ferb/memory/tools.py` (`LearningRead`)
- Modify: `ferb/registry.py` (`add_context_readers` takes `search`)
- Modify: `ferb/daemon/main.py` (`_memory_item_search`; wiring of both tools)
- Test: `tests/test_memory_search.py` (append), `tests/test_memory.py`, `tests/test_tool_origins.py`
- Docs: `ARCHITECTURE.md` §16.12, `README.md`, `CHANGELOG.md`

**Interfaces:**
- Consumes: `Daemon._search(query, **filters)` (Task 3), `shown_text` (Task 3).
- Produces: `MemoryRead(memory, source_origins=None, search: Callable[[str], Awaitable[list[str]]] | None = None)`;
  `LearningRead(store, search: Callable[..., Awaitable[list[dict]]] | None = None)`;
  `add_context_readers(r, continuity, root, memory, source_origins=None, search=None)`;
  `Daemon._memory_item_search(query: str) -> list[str]` (async; memory item ids, best first).

- [ ] **Step 1: Write the failing tests** (append to `tests/test_memory_search.py`)

```python
def test_memory_read_ranks_a_several_word_query(daemon: Any) -> None:
    """Live 2026-10-24: 14 of 20 memory.read queries came back empty; 9 were phrases like "RSNA knee Kaggle rank
    score", which a literal substring match can never find."""
    from ferb.orchestrator.evidence import MemoryRead
    for i in range(4):
        owner_fact(daemon, f"chain_{i}", f"bicycle chain note {i}")
    rsna = owner_fact(daemon, "rsna", "I track my RSNA knee competition rank on Kaggle")
    reader = MemoryRead(lambda: daemon.daemon.memory, search=daemon.daemon._memory_item_search)
    out = daemon.run(reader.run(MemoryRead.Input(query="RSNA knee Kaggle rank score"), None))
    assert [i["id"] for i in out.content["items"]][:1] == [rsna.id]


def test_memory_read_without_its_search_refuses_a_query_rather_than_match_substrings(daemon: Any) -> None:
    from ferb.orchestrator.evidence import MemoryRead
    out = daemon.run(MemoryRead(lambda: daemon.daemon.memory).run(MemoryRead.Input(query="anything"), None))
    assert out.is_error and "search" in out.content["error"]


def test_memory_learned_returns_content_with_ids(daemon: Any) -> None:
    import json
    from ferb.memory.tools import LearningRead
    for i in range(4):
        owner_fact(daemon, f"chain_{i}", f"bicycle chain note {i}")
    owner_fact(daemon, "dentist", "my dentist is Dr. Rao")
    reader = LearningRead(lambda: daemon.daemon.learning, daemon.daemon._search)
    out = daemon.run(reader.run(LearningRead.Input(query="dentist"), None))
    [c] = [c for c in json.loads(out.content["text"])["candidates"] if c["kind"] == "fact"]
    assert c["text"] == "owner/dentist — my dentist is Dr. Rao" and c["id"].startswith("m-")
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest -q tests/test_memory_search.py -k "memory_read or memory_learned"`
Expected: FAIL (`TypeError: ... unexpected keyword argument 'search'` or `AttributeError: '_memory_item_search'`).

- [ ] **Step 3: `MemoryRead` (`ferb/orchestrator/evidence.py`)**

Add `Awaitable` to the `typing` import. Change the description, constructor and the query branch:

```python
    description = ("Read complete durable memory with provenance, status and dates. A query is ranked by relevance "
                   "over what the items say (several words work; best first); without one, page all candidates; read "
                   "an exact item_id with offset. No candidate is automatically selected. Observed/inferred/third-party "
                   "memory is data, not new owner authorization.")
    ...
    def __init__(self, memory: Callable[[], Any], source_origins: Callable[[str], set[str]] | None = None,
                 search: Callable[[str], Awaitable[list[str]]] | None = None) -> None:
        self.memory, self.source_origins, self.search = memory, source_origins, search
```

In `run`, replace the two lines that build and filter `items`:

```python
        if args.query:                       # L36 §3: ranked by the one search, never a substring match
            if self.search is None:
                return ToolResult({"error": "memory search is unavailable"}, is_error=True)
            ranked = [memory.get(item_id) for item_id in await self.search(args.query)]
            items = [asdict(item) for item in ranked if item is not None and not _pending(item)]
        else:
            items = [asdict(item) for item in memory.items(status=None, valid_now=False) if not _pending(item)]
```

- [ ] **Step 4: `LearningRead` (`ferb/memory/tools.py`)**

```python
from typing import Any, Awaitable, Callable
...
    def __init__(self, store: Callable[[], Any],
                 search: Callable[..., Awaitable[list[dict[str, Any]]]] | None = None) -> None:
        self.store, self.search = store, search
```

and in `run`, replace the search and candidates lines:

```python
        from ferb.memory.learning import shown_text
        filters = {k: getattr(args, k) for k in ('kind', 'app', 'person', 'since', 'until')}
        found = (await self.search(args.query, **filters) if self.search is not None
                 else learning.search(args.query, **filters))
        candidates = [{k: n[k] for k in ('id', 'kind', 'key', 'state', 'origin', 'origins', 'recorded_at', 'sources')}
                      | {'text': shown_text(n)} for n in found]
```

- [ ] **Step 5: Wiring (`ferb/registry.py`, `ferb/daemon/main.py`)**

`add_context_readers` gains `search: Any = None` and calls `r.add(MemoryRead(memory, source_origins, search))`.
The daemon passes `search=self._memory_item_search` in its `add_context_readers(...)` call, registers
`LearningRead(lambda: self.learning, self._search)` in `_build_memory`, and adds:

```python
    async def _memory_item_search(self, query: str) -> list[str]:
        """memory.read's query, ranked by the one search (L36 §3): the memory items behind the matching facts."""
        return [str(n["body"]["id"]) for n in await self._search(query, kind="fact") if n["body"].get("id")]
```

- [ ] **Step 6: Update the two tests that build a `MemoryRead` without a search**

- `tests/test_memory.py::test_memory_read_neither_lists_nor_reads_a_pending_inferred_item`:
  `reader = MemoryRead(lambda: daemon.daemon.memory, search=daemon.daemon._memory_item_search)`.
- `tests/test_tool_origins.py` (the `MemoryRead(lambda: memory)` case): add
  `async def no_hits(query: str) -> list[str]: return []` and build `MemoryRead(lambda: memory, search=no_hits)`; the
  `query="no such text"` assertion stays `{"ferb"}`.

- [ ] **Step 7: Run the focused tests**

Run: `uv run pytest -q tests/test_memory_search.py tests/test_memory.py tests/test_tool_origins.py tests/test_forget_complete.py`
Expected: PASS.

- [ ] **Step 8: Docs**

`ARCHITECTURE.md` §16.12 bullet: "- **The tools (§3).** `memory.read` keeps its arguments; a query is ranked by the one
search (`Daemon._memory_item_search`: the memory items behind the matching facts, best first), never a substring match.
`memory.learned` candidates carry `text` (`shown_text`) beside their ids."

`README.md`: change "`memory.learned` gives workers the same read-only records" to "`memory.learned` gives workers the
same read-only records, each candidate with what it says"; add "A worker's `memory.read` query is ranked the same way."

`CHANGELOG.md` bullet: "- A worker's memory lookups find what they look for (L36). Live: 14 of 20 `memory.read`
queries came back empty, because the query had to appear letter for letter in an item ("RSNA knee Kaggle rank score"
never could). Both memory tools now use the same ranked search as the planner, and `memory.learned` returns what each
item says, not only its key."

- [ ] **Step 9: Full checks, commit and push**

Run: `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`.

```bash
git add ferb/orchestrator/evidence.py ferb/memory/tools.py ferb/registry.py ferb/daemon/main.py \
  tests/test_memory_search.py tests/test_memory.py tests/test_tool_origins.py ARCHITECTURE.md README.md CHANGELOG.md
git commit -m "feat(L36): memory.read and memory.learned rank through the one search and return content

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 5: Every worker and plan step gets memory and its parent goal (spec §4)

**Files:**
- Modify: `ferb/orchestrator/supervisor.py` (`Supervisor.memory_push`, `_Tree.parent_goal`, `_worker_memory`,
  `_context`, the step members in `_execute_plan`, `_conversation_steps` and `_loop_step`)
- Modify: `ferb/daemon/main.py` (wire `memory_push`; delete the `recall` stub)
- Create: `tests/test_worker_memory.py`
- Modify: `tests/test_progress_line.py` (`test_recall_is_read_the_same_way_by_the_planner_and_the_worker`)
- Docs: `ARCHITECTURE.md` §16.12, `README.md`, `CHANGELOG.md`; add `"tests/test_worker_memory.py"` to `[L36].tests`

**Interfaces:**
- Consumes: `Daemon._learning_candidates(words, budget, procedures)` (Task 3), `WORKER_MEMORY_ITEMS`,
  `context_layers.ranked_memory_blocks`, `context_layers.target_words` (Task 3; already imported in supervisor.py).
- Produces: `Supervisor.memory_push: Callable[[str], Any] | None` (returns ranked candidates or an awaitable);
  `_Tree.parent_goal: str = ''`; `Supervisor._worker_memory(tree, request) -> list[ContextBlock]` (async).

Ruling (hook): the push uses a new async hook, not the synchronous `recall`, so the query's embedding stays off the
event loop (L28); the daemon's `recall` stub is deleted and the supervisor's default `recall` (returns "") stays only for
a supervisor without the daemon's search.
Ruling (query): the owner's request, a step's parent goal, the task or step goal, and the task's targets
(`target_words`: sites' host labels, apps, recipients, paths' last parts), so a step whose goals don't name the site
still recalls what was said about it.
Ruling (steps): a plan step's activity layer stays empty ("nothing else about step scoping changes"); do/reduce,
converse and loop members all get `parent_goal`.

- [ ] **Step 1: Write the failing tests** (`tests/test_worker_memory.py`)

```python
"""L36 §4: every worker and plan step gets the 20 most relevant memories for the owner's request, the task goal, (for
a step) its parent's goal and the task's targets, and a step sees its parent task's goal above its own.

Live 2026-10-24: the visa task's research step got no memory (the daemon's recall was a stub and plan steps blanked
it) and its step goal replaced the task goal; it typed the owner's email as the username."""
from typing import Any

from ferb.memory.learning import WORKER_MEMORY_ITEMS
from ferb.orchestrator.plans import Plan, Step
from tests.conftest import chat, done, install, outcome, sup, task
from tests.support.memory_items import owner_fact, run


def filler(d: Any, n: int = 6) -> None:
    for i in range(n):
        owner_fact(d, f"hose_{i}", f"garden hose note {i}")


def test_a_plan_step_gets_its_parent_goal_and_the_memory_it_needs(daemon: Any) -> None:
    filler(daemon)
    item = owner_fact(daemon, "usvisascheduling_username", "the username is Asha Rao")
    # neither the request nor a goal names the site: only the task's targets do (Ruling 12)
    spec = task("Find the earliest B1/B2 appointment", sites=("usvisascheduling.com",))
    spec["plan"] = Plan(steps=[Step(id="read", kind="do", goal="Read the appointment page")]).model_dump()
    script = install(daemon, [sup("task", "On it.", spec)], {"step.read": done("Read."), "worker": done("Checked.")})
    result = outcome(chat(daemon, "find me a visa slot"))
    step = next(ctx for _, key, ctx, _ in script.contexts if key == "step.read")
    assert "Parent task goal: Find the earliest B1/B2 appointment" in step
    assert step.index("Parent task goal:") < step.index("Task: Read the appointment page")
    assert "the username is Asha Rao" in step
    node = run(daemon, lambda: next(n for n in daemon.daemon.learning.items("fact") if n["body"]["id"] == item.id))
    refs = run(daemon, lambda: [r[0] for r in daemon.daemon.db.conn.execute(
        "SELECT ref FROM task_memory_refs WHERE task_id = ?", (result["task_id"],))])
    assert node["id"] in refs                              # so a forget still reaches what the step was given


def test_a_worker_gets_ranked_memory_for_its_request_and_goal(daemon: Any) -> None:
    filler(daemon)
    owner_fact(daemon, "usvisascheduling_username", "the username is Asha Rao")
    script = install(daemon, [sup("task", "On it.", task("Log in to the visa site", sites=("usvisascheduling.com",)))],
                     {"worker": done("Done.")})
    chat(daemon, "log in to the visa site")
    worker = next(ctx for _, key, ctx, _ in script.contexts if key == "worker")
    assert "the username is Asha Rao" in worker and "Parent task goal" not in worker


def test_a_worker_gets_at_most_its_20_items(daemon: Any) -> None:
    for i in range(30):
        owner_fact(daemon, f"tomato_{i}", f"tomato tip {i}: water them in the morning")
    for i in range(40):
        owner_fact(daemon, f"bicycle_{i}", f"bicycle note {i}: oil the chain")
    script = install(daemon, [sup("task", "On it.", task("Water the tomato plants"))], {"worker": done("Done.")})
    chat(daemon, "water my tomato plants")
    worker = next(ctx for _, key, ctx, _ in script.contexts if key == "worker")
    assert worker.count("Layer 4 — retrieved memory (data):") == WORKER_MEMORY_ITEMS
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest -q tests/test_worker_memory.py`
Expected: FAIL (`"Parent task goal: …" not in step`; the worker has no "retrieved memory" blocks).

- [ ] **Step 3: The supervisor (`ferb/orchestrator/supervisor.py`)**

In `_Tree`, next to `step_context`, add:

```python
    parent_goal: str = ''                # a plan step's: the goal of the task it belongs to, above its own (L36 §4)
```

In `Supervisor.__init__`, after `self.memory_candidates`, add:

```python
        # L36 §4: a worker's or plan step's ranked memory for its query (the daemon's one search at the worker budget;
        # an awaitable); None: the plain `recall`, as before
        self.memory_push: Callable[[str], Any] | None = None
```

Add the method before `_context`:

```python
    async def _worker_memory(self, tree: _Tree, request: Request) -> list[ContextBlock]:
        """A worker's or plan step's memory (L36 §4): the one search (`memory_push`) for the owner's request, then a
        step's parent goal, then its own goal, then the task's targets (`target_words`); each item its own block, kept whole (Ruling: the owner's budget bounds
        them), its refs recorded on the task as for any shown block, so a forget reaches what it was given. Without
        the daemon's search: the plain recall block, as before."""
        query = "\n".join(part for part in (request.words, tree.parent_goal, tree.spec.goal,
                                            target_words(tree.spec.targets)) if part.strip())
        if self.memory_push is None:
            recalled = self.recall(query)
            return [dataclasses.replace(self._source_block(tree, "Layer 4 — recalled memory",
                                                           self.recall_items(recalled)[0], self.recall_origin(recalled)),
                                        priority=2)]
        from ferb.orchestrator.context_layers import ranked_memory_blocks
        found = await _value(self.memory_push(query))
        return [dataclasses.replace(block, protected=True) for block, _ in ranked_memory_blocks(found)]
```

In `_context`, replace the activity/recall lines and the first five parts:

```python
        activity_source = await asyncio.to_thread(self.relevant_context, words)   # one search, off the loop (L28)
        activity = _labeled(activity_source)
        activity_refs = _context_refs(activity_source)
        if tree.step_context:             # a step gets its own step and handed evidence; its activity is pullable
            activity = ('', origins_.FERB)
        memory = await self._worker_memory(tree, request)
        parts = [ContextBlock("Owner's complete original request (pasted sources remain marked as data):\n" +
                              (request.payload or words), "task.evidence kind=task", protected=True, cache=True),
                 ContextBlock((f"Parent task goal: {tree.parent_goal}\n" if tree.parent_goal else "") +
                              f"Task: {spec.goal}\nWorker type: {spec.worker_type}", "task specification",
                              protected=True, cache=True),
                 dataclasses.replace(self._source_block(tree, "Layer 3 — relevant activity and actions", *activity,
                                                               source_refs=tuple(activity_refs if activity_refs is not None
                                                                                 else self.relevant_source_refs(words)) if activity[0] else (),
                                                               payload=_context_payload(activity_source) if activity[0] else ""), priority=1),
                 *memory,
                 ContextBlock(self.disturbance_context(tree.id), "task.evidence query=disturbance", protected=True)]
```

Give each step member its parent goal: in `_execute_plan`'s `execute` (`member = _Tree(tree.id, child_spec, child, …)`),
in `_conversation_steps`' `turn` (`member = _Tree(tree.id, child_spec, child, …)`) and in `_loop_step`'s `play`
(`member = _Tree(tree.id, spec, child, …)`), add the keyword argument `parent_goal=tree.spec.goal`.

- [ ] **Step 4: The daemon (`ferb/daemon/main.py`)**

Add `WORKER_MEMORY_ITEMS` to the learning import. In `_build_memory`, replace
`self.supervisor.recall = lambda words: ('', {}, [])  # F8 ranked candidates include current facts` with:

```python
            # L36 §4: every worker and plan step gets the one search at the owner's worker budget (no procedures:
            # a worker's procedure is chosen by the planner)
            self.supervisor.memory_push = lambda words: self._learning_candidates(words, WORKER_MEMORY_ITEMS,
                                                                                  procedures=False)
```

- [ ] **Step 5: Keep the legacy-recall test about the legacy path**

In `tests/test_progress_line.py::test_recall_is_read_the_same_way_by_the_planner_and_the_worker`, add as its first
line `daemon.daemon.supervisor.memory_push = None   # the plain recall path (no daemon search), still read the same way`.

- [ ] **Step 6: Run the focused tests**

Run: `uv run pytest -q tests/test_worker_memory.py tests/test_progress_line.py tests/test_task_plans.py tests/test_orchestrated_control.py tests/test_context_exposure.py tests/test_loops.py tests/test_forget_complete.py`
Expected: PASS.

- [ ] **Step 7: Docs**

`ARCHITECTURE.md` §16.12 bullet: "- **Workers and plan steps (§4).** `Supervisor._worker_memory` asks the daemon's
`memory_push` (the one search, `WORKER_MEMORY_ITEMS` 20, no procedures) for the owner's request words, a plan step's
`parent_goal`, its own goal and the task's `target_words`, and adds each item as a protected "Layer 4 — retrieved memory" block, recorded in
`task_memory_refs` like any shown block. Plan steps (do/reduce, converse, loop rounds) no longer blank memory, and
their "Task:" block opens with "Parent task goal: …"; their activity layer stays empty. The daemon's `recall` stub is
gone."

`README.md` "Memory and learning": "Each task worker and each step of a plan gets the 20 memories most relevant to your
request and its goal; a plan step also sees the whole task's goal."

`CHANGELOG.md` bullet: "- Workers and plan steps get memory (L36). Live 2026-10-24: the visa task's research step got
no memory and only its own step goal, so it typed your email as the username even though you had given the username
that morning. Every worker and plan step now gets the 20 memories most relevant to your request and the task, and a
plan step sees its parent task's goal above its own."

Add `"tests/test_worker_memory.py"` to `[L36].tests`.

- [ ] **Step 8: Full checks, commit and push**

Run: `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`.

```bash
git add ferb/orchestrator/supervisor.py ferb/daemon/main.py tests/test_worker_memory.py tests/test_progress_line.py \
  docs/0.27_DECISIONS.toml ARCHITECTURE.md README.md CHANGELOG.md
git commit -m "feat(L36): every worker and plan step gets ranked memory and its parent task's goal

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 6: One login record per site, `secrets.list`, migration (spec §5)

**Files:**
- Modify: `ferb/identity/owner_secrets.py` (records, migration, `login_place`, listing, usernames)
- Create: `ferb/identity/secret_tools.py` (`SecretsList`)
- Modify: `ferb/daemon/main.py` (register `SecretsList`)
- Modify: `ferb/daemon/gateway.py` (`Gateway.ask_secret`, `secret_summary`, `secret_question`)
- Modify: `ferb/orchestrator/prompts/worker.md`
- Test: `tests/test_owner_secrets.py`, `tests/test_question_clarity.py`, `tests/test_prompt_contract.py`
- Docs: `ARCHITECTURE.md` (§7's `owner-secret-handles.json` sentence; §16.12), `README.md`, `CHANGELOG.md`; add
  `"tests/test_owner_secrets.py"` to `[L36].tests`

**Interfaces:**
- Consumes: `refuse_whole_browser`, `normalize_origin` (`ferb/identity/secrets.py`).
- Produces: `owner_secrets.VERSION = 2`, `owner_secrets.WHOLE_BROWSER: str`, `login_place(target: str) -> str`;
  `OwnerSecrets.records() -> dict[str, dict]`, `.unusable() -> list[dict]`, `.reveal(name, target) -> str`,
  `.remember(name, target, value) -> None`, `.set_username(target, username, source) -> None`,
  `.clear_usernames(sources: set[str]) -> list[str]`, `.username(target) -> str | None`,
  `.listing(target) -> dict` (keys `place`, `username`, `entries`, `unusable`);
  `SecretsList(home: Callable[[], Path])` (tool `secrets.list`, input `place`);
  `secret_summary(target, remember, app=None, username=None)`, `secret_question(target, remember, username=None)`.

Ruling (accounts): a new entry's Keychain account is `"<place> · <name>"` (unique per place and name, readable in
Keychain Access); a migrated entry keeps its old account, so its value is never re-entered.
Ruling (unusable): entries saved for a whole browser are listed in every `secrets.list` answer with "ask again", since
they belong to no origin.
Ruling (question): the hidden-input line names "<username> at <site>".
Ruling (independent of ranking): on a login page the username comes from `secrets.list(origin)`, read from the record,
never from the memory budget; worker.md tells the worker to call it before typing into a login form.

- [ ] **Step 1: Write the failing tests** (append to `tests/test_owner_secrets.py`; add `import json`,
  `from typing import Any`, `from ferb.core.types import Identity` and
  `from ferb.identity.owner_secrets import WHOLE_BROWSER, login_place` at the top)

```python
VISA = 'https://www.usvisascheduling.com'


def fake_keychain() -> tuple[dict[str, str], Any]:
    values: dict[str, str] = {}

    def keychain(account: str, value: str | None = None) -> str | None:
        if value is not None:
            values[account] = value
        return values.get(account)
    return values, keychain


def test_a_login_place_is_an_https_origin_or_an_app() -> None:
    assert login_place(VISA + '/en-US/niv/login?x=1') == VISA
    assert login_place('app:com.apple.Notes') == 'app:com.apple.Notes'
    assert login_place('http://www.usvisascheduling.com') == '' and login_place('usvisascheduling.com') == ''


def test_saving_a_name_again_for_a_place_replaces_it(tmp_path: Path) -> None:
    values, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    store.remember('password', VISA, 'first-value')
    store.remember('password', VISA, 'second-value')
    assert store.records()[VISA]['entries'] == {'password': f'{VISA} · password'}
    assert store.reveal('password', VISA) == 'second-value' and len(values) == 1


def test_the_listing_names_the_username_and_entries_never_a_value_or_account(tmp_path: Path) -> None:
    _, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    store.remember('password', VISA, 'first-value')
    store.remember('What is your favorite food?', VISA, 'second-value')
    store.set_username(VISA, 'Asha Rao', 'memory:item-1')
    listing = store.listing(VISA)
    assert listing == {'place': VISA, 'username': 'Asha Rao', 'entries': ['What is your favorite food?', 'password'],
                       'unusable': []}
    text = json.dumps(listing)
    assert 'first-value' not in text and 'second-value' not in text and ' · ' not in text


def test_an_old_handle_file_is_read_by_place_and_a_browser_bound_one_is_unusable(tmp_path: Path) -> None:
    """Review Focus: the live store's flat handle -> place map keeps working; the app-bound visa password, which
    `refuse_whole_browser` refuses, is listed as unusable ("ask again")."""
    values, keychain = fake_keychain()
    values.update({'US visa password (Chrome)': 'old-chrome', 'US visa password': 'old-web', 'notes': 'old-notes'})
    store = OwnerSecrets(tmp_path, keychain)
    old = {'US visa password (Chrome)': 'app:com.google.Chrome', 'US visa password': VISA,
           'notes': 'app:com.apple.Notes'}
    store.path.write_text(json.dumps(old))
    empty = {'username': None, 'username_source': None}
    assert store.records() == {VISA: empty | {'entries': {'US visa password': 'US visa password'}},
                               'app:com.apple.Notes': empty | {'entries': {'notes': 'notes'}}}
    assert store.listing(VISA)['unusable'] == [{'name': 'US visa password (Chrome)', 'why': WHOLE_BROWSER}]
    assert store.reveal('US visa password', VISA) == 'old-web'
    assert json.loads(store.path.read_text()) == old                 # a read never rewrites the file
    store.remember('password', VISA, 'new-web')
    saved = json.loads(store.path.read_text())
    assert saved['version'] == 2 and saved['unusable'][0]['place'] == 'app:com.google.Chrome'
    assert store.reveal('US visa password', VISA) == 'old-web'       # its Keychain item is unchanged


def test_an_unreadable_handle_file_is_never_overwritten(tmp_path: Path) -> None:
    _, keychain = fake_keychain()
    store = OwnerSecrets(tmp_path, keychain)
    store.path.write_text('{not json')
    for attempt in (lambda: store.reveal('password', VISA), lambda: store.remember('password', VISA, 'x'),
                    lambda: store.listing(VISA)):
        with pytest.raises(Refused, match='owner-secret-handles.json'):
            attempt()
    assert store.path.read_text() == '{not json'


def test_secrets_list_is_an_owner_observation_that_normalizes_the_place(daemon: Any) -> None:
    from tests.mackit import gateway_call as call, grant_task as grant
    from tests.support.memory_items import run
    _, keychain = fake_keychain()
    run(daemon, lambda: OwnerSecrets(daemon.daemon.home, keychain).remember('password', VISA, 'v'))
    owner = grant(daemon, 'q', Identity.OWNER, {'task:read'}, None)
    r = call(daemon, owner, 'secrets.list', place=VISA + '/en-US/niv/login?x=1')
    assert not r['is_error'] and r['content']['place'] == VISA and r['content']['entries'] == ['password']
    assert call(daemon, owner, 'secrets.list', place='http://www.usvisascheduling.com')['is_error']
    ferb = grant(daemon, 'f', Identity.FERB, {'task:read'}, None)
    assert 'secrets.list' not in {t['name'] for t in daemon.daemon.gateway.tools_for(ferb)}
```

Update the two existing tests: in `test_concurrent_prompts_keep_both_handle_bindings` the last assertion becomes
`assert OwnerSecrets(tmp_path, keychain).records()['https://accounts.example']['entries'] == {'0': 'https://accounts.example · 0', '1': 'https://accounts.example · 1'}`;
in `test_a_handle_bound_to_a_whole_browser_is_never_revealed_or_saved`, replace `'other' not in store.handles()` with
`'app:' + bundle not in store.records()`.

Append to `tests/test_question_clarity.py`:

```python
def test_the_secret_question_names_the_username_saved_for_the_site(daemon: Any) -> None:
    """L36 §5: "Password for Kartikey Attri at usvisascheduling.com" (spec), so the owner knows which account."""
    from ferb.identity.owner_secrets import OwnerSecrets
    daemon.call_soon(lambda: OwnerSecrets(daemon.daemon.home, lambda *a: None).set_username(VISA, 'Asha Rao', 'memory:x'))
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(_ctx(), VISA,
                                                                                remember='US visa scheduling login'),
                                               daemon.loop)
    ap = _asked(daemon, 'owner_secret')
    try:
        assert 'Asha Rao at atlasauth.b2clogin.com' in ap.summary and 'Asha Rao' in ap.reason
    finally:
        _cancel(daemon, ap, waiting)
```

Add to `SENTENCES` in `tests/test_prompt_contract.py`:

```python
    # L36 §5: a resumed task re-asked for two secrets saved that morning; nothing listed them by site
    ("worker.md",
     'Before typing into a login form, call secrets_list for that site.'),
    ("worker.md",
     'It gives the username saved for that site and the names of its saved secrets, never their values, whatever your '
     'memory shows. Type a listed username like any text; fill a listed secret with browser_secret (or mac_secret) '
     'secret=<its name>; ask the owner, through the hidden input, only for what is not listed, and save it with '
     'remember=<a clear name>: "password", or the security question\'s own text.'),
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest -q tests/test_owner_secrets.py tests/test_question_clarity.py tests/test_prompt_contract.py`
Expected: FAIL (`cannot import name 'WHOLE_BROWSER'`; the worker.md sentence missing).

- [ ] **Step 3: Rewrite `ferb/identity/owner_secrets.py`**

```python
"""Explicit owner-requested secrets. Values stay in the login Keychain; metadata binds each to its exact place.

One login record per place (L36 §5): an HTTPS origin, or `app:<bundle id>` for a native app. A record holds the
username (plain text: owner decision 2026-10-24, "Yes, plain"; set only from the owner's own words, `set_username`, and
cleared when the memory it came from is forgotten) and named sealed entries (the password, each security question's
answer under that question's text), each a Keychain item under service `ferb.owner-secrets`. Saving a name that exists
for that place replaces its value, so no duplicates. The place is always code-derived (D12); the worker chooses only
the entry's name.

`owner-secret-handles.json` (version 2): {"version": 2, "records": {place: {"username", "username_source", "entries":
{name: Keychain account}}}, "unusable": [{"name", "place", "why"}]}. A version-1 file (handle -> place) is read as
version 2 and never rewritten by a read: each handle becomes an entry of its place under its own name (its Keychain
account unchanged), and one bound to a whole web browser is listed unusable. An unreadable file is never overwritten.

The signed Security.framework helper keeps values out of argv, environment and metadata; private pipes carry values.
"""
import json
import os
import threading
from pathlib import Path
from typing import Any

from ferb.core.errors import Refused

SERVICE = 'ferb.owner-secrets'
VERSION = 2
WHOLE_BROWSER = 'it was saved for a whole web browser, which FERB no longer allows; ask the owner again'
_write_lock = threading.RLock()


def _keychain(handle: str, value: str | None = None, home: Path | None = None) -> str | None:
    from ferb.identity.helper import request
    out = request({'operation': 'store' if value is not None else 'lookup', 'service': SERVICE,
                   'account': handle, **({'value': value} if value is not None else {})}, home)
    return out.get('value')


def refuse_whole_browser(target: str) -> None:
    """A secret is bound to the exact place it is filled: a web page's HTTPS origin, or a native app. A web browser
    as an app (`app:com.google.Chrome`) is every site it shows, so no handle is bound to one (owner 2026-10-24: "at
    places only ive approved")."""
    from ferb.act.browser.profiles import is_web_browser
    if target.startswith('app:') and is_web_browser(target[4:]):
        raise Refused('a secret is never bound to a whole web browser; a web page\'s secret is bound to its HTTPS '
                      'address through the browser connection')


def login_place(target: str) -> str:
    """The place a login record is kept for: `app:<bundle id>` as given, else the address's HTTPS origin
    (https://host[:port]); "" for anything else."""
    from ferb.identity.secrets import normalize_origin
    target = target.strip()
    if target.startswith('app:'):
        return target if len(target) > 4 else ''
    origin = normalize_origin(target)
    return origin if origin.startswith('https://') and origin != 'https://' else ''


def _empty() -> dict[str, Any]:
    return {'username': None, 'username_source': None, 'entries': {}}


def _migrated(old: dict[str, str]) -> dict[str, Any]:
    data: dict[str, Any] = {'version': VERSION, 'records': {}, 'unusable': []}
    for handle, place in old.items():
        try:
            refuse_whole_browser(place)
        except Refused:
            data['unusable'].append({'name': handle, 'place': place, 'why': WHOLE_BROWSER})
            continue
        data['records'].setdefault(place, _empty())['entries'][handle] = handle
    return data


class OwnerSecrets:
    def __init__(self, home: Path, keychain: Any = _keychain) -> None:
        self.path = home / 'owner-secret-handles.json'
        self.keychain = (lambda handle, value=None: _keychain(handle, value, home)) if keychain is _keychain else keychain

    def _load(self) -> dict[str, Any]:
        if not self.path.exists():
            return {'version': VERSION, 'records': {}, 'unusable': []}
        try:
            data = json.loads(self.path.read_text())
        except (OSError, ValueError) as e:
            raise Refused(f"FERB's saved secret names ({self.path.name}) can't be read, so nothing was changed: "
                          f"{type(e).__name__}") from None
        if isinstance(data, dict) and data.get('version') == VERSION:
            return data
        if isinstance(data, dict) and all(isinstance(k, str) and isinstance(v, str) for k, v in data.items()):
            return _migrated(data)
        raise Refused(f"FERB's saved secret names ({self.path.name}) are in a form it doesn't know, so nothing was "
                      "changed")

    def records(self) -> dict[str, dict[str, Any]]:
        return dict(self._load()['records'])

    def unusable(self) -> list[dict[str, str]]:
        return list(self._load()['unusable'])

    def username(self, target: str) -> str | None:
        value = self.records().get(target, {}).get('username')
        return str(value) if value else None

    def listing(self, target: str) -> dict[str, Any]:
        """What `secrets.list` shows for one place: its username and its entries' names, never a value or a Keychain
        account (L36 §5)."""
        data = self._load()
        record = data['records'].get(target, _empty())
        return {'place': target, 'username': record.get('username'), 'entries': sorted(record.get('entries', {})),
                'unusable': [{'name': u['name'], 'why': u['why']} for u in data['unusable']]}

    def reveal(self, handle: str, target: str) -> str:
        refuse_whole_browser(target)                    # a handle saved that way before the rule is never filled
        account = self.records().get(target, {}).get('entries', {}).get(handle)
        if account is None:
            raise Refused('the secret handle is not bound to this exact destination')
        value = self.keychain(account)
        if value is None:
            raise Refused('the saved secret is unavailable')
        return value

    def remember(self, handle: str, target: str, value: str) -> None:
        refuse_whole_browser(target)
        # Instances are short-lived per prompt; serialize their metadata updates across daemon worker threads.
        with _write_lock:
            data = self._load()
            record = data['records'].setdefault(target, _empty())
            account = record['entries'].get(handle) or f'{target} · {handle}'
            self.keychain(account, value)
            record['entries'][handle] = account
            self._save(data)

    def set_username(self, target: str, username: str, source: str) -> None:
        """The record's username, from an owner-stated memory item (`source`: "memory:<id>")."""
        with _write_lock:
            data = self._load()
            record = data['records'].setdefault(target, _empty())
            record['username'], record['username_source'] = username, source
            self._save(data)

    def clear_usernames(self, sources: set[str]) -> list[str]:
        """Clear every username that came from one of `sources` (a forgotten or retracted memory item); the places."""
        with _write_lock:
            data = self._load()
            cleared = [place for place, record in data['records'].items() if record.get('username_source') in sources]
            for place in cleared:
                data['records'][place]['username'] = data['records'][place]['username_source'] = None
            if cleared:
                self._save(data)
            return cleared

    def _save(self, data: dict[str, Any]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        tmp = self.path.with_suffix('.tmp')
        fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, 'w') as out:
            os.fchmod(out.fileno(), 0o600)
            json.dump(data, out)
        os.replace(tmp, self.path)
```

- [ ] **Step 4: The tool (`ferb/identity/secret_tools.py`)**

```python
"""Owns the read-only listing of the owner's saved logins (L36 §5). Never a value, never a Keychain account."""
import asyncio
from pathlib import Path
from typing import Any, Callable

from pydantic import BaseModel, Field

from ferb.core import origin as origins_
from ferb.core.types import ActionSpec, Identity, RiskLevel, ToolContext, ToolResult
from ferb.identity.owner_secrets import OwnerSecrets, login_place


class SecretsList:
    name = 'secrets.list'
    description = ("List what is saved for one login place: the username (plain text, from the owner's own words) and "
                   "the names of its sealed entries (the password, security answers). Never a value. Pass the login "
                   "page's address or origin (https://…), or app:<bundle id>. Call it on a login page before asking "
                   "the owner; fill a listed entry with browser_secret or mac_secret secret=<name>.")
    scopes, identities = {'task:read'}, {Identity.OWNER}
    risk, reversible, observation, origin = RiskLevel.LOW, True, True, origins_.PRIVATE

    class Input(BaseModel):
        place: str = Field(description="The login page's address or origin (https://…), or app:<bundle id>")

    def __init__(self, home: Callable[[], Path]) -> None:
        self.home = home

    def describe(self, args: BaseModel) -> ActionSpec:
        return ActionSpec('read')

    async def run(self, args: BaseModel, ctx: ToolContext) -> ToolResult:
        a: Any = args
        place = login_place(str(a.place))
        if not place:
            return ToolResult({'error': 'give an https:// address or app:<bundle id>'}, is_error=True)
        listing = await asyncio.to_thread(OwnerSecrets(self.home()).listing, place)
        # the record is the owner's own data; its entry names are a worker's wording
        return ToolResult(listing, read_sources=(('origin', origins_.PRIVATE), ('origin', origins_.INFERRED)))
```

In `ferb/daemon/main.py`, right after the `add_context_readers(...)` call:

```python
        from ferb.identity.secret_tools import SecretsList
        self.registry.add(SecretsList(lambda: self.home))              # L36 §5: names only, never values
```

- [ ] **Step 5: The hidden-input question names the username (`ferb/daemon/gateway.py`)**

These two functions are being edited by another agent: change their current bodies only as follows.
`secret_summary(target, remember, app=None, username=None)`: right after `place` is computed, add

```python
    if username:                                       # L36 §5: which account, from the owner's own words
        place = f"{' '.join(printable(username).split())} at {place}"
```

`secret_question(target, remember, username=None)`: after the `Destination: {target}.` sentence, add
`+ (f" Account: {' '.join(printable(username).split())}." if username else "")`.
In `Gateway.ask_secret`, after the `if ctx.request_id in self.state.approvals.unattended:` early return:

```python
        username = await asyncio.to_thread(store.username, target)       # shown to the owner, never a secret
        question = secret_question(target, remember, username)
```

and pass `username` as the fourth argument of `secret_summary(...)` in the `ap.summary = self._place_line(...)` line.

- [ ] **Step 6: Tell the worker (`ferb/orchestrator/prompts/worker.md`)**

After the paragraph ending "Use mac_secret for an exact native field.", add the sentence pinned in Step 1:

```markdown
Before typing into a login form, call secrets_list for that site. It gives the username saved for that site and the
names of its saved secrets, never their values, whatever your memory shows. Type a listed username like any text; fill
a listed secret with browser_secret (or mac_secret) secret=<its name>; ask the owner, through the hidden input, only for
what is not listed, and save it with remember=<a clear name>: "password", or the security question's own text.
```

- [ ] **Step 7: Run the focused tests**

Run: `uv run pytest -q tests/test_owner_secrets.py tests/test_question_clarity.py tests/test_prompt_contract.py tests/test_browser_tools.py tests/test_ferb_identity.py`
Expected: PASS.

- [ ] **Step 8: Docs**

`ARCHITECTURE.md` §7: replace "Optional owner-confirmed handles persist nonsecret target metadata in
`owner-secret-handles.json`; values live in Keychain." with "Owner-confirmed secrets are one login record per place (an
HTTPS origin or `app:<bundle id>`, L36) in `owner-secret-handles.json` version 2: a plain username from the owner's
words and named entries whose values live in Keychain; a version-1 file is read by place, its browser-bound handles
listed unusable."

`ARCHITECTURE.md` §16.12 bullet: "- **Login records (§5).** `OwnerSecrets` keeps one record per place (username,
`username_source`, entries name -> Keychain account `"<place> · <name>"`); saving a name again replaces its value. The
owner-only observation `secrets.list` (`identity/secret_tools.py`) returns a place's username, entry names and the
unusable list, never a value or account. The hidden-input line names "<username> at <site>"."

`README.md` "Memory and learning": "Passwords and security answers you save stay in your Mac's Keychain, one login per
site: on a login page FERB lists what's already saved for that site (names only, never values) and asks only for what's
missing. A password saved for a whole browser before 2026-10-24 can't be used and is listed as "ask again"."

`CHANGELOG.md` bullet: "- One login per site (L36, D12). Live 2026-10-24: a resumed visa task couldn't see the two
answers saved that morning and asked for them again, saving them under new names. Saved secrets are now one record per
site (the username, the password and each security answer under its question), a worker lists what's saved for the
page before typing into its login form (`secrets.list`: names only, never values, whatever memory ranked), saving the same name replaces it, and the hidden-input
question names the account ("for Asha Rao at usvisascheduling.com"). Your existing saved secrets keep working; the
visa password saved for Chrome as a whole is listed as "ask again"."

Add `"tests/test_owner_secrets.py"` to `[L36].tests`.

- [ ] **Step 9: Full checks, commit and push**

Run: `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`.

```bash
git add ferb/identity/owner_secrets.py ferb/identity/secret_tools.py ferb/daemon/main.py ferb/daemon/gateway.py \
  ferb/orchestrator/prompts/worker.md tests/test_owner_secrets.py tests/test_question_clarity.py \
  tests/test_prompt_contract.py docs/0.27_DECISIONS.toml ARCHITECTURE.md README.md CHANGELOG.md
git commit -m "feat(L36, D12): one login record per site; secrets.list names what is saved, never values

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 7: The username from the owner's own words (spec §5, "The username")

**Files:**
- Modify: `ferb/orchestrator/schemas.py` (`REMEMBER_SCHEMA.login_origin`, `Remember.login_origin`)
- Modify: `ferb/orchestrator/supervisor.py` (`_turn_sites`; `_side_entries` passes sites; the `remember` default)
- Modify: `ferb/daemon/main.py` (`_remember(…, sites)`, `_save_login_name`; usernames cleared on forget and retract)
- Modify: `ferb/orchestrator/prompts/supervisor.md`
- Create: `tests/test_login_names.py` (uses `tests/support/live_steer.py` from Task 3)
- Modify: `tests/test_prompt_contract.py` (the login sentence)
- Docs: `ARCHITECTURE.md` §16.12, `README.md`, `CHANGELOG.md`; add `"tests/test_login_names.py"` to `[L36].tests`

**Interfaces:**
- Consumes: `OwnerSecrets.set_username / clear_usernames / listing`, `login_place` (Task 6); `items.capture`,
  `items.owner_span` (Task 1); `provenance.sources` (`ferb/policy/provenance.py`); `urls.parse`.
- Produces: `Remember.login_origin: str | None = None`; `Supervisor._turn_sites(out, session) -> frozenset[str]`;
  `supervisor.remember(items, own_words, source_id, sites)`;
  `Daemon._remember(items, own_words, source_id, sites: frozenset[str] = frozenset())`;
  `Daemon._save_login_name(entry, item_id: str, own_words: str, sites: frozenset[str]) -> str`;
  (consumes) `tests/support/live_steer.py`: `StalledWorker(monkeypatch)`, `control_turn(goal, tree_id, action, **entries)`.

Ruling (content): for a login name the entry's `content` is the username alone; the memory item still keeps the
owner's whole line (as for every entry) and the record keeps the owner's own spelling of the username (`owner_span`).
Ruling (places): `login_origin` covers HTTPS origins; an app's username stays ordinary memory. The site counts as named
when its host (with or without "www.") is a token of the owner's words or equals, or is under, a site of the turn's task.

- [ ] **Step 1: Use the shared live-steer helpers**

`tests/support/live_steer.py` (`StalledWorker`, `control_turn`) was created in Task 3; the end-to-end test below uses
it.

- [ ] **Step 2: Write the failing tests** (`tests/test_login_names.py`)

```python
"""L36 §5: a username the owner gives is saved, from their own words only, as the site's login record's username.

Live 2026-10-24 06:31: "the username is Kartikey Attri", said while correcting the running visa task, was saved
nowhere; at 12:34 a plan step of a new visa task typed the owner's email as the username."""
import threading
from typing import Any

import pytest

from ferb.core.types import Identity
from ferb.identity.owner_secrets import OwnerSecrets
from ferb.orchestrator.plans import Plan, Step
from ferb.orchestrator.schemas import Remember
from tests.conftest import chat, done, install, sup, task
from tests.mackit import gateway_call as call, grant_task as grant
from tests.support.live_steer import StalledWorker, control_turn
from tests.support.memory_items import journal, owner_fact, run

VISA = "https://www.usvisascheduling.com"


def entry(content: str = "Asha Rao", origin: str = VISA, quote: str = "the username is Asha Rao") -> Remember:
    return Remember(kind="fact", subject="owner", key="usvisascheduling-username", content=content, quote=quote,
                    login_origin=origin)


def remember(d: Any, items: list[Remember], own: str, sites: frozenset[str]) -> list[dict[str, str]]:
    src = journal(d, "chat.message", {"text": own, "untrusted_parts": 0})
    return list(run(d, lambda: d.daemon._remember(items, own, src, sites)))


def username(d: Any) -> str | None:
    return run(d, lambda: OwnerSecrets(d.daemon.home).username(VISA))


def test_a_username_in_the_owners_words_for_the_tasks_site_is_saved(daemon: Any) -> None:
    lines = remember(daemon, [entry()], "the username is asha rao", frozenset({"usvisascheduling.com"}))
    [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert (item.origin, item.content) == ("owner_stated", "the username is asha rao")
    assert username(daemon) == "asha rao"                            # the owner's own spelling
    assert lines[0]["line"].endswith("Saved as your login name for www.usvisascheduling.com.")
    record = run(daemon, lambda: OwnerSecrets(daemon.daemon.home).records()[VISA])
    assert record["username_source"] == "memory:" + item.id


def test_a_site_the_owner_typed_names_it_too(daemon: Any) -> None:
    remember(daemon, [entry()], "the username is Asha Rao on usvisascheduling.com", frozenset())
    assert username(daemon) == "Asha Rao"


@pytest.mark.parametrize("item, sites", [
    (entry(), frozenset()),                                                            # no site named anywhere
    (entry(content="Asha R."), frozenset({"usvisascheduling.com"})),                    # not the owner's words
    (entry(origin="http://www.usvisascheduling.com"), frozenset({"usvisascheduling.com"})),  # not HTTPS
    (entry(origin="https://evil.example"), frozenset({"usvisascheduling.com"})),        # another site
])
def test_no_username_is_saved_without_the_owners_words_and_a_named_https_site(daemon: Any, item: Remember,
                                                                             sites: frozenset[str]) -> None:
    remember(daemon, [item], "the username is Asha Rao", sites)
    assert username(daemon) is None


def test_forgetting_the_memory_clears_the_username(daemon: Any) -> None:
    """L36 §5: the login record's username goes with the memory it came from."""
    remember(daemon, [entry()], "the username is Asha Rao", frozenset({"usvisascheduling.com"}))
    [item] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    run(daemon, lambda: daemon.daemon._forget_memory(item.id))
    daemon.run(daemon.daemon._settle_forgets())
    assert username(daemon) is None


def test_a_username_said_in_a_live_correction_reaches_the_next_tasks_login_step(daemon: Any,
                                                                               monkeypatch: pytest.MonkeyPatch) -> None:
    """The spec's username case: a live steer saves the fact and the record's username; a new task's login step gets
    the fact in its memory, and secrets.list returns the username."""
    for i in range(6):
        owner_fact(daemon, f"hose_{i}", f"garden hose note {i}")
    worker = StalledWorker(monkeypatch)
    script = install(daemon, [sup("task", "Logging in.", task("Log in to usvisascheduling.com",
                                                              sites=("usvisascheduling.com",)))])
    first: list[dict[str, Any]] = []
    thread = threading.Thread(target=lambda: first.extend(chat(daemon, "log in to the visa site", session="visa")))
    thread.start()
    try:
        assert worker.started.wait(5)
        tree_id = worker.handles[0].task_id.rsplit(".", 1)[0]
        script.sup.append(control_turn("Log in to usvisascheduling.com as Asha Rao", tree_id, "steer", remember=[{
            "kind": "fact", "subject": "owner", "key": "usvisascheduling-username", "content": "Asha Rao",
            "quote": "the username is Asha Rao", "login_origin": VISA}]))
        chat(daemon, "the username is Asha Rao", session="visa", timeout=5)
    finally:
        worker.release.set()
        thread.join(5)
    assert username(daemon) == "Asha Rao"
    spec = task("Log in to usvisascheduling.com and read the appointment page", sites=("usvisascheduling.com",))
    spec["plan"] = Plan(steps=[Step(id="login", kind="do", goal="Sign in on the login page")]).model_dump()
    script.sup.append(sup("task", "On it.", spec))
    script.workers["step.login"] = done("Signed in.")
    chat(daemon, "check my visa appointment", session="other")
    step = next(ctx for _, key, ctx, _ in script.contexts if key == "step.login")
    assert "the username is Asha Rao" in step
    listed = call(daemon, grant(daemon, "q", Identity.OWNER, {"task:read"}, None), "secrets.list", place=VISA)
    assert listed["content"]["username"] == "Asha Rao"
```

Update the login sentence pinned in `tests/test_prompt_contract.py` (the entry commented "live 2026-10-24: the owner
steered the visa task with "the username is …"") to the new prompt text of Step 6.

- [ ] **Step 3: Run them to verify they fail**

Run: `uv run pytest -q tests/test_login_names.py tests/test_control_turn_entries.py`
Expected: `test_control_turn_entries.py` PASSES; `test_login_names.py` FAILS
(`Remember` has no field `login_origin`, or `_remember()` takes 4 positional arguments).

- [ ] **Step 4: Schema (`ferb/orchestrator/schemas.py`)**

`REMEMBER_SCHEMA` gains `"login_origin": _nullable("string")`; `Remember` gains
`login_origin: str | None = None   # L36 §5: this entry's content is the username for this https:// site`.

- [ ] **Step 5: Pass the turn's sites (`ferb/orchestrator/supervisor.py`)**

The `remember` default becomes
`lambda items, own_words, source, sites=frozenset(): [{"refused": "FERB has no memory store running"} for _ in items]`
(type `Callable[..., list[dict[str, str]]]`). Add:

```python
    def _turn_sites(self, out: SupervisorOutcome, session: ChatSession) -> frozenset[str]:
        """The sites this turn's task names: its new task's, or the task a stop, steer, continue or retry acts on (live
        or saved). Where a login name the owner gives may be saved for (L36 §5)."""
        specs: list[Any] = [out.task] if out.task is not None else []
        if out.control is not None:
            live = self._tree_of.get(out.control.task_id)
            saved = None if live is not None else self.continuity.load(out.control.task_id, session.id)
            specs.append(live.spec if live is not None else saved[0] if saved else None)
        return frozenset(site for spec in specs if spec is not None for site in spec.targets.sites)
```

In `_side_entries`, compute `sites = self._turn_sites(out, session)` before the remember loop and call
`self.remember([item], typed_lines, entry_id, sites)`.

- [ ] **Step 6: Tell the planner (`ferb/orchestrator/prompts/supervisor.md`)**

Replace the sentence that starts "When the owner tells you a login name or username for a site or app" (through
"Passwords and other secrets go through the hidden-input path.") with:

```markdown
When the owner tells you a login name or username for a site or app ("the username is Asha Rao" while working on that
site), remember it in the same turn as the task change: kind fact, subject "owner" or the project ("project:<slug>"),
a key naming the site ("usvisascheduling-username"), content: the username alone, exactly as they typed it ("Asha
Rao"), and login_origin: the site's https:// address; FERB saves their whole line as the memory and the username for
that site. Passwords and other secrets go through the hidden-input path. Otherwise login_origin is null.
```

- [ ] **Step 7: Save the username (`ferb/daemon/main.py`)**

`_remember` takes `sites: frozenset[str] = frozenset()`; in its owner-stated branch:

```python
            if m.origin == "owner_stated":
                told = self._save_login_name(it, m.id, own_words, sites) if getattr(it, "login_origin", None) else ""
                saved.append({"receipt": f"remembered ({m.id}): {m.content}",
                              "line": f"Remembered: {text}" + (" " + told if told else "")})
```

Add:

```python
    def _save_login_name(self, entry: Any, item_id: str, own_words: str, sites: frozenset[str]) -> str:
        """L36 §5: an owner-stated entry's username for its site's login record, only when the username is the owner's
        own words (kept in their spelling) and the site is an https:// origin the owner's words or the turn's task name.
        A line for the owner, or "" (the refusal is journaled)."""
        from ferb.identity.owner_secrets import OwnerSecrets, login_place
        from ferb.memory.items import owner_span
        from ferb.policy import provenance, urls
        place = login_place(str(entry.login_origin))
        parsed = urls.parse(place) if place.startswith("https://") else None
        host = parsed.host if parsed is not None else ""

        def same(site: str) -> bool:
            other = (urls.parse(site).host if "://" in site and urls.parse(site) else site.strip().lower().split("/")[0])
            return bool(other) and (host == other or host.endswith("." + other))
        named = bool(host) and (any(provenance.sources(h, own_words, [], kind="structured") != ["unknown"]
                                    for h in {host, host.removeprefix("www.")}) or any(same(s) for s in sites))
        username = owner_span(own_words, str(entry.content))
        if not named or not username:
            why = "the site isn't one you named" if not named else "the username isn't in your words"
            self._journal().append("memory.login_refused", {"item": item_id, "why": why})
            return ""
        try:
            OwnerSecrets(self.home).set_username(place, username, "memory:" + item_id)
        except Refused as e:
            self._journal().append("memory.login_refused", {"item": item_id, "why": str(e)})
            return ""
        return f"Saved as your login name for {host}."
```

Clear usernames when their memory goes: at the end of `_forget_learning_sources(plan)` add

```python
        try:                                    # L36 §5: a username goes with the memory it came from
            OwnerSecrets(self.home).clear_usernames({"memory:" + i for i in plan.items})
        except Refused as e:
            self._journal().append("memory.login_unreadable", {"error": str(e)})
```

and in `_memory_retract`, after `self.memory.retract(...)`, the same `clear_usernames({"memory:" + str(params["id"])})`
in the same `try`. (Import `OwnerSecrets` from `ferb.identity.owner_secrets` at the use sites.)

- [ ] **Step 8: Run the focused tests**

Run: `uv run pytest -q tests/test_login_names.py tests/test_control_turn_entries.py tests/test_prompt_contract.py tests/test_memory.py tests/test_forget_complete.py`
Expected: PASS.

- [ ] **Step 9: Docs**

`ARCHITECTURE.md` §16.12 bullet: "- **The username (§5).** A planner `remember` entry may carry `login_origin`; its
content is then the username alone. `Daemon._save_login_name` writes the owner's own spelling of it (`owner_span`) to
that https:// origin's record only from an owner-stated item, when the host is a token of the owner's words or the
turn's task names the site (`Supervisor._turn_sites`), with `username_source` `memory:<id>`; a forget or retract of that
item clears it. Refusals are journaled (`memory.login_refused`)."

`README.md`: "Tell FERB a site's username while it works ("the username is …") and it is remembered for that site and
offered on its login page; forgetting that memory removes it from the login record too."

`CHANGELOG.md` bullet: "- A username you give is kept for its site (L36). Live 2026-10-24: "the username is …" during
the visa task was saved nowhere, and a later task typed your email. Now the username, in your own spelling, is saved for
the site you named or the task's site, shown in the hidden-input question, listed on the login page and given to the
steps that log in; forgetting the memory removes it."

Add `"tests/test_login_names.py"` to `[L36].tests`.

- [ ] **Step 10: Full checks, commit and push**

Run: `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`.

```bash
git add ferb/orchestrator/schemas.py ferb/orchestrator/supervisor.py ferb/daemon/main.py \
  ferb/orchestrator/prompts/supervisor.md tests/test_login_names.py tests/test_prompt_contract.py \
  docs/0.27_DECISIONS.toml ARCHITECTURE.md README.md CHANGELOG.md
git commit -m "feat(L36, P27): a username in the owner's words is saved for its site's login record

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 8: Out-of-date facts retire (spec §6)

**Files:**
- Modify: `ferb/clock/when.py` (`end_of_day`)
- Modify: `ferb/orchestrator/schemas.py` (`UNTIL_SCHEMA`, `Until`, `Remember.until`)
- Modify: `ferb/memory/items.py` (`prompt_ready(item, now)`, `Memory.items` time-aware, `Memory.expire_at`,
  `Memory.retire`)
- Modify: `ferb/memory/learning.py` (`sync_facts` passes the clock's now)
- Modify: `ferb/memory/distill.py`, `ferb/memory/distill.md` (`retire`)
- Modify: `ferb/orchestrator/steering.py` (`RETIRE_MEMORY_ASK`), `ferb/cli/chat.py` (`FERB_CHECKS`)
- Modify: `ferb/daemon/main.py` (`_until`, `_ask_retire`, retire answers and restore)
- Modify: `ferb/orchestrator/prompts/supervisor.md`
- Create: `tests/test_memory_retirement.py`
- Modify: `tests/support/memory_items.py` (`distill_item` gains `"retire": False`), `tests/test_memory.py`,
  `tests/test_decide.py` (answers carry `"retire": False`), `tests/test_prompt_contract.py`
- Docs: `ARCHITECTURE.md` §16.12, `README.md`, `CHANGELOG.md`; add `"tests/test_memory_retirement.py"` to `[L36].tests`

**Interfaces:**
- Consumes: `owner_said` (Task 1), `Daemon._ask_owner` / `MEMORY_QUESTIONS` (Task 1), `_learning_candidates` (Task 3).
- Produces: `when.end_of_day(w: When, now: datetime, zone=None) -> datetime` (UTC);
  `schemas.Until(quote, day=None, date=None, in_days=None)`, `Remember.until: Until | None = None`;
  `items.prompt_ready(item, now: datetime | None = None)`; `Memory.expire_at(item_id, at: datetime)`;
  `Memory.retire(item_id, reason)`; `Distiller.ask_retire: Callable[[MemoryItem], None]`;
  `steering.RETIRE_MEMORY_ASK = "memory.retire"`; `Daemon._ask_retire(item)`, `Daemon._until(until, item_id, own_words) -> str`.

Ruling (until): `{quote, day | date | in_days}`; the quote must be whole words of what the owner typed and code works out
the day (the model never computes dates, owner 2026-09-30); the item stops being valid at the next local midnight after
that day, whatever its origin. A day already over, or date words the owner didn't type, save the item without an end
and say so.
Ruling (distill retire): an item with `retire: true` names a current item by kind, subject and key; it retires only on
a passing quote (§2's check); otherwise the owner gets "Is this no longer true? …" with Retire or Keep.

- [ ] **Step 1: Write the failing tests** (`tests/test_memory_retirement.py`)

```python
"""L36 §6: out-of-date facts retire. A dated fact leaves search after its day, a later statement supersedes an earlier
one, and distillation retires an item only on the owner's own words (otherwise it asks).

Live 2026-10-24: the 10-11 wedding flight, "DevOps left" and the finished RSNA competition still sat in every planner
prompt; memory item 54 still said "Chennai, notify first" after the owner said "book anywhere"."""
from datetime import datetime, timezone
from typing import Any
from zoneinfo import ZoneInfo

import pytest

from ferb.clock.when import When, WhenError, end_of_day, local_zone
from ferb.orchestrator.schemas import Remember, Until
from tests.support.memory_items import c_handle, distill_day, distill_item, journal, owner_fact, pending, run, said


def test_end_of_day_is_the_next_local_midnight() -> None:
    zone = ZoneInfo("Asia/Kolkata")
    now = datetime(2026, 10, 24, 10, 0, tzinfo=zone)
    assert end_of_day(When(date="10-25"), now, zone) == datetime(2026, 10, 26, tzinfo=zone).astimezone(timezone.utc)
    assert end_of_day(When(day="today"), now, zone) == datetime(2026, 10, 25, tzinfo=zone).astimezone(timezone.utc)
    with pytest.raises(WhenError):
        end_of_day(When(date="2026-10-23"), now, zone)                      # already over
    with pytest.raises(WhenError):
        end_of_day(When(day="today", time="18:00"), now, zone)              # a day, not a time


def chain(d: Any) -> None:
    for i in range(4):
        owner_fact(d, f"chain_{i}", f"bicycle chain note {i}")


def pune(d: Any) -> list[str]:
    return [c["text"] for c in d.run(d.daemon._learning_candidates("where is Pune")) if "Pune" in c["text"]]


def test_a_dated_fact_leaves_search_after_its_day(daemon: Any) -> None:
    zone = local_zone()
    daemon.call_soon(daemon.daemon.clock.set_wall, datetime(2026, 10, 24, 10, 0, tzinfo=zone))
    chain(daemon)
    own = "I'm in Pune until the 25th"
    src = journal(daemon, "chat.message", {"text": own, "untrusted_parts": 0})
    item = Remember(kind="fact", subject="owner", key="city", content="I'm in Pune", quote="I'm in Pune",
                    until=Until(quote="until the 25th", date="10-25"))
    [line] = run(daemon, lambda: daemon.daemon._remember([item], own, src))
    assert line["line"].endswith("(until Sun 25 Oct)")
    [saved] = [i for i in run(daemon, lambda: daemon.daemon.memory.items("active")) if i.key == "city"]
    assert saved.valid_to == datetime(2026, 10, 26, tzinfo=zone)
    assert pune(daemon)
    daemon.call_soon(daemon.daemon.clock.set_wall, datetime(2026, 10, 26, 0, 1, tzinfo=zone))
    assert pune(daemon) == []
    assert run(daemon, lambda: daemon.daemon.memory.get(saved.id)).status == "active"       # kept as history


@pytest.mark.parametrize("until", [Until(quote="till Friday", day="friday"),                 # not what they typed
                                   Until(quote="until the 23rd", date="2026-10-23")])         # already over
def test_an_until_the_owner_didnt_type_or_that_is_over_saves_no_end(daemon: Any, until: Until) -> None:
    daemon.call_soon(daemon.daemon.clock.set_wall, datetime(2026, 10, 24, 10, 0, tzinfo=local_zone()))
    own = "I'm in Pune until the 23rd"
    src = journal(daemon, "chat.message", {"text": own, "untrusted_parts": 0})
    item = Remember(kind="fact", subject="owner", key="city", content="I'm in Pune", quote="I'm in Pune", until=until)
    [line] = run(daemon, lambda: daemon.daemon._remember([item], own, src))
    assert "saved without an end date" in line["line"]
    [saved] = run(daemon, lambda: daemon.daemon.memory.items("active"))
    assert saved.valid_to is None


def test_a_later_statement_replaces_the_earlier_one(daemon: Any) -> None:
    chain(daemon)
    first = owner_fact(daemon, "appointment_preferences",
                       "visa: Chennai for both centers, any date, notify me before booking")
    owner_fact(daemon, "appointment_preferences", "book the visa anywhere, don't wait for Chennai, book then tell me")
    found = [c["text"] for c in daemon.run(daemon.daemon._learning_candidates("visa booking Chennai"))]
    assert any("book the visa anywhere" in t for t in found)
    assert not any("notify me before booking" in t for t in found)
    assert run(daemon, lambda: daemon.daemon.memory.get(first.id)).valid_to is not None


def test_distill_retires_an_item_on_the_owners_own_words(daemon: Any) -> None:
    chain(daemon)
    rsna = owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    handle = c_handle(daemon, said(daemon, "the RSNA competition is over"))
    distill_day(daemon, distill_item(handle, "the RSNA competition is over", "the RSNA competition is over",
                                     kind="fact", key="rsna", retire=True))
    assert run(daemon, lambda: daemon.daemon.memory.get(rsna.id)).valid_to is not None
    assert pending(daemon, "memory.retire") == []


def test_a_retire_without_the_owners_words_is_asked(daemon: Any) -> None:
    chain(daemon)
    rsna = owner_fact(daemon, "rsna", "I'm in the RSNA knee competition")
    handle = c_handle(daemon, said(daemon, "kaggle results came out today"))
    distill_day(daemon, distill_item(handle, "The RSNA competition is over", kind="fact", key="rsna", retire=True))
    assert run(daemon, lambda: daemon.daemon.memory.get(rsna.id)).valid_to is None
    [q] = pending(daemon, "memory.retire")
    assert q.options == ("Retire", "Keep") and "RSNA" in q.reason
    run(daemon, lambda: daemon.daemon._resolve_approval(q.id, True, 1))
    assert run(daemon, lambda: daemon.daemon.memory.get(rsna.id)).valid_to is not None
```

Add to `SENTENCES` in `tests/test_prompt_contract.py`:

```python
    # L36 §6: dated facts (the 10-11 wedding flight) never retired
    ("supervisor.md",
     'When the owner\'s words give the item an end ("until Sunday", "till the 31st"), add until: quote, the owner\'s '
     'exact date words, and only what they said in day (today, tomorrow, a weekday …), date ("MM-DD" or "YYYY-MM-DD") '
     'or in_days; FERB works out the date, and the item leaves memory after that day. Otherwise until is null.'),
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest -q tests/test_memory_retirement.py`
Expected: FAIL at import (`cannot import name 'end_of_day'`).

- [ ] **Step 3: `end_of_day` (`ferb/clock/when.py`)**

```python
def end_of_day(w: When, now: datetime, zone: ZoneInfo | None = None) -> datetime:
    """The end of the calendar day `w` names, its next local midnight (UTC), strictly after `now` (L36 §6: a remembered
    item's last day). Only one of day, date or in_days; never a time."""
    zone = zone or local_zone()
    if w.in_minutes is not None or w.time:
        raise WhenError("an end names a day, not a time")
    if sum(value is not None for value in (w.day, w.date, w.in_days)) != 1:
        raise WhenError("give exactly one calendar day source: day, date or in_days")
    d = _day(w, now.astimezone(zone).date())
    assert d is not None
    end = _wall(d + timedelta(days=1), time(0, 0), zone)
    if end <= now:
        raise WhenError(f"that day is already over ({d:%a %d %b})")
    return end
```

- [ ] **Step 4: Schema (`ferb/orchestrator/schemas.py`)**

Import `DAY_WORDS` with `WHEN_SCHEMA` from `ferb.clock.when`. Add before `REMEMBER_SCHEMA`:

```python
UNTIL_SCHEMA = _obj({"quote": {"type": "string"},
                     "day": {"anyOf": [{"type": "string", "enum": list(DAY_WORDS)}, {"type": "null"}]},
                     "date": _nullable("string"), "in_days": _nullable("integer")})
```

`REMEMBER_SCHEMA` gains `"until": {"anyOf": [{"type": "null"}, UNTIL_SCHEMA]}`. Add the model and field:

```python
class Until(BaseModel):
    """A remembered item's last day in the owner's own date words (L36 §6); code works out the date."""
    quote: str
    day: str | None = None
    date: str | None = None
    in_days: int | None = None
```

`Remember` gains `until: Until | None = None`.

- [ ] **Step 5: Validity in time (`ferb/memory/items.py`, `ferb/memory/learning.py`)**

Add `timedelta` to the `datetime` import. `prompt_ready`:

```python
def prompt_ready(item: MemoryItem, now: datetime | None = None) -> bool:
    """Whether an item may shape FERB's prompts: a current active item (one with an end date, until then: L36 §6), or
    an observed pattern still gathering days (shown as an unconfirmed hint). An inferred item (model wording) waits for
    the owner's Keep (I1, MP2)."""
    current = item.valid_to is None or (now is not None and item.valid_to > now)
    return current and (item.status == "active" or (item.status == "proposed" and item.origin == "observed"))
```

`Memory.items`: replace `if valid_now: q += " AND valid_to IS NULL"` and the return with

```python
        now = self.clock.now()
        if valid_now:                 # a coarse bound in SQL; the exact one in Python (offsets may differ)
            q, args = q + " AND (valid_to IS NULL OR valid_to > ?)", args + [(now - timedelta(days=1)).isoformat()]
        found = [self._item(r) for r in self.db.conn.execute(q + " ORDER BY subject, kind, key, seq", args)]
        return [it for it in found if not valid_now or it.valid_to is None or it.valid_to > now]
```

Add to `Memory` after `drop`:

```python
    def expire_at(self, item_id: str, at: datetime) -> None:
        """The owner's words give this item an end (L36 §6): it stops being valid at `at`, kept as history."""
        stamp = at.astimezone(self.clock.now().tzinfo).isoformat()     # the clock's offset, as other stamps have
        self.db.conn.execute("UPDATE memory_items SET valid_to = ? WHERE id = ? AND valid_to IS NULL", (stamp, item_id))
        self.publish("memory.expires", {"id": item_id, "at": stamp})
        self.on_change()

    def retire(self, item_id: str, reason: str) -> None:
        """The owner's words say this current item no longer holds (L36 §6): it stops being valid now, kept as
        history."""
        it, now = self.get(item_id), self.clock.now()
        if it is None or it.status != "active" or (it.valid_to is not None and it.valid_to <= now):
            raise MemoryRefused(f"no current item {item_id}")
        self.db.conn.execute("UPDATE memory_items SET valid_to = ? WHERE id = ?", (now.isoformat(), item_id))
        self.publish("memory.retired", {"id": item_id, "reason": reason})
        self.on_change()
```

`Learning.sync_facts`: read `now = self.clock.now()` at its start and call `prompt_ready(source, now)` and
`prompt_ready(item, now)`.

- [ ] **Step 6: Distill may retire (`ferb/memory/distill.py`, `ferb/memory/distill.md`)**

`DISTILL_SCHEMA` item: add `"retire"` to `"required"` and `"retire": {"type": "boolean"}` to `"properties"`.
`Distiller.__init__`: add `self.ask_retire: Callable[[Any], None] = lambda item: None`. Add the method:

```python
    def _remembered(self, item: dict[str, Any]) -> Any:
        """The current item a retire names, by kind, subject and key as `Memory.propose` stores them, or None."""
        from ferb.memory.items import slug
        subject = self.memory.redact(str(item.get("subject", "")).strip()) or "owner"
        key = slug(self.memory.redact(str(item.get("key", ""))))
        return next((i for i in self.memory.items("active", subject=subject)
                     if i.kind == item.get("kind") and i.key == key), None)
```

In `run_day`'s item loop, first thing inside the `try:` (before `srcs = check(item, ev)`):

```python
                if item.get("retire"):                 # L36 §6: on the owner's own words, else the owner is asked
                    check(item, ev)
                    target = self._remembered(item)
                    if target is None:
                        raise MemoryRefused("it retires nothing FERB remembers")
                    if owner_said(item, ev) is not None:
                        self.memory.retire(target.id, f"the owner's words on {day}")
                    else:
                        self.ask_retire(target)
                    continue
```

`distill.md`, a bullet after the origin bullets:

```markdown
   - "retire": true when the owner's words say something under "What FERB already remembers" is no longer true ("the
     RSNA competition is over"): give that item's kind, subject and key, content saying what changed, and the quote
     of the owner's words; FERB retires it only when the quote is theirs, and otherwise asks the owner. Every other
     item has "retire": false.
```

- [ ] **Step 7: Questions and answers (`steering.py`, `cli/chat.py`, `ferb/daemon/main.py`)**

`steering.py`: add `RETIRE_MEMORY_ASK = 'memory.retire'` (comment: "L36 §6: the owner's Retire or Keep on an item FERB
thinks is out of date"), add it to `FERB_CHECKS` and `OPTION_ONLY`, and to `MEMORY_QUESTIONS` as
`'retire': (RETIRE_MEMORY_ASK, ('Retire', 'Keep'))`. `ferb/cli/chat.py`: add `"memory.retire"` to `FERB_CHECKS`.

`main.py`: constant `RETIRE_QUESTION = "Is this no longer true? “{text}” FERB thinks something you said changed it."`;
in `_build_memory`, `self.distiller.ask_retire = self._ask_retire`; add:

```python
    def _ask_retire(self, item: Any) -> None:
        """L36 §6: an item distillation thinks is out of date, without the owner's words for it: asked once."""
        if self.db.conn.execute("SELECT 1 FROM memory_questions WHERE target = ? AND kind = 'retire'",
                                (item.id,)).fetchone():
            return
        self._ask_owner("retire", item.id, RETIRE_QUESTION.format(text=" ".join(item.content.split())),
                        "memory-retire-" + item.id)

    def _until(self, until: Any, item_id: str, own_words: str) -> str:
        """A remembered item's last day from the owner's own date words (L36 §6): the quote must be whole words of what
        they typed; code works out the day (`end_of_day`) and the item leaves memory after it. A line for the owner."""
        from ferb.clock.when import When, WhenError, end_of_day
        from ferb.memory.items import contains_words
        if not contains_words(own_words, str(until.quote)):
            self._journal().append("memory.until_refused", {"item": item_id, "why": "date words not typed"})
            return "(saved without an end date: the date words I quoted aren't in what you typed)"
        try:
            end = end_of_day(When(day=until.day, date=until.date, in_days=until.in_days), self.clock.now())
        except WhenError as e:
            self._journal().append("memory.until_refused", {"item": item_id, "why": str(e)})
            return f"(saved without an end date: {e})"
        self.memory.expire_at(item_id, end)
        return f"(until {(end - timedelta(seconds=1)).astimezone(self.clock.now().tzinfo):%a %d %b})"
```

In `_remember`, after the `Remembered: …` line is built for the saved item (owner-stated or not), add
`if getattr(it, "until", None) is not None:` then append `" " + self._until(it.until, m.id, own_words)` to the line
(for an inferred item, to its "Saved in my words…" line).

`_question_answered`, before the forget handling:

```python
        if what == "retire":
            if ap.state == "granted" and ap.choice == "Retire":
                try:
                    self.memory.retire(ident, "the owner said so")
                except MemoryRefused:
                    return "That memory isn't current any more."
                return "Retired: FERB won't use it any more."
            return "Kept as it is."
```

`_restore_memory_questions`: for kind `retire`, `gone = item is None or item.status != "active" or (item.valid_to is
not None and item.valid_to <= self.clock.now())`.

- [ ] **Step 8: Tell the planner (`ferb/orchestrator/prompts/supervisor.md`)**

At the end of the "remember" bullet (before "Don't say in text whether it was saved"), add the sentence pinned in
Step 1.

- [ ] **Step 9: Answers carry `retire`**

`tests/support/memory_items.py::distill_item`: the dict gains `"retire": False` before `| fields`.
`tests/test_memory.py` (the distill test's `item()` dict) and `tests/test_decide.py` (the memory proposal item) gain
`"retire": False`.

- [ ] **Step 10: Run the focused tests**

Run: `uv run pytest -q tests/test_memory_retirement.py tests/test_memory_capture.py tests/test_memory.py tests/test_decide.py tests/test_prompt_contract.py tests/test_schedules.py tests/test_memory_search.py`
Expected: PASS.

- [ ] **Step 11: Docs**

`ARCHITECTURE.md` §16.12 bullet: "- **Retirement (§6).** A planner `remember` entry may carry `until` {quote, day | date
| in_days}: the quote must be whole words of the owner's typed text; `when.end_of_day` gives the next local midnight
after that day and `Memory.expire_at` sets `valid_to`; `prompt_ready(item, now)` and `Memory.items` treat an item as
current until then, so it leaves search afterwards. Distillation's `retire: true` items name a current item; with a
passing quote `Memory.retire` ends it now, else the owner gets a Retire / Keep question (`memory.retire`, a FERB check).
Supersession by a reused key works on every turn path (1c22dcb)."

`README.md`: "Things you say with a date ("I'm in Pune until the 25th") stop being used after that day; when you say
something is over, FERB retires it, or asks you when it only thinks so."

`CHANGELOG.md` bullet: "- Old facts retire (L36). Live: the 10-11 wedding flight and the finished RSNA competition were
still in every planner prompt. A fact you give with a date now leaves memory after that day, and when your words say
something is over FERB retires it (or asks: "Is this no longer true? …", Retire or Keep)."

Add `"tests/test_memory_retirement.py"` to `[L36].tests`.

- [ ] **Step 12: Full checks, commit and push**

Run: `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`.

```bash
git add ferb/clock/when.py ferb/orchestrator/schemas.py ferb/memory/items.py ferb/memory/learning.py \
  ferb/memory/distill.py ferb/memory/distill.md ferb/orchestrator/steering.py ferb/cli/chat.py ferb/daemon/main.py \
  ferb/orchestrator/prompts/supervisor.md tests/test_memory_retirement.py tests/support/memory_items.py \
  tests/test_memory.py tests/test_decide.py tests/test_prompt_contract.py docs/0.27_DECISIONS.toml ARCHITECTURE.md \
  README.md CHANGELOG.md
git commit -m "feat(L36): dated facts leave memory after their day; distill retires on the owner's words or asks

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 9: Redaction matches sealed values safely (spec §7)

**Files:**
- Modify: `ferb/sanitize/redact.py` (`FERB_ID`, `sealed_spans`, `holds_sealed`; `Redactor._registered`, `Redactor.ident`)
- Modify: `ferb/memory/learning.py` (`redacted_json`'s final check)
- Create: `ferb/sanitize/dictionary.py` (`ordinary_word`)
- Modify: `ferb/daemon/gateway.py` (`Gateway.ask_secret` journals `secret.ordinary_word`)
- Modify: `ferb/orchestrator/supervisor.py` (`ORDINARY_WORD_LINE`, `_secret_lines`, used in `_final`)
- Create: `tests/test_sealed_words.py`
- Docs: `ARCHITECTURE.md` (§12's redaction paragraph if it names substring matching; §16.12), `README.md`,
  `CHANGELOG.md`; add `"tests/test_sealed_words.py"` to `[L36].tests`

**Interfaces:**
- Consumes: `ID_BODY`, `_mask_spans`, `REDACTED` (redact.py); `secret_summary`'s `_site` (gateway.py).
- Produces: `redact.FERB_ID: re.Pattern`, `redact.sealed_spans(s: str, value: str) -> list[tuple[int, int]]`,
  `redact.holds_sealed(s: str, values: Iterable[str]) -> bool`; `dictionary.SYSTEM_WORDS: Path`,
  `dictionary.ordinary_word(value: str, words: Path | None = None) -> bool`; journal kind `secret.ordinary_word`
  `{task_id, place}`; `supervisor.ORDINARY_WORD_LINE`, `Supervisor._secret_lines(tree_id) -> list[str]`.

Ruling (matching): a sealed value made only of letters, marks and spaces (a word-like answer: a city, a food) is replaced
only as whole words, edge-aware (a boundary is required only where the value's own edge is a letter); any other sealed
value (a password, a PIN) is still replaced wherever it appears; neither is ever replaced inside one of FERB's ids. The
spec's "only as a whole token" for every value would let a password glued to other text through (rule 14).
Ruling (line): "The answer you saved for <place> is an ordinary word, so FERB hides it wherever it appears.", once per
place at the end of the task's answer, from FERB's journal (never the word).

- [ ] **Step 1: Write the failing tests** (`tests/test_sealed_words.py`)

```python
"""L36 §7: sealed values are redacted safely. A word-like answer is hidden as a word, never inside longer words; no
sealed value is ever replaced inside FERB's own ids; a password glued to other text is still hidden; and saving an
answer that is an ordinary word is said once.

Live 2026-10-24: one sealed answer is an ordinary word, so that word read [redacted] inside FERB's notes about the
owner and the visa task's episode was refused twice; on 10-14, 48 captures failed because a short sealed value matched
inside hex ids."""
import asyncio
import json
import time
from pathlib import Path
from typing import Any

import pytest

from ferb.core.types import Identity, ToolContext
from ferb.memory.learning import Learning
from ferb.orchestrator.supervisor import ORDINARY_WORD_LINE
from ferb.sanitize.dictionary import ordinary_word
from ferb.sanitize.redact import Redactor, sealed_spans
from tests.support.learning_fixtures import Clock
from tests.support.memory_items import run


def test_a_word_like_sealed_value_is_hidden_as_a_word_not_inside_longer_words() -> None:
    r = Redactor()
    r.remember_hidden_secret("pen")
    assert r.text("a pen, an open door and penguins") == "a [redacted], an open door and penguins"


def test_a_sealed_value_is_never_replaced_inside_ferbs_ids() -> None:
    r = Redactor()
    r.remember_hidden_secret("8e95")
    ident = "m-" + "8e95" * 16
    assert r.text(f"see {ident} done") == f"see {ident} done"
    assert r.ident(ident) == ident
    assert r.text("value 8e95 here") == "value [redacted] here"     # a value with digits: anywhere outside ids


@pytest.mark.parametrize("text, want", [("?p=hunter2!x", "?p=[redacted]x"), ("userhunter2!", "user[redacted]")])
def test_a_password_glued_to_other_text_is_still_hidden(text: str, want: str) -> None:
    """Review Focus."""
    r = Redactor()
    r.remember_hidden_secret("hunter2!")
    assert r.text(text) == want


def test_a_sealed_value_with_spaces_at_its_edges_keeps_matching() -> None:
    assert sealed_spans("prefix a suffix", " a ") == [(6, 9)]


def test_a_card_whose_ids_hold_a_short_sealed_value_is_stored(tmp_path: Path) -> None:
    from ferb.store.db import Database
    db = Database(tmp_path / "m.db")
    db.migrate()
    r = Redactor()
    r.remember_hidden_secret("8e95")
    r.remember_hidden_secret("pizza")
    store = Learning(db, Clock(), lambda *args: None, r.text, redact_id=r.ident)
    store.secret_values = r.secret_values
    card = store.put("episode", "t1", {"task_id": "t1", "intent": "order food", "status": "done",
                                       "actions": [{"id": "8e95" * 8, "tool": "browser.act", "args": {}}],
                                       "narrative": "I like pizza and pizzazz"},
                     [], origin_label="ferb", made_by="code:evidence-card/4", state="active")
    assert card["body"]["actions"][0]["id"] == "8e95" * 8
    assert card["body"]["narrative"] == "I like [redacted] and pizzazz"
    db.close()


def test_ordinary_words_come_from_the_system_word_list(tmp_path: Path) -> None:
    words = tmp_path / "words"
    words.write_text("Pizza\nopen\n")
    assert ordinary_word("pizza", words) and ordinary_word(" Open ", words)
    assert not ordinary_word("pizza2", words) and not ordinary_word("new delhi", words)
    assert not ordinary_word("pizza", tmp_path / "missing")


def test_saving_an_ordinary_word_answer_is_said_once_at_the_end_of_the_answer(daemon: Any,
                                                                             monkeypatch: pytest.MonkeyPatch,
                                                                             tmp_path: Path) -> None:
    words = tmp_path / "words"
    words.write_text("pizza\n")
    monkeypatch.setattr("ferb.sanitize.dictionary.SYSTEM_WORDS", words)
    monkeypatch.setattr("ferb.identity.owner_secrets.OwnerSecrets.remember", lambda self, name, target, value: None)
    ctx = ToolContext("t-food", "w1", "r-food", Identity.OWNER, frozenset({"browser:act"}), "dev", None, "/tmp", (),
                      ())
    waiting = asyncio.run_coroutine_threadsafe(daemon.daemon.gateway.ask_secret(
        ctx, "https://accounts.example", remember="favorite food"), daemon.loop)
    deadline = time.monotonic() + 5
    while not (asked := [ap for ap in daemon.daemon.state.approvals.pending() if ap.kind == "owner_secret"]):
        assert time.monotonic() < deadline
        time.sleep(0.01)
    run(daemon, lambda: daemon.daemon._resolve_approval(asked[0].id, True, None, "pizza"))
    assert waiting.result(5) == "pizza"
    [entry] = run(daemon, lambda: daemon.daemon._journal().of_kind("secret.ordinary_word"))
    assert entry.data == {"task_id": "t-food", "place": "accounts.example"}
    assert "pizza" not in json.dumps(entry.data)
    assert run(daemon, lambda: daemon.daemon.supervisor._secret_lines("t-food")) == \
        [ORDINARY_WORD_LINE.format(place="accounts.example")]
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest -q tests/test_sealed_words.py`
Expected: FAIL at import (`cannot import name 'ORDINARY_WORD_LINE'` / `ferb.sanitize.dictionary` missing).

- [ ] **Step 3: Matching (`ferb/sanitize/redact.py`)**

Add `import unicodedata` and `Iterable` to the imports. After `ID_BODY`, add:

```python
# One of FERB's own ids as text shows it (L36 §7): an optional lowercase prefix ("m-", "key-", "chat-") and its
# generated body. A sealed value is never replaced inside one: ids keep their form, and a collision there can't be
# mistaken for the secret.
FERB_ID = re.compile(r"(?<![0-9A-Za-z])(?:[a-z]+-)?(?:[0-9a-f]{64}|[0-9a-f]{32}|[0-9a-f]{12})(?![0-9A-Za-z])")


def _letter(ch: str) -> bool:
    cat = unicodedata.category(ch)
    return cat[0] in "LNM" or cat == "Co"


def _word_like(value: str) -> bool:
    """A sealed value made only of letters, marks and spaces: a word-like answer (a city, a food)."""
    return any(not c.isspace() for c in value) and all(c.isspace() or unicodedata.category(c)[0] in "LM"
                                                       for c in value)


def sealed_spans(s: str, value: str) -> list[tuple[int, int]]:
    """Where a sealed value stands in `s` (L36 §7): never inside one of FERB's ids (`FERB_ID`); a word-like value only as
    whole words (a boundary needed only where the value's own edge is a letter), so "pen" is hidden in "a pen" but not in
    "open"; any other value (a password, a PIN) wherever else it appears, since glued to other text it is still the
    secret (rule 14)."""
    if not value:
        return []
    ids = [m.span() for m in FERB_ID.finditer(s)]
    whole = _word_like(value)
    out, start = [], s.find(value)
    while start != -1:
        end = start + len(value)
        left = start == 0 or not _letter(value[0]) or not _letter(s[start - 1])
        right = end == len(s) or not _letter(value[-1]) or not _letter(s[end])
        if not any(a < end and start < b for a, b in ids) and (not whole or (left and right)):
            out.append((start, end))
        start = s.find(value, start + 1)
    return out


def holds_sealed(s: str, values: Iterable[str]) -> bool:
    """Whether `s` still shows one of the sealed `values` where the redactor would replace it."""
    return any(sealed_spans(s, v) for v in values)
```

`Redactor._registered`:

```python
    def _registered(self, s: str) -> str:
        """Every registered secret value replaced where it stands (`sealed_spans`), longest first."""
        for v in sorted(self.secret_values(), key=len, reverse=True):
            spans = sealed_spans(s, v)
            if spans:
                s = _mask_spans(s, spans)
        return s
```

`Redactor.ident`: replace its `bodies = …` / `hits` loop with
`hits = [span for value in self.secret_values() for span in sealed_spans(s, value)]` and keep its merging loop
(`out, kept = [], 0` … `return self._patterns("".join(out) + s[kept:])`) unchanged.

- [ ] **Step 4: The stored-card check (`ferb/memory/learning.py`)**

Import `holds_sealed` with the other redact imports; in `redacted_json` replace
`any(secret and secret in stored for secret in secrets)` with `holds_sealed(stored, secrets)`.

- [ ] **Step 5: The word list (`ferb/sanitize/dictionary.py`)**

```python
"""Owns one fact about a sealed value (L36 §7): whether it is a single ordinary word, from the macOS system word list
(/usr/share/dict/words), never a hand-made list. It only tells the owner why FERB hides that word everywhere; it
decides nothing about an action or an intent."""
from functools import lru_cache
from pathlib import Path

SYSTEM_WORDS = Path("/usr/share/dict/words")


@lru_cache(maxsize=4)
def _words(path: Path) -> frozenset[str]:
    try:
        return frozenset(w.strip().casefold() for w in path.read_text(errors="ignore").splitlines() if w.strip())
    except OSError:
        return frozenset()


def ordinary_word(value: str, words: Path | None = None) -> bool:
    """Whether `value` is one word of the system word list, case aside; False when the list can't be read."""
    word = value.strip()
    return bool(word) and word.isalpha() and word.casefold() in _words(words or SYSTEM_WORDS)
```

- [ ] **Step 6: Say it once (`ferb/daemon/gateway.py`, `ferb/orchestrator/supervisor.py`)**

In `Gateway.ask_secret`, replace `await asyncio.to_thread(store.remember, remember, target, ap.answer)` with:

```python
                await asyncio.to_thread(store.remember, remember, target, ap.answer)
                from ferb.sanitize.dictionary import ordinary_word
                if await asyncio.to_thread(ordinary_word, ap.answer):     # L36 §7: told once, never the word
                    root = (self.tasks.tree_of(ctx.task_id) or ctx.task_id).split(".", 1)[0]
                    self.journal_problem("secret.ordinary_word", {"task_id": root, "place": _site(target)})
```

(`journal_problem` is the daemon's plain journal append.) In `supervisor.py`, a constant near the other code lines:

```python
ORDINARY_WORD_LINE = "The answer you saved for {place} is an ordinary word, so FERB hides it wherever it appears."
```

and a method:

```python
    def _secret_lines(self, tree_id: str) -> list[str]:
        """Code's line for each place where this task saved a sealed answer that is an ordinary word (L36 §7), once
        each, from FERB's journal (never the word)."""
        places = dict.fromkeys(str(e.data.get("place", ""))
                               for e in self.journal.of_kind("secret.ordinary_word", "task_id", tree_id))
        return [ORDINARY_WORD_LINE.format(place=p) for p in places if p]
```

In `_final`, after the "Sites contacted" lines: `for line in self._secret_lines(tree.id): answer += "\n\n" + line`.

- [ ] **Step 7: Run the focused tests**

Run: `uv run pytest -q tests/test_sealed_words.py tests/test_redact.py tests/test_learning_alive.py tests/test_escape_hatch.py tests/test_redaction_scaling.py tests/test_secret_classes.py tests/test_labelled_secret_classes.py tests/test_question_clarity.py tests/test_episode_capture.py`
Expected: PASS.

- [ ] **Step 8: Docs**

`ARCHITECTURE.md` §16.12 bullet: "- **Sealed values (§7).** `redact.sealed_spans` decides where a registered value is
replaced: never inside one of FERB's ids (`FERB_ID`); a word-like value (letters, marks, spaces) only as whole words; any
other value wherever else it appears. `Redactor._registered`, `Redactor.ident` and `redacted_json`'s final check
(`holds_sealed`) use it. When a saved sealed answer is an ordinary word of `/usr/share/dict/words`
(`sanitize/dictionary.py`), the gateway journals `secret.ordinary_word` {task_id, place} and the task's answer ends
with one line per place." If §12 says registered values are replaced "as raw substrings", replace that clause with
"where `sealed_spans` finds them".

`README.md`: "A saved answer that is an ordinary word is hidden wherever that word appears; FERB tells you so once when
you save it. Your answers are never hidden inside FERB's own ids or inside longer words."

`CHANGELOG.md` bullet: "- Saved answers no longer damage other text (L36, rule 14). Live: a saved security answer that
is an ordinary word showed as [redacted] inside FERB's notes about you, the visa task's record was refused twice, and on
10-14 48 records failed because a short saved value matched inside FERB's ids. A word-like answer is now hidden only as
a whole word, nothing is hidden inside FERB's ids, a password is still hidden wherever it appears, and saving an answer
that is an ordinary word adds one line to the task's answer saying FERB hides that word everywhere."

Add `"tests/test_sealed_words.py"` to `[L36].tests`.

- [ ] **Step 9: Full checks, commit and push**

Run: `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`.

```bash
git add ferb/sanitize/redact.py ferb/memory/learning.py ferb/sanitize/dictionary.py ferb/daemon/gateway.py \
  ferb/orchestrator/supervisor.py tests/test_sealed_words.py docs/0.27_DECISIONS.toml ARCHITECTURE.md README.md \
  CHANGELOG.md
git commit -m "fix(L36, D12): sealed values are redacted as words or anywhere, never inside FERB's ids

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

### Task 10: Learning unblocked, use recorded, live checks (spec §8)

**Files:**
- Modify: `ferb/memory/learning.py` (`comparable`, `version_key`, `observed_ranges`, `version_bounds`, `in_range`,
  `Learning.home`, `propose_procedure`, `_refresh`, `procedure_parts`, `Learning.followed`)
- Modify: `ferb/daemon/main.py` (`_procedure_outcome`)
- Modify: `ferb/memory/intent.md`
- Modify: `ferb/cli/admin.py` (`stats`)
- Create: `tests/test_learning_unblocked.py`
- Modify: `tests/test_learning.py` (`test_verified_threshold_slots_and_current_version`), `tests/test_intent_labels.py`
- Docs: `ARCHITECTURE.md` (§14 F8 and §16.12), `README.md` (the "An app update sends a learned procedure back to
  candidate" sentence), `CHANGELOG.md`, `docs/OWNER_ACCEPTANCE.md`; add `"tests/test_learning_unblocked.py"` to
  `[L36].tests`

**Interfaces:**
- Consumes: `FERB_ID` (Task 9), `render_slots`, `Learning.receipt`.
- Produces: `comparable(value, home: str, key: str | None = None) -> Any`, `version_key(version: str) -> tuple`,
  `observed_ranges(cards) -> dict[str, list[str]]`, `version_bounds(value) -> tuple[str, str] | None`,
  `in_range(version: str, bounds: tuple[str, str]) -> bool`, `Learning.home: str`,
  `Learning.followed(procedure_id: str, card: dict, slots: dict | None) -> bool`.

Ruling (handles): per-run handles are recognized by the shapes FERB's tools give them (`t39` tabs, `s1e8` elements,
`p-e954619d` shell processes, FERB ids), integers under `window_id`, `window` or `pid`, and paths under a temporary
folder; `~` and `~/` expand to the daemon user's home on both sides. Anything else must be equal, so a real mismatch
(another text, another timeout) is still refused.
Ruling (versions): out of range, a learned procedure stays learned and the worker's mode line says "New version: verify
each step" (this replaces "an app update sends a learned procedure back to candidate").
Ruling (receipts): a run's receipt is written only when it followed every required step (same tool; arguments equal
after `comparable`, with the planner's slot values filled; a step whose slots have no value matches by tool); otherwise
`memory.procedure_not_followed` is journaled.

- [ ] **Step 1: Write the failing tests** (`tests/test_learning_unblocked.py`)

```python
"""L36 §8: learning unblocked. The audit's refused consolidations (2026-10-24: 0 procedures, all 5 refused) as
fixtures: actions compared modulo per-run handles and expanded paths, app versions as observed ranges, and a receipt
when a run follows its procedure."""
import io
from pathlib import Path
from typing import Any

import pytest

from ferb.cli.admin import stats
from ferb.memory.learning import Learning, LearningRefused, comparable, observed_ranges
from ferb.store.db import Database
from tests.support.learning_fixtures import Clock

HOME = "/Users/owner"


@pytest.fixture
def store(tmp_path: Path) -> Any:
    db = Database(tmp_path / "ferb.db")
    db.migrate()
    learning = Learning(db, Clock(), lambda *args: None, lambda s: s)
    learning.home = HOME
    yield learning
    db.close()


def card(store: Learning, task_id: str, actions: list[dict[str, Any]], apps: dict[str, str] | None = None) -> Any:
    return store.put("episode", task_id, {"task_id": task_id, "verified": True, "request": "r", "intent": "i",
                                          "apps": apps or {}, "slots": {}, "status": "done", "ended_at": "2026-10-20",
                                          "actions": [a | {"outside_action": False} for a in actions]},
                     [], origin_label="ferb", made_by="code:evidence-card/4", state="active")


def propose(store: Learning, cards: list[Any], steps: list[dict[str, Any]], slots: dict[str, str],
            values: list[dict[str, Any]]) -> Any:
    return store.propose_procedure({"intent": "i", "slots": slots, "preconditions": {"apps": {}}, "ui_hints": "",
                                    "steps": steps,
                                    "examples": [{"episode": c["id"], "slots": v} for c, v in zip(cards, values)]},
                                   "b1", "model:test|prompt:1")


def test_handles_paths_and_temp_folders_compare_by_kind() -> None:
    assert comparable({"tab": "t39", "element": "s1e8", "id": "p-e954619d", "path": "~/a.txt", "window_id": 7},
                      HOME) == {"tab": "<tab>", "element": "<element>", "id": "<process>", "path": f"{HOME}/a.txt",
                                "window_id": "<handle>"}
    assert comparable("/private/var/folders/x/T/f.png", HOME) == "<temp>"
    assert comparable({"timeout_s": 70}, HOME) != comparable({"timeout_s": 120}, HOME)


def test_a_slot_written_with_a_tilde_reproduces_an_expanded_path(store: Learning) -> None:
    cards = [card(store, f"w{i}", [{"id": f"w{i}-a", "tool": "fs.write",
                                    "args": {"path": f"{HOME}/ferb-test/n{i}.txt", "content": f"text {i}",
                                             "overwrite": True}}]) for i in range(3)]
    p = propose(store, cards, [{"tool": "fs.write", "args": {"path": "{path}", "content": "{content}", "overwrite": True},
                                "expect": "the file holds the text", "evidence": [f"w{i}-a" for i in range(3)]}],
                {"path": "string", "content": "string"},
                [{"path": f"~/ferb-test/n{i}.txt", "content": f"text {i}"} for i in range(3)])
    assert p["state"] in ("candidate", "learned")


def test_per_run_process_ids_and_tabs_differ_but_a_real_mismatch_is_refused(store: Learning) -> None:
    def run_of(task_id: str, process: str, tab: str, timeout: int) -> Any:
        return card(store, task_id, [
            {"id": task_id + "-s", "tool": "shell.start", "args": {"command": "sleep 60", "cwd": f"{HOME}/.ferb/w"}},
            {"id": task_id + "-w", "tool": "shell.wait", "args": {"id": process, "since": 0, "timeout_s": timeout}},
            {"id": task_id + "-c", "tool": "browser.act", "args": {"tab": tab, "element": "s1e8", "action": "click"}}])
    steps = [{"tool": "shell.start", "args": {"command": "sleep 60", "cwd": f"{HOME}/.ferb/w"}, "expect": "started",
              "evidence": ["a-s", "b-s"]},
             {"tool": "shell.wait", "args": {"id": "p-e954619d", "since": 0, "timeout_s": 120}, "expect": "exited",
              "evidence": ["a-w", "b-w"]},
             {"tool": "browser.act", "args": {"tab": "t39", "element": "s1e8", "action": "click"}, "expect": "clicked",
              "evidence": ["a-c", "b-c"]}]
    good = [run_of("a", "p-e954619d", "t39", 120), run_of("b", "p-a46c91e9", "t41", 120)]
    assert propose(store, good, steps, {}, [{}, {}])["kind"] == "procedure"
    bad = [run_of("a2", "p-e954619d", "t39", 120), run_of("b2", "p-a46c91e9", "t41", 70)]
    for step, ref in zip(steps, ("s", "w", "c")):
        step["evidence"] = ["a2-" + ref, "b2-" + ref]
    with pytest.raises(LearningRefused, match="reproduce"):
        propose(store, bad, steps, {}, [{}, {}])


def learned_on_two_brave_versions(store: Learning) -> Any:
    versions = ["155.1.97.59", "151.1.93.136", "155.1.97.59"]
    cards = [card(store, f"k{i}", [{"id": f"k{i}-a", "tool": "browser.navigate", "args": {"url": f"https://k/{i}"}}],
                  apps={"com.brave.Browser": v}) for i, v in enumerate(versions)]
    return propose(store, cards, [{"tool": "browser.navigate", "args": {"url": "{url}"}, "expect": "the board",
                                   "evidence": [f"k{i}-a" for i in range(3)]}], {"url": "string"},
                   [{"url": f"https://k/{i}"} for i in range(3)])


def test_app_versions_are_an_observed_range(store: Learning) -> None:
    unknown = [{"body": {"apps": {"unresolved-browser": "unknown", "com.apple.Notes": "4.11"}}},
               {"body": {"apps": {"com.apple.Notes": "unknown"}}}]
    assert observed_ranges(unknown) == {"com.apple.Notes": ["4.11", "4.11"]}      # what FERB couldn't capture adds nothing
    p = learned_on_two_brave_versions(store)
    assert p["body"]["preconditions"]["apps"] == {"com.brave.Browser": ["151.1.93.136", "155.1.97.59"]}
    assert store.procedure(p["id"], {"com.brave.Browser": "160.0.1.1"})["state"] == "learned"
    newer = store.procedure_parts(p["id"], {"com.brave.Browser": "160.0.1.1"}, None)[0].text
    assert "New version: verify each step" in newer and "151.1.93.136–155.1.97.59" in newer
    within = store.procedure_parts(p["id"], {"com.brave.Browser": "153.0.0.1"}, None)[0].text
    assert "Eligible for learned fast path" in within


def test_a_run_that_follows_its_procedure_gets_a_receipt_and_one_that_does_not_is_noted(daemon: Any) -> None:
    from tests.support.memory_items import run
    d = daemon.daemon
    d.learning.home = HOME
    p = run(daemon, lambda: learned_on_two_brave_versions(d.learning))
    run(daemon, lambda: card(d.learning, "followed", [{"id": "f-a", "tool": "browser.navigate",
                                                        "args": {"url": "https://k/9"}}]))
    run(daemon, lambda: card(d.learning, "ignored", [{"id": "g-a", "tool": "browser.act", "args": {}}]))
    run(daemon, lambda: d._procedure_outcome(p["id"], "followed", {"url": "https://k/9"}))
    run(daemon, lambda: d._procedure_outcome(p["id"], "ignored", {"url": "https://k/9"}))
    episodes = run(daemon, lambda: {r[0] for r in d.db.conn.execute(
        "SELECT n.key FROM learning_receipts r JOIN learning_nodes n ON n.id = r.episode_id")})
    assert "followed" in episodes and "ignored" not in episodes
    assert run(daemon, lambda: [e.data["task_id"] for e in d._journal().of_kind("memory.procedure_not_followed")]) == \
        ["ignored"]


def test_stats_show_procedures_learned_used_and_refused(store: Learning, tmp_path: Path) -> None:
    p = learned_on_two_brave_versions(store)
    run_card = card(store, "used", [{"id": "u-a", "tool": "browser.navigate", "args": {"url": "https://k/7"}}])
    store.receipt(p["id"], run_card["id"], {"url": "https://k/7"})
    store.db.conn.execute("INSERT INTO learning_jobs (source_ref, status, at, error, inputs) VALUES (?,?,?,?,?)",
                          ("intent:i-x", "failed", "2026-10-21", "every procedure proposed was refused: x", "g"))
    out = io.StringIO()
    assert stats(tmp_path, out, days=36500) == 0
    assert ("Procedures: 1 stored, 1 learned; used in 1 run(s); 1 kind(s) of task whose every proposed procedure was "
            "refused") in out.getvalue()
```

Append to `tests/test_intent_labels.py`:

```python
def test_the_labeler_is_told_to_name_the_purpose_so_repeats_group() -> None:
    """L36 §8: "keep media playing" ran as three per-site intents (3 cards, 0 verified each)."""
    from pathlib import Path
    text = " ".join((Path(__file__).resolve().parents[1] / "ferb" / "memory" / "intent.md").read_text().split())
    assert ('Name the purpose the owner had, not the site or app this run used ("keep a background tab\'s media '
            'playing", not "keep Instagram playing in Brave"), so runs with the same purpose on different sites group '
            'together.') in text
```

In `tests/test_learning.py`, replace `test_verified_threshold_slots_and_current_version` with:

```python
def test_verified_threshold_slots_and_version_range(learning):
    a, b, c = [card(learning, str(i), slot=s) for i, s in enumerate(('a', 'b', 'a'))]
    p = procedure(learning, [a, b, c])
    assert p['body']['preconditions']['apps'] == {'com.apple.mail': ['1', '1']}
    assert learning.procedure(p['id'], {'com.apple.mail': '1'})['state'] == 'learned'
    # L36 §8: an update keeps it learned; the worker is told to verify each step on the new version
    assert learning.procedure(p['id'], {'com.apple.mail': '2'})['state'] == 'learned'
    assert 'New version: verify each step' in learning.procedure_parts(p['id'], {'com.apple.mail': '2'}, None)[0].text
```

- [ ] **Step 2: Run them to verify they fail**

Run: `uv run pytest -q tests/test_learning_unblocked.py tests/test_learning.py tests/test_intent_labels.py`
Expected: FAIL at import (`cannot import name 'comparable'`).

- [ ] **Step 3: Comparison helpers (`ferb/memory/learning.py`)**

Add `import re` and `from pathlib import Path`; import `FERB_ID` from `ferb.sanitize.redact`. After `render_slots`, add:

```python
# The per-run handles FERB's own tools hand out, by their shapes (L36 §8): a tab ("t39"), a snapshot element ("s1e8"),
# a shell process ("p-e954619d"); one of FERB's ids (`FERB_ID`). A window or process number under one of these keys is
# one too. None of them means the same thing in another run.
_HANDLES = ((re.compile(r"t\d+"), "<tab>"), (re.compile(r"s\d+e\d+"), "<element>"),
            (re.compile(r"p-[0-9a-f]{8}"), "<process>"))
_NUMBER_HANDLES = frozenset({"window_id", "window", "pid"})
_TEMP_ROOTS = ("/tmp/", "/private/tmp/", "/var/folders/", "/private/var/folders/")


def comparable(value: Any, home: str, key: str | None = None) -> Any:
    """An action argument as consolidation compares it with a procedure step's (L36 §8): each per-run handle reads as its
    kind, `~` and `~/` expand to `home`, and a path under a temporary folder reads as "<temp>". Everything else is
    compared as it is, so a real difference (another text, another timeout) still differs."""
    if isinstance(value, dict):
        return {k: comparable(v, home, str(k)) for k, v in value.items()}
    if isinstance(value, list):
        return [comparable(v, home, key) for v in value]
    if isinstance(value, int) and not isinstance(value, bool) and key in _NUMBER_HANDLES:
        return "<handle>"
    if not isinstance(value, str):
        return value
    for shape, kind in _HANDLES:
        if shape.fullmatch(value):
            return kind
    if FERB_ID.fullmatch(value):
        return "<id>"
    path = home + value[1:] if value == "~" or value.startswith("~/") else value
    return "<temp>" if path.startswith(_TEMP_ROOTS) else path


def version_key(version: str) -> tuple[tuple[int, Any], ...]:
    """A version as ranges order it: its parts, numbers by value ("151.1.93.136" before "155.1.97.59")."""
    return tuple((0, int(p)) if p.isdigit() else (1, p) for p in re.split(r"[.\-+_]", version) if p)


def observed_ranges(cards: Iterable[dict[str, Any]]) -> dict[str, list[str]]:
    """The app versions a procedure's examples ran on, [lowest, highest] per app (L36 §8). A version FERB couldn't
    capture ("unknown", an "unresolved-…" app) adds nothing."""
    out: dict[str, list[str]] = {}
    for c in cards:
        for app, version in (c['body'].get('apps') or {}).items():
            if app.startswith('unresolved-') or not isinstance(version, str) or version in ('', 'unknown'):
                continue
            low, high = out.get(app, [version, version])
            out[app] = [min(low, version, key=version_key), max(high, version, key=version_key)]
    return out


def version_bounds(value: Any) -> tuple[str, str] | None:
    """A stored app precondition as (lowest, highest): a range, or one version (a procedure stored before L36)."""
    if isinstance(value, str) and value:
        return value, value
    if isinstance(value, list) and len(value) == 2 and all(isinstance(v, str) for v in value):
        return value[0], value[1]
    return None


def in_range(version: str, bounds: tuple[str, str]) -> bool:
    return version_key(bounds[0]) <= version_key(version) <= version_key(bounds[1])
```

In `Learning.__init__`: `self.home = str(Path.home())   # where a step's "~/" points (L36 §8); tests set it`.

- [ ] **Step 4: `propose_procedure`, `_refresh`, `procedure_parts`, `followed`**

`propose_procedure`: in the examples loop, delete the `expected_apps = …` assignment and the
`if any(expected_apps.get(k) != v …): raise LearningRefused('preconditions must preserve captured app versions')` after
it. In the step loop, compare modulo handles:

```python
            for ex, card in needed:
                actual = {a['id']: a for a in card['body'].get('actions', [])}
                expected = comparable(render_slots(step['args'], ex['slots']), self.home)
                if not any(ref in actual and comparable(actual[ref].get('args'), self.home) == expected for ref in refs):
                    raise LearningRefused('template arguments do not reproduce a cited action for each example')
```

and where `proposal` is rebuilt before `put`, store the observed ranges (Ruling: code's facts, never the model's
version text):

```python
        proposal = proposal | {'steps': [s | {'optional': s.get('optional', False)} for s in steps],
                               'preconditions': dict(proposal.get('preconditions') or {})
                               | {'apps': observed_ranges(cards)}}
```

`_refresh`: delete `expected = …` and `current = …`, and compute

```python
        # L36 §8: versions are a range shown to the worker (`procedure_parts`), never a requirement; learned needs a
        # latest run whose versions FERB captured
        matches = (latest is not None and bool(latest['body'].get('app_versions_captured', True))
                   and 'unknown' not in latest_apps.values())
```

`procedure_parts`: move `body, filled = p['body'], slots or {}` above `mode`, and after `mode` is chosen add:

```python
        newer = [(app, apps[app], bounds) for app, value in sorted((body.get('preconditions') or {}).get('apps', {}).items())
                 if (bounds := version_bounds(value)) and app in apps and not in_range(apps[app], bounds)]
        if p['state'] == 'learned' and newer:
            app, now_version, (low, high) = newer[0]
            mode = (f"New version: verify each step. {app} is {now_version} now; the runs this was learned from used "
                    f"{low if low == high else low + '–' + high}. Follow these steps, check each step's result before "
                    "the next and the final state; on any mismatch use full reasoning.")
```

Add to `Learning`:

```python
    def followed(self, procedure_id: str, card: dict[str, Any], slots: dict[str, Any] | None) -> bool:
        """Whether a run used its procedure (L36 §8): each required step has a successful action of the same tool whose
        arguments, with the planner's slot values filled in, equal the step's after `comparable`; a step whose slots
        this run has no value for matches by its tool alone."""
        p = self.get(procedure_id)
        if p is None or p['kind'] != 'procedure':
            return False
        actions = [a for a in card['body'].get('actions') or [] if isinstance(a, dict) and not a.get('error')]
        declared = p['body'].get('slots') or {}
        for step in p['body'].get('steps') or []:
            if step.get('optional'):
                continue
            args = render_slots(step.get('args') or {}, slots or {})
            unfilled = any('{' + k + '}' in encoded(args) for k in declared)
            if not any(a.get('tool') == step.get('tool') and
                       (unfilled or comparable(a.get('args'), self.home) == comparable(args, self.home))
                       for a in actions):
                return False
        return True
```

- [ ] **Step 5: Receipts on use (`ferb/daemon/main.py`)**

In `_procedure_outcome`, inside `if node:` before the receipt:

```python
            if not self.learning.followed(procedure_id, node, slots):      # L36 §8: use is following its steps
                self._journal().append("memory.procedure_not_followed", {"procedure_id": procedure_id,
                                                                         "task_id": task_id})
                return
```

- [ ] **Step 6: Grouping (`ferb/memory/intent.md`)**

After the paragraph that ends "decide from what the owner wanted done.", add:

```markdown
Name the purpose the owner had, not the site or app this run used ("keep a background tab's media playing", not "keep
Instagram playing in Brave"), so runs with the same purpose on different sites group together.
```

- [ ] **Step 7: Stats (`ferb/cli/admin.py`)**

In `stats`, after the learning-calls block (before `_resources(...)`):

```python
        if "learning_nodes" in tables:            # L36 §8: what learning produced and whether it was used
            procs = c.execute("SELECT state, body_json FROM learning_nodes WHERE kind = 'procedure' AND valid_to IS "
                              "NULL AND state NOT IN ('retired', 'rolled_back')").fetchall()
            examples = {e.get("episode") for _, body in procs for e in json.loads(body).get("examples") or []
                        if isinstance(e, dict)}
            used = sum(1 for (ep,) in c.execute("SELECT DISTINCT episode_id FROM learning_receipts") if ep not in examples)
            refused = c.execute("SELECT COUNT(*) FROM learning_jobs WHERE source_ref LIKE 'intent:%' AND error LIKE "
                                "'every procedure proposed was refused%'").fetchone()[0]
            print(f"Procedures: {len(procs)} stored, {sum(1 for s, _ in procs if s == 'learned')} learned; used in "
                  f"{used} run(s); {refused} kind(s) of task whose every proposed procedure was refused", file=out)
```

- [ ] **Step 8: Run the focused tests**

Run: `uv run pytest -q tests/test_learning_unblocked.py tests/test_learning.py tests/test_intent_labels.py tests/test_learning_alive.py tests/test_learning_by_intent_e2e.py tests/test_consolidation_cost.py tests/test_learning_calls_visible.py tests/test_planner_context.py`
Expected: PASS. A failure in an older test that pinned exact-argument equality or "an update sends it back to candidate"
is updated to the ruling above and listed in the commit body; one that pins quarantine, provenance or forgetting is a
bug in this task.

- [ ] **Step 9: Docs and live checks**

`ARCHITECTURE.md` §14 "F8 derived memory and learning": add "Since L36, a procedure step reproduces a cited action when
their arguments are equal after `comparable` (per-run handles by kind, `~` expanded, temporary paths as `<temp>`); app
versions are stored as the observed range per app (`observed_ranges`) and a run on a version outside it gets "New
version: verify each step"; a run's receipt is written only when it followed the required steps (`followed`)."

`ARCHITECTURE.md` §16.12 bullet: "- **Learning (§8).** Comparisons modulo per-run handles; version ranges; the intent
labeler names a task's purpose; receipts on use; `ferb stats` prints "Procedures: N stored, N learned; used in N
run(s); N kind(s) of task whose every proposed procedure was refused"."

`README.md`: replace "An app update sends a learned procedure back to candidate until it succeeds on the new version;"
with "A learned procedure remembers the app versions it was learned on; on a newer version it still runs, checking
each step;".

`CHANGELOG.md` bullet: "- FERB learns from repeated tasks (L36). Live: 0 procedures in two weeks; all 5 attempts were
refused because tab, element and process ids differ every run, `~/` paths didn't match their expanded form, and Brave
had updated between runs (151 vs 155). Steps are now compared without those per-run ids and with paths expanded,
versions are a range (a newer one runs with each step checked), runs of the same purpose on different sites group
together, a run that follows a procedure is counted, and `ferb stats` shows procedures stored, learned, used and refused."

Append to `docs/OWNER_ACCEPTANCE.md`:

```markdown
## Memory and learning that work (L36) — pending live checks (2026-10-24)

Checkout validation is in the changelog entry; nothing here is installed or observed yet. Run after
`ferb setup --update`, with the owner watching; pause between supervised actions and record what the owner saw.

- Tell FERB a fact while it works on something else ("the username for usvisascheduling.com is …"), then start an
  unrelated multi-step task that logs in there: `ferb trace TASK --full` shows the login step's "Parent task goal" and
  a "retrieved memory" block with the username, and the step types it (not the email).
- Log in to the same site twice: the second login calls `secrets.list`, asks for nothing already saved, and the
  hidden-input line names "<username> at <site>".
- A guess about you (say something FERB has to reword): a "Keep this?" question with Keep, Edit and Drop; Edit asks for
  your words; Drop removes it for good.
- Open the notch once after the update: "Review N older guesses" appears once in today's chat.
- A fact with a date ("I'm in Pune until Sunday"): the next Monday, `ferb memory` shows it ended and the planner no
  longer gets it.
- Saving a security answer that is an ordinary word: the task's answer ends with one line saying FERB hides it
  everywhere; that word elsewhere in a task record shows [redacted], and ids and longer words keep their form.
- `ferb stats`: the Procedures line, before and after a repeated task runs three verified times.
```

Add `"tests/test_learning_unblocked.py"` to `[L36].tests`.

- [ ] **Step 10: Full checks, commit and push**

Run: `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`, and `cd ui/mac/FERBNotch && swift test`.

```bash
git add ferb/memory/learning.py ferb/daemon/main.py ferb/memory/intent.md ferb/cli/admin.py \
  tests/test_learning_unblocked.py tests/test_learning.py tests/test_intent_labels.py docs/0.27_DECISIONS.toml \
  ARCHITECTURE.md README.md CHANGELOG.md docs/OWNER_ACCEPTANCE.md
git commit -m "feat(L36): procedures learn modulo per-run handles with version ranges; use is recorded and shown

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push origin main
```

---

## Self-review notes

- Spec coverage: §1 done (1c22dcb, f747d37); §2 Tasks 1–2 (quote, check against typed words, owner_stated, inferred asked
  at once, Keep/Edit/Drop, never expiring, supersession through versioning, no keyword list); §3 Tasks 3–4 (one function,
  content index, df filter, bm25 + recency + embedding, 60/20, owner first, episodes with goal/outcome/date, both tools);
  §4 Task 5 (recall wired via `memory_push`, query from request + goals + targets, steps not blanked, parent goal,
  `task_memory_refs`); §5 Tasks 6–7 (record, Keychain service unchanged, replace on same name, code-derived place,
  username via `login_origin` from owner_stated items and named sites, shown in the question, plain in memory,
  `secrets.list`, migration with the app-bound password unusable); §6 Task 8 (supersession, `until`, distill retire);
  §7 Task 9 (boundaries, ids, redaction kept, ordinary word from the system list); §8 Task 10 (normalization, `~/`,
  slots, version ranges, grouping prompt, receipts, stats); Testing section: each named test is in its task; live
  checks in Task 10.
- Spec gaps (decided above, for the owner): "no longer all protected" would let the conversation push memory out of
  the planner's window (Ruling 9); whole-token matching for every sealed value would leak a glued password (Ruling 20);
  "Drop forgets it" through `forget` would purge the owner's other versions and messages (Ruling 4); a login entry's
  `content` must be the username alone (Ruling 14).
- The audit's 12:17 and 12:22 messages share no word with the username fact. Fixed generally, not accepted: the planner
  recalls for its message plus the goals and targets of the tasks its layers show (Ruling 12a, Task 3), a worker or step
  for its request, goals and the task's targets (Ruling 12, Task 5), and a fact is indexed with its key and subject
  slug words (Ruling 7). Tested with the audit's real texts: both messages with the running visa task recall the
  username within the planner's 60, the research_official step within its 20 (Task 3), a step whose goals don't name
  the site through the task's targets (Task 5), and on the login page `secrets.list` gives it whatever the ranking
  (Task 6, worker.md pinned).
