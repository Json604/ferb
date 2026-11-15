-- ENG-8, ENG-10, ENG-11 (stage 2): the conversation as the engine keeps it, one row per entry (ferb-core chat_entry),
-- shared by every client: FERB.app and the phones read pages, changes after a cursor and their own marks from here.
-- Kept forever with the rest of the chat (STORE-401, cleanup.PROTECTED); nothing here is ever hard-deleted (a deleted
-- entry is a tombstone revision). chat_history (0062, APP-805) stays as it is; its rows are carried over below.
--
-- chat_streams: one row per conversation, the counters bumped inside the engine's write transactions
--   conversation_id  the conversation
--   epoch            random per stream (32 hex): a client's cursor from another epoch reads from the start
--   last_seq         the conversation's change counter: the highest seq given (creation and every revision)
--   last_pos         the highest pos used; the engine's own entries start at POS_BASE = 4294967296 (2^32)
--   low_pos          the lowest pos used; imported older history always goes below it, so pos is set once, unique,
--                    and display order by pos is chronological across imports
--
-- chat_entries: every field of ferb-core chat_entry::Entry, and how the row came to be
--   id               the entry's id ('h:' || chat_history.id for an imported row)
--   conversation_id  its conversation
--   pos              creation order in the conversation, set once
--   seq              the conversation's change number at this revision (a row holds only its latest revision)
--   rev              1 at creation, +1 at each revision
--   kind             message | plan | answer | question | notice | post | error | legacy
--   state            open | done | failed | cancelled | interrupted | answered | declined | expired | merged
--   open             1 while it waits on FERB or the owner
--   request_id       the message it belongs to (chat.send's request id)
--   task_id          its task
--   approval_id      its question's approval
--   ref              the id FERB gave a notice or post (a notice's id, reminder:<timer id>); an imported row's ref
--   answers          an answer's message entry
--   placement        inline | below (answers and errors)
--   origin           who the words are from
--   client_id        the client that sent a message
--   submission_id    that client's own id for the submission
--   journal_id       the journal row the entry was made from, when known
--   created_at       when it was made (ISO 8601 with offset)
--   updated_at       its latest revision's time
--   day              the Mac's local day of created_at (YYYY-MM-DD)
--   body_json        its redacted body (JSON object; a legacy entry's is {"kind": ..., "body": <the app's bytes>})
--   deleted          1 for a tombstone (body emptied, row kept)
--   imported_from    NULL for the engine's own entries; 'chat_history' for rows carried over here; 'archive' for the
--                    one-time import of the pre-Rust archive
--
-- chat_submissions: each accepted chat.send by its client's own key, so a repeat returns the same acceptance
--   client_id, submission_id  the client and its id for the submission (the key)
--   conversation_id, request_id, entry_id, seq  the acceptance: where the message went, its entry and that entry's seq
--   text_sha256      SHA-256 (hex) of the message's REDACTED text: the same key with other words is a conflict
--   accepted_at      when it was accepted
--
-- chat_clients: each client that has talked to the engine
--   client_id, first_seen, last_seen
--
-- chat_client_marks: per client and conversation, how far it has received and read (each only moves forward)
--   client_id, conversation_id
--   delivered_seq    the highest seq the client holds
--   read_pos         the highest pos the owner has read there
--   updated_at       when a mark last moved
--
-- chat_entries_from: one row, the journal's highest seq when this migration ran (absent when the journal was empty:
-- then every journal row is newer). The engine's journal backstop reads only journal rows after it; older chat is
-- the carried chat_history.
--   singleton        always 1
--   journal_seq      journal.seq at the migration
CREATE TABLE IF NOT EXISTS chat_streams (
    conversation_id TEXT PRIMARY KEY,
    epoch           TEXT NOT NULL,
    last_seq        INTEGER NOT NULL,
    last_pos        INTEGER NOT NULL,
    low_pos         INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS chat_entries (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL,
    pos             INTEGER NOT NULL,
    seq             INTEGER NOT NULL,
    rev             INTEGER NOT NULL,
    kind            TEXT NOT NULL,
    state           TEXT NOT NULL,
    open            INTEGER NOT NULL,
    request_id      TEXT,
    task_id         TEXT,
    approval_id     TEXT,
    ref             TEXT,
    answers         TEXT,
    placement       TEXT,
    origin          TEXT,
    client_id       TEXT,
    submission_id   TEXT,
    journal_id      TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    day             TEXT NOT NULL,
    body_json       TEXT NOT NULL,
    deleted         INTEGER NOT NULL DEFAULT 0,
    imported_from   TEXT,
    UNIQUE (conversation_id, pos),
    UNIQUE (conversation_id, seq)
);
CREATE INDEX IF NOT EXISTS chat_entries_day ON chat_entries (conversation_id, day, pos);
CREATE INDEX IF NOT EXISTS chat_entries_open ON chat_entries (conversation_id, open) WHERE open = 1;
CREATE INDEX IF NOT EXISTS chat_entries_request ON chat_entries (request_id);
CREATE INDEX IF NOT EXISTS chat_entries_task ON chat_entries (task_id);
CREATE INDEX IF NOT EXISTS chat_entries_approval ON chat_entries (approval_id);
CREATE INDEX IF NOT EXISTS chat_entries_ref ON chat_entries (conversation_id, ref);
CREATE TABLE IF NOT EXISTS chat_submissions (
    client_id       TEXT NOT NULL,
    submission_id   TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    request_id      TEXT NOT NULL,
    entry_id        TEXT NOT NULL,
    seq             INTEGER NOT NULL,
    text_sha256     TEXT NOT NULL,
    accepted_at     TEXT NOT NULL,
    PRIMARY KEY (client_id, submission_id)
);
CREATE TABLE IF NOT EXISTS chat_clients (
    client_id  TEXT PRIMARY KEY,
    first_seen TEXT NOT NULL,
    last_seen  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS chat_client_marks (
    client_id       TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    delivered_seq   INTEGER NOT NULL DEFAULT 0,
    read_pos        INTEGER NOT NULL DEFAULT 0,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (client_id, conversation_id)
);
CREATE TABLE IF NOT EXISTS chat_entries_from (
    singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
    journal_seq INTEGER NOT NULL
);
INSERT INTO chat_entries_from (singleton, journal_seq)
SELECT 1, top FROM (SELECT MAX(seq) AS top FROM journal) WHERE top IS NOT NULL;
-- The app's saved chat becomes legacy entries of the current conversation, in (at, id) order, just below POS_BASE;
-- the body keeps the app's bytes verbatim inside the LegacyBody shape.
INSERT INTO chat_entries (id, conversation_id, pos, seq, rev, kind, state, open, ref, created_at, updated_at, day,
                          body_json, deleted, imported_from)
SELECT 'h:' || id,
       COALESCE((SELECT conversation_id FROM current_conversation WHERE singleton = 1), 'owner'),
       4294967296 - (COUNT(*) OVER ()) + (ROW_NUMBER() OVER (ORDER BY at, id)) - 1,
       ROW_NUMBER() OVER (ORDER BY at, id),
       1, 'legacy', 'done', 0, ref, at, updated_at, day,
       '{"kind": ' || json_quote(kind) || ', "body": ' || body_json || '}',
       0, 'chat_history'
FROM chat_history;
INSERT INTO chat_streams (conversation_id, epoch, last_seq, last_pos, low_pos)
SELECT COALESCE((SELECT conversation_id FROM current_conversation WHERE singleton = 1), 'owner'),
       lower(hex(randomblob(16))), n, 4294967295, 4294967296 - n
FROM (SELECT COUNT(*) AS n FROM chat_history) WHERE n > 0;
