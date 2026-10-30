# Live preview of FERB's work

The notch shows a live picture of what FERB is working on: the task's own browser tab or the one Mac app window the
current call targets, never the whole screen. Built 2026-10-29 (758ce419); assessed read-only 2026-10-31.

## Owner decisions (2026-10-31)
1. **Always live, as today**: the work-row circles, the popover and the floating player stream while a task runs.
2. **Status over the last frame**: when FERB stops working in a window (moves to another step, pauses, waits, asks),
   the preview freezes on FERB's last frame of it and shows what FERB is doing now over it ("Asking you: …",
   "Waiting for …", "Working in Terminal", "Paused"). It never keeps streaming the owner's window after FERB has left
   it (today it does: new WhatsApp messages would stream).
3. **Frame rate: measure first.** No cap until the heat is confirmed from data. The 33-53% CPU windows on 10-30
   (`resource_samples`, the daemon process's own CPU only, so test suites in other processes are not in it) came while
   previews ran, but the tasks differed, so it isn't proven. Preview frames and capture time get counted; step 13
   measures the same task with the preview open vs closed; the owner decides a cap from that.
4. **Hidden from screen sharing and recordings** (popover and floating player).

## What exists today (verified 2026-10-31)
- The gateway records a per-root-task target before each Mac/browser call (call_hooks.py:122-192, gateway.py:1446,
  1457; in memory only, task_preview.py). The app loops per task: new socket, `cli` login, `tasks.preview`
  (TaskPreview.swift:20-47, DaemonRPC.swift). One frame = Mac: two AX passes + one ScreenCaptureKit window capture +
  secure fields painted (driver.py:926-941); browser: the extension's `record_frame` with secret/card/code fields
  painted (ext_bridge.py:1956-1981). Then OCR masking of codes, card numbers and known secrets (images.py:90-111).
  Nothing is written to disk, DB, trace or log. Stops at task end/stop.
- Live use: 12 `daemon.slow_call` for `tasks.preview` on 10-30 (10-59 s each). The installed 10-28 build has no
  `tasks.preview`, so a notch built from the checkout gets "unauthorized" and retries every second until the update.

## Fix list
- [ ] Never queue on worker locks: try the lock without waiting; busy → the cached last frame with a state. One capture
      at a time per task; daemon time limit under the app's 10 s; cancel the capture when the app disconnects (today
      abandoned requests queue and run ahead of the worker's next call, server.py:262, locks.py FIFO).
- [ ] Decision 2: the daemon reports a preview state with each frame (working here / working elsewhere / asking /
      waiting / paused / done) plus a short line; the app draws it over the frozen last frame. The target freezes
      when FERB's call leaves that surface, the task pauses (`interrupt_tasks` keeps it today, gateway.py:711-715) or
      waits.
- [ ] Capture only while someone can see it: no capture when the panel is covered, the screen is locked or the
      display sleeps (decision 1 keeps it always live when visible).
- [ ] Decision 3: count preview frames and capture milliseconds in `resource_samples.work_json`; step 13 A/B.
- [ ] Decision 4: `sharingType = .none` on the preview popover and floating panel (TaskActivityBubble.swift:131-150).
- [ ] Restrict `tasks.preview` to FERB.app's session (today any program running as the owner can pull masked
      window pictures through the `cli` role, auth.py:40, rpc.py:198-201).
- [ ] Target choice by capability declaration (`preview_target(args, result)`, like `point`/`on_screen`), shared
      with the cursor-point resolution; remove the name/argument lists (call_hooks.py:131-177).
- [ ] Target setup lazily on the first frame request; until then wrap the hook in try/except with a timeout like
      `point` (a hook failure must not lose a successful `mac.snapshot`, gateway.py:1446, 1457).
- [ ] Keep the masking note: when AX can't read a window and only OCR masked it, say so (call_hooks.py:184 drops it).
- [ ] Clear app states: no visual surface yet / Screen Recording not allowed / window minimised / withheld / daemon
      too old; keep the last frame on brief failures.
- [ ] Parallel steps in one task: key the preview by root task + worker, or label which step it shows.
- [ ] 8d: a watch button on the working part's row in the turn card; the streamed plan text (`plan_delta`) merges into
      decision 19's plan line (one path).
- [ ] Tests: an abandoned request doesn't hold up the worker; no frame after task end or for a tab the task doesn't
      own; nothing persisted. Drop wiring-only tests in tests/test_task_preview.py.

## Live checks (step 13)
- Same browser task with the preview open vs closed: daemon CPU and call durations; no `daemon.slow_call` for previews.
- kartikey.xyz login with `browser.secret` + a code field: both stay painted in the popover.
- WhatsApp "message yourself": the preview freezes with a status line when FERB moves on; later messages don't stream.
- Minimised window, Screen Recording off, screen locked, two tasks at once; a Zoom/Meet share doesn't show previews.
- Whether macOS shows its screen-recording indicator while the preview runs.
