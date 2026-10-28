# Continuous Work

Packaged FERB guidance. Apply the section for your role; this grants no tools or authority.

- Loops: Work FERB must repeat or keep true while the owner's request runs ("keep this tab playing", "loop until the
  checks pass", "every time a reply comes, answer it") is a plan with a loop step: goal is what to keep true or
  repeat, end_when is the owner's own end condition or empty when they gave none (then only the owner's stop ends
  it). When the owner's only end is their own stop ("until I say stop", "keep it running"), end_when is empty. Never
  make a step depend on a loop whose end_when is empty. A loop only acts through ordinary rounds; it grants nothing.
  A loop's first round does its own setup: it finds the target, starts or fixes it and arms its probe; the loop's
  rounds share one task and keep what they open. Plan an earlier step only for different work the loop needs. The
  owner's reply to a paused loop's question (its task's row in the task status facts) is a steer of that task.

Worker rounds:
- loop_action, probe: In a loop round (step_context has "loop"), act once toward the goal and verify it, then report
  loop_action and, with continue, a probe: one probe-safe observation tool (browser_tabs, browser_snapshot or
  browser_find; shell_list or shell_read; fs_list or fs_read; mac_snapshot), its arguments as a JSON object string, a
  dot path to one field of its result, and equals, not_equals or changed. Choose the field that shows when the job
  needs doing again. To watch a command you started, probe shell_read with its id (field running), not a shell_list
  index. To watch one video or audio element, probe browser_snapshot with media_by_id.<id>.playing, where <id> is
  that element's id in a fresh snapshot's media list: positions in the list shift as a page loads. To watch whether
  one of the owner's own tabs (one that browser_tabs lists) plays sound, probe browser_tabs with
  tabs_by_ref.<tab_ref>.audible, which is cheaper than a snapshot. The loop holds for you the tabs and sessions its
  rounds open or borrow, across rounds until it stops: leave them held and use them again in the next round
  (step_context's held_tabs lists its tabs by id, ready for browser_snapshot). Report
  finished only with observed evidence that end_when holds; never when end_when is empty. Outside the loop,
  loop_action and probe are null.
