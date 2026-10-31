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
   Owner, 2026-10-31: this replaces the 2026-10-08 P27 rule 8 ("FERB doesn't recite its runtime, model names or
   design in chat"): FERB may talk about its models, settings and how it works when asked.
7. **FERB's own email account is an optional setup step in the app** (owner confirmed, 2026-10-30). FERB's own email account becomes an optional setup step in the app. Today it
   is per-user, not hard-coded: `[mailbox] address` and `[identity] browser/profile`, set only by the CLI
   `setup --identity` (cli/setup.py:1003-1020). Proposed: the app's setup offers "Give FERB its own Google account
   (recommended)" — the owner creates the account (account creation needs human verification) and signs it into a
   browser profile; FERB finds and pairs it. If skipped, FERB browses as the owner in a profile the owner picks;
   features that need FERB's own inbox (one-time codes for FERB's own sign-ups, sending from FERB's address) say they
   aren't set up. Note: the diagnostic-report address is hard-coded to the developer (reports.py:26). (2026-11-01:
   superseded by decision 10; reports go to the relay, the address is gone.)

8. **The app gains Restore, Delete saved secret/card and Uninstall** (owner, 2026-10-30), functional only (owner
   polishes the UI later): Restore lists FERB's Trash moves, backup copies and snapshots and restores one; Delete removes
   a stored owner secret or card (OwnerSecrets gains a delete); Uninstall removes FERB fully, Keychain items included.
   Daemon RPCs plus minimal app screens; the added UI pieces are listed for the owner.
9. **Jev stays off after an upgrade** even when a key exists (owner, 2026-10-30).

## Owner decisions (2026-11-01)
10. **Failure reports go through the developer's relay, not email** (owner, 2026-11-01; replaces the 2026-10-25 Mail
    path, docs-cleanup item 14): reports and diagnostic files become issues in the **private** GitHub repo
    `Json604/ferb-reports`, through a relay the owner owns (a Cloudflare Worker holding a fine-grained GitHub token
    limited to Issues read/write on that one repo, no expiration). The owner created the repo, the Cloudflare account
    and the token, and deploys the relay. **The user only presses Send in FERB**: no GitHub account, browser, Mail app or
    extra step; the user still previews the cleaned report before sending. The repo stays general (no personal data;
    the repo name and the relay's public URL are fine).
    Detail (owner, 2026-11-01, later the same day): **each Send is exactly one issue** holding a short summary (what
    failed, the errors and questions, FERB's version, the report id); **the full redacted report is one file in that
    repo** (`reports/YYYY/MM/<report id>….txt`), linked from the issue; no reply-comment threads. **Cap 25 MB**: a
    report over it is shortened by FERB to fit (the summary, every error and question and the last part of the trace
    kept; what was cut stated at the top); the full report always stays on the Mac. The token gains Contents: Read and
    write on ferb-reports only, beside Issues: Read and write; nothing else. The user still only previews and presses
    Send.

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
- [x] Report relay (decision 10, 2026-11-01). `relay/`: the Worker (`worker.mjs`, plain JavaScript, no build, no
      dependencies), `wrangler.toml` (worker `ferb-reports`, `GITHUB_REPO`, the `REPORT_LIMIT` rate-limit binding),
      `README.md` (the owner's deploy, test, rotate and delete steps) and `test.mjs` (`node relay/test.mjs`, fake GitHub).
      FERB: `reports.build` / `reports.diagnostic` only write the file (no address, nothing sent); the app shows View
      and Send; `reports.send {path, title, app_version}` posts that file. The Mail compose path, `FEEDBACK_ADDRESS`
      and `[feedback] address` are gone (config upgrade `feedback-relay-2026-11-01` drops the key once, journaled
      without its value). Waiting: the owner deploys the relay and sends its address for `reports.RELAY_URL`.
      (2026-11-01, decision 10 detail: one issue with a summary + the full report as one linked file, 25 MB cap, shorten
      to fit; two requests per Send, `/file` streamed through to GitHub's Contents API, then `/report` with a signed
      ticket. Waiting: the owner adds Contents: Read and write to the token and redeploys `relay/`.)
      (Tests: tests/test_failure_report.py, test_advanced_settings.py, test_config.py, test_upgrade.py;
      FailureReportTests, FERBWindowStoreTests.)
- [x] "What FERB saw" (owner, 2026-10-31, step 12e): a button on each message in the notch opens the screenshot taken
      when it was sent and the images its task captured, each exactly as the model got it, with time and app or site;
      Settings → Data lists every saved screenshot of the last 7 days, newest first. FERB.app-only RPCs over the stored
      redacted copies. (12e: see Rulings (step 12e); tests/test_what_ferb_saw.py, AppScreensTests.)
- [x] **The live daemon must never import code from a checkout** (found 2026-10-31): the installed
      `~/Library/LaunchAgents/com.ferb.daemon.plist` has `WorkingDirectory = /Users/kartikey/Desktop/FERB` and runs
      `python -m ferb.daemon`, which puts the working directory first on `sys.path`, so the owner's running FERB loaded
      the checkout's code instead of its installed copy (`~/.ferb/venv/.../site-packages/ferb`). The rebuild deleting
      `resources.md` made the scheduled visa run fail at 00:27 on 10-31. Fix: run with `python -P` (never prepend the
      working directory) and keep WorkingDirectory at FERB's home (install.py already writes `self.home`; the live
      plist predates that); SMAppService (decision 2) covers it too.
      (Step 12b: a checkout's LaunchAgent runs `~/.ferb/venv/bin/python -P -m ferb.daemon` with `WorkingDirectory` =
      FERB's home, and its native host `python -P`; FERB.app's definition (inside the app, below) runs the isolated
      agent with exactly `-m ferb.daemon`; the daemon itself makes FERB's home its working folder and sends output
      launchd left at /dev/null to logs/daemon.{out,err}.log (`main.service_streams`). Live check 2026-10-31: the
      running service was FERB.app's agent (build 0dd5185, `/Applications/FERB.app/.../FERB -m ferb.daemon`), not the
      hand-edited venv plist. Tests: tests/test_install.py.)
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
- [x] Blocking calls to threads (codesign, sandbox check, retention, profile discovery, TCC checks). (Step 2: all but
      retention. Retention's slow part is a span scan inside its delete transaction; a worker-thread writer would make
      the loop's writes wait on SQLite's lock. Fix with step 12: ids read off the loop, short delete batches.)
      (Step 12a: the pass reads what goes in a worker thread on a read-only connection (`retention.plan`), then deletes
      on the loop's connection 200 rows a transaction, the loop free between batches; first pass on a copy of the
      owner's store: longest loop gap 189 ms.)
- [x] `chat.status` read-only (marking shown is explicit).
- [x] `ps.list` redacted; `memory.learning retire` through Learning; search off the loop.
- [x] Privacy settings applied live to the Redactor, proxy and hearing.
- [x] Remove `questions.answer` code, `memory.distill`, dead hooks, unread config keys.
- [x] Setup: `permissions_version` only when granted; install into a new venv and swap after success (no mixed code);
      pinned checkout installs (lockfiles); Codex installed only if a role uses it; doctor fixes; uninstall removes
      every FERB Keychain item; CLI errors shown plainly.
      (Step 12b: `permissions.missing` over every row (the daemon's four, FERB Voice's hearing, the clipboard and the
      app's own Notifications/Microphone, which the app's Done now sends) decides both the window's `setup.finish` and
      `ferb setup`; nothing is recorded otherwise and `missing` says what isn't granted. A checkout installs into
      `~/.ferb/venv.new` (`uv venv --relocatable`, `uv export --frozen` → `uv pip install --require-hashes`, FERB with
      `--no-deps`), the secret helper into `.ferb-secrets.new`, Codex with `npm ci` from package-lock.json into
      `node.new` (version checked), and only then, with the daemon stopped, swaps the venv in (`_swap_venv`; a cut-off
      swap is finished next time); any failure leaves the venv, plist and running daemon untouched. `codex_wanted`:
      Codex is (re)installed only when a role uses it, or when the owner signs in to it. Doctor: every permission
      fails when off (none optional), Full Disk Access says it waits for containment, "installed code differs" only
      for a checkout's own daemon, the service definition check for FERB.app, no hints to removed commands.
      `uninstall --purge` deletes every item of FERB's Keychain services (`ferb`, `ferb.owner-secrets`, Claude Code's
      item for FERB's Claude home) by service, never reading a value. CLI errors: one plain line (`plain_error`), an
      internal daemon failure says so and points at `ferb stats`. Tests: test_install.py, test_setup_flow.py,
      test_setup_rpcs.py, test_cli.py.)
- [x] Signal handlers before `start`; `start` failure runs `shutdown`.
- [x] Disturbance sampler around acting tools; owner cursor ignored; facts to the checker; race fixed (decision 5).
      (6b: samples only around `on_screen` calls plus a task baseline, no timer; front app, focus, fullscreen, Space
      only; per-call `screen` facts on the span and `screen_changes` in the summary; start/finish race closed; see
      notes/gateway.md Rulings (step 6b).)
- [x] Remove CLI chat; `task.stop` requires ids (decision 1).
      (Step 3: cli/chat.py, live_chat.py removed; `task.stop` without ids is refused (test_ux).)
- [x] SMAppService registration (decision 2); rule-10 protection of the service definition follows.
      (Step 12b: ui/mac/FERBAgent/com.ferb.daemon.plist is copied by scripts/build-app.sh to
      FERB.app/Contents/Library/LaunchAgents/com.ferb.daemon.plist (BundleProgram = the agent, FERB_APP_BUILD set,
      no user paths) before signing. `ferb setup --service` from the app does the file work, retires the old user
      LaunchAgent (stopped, then removed; only a plist naming FERB's label), records `~/.ferb/service.json` (build,
      app) and restarts the service if macOS has it loaded; then the app registers it (`AppServiceAgent`:
      `SMAppService.agent(plistName:)`, "already registered" counts as on), waits until FERB answers, and shows macOS's
      "needs your OK" with an Open Login Items button, a missing definition or a refusal in the app's words
      (`ServicePlan`). Launch decides from the old LaunchAgent, the service status and the record
      (`AppSetupPlan.decide`). The clean-up's daemon-stopped work stops the app's daemon with `launchctl kill SIGTERM`
      (clean exit, so KeepAlive doesn't restart it) and `kickstart`s it after; uninstall unregisters it from the app.
      A checkout refuses to take over while launchd runs the label from the app's definition. Protection:
      `BUNDLE_SERVICE` joins `service_files` (FERB_SERVICE, never lifted, the shell profile denies writing it);
      the user plists were already FERB_SERVICE. Tests: test_install.py, test_protection.py, AppSetupTests,
      SetupModelTests.)
- [x] CLI trim (decisions 3, 4); `ferb uninstall` stays in the toolbox. Remove the `voice` and `decide` commands.
      (Step 3: removed chat, voice, decide, conversations, notifications, test-mode, regress, watch and the Mac probe/watch harness (probe.py, watch.py, detector.py, watch_ext, watch_page.html), the permission walkthrough (`_grant`, `_automation`, `hearing_setup`, `clipboard_setup`, `voice_setup`; `ferb setup` records setup only when every permission is granted) and `trace --steps` (`render.steps`). Still in the CLI: schedules, reminders, timeline, pause/resume, stop/resume-tasks, kill, attach, device, baseline, release-checklist, setup --key/--jev/--identity/--persona/--claude-login.)
      (Step 12b: those removed (cli/attach.py, eval/comparison.py, their admin functions and tests); `setup
      --claude-logout` stays because FERB.app now runs it (Settings → AI → Sign out of Claude); `ferb.cli.APP_COMMANDS`
      lists what the app runs and a test keeps them parsing. The daemon's failed-sign-in text no longer names
      `ferb setup --claude-login`. Identity-profile setup has no path until the app's screen exists (decision 7).)
- [x] Remove the daemon push-to-talk stack together with CLI chat (they share CLI rendering; notes/voice.md decision 3).
      (Step 3: see notes/voice.md.)
- [x] Identity-profile setup moves into the app (owner's UI side); `identity.check`/`identity.pair` RPCs stay.
      (Step 12c: see "FERB email setup in the app" below.)
- [x] Test-mode send allowlist kept in config for live send tests (CLAUDE.md rule 22); no CLI command.
      (Step 3: the `test.mode` RPC and config stay; the CLI command is gone.)
- [x] "About FERB" source and worker lookup tool (decision 6).
      (Step 12c: ferb/daemon/about.py + areas/about.py. One document built on each read from code and config: version,
      models per role (config, runtime table defaults), FERB's own account, permissions (the daemon's own grants),
      every setting FERB.app changes with its value and where it lives, what FERB keeps / where / how long
      (`retention.describe` over the new rule table `retention.RULES`, which the hourly pass now reads its windows
      from, and which settings.py takes its retention defaults from), health (database size, clean-up waiting,
      CPU share and memory over the last day from `resource_samples`), cost (priced model calls in traces, 7 and 30
      days, API-equivalent) and FERB.app's how-tos and shortcuts. The always-on line (`about.summary`: version, models,
      where settings live, the tool) is the first line of "Facts about this FERB" (`_facts`), the shared path every
      role's instructions take; no live numbers. Workers read the whole document with `about.read` (observation,
      gateway, redacted; checkers may use it). FERB.app's Settings → About reads `about.get`. The how-tos, shortcuts and
      the setting places are a list kept with the UI (NotchCore `AppGuide`), which FERB.app hands to the daemon with
      `about.guide` (APP_ONLY) on each connection; kept in memory. Tests: tests/test_app_screens.py, AppScreensTests.)
- [x] FERB email setup in the app (decision 7); identity RPCs for the app.
      (Step 12c: an optional setup step after Browsers, "Give FERB its own Google account (recommended)", also Settings
      → Browsers → FERB's own account. `identity.find {address}` lists every installed browser's profiles signed into
      that address (Local State + Preferences only) and whether each holds it alone; `identity.choose` checks the pick
      with `seed.verify` and writes `[mailbox] provider/address` and `[identity] browser/profile` in one write through
      `settings.write_identity` (journaled `settings.changed`, key identity; `clear` removes them); then
      `identity.pair` as before. All three are APP_ONLY. Skipped: the owner picks one of their paired profiles
      (`browsing.owner_profile`, a new setting) and the browser resolver uses it for an owner task with no profile
      named, only while FERB has no profile of its own; the facts line says FERB has no account of its own, what isn't
      set up, and which profile web work uses (prompts decision 5 reconciled: FERB's own profile stays the default when
      it exists). `identity.check` reads the configured browser's data folder (it read Chrome's always). The
      diagnostic-report address stays hard-coded to the developer (reports.py:26), unchanged.) (2026-11-01: decision 10
      replaced the address with the report relay.)
- [x] Restore, Delete saved secret/card and Uninstall in the app (decision 8).
      (Step 12c: Settings → Data → Restore lists `recovery.list` (copies, Trash moves, snapshots: snapshots say how to
      open them) and restores with `recovery.restore`, the same handlers `ferb restore` uses. Settings → Data → Saved
      secrets and cards: `secrets.saved` (place, name, kind; never a value, Keychain account or username) and
      `secrets.delete {place, name}` (both APP_ONLY), after one question in the app; `OwnerSecrets.delete` deletes the
      Keychain item through the signed helper's new `delete` operation (never read first; an item another entry still
      names stays) and drops the name, journaled `secret.deleted` with place and name. Settings → Advanced → "Uninstall
      FERB and delete all its data…": one question in the app, then `ferb uninstall --purge --yes` (the app's service
      unregistered first, as plain uninstall does). Tests: tests/test_app_screens.py, AppScreensTests.)
- [x] `ferb logs --policy` filter fixed; main.py's raw SQL moved into the stores; investigate the 2.2-2.4 GB footprint.
      (Step 2: `--policy` fixed and an unknown filter refused; 13 of 54 statements moved (task_results, tasks lookups,
      task_memory_refs, learning retire, trail rows); the rest belong to areas later steps rewrite or remove. Footprint
      not investigated.)
      (Step 12b: tasks lookups, the task page, verdicts and a schedule's last result into TaskTree/TaskResults; journal
      reads into Journal (`recent_values`, `by_ids`, `latest_kind`, `of_running_tasks`); timer deliveries into Timers;
      memory questions into ferb/memory/questions.py; the dead `reminders.unseen` RPC removed. Left in the areas: the
      memory area's card/search/activity reads (9, the memory stores' own seams), Jev's 3 (switched off), PRAGMAs and the
      schedule transaction. Footprint: see "Footprint (step 12b)".)

## Report relay (2026-11-01, decision 10)
Rulings (technical):
- **Flow**: Send to developer → `reports.build` writes the redacted file as before (the 2026-10-24 design) → the notch
  row shows "View report" (opens the file) and "Send" → `reports.send` → "Sent", or "Couldn't send: <why>. The report
  is saved; you can send it again." with **Send again**. Settings → Advanced → Send diagnostics to developer works the
  same way (`reports.diagnostic` now returns a `subject`). One try per press; nothing retries by itself, except the
  file again on a commit conflict (below). The app's flow is unchanged; it waits up to 600 s (`ReportSender.sendWait`)
  since a 25 MB report goes up on the owner's connection; the daemon gives up by itself at 540 s and says so.
- **What is sent** is the saved file the owner viewed, through the Redactor's text pass once more (a value sealed
  since the build is hidden too; the saved file is rewritten to match, so what is kept is what was sent). `send` takes
  only a regular file named like a report or diagnostic file directly in `~/.ferb/reports` (no symlink), read with
  `O_NOFOLLOW`. `report_id` is the file's name without `.txt`.
- **Shorten to fit** (`reports.shorten`): over `MAX_REPORT_BYTES` (25,000,000 bytes of UTF-8) the sent copy keeps
  everything outside Steps (Summary, Request, FERB's recorded problems), every step whose recorded details hold an
  error, every question (an `owner.ask` call, FERB's own confirmations included, and `model.question`), and as many of
  the last steps as fit; each run of cut steps leaves "[FERB cut N steps here to fit 25 MB]" and a note at the top
  says how many steps (and MB) were cut and that the full report stays on the sender's Mac. If even that is too big
  (or a file has no Steps), the middle is cut and the beginning and end kept. Structural facts only (span kind, tool
  name, an `"error"` that isn't null), never a judgment of content. The saved file stays whole.
- **Summary** (`reports.summarize`, from the same redacted text, through the text pass again, ≤ 30,000 characters):
  the shortening note when there is one, the report's Summary section (versions, task, status, failure reason, times),
  "Errors (n)" (one line per step with an error: time, kind, tool, the error), "Questions (n)" (the question and its
  answer) and "FERB's recorded problems (n)", one line each; "… n more in the full report" past the cap.
- **Protocol** (two requests per Send, so the Worker never parses or re-encodes the big file: the Workers free plan
  allows 10 ms CPU per request, and waiting on fetch doesn't count):
  1. `POST <relay>/file?kind=…&report_id=…`, body = exactly GitHub's "create a file" JSON
     `{"message", "content": base64}` built by FERB (`reports.prepare`), `Content-Length` ≤ 33,337,432. The Worker
     checks only the headers, the query and the rate limit, picks `reports/YYYY/MM/<report_id>-<8 random>.txt` (UTC
     month; the random part because ids repeat across Macs, and a path that never existed can't be overwritten by a
     body naming a `sha`) and streams the request body to `PUT /repos/{repo}/contents/{path}` untouched. 201 →
     `{"ok": true, "ticket"}`: path, kind, report id and expiry (1 h), HMAC-SHA-256 signed with a key derived from
     the token. GitHub 409 (another commit landed at that moment; the Contents API conflicts on concurrent commits) →
     relay 409 `retry: true`: the relay can't replay a streamed body, so FERB sends the file again, up to
     `CONFLICT_TRIES` = 3, 1 s then 2 s apart.
  2. `POST <relay>/report` `{"version": 2, "ticket", "title", "summary", "app_version"}` ≤ 64 KB: shape and ticket
     checked (forged, changed or expired → 403; a ticket already filed in that instance → 409), then one issue: a
     "Full report: [path](https://github.com/{repo}/blob/HEAD/{path})" line (built only from the Worker's own path),
     then a code block with "FERB <kind> report <id> · app <version>" and the summary. Kind and report id come from
     the ticket, not the request. Fixed on the way: the old head line showed `app_version` outside the code block, so
     an `@name` in it could notify; now every sender-supplied text is inside the block.
  Why the Contents API and not the Git Data API (blobs/trees/commits/refs): one call, works on an empty repo (it makes
  the first commit; the Git Data API refuses an empty repo), and its documented file limit is 100 MB. Its JSON-only
  base64 body is why FERB builds the exact body and the Worker passes it through.
- **Where**: `reports.RELAY_URL` (empty until the owner's relay is deployed), overridden by config `[feedback]
  relay_url`: the relay's own address (an address ending in `/report`, the first relay's endpoint, works too); https
  only (plain http only to 127.0.0.1/localhost, for tests); none → "Sending reports to the developer isn't set up
  yet" and the file is kept. The address never reaches the app.
- **How**: the daemon sends asynchronously with httpx (the file read, redaction, shortening, summary and base64 run
  in a thread), 60 s per connect/read/write step, 540 s for the whole Send, like its other outbound calls (model APIs,
  Jev): direct, not through FERB's egress proxy, which is for FERB's shell commands. Journaled `report.sent` (file,
  kind, report_id, shortened) or `report.send_failed` (file, kind, why); never the text. The reply adds `shortened`.
- **Relay limits kept**: only `POST /file` and `POST /report`, `application/json` (415), wrong method 405, other
  paths 404, no token 503; per sender address, files and issues counted apart, 2 a minute per Cloudflare location
  (the binding allows only 10 or 60 s windows) plus 5 per 10 minutes per running instance (best effort); never logs a
  body; never answers with the issue URL or number (the ticket names the file's path in the private repo, useless
  without access). Labels: the kind and the app version.
- **Limits verified (2026-11-01)**: Cloudflare Workers free plan: 10 ms CPU per HTTP request, waiting on `fetch()`
  not counted; request body 100 MB on a Free/Pro account plan; 128 MB memory; 50 subrequests (one Send uses one per
  request); no duration limit while the client stays connected
  (developers.cloudflare.com/workers/platform/limits/). GitHub: create-or-update-file-contents limit 100 MB
  (github.blog changelog 2022-05-03); a request over 10 s of GitHub's processing is cut off
  (docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api). Not documented, to confirm live: that
  streaming a body straight into `fetch()` costs no CPU per byte, and that GitHub takes a ~33 MB Contents request
  within its limits (a 2025 community report saw 422 "too large" at 50 MB and success at 10 MB).

Added UI pieces (functional only, for the owner to polish): the notch's report row (View report, Send / Send again,
the status line); Settings → Advanced → Diagnostic file (View diagnostic file, Send / Send again, the status line).

Live checks for step 13: notes/test-tasks.md sitting 3 ("Send a failure report from FERB").

## Rulings (step 12c, technical)
- **About's three readers, one builder**: the always-on line rides in `_facts` ("Facts about this FERB"), which
  `Supervisor._system` adds for every role: one shared path, no per-role text (notes/prompts.md contract: a fact line,
  not a prompt rule). It holds version, models per role and where settings live; it changes only when the owner changes
  a model or FERB is updated, so the cacheable prefix holds between calls. Live numbers (size, CPU, cost) are only in
  the document.
- **The retention rule table** (`retention.RULES`) is load-bearing: the hourly pass's windows (`retention.windows`),
  the retention settings' defaults (settings.py) and the About rows (`retention.describe`) all read it. Rows with no
  window (forever, until deleted) and the Keychain row are in it so "what FERB keeps about you" is one table.
- **The app guide** lives in NotchCore (`AppGuide`), beside the screens it names; FERB.app sends it with `about.guide`
  (APP_ONLY: only FERB.app's words become FERB's guide) on every connection, the daemon keeps it in memory (until the
  app connects after a daemon start the document says so). Setting places come from the same list.
- **Cost** is the API-equivalent price of the model calls traces carry (spans age out at 30 days, so 7 and 30 days are
  what it can say); a subscription is billed by its plan, as the document states.
- **APP_ONLY** (auth.py) now also covers `secrets.saved`, `secrets.delete`, `identity.find`, `identity.choose`,
  `identity.pair` and `about.guide`. `recovery.list` / `recovery.restore` stay on the cli role: the developer
  toolbox's `ferb restore` (decision 4) uses them, and the owner-uid socket check already keeps FERB's own shell out
  (sandbox.py names the socket). Owner question below.
- **Uninstall and the clean-up are commands, not RPCs**: both stop the daemon. `--yes` (with `--purge`) means the owner
  answered FERB.app's one question; the CLI still asks for "delete" without it. FERB's own shell can't use either to
  any effect: its sandbox denies FERB's home, the Keychain service (`com.apple.SecurityServer`), signals outside its
  sandbox and job creation.
- **Deleting a saved secret** goes through the signed helper that made the item (`delete` operation, SecItemDelete by
  service and account), so macOS needs no extra approval and no value is read; an item not found counts as deleted.
  A FERB.app built before this step has an older helper: delete fails until `ferb setup --update` rebuilds it.
- **FERB's own account** is written only by `settings.write_identity` (one atomic write of the four keys) after
  `seed.verify` saw exactly that account in that profile. The account itself is the owner's to create (human
  verification); FERB only finds and pairs it.
- **Browsing as the owner without FERB's own profile**: `browsing.owner_profile` ("browser/folder", the owner's pick in
  the app) is used by the resolver only when no FERB profile is configured and that profile is paired; with FERB's own
  profile configured nothing changes (decision 5's default).
- **Notice cards stored with the notice** (migration 0057, `attention_notices.cards_json`): a notice read from
  `chat.status` after the app was away shows its cards too, not only the live event.
- **The doctor's "waiting for your OK"** reaches the app as `doctor.check`'s `database {bytes, cleanup_pending}` (Health
  line "Database") and in About's health; Settings → Data shows it above the clean-up.

## Added UI pieces (step 12c; functional only, for the owner to polish)
- Settings → About (new tab): FERB (version, models, own account), what FERB keeps / where / how long, permissions,
  your settings with where each lives, health, cost, shortcuts, how-tos.
- Settings → Data (new tab): Restore list with Restore buttons; Saved secrets and cards with Delete… and its one
  question; Clean up (preview list, sizes, OK, progress line, result) with the "waits for your OK" line.
- Setup window: the optional "Give FERB its own Google account (recommended)" step (address field, Find its profile,
  candidate rows with Use this profile, Pair it / Stop using it, and the "browses as you in" profile picker).
- Settings → Browsers: the same as a "FERB's own account" section.
- Settings → Advanced: "Uninstall FERB and delete all its data…" button and its alert.
- Settings → Privacy: an "Operational records" days stepper (the setting existed without one).
- Settings → Advanced → Health: a "Database" line when the clean-up waits.
- Notch: a memory import notice shows its example note cards and an "Open the Memory page" button; the Memory page
  shows "Showing what FERB imported from …" with Show everything.

## Open owner questions (step 12c)
- Restore stays a cli-role method (the developer toolbox's `ferb restore` uses it); making it FERB.app-only would
  remove `ferb restore`. Keep as is, or FERB.app-only?

## Live checks for step 13 (step 12c)
- Settings → About: version and models match Settings → AI; "What FERB keeps" lists 30-day traces (change Task traces
  to 20 days on the Privacy page; About shows 20); Health shows the database size; Cost shows two lines. Ask FERB "how
  long do you keep my traces and where do I change it?": the answer cites 20 days and Settings → Privacy (the worker's
  `about.read` in its trace).
- `ferb trace` of any task: the planner's instructions carry the "About FERB" line once; two turns in a row have the
  same instructions prefix (prompt cache hits in the trace's usage).
- Setup window from the menu: the new FERB's own account step after Browsers. With FERB's account signed into its own
  profile: type the address, Find its profile, Use this profile, turn the extension on if asked: "set up and paired";
  config.toml has `[mailbox] address` and `[identity] browser/profile`; `ferb doctor` ✓ identity profile. Stop using
  it: the keys go. Skip instead and pick a paired profile: a request like "open example.com" with no profile named
  opens in that profile.
- Settings → Data → Restore: ask FERB to trash a disposable file on the Desktop, then Restore it from the list.
- Save a disposable secret during a test login, then Settings → Data → Delete… it: Keychain Access no longer lists
  the item (service ferb.owner-secrets); `ferb trace`/journal show `secret.deleted` with only place and name.
- Settings → Data → Clean up on the live store: the list, OK, FERB stops and comes back, the result line; Health's
  Database line clears.
- A memory import notice: Open the Memory page opens FERB's window on Memory filtered to that assistant.
- Uninstall and delete everything only on a disposable install (another macOS user), never the owner's.

## Rulings (step 12b, technical)
- **Dev launch path for a checkout**: the same label as a user LaunchAgent in ~/Library/LaunchAgents
  (`Installer.plist`, `-P`, WorkingDirectory = FERB's home), loaded with launchctl. FERB.app never writes one: its
  definition is the app's, registered by the app. A checkout refuses to install while launchd runs the label from
  FERB.app's definition (only the app can unregister it); an old app plist is still taken over as before.
- FERB.app's definition can't name the owner's home (it is fixed and signed), so it has no WorkingDirectory or log
  paths: the daemon `chdir`s to FERB's home and opens its logs itself when launchd left stdout/stderr at /dev/null.
- The setup command stays the app's file step (`ferb setup --service`); registration is the app's (Python can't call
  SMAppService for the app's bundle). The old user LaunchAgent is retired by that command, before the app registers,
  because launchd can't hold the same label twice.
- "Daemon stopped" for FERB.app's service = `launchctl kill SIGTERM` + wait for its process, then `launchctl
  kickstart`: a clean exit isn't restarted (KeepAlive SuccessfulExit false) and the registration stays untouched
  (`bootout`/`bootstrap` can't reload a BundleProgram definition).
- Which build is set up: `~/.ferb/service.json` (build, app path, label), written by the setup command, read by the app.
- A checkout's new venv is built `--relocatable` at `venv.new` and renamed into place with the daemon stopped (old one
  removed after); every checkout update builds a fresh venv from uv.lock (hashes required) and restarts by unload +
  load, not kickstart.
- Codex is installed when a role uses it (Codex when unset) or when the owner signs in to it (`--codex-login`
  installs it first); a link into FERB.app follows the same rule.
- Uninstall's Keychain clean-up deletes by service (`security delete-generic-password -s SERVICE`, repeated until none
  is left, at most 10,000): FERB never lists or reads a value. Only `--purge` does it (the owner typed `delete`).
- Permission rows: every row reports `required: true` (field kept for older apps); "setup done" needs every row
  granted, the app's own included (the app sends them with Done); Full Disk Access blocked by containment counts as
  not granted.
- Embedding model: FERB.app ships it at Contents/Resources/models/embeddings/potion-base-8M@bf8b056651a2 (build-app.sh
  fetches it once with the pinned hashes into its build cache and copies it in); the daemon reads it there and never
  fetches. A checkout's setup fetches it at install (`Installer.fetch_model`); a failure is said and doesn't stop the
  install; that daemon keeps its one-time fetch as the fallback. Every load checks the hashes.
- Provider HTTP refusals say a plain cause from the status code first, then the provider's words (`api.http_cause`).
  Runtime errors point at Settings → AI, never a `ferb` command or config.toml.

## Footprint (step 12b, read-only: resource_samples and ps, 2026-10-31)
- What is measured: the daemon process's own physical footprint (`ri_phys_footprint`). Not the database: no `mmap_size`,
  and clean file pages don't count; children stay small (≤ 250 MB, `child_footprint`).
- It only grows, then drops at a restart: 10-30 rose 1.48 GB (00:01) → 8.47 GB (23:55); the 00:47 restart on 10-31 went
  back to 337 MB; this morning's process (11:09) 293 → 686 MB by 15:38; `ps` RSS 507 MB at 16:00.
- Two parts: ~100-160 MB an hour with nothing happening (no events, no tool calls, ~1% CPU: 10-30 14:12 → 20:43,
  6.99 → 8.03 GB), and 200-400 MB per 5-minute window while worker tasks run (10-30 08:00-09:20: 3.07 → 6.40 GB over
  ~6 tasks, never returned).
- The live process is the old build (FERB.app 0dd5185, 10-28, with Jev shadow on), so the growth is a leak in long-lived
  process state, not caches that page out. Measured here (scratch scripts, not the daemon): PyObjC calls on threads
  with no autorelease pool leak 300-450 bytes a call (NSWorkspace app list, permission checks), 55-90 with a pool:
  real but ~1-2 MB an hour at today's call rates, too small to explain it. The cause needs a measurement inside the
  daemon (tracemalloc or `footprint`/`heap` on the process), which this step did not do (no attaching to the owner's
  process); the step-13 check below repeats the read on the rebuilt code first.

## Footprint (step 12d: measured on the checkout's code)
- Idle, reproduced on the checkout: a test daemon in a temporary home (`preload_secrets=False`, fake model, no browsers,
  watchers off, the real clock, resource sampling and retention on) stayed at 108.3 MB for 25 minutes (one reading a
  minute; tracemalloc's traced memory flat as well). The idle growth does not reproduce without the watchers, and the
  watchers can't be measured faithfully from a test process: Accessibility answers it nothing (system-wide focused app:
  -25204), so the AX paths return early.
- Tasks, reproduced on the checkout: 400 fake tasks (planner, one worker with 8 file tool calls, the checker) after the
  first 25 (warm-up 108 → 191 MB): 191.3 → 203.4 MB, ~32 KB a task, gc objects +10 a task; nothing near the live
  200-400 MB per 5 minutes. Python objects are not the holder.
- Native, measured per call on one long-lived worker thread without and with an autorelease pool (scratch scripts on
  this Mac, macOS 27.2): on-device text recognition of a 1440×900 image (`recognize_text`, every image FERB sends a
  model or keeps passes through it) **210 KB a call without a pool, 2-8 KB with one**; NSRunningApplication by pid 269
  bytes vs 0; an Accessibility window read (`app_window`) 380 vs 0; an AX event's facts 72 vs 33; FSEvents callbacks
  19 bytes an event. FERB ran all of these on threads that never drain a pool (asyncio's default executor, watcher
  polls, native run-loop threads), so every image redacted during a task stayed in memory: a task taking screenshots,
  window pictures and recording frames keeps hundreds of them, which is consistent with the per-task growth seen live
  (an estimate, not measured on the live process: a Retina capture has about 4× the measured image's pixels).
- Fixed (ferb/device/autorelease.py): the daemon loop's default executor drains one pool per job (`DrainingExecutor`,
  so every `asyncio.to_thread` call: image redaction, screenshots, permission and app reads), and so do each watcher
  poll step, each run-loop slice of the Accessibility and workspace watchers, each FSEvents callback and the owner-input
  tap's text capture. Guard: tests/test_daemon_foundation.py
  `test_work_on_the_daemons_threads_drains_its_autorelease_pool` (an object autoreleased in a job is freed when the job
  ends).
- Found on the way: on macOS 27.2 Vision refused the Python `{}` FERB passed as its options ("key does not exist"), so
  every recognition failed and every image was withheld ("image redaction unavailable; pixels withheld"); the options
  are an NSDictionary now (ferb/sanitize/images.py). Guard: tests/test_screenshot_redaction.py
  `test_on_device_recognition_reads_an_image_and_its_secret_is_painted_over` (real recognition on a drawn image). Tests
  had stubbed recognition everywhere, so nothing caught it.
- The live process is still the old 0dd5185 build (Jev shadow on); the step-13 read of `resource_samples` after a day
  on the new build says whether idle and per-task growth are gone.

## Rulings (step 12d, technical)
- One autorelease pool per unit of work on FERB's long-lived threads (above). The owner-input tap's callback itself is
  not wrapped (it returns the event to the system); only its text capture, which reads Accessibility, is.
- Standing permissions are built once in composition and passed to the gateway in `GatewayDeps.standing`; the
  supervisor uses the same store. The supervisor's observer and conversation guard reach the gateway through one
  `Gateway.attach` call (notes/gateway.md).
- Removed as dead after the test deletion (no caller left in ferb/): `ferb/eval/release.py`, `regress.py`,
  `destructive.py`, `eval/sets/` (the owner-acceptance and security catalogues; the P20 injection eval is
  tests/security/test_planner_injection_real.py with tests/support/injection_scenarios.py), `eval/security/` (its trace
  scanner moved to tests/support/trace_scan.py for tests/security/test_catalog_replay.py), `context.assemble_context`
  and `assemble_parts`, `learning.prompt_json` and `line_safe`, `setup._voice_name`, `profile.methods_for`,
  `eval.memory.paired_results`, `supervisor._env_text`, `tls._commands`, `proxy.parse_sni`, `profiles.has_extension`,
  `recovery.mark_written` (the async one stays), and five unused imports.

## Rulings (step 12e, technical: "What FERB saw")
- The owner wants to see the screenshots FERB kept. Two RPCs, both in auth.APP_ONLY (FERB.app's own process, checked by
  code signature; any other program as the owner gets "FERB.app only"), in ferb/daemon/areas/about.py beside the saved
  secrets: `seen.list {request_id? | task_id?, limit?}` lists stored images, newest first, with their facts (time,
  kind, app, site = the URL's host, page/window title, tool), and `seen.image {id}` returns one stored image's bytes.
  The list is app-only too: its titles and sites are the owner's own screen.
- Only the evidence store's stored copies are served: exactly the image the model got (front windows are kept after
  F5 redaction, tool images after masking and the sanitizer, call_hooks.keep_image). Nothing is captured for this.
  Only images (`mime image/*`: never a recording's video, never a data-only row whose image was withheld) of the
  evidence window (`retention.evidence_days`, 7). An image of an app or site on the exclusion list *now* is never
  listed or served (the owner may add one after it was kept): its app/bundle id, its URL's host, the redactor's
  excluded label or an excluded origin label.
- One message's screenshots: its front window (evidence keyed by the request id) and every image the tasks of its
  request captured (`tasks.request_id`/`tree_id`). A task this chat didn't send: by its tree. Settings → Data: all.
- `EvidenceStore.stored_file` is the one way the RPC reaches a file (inside the store only; the read runs off the loop).

## Rulings (12e review, technical)
- **Uninstall waits for the service to unregister.** FERB.app's Uninstall (Settings → Advanced, plain or Delete
  Everything) awaits `SMAppService.unregister()` and then checks the status (`unregisterForUninstall`): only
  not-registered or not-found goes on to `ferb uninstall [--purge --yes]`. A throw or a service macOS still lists
  shows why in the output lines and nothing is uninstalled (before, the unregister was fire-and-forget and the
  uninstall started at once). FERBWindowViewTests `testUninstallWaitsForTheServiceToUnregisterAndStopsIfItDidnt`.
- **A checkout never uninstalls over FERB.app's service.** `Installer.uninstall()` refuses, before removing anything,
  while launchd runs FERB's label from a definition that isn't the checkout's LaunchAgent (`foreign_service`), as
  `install()` already did (one `check_not_foreign`). Refusing rather than leaving the service alone and removing the
  rest: the rest (native-host manifests, `--purge` of ~/.ferb) would break the FERB.app install that is running, and
  booting it out left FERB.app's registration with nothing running; only FERB.app can unregister it.
- **Nothing runs while the stopped daemon is still exiting.** `while_stopped` (a checkout's venv swap, the store
  clean-up, setup's store work; both the checkout's unload and FERB.app's stop) checks `exiting()` after the stop's
  10 s wait and raises InstallError instead of running the work; the daemon is started again either way. The clean-up
  reports it as before ("hasn't exited since launchd stopped it", exit 1).
- **Clearing FERB's own account (`identity.choose {clear}`)** removes `[mailbox] provider`, `[mailbox] address`,
  `[identity] browser` and `[identity] profile` in one atomic save (`settings.IDENTITY_KEYS`, the same four the
  choice writes), and detaches FERB's browser: its `ferb_browser.*` tools are unregistered and the tabs it opened
  close (no task could release them through it any more). Its pairing stays in Browsers; choosing and pairing again
  attaches it.
- **Autorelease pools on the event loop's thread.** `_macos_events` drains one pool per run-loop turn, and
  `_running_apps` (the setup permissions step and the planner's environment port call it on the loop) drains its own.
- **`setup.finish` is FERB.app's alone** (auth.APP_ONLY): only the Swift app calls it, and it records setup done from
  the grants FERB.app reports for itself. `ferb setup` (the CLI) no longer records `permissions_version`: every row
  decides it, FERB.app's own Notifications and Microphone included, and the CLI can't read those; it still lists
  what the daemon lacks.
- **A daemon not run from FERB.app protects the installed one.** For a daemon run from ~/.ferb/venv or a checkout,
  `protection.load` closes the installed FERB.app (`paths.installed_app`: /Applications, then ~/Applications, with
  FERB's bundle id; the lookup `AppPeers` uses) like the bundled one: never written or removed by FERB's tools or its
  shell, and its service definition counts as FERB's (rule 6). Read at daemon start.
- **Not built (needs the owner's choice): pinning FERB.app's identity in "installed" mode.** `AppPeers` derives the
  requirement again from whatever bundle with FERB's id sits at /Applications/FERB.app, so another program running as
  the owner that replaces that bundle passes the app-only check. Pinning at setup (as `sealed_requirement` does for
  the secrets helper) needs a choice: what to pin (the certificate leaf survives release and local-certificate updates;
  an ad-hoc build's code hash changes every build and would lock the notch out until setup runs again) and who
  records it again after an app update in that mode.

## Added UI pieces (step 12e; functional only, for the owner to polish)
- Notch: a small "What FERB saw" text button under each of the owner's messages (and under a task this chat didn't
  send), opening the "What FERB saw" window.
- "What FERB saw" window (SeenViewer.swift `SeenWindowController`, hidden from screen sharing): a list of that
  message's screenshots, newest first, each row the time, the site or app (and page title) and which capture it was;
  clicking a row opens the image inline, as the model got it.
- Settings → Data → "What FERB saw": the same list for every screenshot of the last 7 days, newest first, with
  "Showing the newest N of M" when long, and Refresh.
- The app guide (`AppGuide.howtos`) names both places.

## Live checks for step 13 (step 12e: "What FERB saw")
- Send a message with a browser window in front, then one that makes a task take a screenshot: the notch's "What FERB
  saw" under each opens its window; the first lists your front window (time, site, page title) and opening it shows
  the redacted picture (a password field painted over); the second also lists the task's screenshot.
- Settings → Data → What FERB saw lists both, newest first; nothing older than 7 days.
- Add the app in one picture to Settings → Privacy → excluded apps: it disappears from both lists.
- `ferb trace`/the journal show no new capture from opening the viewer; calling `seen.list` from a terminal
  (`ferb` developer toolbox or a socket client) gets "FERB.app only".

## Live checks for step 13 (step 12b)
- Moving from the hand-edited LaunchAgent to SMAppService: build FERB.app (`scripts/build-app.sh`), install it in
  /Applications, open it. Expected: launch decides `.update` (the old ~/Library/LaunchAgents/com.ferb.daemon.plist runs
  this app's agent) or asks first if that plist runs a checkout ("Set up FERB from this app?" → Set Up From This App);
  the setup command retires the plist (gone from ~/Library/LaunchAgents), macOS lists FERB under System Settings →
  General → Login Items → "Allow in the Background" (allow it if the window says macOS needs your OK), and the notch
  answers. Then: `launchctl print gui/$UID/com.ferb.daemon` shows `path = /Applications/FERB.app/Contents/Library/
  LaunchAgents/com.ferb.daemon.plist`; `ps` shows the agent with `-m ferb.daemon`; `~/.ferb/service.json` names the
  build; `lsof -p <pid> | grep cwd` shows ~/.ferb; logs/daemon.err.log receives output.
- Restart FERB from the menu, log out and in: the daemon comes back by itself. Switch it off in Login Items: the next
  launch shows the setup window with the approval note and the button.
- `ferb setup --cleanup --ok …` with the app's service: the daemon stops (`launchctl kill`), the clean-up runs, it starts
  again; the registration stays.
- From the checkout while the app's service is registered, `ferb setup --update` refuses and says why. Uninstall from
  Settings → Advanced: Login Items no longer lists FERB's service.
- A checkout `ferb setup --update` while the daemon runs: `~/.ferb/venv.new` appears, then the daemon restarts once on
  the new venv; with the network off it fails and the running daemon keeps working (no restart).
- Setup window Done with one permission off: the window comes back at the next launch; with all on it doesn't.
- A fresh FERB.app install: `ferb memory embeddings` reports the model loaded from FERB.app, no `memory.embedding_model
  fetched` row, nothing under ~/.ferb/models.
- Settings → AI → Sign out of Claude signs FERB's Claude out; a failed Claude sign-in says why without a `ferb` command.
- Footprint: after a day on the rebuilt code, read `resource_samples` again (idle and per-task growth); if it still
  grows, a tracemalloc snapshot through a developer RPC (owner's go) names the holder. (Step 12d: the autorelease-pool
  fix above is what this read checks first.)
