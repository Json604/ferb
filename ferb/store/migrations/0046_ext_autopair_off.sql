-- L25 automatic pairing (owner 2026-10-24): profiles the owner unpaired with `ferb browsers unpair`. FERB never pairs
-- them by itself again; any pairing of the profile (`ferb browsers pair`, setup's identity step) removes the row.
-- Safe to apply again (like 0043-0045).
CREATE TABLE IF NOT EXISTS ext_autopair_off (
    browser TEXT NOT NULL,
    folder TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (browser, folder)
);
