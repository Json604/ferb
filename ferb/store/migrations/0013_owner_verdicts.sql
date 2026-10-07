-- Owner's pass/fail verdict on a task, collected by `ferb verdict`. Acceptance is the owner's word, never inferred.
CREATE TABLE owner_verdicts (
    seq      INTEGER PRIMARY KEY AUTOINCREMENT,
    id       TEXT NOT NULL,
    task_id  TEXT NOT NULL,
    verdict  TEXT NOT NULL CHECK (verdict IN ('pass', 'fail')),
    note     TEXT NOT NULL DEFAULT '',
    at       TEXT NOT NULL
);
CREATE INDEX owner_verdicts_task ON owner_verdicts(task_id, seq);
