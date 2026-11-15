-- F3 (MP2, S3): every journal row says where its content came from: comma-joined origins (ferb/core/origin.py).
-- Older rows stay NULL; readers derive their origin from the recorded kind and data (journal_origin), never a guess.
ALTER TABLE journal ADD COLUMN origin TEXT;
-- Lookups by kind (a chat turn's request, clarify and schedule origins) read one kind in order, not the whole journal.
CREATE INDEX IF NOT EXISTS journal_kind ON journal(kind, seq);
