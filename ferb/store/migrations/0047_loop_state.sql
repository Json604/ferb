-- L31: a loop step's durable state (one row per running or ended loop step). Numbers and FERB's own records only:
-- the armed probe (tool, arguments, field, comparison), its baseline value, round counts, pacing and the cursors
-- that say which plan event and which owner correction it has already handled.
CREATE TABLE IF NOT EXISTS loop_state (
    task_id       TEXT NOT NULL,
    step_id       TEXT NOT NULL,
    state         TEXT NOT NULL DEFAULT 'acting',   -- acting / watching / backing_off / paused / finished / stopped
    rounds        INTEGER NOT NULL DEFAULT 0,
    probe_json    TEXT,                             -- the armed probe, NULL when none
    baseline_json TEXT,                             -- its baseline value (JSON), NULL when none
    reads         INTEGER NOT NULL DEFAULT 0,       -- probe reads since it was armed
    pacing_json   TEXT NOT NULL DEFAULT '{}',
    recent_json   TEXT NOT NULL DEFAULT '[]',       -- the last few rounds' one-line results, for the next round
    note          TEXT NOT NULL DEFAULT '',         -- why the last probe was refused, for the next round
    event_cursor  INTEGER NOT NULL DEFAULT 0,
    steer_seen    INTEGER NOT NULL DEFAULT 0,
    updated_at    TEXT NOT NULL,
    PRIMARY KEY (task_id, step_id)
);
