-- MP1–MP7: immutable derived bodies plus explicit lineage. State/counters are derived metadata.
CREATE TABLE learning_nodes (
 id TEXT PRIMARY KEY, kind TEXT NOT NULL, key TEXT NOT NULL, body_json TEXT NOT NULL,
 origin TEXT NOT NULL, made_by TEXT NOT NULL, recorded_at TEXT NOT NULL,
 valid_from TEXT NOT NULL, valid_to TEXT, supersedes TEXT, batch_id TEXT,
 state TEXT NOT NULL DEFAULT 'candidate', confirmed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX learning_kind_key ON learning_nodes(kind,key,valid_to);
CREATE TABLE learning_sources (
 node_id TEXT NOT NULL REFERENCES learning_nodes(id) ON DELETE CASCADE,
 source_ref TEXT NOT NULL, PRIMARY KEY(node_id,source_ref)
);
CREATE INDEX learning_source_ref ON learning_sources(source_ref);
CREATE TABLE learning_forgotten (id TEXT PRIMARY KEY, at TEXT NOT NULL);
CREATE TABLE learning_changes (
 seq INTEGER PRIMARY KEY, batch_id TEXT NOT NULL, node_id TEXT NOT NULL,
 action TEXT NOT NULL, prior_id TEXT, at TEXT NOT NULL, rolled_back INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE learning_jobs (source_ref TEXT PRIMARY KEY, status TEXT NOT NULL, at TEXT NOT NULL, error TEXT);
CREATE TABLE learning_receipts (
 procedure_id TEXT NOT NULL REFERENCES learning_nodes(id) ON DELETE CASCADE,
 episode_id TEXT NOT NULL REFERENCES learning_nodes(id) ON DELETE CASCADE,
 slots_json TEXT NOT NULL, successful INTEGER NOT NULL, at TEXT NOT NULL,
 PRIMARY KEY(procedure_id,episode_id)
);
CREATE TABLE learning_vectors (
 node_id TEXT NOT NULL REFERENCES learning_nodes(id) ON DELETE CASCADE,
 model TEXT NOT NULL, vector_blob BLOB NOT NULL, month TEXT NOT NULL, text_hash TEXT NOT NULL,
 PRIMARY KEY(node_id,model)
);
CREATE INDEX learning_vectors_partition ON learning_vectors(model,month);
CREATE TABLE learning_embedding_evals (
 id TEXT PRIMARY KEY, model TEXT NOT NULL, dataset_hash TEXT NOT NULL, at TEXT NOT NULL,
 result_json TEXT NOT NULL, adopted INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE learning_digests (day TEXT PRIMARY KEY, ids_json TEXT NOT NULL, at TEXT NOT NULL);
CREATE VIRTUAL TABLE learning_fts USING fts5(id UNINDEXED, text, tokenize="unicode61 categories 'L* N* Co M*'");
CREATE VIRTUAL TABLE learning_titles USING fts5(id UNINDEXED, text, tokenize='trigram');
CREATE TRIGGER learning_body_immutable BEFORE UPDATE OF body_json, origin, made_by, recorded_at ON learning_nodes
BEGIN
 SELECT RAISE(ABORT, 'derived content is versioned, never overwritten');
END;
CREATE TRIGGER learning_delete_indexes AFTER DELETE ON learning_nodes
BEGIN
 DELETE FROM learning_fts WHERE id=old.id;
 DELETE FROM learning_titles WHERE id=old.id;
END;
