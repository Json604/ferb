-- Fix wave 3 fix round (review Q1): the memory each task was given. A planner or worker exposure that cites a memory
-- item (memory:<id>) or a learned node (m-...) is recorded on the task, so a forget finds the tasks that were given the
-- item directly, after the task's trace has aged out too. Ids only. Existing exposures are carried over.
CREATE TABLE task_memory_refs (task_id TEXT NOT NULL, ref TEXT NOT NULL, PRIMARY KEY (task_id, ref));
CREATE INDEX task_memory_refs_ref ON task_memory_refs(ref);
INSERT OR IGNORE INTO task_memory_refs (task_id, ref)
    SELECT s.task_id, r.value FROM spans s, json_each(s.input_json, '$.source_refs') r
    WHERE s.kind = 'context.exposure' AND s.task_id IS NOT NULL AND json_valid(s.input_json)
      AND json_type(s.input_json, '$.source_refs') = 'array' AND (r.value LIKE 'memory:%' OR r.value LIKE 'm-%');
