-- Step 12a (notes/storage.md target design 4, fix list). Applied once at the first start of this code.
--
-- 1. Chat replies stop carrying `source_refs` (the lists of every row their planner was shown; read only by the echo
--    scrub, removed in step 3): the lists come out of the rows already written. Journal rows are append-only, so the
--    trigger that refuses updates is dropped for this one rewrite and made again, unchanged, in the same transaction.
DROP TRIGGER journal_append_only;
UPDATE journal SET data_json = json_remove(data_json, '$.source_refs')
    WHERE kind IN ('chat.reply', 'chat.clarify') AND json_valid(data_json)
      AND json_type(data_json, '$.source_refs') IS NOT NULL;
CREATE TRIGGER journal_append_only BEFORE UPDATE ON journal
BEGIN
    SELECT RAISE(ABORT, 'journal is append-only');
END;
-- 2. One tombstone table for explicit forgets (`forgotten_refs`: an id or ref, never what it held). The learned-node and
--    conversation-source tombstones join it; `memory_forgotten` stays apart (it holds content fingerprints, not ids).
INSERT OR IGNORE INTO forgotten_refs (ref, at) SELECT id, at FROM learning_forgotten;
INSERT OR IGNORE INTO forgotten_refs (ref, at) SELECT source_id, '' FROM conversation_forgotten_sources;
DROP TABLE learning_forgotten;
DROP TABLE conversation_forgotten_sources;
-- 3. Tables no code reads or writes any more (`owner_questions`, `notification_rules`, `learning_receipts`,
--    `learning_jobs`) stay until the owner's OK'd clean-up drops them (decision 4; ferb/store/cleanup.py `DROPPED`).
-- 4. The retention pass ages decisions out by time.
CREATE INDEX IF NOT EXISTS decisions_at ON decisions(at);
