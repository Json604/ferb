-- Index Devanagari (and other mark-bearing scripts) as whole words: unicode61's default token categories exclude
-- combining marks, which split words like दिल्ली into fragments. Rebuild from memory_items (external content).
DROP TABLE memory_fts;
CREATE VIRTUAL TABLE memory_fts USING fts5(subject, key, content, content='memory_items', content_rowid='seq',
    tokenize="unicode61 categories 'L* N* Co M*'");
INSERT INTO memory_fts(memory_fts) VALUES ('rebuild');
