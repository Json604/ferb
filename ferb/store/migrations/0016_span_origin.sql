-- F3 (S3): where a tool call's result came from (comma-joined origins, ferb/core/origin.py). NULL: nothing was
-- returned (the call failed before a result) or the span predates F3.
ALTER TABLE spans ADD COLUMN origin TEXT;
