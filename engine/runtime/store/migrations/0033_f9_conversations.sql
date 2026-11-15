CREATE TABLE delegated_conversations (
    id TEXT PRIMARY KEY,
    tree_id TEXT NOT NULL,
    scope_json TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('proposed','active','waiting','paused','handed_over','closed','declined','blocked')),
    approval_id TEXT,
    turn INTEGER NOT NULL DEFAULT 0,
    cursor INTEGER NOT NULL DEFAULT 0,
    turn_cursor INTEGER NOT NULL DEFAULT 0,
    handover_json TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX delegated_conversations_tree ON delegated_conversations(tree_id);
CREATE TABLE conversation_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES delegated_conversations(id),
    source_id TEXT NOT NULL,
    origin TEXT NOT NULL,
    data_json TEXT NOT NULL,
    at TEXT NOT NULL,
    UNIQUE(conversation_id,source_id)
);
CREATE TABLE conversation_turns (
    conversation_id TEXT NOT NULL REFERENCES delegated_conversations(id),
    number INTEGER NOT NULL,
    output_json TEXT,
    status TEXT NOT NULL,
    PRIMARY KEY(conversation_id,number)
);
CREATE TABLE conversation_workers (
    task_id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES delegated_conversations(id)
);
CREATE TABLE conversation_owner_changes (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES delegated_conversations(id),
    action TEXT NOT NULL,
    owner_words TEXT NOT NULL,
    at TEXT NOT NULL
);
CREATE TABLE attention_notices (
    id TEXT PRIMARY KEY,
    source_refs_json TEXT NOT NULL,
    text TEXT NOT NULL,
    origin TEXT NOT NULL,
    created_at TEXT NOT NULL,
    delivered_at TEXT
);
CREATE TABLE conversation_forgotten_sources (
    source_id TEXT PRIMARY KEY
);
