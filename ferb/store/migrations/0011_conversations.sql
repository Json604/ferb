-- Connections are transient; conversations and task context survive restarts. No approval grants or secrets.
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    pending TEXT NOT NULL DEFAULT '',
    asked TEXT NOT NULL DEFAULT ''
);
CREATE TABLE current_conversation (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    conversation_id TEXT NOT NULL REFERENCES conversations(id)
);
CREATE TABLE task_context (
    task_id TEXT PRIMARY KEY REFERENCES tasks(id),
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    spec_json TEXT NOT NULL,
    owner_words TEXT NOT NULL,
    prior_task TEXT REFERENCES tasks(id)
);
CREATE INDEX task_context_conversation ON task_context(conversation_id);
