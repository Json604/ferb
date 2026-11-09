-- APP-805 (owner, 2026-11-09: "A. FERB's database only"): the chat as FERB.app shows it, one row per entry (a turn,
-- a late reply, a notice) in the app's own shape, kept here instead of the app's preferences. Kept forever with the
-- rest of the chat (STORE-401); written only by FERB.app (auth.APP_ONLY), redacted as it arrives.
--   id       the entry's id as the app made it (a UUID; the archive's ids carry over)
--   kind     turn | reply | notice
--   at       when the entry was made (UTC ISO 8601, microseconds: sorts as text)
--   day      the app's calendar day of `at` ("YYYY-MM-DD"): what History lists and a day's page reads
--   open     1 while a turn waits for FERB's final answer (the app loads these at launch)
--   ref      a notice's daemon id (its text follows attention_notices; a forget clears it here too)
--   body_json the entry as the app encodes it
CREATE TABLE IF NOT EXISTS chat_history (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,
    at         TEXT NOT NULL,
    day        TEXT NOT NULL,
    open       INTEGER NOT NULL DEFAULT 0,
    ref        TEXT,
    body_json  TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS chat_history_day ON chat_history (day, at, id);
CREATE INDEX IF NOT EXISTS chat_history_open ON chat_history (open) WHERE open = 1;
CREATE INDEX IF NOT EXISTS chat_history_ref ON chat_history (kind, ref);
