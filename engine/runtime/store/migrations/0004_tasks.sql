-- Build step 8: the task tree (section 7), persisted so a restart knows what was running.
CREATE TABLE tasks (
    id           TEXT PRIMARY KEY,
    tree_id      TEXT NOT NULL,              -- the request's root task
    parent_id    TEXT,
    request_id   TEXT NOT NULL,
    goal         TEXT NOT NULL,
    identity     TEXT NOT NULL,
    role         TEXT NOT NULL,
    scopes_json  TEXT NOT NULL,
    depends_json TEXT NOT NULL DEFAULT '[]',
    status       TEXT NOT NULL CHECK (status IN ('planned', 'running', 'done', 'failed', 'unknown', 'cancelled',
                                                 'needs_user', 'skipped', 'interrupted')),
    outcome_json TEXT NOT NULL DEFAULT '{}',
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE INDEX tasks_tree ON tasks(tree_id);
CREATE INDEX tasks_status ON tasks(status);
