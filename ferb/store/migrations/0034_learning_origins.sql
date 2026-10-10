-- Trust rank and privacy are separate: an outside message can also contain the owner's private data.
ALTER TABLE learning_nodes ADD COLUMN origins_json TEXT NOT NULL DEFAULT '[]';
-- Legacy derived rows did not retain all privacy labels. Preserve trust and conservatively retain privacy.
UPDATE learning_nodes SET origins_json = CASE WHEN origin='owner' THEN json_array(origin)
 ELSE json_array(origin,'private') END;
DROP TRIGGER learning_body_immutable;
CREATE TRIGGER learning_body_immutable BEFORE UPDATE OF body_json,origin,origins_json,made_by,recorded_at ON learning_nodes
BEGIN
 SELECT RAISE(ABORT, 'derived content is versioned, never overwritten');
END;
