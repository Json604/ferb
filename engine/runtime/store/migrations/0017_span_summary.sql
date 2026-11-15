-- F3 (P26): a tool call's one-line result summary, composed by code from its result (ferb/trace/lines.py).
ALTER TABLE spans ADD COLUMN summary TEXT;
