CREATE TABLE task_plans (
 task_id TEXT PRIMARY KEY, plan TEXT NOT NULL, request TEXT NOT NULL, conversation TEXT,
 status TEXT NOT NULL DEFAULT 'running', approved INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE plan_steps (
 task_id TEXT NOT NULL REFERENCES task_plans(task_id), step_id TEXT NOT NULL,
 status TEXT NOT NULL DEFAULT 'pending', output TEXT NOT NULL DEFAULT '{}',
 PRIMARY KEY(task_id, step_id)
);
CREATE TABLE plan_events (
 seq INTEGER PRIMARY KEY AUTOINCREMENT, event TEXT NOT NULL, payload TEXT NOT NULL
);
CREATE TABLE standing_permissions (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL, scope TEXT NOT NULL, approval_id TEXT NOT NULL,
 active INTEGER NOT NULL DEFAULT 1
);
