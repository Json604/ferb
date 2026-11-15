-- F4 (S1): FERB's own file changes are recoverable for 7 days. Before FERB overwrites or edits a file it keeps a
-- clone (retention.recovery_days) and this record; a delete goes to the Trash with its original path kept durably
-- (the span that held it ages out at 30 days, so recovery must not depend on it). This is what makes an overwrite or
-- delete "not destructive" for F5's confirmations.
-- written_sha256: the file as FERB left it. A restore puts the copy back in place only while the file is still exactly
-- that; otherwise the copy goes beside it, so a change the owner made since is never overwritten.
CREATE TABLE recovery (
    id             TEXT PRIMARY KEY,
    task_id        TEXT,
    kind           TEXT NOT NULL,       -- clone | trash
    original       TEXT NOT NULL,       -- the path FERB changed or deleted
    saved_to       TEXT,                -- the clone's path under ~/.ferb/recovery (clone), or the Trash path (trash)
    reason         TEXT,                -- overwrite | edit | trash
    at             TEXT NOT NULL,
    written_sha256 TEXT                 -- clone: SHA-256 of the file after FERB's change (NULL: not known)
);
CREATE INDEX recovery_task ON recovery(task_id);
CREATE INDEX recovery_time ON recovery(at);
