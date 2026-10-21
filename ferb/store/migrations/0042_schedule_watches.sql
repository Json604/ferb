-- Fix wave 6c (owner live case 2026-10-15: an alert at 10% battery built as a shell loop inside a task died on every
-- restart and update): "tell me when …" is a watch, a schedule stored here like any other. 'until_met' ends itself
-- once a confirmed run finds its condition met; 'standing' keeps watching; 'none' is ordinary scheduled work.
ALTER TABLE schedules ADD COLUMN watch TEXT NOT NULL DEFAULT 'none' CHECK (watch IN ('none', 'until_met', 'standing'));
