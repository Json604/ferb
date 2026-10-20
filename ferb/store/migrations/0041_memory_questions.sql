-- Fix wave 3, re-review F5 (CLAUDE.md rule 9: a question never expires while it waits): FERB's own Yes/No questions
-- about memory (keep FERB's wording of a memory; forget an item) survive a restart. Nothing acts before the owner's Yes.
CREATE TABLE memory_questions (
    approval_id TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,      -- 'confirm', 'forget:memory' or 'forget:learning'
    target      TEXT NOT NULL,      -- the memory item or learned node it is about
    reason      TEXT NOT NULL,      -- the question as the owner saw it
    request_id  TEXT NOT NULL,
    at          TEXT NOT NULL
);
