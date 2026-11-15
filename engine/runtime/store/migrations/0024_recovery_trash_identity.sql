-- F4 (S1) fix: a trash record names the very item FERB put in the Trash (its device and inode, read right after the
-- move), so a restore never takes out a different item that later got the same name there. Snapshot records
-- (kind 'snapshot': original '/', saved_to the snapshot's name) need no new column. ADD COLUMN: no table rebuild.
ALTER TABLE recovery ADD COLUMN saved_dev INTEGER;
ALTER TABLE recovery ADD COLUMN saved_ino INTEGER;
