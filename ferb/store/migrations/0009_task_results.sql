-- Build step 14a: what FERB told (or will tell) the owner about each request's task. A task keeps running when
-- `ferb chat` closes; its answer waits here (shown = 0) and chat shows it next time (then shown = 1).
CREATE TABLE task_results (
    task_id   TEXT PRIMARY KEY,
    at        TEXT NOT NULL,
    status    TEXT NOT NULL,
    goal      TEXT NOT NULL,
    answer    TEXT NOT NULL,
    shown     INTEGER NOT NULL DEFAULT 0
);
