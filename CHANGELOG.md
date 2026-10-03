# FERB version history

Versions use three numbers, `MAJOR.MINOR.PATCH`:

- **MAJOR** stays `0` until FERB leaves Block 1 (the personal, single-Mac version).
- **MINOR** goes up for a new capability (a build step, a unit, or a spike that changed how FERB works).
- **PATCH** goes up for fixes and smaller changes inside that capability.

`0.0.x` is the pre-release foundation, ending at the §21 safety suite. `PROGRESS.md` called that milestone "v0.1".
Each entry names the commits it covers, so `git show <hash>` takes you to the code. The current version is in
`pyproject.toml` and `ferb/__init__.py`.

## 0.21.0 — 2026-10-03 — Window-targeted scrolling, owner-verified rollout

- Stop treating row revelation as directional scrolling; require the intended pane, use scrollbar progress, and
  restrict Page keys to text views. Bound batches and stop on uncertain progress without claiming the end.
- Add wheel scrolling to screenshot-bound native pointer input and an optional SkyLight window-targeted transport.
  No focus transfer, activation or double posting. Availability permits an attempt; cross-Space acceptance is pending.
- Let the model choose distance per step separately from batch count, from small nudges to large viewport steps.
  Show scroll direction, distance, amount and target in CLI traces. Worker guidance forbids substituting chat navigation for
  scrolling. Add `ferb watch --mac --only M14` for one owner-verified tick, functional and keyboard-focus verdicts.
- Preserve watch setup/action/cleanup order; verify a newly created document before accepting it as test-owned.
- Research, limitations and acceptance procedure: `docs/BACKGROUND_SCROLL_021.md`. No daemon update. Owner verified WhatsApp scrolling on another Space without disturbance; see the recorded acceptance scope.

## 0.20.1 — 2026-10-02 — Personal browsers require explicit direction

- Default web requests to FERB's own profile. Repair an owner-web classification without explicit personal-browser
  direction. Remove personal-browser scopes from owner tasks without that direction, including retries; an earlier
  agent's goal/profile choice cannot grant access. Explicit owner-to-FERB route correction can proceed.
- Use explicit browser/profile naming for authority; account names and matching site domains alone no longer count.
  Guard native Mac browser tools against bypassing absent personal-browser scopes.
- Report disconnected paired profiles as connection failures when listing tabs, including recorded version refusal
  details. Do not return an empty tab list and invite profile guessing or Mac fallbacks.
- Remove the visible wake-page fallback after a windowless connection attempt times out. Opening a new visible
  browser window and then switching the owner back is not reliable background operation.
- Diagnosis, migration and limits: `docs/BROWSER_DEFAULT_0201.md`. No installed daemon restart or owner-profile action.

## 0.20.0 — 2026-10-02 — Durable conversations and exact pointer gestures

- Resume the last chat across CLI and daemon restarts; add `ferb chat --new` and `--resume ID`. Persist redacted
  task context, prior attempts and pending questions. Recover older recorded chat attempts; pass real, scoped task
  IDs to the supervisor. Retries keep original goal/profile context under fresh authority; unknown effects block replay.
- Preserve browser screenshot coordinates through policy-gated clicks, double/right clicks, hover and same-frame
  drag. Require fresh visual state and trusted frame-local mousemove readiness, including cross-site frames.
- Add native `mac.pointer` for bounded process-directed gestures on the current Space, without OS cursor takeover.
  Validate geometry/freshness, retain both drag-endpoint policy floors, and release held input on cancellation.
- Carry pointer cancellation from the daemon to the extension; distinguish pre-input refusal from uncertain partial
  input. Document input delivery separately from task success. Extension 0.9.0, protocol 5 requires owner reload.
- Evidence, installation and remaining owner acceptance: `docs/POINTER_CONTINUITY_020.md`.

## 0.19.0 — 2026-10-02 — Reliable browser targets, login handoff and honest outcomes

- Make Chromium Accessibility the primary semantic view, backed by isolated DOM element references. Bind frames
  by CDP identity, route child debugger sessions, include embedded-frame text, and refresh frame positions before input.
- Refuse covered/disabled targets and unconfirmed focus; never silently downgrade trusted input to page events.
  Preserve failed and unknown extension outcomes (extension 0.8.0, protocol 4), including after reconnects.
- Add task-tab screenshots and point-to-element resolution through the normal action policy. Add hidden owner OTP
  input bound to a visible code field and HTTPS frame origin; values travel sealed and never enter model results/logs.
- Require fresh outcome evidence for ledger-based goal verification; no conditions means unverified. Normalize
  snapshot bookkeeping for loop detection and stop repeated consequential actions without observed progress.
- Use raw Mac AX roles for text editing, verify set-value results and focus, and retry learned failed mechanisms
  after an hour. Exclude comma-separated content previews from action-label floors while keeping call/pay/delete floors.
- Keep wakeable paired browsers available. Structured chat stop/retry/continue cancels and waits for the old run,
  preserves original request context and scoped authority, and blocks retries of unknown actions.
- Validation and remaining limits: `docs/RELIABILITY_019.md`. Owner acceptance and installation remain separate.

## 0.18.0 — 2026-10-02 — Background first: the drivers rebuilt on what the owner watched work

### Documentation follow-up — 2026-10-02
- Recover the interrupted documentation drafts and finish the cold-start guide: rewrite ARCHITECTURE around
  the implemented one-worker flow, refresh README / CAPABILITIES / CODE_TOUR, and complete FILE_MAP with the
  rework docs, owner questions, learned app profiles and watch/stats tooling. Keep the runtime at 0.18.0.
- Correct stale section references and distinguish current code from plans and historical test runs. Document
  the pairing-guidance flag, submit-versus-send undo behavior, driver-only browser screenshots, and the meaning
  of verification when no success conditions are supplied. Preserve the pre-rework contract in git history.

### Browser
- Browser driver rebuilt background-first (PLATFORM_BROWSER.md 13), extension 0.7.0 / protocol 3: one FERB window
  per profile born minimized and kept, task tabs never active; a debugger session per task tab for its life (real
  input by default, mac shortcuts, dialogs answered by the extension, uploads, jpeg screenshots in parts); snapshots
  with in_view / occluded / scroll containers / shadow roots / iframe offsets; container scrolls by real wheel;
  bridge fixes from the audit (timeouts reconciled, outbox order, same-connection hello, confirmed close, YouTube
  account read once). Real lab test: `tests/test_browser_lab_real.py` (throwaway Chrome, frontmost app never
  changed). (`3937d45`, plus the lab-test commit)

## 0.17.0 — 2026-10-02 — One worker per request, gated at the action
Phase 4 of `docs/REWORK_PLAN.md`, as designed in `docs/DESIGN_ONE_WORKER.md`.
- The supervisor answers with a reply, one question, or a task (goal, identity, informational targets, success
  conditions). No subtasks, readers, actors, verbs or scopes; the only check left in code is the identity floor
  (one repair round, then a question). One worker runs the whole request with every tool its identity allows, in
  the background; its summary is the owner's answer (the separate answer run is gone).
- Every act call declares an `effect` (navigate / edit / send / pay / destroy) and `why`. The gateway raises the
  declaration to the control's floor: tool kinds are facts (a plain click is a click, a search box is a search,
  Return is a submit, a form's button submits it) and one label list of floor words (Send / Submit / Call / Pay /
  Buy / Order / Delete / Remove / Unsubscribe / Sign out). Navigate and edit run; a send asks once per (request,
  effect, target) with the worker's reason unless the owner's own message names the target; pay and destroy ask
  every time. Hard floor, undo hold, duplicate-send and contact checks unchanged. Owner data read in the request may
  only go to a target of it, something the owner named, or something they approved.
- `owner_ask`: a worker's question to the owner, shown in `ferb chat` with numbered options or a free-text prompt
  (`chat.answer`); declined at once during a scheduled run.
- Schedules record targets from the owner's words; a run is held to them.
- Deleted: the intent registry and verb tables, the reader label lists, the owner-browser phrase gate, the
  own-words typing rule, the spelling corrector, the route picker (the catalogue stays for the environment line).
- Tests: `test_policy`, `test_supervisor`, `test_routes`, `test_ux`, `test_gateway` rewritten; `test_explore` and
  `test_spelling` deleted; the acceptance set in `test_acceptance_one_worker.py` replays eight real failures.

### Mac
- Snapshots show what is really there: a control is anything that exposes an action (pressable rows, groups, cells
  and images included), with its actions, role, value and disabled / selected / expanded state; headings and the
  window title; an open sheet, dialog, pop-up menu or popover is in the snapshot with its place; the app's windows
  are listed (window server + the Accessibility routes that reach other-desktop windows) and one can be read by
  title or id; truncation says why. One tree walk per snapshot serves the snapshot, the before / after check and the
  password scan.
- Actions go background first, in the order the platform probes and the owner-verified runs established, each rung
  only after the one before verifiably did nothing: press → the enclosing control → focus + Return; type by keystrokes
  to the app's process (read back) → Accessibility keyboard where it has worked → the box's value; new `edit`
  (exact range through the text's selection, never arrow keys); a ⌘ shortcut runs its menu item and is refused with
  the Accessibility alternative while the app reports it disabled (it would only beep); escape cancels the open sheet
  / dialog / menu, never blind; new `scroll` (Page Up / Down with the area focused, then the scroll bar; sliders
  step); new `select` for pop-up buttons; click-at-point turns the point into the control under it first, posts a
  click to the app alone only for a window on the current desktop, and uses the real mouse only with
  `allow_foreground=true` — which is now the only way any `mac.*` call brings an app in front.
- What works per app is learned, not written: a per-app profile (`~/.ferb/device/apps/<bundle>.json`) counts each
  mechanism's hits and misses from every action, skips one after three misses in a row until it works again, and
  can be seeded by `ferb.act.mac.probe --seed`. The old per-daemon "ignores background keys" flag is gone.
- Full key table (letters, digits, F-keys, Home / End / Page Up / Down, forward delete, punctuation, keypad) and
  modifier aliases; ⌘Q / ⌘H / ⌘M refused like app switching.
- Real test: TextEdit and Calculator driven through the driver with the owner's frontmost app polled every 50 ms and
  never changing (`tests/test_mac_background_real.py`, `pytest -m real`).

## 0.16.1 — 2026-10-02
- Reminders: the planner's quote is checked against the words as typed. Before, the typo fix ran first, read "looks"
  as the Books app and dropped a test reminder. Word forms like "looks" and "minutes" now count as English.
  If a reminder is refused, FERB says so instead of showing a reply that claims it was set. (`2173352`)

## 0.16.0 — 2026-10-01 — Readers open what they need to read
- Readers may click to read on web pages: email rows, list items, tabs, and links and buttons with harmless labels.
  Forms and consequential labels are still refused. (`ff684ae`)
- Click-to-read counts as navigation, and a reader typing the owner's own words counts as a search, so a reading
  request's intent allows both. Security pages are checked first and still always ask. (`06299d6`)

## 0.15.1 — 2026-10-01
- Readers are told how to search an app. A refused reader press says what the reader can do instead. (`3d40f38`)

## 0.15.0 — 2026-10-01 — GPT-6 under Codex (spike 21)
- FERB gives Codex its own model catalog, with code mode, sub-agents and experimental tools switched off.
  The `functions.*` tool namespace is read as plain names. Model `gpt-6-sol`. Runtime safety tests pass on gpt-6-sol,
  gpt-6-astra and gpt-6-luna. (`baa7495`)

## 0.14.0 — 2026-10-01 — WhatsApp search in the background (spike 20)
- A reader may type the owner's own words into any box. (`9eb3844`)
- Search boxes that aren't real text fields are clicked and typed into with the app in front. set_value is used only
  on real text fields. (`c547901`)
- Background typing through the Accessibility keyboard (`AXUIElementPostKeyboardEvent`): WhatsApp search no longer
  needs the window in front. (`77f5491`, `f36bbb2`, `28f4897`)

## 0.13.0 — 2026-10-01 — Readers can open things
- Readers may click to read; the gateway keeps them to reversible kinds. The folder-access question comes for any
  folder named in the owner's words. (`0d352ac`)
- A running browser is woken through its binary. Search boxes count as searches. A step whose input is missing is
  skipped instead of run on an empty value. (`f773101`)
- Contract: the owner keeps the reader widenings, risk acknowledged. (`9315e24`)
- The reader instructions match what the gateway allows. (`8b8d5b9`)
- New `mac.menu` tool: an app's menus in the background. Readers may press only harmless items. Refused reader calls
  appear in the trace. (`1f44ab0`)

## 0.12.1 — 2026-10-01
- Docs brought up to date. (`8e45a80`)
- The workers' link to the daemon waits up to an hour. A slow reply is no longer reported as "the daemon went away".
  (`69fbbcd`)

## 0.12.0 — 2026-10-01 — The browser, profile and folder the owner names
- The browser the owner names is the one used: the planner is told, and the router enforces it. Extension install
  is guided. (`b0d2efd`)
- The owner decides what's appropriate for their own work; FERB does it in full. (`291f380`)
- Paths count by their folder names. Desktop, Documents and Downloads are handled, and so are Finder's ":" names.
  `ferb trace` shows every step. (`82451e2`, `8f72992`, `e9a0b12`, `18d119a`)

## 0.11.1 — 2026-10-01
- A task's verify and reconcile checks end with it, so their tabs no longer leak. (`2b7f8cb`)
- New `ferb voice on|off`. (`bbe9479`)

## 0.11.0 — 2026-10-01 — The browser wakes without moving the owner (spike 19)
- The browser starts with `open -g` and no startup window, and the first FERB window is created minimized. If the
  screen shifts anyway, FERB returns the owner to where they were. New paste switch,
  `[policy] pasted_text_is_data`. (`52c7278`)

## 0.10.0 — 2026-10-01 — Mac actions in the background (spike 18)
- Order of methods: Accessibility, then keystrokes posted to the app's process, then the foreground once per stretch
  of work, with the owner's input winning. Buttons are used over keystrokes. A second send to the same contact asks
  first. (`31d8a53`)
- Apps work from the background. The running-app list stays current. (`eb0c44d`)
- One close typo match is used and said; several are asked about. Plain reasons replace a vague apology. Read-only
  steps may open apps. Electron apps' Accessibility trees are switched on. (`009d05e`)

## 0.9.0 — 2026-10-01 — Voice
- Spikes 9 and 17: system audio taps, the microphone through Core Audio, on-device speech. (`8352ffc`, `5d26d4f`)
- Push-to-talk: hold Right Option. FERB.app is signed and runs in the background, and never uses a Bluetooth mic.
  (`2521022`, `15fe2ca`)
- `ferb voice`: a live view of what FERB heard and what it said. (`e1df2d7`)

## 0.8.0 — 2026-10-01 — Mac snapshots with a numbered image (Unit 2)
- `mac.snapshot` returns the window image with each control numbered (set-of-marks). (`5354065`)

## 0.7.0 — 2026-10-01 — Unit 1
- The owner stays on their desktop. FERB knows what it can do. Model-service errors are plain. The planner is faster.
  FERB can speak its replies (`--speak`). (`14e3e98`)

## 0.6.2 — 2026-10-01
- A loader in `ferb chat` while FERB works; plainer `ferb browsers`. (`1efdc6c`, `2060a07`, `efd729d`)

## 0.6.1 — 2026-09-30
- Several requests at once in `ferb chat`, later taken back to one at a time. FERB's own profile counts as available.
  A `[mac] yield_to_owner` switch. (`aacff6e`, `e42e795`, `ad587ef`)

## 0.6.0 — 2026-09-30 — Whose browser
- The owner's profile is used only when their words say so; otherwise FERB's own browser. FERB can take over an open
  tab and scroll the whole page. (`3652075`)
- Pasted requests and "yes" answers; `browser.find` searches the whole page; app aliases like "wa"; sites from
  history need site wording. (`7d3ff63`, `3611440`, `afbb25d`)

## 0.5.1 — 2026-09-30
- Docs: README, CODE_TOUR, CAPABILITIES. (`c1bf985`)

## 0.5.0 — 2026-09-30 — Exploring beyond the plan
- Workers may read outside the plan with a stated reason; acting elsewhere becomes a route change the owner approves.
  Real tests are locked down. (`87b91cb`)
- The planner's capability list is built in code from what's usable now. (`ca11c90`)

## 0.4.1 — 2026-09-30
- Screenshot clicks in messaging and calling apps declare their purpose. Real contact test. (`077c6c7`)

## 0.4.0 — 2026-09-30 — Contacts never guessed (build step 16a)
- The contact is checked against what was on screen, with a numbered "Which Rahul?" question. (`535d4d9`)

## 0.3.1 — 2026-09-30
- Structured times: the model says when, and code computes the date. Fuzzy time words, with the owner's corrections
  remembered. Service names that are everyday words count only in context. (`aff3e28`, `416baf5`, `4e1307f`)

## 0.3.0 — 2026-09-30 — Environment-aware resolution; calls and messages (build step 16)
- Routes are chosen in code: correction, then 14-day usage, then app, then paired web, then ask. Calls and messages
  get an undo window. (`871a010`)

## 0.2.0 — 2026-09-30 — Scheduled and recurring tasks (build step 15)
- Schedules are held to the recorded intent. Progressive setup. Time awareness. (`b413a38`, `2d501b7`)

## 0.1.0 — 2026-09-30 — §21 safety suite: all 24 pass
- Offline and real (Codex). Mailbox send is confirmed from the Sent folder. (`eacab8d`)

## 0.0.x — the foundation (2026-09-28 to 2026-09-30)

| Version | Build step / spikes | Commits |
|---|---|---|
| 0.0.16 | 14a: UX fixes from the owner's first session | `517c0af`, `cea1d01` |
| 0.0.15 | 14: durable timers and reminders, notifications, `time.*` tools, measured durations, mailbox send | `68b9d2c`, `6ee39b1`, `d50dc08` |
| 0.0.14 | 13: Jev in shadow mode | `ae638fa`, `6898536`, `dcb99c0`, `b5496b1` |
| 0.0.13 | 11: memory with provenance | `ff82446` |
| 0.0.12 | 10: watchers and the trail; budgets replaced by runaway guards | `4a698ba`, `fc96a7b`, `b0abac6` |
| 0.0.11 | 9: screenshot and click | `e544adc`, `b309ee1`, `2d6dc4d` |
| 0.0.10 | 8: multi-worker orchestrator | `1b58b45`, `94b7e40`, `3523d0d` |
| 0.0.9 | 7: Mac control | `b3f6a22`, `6200efd`, `e40e274` |
| 0.0.8 | 6b: FERB's identity on the extension, sealed secrets, inbox | `1da4387`, `419c67b`, `5372031` |
| 0.0.7 | 6a: extension, native host, pairing, owner `browser.*` tools | `c8a35b1` … `9376ecb` |
| 0.0.6 | 5: FERB's own browser, Keychain secrets, OTP; spike 16 | `6cae905` … `eca134d` |
| 0.0.5 | 4: runtimes, `ferb mcp`, gateway, files, `ferb chat` | `d595c25`, `702fe41` |
| 0.0.4 | 3: device, permissions, installer, setup / doctor; spike 15 | `7ed35dd` … `d8da763` |
| 0.0.3 | 2: sanitize, policy engine, ledger, `ferb stop` | `e862b3f` |
| 0.0.2 | 1: core, store, bus, clock, daemon, auth, journal, logs, trace | `33bcd0a`, `1e8041d` |
| 0.0.1 | Spikes 1–4, 6, 8, 10, 12–14 | `8d5802d` … `c4b0b07` |
