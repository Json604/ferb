-- Build step 13: every Jev answer next to FERB's own decision for the same question (section 12). No payloads are
-- stored: `ref` points at what was decided (a trail row, a span, a task, a memory proposal), so examples are read
-- from the source while it exists and disappear with it (retention purge, `forget`).
CREATE TABLE decisions (
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
    mode        TEXT NOT NULL CHECK (mode IN ('shadow', 'enabled')),
    effect      TEXT                       -- what Jev's answer did (only ever stricter), NULL = nothing
);
CREATE INDEX decisions_question ON decisions(question, seq);
CREATE INDEX decisions_ref ON decisions(ref);
