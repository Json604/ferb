# Memory: decisions and fix list

Area: `ferb/memory/`, `ferb/sense/`, `ferb/decide/`, `ferb/orchestrator/attention.py`, and their wiring in
`ferb/daemon/main.py` and `ferb/orchestrator/supervisor.py`.
Basis: a read of every line of that code (2026-10-29/30), plus read-only counts from the live DB. File:line refs are
to commit c6621c3.

## Owner decisions (2026-10-30)

1. **Learning becomes the hybrid.** Keep the owner fact store; replace procedure learning with a short "how I did
   it" note on each verified episode, retrieved by search for similar requests.
2. **Forget is simple.** Delete the fact, its versions and its watcher-captured sources (the owner's chat message stays), and block it from being re-learned. No
   scrubbing of FERB's past answers or traces.
3. **Accessibility events stay recorded, pull-only.** FERB knows the data exists and reads it when it needs to
   (`trail.query`); it is not pushed into prompts. These rows are kept **7 days**.
4. **Jev, decide, perception batches and attention: code stays, switched off.** While paused, nothing in them runs.
5. **The owner's pass verdict counts as verified** for memory, as the checker's pass does; the owner's fail already
   overrides. (Verdicts come from the FERB window's task history, `verdict.set`, and `ferb verdict`.)
6. **Remember keeps today's behavior** (owner, 2026-10-30): the planner's wording is saved as the owner's own words
   when it appears word for word in what they typed; otherwise it is saved as FERB's wording and waits for Keep/Edit/
   Drop. (Saving the raw line every time was considered and rejected.)
7. **Memory is built for years of use, not today's size** (owner, 2026-10-30): three tiers (below). The always-on part
   has a hard budget; everything else is retrieved within limits, so prompts don't grow with memory.
8. **Retrieval by meaning plus keywords**: a local embedding model on the Mac fused with keyword search (bm25).
9. **The core profile is maintained by code and a model together** (owner confirmed). A change
   to a fact in the profile updates it at once, by code, no model; a weekly model pass (or sooner when the budget is
   exceeded) compacts it: it chooses and groups entries, code renders them from the facts, so the owner's words are
   never paraphrased.
10. **The planner stays tool-less; workers do the lookups** (owner, 2026-10-30, reversing an earlier yes). The planner
    is one fast call that changes nothing until it returns; lookups would add model round trips that each re-send
    the whole context, need the worker tool path (MCP shim, run token) in supervisor mode, complicate reply
    streaming, and hold the chat longer. Pushed context (Tier 1 + retrieval by meaning) covers the planner; a question
    that needs more becomes a task, whose worker has `memory.read`, `chat.history`, `task.evidence`, `trail.query`.
    Revisit if exposure records show planner memory misses.

## What exists today (verified)

| Part | Live data | State |
|---|---|---|
| Facts (`memory_items`) | 46 items: 26 owner-stated, 18 guesses, 2 observed | Works. Copied into `learning_nodes` on every search (`sync_facts`, learning.py:1851). |
| Learning (cards → intents → procedures) | 144 cards, 90 intents, **0 procedures, 0 lessons** | All 5 eligible intents refused by the exact-args check (learning.py:1180-1188). 15 cards parked by redaction idempotence (learning.py:178). |
| Daily distill | 30 days distilled | Works; one call a day. |
| Accessibility trail | 701,679 rows, 30–81k/day, 92% of the trail | No values; actor never `owner` (701,501 unknown, 178 ferb). Floods the activity search. |
| Jev batcher (paused) | ~10.6k batches, ~10.5k failed decisions, ~86k receipts per day | Still runs: gated only on `jev() is not None` (batches.py:307). |
| Forget / echo scrub | `forgotten_refs` = 0 | Never completed live. ~1,500 lines. |

## Target design

1. **Three tiers.**
   - **Tier 1 — core profile, always on in every role's prompt (planner and workers), hard budget (~2–3k
     tokens).** Who the owner is, standing preferences,
     which account or profile goes with what, current projects. Each entry points at its fact; the owner's stated
     preferences appear word for word. Code updates an entry the moment its fact changes; a weekly model pass (or
     sooner when over budget) chooses, groups and compacts within the budget, moving ended or situational items to
     Tier 2. Readable and editable as markdown. Replaces today's "Layer 1 identity" rule (only owner-stated owner
     preferences, learning.py:1891).
   - **Tier 2 — fact store, everything, versioned.** `memory_items` stays the source of truth, with source and
     validity window. A new statement replaces the old version and FERB's reply names what it replaced; when saving,
     the planner sees similar existing facts and names any a new fact replaces (code checks they exist). Facts tied
     to a period carry an end date or review date; the daily distill proposes retiring stale ones (`ask_retire`).
     Retrieved every turn by meaning + keywords within a limit. No fact/identity copies in `learning_nodes`, no
     `sync_facts`.
   - **Tier 3 — episodes and playbooks.** Each verified task keeps a `how` note (worker report field, saved on the
     episode/card, in its search and shown text). Similar requests retrieve similar verified episodes. When several
     verified tasks are the same kind of work, a model merges their notes into one playbook, rewritten as new runs
     succeed; owner-visible and editable. No exact-argument check.
2. **Retrieval:** local embedding model (`embeddings.py`, today dormant: needs the model bundled with the app) fused
   with bm25 keywords and recency; the same search serves facts and episodes; limits per role (planner, worker).
3. **Push to the planner, pull by workers:** the planner gets Tier 1 and retrieved memory; workers can also look memory
   up (`memory.read` and the other read tools). The planner prompt says a reply that depends on something not in its
   context becomes a task (decision 10).
4. **Visible:** every turn records which memories were used (`context.exposure`); a repeated owner correction ("I
   told you before") is traceable to a memory miss.
5. **Daily distill stays** (distill.py, summary.py, distill.md, calls.py); it also proposes Tier 1 changes.
6. **Forget:** delete the item, its versions, its `memory_sources` links and the trail rows it came from; write the
   fingerprint so it is never re-learned; close its questions; re-export. Synchronous and small.
7. **Perception:** all watchers keep recording. Accessibility goes only to `trail`, kept 7 days. The worker
   prompt says recorded activity, accessibility events included, can be read with `trail.query`; the planner
   (tool-less) is told such a question becomes a task.
8. **Jev off:** one switch, off by default. While off, no batcher, no decisions, no receipts, no shadow checks, no
   startup requests.

## Depends on
- notes/orchestration.md: the worker report's `how` note.
- notes/gateway.md: the redaction rebuild (parked cards); the `task_memory_refs` writer sits in the gateway
  (gateway.py:1971-1989) and goes with the pipeline rewrite.
- notes/daemon.md: packaging the local embedding model with the app.
- notes/storage.md: retention and the one-time cleanup of the removed learning tables.

## Fix list

### Facts
- [ ] Search `memory_items` directly; remove `sync_facts` and the `fact`/`identity` node mirroring
      (learning.py:1851-1893; main.py:1745, 1800, 1817). Identity from owner-stated owner preferences in `memory_items`.
- [ ] Wrong source after a clarifying question: `typed_lines` includes `session.pending` (supervisor.py:1302-1305)
      but the item's source is the current `entry.id`. Source the item to the message that holds the quote.
- [ ] Same-key silent replace: a new fact with the same kind/subject/key ends the old one and clears login names
      (items.py:477-486; main.py:1900-1902). At minimum the "Remembered:" line names what it replaced. Subject is
      never slugged (items.py:383): normalize it like the key.
- [ ] `memory.read` lists retracted and ended items (evidence.py:367-389): current items only, unless asked by id.
- [ ] Dashboard `op=retire` on a fact retires only the node (main.py:2365-2367): disappears with the mirroring;
      retire the item.
- [ ] Remove dead code: `ferb/memory/recall.py`, `Memory.recall`, `Recalled` (items.py:219-223, 315-333),
      `owner_span` (items.py:181-184), the supervisor `recall` hook path (supervisor.py:602-604, 1884-1887,
      2154-2164), the `forget(finish=True)` branch and `after_forget` hook (items.py:686-689, 272; main.py:1534).
- [ ] Prompt: supervisor.md:68 names a "What FERB remembers" label no code emits; use the real one.

### Tiers and retrieval
- [ ] Tier 1 core profile: storage, budget, code updates on fact change, weekly compaction pass, markdown view.
- [ ] Replacement on save: planner sees similar facts and names replaced ids; reply names what was replaced.
- [ ] Validity: end/review dates for period facts; distill proposes retirement.
- [ ] Local embedding model bundled; fused search (embeddings + bm25 + recency) over facts and episodes.
- [ ] Playbooks: merge verified how-notes of the same kind of work.
- [ ] Upgrade: build an initial Tier 1 from existing facts and initial embeddings on first start of the new
      version (notes/storage.md decision 5).
- [ ] Prompt text names the real memory blocks; only roles with tools are told to use lookup tools.

### Learning → how-notes
- [ ] Add `how` to the worker report schema, store it on the episode/card, index and show it (above).
- [ ] Owner's pass verdict counts as verified (learning.py:906 counts only the checker; owner, 2026-10-30).
- [ ] `memory.learned` puts `sources` before `text` (tools.py:44); a card with 58k refs fills the page. Drop
      sources from its output.
- [ ] Remove: `consolidate.py` (keep the ~35 lines indexing day narratives only if wanted), `intents.py`,
      `strict.py` (only intents/consolidate/procedure use it), `intent.md`, `consolidate.md` (keep `embeddings.py`:
      decision 8); the
      procedure/lesson/receipt code in learning.py (~630 lines: 528-662, 1128-1406, 1701-1849, `lesson_feedback`,
      `outside_*`); the rollups (`summary`/`entity` nodes); main.py procedure parts (`_replay_deferred_receipts`,
      `_procedure_problem`, `_procedure_outcome`, `_note_use`, the procedure half of `_refresh_learning_verdict`,
      procedure hooks ~1562-1571, `_learning_context`, the `consolidate` op); supervisor.py procedure parts
      (`_procedure_blocks` 3531-3554, `_ignore_procedure`, `_planner_slots`, `_slot_problem`, hooks 564-585,
      2255-2265); `procedure_id`/`procedure_slots` (schemas.py:78-79, 235-236); prompt text supervisor.md:243-247,
      worker.md:263-266.
- [ ] Hourly pass keeps only distill; no intent-label or consolidate calls.
- [ ] Parked cards: find why redaction isn't idempotent for 15 cards (learning.py:146-180), fix, re-capture.
- [ ] Card source fan-out: 1.5M `learning_sources` rows, up to 58,250 per card, from refs inside tool results
      (lineage.py:44-76). Keep only refs needed for origin labels.
- [ ] Whole-table reads with all refs (learning.py:694-706; main.py:1720 on every verdict): read only what's needed.

### Forget
- [ ] Replace `Memory.forget`/`plan_forget` with the simple version; remove `echoes.py`, `_Rescrub`/`_rescrubs`,
      `_forget_scrub`/`_after_forgets`/`_finish_forget`/`_forget_stating_cards` (main.py ~245-255, 1939-2073), the
      `task_memory_refs` writers (supervisor `_given_memory` 3608-3613; gateway.py:1971-1989 → main.py:886).
      Keep `spans.py` rewrite helpers and `swap` only if something else still uses them (the attention purge does).
- [ ] Stop the same-message cascade: forgetting one fact no longer deletes every fact sharing its source message.
- [ ] Chat "Forget for good?" text states exactly what will be deleted.
- [ ] A forget clears the fact from a pending clarifying question (`conversations.pending`).
- [ ] Forget keeps the owner's chat message that stated the fact (owner, 2026-10-30): chat history is the owner's
      own record. Watcher-captured sources (trail rows) are deleted.

### Perception
- [ ] Accessibility rows: out of `today.jsonl` and the per-turn activity search; `trail` only; purge after 7 days.
- [ ] Privacy: accessibility records browser window titles with no incognito or excluded-domain check; typed text
      and clipboard check only app exclusions. Apply the browser checks (as windows.py:3-5 does).
- [ ] Activity search indexes JSON keys as words (daily.py:180): index values only, prefer recent rows.
- [ ] `today.jsonl` stores each row's data twice (`data` and `where`, main.py:4521-4522).
- [ ] Redaction runs ~6 times per event (watcher, trail, four in `_publish` main.py:4517-4528): once.
- [ ] Software watcher re-reads TCC and all LaunchAgent plists every second (watchers/software.py:20-37): on change.
- [ ] `purge_text` rewrites and fsyncs `today.jsonl` once per expired 7-day-old row, which can never be in today's
      file (perception.py:152 → daily.py:117-122).
- [ ] Typed text dropped if focus moves within 0.5 s of the last key (capture.py:82-99).
- [ ] Prompts: one line in the worker prompt that recorded activity, accessibility events included, is available
      through `trail.query` (the planner is tool-less: decision 10).

### Jev off
- [ ] One switch, off by default; the batcher, decider, shadow checks (main.py:3749-3760) and attention check it.
      The batcher today ignores `refused` (decider.py:62) and re-redacts up to 32k tokens per batch before checking.

### Data already on disk (cleanup: notes/storage.md decision 4)
- Learning tables no longer used: `learning_jobs`, `learning_receipts`, `learning_digests`, rollup nodes (the embedding tables
  `learning_vectors`/`learning_embedding_evals` stay: decision 8); `task_memory_refs` (16,680 rows).
- Perception tables, never purged: `perception_batches`, `perception_receipts`, `perception_seen` (~130 MB);
  `decisions` (~20 MB).

## Known residuals (accepted or deferred)
- Accessibility rows hold no values and never name the owner as actor, so they answer "something changed in app Y
  at time T", not "what changed" or "who did it". Making them richer is a separate project.
- Text sent to a model provider can't be recalled by a local forget.

## Size
About 3,200 lines go (learning ~1,550, forget ~1,500, dead code ~200; embeddings stay); sense and decide stay. Daily row growth drops
by what the paused Jev batcher writes (~100k rows); accessibility stays, capped at 7 days.
