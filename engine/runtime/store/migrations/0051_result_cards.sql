-- Step 8e (typed cards in the notch): what FERB told the owner about a task includes its cards (a scheduled run's
-- reminder card, a result's links, files, payments, sends), built from FERB's own records, so a client that shows the
-- result later (`chat.status`) draws them as the live result did. A table of its own beside `task_results` (written
-- with each result, read with it; re-runnable as 0050 is): a result told before cards, or without any, has no row.
CREATE TABLE IF NOT EXISTS task_result_cards (
    task_id    TEXT PRIMARY KEY,
    cards_json TEXT NOT NULL            -- JSON list of cards, redacted as the result is
);
