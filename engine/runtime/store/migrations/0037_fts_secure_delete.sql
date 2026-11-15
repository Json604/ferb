-- Fix wave 3, memory review I4/M9: a forget never rebuilds a full-text index on the event loop. FTS5's secure-delete
-- (as trail_fts and summary_fts already have, 0019) zeroes a deleted row's term bytes when it is deleted, so the
-- per-forget 'rebuild' of these indexes is gone. One rebuild here clears what earlier deletions left in them.
INSERT INTO learning_fts(learning_fts, rank) VALUES('secure-delete', 1);
INSERT INTO learning_titles(learning_titles, rank) VALUES('secure-delete', 1);
INSERT INTO memory_fts(memory_fts, rank) VALUES('secure-delete', 1);
INSERT INTO learning_fts(learning_fts) VALUES('rebuild');
INSERT INTO learning_titles(learning_titles) VALUES('rebuild');
INSERT INTO memory_fts(memory_fts) VALUES('rebuild');
