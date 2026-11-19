-- APP-979, for APP-974 and APP-976 (owner, 2026-11-19: "i want it better for eveyrone else that later installs").
-- FERB.app before APP-975 could file a task it had already shown a second time: at the first launch of a new day it took
-- up the task it remembered, read that task's old result and began a new turn dated the launch, with no request and no
-- goal, beside the task's own turn on an earlier day. This removes those copies, once, on every install that has them.
--
-- A copy is a turn of chat_history (FERB.app's own page table, APP-805 and DMN-555) whose request and goal are empty and
-- which names a task, when an earlier day holds another turn of the same task with the same status and the same answer.
-- The earliest turn of a task always stays: it has no earlier one. A later turn of the task that says something else
-- (another answer or status) stays, and so does a turn FERB.app filed for a task it had no turn for.
-- chat_history rows are deleted outright as data.delete does (STORE-424). The no-hard-delete rule of ENG-12 is for the
-- 0063 tables (chat_entries and its streams), which this leaves as they are.
DELETE FROM chat_history
WHERE kind = 'turn'
  AND COALESCE(json_extract(body_json, '$.request'), '') = ''
  AND COALESCE(json_extract(body_json, '$.taskGoal'), '') = ''
  AND COALESCE(json_extract(body_json, '$.backendTaskID'), '') <> ''
  AND EXISTS (
      SELECT 1 FROM chat_history AS earlier
      WHERE earlier.kind = 'turn'
        AND earlier.id <> chat_history.id
        AND json_extract(earlier.body_json, '$.backendTaskID') = json_extract(chat_history.body_json, '$.backendTaskID')
        AND earlier.day < chat_history.day
        AND COALESCE(json_extract(earlier.body_json, '$.status'), '') = COALESCE(json_extract(chat_history.body_json, '$.status'), '')
        AND COALESCE(json_extract(earlier.body_json, '$.answer'), '') = COALESCE(json_extract(chat_history.body_json, '$.answer'), '')
  );
