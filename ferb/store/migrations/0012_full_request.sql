-- Authority words stay separate from complete marked request data across clarification/retry/restart.
ALTER TABLE task_context ADD COLUMN request_payload TEXT NOT NULL DEFAULT '';
ALTER TABLE conversations ADD COLUMN pending_payload TEXT NOT NULL DEFAULT '';
