-- Fix wave 3, memory review I3 and M5: the sleep service works on what changed.
-- learning_card_inputs: the digest of what each episode card was last read from (the episode as recorded, the owner's
-- latest verdict, the card composer); a card is read again only when that digest changes. A failed read keeps its
-- error and waits for a change or `ferb memory consolidate`.
CREATE TABLE learning_card_inputs (task_id TEXT PRIMARY KEY, inputs TEXT NOT NULL, status TEXT NOT NULL, error TEXT);
-- learning_jobs.inputs: the group of episodes a consolidation job was given; a failed job is tried again when its group
-- changes (or on `ferb memory consolidate`), never on every idle hour.
ALTER TABLE learning_jobs ADD COLUMN inputs TEXT;
-- distill_runs.inputs: the day summary a failed distillation read; it is tried again when that summary changes (a
-- forget rebuilt it) or on `ferb memory distill DAY`.
ALTER TABLE distill_runs ADD COLUMN inputs TEXT;
