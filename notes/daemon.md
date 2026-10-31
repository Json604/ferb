# Daemon wiring, CLI and setup: decisions and fix list

Area: `ferb/daemon/main.py` and `server.py`, `disturbance.py`, `resources.py`, `reports.py`, `settings.py`,
`persona.py`, `banners.py`, `task_preview.py`; `ferb/bus.py`, `config.py`, `paths.py`, `ferb/clock`; `ferb/cli/*`,
`ferb/device/*`, `ferb/native_host` install, `ferb/voice`, `ferb/identity`, `ferb/eval`, `ferb/trace`, packaging.
Basis: a read of every line (2026-10-30), read-only DB counts. M = main.py.

## Owner decisions (2026-10-30)
1. **CLI chat goes** (owner: "I stopped using it once the notch was made"): remove `ferb chat` (cli/chat.py,
   live_chat.py) and the dead `questions.answer` loop with it. Voice and any text that points to "ferb chat's hidden
   prompt" points to the app instead. With no CLI chat, `task.stop` requires task ids (the app sends them); "stop
   everything" stays the separate global stop (`stop.engage`).
2. **Background service registered inside the app** (SMAppService): its definition lives in the signed FERB.app, nothing
   can edit it, deleting the app removes it, macOS lists it under Login Items. Replaces
   `~/Library/LaunchAgents/com.ferb.daemon.plist` for app installs; a checkout (no app bundle) still needs a dev
   launch path (decided at build time).
3. **No user-facing CLI** (owner, 2026-10-30). The `ferb` program stays only as invisible plumbing — `setup --service /
   --update / --codex-login / --codex-logout / --cli` (the app calls these), `mcp` (the models' tool bridge), `daemon
   run` — plus a developer toolbox (trace, logs, stats, doctor, memory, restore, ps, verdict, runtimes, browsers) used
   to debug; never presented to users. All user commands go (chat, voice, conversations, notifications, test-mode,
   decide). Push-to-talk and identity-profile setup move into the app (notes/voice.md).
4. **CLI trimmed to a development tool**: remove the Mac probe and watch harness (`device/probe.py`, `watch.py`,
   `detector.py`, `watch_ext`), the CLI permission walkthrough (the app does it), the old chat renderer, the duplicate
   trace renderer, and the dormant `conversations` / `notifications` / `test-mode` commands. Keep trace, logs, stats,
   doctor, memory, restore, ps, setup/uninstall, verdict, runtimes, browsers, mcp.
5. **Disturbance sampler only around acting tools** (owner, 2026-10-30): sample before and after tools that can change
   the owner's screen (Mac input, foreground, app open), not read-only tools and no 0.25 s polling; ignore the owner's
   own cursor moves; record only changes FERB could have caused (front app, focus, Space, fullscreen); give them to
   the merged checker as facts for "how the work was done" (notes/orchestration.md decision 9). Fix the start/finish
   race. (Today 40,679 of 87,099 spans, 74% cursor-only, used only in prompts and traces.)

6. **"About FERB" self-knowledge** (owner, 2026-10-30): a read-only self-description generated from code and config so
   it never goes stale: version, models per role, permissions, current settings, what FERB records about the owner,
   where and for how long (from the retention rules), health (DB size, CPU) and cost; a short summary in every
   prompt, full detail through a worker lookup tool. App how-tos and shortcuts come from a small list kept with the
   UI. Settings stay the owner's to change in the app; FERB says where. (Today FERB knows its email, secret names,
   schedules, reminders, time meanings, paired profiles and tool groups, but not its settings, what it stores, its
   version or models, or how to do things in FERB.)
7. **FERB's own email account is an optional setup step in the app** (owner confirmed, 2026-10-30). FERB's own email account becomes an optional setup step in the app. Today it
   is per-user, not hard-coded: `[mailbox] address` and `[identity] browser/profile`, set only by the CLI
   `setup --identity` (cli/setup.py:1003-1020). Proposed: the app's setup offers "Give FERB its own Google account
   (recommended)" — the owner creates the account (account creation needs human verification) and signs it into a
   browser profile; FERB finds and pairs it. If skipped, FERB browses as the owner in a profile the owner picks;
   features that need FERB's own inbox (one-time codes for FERB's own sign-ups, sending from FERB's address) say they
   aren't set up. Note: the diagnostic-report address is hard-coded to the developer (reports.py:26).

8. **The app gains Restore, Delete saved secret/card and Uninstall** (owner, 2026-10-30), functional only (owner
   polishes the UI later): Restore lists FERB's Trash moves, backup copies and snapshots and restores one; Delete removes
   a stored owner secret or card (OwnerSecrets gains a delete); Uninstall removes FERB fully, Keychain items included.
   Daemon RPCs plus minimal app screens; the added UI pieces are listed for the owner.
9. **Jev stays off after an upgrade** even when a key exists (owner, 2026-10-30).

## What exists today (verified)

### Composition
- `Daemon.__init__` builds ~121 attributes; `start` (M:451-591) builds DB, stores, browsers, Mac/shell, agents
  (gateway + supervisor), sense, memory, Jev, timers, the RPC table (86 methods), voice, then opens the socket and
  resumes plans. Signal handlers only after `start` returns (M:666); a failure in `start` skips `shutdown`.
- **42 hooks set on the supervisor after construction** (M:909-944, 1404, 1549-1571, 2850-2855), each with a silent
  default (supervisor.py:518-638), so missing wiring degrades quietly; **17 gateway attributes** set from outside
  (M:882-902, 935-936, 1398, 3741, 4766) plus 3 the supervisor sets; `confirmations`/`destinations` None skips the
  prior-No check (fail open outside the daemon). Cycles: daemon ↔ gateway ↔ supervisor; `Perception(self)` holds the
  whole daemon; registry tools hold private daemon methods. main.py runs 54 raw SQL statements across 14 tables.
- Section headers in main.py are wrong (e.g. "Jev" holds environment, ps.* and runtimes).

### Events and the event loop
- `_publish` (M:4513-4532) redacts the same data up to 5 times, parses config.toml twice per trail event, and
  `notify_plan_event` loads every running plan per event. The notch subscribes with no filter and receives every
  trail event.
- **`Config.get` re-reads and re-parses config.toml on every call** (config.py:51-56, 73-76; 2.36 ms measured, 59 call
  sites, several hot): roughly 7% of idle daemon CPU (estimate). 9.9 KB of the 13 KB config is the persona proposal
  in `[setup]`.
- Daemon CPU since 10-24: ~9.2% of a core idle, ~15% busy; footprint up to 2.2-2.4 GB.
- No asyncio exception handler; unretrieved task exceptions go to unredacted stderr; several fire-and-forget tasks;
  `Timers.run` swallows exceptions (a raising reminder could block later timers; suspected).
- Blocking on the loop: `codesign` in the voice hello, `Sandbox.available()` at start, the retention pass on a 3 GB DB,
  `_facts()` profile discovery per binding, `_availability()` TCC checks per call.

### Disturbance sentinel
- Samples front app, key window, fullscreen, cursor, all windows, Space epoch: every 0.25 s while any tree runs, and
  before and after every tool call (read-only ones included), under one global lock. Sample time median 3.7 ms, p99
  611 ms, worst 16.2 s.
- **40,679 of 87,099 spans (47%) are `disturbance.change`; 74% of those are cursor-only (the owner moving the mouse).**
- Results go into agent prompts and traces only — no checker facts, no owner notices.
- Start/finish race leaves a new tree unsampled with no Space tracking (disturbance.py:169-186).

### RPC surface
- 86 methods. Dead: `questions.answer` (cli/chat.py:409-411; not registered), `memory.distill` (no caller).
- **`task.stop` with no ids stops every running tree** (M:3353), including scheduled runs and other chats; CLI `/stop`
  sends an empty list.
- **`chat.status` without `peek` marks every unshown result as shown** (M:3057-3059), so the CLI can consume the
  notch's results.
- `ps.list` returns raw commands (M:4067); `memory.learning op=retire` writes raw SQL; `op=search` runs on the loop.

### Crash and restart
- 168 `daemon.started` vs 160 `daemon.stopped` (~8 unclean exits). After a crash a plan is both marked interrupted
  (an "interrupted… pick it up again?" result + an episode, M:484-496) and auto-resumed (M:588; plan_store.py:91).

### Config and settings
- Privacy exclusions changed in Settings don't reach the Redactor, proxy or hearing until restart (M:842-847).
- Keys never read: `[setup] persona`, `[gates] owner_profiles`, `[policy] trusted_accounts`.

### CLI, setup, device
- The app uses `setup --service [--update]`, `--codex-login/--logout`, `--cli`; permissions, AI sign-in, browsers
  and persona go through daemon RPCs.
- **CLI `ferb setup` records `permissions_version = 27` unconditionally** (cli/setup.py:271), even after skipping
  required permissions; the app treats 27 as setup done. `--update` checks first (setup.py:217-224).
- **Checkout `setup --update` overwrites the venv under the running daemon**; if the Codex install fails, the old
  daemon keeps running mixed code (install.py:351-356). Checkout installs are unpinned (npm without the lockfile,
  `uv pip install` without `uv.lock`); the app build is pinned (build-app.sh).
- No rollback on failed setup. `ensure_codex` runs even when no role uses Codex.
- Tracebacks: `test-mode … --allow-send`, `conversations show` without id, `notifications label` without
  `--expected`, server-side errors shown as bare `ferb: ValueError`. `ferb logs --policy` streams everything.
- Doctor: dead "unregistered runtime" check; fails on Full Disk Access while setup defers it; checkout doctor on an
  app install reports false mismatches.
- `uninstall --purge` leaves API keys, the Jev key and owner-secret Keychain items.
- **Push-to-talk and FERB's identity profile can only be set up from the CLI**; spoken redirects tell the owner to use
  "ferb chat's hidden prompt".
- Dev-only code shipped: `device/probe.py` (768), `watch.py` + `detector.py` + watch_ext (~880), eval security/
  destructive/paired results; CLI permission walkthrough duplicates the app window; two chat renderers; two trace
  renderers; scan.py has its own 3-browser table.
- LaunchAgent plist at `~/Library/LaunchAgents/com.ferb.daemon.plist`, protected only by a liftable rule.

## Target design
1. **One composition function** builds stores first, then hands the gateway one deps object and the supervisor a
   few typed ports (memory, schedules, environment, desktop, browser, notify); no hooks set afterwards, no silent
   defaults — missing wiring fails at construction; a small question router breaks the gateway↔supervisor cycle.
2. **main.py split** by area: compose, memory glue, notify, schedules, browsers, runtimes, retention, egress, rpc/.
3. **One events object** that redacts once and fans out; the daemon filters topics by default (no notch change
   required).
4. **Config cached** and reloaded on file change; settings take effect without restart.
5. **Global asyncio exception handler** that redacts and journals; background tasks tracked and restarted or surfaced.
6. Heavy work off the event loop.

## Depends on
- notes/storage.md decision 5 (upgrade path): SMAppService plist removal, persona out of config,
  `permissions_version`, venv swap.
- The crash "interrupted + auto-resume" fix is owned by notes/orchestration.md.

## Fix list
- [ ] **The live daemon must never import code from a checkout** (found 2026-10-31): the installed
      `~/Library/LaunchAgents/com.ferb.daemon.plist` has `WorkingDirectory = /Users/kartikey/Desktop/FERB` and runs
      `python -m ferb.daemon`, which puts the working directory first on `sys.path`, so the owner's running FERB loaded
      the checkout's code instead of its installed copy (`~/.ferb/venv/.../site-packages/ferb`). The rebuild deleting
      `resources.md` made the scheduled visa run fail at 00:27 on 10-31. Fix: run with `python -P` (never prepend the
      working directory) and keep WorkingDirectory at FERB's home (install.py already writes `self.home`; the live
      plist predates that); SMAppService (decision 2) covers it too.
- [x] Composition function + typed ports; remove the 42 supervisor hooks and 17 gateway injections. (Step 2: stores
      first (`_build_stores`), gateway gets one `GatewayDeps`, supervisor gets `ports.py` Memory/Schedule/Environment/
      Desktop/Browser/Notify ports + a `QuestionRouter`; all fields required. Gateway refuses to start without recovery,
      confirmations, destinations or protection. `recall`/`relevant_source_refs` hooks removed. Left: the supervisor
      still sets `gateway.standing/conversation_guard/observer` in its own `__init__`; `test.mode` changes the
      allowlist at run time.)
- [x] Split main.py; correct sections. (Step 2: 15 mixins in `ferb/daemon/areas/`; main.py keeps state, lifecycle and
      errors; RPC table in `areas/rpc.py:_register_rpc`.)
- [x] Events object: redact once; plan events looked up by key, not every plan per event. (Step 2: `daemon/events.py`;
      plan topics cached per executor and plan revision. Topic filtering for the notch not done: needs the app.)
- [x] Config cache with mtime reload; persona proposal out of config.toml. (Step 2: reparse on file/owner/mode change;
      `persona.json`, moved from `[setup]` on first read.)
      (Step 9a: the persona step is replaced by memory decision 16: `persona.json` keeps `seen` (summary, facts,
      fingerprint, when); `setup.persona` answers state "seen" with `summary` and `facts`; no approval.)
- [x] Exception handler; fire-and-forget tasks tracked; timers isolate a failing consumer. (Step 2: `daemon.unhandled`,
      `daemon.background_failed`, `daemon.loop_failed`, `timer.failed` journal rows.)
- [ ] Blocking calls to threads (codesign, sandbox check, retention, profile discovery, TCC checks). (Step 2: all but
      retention. Retention's slow part is a span scan inside its delete transaction; a worker-thread writer would make
      the loop's writes wait on SQLite's lock. Fix with step 12: ids read off the loop, short delete batches.)
- [x] `chat.status` read-only (marking shown is explicit).
- [x] `ps.list` redacted; `memory.learning retire` through Learning; search off the loop.
- [x] Privacy settings applied live to the Redactor, proxy and hearing.
- [x] Remove `questions.answer` code, `memory.distill`, dead hooks, unread config keys.
- [ ] Setup: `permissions_version` only when granted; install into a new venv and swap after success (no mixed code);
      pinned checkout installs (lockfiles); Codex installed only if a role uses it; doctor fixes; uninstall removes
      every FERB Keychain item; CLI errors shown plainly.
- [x] Signal handlers before `start`; `start` failure runs `shutdown`.
- [x] Disturbance sampler around acting tools; owner cursor ignored; facts to the checker; race fixed (decision 5).
      (6b: samples only around `on_screen` calls plus a task baseline, no timer; front app, focus, fullscreen, Space
      only; per-call `screen` facts on the span and `screen_changes` in the summary; start/finish race closed; see
      notes/gateway.md Rulings (step 6b).)
- [x] Remove CLI chat; `task.stop` requires ids (decision 1).
      (Step 3: cli/chat.py, live_chat.py removed; `task.stop` without ids is refused (test_ux).)
- [ ] SMAppService registration (decision 2); rule-10 protection of the service definition follows.
- [x] CLI trim (decisions 3, 4); `ferb uninstall` stays in the toolbox. Remove the `voice` and `decide` commands.
      (Step 3: removed chat, voice, decide, conversations, notifications, test-mode, regress, watch and the Mac probe/watch harness (probe.py, watch.py, detector.py, watch_ext, watch_page.html), the permission walkthrough (`_grant`, `_automation`, `hearing_setup`, `clipboard_setup`, `voice_setup`; `ferb setup` records setup only when every permission is granted) and `trace --steps` (`render.steps`). Still in the CLI: schedules, reminders, timeline, pause/resume, stop/resume-tasks, kill, attach, device, baseline, release-checklist, setup --key/--jev/--identity/--persona/--claude-login.)
- [x] Remove the daemon push-to-talk stack together with CLI chat (they share CLI rendering; notes/voice.md decision 3).
      (Step 3: see notes/voice.md.)
- [ ] Identity-profile setup moves into the app (owner's UI side); `identity.check`/`identity.pair` RPCs stay.
- [x] Test-mode send allowlist kept in config for live send tests (CLAUDE.md rule 22); no CLI command.
      (Step 3: the `test.mode` RPC and config stay; the CLI command is gone.)
- [ ] "About FERB" source and worker lookup tool (decision 6).
- [ ] FERB email setup in the app (decision 7); identity RPCs for the app.
- [ ] `ferb logs --policy` filter fixed; main.py's raw SQL moved into the stores; investigate the 2.2-2.4 GB footprint.
      (Step 2: `--policy` fixed and an unknown filter refused; 13 of 54 statements moved (task_results, tasks lookups,
      task_memory_refs, learning retire, trail rows); the rest belong to areas later steps rewrite or remove. Footprint
      not investigated.)
