-- Build step 2: action ledger (section 7 step 9, section 19). A row is written BEFORE a consequential action runs.
CREATE TABLE action_ledger (
    id            TEXT PRIMARY KEY,          -- also the idempotency key sent to drivers
    task_id       TEXT NOT NULL,
    capability    TEXT NOT NULL,
    target_json   TEXT NOT NULL,
    payload_hash  TEXT NOT NULL,
    state         TEXT NOT NULL CHECK (state IN ('intended', 'executing', 'done', 'failed', 'unknown')),
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    reconciled    INTEGER NOT NULL DEFAULT 0,
    evidence_json TEXT NOT NULL DEFAULT '[]'
);
CREATE INDEX action_ledger_same_action ON action_ledger(task_id, capability, payload_hash, state);
CREATE INDEX action_ledger_state ON action_ledger(state);
