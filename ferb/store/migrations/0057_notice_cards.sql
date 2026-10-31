-- Step 12c: a notice's typed cards (a memory import's examples and its Memory page button) kept with the notice, so
-- FERB.app shows them when it reads the notice from chat.status after being away, not only from the live event.
ALTER TABLE attention_notices ADD COLUMN cards_json TEXT NOT NULL DEFAULT '[]';
