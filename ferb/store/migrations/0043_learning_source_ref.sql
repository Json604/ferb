-- Plan 2026-10-23 Task 1 (L28, C1): the store clean-out (`Learning.prune_card_sources`, run by setup) drops this
-- index (migration 0030) while it deletes and makes it again after. A power loss in between leaves it missing: the
-- first start of this code makes it again, so forgets and missing-source lookups never scan the whole table.
CREATE INDEX IF NOT EXISTS learning_source_ref ON learning_sources(source_ref);
