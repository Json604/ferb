# Memory import from other AI assistants (new area, not yet scheduled)

Owner, 2026-10-30: extract the owner's memory from the major AI assistants they use (web, CLI and Mac apps): about
1-1.5 years of memories and chat history. Schema-based extraction and storage per assistant, then correct extraction
of what matters. Many model calls are acceptable; **accuracy and recall** are what count, and the result must make
sense in FERB's memory and actually help it.

Status: captured, not designed. Builds on the memory tiers (notes/memory.md, fix-order step 9), so it is designed and
built after that step unless the owner says otherwise. Everything below is a first outline to discuss; facts marked
"verify" were not checked.

## Owner decisions
1. **Last 3 months only** (owner, 2026-10-30): the import reads only the last 3 months of each assistant's
   transcripts, usage and saved memory; anything older is not read.

2. **Imported facts are FERB's understanding, never the owner's own words**, and never add known destinations
   (owner, 2026-10-30): exports don't mark pasted text, so an imported "owner" line can't carry owner authority.

## Sources (verify each)
- ChatGPT (web + Mac app): the account's data export (conversations, and whether saved memories are included).
- Claude (claude.ai web + Mac app): the account's data export; saved memories/projects.
- Gemini: Google Takeout (Gemini activity).
- Claude Code: local transcripts under `~/.claude/projects/*.jsonl` and CLAUDE.md memory files.
- Codex CLI: local sessions under `~/.codex/`.
- Others the owner uses (Perplexity, Cursor, Copilot…): to list with the owner.
- Mac apps may also keep local caches; prefer official exports and local transcript files over scraping app storage.

## Shape (first outline)
1. **Per-source adapters → one normalized schema**, stored raw on the Mac (owner-only): source, conversation id, title,
   timestamps, each message with role (owner / assistant / tool), model, attachments noted. Redaction at import.
2. **Trust follows origin**: only the owner's own messages can state facts about them; assistant replies and pasted
   material are data (they can corroborate, never authorize). The assistant's saved "memories" are claims to verify
   against the owner's words where possible.
3. **Extraction for recall, then verification for accuracy**: a recall pass over each conversation (chunked) proposes
   candidate facts, preferences, projects, people, and recurring tasks, each with the exact owner quote and message
   reference; a verification pass checks every candidate against its quote and context and drops what the quote
   doesn't support.
4. **Across sources and time**: merge duplicates, resolve contradictions by date (newer wins; older kept as history),
   mark facts that were true for a period (old jobs, finished projects) as ended.
5. **Into FERB's tiers**: verified owner-stated facts → Tier 2; stable preferences/identity → Tier 1 core profile
   proposals; how the owner gets recurring work done → Tier 3 notes; optionally the conversations themselves
   searchable as history.
6. **Owner review**: imported facts arrive as a reviewable batch in the app (accept / edit / drop), grouped by topic,
   with the source quote; nothing becomes owner-stated without the owner's words or confirmation.
7. **Measured quality**: a small owner-labelled sample to measure recall and precision before trusting the full import;
   cost and number of model calls reported up front.

## Open questions for the owner
- Which assistants and which accounts, in what order.
- Whether full chat histories should also be searchable in FERB, or only the extracted memory.
- How much review the owner wants (every item, or only uncertain ones).
- Hosted model processing of the redacted history is consistent with CLAUDE.md rule 8; confirm for this volume.
