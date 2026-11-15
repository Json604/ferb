-- F3 (L1, D21, MP1): one permanent record per root task, composed by code from its trace when it ends. Kept forever;
-- the spans it was built from age out after 30 days, and retention never purges a task without one. The id is the
-- task id; made_by names the composer version; rebuildable from spans while they exist.
CREATE TABLE episodes (
    task_id         TEXT PRIMARY KEY,
    request_id      TEXT,
    request_text    TEXT NOT NULL,
    goal            TEXT NOT NULL,
    status          TEXT NOT NULL,
    verified        INTEGER,
    started_at      TEXT,
    ended_at        TEXT,
    seconds         REAL,
    answer          TEXT NOT NULL,
    conditions_json TEXT NOT NULL,   -- [{text, met: true|false|null, evidence}]
    steps_json      TEXT NOT NULL,   -- [{at, role, seconds, text, mark}]
    problems_json   TEXT NOT NULL,   -- ["⚠ …"]
    tools_json      TEXT NOT NULL,   -- {tool: {calls, errors}}
    origins         TEXT NOT NULL,   -- comma-joined origins of the task's request, context sources and tool results
    tool_origins    TEXT NOT NULL DEFAULT '',   -- only the task's own tool results' origins (F5's sink rule: own reads
                                                -- apart from inherited context)
    external_read   INTEGER NOT NULL,
    usage_json      TEXT NOT NULL,
    desktop_json    TEXT NOT NULL,
    made_by         TEXT NOT NULL,
    built_at        TEXT NOT NULL
);
