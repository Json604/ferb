-- F3 (D9): raw activity and daily summaries are retrievable by content as well as by date. Same tokenizer as memory
-- (0014: Devanagari words stay whole). Each index holds the decoded text values of the row's JSON (json_tree), not
-- the raw JSON: the journal stores \uXXXX escapes, and keys would match every row. Triggers keep both indexes in step
-- with inserts, updates and deletes, so a purged or forgotten row leaves the index.
CREATE VIRTUAL TABLE trail_fts USING fts5(text, tokenize="unicode61 categories 'L* N* Co M*'");
CREATE TRIGGER trail_fts_ai AFTER INSERT ON trail BEGIN
    INSERT INTO trail_fts(rowid, text)
        SELECT new.seq, coalesce(group_concat(value, ' '), '') FROM json_tree(new.data_json) WHERE type = 'text';
END;
CREATE TRIGGER trail_fts_ad AFTER DELETE ON trail BEGIN
    DELETE FROM trail_fts WHERE rowid = old.seq;
END;
CREATE TRIGGER trail_fts_au AFTER UPDATE OF data_json ON trail BEGIN
    DELETE FROM trail_fts WHERE rowid = old.seq;
    INSERT INTO trail_fts(rowid, text)
        SELECT new.seq, coalesce(group_concat(value, ' '), '') FROM json_tree(new.data_json) WHERE type = 'text';
END;
INSERT INTO trail_fts(rowid, text)
    SELECT t.seq, (SELECT coalesce(group_concat(j.value, ' '), '') FROM json_tree(t.data_json) j WHERE j.type = 'text')
    FROM trail t;

CREATE VIRTUAL TABLE summary_fts USING fts5(text, tokenize="unicode61 categories 'L* N* Co M*'");
CREATE TRIGGER summary_fts_ai AFTER INSERT ON journal
WHEN new.kind IN ('trail.day_summary', 'trail.day_narrative') BEGIN
    INSERT INTO summary_fts(rowid, text)
        SELECT new.seq, coalesce(group_concat(value, ' '), '') FROM json_tree(new.data_json) WHERE type = 'text';
END;
CREATE TRIGGER summary_fts_ad AFTER DELETE ON journal
WHEN old.kind IN ('trail.day_summary', 'trail.day_narrative') BEGIN
    DELETE FROM summary_fts WHERE rowid = old.seq;
END;
INSERT INTO summary_fts(rowid, text)
    SELECT t.seq, (SELECT coalesce(group_concat(j.value, ' '), '') FROM json_tree(t.data_json) j WHERE j.type = 'text')
    FROM journal t WHERE t.kind IN ('trail.day_summary', 'trail.day_narrative');
