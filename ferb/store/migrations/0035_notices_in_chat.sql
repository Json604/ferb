-- L9 (launch-week fixes, 2026-10-13).
-- C3/Q4: trail.attention is live. Its decisions are recorded as 'live' with their effect, so the mode CHECK gains
-- 'live'. SQLite cannot change a CHECK in place: the table is rebuilt with every row and both indexes kept.
CREATE TABLE decisions_live (
    seq         INTEGER PRIMARY KEY AUTOINCREMENT,
    id          TEXT NOT NULL UNIQUE,
    at          TEXT NOT NULL,
    question    TEXT NOT NULL,
    ref         TEXT,
    baseline    TEXT NOT NULL,             -- FERB's existing decision (current behaviour)
    jev         TEXT,                      -- Jev's answer; NULL when Jev wasn't asked or didn't answer
    confidence  REAL,
    latency_ms  INTEGER,
    error       TEXT,                      -- why Jev didn't answer (unavailable, timeout, no key, ...)
    mode        TEXT NOT NULL CHECK (mode IN ('shadow', 'enabled', 'live')),
    effect      TEXT                       -- what the answer did; NULL = nothing
);
INSERT INTO decisions_live (seq, id, at, question, ref, baseline, jev, confidence, latency_ms, error, mode, effect)
    SELECT seq, id, at, question, ref, baseline, jev, confidence, latency_ms, error, mode, effect FROM decisions;
DROP TABLE decisions;
ALTER TABLE decisions_live RENAME TO decisions;
CREATE INDEX decisions_question ON decisions(question, seq);
CREATE INDEX decisions_ref ON decisions(ref);

-- I1: perception starts at its install. Trail rows recorded before this migration never reach Jev, and the
-- Notification Center records already there at the first read after it are marked seen, never recorded.
CREATE TABLE perception_floor (
    source TEXT PRIMARY KEY CHECK (source IN ('trail', 'notification')),
    seq    INTEGER,          -- trail: the last trail seq before install; notification: records found at the first
                             -- read (NULL until that read)
    at     TEXT NOT NULL
);
INSERT INTO perception_floor VALUES ('trail', (SELECT COALESCE(MAX(seq), 0) FROM trail),
                                     strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'));
INSERT INTO perception_floor VALUES ('notification', NULL, strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'));

-- L9: a notice is posted to the owner's chat when it is made, and told from then on; delivered_at records that a
-- client showed it. Quotes keep outside text apart with its origin (MP2). considered_refs are every row the
-- consideration saw, so the D14 purge reaches text derived from typed or clipboard rows (I3).
ALTER TABLE attention_notices ADD COLUMN quotes_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE attention_notices ADD COLUMN considered_refs_json TEXT NOT NULL DEFAULT '[]';
UPDATE attention_notices SET considered_refs_json = (
    SELECT json_group_array('trail:' || e.value) FROM perception_batches b, json_each(b.event_ids_json) e
    WHERE 'batch:' || b.id = attention_notices.id);
-- Notices made before chat delivery went out as macOS banners when they were made: delivered then, not replayed.
UPDATE attention_notices SET delivered_at = created_at WHERE delivered_at IS NULL;
