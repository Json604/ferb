-- Step 9a (notes/memory.md decisions 9, 13, 15; target design 1). Tables beside `memory_items` (re-runnable as 0050-0052
-- are): an item made before has no row and reads as before.
--
-- What a remembered item is about (decision 13: a short label code stores next to the owner's exact words, from the
-- task) and when FERB checks it still holds (a period fact without an end date in the owner's words).
CREATE TABLE IF NOT EXISTS memory_context (
    item_id   TEXT PRIMARY KEY,
    about     TEXT NOT NULL DEFAULT '',
    review_at TEXT                         -- ISO time; NULL = no check due
);
-- Tier 1, the core profile: which facts are always on in every role's prompt, grouped and ordered. Each row points at
-- its fact; the text shown is always the fact's own (code renders it), so the owner's words are never paraphrased.
CREATE TABLE IF NOT EXISTS memory_profile (
    item_id  TEXT PRIMARY KEY,
    section  TEXT NOT NULL,
    position INTEGER NOT NULL,
    added_by TEXT NOT NULL,                -- 'seed' | 'code' | 'pass' | 'owner'
    added_at TEXT NOT NULL
);
