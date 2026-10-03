# Owner acceptance log

## 2026-10-03 — search interrupted owner focus (0.23.0)

The owner reported a single unexpected move to VS Code during a search. The journal records Brave → VS Code at
13:06:27.330 through FERB's browser focus-restoration callback, while its own Chrome search tab was opening.
This is an owner-reported disturbance, despite the earlier Coursera/FMI run's no-disturbance verdict. 0.23.2
removes the stale restoration callback; regression tests reproduce the race offline. Installation and the
owner's live verdict on the fix remain pending. See `BROWSER_FOCUS_0232.md` for evidence and scope.

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

## 2026-10-03 — scaler connection repaired; managed extension update

The owner authorized the existing pairing method's one visible tab: “Yes—repair scaler now.” The CLI completed
pairing for Chrome scaler; a fresh `browsers.list` returned paired=true, connected=true, version=0.9.0, refused=null.
The current extension had fresh unpaired storage while the daemon remembered its older identity. No daemon or
browser restart was needed. This is a connection repair, not a new background-task acceptance verdict.

Branch 0.23's separate disposable-Chrome test migrated an existing unpacked installation to a managed folder,
published a newer version, and waited for the real 30-second alarm. It preserved pairing and reconnected as 0.10.1
in 32.98 s. No visible window opened and Chrome did not become frontmost. The setup uses a hidden test-owned
settings page to enable Developer mode; early attempts without it correctly failed Chrome's reload requirement.
The owner profiles were not used for these experiments. New release installation and Brave auto-update acceptance
remain pending; scaler's successful repair remains live on the previously installed daemon.

Final status: Chrome is no longer running. Scaler therefore has no current connection, but its installation UUID,
pairing key and sequence match the daemon. Do not describe this closed-browser state as a recurrence of lost pairing.

## 2026-10-03 — authorized 0.23 rollout verification

The owner approved migrating the paired extensions and installing/restarting FERB. At rollout, the running daemon
already reported 0.23.0 with the same code hash as `astra/version`; no extra restart or reinstall was performed.
The managed bundle/native-host registrations were prepared. The existing Chrome scaler and FERB profile extensions
were loaded from `~/.ferb/extensions/chromium` without removal; their pairings survived and both connected on 0.10.0.
Browser-control policy refused Brave's internal extension setup page and prohibited alternate automation. The owner
performed the manual Load unpacked step and confirmed completion; its managed path and live paired 0.10.0 connection
were verified. Both Chrome connections dropped later, and a read-only app inventory confirmed Chrome was closed
at the final check. No unrelated profile was installed. These are deployment checks, not new owner acceptance of pointer behavior or
the assignment task. Earlier closed-browser and not-installed observations above describe their own checkpoint.

## 2026-10-03 — Coursera and Claude FMI redo, installed 0.23

Owner verdict: “it didnt disturb me at all.” This applies to the quoted Coursera check and reminder/FMI redo run.
Saved source snapshots, eight pending reminder rows, the completed Claude session, output files and independent
NPV/IRR/crossover calculations corroborated the main work. Browser interaction used the extension, with no recorded
mac.click, mac.pointer or mac.act. This is full-workflow background acceptance for that run, not a test of every
native pointer gesture. The audit found reporting/checker and process-monitoring issues addressed in branch 0.23.1;
the owner's verdict precedes those fixes and does not establish their installed acceptance.
