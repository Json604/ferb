-- Hands decision 7 (owner 2026-10-31): a task tree's download budget (32 GB per request) counts what its shell commands
-- receive through FERB's egress proxy beside the files its browser tabs take (the `downloads` rows), one counter per
-- tree. One row per tree that ran a shell command which received bytes; kept so a daemon restart mid-task doesn't
-- start the tree's count afresh. Facts only (a byte count), never content. Goes with its task (operational records).
CREATE TABLE IF NOT EXISTS shell_downloads (
    tree_id TEXT PRIMARY KEY,
    bytes   INTEGER NOT NULL,           -- bytes received through the proxy by the tree's shell commands so far
    at      TEXT NOT NULL               -- last update (UTC ISO)
);
