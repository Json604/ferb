-- Decision 22 (owner 2026-10-31): a schedule can end on a date the owner's words gave it ("until Nov 3", "for 2
-- weeks"); code works out the time and stores it here, and no run is booked after it. A table of its own beside
-- `schedules` (re-runnable as 0050 and 0051 are): a schedule made before, or without an end, has no row and runs
-- until stopped, as before.
CREATE TABLE IF NOT EXISTS schedule_ends (
    schedule_id TEXT PRIMARY KEY REFERENCES schedules(id) ON DELETE CASCADE,
    ends_at     TEXT NOT NULL            -- UTC ISO time; no run after it
);
