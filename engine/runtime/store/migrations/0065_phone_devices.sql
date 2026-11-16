-- Stage 3 S3 (T-1202): the phones paired with this Mac and what they did. The daemon holds these, never the link helper
-- (PHN-124, PHN-441): the helper only passes a phone's calls and proofs on. Public keys only: the link's own private
-- key, relay token and seal key live in the Keychain (ferb-secrets, S4), and a phone's private keys never leave it.
--
-- One row per pairing. A removed phone's row stays (removed_at): nothing is accepted under it again; pairing the same
-- phone afresh (the code and the owner's Yes) is a new row. At most 5 live rows (PHN-419), checked by the daemon in
-- the transaction that adds one.
CREATE TABLE phone_devices (
    id         INTEGER PRIMARY KEY,
    endpoint   TEXT NOT NULL,              -- the phone's link identity (its endpoint id): the device the link names
    name       TEXT NOT NULL,
    app        TEXT NOT NULL DEFAULT '',   -- the app's own words for itself, as its hello gave them
    unlock_pub TEXT,                       -- the lock key (PIN, face or fingerprint): the unlock window, a private
                                           -- answer (PHN-443); P-256, SubjectPublicKeyInfo DER, base64
    device_pub TEXT,                       -- the device key (hardware-held, no prompt): this very phone, for
                                           -- notification text and a reconnect (PHN-126)
    strong_pub TEXT,                       -- the fingerprint-only key: sensitive calls, a hard Yes (PHN-133, PHN-137)
    paired_at  TEXT NOT NULL,
    last_seen  TEXT NOT NULL,
    warned_at  TEXT,                       -- when the 30-day unused warning was given; cleared when seen (PHN-419)
    removed_at TEXT,
    removed_by TEXT                        -- 'mac', or the endpoint of the phone that removed it (PHN-419)
);
CREATE UNIQUE INDEX phone_devices_live ON phone_devices(endpoint) WHERE removed_at IS NULL;

-- What phones did (PHN-114, CONTRACT §11): one row per call that is not a routine read, every refusal, and the
-- pairing and removal; never a parameter's value (rule 7). Kept 90 days (PHN-130).
CREATE TABLE phone_audit (
    seq         INTEGER PRIMARY KEY,
    at          TEXT NOT NULL,
    device      TEXT NOT NULL,             -- the phone's endpoint; '' for what the Mac did (a pairing, a removal)
    method      TEXT NOT NULL,
    class       TEXT NOT NULL,             -- plain, private, sensitive, never, or 'link' for the session's own calls
    outcome     TEXT NOT NULL,             -- 'ok', or the refusal's code
    detail_json TEXT NOT NULL DEFAULT '{}' -- structural facts only (an approval id, a removed phone), redacted
);
CREATE INDEX phone_audit_at ON phone_audit(at);
