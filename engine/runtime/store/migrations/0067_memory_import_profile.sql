-- The memory import's profile (kb area memory, MEM-439..447; the owner's decisions from the memory-import lab,
-- IMP-11..IMP-53). Nothing here holds what the owner's conversations said (MEM-404): checked facts, the lines they
-- were merged into, where each was said (a session's folder or a chat's title), hashes and dates.
--
-- Every fact the import's recall and verify kept, before it is placed: the evidence a profile line rests on. A fact
-- is placed once: it joins or changes a line (`line_id`), becomes a new line, or is left out with a reason
-- (`left_why`, shown only when the owner asks, IMP-20).
CREATE TABLE IF NOT EXISTS memory_import_facts (
    id            TEXT PRIMARY KEY,               -- f + a hash of source, kind, subject, key, content and when said
    source        TEXT NOT NULL,                  -- chatgpt | claude | claude_code | codex | gemini
    run_id        TEXT NOT NULL,
    kind          TEXT NOT NULL,
    subject       TEXT NOT NULL,
    key           TEXT NOT NULL,
    content       TEXT NOT NULL,                  -- redacted, third person
    said_at       TEXT NOT NULL,
    conversation  TEXT NOT NULL DEFAULT '',       -- the conversation or session it came from (its id)
    place_kind    TEXT NOT NULL DEFAULT '',       -- folder | chat | '' (a saved memory: no one place)
    place         TEXT NOT NULL DEFAULT '',       -- the session's folder, or the chat's title
    claim         INTEGER NOT NULL DEFAULT 0,     -- proposed from an assistant's saved memory
    ended         INTEGER NOT NULL DEFAULT 0,
    until         TEXT NOT NULL DEFAULT '',
    review_days   INTEGER NOT NULL DEFAULT 0,
    line_id       TEXT,                           -- the profile line it rests on
    left_why      TEXT,                           -- why it is not in the profile
    at            TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS memory_import_facts_line ON memory_import_facts(line_id);
-- One line of the profile: a memory item's name kept for good (its kind, subject and key) and an id that never
-- changes, so every later import joins, changes or leaves it and never rewrites it (IMP-31). Its words are the memory
-- item's (`item_id`, the current version); `data_json` holds the rest: the wording to the owner and to FERB, the
-- offer, the dates and sources set by code from its facts, the scope and who set it, its history.
CREATE TABLE IF NOT EXISTS memory_import_lines (
    id         TEXT PRIMARY KEY,
    item_id    TEXT NOT NULL,
    kind       TEXT NOT NULL,
    subject    TEXT NOT NULL,
    key        TEXT NOT NULL,
    data_json  TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS memory_import_lines_item ON memory_import_lines(item_id);
-- What each conversation with an assistant is about (one project, or '' for a general one), read once (IMP-43).
CREATE TABLE IF NOT EXISTS memory_import_chats (
    conversation TEXT PRIMARY KEY,
    about        TEXT NOT NULL
);
-- How recent each source's newest imported export was, so an older export says so and sends nothing (MEM-438).
CREATE TABLE IF NOT EXISTS memory_import_exports (
    source TEXT PRIMARY KEY,
    newest TEXT NOT NULL,
    at     TEXT NOT NULL
);
-- An export FERB fetched itself (never one the owner gave): deleted when its import is done, or a day after a failed
-- or cancelled one, with one notice (IMP-32, MEM-440).
CREATE TABLE IF NOT EXISTS memory_import_files (
    path       TEXT PRIMARY KEY,
    source     TEXT NOT NULL,
    run_id     TEXT NOT NULL DEFAULT '',
    fetched_at TEXT NOT NULL
);
-- The profile's own small state: when the offers were last checked and against what (a hash), the offers being
-- asked about now. Keys and short values only.
CREATE TABLE IF NOT EXISTS memory_import_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
