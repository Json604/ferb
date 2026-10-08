-- F4 (V1, D20): captured state (accessibility snapshots, recordings, app-window screenshots) the checker can
-- re-inspect, kept 7 days (retention.evidence_days). Media files live under ~/.ferb/evidence; this indexes them.
-- Screenshots are stored with accessibility-known secret fields masked (ferb/act/evidence/mask.py); full secret
-- redaction is F5. Nothing here holds a secret value.
CREATE TABLE evidence (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL,
    span_id     TEXT,
    kind        TEXT NOT NULL,        -- app_window | snapshot | recording | screenshot
    mime        TEXT,                 -- image/jpeg, video/mp4; NULL for a data-only row
    path        TEXT,                 -- file on disk, relative to the evidence root; NULL for a data-only row
    data_json   TEXT,                 -- small structured facts (title, url, element count), redacted
    origin      TEXT NOT NULL,        -- ferb/core/origin.py
    captured_at TEXT NOT NULL
);
CREATE INDEX evidence_task ON evidence(task_id);
CREATE INDEX evidence_time ON evidence(captured_at);
