-- Memory import from other AI assistants (notes/memory-import.md): the runs, progress and which facts came from where.
-- Nothing here holds what the owner's conversations said (decision 4: facts only): ids, counts, hashes and dates.
--
-- One import of one source: the export path the owner chose (re-read on a resume), its state and its counts.
CREATE TABLE IF NOT EXISTS memory_import_runs (
    id          TEXT PRIMARY KEY,
    source      TEXT NOT NULL,                 -- chatgpt | claude | claude_code | codex | gemini
    path        TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL,                 -- running | done | failed | cancelled
    started_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    finished_at TEXT,
    counts_json TEXT NOT NULL DEFAULT '{}',
    error       TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS memory_import_runs_status ON memory_import_runs(status);
-- Progress: the hash of each conversation part already read (a restart, or a later import of the same export, skips it)
CREATE TABLE IF NOT EXISTS memory_import_done (
    source TEXT NOT NULL,
    part   TEXT NOT NULL,
    run_id TEXT NOT NULL,
    at     TEXT NOT NULL,
    PRIMARY KEY (source, part)
);
-- Which assistant a fact was imported from and when the owner said it there (newer words win across sources and time)
CREATE TABLE IF NOT EXISTS memory_import_items (
    item_id TEXT NOT NULL,
    source  TEXT NOT NULL,
    run_id  TEXT NOT NULL,
    said_at TEXT NOT NULL,
    PRIMARY KEY (item_id, source)
);
CREATE TRIGGER IF NOT EXISTS memory_import_item_gone AFTER DELETE ON memory_items BEGIN
    DELETE FROM memory_import_items WHERE item_id = old.id;
END;
