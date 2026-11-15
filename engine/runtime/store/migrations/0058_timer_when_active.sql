-- Orchestration decision 17 (owner 2026-10-30): reminders and schedules can fire when the owner is at the Mac ("when
-- you're back at your Mac"). A timer with a row here is not claimed while the owner is away (the owner-input tap's
-- idle/active signal); with `return_after`, only in an active stretch that began after that time. A table of its own
-- beside `timers` (re-runnable as 0052 is): every timer made before, or without it, fires at its time as before.
CREATE TABLE IF NOT EXISTS timer_when_active (
    timer_id     TEXT PRIMARY KEY REFERENCES timers(id) ON DELETE CASCADE,
    return_after TEXT                    -- UTC ISO time, or NULL
);
