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
6. *Superseded by decision 15.* **Remember keeps today's behavior** (owner, 2026-10-30): the planner's wording is saved as the owner's own words
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

11. **Facts stated in passing are captured** (owner, 2026-10-30). Example: "what are some easy piano pieces? I'm a
    beginner" (10-30) stored nothing: the planner's `remember` was empty although supervisor.md:47 asks for durable
    statements about the owner, and the daily distill of 10-12 and 10-14 (which also mentioned piano) proposed 3 and 4
    items, none about it. The planner rule and the distill's chat pass both cover durable facts said in passing, not
    only "remember …"; paraphrased ones arrive as guesses with Keep/Edit/Drop (decision 6).

12. **Health and finance facts are stored like any other** (owner, 2026-10-30): no special exclusion in the planner or
    the distill (distill.md's exclusion goes); secrets are masked by redaction as always.
13. **A remembered quote carries a short label** (owner, 2026-10-30): code stores what the quote is about (from the
    task) next to the owner's exact words, so a line like "ok. score this 1 star - not ordering this again" still makes
    sense later; the words themselves are never reworded.
14. **Forgetting a person or site also removes it from known destinations** (owner, 2026-10-30), so sends to it ask
    again.

15. **Memory without Keep questions** (owner, 2026-10-30; replaces decision 6). FERB saves facts itself — what the
    owner states and what it works out — each labelled by where it came from (the owner's words / FERB's understanding
    / observed). No Keep/Edit/Drop questions: a quiet "Memory: noted …" line the owner can ignore or correct by talking
    ("that's wrong", "forget that"). The weekly profile pass promotes FERB's understanding into Tier 1 only once it
    holds up (seen again, used without correction). Memory never authorizes an action. A memory page in the app shows
    and edits everything. (Same model as ChatGPT's and Claude's memory: transparency and correction, not approval.)

16. **FERB works out who the owner is from their Mac and shows it** (owner, 2026-10-31): from installed apps and their
    usage FERB works out what the owner mostly does, which apps they use most, their main browser and the main
    profile/account in it; it presents that once as "here's what I saw: ..." (no yes/no loop, per decision 15), stores
    it as observed facts, and the weekly profile pass promotes what holds up into Tier 1. Refreshed as usage changes.
    Replaces today's persona step, whose confirmed dump was stored as one 1,275-char owner_stated item.

17. **Stale facts, split by source** (owner, 2026-10-31): FERB's own understanding and observed facts that look over
    are retired automatically and the owner is told once. The owner's own words are retired automatically only when
    their recorded end date passes; otherwise FERB tells the owner once "this looks over, I'll stop using it" and stops
    using it, kept as history and restored by saying it still holds.

18. **When things enter the always-on profile** (owner, 2026-10-31): the owner's own words join at once (same turn).
    FERB's understanding and observed facts join only once they hold up (seen on 2 separate days, or standing 7 days
    uncorrected); until then they are saved and retrieved whenever relevant. When one of them joins, FERB adds one
    quiet "Now always on: … (label)" note card with Correct / Forget — never a question. The profile stays within its
    fixed budget; the weekly pass only chooses and groups (code prints every entry word for word).
    Guarantee (test): the profile text, and so the cached prefix, is identical between turns unless a profile entry
    changed; saving a fact outside the profile changes nothing; the weekly pass is the only scheduled rewrite.

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
- [x] Decision 18: "Now always on: …" note card when FERB's understanding/observed joins the profile (9b), and the cache
      guarantee test (profile/prefix identical across turns unless an entry changed).
      (Step 9b: `CoreProfile.saved` takes the owner's words about the owner of any kind at once, and FERB's understanding
      or observed facts when a save shows them holding up (a second day); the pass's joins too; each join of the latter
      calls `on_joined` → `MemoryGlue._now_always_on`: one notice "Now always on: “…” (label)" carrying the 8e note card
      with `always_on: true` (notices now carry `cards`, journaled and published). An owner fact that doesn't fit records
      `memory.profile_waiting`, which makes the pass due early, at most once a day (`EARLY_PASS_HOURS`).
      tests/test_memory_recall.py: the join and its card, the cache guarantee, the burst.)
- [x] Decision 17: stale facts split by source (9b).
      (Step 9b: `Memory.looks_over`: FERB's understanding / observed → retired; the owner's words → set aside
      (`memory_set_aside`, migration 0054; `MemoryItem.set_aside`; out of `prompt_ready`, search and the profile, kept as
      history), told once ("… look over, so I'll stop using them. They're kept; tell me if any still holds"); restored
      when the owner says the same words again (`Memory.save` credits them and `restore`s) or by RPC `memory.restore`.
      The weekly pass's `ended` and the distill's own-check retire use it; a retire on the owner's own words and an end
      date they gave still end the item. tests/test_memory_recall.py, test_memory_retirement.py.)

### Facts
- [x] Search `memory_items` directly; remove `sync_facts` and the `fact`/`identity` node mirroring
      (learning.py:1851-1893; main.py:1745, 1800, 1817). Identity from owner-stated owner preferences in `memory_items`.
      (Step 9a: the identity half done: no identity nodes are written (`sync_facts`), the clean-up closed the old ones and
      Tier 1 replaces the Layer 1 identity; searching `memory_items` directly is step 9b.)
      (Step 9b: `MemorySearch.facts` (ferb/memory/search.py) ranks `memory_items` through `memory_fts` and the fact vectors;
      `sync_facts` and `_identity_twins` removed; open fact/identity nodes closed once (`upgrade.close_fact_nodes`,
      journaled `memory.fact_nodes_closed`; a forget still deletes an item's old copies, the step-12 clean-up the rest).)
- [x] Old chat searchable by meaning (owner, 2026-10-31): owner messages and FERB's short replies indexed in the same
      fused search as facts and episodes; the planner gets the few old exchanges matching the current message;
      `chat.history` gains search by meaning and by date and reads pages from the DB instead of loading the whole
      history (evidence.py:333 loads everything and matches literal text only). (Step 9.)
      (Step 9b: `chat_fts` (migration 0054) holds the owner's own words of each message and FERB's replies, answers,
      questions and notices, filled from the journal as it grows (`MemorySearch.index_chat`) and embedded in the
      background; the planner gets "Layer 4 — earlier conversation matching this message" (`MemoryGlue._old_chat`: each
      exchange the owner's words + FERB's short reply, from any owner chat, never one already in its conversation, never
      in a scheduled run); `chat.history` takes `query` (meaning and keywords), `since`/`until`, and pages from SQL
      (`Continuity.entries/entry/belongs/tasks`; `Continuity.history` removed).)
- [x] Wrong source after a clarifying question: `typed_lines` includes `session.pending` (supervisor.py:1302-1305)
      but the item's source is the current `entry.id`. Source the item to the message that holds the quote.
      (Step 8c: `Supervisor._quote_source`: this turn's messages first, then the earlier ones FERB's question continued.)
- [x] Same-key silent replace: a new fact with the same kind/subject/key ends the old one and clears login names
      (items.py:477-486; main.py:1900-1902). At minimum the "Remembered:" line names what it replaced. Subject is
      never slugged (items.py:383): normalize it like the key.
      (Step 9a: `Memory.save` returns what it replaced (`Saved`); the "Memory: noted …" line says "— replaces “…”";
      login names clear through `on_ended` as before; `subject_key` normalizes subjects, existing rows by the clean-up.)
- [x] `memory.read` lists retracted and ended items (evidence.py:367-389): current items only, unless asked by id.
      (Step 9a: `Memory.current_items`; a query keeps `prompt_ready` items; an id reads any version with its status.)
- [x] Dashboard `op=retire` on a fact retires only the node (main.py:2365-2367): disappears with the mirroring;
      retire the item. (Step 9a: `memory.learning` op retire on a fact node retires its memory item.)
- [x] Remove dead code: `ferb/memory/recall.py`, `Memory.recall`, `Recalled` (items.py:219-223, 315-333),
      `owner_span` (items.py:181-184), the supervisor `recall` hook path (supervisor.py:602-604, 1884-1887,
      2154-2164), the `forget(finish=True)` branch and `after_forget` hook (items.py:686-689, 272; main.py:1534).
      (Step 3: recall.py, `Memory.recall`, `Recalled`, `owner_span`, `forget(finish=)` and `after_forget` removed; the supervisor recall hook went in step 2.)
- [x] Prompt: supervisor.md:68 names a "What FERB remembers" label no code emits; use the real one.
      (Step 9a: the line names "Tier 1 — core profile" and "Layer 4 — retrieved memory (data)", the labels code emits.)
- [x] Decision 15, no Keep questions: FERB saves what it understands at once, labelled (`items.LABELS`: your words /
      FERB's understanding / observed), with a quiet "Memory: noted “…” (label)" line and its note card (`label`,
      `origin`, `replaces`); correction by talking (remember with `replaces`, or forget). Removed: `Memory.confirm`/`drop`,
      the guess settling, `_ask_keep`/`_settle_guess`/`_guess_state`/`_guess_backlog`, the Keep/Edit/Drop and Retire/Keep
      questions (`MEMORY_QUESTIONS` keeps only forget), the `memory.confirm` RPC and CLI `memory confirm`/`identity`.
      Old waiting guesses become FERB's understanding and their questions are dropped at start (upgrade).
      (Step 9a.)
- [x] Decision 12: health and finance facts stored like any other (distill.md's exclusion removed; no code exclusion
      existed; redaction still masks secrets). (Step 9a.)
- [x] Decision 13: a remembered quote carries a short label: `memory_context.about` (migration 0053), from the turn's
      task (`Supervisor._remember_about`: the new task's goal, else the controlled task's), else the item's key; shown in
      Tier 1, retrieved memory, memory.read and `memory.list`, never in the owner's words. (Step 9a.)
- [x] Decision 14: forgetting a person or site removes it from known destinations (`Memory._named_destinations`:
      stored values found as whole words in the item, its label or its subject's name; a site by host or name; a phone
      by digits), said in the "Forget for good?" list. (Step 9a.)
- [x] Decision 16: who the owner is, from their Mac (ferb/device/persona.py gathers; ferb/memory/mac_profile.py and
      mac_profile.md interpret and store as observed; ferb/daemon/persona.py shows it once). (Step 9a.)

### Tiers and retrieval
- [x] Clean memory before Tier 1 is seeded: merge the 4 duplicate same-key pairs (message_call_default_app,
      word_meaning_only, owner_built_ferb, communication_critical_not_yes_man); `setup_persona` (1,275-char app dump
      labelled owner_stated) becomes observed and stays out of Tier 1; the distill skips keys already stored whatever
      their origin; the remember rule resolves relative dates ("the 16th") when saving; one ask_retire pass for dated
      facts (e.g. wedding_flight_home) before seeding.
      (Step 9a: ferb/memory/upgrade.py `clean_before_tier1`, journaled `memory.cleanup` once; the distill refuses a
      stored key unless the owner's quote replaces it; the remember rule gives a dated fact `until` (code resolves the
      date); dated facts are judged by the first profile pass, which runs at once after the seed and retires what is
      over, told once.)
- [x] Decision-11 test scenarios from real chats: "I'm a beginner" (piano), "make a short summary list, I can't read
      this much jargon", "sounds too linkedinish", "my usual playbook of cold mailing", "kaggle on my brave kaggle
      personal account". Method hints the owner gives mid-task ("hard refresh usually fixes it", "try its a11y tree
      buttons", "search in the main WhatsApp search box and keep scrolling") go to Tier 3 how-notes with owner origin.
      (Step 9a: the fact scenarios are tests/test_memory_capture.py `PASSING` and the distill's chat pass; the method
      hints → how-notes are step 9b.)
      (Step 9b: an episode card carries `owner_hints`: the owner's corrections to the running task in their own words
      (`Learning._owner_hints` from `task.steered`), quoted in its how-note with their origin.)
- [x] "Gets better at repeated tasks" is measured: attempts and time on repeat runs of the same kind of work, and
      exposure records showing the how-note was retrieved; a live scenario runs the same task twice.
      (Step 9b: `playbooks.repeat_stats`: each playbook's runs in order with attempts (worker steps), seconds, verified
      and `used_memory` (its context exposures cite the playbook or an earlier run's card); RPC `memory.repeats`; `ferb
      stats` "Repeated work (playbooks)". The live scenario is step 13.)
- [x] How-notes built from the recorded tool calls plus the worker's text (not the worker's text alone).
      (Step 9b: `learning.how_note`: the worker's `how`, the owner's hints, and `recorded_steps` from the card's recorded
      calls (tool + what it acted on, repeats folded, failed calls counted, at most 12 steps, 600 chars); in the card's
      search text (SEARCH_TEXT_VERSION content/2, re-indexed at start) and its shown line when verified.)
- [x] Durable facts said in passing are captured (decision 11): planner `remember` rule and the distill's chat pass,
      with this case as a test scenario ("I'm a beginner" while asking about piano pieces).
      (Step 9a: supervisor.md and distill.md lines; tests/test_memory_capture.py.)
- [x] Tier 1 core profile: storage, budget, code updates on fact change, weekly compaction pass, markdown view.
      (Step 9a: ferb/memory/profile.py `CoreProfile`, `memory_profile` (migration 0053); `PROFILE_TOKENS` 2,500 enforced
      on every render; `follow`/`saved`/`remove` by code; `run_pass` weekly or when over budget (profile.md); in every
      role's instructions where "Layer 1 — identity" was (`Supervisor._system`); RPC `memory.profile` (get, add,
      remove, pass) and `memory.list` `label`/`about`/`in_profile`/`review_at`; the window's Memory page shows both.)
- [x] Replacement on save: planner sees similar facts and names replaced ids; reply names what was replaced.
      Live case (2026-10-31): `appointment_preferences` still says "notify me before booking" although the owner later
      said to just book (the visa schedule books immediately); the later instruction must replace the earlier one.
      (Step 9a: remember entries' `replaces` (memory ids, checked by `Memory._replaceable`); Tier 1 and retrieved facts
      show memory ids; tests/test_core_profile.py the booking case.)
- [x] Validity: end/review dates for period facts; distill proposes retirement.
      (Step 9a: `until` for dated facts, `review_in_days` → `memory_context.review_at`; the distill sees items "Due for a
      check" and retires what is over, told once (no Retire/Keep question); one not retired holds up, its date cleared.)
- [x] Local embedding model bundled; fused search (embeddings + bm25 + recency) over facts and episodes.
      (Step 9b: potion-base-8M read by ferb/memory/embeddings.py (Ruling below), fetched once into ~/.ferb/models or read
      from `[memory] embedding_model` (bundling into FERB.app is step 12); `recall_vectors` (migration 0054) for facts,
      learned nodes and chat; `MemorySearch` fuses bm25, focus, nearest and recency per kind; the MLX path, its
      evaluate/adopt flow and the `memory` extra removed.)
- [x] Playbooks: merge verified how-notes of the same kind of work.
      (Step 9b: ferb/memory/playbooks.py + playbook.md: code groups verified cards by goal similarity, one model call
      says same kind or not and writes the steps, code checks the ids; a `playbook` node (rewritten as new runs join;
      the owner's edit is never replaced; a declined group is not asked again), at most 3 merges an hourly pass.)
- [x] Upgrade: build an initial Tier 1 from existing facts and initial embeddings on first start of the new
      version (notes/storage.md decision 5).
      (Step 9a: Tier 1 done: `CoreProfile.seed` after the clean-up, journaled `memory.profile_seeded`, then the first
      pass; tests/test_memory_upgrade.py on a schema-52 store shaped like the owner's. Embeddings are step 9b.)
      (Step 9b: the background embedder embeds everything already stored at the first start after the model is there
      (`_embed_loop`), never in a turn.)
      (Step 12a: checked on a copy of the owner's schema-48 store: the first start applied 0049–0056, journaled
      `memory.cleanup`, `memory.profile_seeded` (46 Tier 1 items) and `memory.fact_nodes_closed` once each; a second start
      changes nothing (tests/test_upgrade.py, tests/test_memory_upgrade.py); `_embed_now` embeds what an older store holds
      (tests/test_memory_recall.py). The real model's first fill on the owner's store is a step-13 live check: the model
      isn't on the owner's Mac yet.)
- [x] Prompt text names the real memory blocks; only roles with tools are told to use lookup tools.
      (Step 9a: supervisor.md names the real blocks; `TOOLLESS_ROLES` (planner, question router, attention) get no
      skills/abilities catalog.)

### Learning → how-notes
- [x] Add `how` to the worker report schema, store it on the episode/card, index and show it (above).
      (Step 8b: in the report schema, on the task's outcome, the episode (`episodes.how`, migration 0050) and the
      card body; indexing and showing it are step 9.) (Step 9b: indexed and shown in the how-note.)
- [x] Owner's pass verdict counts as verified (learning.py:906 counts only the checker; owner, 2026-10-30).
      (Step 9a: the owner's latest verdict decides, else the checker's.)
- [x] `memory.learned` puts `sources` before `text` (tools.py:44); a card with 58k refs fills the page. Drop
      sources from its output. (Step 9b: id and text first, no source refs; facts are memory.read's.)
- [x] Remove: `consolidate.py` (keep the ~35 lines indexing day narratives only if wanted), `intents.py`,
      `strict.py` (only intents/consolidate/procedure use it), `intent.md`, `consolidate.md` (keep `embeddings.py`:
      decision 8); the
      procedure/lesson/receipt code in learning.py (~630 lines: 528-662, 1128-1406, 1701-1849, `lesson_feedback`,
      `outside_*`); the rollups (`summary`/`entity` nodes); main.py procedure parts (`_replay_deferred_receipts`,
      `_procedure_problem`, `_procedure_outcome`, `_note_use`, the procedure half of `_refresh_learning_verdict`,
      procedure hooks ~1562-1571, `_learning_context`, the `consolidate` op); supervisor.py procedure parts
      (`_procedure_blocks` 3531-3554, `_ignore_procedure`, `_planner_slots`, `_slot_problem`, hooks 564-585,
      2255-2265); `procedure_id`/`procedure_slots` (schemas.py:78-79, 235-236); prompt text supervisor.md:243-247,
      worker.md:263-266.
      (Step 3: consolidate.py, intents.py, strict.py, intent.md, consolidate.md and the procedure/lesson/receipt/outside_* code in learning.py removed, with the rollups (day/week/month/year `summary` and `entity` nodes); the day-summary indexing (`activity_summary`) kept as `learning.index_summaries`. Daemon and supervisor procedure parts, `procedure_id`/`procedure_slots` and the prompt text removed. Episode cards, card search and embeddings stay.)
- [x] Hourly pass keeps only distill; no intent-label or consolidate calls.
      (Step 3: the hourly pass is distill, the episode-card refresh (`_refresh_cards`: cards whose inputs changed, e.g. a verdict; then day-summary indexing and embeddings) and the embedding restore.)
- [x] Parked cards: find why redaction isn't idempotent for 15 cards (learning.py:146-180), fix, re-capture.
      (6a: masking removed context that had exempted another value, and one pass consumed text another rule needed;
      the rebuilt redactor masks spans found on the original text, then reaches a fixed point, and `redacted_json`
      redacts by structure and refuses only a known value or a secret format. A card parked with the old error
      ("redaction would still change it") is read again once (`PARKED_BY_REDACTION` in `cards_to_read`).)
- [x] Card source fan-out: 1.5M `learning_sources` rows, up to 58,250 per card, from refs inside tool results
      (lineage.py:44-76). Keep only refs needed for origin labels.
      (Step 9b: `_card_labels` computes the labels from every ref but the card keeps its episode, journal and memory
      refs and one ref per distinct origin set. The existing 1.5M rows stay for the step-12 clean-up (owner's OK).)
- [x] Whole-table reads with all refs (learning.py:694-706; main.py:1720 on every verdict): read only what's needed.
      (Step 9b: a card refresh reads the old card without refs (its refs only when its trace is gone); the memory page's
      inventory and `get` read nodes without refs (`source_count`), `get`'s history by kind and key.)

### Forget
- [x] Replace `Memory.forget`/`plan_forget` with the simple version; remove `echoes.py`, `_Rescrub`/`_rescrubs`,
      `_forget_scrub`/`_after_forgets`/`_finish_forget`/`_forget_stating_cards` (main.py ~245-255, 1939-2073), the
      `task_memory_refs` writers (supervisor `_given_memory` 3608-3613; gateway.py:1971-1989 → main.py:886).
      Keep `spans.py` rewrite helpers and `swap` only if something else still uses them (the attention purge does).
      (Step 3: echoes.py, `_Rescrub`, the rescrub/scrub queue and both `task_memory_refs` writers removed; `Memory.forget` deletes the versions, their source links and trail rows (fingerprint, summary rebuild, questions closed, export). `spans.py` keeps `rewrite_recorded`/`swap` (attention purge, `update_input`); `SpanWatch`/`rewrite_until_still` removed.)
- [x] Stop the same-message cascade: forgetting one fact no longer deletes every fact sharing its source message.
      (Step 3: plan_forget takes the item's versions only; test_memory forget test.)
- [x] Chat "Forget for good?" text states exactly what will be deleted.
      (Step 3: `ForgetPlan.describe` + `keeps()`: the items, the activity rows, the summaries rebuilt, and that the owner's messages stay.)
- [x] A forget clears the fact from a pending clarifying question (`conversations.pending`).
      (Step 3: `Supervisor.forget_pending`: a chat whose pending request, question or payload holds a forgotten item's words drops that pending question.)
- [x] Forget keeps the owner's chat message that stated the fact (owner, 2026-10-30): chat history is the owner's
      own record. Watcher-captured sources (trail rows) are deleted.
      (Step 3: journal sources are never deleted; trail sources are.)

### Perception
- [x] Accessibility rows: out of `today.jsonl` and the per-turn activity search; `trail` only; purge after 7 days.
      (Step 9b: the activity listener skips them; `Trail.purge_source("accessibility", 7)` in chunks each hourly pass.)
- [x] Privacy: accessibility records browser window titles with no incognito or excluded-domain check; typed text
      and clipboard check only app exclusions. Apply the browser checks (as windows.py:3-5 does).
      (Step 9b: a browser's AX events keep no title; text typed or copied in a browser carries its page's host and
      incognito flag from the last tab observation (seen before the exclusions apply), so the rules drop it; with no
      known page it isn't recorded (`Trail._in_browser_page`).)
- [x] Activity search indexes JSON keys as words (daily.py:180): index values only, prefer recent rows.
      (Step 9b: `daily.values`; a later row gains less than one shared term; the block stays within its limit.)
- [x] `today.jsonl` stores each row's data twice (`data` and `where`, main.py:4521-4522). (Step 9b: no `where` copy.)
- [x] Redaction runs ~6 times per event (watcher, trail, four in `_publish` main.py:4517-4528): once.
      (Step 2: one events object. 6a: every string the redactor returns is marked for the current known values, so
      the trail's, the events object's and other later passes over an already-redacted value skip its text.)
- [x] Software watcher re-reads TCC and all LaunchAgent plists every second (watchers/software.py:20-37): on change.
      (Step 9b: `software.change_stamp` (stat of the TCC databases and launch-job folders) each tick; re-read on change.)
- [x] `purge_text` rewrites and fsyncs `today.jsonl` once per expired 7-day-old row, which can never be in today's
      file (perception.py:152 → daily.py:117-122). (Step 9b: no per-row rewrite; one `roll_over` when rows expired.)
- [x] Typed text dropped if focus moves within 0.5 s of the last key (capture.py:82-99).
      (Step 9b: a burst is emitted when focus moves to another element (`Field.place`); one whose own field turned
      secret, or under Secure Input, is still dropped.)
- [x] Prompts: one line in the worker prompt that recorded activity, accessibility events included, is available
      through `trail.query` (the planner is tool-less: decision 10). (Step 9b: worker.md; supervisor.md names the
      earlier-conversation block.)

### Jev off
- [x] One switch, off by default; the batcher, decider, shadow checks (main.py:3749-3760) and attention check it.
      The batcher today ignores `refused` (decider.py:62) and re-redacts up to 32k tokens per batch before checking.
      (Step 3: `[jev] on` (default false). Off: no decider built (no Keychain read, no request), trail tagging a no-op, no batcher work, no decisions/receipts, no shadow checks; on but refused (401/402/403): the same (`_jev_active`). test_decide.)

### Data already on disk (cleanup: notes/storage.md decision 4)
- Learning tables no longer used: `learning_jobs`, `learning_receipts`, `learning_digests`, rollup nodes (the embedding tables
  `learning_vectors`/`learning_embedding_evals` stay: decision 8); `task_memory_refs` (16,680 rows).
- Perception tables, never purged: `perception_batches`, `perception_receipts`, `perception_seen` (~130 MB);
  `decisions` (~20 MB).

## Rulings (step 9a, technical)
- Tier 1 budget: `PROFILE_TOKENS` = 2,500 (within decision 7's ~2-3k), counted as FERB's context counts (4 chars a
  token). Every render stops at it; what is left out is counted in one line and makes the pass due.
- "Holds up" (decision 15): the owner's words at once; FERB's understanding and observed facts once their sources span
  two separate days (seen again) or they have stood a full weekly pass (7 days) without being replaced, retired or
  forgotten. "Used without correction" needs exposure records (step 9b). An item the owner kept under an older build's
  Keep question (active inferred at the clean-up, `owner_kept`) holds up at once. The old setup dump never.
- Code's own Tier 1 additions: only an owner-stated preference about the owner (the old Layer 1 rule, so nothing reaches
  prompts later than before), under "Recently said", while it fits; everything else waits for the pass. The owner's
  "Always on" pins (`memory.profile` add) survive every pass.
- The pass: weekly, or sooner when over budget, at most one try an hour; the model returns sections of ids and `ended`;
  code refuses ids that don't hold up, renders, retires the ended ones ("FERB's weekly check"), told once in chat.
- Replacement: a version resting on the owner's words now (chat capture, `memory add`/`edit`, a distilled quote that
  passes `_may_end`) ends every current version of its fact and the ids named in `replaces` that hold; any other version
  never ends one the owner stated. Identical words are credited to the existing item (the owner's first), never a twin.
- The distill no longer asks: its retire on the owner's words (passing `_may_end`) or on its own check is applied and
  told once; words older than the item are refused. A stored key is skipped unless the owner's quote replaces it.
- `review_in_days` (planner; 0 = none) sets `review_at`; due items are listed to the distill; one it doesn't retire has
  its date cleared.
- Decision 14 matching: a known destination goes when its stored value is whole words of the item's content, label or
  subject name; a host also by its bare host or its site label (4+ letters: "kaggle" for www.kaggle.com); a phone by
  its digits (6+). Over-matching only makes the next send ask again.
- Decision 16: code gathers the 25 most-used apps (`TOP_APPS`), installed/used counts, browsers and Chromium profiles
  (Local State: name, account, last used); one model call (mac_profile.md) writes the summary and facts; facts are
  observed, active at once (their source is the gathered facts, a `memory.mac_profile` journal row), Tier 1 only once
  they hold up. Shown once: by the setup step (`setup.persona` state "seen", `summary`), else as one chat notice.
  Re-checked weekly with the profile pass; re-interpreted only when the 10 most-used apps, browsers or profiles changed
  (`facts_fingerprint`); a key not given again ends.
- The clean-up (ferb/memory/upgrade.py) is SQL only: no `on_ended` hooks, so a merged twin's login name is left as is
  (both versions said the same).
- `memory.read`: no query lists current items; a query keeps `prompt_ready` hits; an id reads any version.

## Rulings (step 9b, technical)
- **Embedding model: minishlab/potion-base-8M** (MIT, revision bf8b0566…, pinned SHA-256s), a model2vec static table:
  29,528 WordPiece tokens × 256 float32, a text = the normalized mean of its tokens' rows. Read by FERB's own ~150 lines
  (WordPiece tokenizer + memory-mapped safetensors; checked token-for-token against the `tokenizers` library on 8 mixed
  scripts): no new framework (no numpy, torch, MLX or onnxruntime). Measured on this M-series Mac (2026-10-31):
  download 31 MB (config 0.2 KB, weights 30.2 MB, tokenizer 0.7 MB); load 27 ms; RAM +~10 MB resident (36 vs 26 MB
  max RSS; the table is mmap'd); 0.2 ms CPU per short text, 2.9 ms per 2,000 characters. On a synthetic store shaped
  like the owner's (52 facts, 304 learned nodes, 1,452 chat entries at the owner's measured lengths): full backfill
  1,808 vectors in 23 batches, 1.2 s CPU (53 ms a batch; the daemon sleeps 50 ms between batches); one planner query
  (facts + nodes + chat, query embedding included) 9 ms. At 10× that store (18,080 vectors): backfill 12.5 s CPU once,
  33 ms a query, 99 MB max RSS for the whole process. Quality: related pairs from the owner's real chat phrasings scored
  0.21–0.47, unrelated −0.09–0.14; it misses coined words ("linkedinish") — bm25 and the owner's words cover those.
  English-only vocabulary (Hindi/Hinglish fall back to keywords). Not chosen (estimates, not measured here): MiniLM/bge
  through onnxruntime (a ~30 MB native runtime plus a 23–130 MB model, a transformer pass per text: more CPU and heat);
  the old MLX path (mlx plus transformers); Apple's NLEmbedding (a new PyObjC binding; its quality not evaluated).
- Model files: `[memory] embedding_model` (a folder; the app bundle can point there, step 12) else fetched once into
  `~/.ferb/models/embeddings/potion-base-8M@bf8b056651a2` (temporary folder, hashes checked, then renamed). Until it
  is there, search runs on keywords and recency. Tests never download (`embeddings=False`, `FakeEmbedder`).
- Vectors: normalized int8 + sign bits in `recall_vectors`; `nearest` takes the 256 closest by sign-bit Hamming distance
  above 1,000 vectors, then exact int8 cosine; an in-memory copy per database and model reloads when the table's
  trigger-kept generation changes. A forgotten fact's or deleted node's vector goes with it (triggers).
- `MIN_SIMILARITY` 0.2 (from the measured pairs above); `NEAREST` 100 per kind; recency weight and half-life as
  Learning.search (0.05, 30 days; the owner's words don't age); `EMBED_CHARS` 2,000; background pass every 60 s
  (one cheap query when nothing is new), 64 per batch; a backfill-sized pass is journaled `memory.embedded` with counts
  and CPU/wall seconds.
- Limits per role (decision 7; coordinator 2026-10-31): `search.PLANNER` — facts + learned items ≤ 4,000 tokens and ≤ 20
  items, old chat ≤ 2,000 tokens and ≤ 5 exchanges, today's activity ≤ 2,000 tokens; `search.WORKER` — 2,000 tokens /
  10 items, no pushed chat, activity 2,000 (one activity search serves both roles). Retrieved blocks are no longer
  protected; what doesn't fit is one counted line naming memory.read / memory.learned / trail.query. Lookup tools rank
  at most 100. These replace the 2026-10-24 "Planner 60, worker 20" item budgets (they bounded items, not size).
- A planner turn's old chat: exchanges from any owner chat matching the message, never an entry already in its
  conversation or this turn's messages, never in a scheduled run's turn.
- Playbooks: `PLAYBOOK_SIMILARITY` 0.5 (same-kind goal pairs 0.54–0.75, unrelated ≤ 0.15), at most 3 merges an hourly
  pass and only while no task runs, at most the 6 newest runs per call; a run's attempts = its episode's worker steps.
- Card sources: every episode/journal/memory ref, one ref per distinct origin set of the others; labels from all.
- How-notes show for verified runs only; at most 12 steps and 600 characters; the owner's hints quoted, 300 chars each.
- Decision 18: code joins FERB's understanding / observed facts only on an event that shows them holding up (a save on
  a second day); "standing 7 days uncorrected" is the weekly pass's to see (the pass stays the only scheduled rewrite).
  Code's joins are facts about the owner (`subject` "owner"); the pass places the rest. An owner fact that doesn't fit
  makes the pass due early, at most once in 24 h (`EARLY_PASS_HOURS`).
- Decision 17: "set aside" is its own state beside `memory_items` (`memory_set_aside`), not an end: the login username
  an owner fact gave stays (it is still theirs and current); the profile drops it at once.
- Accessibility rows go after 7 days whether or not their day was summarized; deleted 2,000 rows a statement.
- Typed or copied text in a browser whose page FERB hasn't seen is not recorded (it can't be checked).
- Bot checks: a frame whose address is a known challenge frame (Cloudflare Turnstile, hCaptcha, reCAPTCHA, Arkose,
  DataDome, AWS WAF, GeeTest; `snapshot.CHALLENGE_FRAMES`) puts `bot_check` {widgets, frames, ability
  browser_verification, read} first in the snapshot result. The extension already reported each frame's address and
  each frame element's `src`: no protocol change, no extension reload.

## Known residuals (accepted or deferred)
- The 1.5M existing `learning_sources` rows of older cards and the closed fact/identity nodes stay until the step-12
  clean-up (owner's OK); new cards no longer fan out.
- Old chat and memory are English-first by meaning (the model's vocabulary); other languages match by keywords.
- Accessibility rows hold no values and never name the owner as actor, so they answer "something changed in app Y
  at time T", not "what changed" or "who did it". Making them richer is a separate project.
- Text sent to a model provider can't be recalled by a local forget.

## Size
About 3,200 lines go (learning ~1,550, forget ~1,500, dead code ~200; embeddings stay); sense and decide stay. Daily row growth drops
by what the paused Jev batcher writes (~100k rows); accessibility stays, capped at 7 days.
