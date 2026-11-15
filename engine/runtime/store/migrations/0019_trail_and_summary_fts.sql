-- F3 (D9): raw activity and daily summaries are retrievable by content as well as by date. Same tokenizer as memory
-- (0014: Devanagari words stay whole). trail_fts holds the decoded text values of a trail row's JSON (json_tree), not
-- the raw JSON: the journal stores \uXXXX escapes, and keys would match every row. summary_fts holds only a summary's
-- content: window titles, per-hour apps and sites, FERB task summaries, a narrative's text, and the KEYS of sites,
-- folders and apps_min (hosts, paths and app names are keys in the digest). The fixed values of every row (made_by,
-- day, rebuilt, the duration wording) are not indexed. A row that is not valid JSON indexes '' instead of failing.
-- Triggers keep both indexes in step with inserts, updates and deletes, so a purged or forgotten row leaves the index;
-- secure-delete makes FTS5 zero the removed term bytes instead of leaving them until a merge (as secure_delete does
-- for the tables, db.py). Needs SQLite 3.42+. Journal rows are never updated, so summary_fts has no update trigger.
CREATE VIRTUAL TABLE trail_fts USING fts5(text, tokenize="unicode61 categories 'L* N* Co M*'");
INSERT INTO trail_fts(trail_fts, rank) VALUES('secure-delete', 1);
CREATE TRIGGER trail_fts_ai AFTER INSERT ON trail BEGIN
    INSERT INTO trail_fts(rowid, text) VALUES (new.seq, coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END)) WHERE type = 'text'), ''));
END;
CREATE TRIGGER trail_fts_ad AFTER DELETE ON trail BEGIN
    DELETE FROM trail_fts WHERE rowid = old.seq;
END;
CREATE TRIGGER trail_fts_au AFTER UPDATE OF data_json ON trail BEGIN
    DELETE FROM trail_fts WHERE rowid = old.seq;
    INSERT INTO trail_fts(rowid, text) VALUES (new.seq, coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END)) WHERE type = 'text'), ''));
END;
INSERT INTO trail_fts(rowid, text) SELECT t.seq, coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END)) WHERE type = 'text'), '') FROM trail t;

CREATE VIRTUAL TABLE summary_fts USING fts5(text, tokenize="unicode61 categories 'L* N* Co M*'");
INSERT INTO summary_fts(summary_fts, rank) VALUES('secure-delete', 1);
CREATE TRIGGER summary_fts_ai AFTER INSERT ON journal
WHEN new.kind IN ('trail.day_summary', 'trail.day_narrative') BEGIN
    INSERT INTO summary_fts(rowid, text) VALUES (new.seq,
            coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END), '$.windows') WHERE type = 'text'), '')
            || ' ' || coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END), '$.hours') WHERE type = 'text'), '')
            || ' ' || coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END), '$.ferb.tasks') WHERE type = 'text' AND key = 'summary'), '')
            || ' ' || coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END), '$.text') WHERE type = 'text'), '')
            || ' ' || coalesce((SELECT group_concat(key, ' ') FROM json_each((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END), '$.sites') WHERE json_type((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END), '$.sites') = 'object'), '')
            || ' ' || coalesce((SELECT group_concat(key, ' ') FROM json_each((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END), '$.folders') WHERE json_type((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END), '$.folders') = 'object'), '')
            || ' ' || coalesce((SELECT group_concat(key, ' ') FROM json_each((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END), '$.apps_min') WHERE json_type((CASE WHEN json_valid(new.data_json) THEN new.data_json ELSE '{}' END), '$.apps_min') = 'object'), ''));
END;
CREATE TRIGGER summary_fts_ad AFTER DELETE ON journal
WHEN old.kind IN ('trail.day_summary', 'trail.day_narrative') BEGIN
    DELETE FROM summary_fts WHERE rowid = old.seq;
END;
INSERT INTO summary_fts(rowid, text)
    SELECT t.seq,
            coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END), '$.windows') WHERE type = 'text'), '')
            || ' ' || coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END), '$.hours') WHERE type = 'text'), '')
            || ' ' || coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END), '$.ferb.tasks') WHERE type = 'text' AND key = 'summary'), '')
            || ' ' || coalesce((SELECT group_concat(value, ' ') FROM json_tree((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END), '$.text') WHERE type = 'text'), '')
            || ' ' || coalesce((SELECT group_concat(key, ' ') FROM json_each((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END), '$.sites') WHERE json_type((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END), '$.sites') = 'object'), '')
            || ' ' || coalesce((SELECT group_concat(key, ' ') FROM json_each((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END), '$.folders') WHERE json_type((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END), '$.folders') = 'object'), '')
            || ' ' || coalesce((SELECT group_concat(key, ' ') FROM json_each((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END), '$.apps_min') WHERE json_type((CASE WHEN json_valid(t.data_json) THEN t.data_json ELSE '{}' END), '$.apps_min') = 'object'), '')
    FROM journal t WHERE t.kind IN ('trail.day_summary', 'trail.day_narrative');
