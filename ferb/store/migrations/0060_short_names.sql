-- T-1055 (owner D32, 2026-11-05): schedules, reminders and timers show a short name in every heading, written by the
-- planner when it makes them. Tables of their own beside `schedules` and `timers` (re-runnable as 0052 and 0058 are):
-- a row made before has no name and shows its instruction's first clause, cut by code; nothing renames it. A task's
-- name is kept in its spec (task_context.spec_json); a told result keeps the name it was told with (a scheduled run
-- the planner answered itself has no task), beside `task_results` (no foreign key: a result is replaced in place).
CREATE TABLE IF NOT EXISTS schedule_names (
    schedule_id TEXT PRIMARY KEY REFERENCES schedules(id) ON DELETE CASCADE,
    name        TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS timer_names (
    timer_id TEXT PRIMARY KEY REFERENCES timers(id) ON DELETE CASCADE,
    name     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS task_result_names (
    task_id TEXT PRIMARY KEY,
    name    TEXT NOT NULL
);
