-- F4 (V4): "done but unverified" is its own status, distinct from "failed", so stats and the owner can tell "didn't
-- do it" from "couldn't check it". SQLite can't alter a CHECK in place; rebuild the tasks table's constraint.
-- (Follow the existing 0004_tasks.sql column list exactly; add 'done_unverified' to the CHECK.)
-- No later migration changed tasks, so 0004's columns, defaults and indexes are the current shape. Row ids are kept:
-- continuity orders a conversation's tasks by tasks.rowid. task_context references tasks(id), and the runner applies
-- this file inside one transaction with foreign keys on, where PRAGMA foreign_keys is a no-op and DROP TABLE's
-- implicit delete would fail on the referencing rows (a deferred check would still fail at COMMIT). So its rows are
-- held aside, cleared, and put back once the new table has the old name; every constraint stays enforced throughout.
CREATE TABLE tasks_new (
    id           TEXT PRIMARY KEY,
    tree_id      TEXT NOT NULL,              -- the request's root task
    parent_id    TEXT,
    request_id   TEXT NOT NULL,
    goal         TEXT NOT NULL,
    identity     TEXT NOT NULL,
    role         TEXT NOT NULL,
    scopes_json  TEXT NOT NULL,
    depends_json TEXT NOT NULL DEFAULT '[]',
    status       TEXT NOT NULL CHECK (status IN ('planned', 'running', 'done', 'done_unverified', 'failed', 'unknown',
                                                 'cancelled', 'needs_user', 'skipped', 'interrupted')),
    outcome_json TEXT NOT NULL DEFAULT '{}',
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
INSERT INTO tasks_new (rowid, id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, depends_json,
                       status, outcome_json, created_at, updated_at)
    SELECT rowid, id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, depends_json,
           status, outcome_json, created_at, updated_at FROM tasks;
CREATE TABLE task_context_hold AS
    SELECT rowid AS kept_rowid, task_id, conversation_id, spec_json, owner_words, prior_task, request_payload
    FROM task_context;
DELETE FROM task_context;
DROP TABLE tasks;
ALTER TABLE tasks_new RENAME TO tasks;
CREATE INDEX tasks_tree ON tasks(tree_id);
CREATE INDEX tasks_status ON tasks(status);
INSERT INTO task_context (rowid, task_id, conversation_id, spec_json, owner_words, prior_task, request_payload)
    SELECT kept_rowid, task_id, conversation_id, spec_json, owner_words, prior_task, request_payload
    FROM task_context_hold;
DROP TABLE task_context_hold;
