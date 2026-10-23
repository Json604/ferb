-- L30 (perf plan 2026-10-23, Task 4): FERB's resource log. Every 5 minutes the daemon writes one row of numbers
-- (ferb/daemon/resources.py), kept `[retention] resource_days` (30) and shown per day by `ferb stats`. Numbers only:
-- never content, arguments or paths; work_json names trail sources, flush reasons, span kinds and the busiest
-- processes' executable basenames (redacted). Safe to apply again (like 0043, 0044).
CREATE TABLE IF NOT EXISTS resource_samples (
    id              INTEGER PRIMARY KEY,
    at              TEXT NOT NULL,      -- the window's end, ISO 8601 with offset
    day             TEXT NOT NULL,      -- local date (YYYY-MM-DD): the unit of `ferb stats` and of retention
    window_s        REAL NOT NULL,      -- seconds the window covers (monotonic clock: stops while the Mac sleeps)
    cpu_s           REAL NOT NULL,      -- the daemon's user + system CPU seconds in the window
    footprint       INTEGER,            -- the daemon's physical footprint, bytes; NULL: couldn't be read
    peak_footprint  INTEGER,            -- its lifetime maximum
    child_count     INTEGER NOT NULL,   -- the daemon's descendants at the window's end
    child_footprint INTEGER NOT NULL,
    child_cpu_s     REAL NOT NULL,      -- their CPU in the window, ended ones included
    app_cpu_s       REAL,               -- FERB.app's other processes; NULL: not running
    app_footprint   INTEGER,
    db_bytes        INTEGER,            -- the database and its WAL
    work_json       TEXT NOT NULL       -- the window's work counts
);
CREATE INDEX IF NOT EXISTS resource_samples_day ON resource_samples(day);
