-- F4 (D35): a browser download never opens a dialog on the owner's screen. The extension catches it in a tab FERB
-- drives for a task; the daemon takes the bytes into ~/.ferb/workspace/downloads/<id>-<name> and keeps this record of
-- facts: where it came from, what it is (size, SHA-256 of the bytes FERB received) and where it is. Never the file's
-- content; source_url is stored redacted (rule 14). The file is outside content (origin external:download, MP2).
-- state: taken | refused (over browser.download_max_mb) | failed (the transfer broke) | not_taken (the browser
-- started it itself and the extension couldn't intercept it).
CREATE TABLE downloads (
    id         TEXT PRIMARY KEY,
    task_id    TEXT,
    profile    TEXT NOT NULL,         -- browser/folder
    identity   TEXT NOT NULL,         -- owner | ferb
    tab        TEXT,                  -- the task's tab alias (t1)
    state      TEXT NOT NULL,
    path       TEXT,                  -- absolute path of the taken file (NULL unless taken)
    name       TEXT,                  -- the safe file name (after the id prefix)
    bytes      INTEGER,               -- bytes received (taken) or announced (refused)
    sha256     TEXT,
    mime       TEXT,
    source_url TEXT,                  -- redacted
    reason     TEXT,
    origin     TEXT NOT NULL,
    at         TEXT NOT NULL
);
CREATE INDEX downloads_task ON downloads(task_id);
