-- Step 8b (notes/memory.md Tier 3, decision 1): each finished task keeps the worker's `how` note, a few lines on the
-- steps that worked, beside its permanent episode record, so the memory step can index and show it. A table of its own
-- (written with the episode, read with it): a task that didn't finish, or an older episode, has no row.
CREATE TABLE IF NOT EXISTS episode_how (
    task_id TEXT PRIMARY KEY REFERENCES episodes(task_id) ON DELETE CASCADE,
    how     TEXT NOT NULL
);
