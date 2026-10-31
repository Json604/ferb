-- Step 9b (notes/memory.md decisions 8, 17; fix list "Old chat searchable by meaning"): the one fused memory search.
--
-- Embeddings of what search serves, by the local model (ferb/memory/embeddings.py): a fact (`memory:<id>`), a learned
-- node (its id: an episode card with its how-note, a playbook, a day summary) or a conversation entry
-- (`journal:<id>`). Vectors are normalized int8 with their sign bits; computed in the background from redacted text.
CREATE TABLE IF NOT EXISTS recall_vectors (
    doc       TEXT NOT NULL,
    model     TEXT NOT NULL,
    kind      TEXT NOT NULL,                -- 'fact' | a learning node kind | 'chat'
    at        TEXT NOT NULL,
    text_hash TEXT NOT NULL,
    vector    BLOB NOT NULL,
    bits      BLOB NOT NULL,
    PRIMARY KEY (doc, model)
);
-- Small named counters and watermarks: 'vectors' (bumped by every change to recall_vectors, so a reader's cached copy
-- knows to reload) and 'chat_seq' (the last journal row the conversation index read).
CREATE TABLE IF NOT EXISTS recall_state (name TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT OR IGNORE INTO recall_state VALUES ('vectors', '0');
CREATE TRIGGER IF NOT EXISTS recall_vectors_added AFTER INSERT ON recall_vectors BEGIN
    UPDATE recall_state SET value = CAST(value AS INTEGER) + 1 WHERE name = 'vectors';
END;
CREATE TRIGGER IF NOT EXISTS recall_vectors_removed AFTER DELETE ON recall_vectors BEGIN
    UPDATE recall_state SET value = CAST(value AS INTEGER) + 1 WHERE name = 'vectors';
END;
-- A forgotten fact or a deleted learned node takes its vector with it (a vector is derived from its text)
CREATE TRIGGER IF NOT EXISTS recall_vectors_item_gone AFTER DELETE ON memory_items BEGIN
    DELETE FROM recall_vectors WHERE doc = 'memory:' || old.id;
END;
CREATE TRIGGER IF NOT EXISTS recall_vectors_node_gone AFTER DELETE ON learning_nodes BEGIN
    DELETE FROM recall_vectors WHERE doc = old.id;
END;
-- The conversation, searchable: the owner's own words of each message, FERB's replies, answers, questions and notices
-- (`journal:<id>`, its kind, its chat and its time beside the text). Journal rows are append-only; the index is
-- filled from `recall_state.chat_seq` on.
CREATE VIRTUAL TABLE IF NOT EXISTS chat_fts USING fts5(doc UNINDEXED, kind UNINDEXED, session UNINDEXED,
    at UNINDEXED, request UNINDEXED, text, tokenize="unicode61 categories 'L* N* Co M*'");
INSERT INTO chat_fts(chat_fts, rank) VALUES('secure-delete', 1);
-- A journal entry that goes (a notice cleared by the seven-day purge or a forget) takes its index copy and vector along
CREATE TRIGGER IF NOT EXISTS recall_journal_gone AFTER DELETE ON journal BEGIN
    DELETE FROM chat_fts WHERE doc = 'journal:' || old.id;
    DELETE FROM recall_vectors WHERE doc = 'journal:' || old.id;
END;
-- Decision 17: one of the owner's own facts that looks over but has no end date of theirs is set aside, not ended:
-- FERB stops using it, keeps it as history, and the owner restores it by saying it still holds.
CREATE TABLE IF NOT EXISTS memory_set_aside (
    item_id TEXT PRIMARY KEY,
    at      TEXT NOT NULL,
    why     TEXT NOT NULL DEFAULT ''
);
