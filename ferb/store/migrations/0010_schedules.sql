-- Build step 15: scheduled and recurring tasks (section 18 step 15). The intent is recorded from the owner's words
-- when the schedule is made; every run is held to it.
CREATE TABLE schedules (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    id           TEXT NOT NULL UNIQUE,
    request      TEXT NOT NULL,             -- what to do, in the owner's words
    own_words    TEXT NOT NULL,             -- the owner's whole message when it was scheduled (target checks)
    intent_json  TEXT NOT NULL,             -- the recorded intent: kinds, recipients, sites, accounts, paths, amount
    rule_json    TEXT NOT NULL,             -- repeat: none | daily | weekdays | weekly | every; at (HH:MM), tz, ...
    next_run     TEXT,                      -- ISO UTC; NULL when finished
    timer_id     TEXT,
    state        TEXT NOT NULL CHECK (state IN ('active', 'paused', 'cancelled', 'done')),
    failures     INTEGER NOT NULL DEFAULT 0,
    last_run     TEXT,
    created_at   TEXT NOT NULL
);

-- Questions FERB couldn't ask when it needed to (an unattended run): asked at the owner's next chat (section 8).
CREATE TABLE owner_questions (
    seq      INTEGER PRIMARY KEY AUTOINCREMENT,
    id       TEXT NOT NULL UNIQUE,
    at       TEXT NOT NULL,
    kind     TEXT NOT NULL,                 -- folder_access
    target   TEXT NOT NULL,
    question TEXT NOT NULL,
    state    TEXT NOT NULL CHECK (state IN ('open', 'yes', 'no'))
);

-- Timers gain the 'schedule' kind (a schedule's next run is a durable timer, delivered exactly once).
CREATE TABLE timers_new (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    id           TEXT NOT NULL UNIQUE,
    kind         TEXT NOT NULL CHECK (kind IN ('reminder', 'timer', 'sleep', 'schedule')),
    fire_at      TEXT NOT NULL,
    message      TEXT NOT NULL DEFAULT '',
    task_id      TEXT,
    request_id   TEXT,
    state        TEXT NOT NULL CHECK (state IN ('pending', 'claimed', 'delivered', 'cancelled')),
    claim_token  TEXT,
    claimed_at   TEXT,
    delivered_at TEXT,
    overdue_s    REAL,
    chat_shown   INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL
);
INSERT INTO timers_new SELECT * FROM timers;
DROP TABLE timers;
ALTER TABLE timers_new RENAME TO timers;
CREATE INDEX timers_due ON timers(state, fire_at);
