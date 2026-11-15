-- Build step 6a: browser profiles (section 9.5) and paired extension installs (section 9.6).
CREATE TABLE browser_profiles (
    device_id    TEXT NOT NULL,
    browser      TEXT NOT NULL,
    folder       TEXT NOT NULL,              -- identity of a profile; display names change, folders don't
    display_name TEXT NOT NULL,
    email        TEXT NOT NULL DEFAULT '',
    aliases_json TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (device_id, browser, folder)
);
CREATE TABLE ext_installs (
    install_id  TEXT PRIMARY KEY,
    browser     TEXT NOT NULL,
    folder      TEXT NOT NULL,
    identity    TEXT NOT NULL CHECK (identity IN ('owner', 'ferb')),
    key         BLOB NOT NULL,               -- per-install HMAC key; never logged or returned by any method
    last_seq    INTEGER NOT NULL DEFAULT 0,
    paired_at   TEXT NOT NULL,
    ext_version TEXT NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX ext_installs_profile ON ext_installs(browser, folder);
