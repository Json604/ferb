-- L36 §3: which version of the content-only search text `learning_fts` holds (`learning.SEARCH_TEXT_VERSION`), and
-- while a re-index runs, the id of the last node it re-indexed (`through`, in id order: never a rowid, which VACUUM may
-- renumber; NULL once every node holds that version). Empty here: no table is rewritten in this migration. The
-- daemon's start re-indexes every node by its content (`learning.reindex_search`) in a worker thread, in short batches
-- each its own transaction, so a stop or a power loss keeps what was done and the next start finishes the rest.
CREATE TABLE IF NOT EXISTS learning_search_version (version TEXT NOT NULL, through TEXT);
