# Memory import from other AI assistants

Owner, 2026-10-30: extract the owner's memory from the major AI assistants they use (web, CLI and Mac apps): about
1-1.5 years of memories and chat history. Schema-based extraction and storage per assistant, then correct extraction
of what matters. Many model calls are acceptable; **accuracy and recall** are what count, and the result must make
sense in FERB's memory and actually help it.

Status: built 2026-10-31 on ui/ux after step 9b (fix-order 9c); checkout-validated with a scripted model; live checks
with step 13. The "Shape" below is the outline the build followed; "What exists" and "Rulings" say what was built.

## Owner decisions
1. **Last 3 months only** (owner, 2026-10-30): the import reads only the last 3 months of each assistant's
   transcripts, usage and saved memory; anything older is not read.

2. **Imported facts are FERB's understanding, never the owner's own words**, and never add known destinations
   (owner, 2026-10-30): exports don't mark pasted text, so an imported "owner" line can't carry owner authority.

3. **Sources: ChatGPT, Claude (claude.ai), Claude Code + Codex CLI, Gemini** (owner, 2026-10-31): ChatGPT and
   claude.ai from their data exports (web + Mac app, saved memories/projects where the export has them), Claude Code
   and Codex CLI from local transcripts on this Mac, Gemini from Google Takeout.
4. **Facts only** (owner, 2026-10-31): FERB extracts facts, preferences, projects, people and recurring tasks (as FERB's
   understanding, decision 2); the raw imported chats are not kept or made searchable.
5. **Hosted processing is fine** (owner, 2026-10-31): extraction runs on the configured AI service after redaction
   (rule 8: storage stays on the Mac, only redacted text leaves).
6. Review follows memory decisions 15/18: no Keep questions; imported facts are FERB's understanding, shown on the
   memory page and as one summary notice, joining the core profile only once they hold up.

## Sources (formats verified 2026-10-31)
Verified from public descriptions and parsers that read real exports (no vendor publishes a schema; formats change):
the openai/codex source (codex-rs `history`, `protocol` crates) for Codex; convoviz, Letta's import notes, foxpack and
chatgpt-exporter for ChatGPT; ClaudeReader/ClaudeSearch (measured on real exports) and PDP-Connect's split-export
fixtures for claude.ai; Google's Gemini help (support.google.com/gemini/answer/16920332) and real-export parsers
(420AI, imbue datalib) for Takeout; code.claude.com/docs/en/sessions plus txcript for Claude Code. The owner's own
Claude Code and Codex files were inspected for structure only (key names and record types, never content).
- **ChatGPT**: Settings → Data controls → Export data, a zip: `conversations.json` (larger exports
  `conversations-000.json`, `-001`, …); each conversation a `mapping` tree (edits and regenerations kept), the shown
  branch from `current_node` up the parents; unix-float times; `author.role` system/user/assistant/tool; custom
  instructions as a hidden `user_editable_context` (or `metadata.user_context_message_data`). **No memory file**: saved
  memory appears only as `model_editable_context` snapshots and assistant writes with recipient `bio`.
- **claude.ai**: Settings → Privacy → Export data. Until ~Aug 2026 one zip (`conversations.json`, `projects.json`,
  `memories.json`, `users.json`); since Sept 2026 an emailed manifest linking one zip per kind (`conversations-000.zip`,
  `projects-000.zip` with `projects/<uuid>.json`, `memories-000.zip` with `memories/<uuid>.json`, …), each link usable
  once while signed in. `chat_messages` with `sender` human/assistant, text mostly in `content` blocks, ISO times;
  memory keys `conversations_memory`, `project_memories`, `memory_files`.
- **Gemini**: Google Takeout → "My Activity" (filter Gemini Apps) → `Takeout/My Activity/Gemini Apps/MyActivity.json`
  (choose JSON; the default is HTML). Flat records, no thread ids: `title` "Prompted …", `time` ISO UTC, reply HTML in
  `safeHtmlItem`; a variant with `details` Request/Response. HTML: `outer-cell` blocks, local time with a zone name.
  Names are localized ("Prompted", folder names). The separate "Gemini" product (Gems) and Saved info are not read.
- **Claude Code**: `~/.claude/projects/<project>/<session>.jsonl` (internal format; 30-day default retention).
  Typed turns are `user` records that aren't meta, compact summaries, sidechains or tool results (`turnOrigin` human
  where recorded); subagents in `<session>/subagents/` (older builds `agent-<id>.jsonl`); SDK sessions carry
  `entrypoint` sdk-*. Saved memory notes `<project>/memory/*.md`.
- **Codex CLI**: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` and `archived_sessions/`; lines
  `{timestamp, type, payload}`; `session_meta.source` cli / vscode / exec / mcp / subagent; owner turns are
  `response_item` user messages minus injected context (`# AGENTS.md instructions for …`, `<environment_context>`,
  other tags) and messages inherited from a parent thread. On this Mac 285 of 546 sessions are `codex exec` runs
  (programs' prompts) and 127 subagent threads; FERB's own Codex runs use an empty home and ephemeral threads, so none
  are here.
- Others the owner uses (Perplexity, Cursor, Copilot…): to list with the owner.

## Shape (first outline)
1. **Per-source adapters → one normalized schema**, in memory only (decision 4: raw chats are not kept): source,
   conversation id, title, timestamps, each message with role (owner / assistant / tool). Redaction at read.
2. **Trust follows origin**: only the owner's own messages can state facts about them; assistant replies and pasted
   material are data (they can corroborate, never authorize). The assistant's saved "memories" are claims to verify
   against the owner's words where possible.
3. **Extraction for recall, then verification for accuracy**: a recall pass over each conversation (chunked) proposes
   candidate facts, preferences, projects, people, and recurring tasks, each with the exact owner quote and message
   reference; a verification pass checks every candidate against its quote and context and drops what the quote
   doesn't support.
4. **Across sources and time**: merge duplicates, resolve contradictions by date (newer wins; older kept as history),
   mark facts that were true for a period (old jobs, finished projects) as ended.
5. **Into FERB's tiers**: verified facts → Tier 2 as FERB's understanding (decision 2); Tier 1 only once they hold up
   (decision 6). (Superseded: Tier 3 notes from imports and searchable conversations — decision 4.)
6. **Owner review** (superseded by decision 6): no batch review; one summary notice, the memory page shows them.
7. **Measured quality**: a small owner-labelled sample to measure recall and precision before trusting the full import;
   cost and number of model calls reported up front.

## What exists (2026-10-31)
- Readers → one normalized form, in memory only: `ferb/memory/import_sources.py` (`read`, `window_start`, `finish`,
  `owner_words`, one reader per source). Window: three calendar months by each message's own time; texts cut at a word
  boundary (owner 6,000 chars, assistant 700, a saved memory 6,000; the cut said in the text) and redacted before they
  leave the module; tool output not read; injected `<tag>…</tag>` blocks dropped (pasted ones named "[pasted text, not
  read]").
- Extraction: `ferb/memory/import_extract.py`: parts with stable per-message references, chunks of ~24,000 chars;
  recall (`import_recall.md`, `RECALL_SCHEMA`) → code keeps candidates citing an owner message (or a saved-memory claim)
  whose quote is whole words of it (`quoted`), and redacts what the model wrote; verification (`import_verify.md`,
  `VERIFY_SCHEMA`, batches of ~30,000 chars) with each candidate's evidence (the quoted message and its neighbours, or
  for a claim the owner's messages of that source sharing its rarer words, `OwnerIndex`) and the held facts that look
  alike (the fused search) → code keeps a supported verdict only when its quote is whole words of an owner message
  shown as evidence; `save`: merge by `same_as` / kind+subject+key, oldest first, as `inferred` with about "imported
  from <assistant>", resting on the run's `memory.import` journal row; `ended` / `until` → `expire_at`,
  `review_in_days` → `set_review`.
- Job: `ferb/memory/import_job.py` (`ImportJob`: `estimate`, `create`, `run`, `status`, `end`, `tell_done`), daemon glue
  `ferb/daemon/areas/memory_import.py` (`MemoryImportGlue`), migration 0055 (`memory_import_runs`,
  `memory_import_done` part hashes, `memory_import_items` item → source + said_at, deleted with the item).
- RPCs (cli role): `memory.import.estimate {source, path?}` (dry run: conversations, messages, chunks,
  `model_calls_estimate`, tokens; no model call), `memory.import.start {source, path?}` (answer: run id + estimate),
  `memory.import.status` (current run and the last 10: status, counts incl. `model_calls`, `facts_saved`, error),
  `memory.import.cancel`. Event `memory.import.progress` (telemetry). `memory.list` gains `imported_from` per item and
  an `imported_from` filter (a source or "any").
- App: Memory page section "Import from other assistants" (menu of the five sources, a file/folder picker for the
  exports, the estimate line, Start, progress, Cancel import): `ui/mac/FERBNotch` WindowPages.swift,
  FERBWindowStore.swift, NotchCore `MemoryImportSource` / `MemoryImportStatus` / `MemoryImportEstimate`.
- Tests: tests/test_memory_import.py (readers per source incl. window, roles, split ChatGPT files and memory traces,
  claude.ai 2026 zips, Gemini JSON and HTML; redaction before any call; recall → verify through the scripted model;
  saved as FERB's understanding, no known destinations, not in Tier 1 yet, one notice; newer wins; the owner's words
  never replaced; a claim needs the owner's words; a verifier quote not in the message dropped; nothing raw in any
  table, the DB file or WAL; waits for the owner's turn; cancel then go on; resume after restart; the quality eval and
  its `-m real` variant); tests/support/import_fixtures.py; Swift MemoryImportRowsTests.
- Measured (scripted, credulous model, 5 sources, 11 true facts, 12 traps): recall 1.0, precision 0.846 (13 facts kept);
  the one trap kept is unmarked pasted text, which only a model can see. The live score is the `-m real` run (step 13).

## Rulings (technical, 2026-10-31)
- Who is the owner in a transcript: ChatGPT `user`, claude.ai `human`, Gemini "Prompted" records, Claude Code typed
  turns (not meta/sidechain/tool result/compact summary, `turnOrigin` human where present, no SDK sessions or
  subagent files), Codex interactive sessions only (`source` cli / vscode: `codex exec` and subagent prompts are
  written by programs). Custom instructions and claude.ai project instructions count as the owner's lines; uploaded
  files and attachments are not read.
- Saved memories (ChatGPT snapshot + `bio` writes, claude.ai memory, Claude Code memory notes) are claims: a fact from
  one is kept only when the verifier cites an owner message of that source with an exact quote. An undated claude.ai
  memory is dated as of the import (it is the assistant's memory now); its supporting owner words must still be in the
  window.
- Newer wins by when the owner said it (`memory_import_items.said_at`, else a held fact's save time): an older imported
  version is skipped, not inserted as history behind a newer one; a fact the owner stated in their own words is never
  replaced or duplicated by an import (counted `kept_owner`).
- Holds up: imported facts rest on the import day's journal row, so two-day evidence comes only from later use; in
  practice they join Tier 1 after a week uncorrected (the weekly pass) or when the owner says it again.
- Progress per conversation part (hash), across runs: re-importing the same export later reads only what is new; a
  forgotten fact is never re-imported (its fingerprint). A chunk is marked done in the same step its facts are saved.
- Never on the owner's turn: each model call waits while a chat turn runs (`_turns`); running tasks don't block it
  (a long loop would stall imports forever). Three failed chunks in a row stop the run ("failed", told once).
- Model calls: one recall call per chunk plus verification calls (about one per chunk); the estimate says
  2 × chunks; the run's actual count is in `counts.model_calls` and the summary.
- Runtime: the worker runtime, no tools, typed output (as distill and the profile pass); spans `model.learning` with
  roles `memory_import_recall` / `memory_import_verify`, the goal "import memory from <assistant>", never the input.
- Gemini HTML times written with a zone other than UTC/GMT are read in this Mac's zone (a day's error at most).

## Open questions for the owner
- Others to add (Perplexity, Cursor, Copilot…) — only if wanted.

## Fix list
- [x] Readers for ChatGPT, claude.ai, Claude Code, Codex CLI, Gemini → one normalized form, last 3 months.
- [x] Recall → verification → merge (newer wins) → saved as FERB's understanding; one summary notice; memory page source.
- [x] Background job: estimate, progress, resume after restart, cancel, never on the owner's turn.
- [x] Measured quality on labelled fixtures (offline); live score with step 13.
- [ ] Live checks (step 13): import each real source once; read the notice, the memory page filter and a sample of
      facts against the owner's memory of them; run `-m real tests/test_memory_import.py::test_quality_live`.
- [ ] Localized Takeout ("Prompted" in other languages), zstd-compressed Codex rollouts (`.jsonl.zst`): not read yet.
- [ ] The app renders the notice's `page` card (opens the Memory page filtered to the source); today it shows the
      text and note cards only.
