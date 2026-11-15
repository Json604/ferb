CREATE TABLE perception_seen (
    source TEXT NOT NULL,
    external_id TEXT NOT NULL,
    event_id TEXT,
    PRIMARY KEY(source, external_id)
);
CREATE TABLE perception_batches (
    id TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    reason TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','done','unavailable')),
    event_ids_json TEXT NOT NULL,
    error TEXT
);
CREATE TABLE perception_receipts (
    event_id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES perception_batches(id) ON DELETE CASCADE
);
CREATE TABLE notification_rules (
    id TEXT PRIMARY KEY,
    app TEXT NOT NULL,
    conversation TEXT,
    topic TEXT,
    owner_quote TEXT NOT NULL,
    until_at TEXT,
    exceptions_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL
);
CREATE TABLE notification_told (
    id TEXT PRIMARY KEY,
    app TEXT NOT NULL,
    conversation TEXT NOT NULL,
    event_id TEXT NOT NULL,
    text TEXT NOT NULL,
    at TEXT NOT NULL
);
CREATE TABLE perception_labels (
    event_id TEXT PRIMARY KEY,
    expected TEXT NOT NULL,
    owner_quote TEXT NOT NULL,
    at TEXT NOT NULL
);
