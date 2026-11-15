-- Build step 10: the monitoring trail (section 11). Append-only: what was observed, when and by whom never changes.
-- Only data_json / relevance / sensitive may be updated later (extra redaction after Jev tagging, build step 13).
-- Rows leave only through the retention purge (a day already summarized in the journal, section 14) or `forget`.
CREATE TABLE trail (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    id         TEXT NOT NULL UNIQUE,
    at         TEXT NOT NULL,              -- ISO 8601 with offset
    day        TEXT NOT NULL,              -- local date (YYYY-MM-DD): the unit of summaries and retention
    device_id  TEXT NOT NULL,
    actor      TEXT NOT NULL CHECK (actor IN ('owner', 'ferb', 'unknown')),
    task_id    TEXT,
    source     TEXT NOT NULL,
    data_json  TEXT NOT NULL,
    relevance  TEXT,
    sensitive  INTEGER
);
CREATE INDEX trail_day ON trail(day);
CREATE INDEX trail_source ON trail(source, seq);
CREATE TRIGGER trail_append_only BEFORE UPDATE OF id, at, day, device_id, actor, task_id, source ON trail
BEGIN
    SELECT RAISE(ABORT, 'trail rows are append-only');
END;
