-- P1/D10 (perf plan 2026-10-23, Task 3): the perception batcher keeps the unsent trail rows in memory and reads only
-- rows after the last seq it saw. A row deleted (retention purge, forget) or whose data_json is rewritten (the D14/P8
-- seven-day text expiry) changes what it holds: these triggers count such changes on any connection, and on a new
-- count it reads the rows it holds again (never the whole trail). A new row needs no count (it comes after the last
-- seq); relevance and sensitive never reach Jev.
CREATE TABLE trail_changes (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    n  INTEGER NOT NULL
);
INSERT INTO trail_changes VALUES (1, 0);
CREATE TRIGGER trail_changes_delete AFTER DELETE ON trail
BEGIN
    UPDATE trail_changes SET n = n + 1 WHERE id = 1;
END;
CREATE TRIGGER trail_changes_data AFTER UPDATE OF data_json ON trail
BEGIN
    UPDATE trail_changes SET n = n + 1 WHERE id = 1;
END;
