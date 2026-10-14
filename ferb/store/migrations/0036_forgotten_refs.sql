-- Fix wave 3, memory review C1: durable forget tombstones. One row per ref an explicit owner forget deleted
-- (journal:, trail:, memory: and episode: ids, and learning node ids): the id only, never what it held. A missing
-- source with a tombstone was forgotten; one without was removed by retention (aged out). Rows a forget already
-- recorded elsewhere are carried over.
CREATE TABLE forgotten_refs (ref TEXT PRIMARY KEY, at TEXT NOT NULL);
INSERT OR IGNORE INTO forgotten_refs (ref, at) SELECT source_id, '' FROM conversation_forgotten_sources;
INSERT OR IGNORE INTO forgotten_refs (ref, at) SELECT id, at FROM learning_forgotten WHERE id LIKE 'm-%';
