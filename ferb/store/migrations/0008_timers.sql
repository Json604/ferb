-- Build step 14: durable timers (section 13). Survive restarts and sleep; delivered exactly once (safety test 19):
-- pending -> claimed (with a token) -> delivered; a claim left unacknowledged by a crash is delivered again and the
-- consumer skips a timer id it already handled (timer_deliveries).
CREATE TABLE timers (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    id           TEXT NOT NULL UNIQUE,
    kind         TEXT NOT NULL CHECK (kind IN ('reminder', 'timer', 'sleep')),
    fire_at      TEXT NOT NULL,            -- ISO 8601 UTC
    message      TEXT NOT NULL DEFAULT '',
    task_id      TEXT,                     -- the task that set it (sleep, task timers)
    request_id   TEXT,                     -- the owner's message it came from
    state        TEXT NOT NULL CHECK (state IN ('pending', 'claimed', 'delivered', 'cancelled')),
    claim_token  TEXT,
    claimed_at   TEXT,
    delivered_at TEXT,
    overdue_s    REAL,
    chat_shown   INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL
);
CREATE INDEX timers_due ON timers(state, fire_at);

-- The consumer's own record: one row per timer it acted on; a second delivery of the same id is a no-op.
CREATE TABLE timer_deliveries (
    timer_id TEXT PRIMARY KEY,
    at       TEXT NOT NULL
);
