-- Build step 11: memory items (section 14). SQLite is the source of truth; markdown is an export.
CREATE TABLE memory_items (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    id          TEXT NOT NULL UNIQUE,
    kind        TEXT NOT NULL CHECK (kind IN ('fact', 'preference', 'person', 'project', 'env')),
    subject     TEXT NOT NULL,             -- "owner", "project:ferb", "person:<slug>", ...
    key         TEXT NOT NULL,             -- what the item is about (e.g. "dentist"): versions share it
    content     TEXT NOT NULL,
    origin      TEXT NOT NULL CHECK (origin IN ('owner_stated', 'observed', 'inferred', 'third_party')),
    status      TEXT NOT NULL CHECK (status IN ('proposed', 'active', 'retracted')),
    confidence  REAL NOT NULL,
    observed_at TEXT NOT NULL,
    valid_from  TEXT NOT NULL,
    valid_to    TEXT,                      -- set when superseded: history is kept, never overwritten
    supersedes  TEXT,
    created_at  TEXT NOT NULL
);
CREATE INDEX memory_items_key ON memory_items(kind, subject, key);

-- Where each item came from: journal entries and trail rows (actor kept: FERB's own never make owner habits).
CREATE TABLE memory_sources (
    item_id   TEXT NOT NULL,
    kind      TEXT NOT NULL CHECK (kind IN ('journal', 'trail')),
    source_id TEXT NOT NULL,
    day       TEXT,
    actor     TEXT,
    PRIMARY KEY (item_id, kind, source_id)
);
CREATE INDEX memory_sources_source ON memory_sources(kind, source_id);

CREATE VIRTUAL TABLE memory_fts USING fts5(subject, key, content, content='memory_items', content_rowid='seq');
CREATE TRIGGER memory_fts_insert AFTER INSERT ON memory_items BEGIN
    INSERT INTO memory_fts(rowid, subject, key, content) VALUES (new.seq, new.subject, new.key, new.content);
END;
CREATE TRIGGER memory_fts_delete AFTER DELETE ON memory_items BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, subject, key, content) VALUES ('delete', old.seq, old.subject, old.key,
                                                                              old.content);
END;
CREATE TRIGGER memory_fts_update AFTER UPDATE OF subject, key, content ON memory_items BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, subject, key, content) VALUES ('delete', old.seq, old.subject, old.key,
                                                                              old.content);
    INSERT INTO memory_fts(rowid, subject, key, content) VALUES (new.seq, new.subject, new.key, new.content);
END;

-- One-way fingerprints of forgotten items: distillation may not propose them again. Nothing readable is kept.
CREATE TABLE memory_forgotten (
    fingerprint TEXT PRIMARY KEY,
    at          TEXT NOT NULL
);

-- Daily distillation bookkeeping (attempts per day; the summary itself lives in the journal).
CREATE TABLE distill_runs (
    day       TEXT PRIMARY KEY,
    attempts  INTEGER NOT NULL DEFAULT 0,
    status    TEXT NOT NULL,              -- 'summarized' | 'distilled' | 'failed'
    at        TEXT NOT NULL,
    detail    TEXT
);
