-- F5a (S4, SQ6, D33, D31): what the owner confirmed, and the destinations FERB knows.
-- confirmations: one Yes or No per (task tree, key); the answer carries across that tree's retries (rule 9). An
-- escape row is used once (used = 1) and never covers a second run (D31); so is the Yes to a payment or an
-- account-security change once that call has run (SQ1: they always ask).
-- known_destinations: a destination the owner named in a request, chose in an answer, or confirmed to FERB's question;
-- a destination here is not "new" for the sink rule in any later task (SQ6). Values are normalized (lowercased email
-- or host, the digits of a phone number); never message content.
CREATE TABLE confirmations (
    id        TEXT PRIMARY KEY,
    tree_id   TEXT NOT NULL,
    rule      TEXT NOT NULL,        -- floor:pay | floor:account_security | floor:delete_permanent | floor:new_recipient
                                    -- | sink:new_destination | sink:outside_value | destination | escape:<rule id>
    key       TEXT NOT NULL,        -- rule plus the exact value(s) one answer covers
    granted   INTEGER NOT NULL,     -- 1 Yes, 0 No
    ask_span  TEXT,                 -- the owner.ask tool.call span that recorded the answer
    used      INTEGER NOT NULL DEFAULT 0,
    at        TEXT NOT NULL
);
CREATE INDEX confirmations_tree ON confirmations(tree_id, key);
CREATE TABLE known_destinations (
    id         TEXT PRIMARY KEY,
    kind       TEXT NOT NULL,       -- email | phone | host | name
    value      TEXT NOT NULL,
    how        TEXT NOT NULL,       -- named | answered | confirmed
    request_id TEXT,
    span_id    TEXT,
    at         TEXT NOT NULL
);
CREATE UNIQUE INDEX known_destinations_value ON known_destinations(kind, value);
