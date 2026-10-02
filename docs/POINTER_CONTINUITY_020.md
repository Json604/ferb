# 0.20.0 — durable conversations and exact pointer input

Implementation is on `astra/version`. The installed daemon is unchanged. This extends the earlier
[0.19.0 reliability work](RELIABILITY_019.md); it does not establish that the live Zomato flow succeeds.

## Why a new chat forgot the task

The journal contained the earlier conversation, but resumable task specifications and the chat's task IDs lived
only in daemon memory. Each CLI connection also generated a new session ID. The model could remember an attempt
in prose while the control path rejected its ID as belonging to another chat.

`chat.attach` now returns a durable conversation ID. Plain `ferb chat` resumes the last conversation; `--new` creates
another and prints its ID, and `--resume ID` switches to an existing one. SQLite stores redacted task specifications,
owner words, pending clarification and links between attempts. The supervisor receives real IDs for its latest
32 candidates, scoped to that conversation. Recent chat/task history is also scoped; owner preferences, notifications
and account-wide reminders remain shared. Closing the CLI still leaves running tasks alone.

A continuation loads the original goal and last recorded result, restores named-profile context from the owner's
words, and reads current state afresh. Explicit profile changes in the new request take precedence. It creates a
new attempt under fresh request authority: approvals, run tokens, OTPs and passwords are not restored. An earlier
action with unknown outcome blocks retry pending verification; this release does not automatically reconcile it.
If the model returns an invalid/ambiguous task reference, one repair asks for a specific choice before work starts.

On default conversation attachment, up to 100 recent root tasks missing context are considered for legacy recovery.
Recovery requires a recorded chat message and task span; scheduled/non-chat work is excluded. Missing success
conditions and permissions are never invented. Deleted or incomplete old records cannot be recovered.

## Where pointer input helps

Semantic AX/DOM actions remain the first choice for named controls. Exact pixels are useful for custom drawn
controls, canvas regions, sliders, right-click menus, double clicks and dragging. They don't repair a disconnected
profile, a server rejecting login, or an OTP flow that never advanced.

Browser flow (both browser tool families):

1. Take a task-tab screenshot. Coordinates use the returned image dimensions.
2. Resolve a visible point with `browser.point`; inspect its policy-visible label.
3. Call `browser.act` with click, dblclick, rightclick or hover. For drag, resolve both endpoints from the same image
   and pass the second ID as `end_element`.
4. Take a fresh observation and check the expected result.

The selected pixel is preserved; it is not replaced by an ancestor's center. Screenshot tokens expire after 30
seconds and are consumed by one gesture. Document, viewport and scroll changes invalidate them. Frame identity and
hit targets are checked; a trusted mouse move must reach the intended frame-local coordinate before pressing.
This also handles compositor/layout timing in cross-site frames. Drag endpoints must share one frame. Both labels
contribute to the action's policy floor; account checks, effect declarations, approvals and the ledger still apply.
The tab stays in FERB's minimized window and pointer input does not use the OS cursor.

`mac.pointer` accepts a fresh `mac.screenshot` ID, image `x/y`, an action, and `end_x/end_y` for drag. It sends marked
Quartz events to the target process, never activates the app, switches Spaces or moves the global cursor. It checks
window geometry/current Space, initial AX state and owner activity in that app. It refuses browser pages, which
use the extension. Native apps can ignore process-directed mouse events; input delivery is not proof of success.

Gestures are bounded; no separate indefinite mouse-down API is exposed. Python cancellation sends a browser cancel
command; extension timeout/disconnect also requests cancellation. Native cleanup releases before the GUI lock is
freed. Both paths attempt release after partial failure. A killed browser/process or broken transport can prevent
cleanup confirmation, so uncertain outcomes remain unknown and must not be blindly repeated.

Freshness checks do not prove every pixel is unchanged: canvas/video/animation changes may not appear in DOM or AX
state. Long drags, cross-frame dragging, arbitrary drawing paths, wheel-at-point and a graphical cursor overlay are
not implemented. Existing tool traces record target/action context; FERB remains CLI-only.

## Validation

- `test_persistent_chat`: reopened CLI, actual daemon restart, prior goal/profile/result recovery, redaction, legacy
  import, scoped IDs/history, invalid-ID repair and no replay after unknown outcomes.
- `test_pointer` / `test_mac_pointer`: exact pixels, consumed/stale image refusal, cancellation propagation and
  mouse-up cleanup, interrupted-native unknown outcomes and destination policy floor.
- Real throwaway Chrome: **7 passed** (7.82 s). Trusted exact canvas clicks, double/right click, hover, drag, cross-site
  frame targeting, a native HTML range slider, stale-DOM refusal and no frontmost-app changes.
- Real TextEdit/Calculator background test: **1 passed** (4.39 s). The added native pointer probe refused the TextEdit
  window on another Space without changing focus. Actual on-screen native dragging remains unverified.
- Full offline suite: **510 passed, 50 deselected** (118.30 s). Core mypy (5 files), extension JavaScript syntax
  and `git diff --check` passed. The legacy continuity test now reopens actual CLI objects via `chat.attach`.

Owner acceptance remains: run relevant `ferb watch` checks and the app tasks in `OWNER_ACCEPTANCE.md`, including an
on-screen disposable native window. Offline contracts and the lab do not replace the owner's observation.

## Install when ready

From this branch, the owner runs `ferb setup --update` and reloads the unpacked extension in each paired profile.
The extension is **0.9.0 / protocol 5**; older extensions are refused. Restart `ferb chat` normally to reuse the default
conversation and recover eligible older attempts. Nothing was pushed or installed by this change.
