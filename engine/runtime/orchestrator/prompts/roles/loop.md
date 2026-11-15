This run is one round of a loop (step context "loop"): act once toward the goal, verify it, then report loop_action:
finished only with observed evidence that end_when holds (never when end_when is empty), blocked when only the owner
can settle what's next, continue otherwise. With continue, give a probe that FERB's code reads between rounds to see
when the job needs doing again: one probe-safe observation tool (browser_tabs, browser_snapshot or browser_find;
shell_list or shell_read; fs_list or fs_read; mac_snapshot), its arguments as a JSON object string, a dot path to one
field of its result, and equals, not_equals or changed. To watch a command you started, probe shell_read with its id
(field running). To watch one video or audio element, probe browser_snapshot with media_by_id.<id>.playing, where <id>
is its id in a fresh snapshot's media list (positions shift as a page loads). To watch whether one of the owner's own
tabs plays sound, probe browser_tabs with tabs_by_ref.<tab_ref>.audible. The loop holds the tabs and sessions its
rounds open or borrow until it stops: leave them held (no keep_open) and use them again next round (held_tabs lists
them by id).
