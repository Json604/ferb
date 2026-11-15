-- Build step 1: spans (traces) and the append-only journal.
CREATE TABLE spans (
    id          TEXT PRIMARY KEY,
    parent_id   TEXT REFERENCES spans(id),
    task_id     TEXT,              -- NULL for daemon-level spans
    kind        TEXT NOT NULL,
    device_id   TEXT,
    started_at  TEXT NOT NULL,     -- ISO 8601 with offset
    ended_at    TEXT,
    input_json  TEXT NOT NULL,
    output_json TEXT,
    cost_usd    REAL,
    error       TEXT
);
CREATE INDEX spans_task ON spans(task_id);
CREATE INDEX spans_parent ON spans(parent_id);

CREATE TABLE journal (
    seq       INTEGER PRIMARY KEY AUTOINCREMENT,
    id        TEXT NOT NULL UNIQUE,
    at        TEXT NOT NULL,
    kind      TEXT NOT NULL,
    data_json TEXT NOT NULL
);
-- Append-only: rows are never changed. (Deletion stays possible for "forget = purge", section 14.)
CREATE TRIGGER journal_append_only BEFORE UPDATE ON journal
BEGIN
    SELECT RAISE(ABORT, 'journal is append-only');
END;
