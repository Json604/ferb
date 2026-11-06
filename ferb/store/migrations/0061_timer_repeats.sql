-- DMN-548 (2026-11-06): a fired timer repeats at most once. The done alert's Repeat records which timer it started, so
-- a second Repeat (the notch's alert, then the chat's card) returns that one instead of starting another. A table of
-- its own beside `timers` (re-runnable as 0058 and 0060 are): a timer made before has no row and can be repeated once.
-- Both ends go with their timer rows (STORE-404's retention deletes timers; the rows cascade).
CREATE TABLE IF NOT EXISTS timer_repeats (
    timer_id  TEXT PRIMARY KEY REFERENCES timers(id) ON DELETE CASCADE,
    repeat_id TEXT NOT NULL UNIQUE REFERENCES timers(id) ON DELETE CASCADE
);
