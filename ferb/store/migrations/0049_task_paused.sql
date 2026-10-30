-- Owner decision 8 (2026-10-30): a task can be paused in place, waiting for the owner, so `paused` is a status of its
-- own; the dead `depends_json` column goes (dependencies live in the task's plan). 'skipped' and 'unknown' stay in the
-- CHECK only for rows an earlier build wrote; nothing writes them now. SQLite can't alter a CHECK in place: the table
-- is rebuilt as 0021 did (its notes apply: row ids kept, task_context's rows held aside and put back).
CREATE TABLE tasks_new (
    id           TEXT PRIMARY KEY,
    tree_id      TEXT NOT NULL,              -- the request's root task
    parent_id    TEXT,
    request_id   TEXT NOT NULL,
    goal         TEXT NOT NULL,
    identity     TEXT NOT NULL,
    role         TEXT NOT NULL,
    scopes_json  TEXT NOT NULL,
    status       TEXT NOT NULL CHECK (status IN ('planned', 'running', 'paused', 'done', 'done_unverified', 'failed',
                                                 'cancelled', 'needs_user', 'interrupted', 'skipped', 'unknown')),
    outcome_json TEXT NOT NULL DEFAULT '{}',
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
INSERT INTO tasks_new (rowid, id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status,
                       outcome_json, created_at, updated_at)
    SELECT rowid, id, tree_id, parent_id, request_id, goal, identity, role, scopes_json, status, outcome_json,
           created_at, updated_at FROM tasks;
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
