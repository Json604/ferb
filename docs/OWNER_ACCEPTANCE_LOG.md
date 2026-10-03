# Owner acceptance log

## 2026-10-03 — owner-supervised WhatsApp scrolling (`astra/version`, 0.21.0)

Owner explicitly requested testing their everyday WhatsApp app. Branch driver invoked directly; no daemon update.
Window on another Space. One line-unit wheel event moved the list; owner confirmed no disturbance and clarified
that detected cursor/app changes were their own. A following batch was **rejected for poor speed** by the owner.
Changed to viewport-sized pixel steps and visible-item progress; three steps (548 pixels each) completed in 1.5 s,
three observed changes, cursor and front app unchanged, no Space/activation notifications. Owner: “yes the scroll
test worked with no disturbance to me.” Added independent distance control; 5% step requested 34 pixels and owner
confirmed it was a very small movement. These findings establish WhatsApp scrolling in this setup, not other
apps or click/drag acceptance.

## 2026-10-03 — current Brave YouTube comments (installed FERB)

Owner explicitly requested reaching the end in their current Brave tab and identifying the last comment, keeping
the current sort. FERB scrolled the existing tab in the background. Final document height 60,680, scroll position
59,894, viewport height 786; repeated scrolling/waiting produced no further growth. Owner confirmed: “it did load
to the last comment available - confirmed.” Independent read found 454 loaded top-level comment nodes and matched
the final author/comment reported by FERB. This establishes the currently loaded end under this sort, not a claim
to have expanded every reply. No separate owner verdict about desktop disturbance was supplied for this run.

The task's checker could not see the worker-held tab and marked conditions false, producing an incorrect failed
task outcome. That historical outcome remains unchanged. Branch 0.21.1 repairs the registered checker handoff and
explicitly blocks checker edits; its full chat regression uses a fake browser. Installed live checker acceptance
is pending an owner-authorized update. No daemon restart, profile switch or new task was used to hide the failure.

Branch validation after the handoff fix: 546 offline tests passed, 50 deselected (119.96 s); core mypy and
`git diff --check` passed. These checks do not replace the owner's live verdict.


## 2026-10-03 — 0.22 orchestration rewrite

Repository-only work on `astra/version`. No installed-daemon update, real app operation or external message was
performed for this rewrite. Offline tests cover the new control contracts; live acceptance is **pending**. Prior
WhatsApp/YouTube scroll confirmations remain limited to the earlier tested operations. Next owner check is the
scaler Gmail → coding-session task in `OWNER_ACCEPTANCE.md`.

## 2026-10-03 — native background Chrome click (0.22.1 branch driver)

The owner reported much better 0.22 task progress but repeated desktop/focus shifts. Trace inspection found a
stale scaler extension and five delivered foreground clicks. The branch fix connects `mac.click` to the newer
exact-window pointer route, removes misleading foreground advice and corrects window-local coordinates.

Two Chrome harness startup failures sent no input. After fixing pipe inheritance, a single native click reached
the disposable canvas as one trusted event; the owner confirmed: “No disturbance—I kept working normally.” The
receiver exposed a coordinate error equal to the window origin. The corrected single-click repeat reached exactly
`(20,30)` on a canvas in a window on another Space. Cursor samples matched; front PID was unchanged and no front-app
transitions were recorded. `tests/test_mac_pointer_real.py` passed in 1.98 s. The second owner verdict is pending.
Each check closed only its own temporary Chrome process; no personal profile or installed daemon was changed.
These checks do not establish acceptance for other gestures/apps or the full assignment task.
