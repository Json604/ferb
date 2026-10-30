# Hands (Mac, browser, shell, files): decisions and fix list

Area: `ferb/act/mac/*`, `ferb/act/browser/*`, `ferb/native_host`, `extensions/chromium/*`, `ferb/act/shell/*`,
`ferb/act/fs/*`, `ferb/act/{aftercheck,point,gui_lock,locks,listen,recording,ask,catalog}.py`, `ferb/act/evidence`.
Basis: a read of every line (2026-10-30), scratch tests on fake files and generated sandbox profiles, read-only DB
counts. File:line refs are to commit c6621c3 / 68dc95b (same files).

## Owner decisions (2026-10-30)
1. **One read question per protected folder per task** (escape hatch and file tools): "Let FERB read your Chrome
   profile folder for this task?" covers every read of that folder in the task; writes and admin still ask per
   command. (Live: 11 questions in one visa task, 6 in another, for the same folder; the old per-command shell rule
   asked 46 and 19 times on 10-13/14 and is already gone.)
2. **Minimal shortcut guard**: in-app shortcuts unchanged; `cmd+q`/`cmd+h` go through FERB's safe quit/hide, which only
   acts on apps FERB launched and checks unsaved work (launched.py:113-145); screen-wide combos (`cmd+tab`,
   `cmd+space`, `ctrl+arrows`, force quit) refused.
3. **Tab groups**: one collapsed, labelled group per task holding only tabs FERB opened. FERB never closes a tab it
   didn't open; a borrowed owner tab is never moved into the group or closed and is left where the work finished; a
   FERB tab holding the result is handed over (moved out, left open); others close with the group at task end; a tab
   dragged out becomes the owner's; an owner tab dragged in stays the owner's; closing a FERB tab or the group
   mid-task counts as the owner stopping that part (FERB asks whether to continue); "keep the tabs open" ungroups and
   leaves them; pause keeps the group; loops keep it collapsed; after a reconnect or restart FERB re-adopts its group
   or closes it if the task ended (no orphans); one group per task; a profile with no window uses FERB's minimized
   window. Needs the `tabGroups` permission and a protocol bump (manifest, JS/Python protocol, minimum versions
   together).
4. **Shell changes: scope + snapshots (A + B)** (owner, 2026-10-30; refines notes/gateway.md decision 3).
   - **A.** Terminal commands can't change anything (write, truncate, delete, rename) in the owner's visible folders
     unless the folder is named — in the owner's words, or in the plan line FERB stated and the owner didn't correct.
     Visible folders: non-hidden folders in the home folder, iCloud Drive (`~/Library/Mobile Documents`), and
     `/Volumes`. Programs keep working: hidden folders (`.git`, `.npm`, `.codex`, `.claude`, `.venv`…), `~/Library`
     (minus the protection lists), temp folders and FERB's workspace stay writable. Kernel-enforced by the sandbox,
     after shell expansion. The owner's files change through file tools (backup copies). A refused command tells the
     worker why; it uses file tools or asks which folder.
   - **How "named" works (typos included):** code never fuzzy-matches folder names. The planner resolves what the
     owner meant ("Downlods" → `~/Downloads`) into a structured `folders` field on the task, shown in the plan line
     ("I'll work in ~/Downloads"); code allows exactly those resolved paths. An ambiguous name → one which-question.
     Work starts at once (rule 9); a wrong guess the owner doesn't correct is covered by B and by file-tool backups.
   - **Hidden folders inside visible ones are writable** (owner, 2026-10-30): any path with a component starting
     with "." (e.g. `~/Desktop/repo/.git`) counts as a program folder, so git works in any repo; B covers mistakes.
   - **B.** A fresh APFS local snapshot before each terminal command, skipped only if no command ran since the last
     one (today at most one per 15 minutes, recovery.py:41). FERB says so when a snapshot couldn't be taken (e.g. a
     nearly full disk); snapshots last about 24 hours and don't cover external drives.
   - Same pattern as Codex and Claude Code (shell writes scoped to the project) and the incident write-ups (scoped
     access + snapshots), widened for a whole-Mac assistant.

(Decisions from notes/gateway.md apply here: tool-reported facts, card fields secret, P19 stable identity,
file tools bound by the protection lists, shell scope per decision 4 below, browser lists
from installed apps, terminal detection fails closed)*

## What exists today (verified)

### Reliability (live tool calls, 2026-09-29 → 10-30)
| Tools | Calls | Errors | Main causes |
|---|---|---|---|
| browser.* / ferb_browser.* | 5,357 | 6.1% (5.2% since 10-24) | profile not matched/connected 110, stale refs 45, reads marked "outcome unknown" 42 of 56, old "no FERB window" 33 |
| mac.click | 121 | 43% | another window over the point, window moved, owner active, stale signature |
| mac.screenshot | 269 | 26% | ScreenCaptureKit "Failed to start stream" 38 |
| mac.act | 943 | 146 + 58 not-ok | focus elsewhere 29, AXValue not confirmed 17 |
| mac.pointer | 49 | 16 | window moved, app changed |
| shell.run | 1,362 | 4 (1 since 10-20) | — |
| fs.* | ~460 | ~30 | missing paths, non-UTF-8 |

### Browser
- Path: daemon ↔ native host ↔ service worker ↔ content script (injected on demand, all frames). Extension→daemon
  HMAC + seq; daemon→extension unsigned; the extension adopts any `paired` message (background.js:201-213); the native
  host's origin is self-declared (auth.py:132-137). Safe only under "same-uid processes are trusted".
- Reconnect orphans task tabs: refs older than `bridge.gen` refused (ext_bridge.py:1807-1808), FERB tabs can't be
  closed, armed tabs disarmed but left open after a daemon restart; orphaned tabs block the extension's self-reload.
- Queued outbox messages are flushed before the new hello and dropped by the daemon (background.js:102,186;
  ext_bridge.py:428-430).
- **press into password/code fields is not blocked** (only `type`: ext_bridge.py:1162, background.js:1129).
- **Read failures become "outcome unknown"** (background.js:309-310 → ext_bridge.py:161-163): 42 of 56 unknowns were
  reads, which then trip P19.
- `list_tabs` and the pending-dialog snapshot return unmasked URLs (background.js:1458, 1464).
- Card fields not secret: `isSecret` = sealed, password, code (content.js:181-189); cc-number/cc-exp/CVV values appear
  in snapshots.
- Window choice auto-picks the first unfocused window and isn't remembered (ext_bridge.py:833-836).
- Typing into an input inside a closed shadow root always fails (`deepActive`, content.js:764-780); pages can detect
  FERB (`ferb-armed` event, `data-ferb-*` attributes).
- Structural facts available: `el.form`, `autocomplete`, `input_type` already read in content.js; `backendDOMNodeId`
  held in ax.js and discarded (usable for stable P19 identity). **No structured source for checkout or bank-page
  amounts**: page text only, bank 3DS pages are usually cross-origin frames (content.js runs in all frames, so often
  readable). The payment decision's amount check is therefore best effort, with the "couldn't read the amount"
  fallback.
- Site-specific code is small: Gmail Atom feed read source, xterm.js detection, the Zomato split-code heuristic
  (generic logic), Google's show-password field, Cloudflare closed shadow roots.

### Mac
- **Pointer input refused in apps with more than one window**: the shot's signature comes from the explicit window's
  walk (driver.py:660-661, 700), `pointer_at` compares a no-window walk (564-566, 908) and `app_window()` returns None
  for 2+ windows (accessibility.py:176-179). Same root breaks `observed_change` and `ensure_window` (waits 10 s).
- **Browser list of 3** (Chrome, Brave, Safari; ax_browser.py:5): Mac snapshots/screenshots of Arc, Edge, Firefox,
  Vivaldi, Chrome Beta are labelled the owner's own app data, not outside web content (tools.py:61-67) — outside
  content treated as trusted. Four browser lists in the code disagree.
- **Key presses into secure fields not checked** (only type/set_value/edit check `n.secret`, driver.py:1540).
- **Terminal detection fails open** when the focused element can't be read (driver.py:1040-1046; policy/terminal.py:97-109).
- **cmd+q / cmd+h / cmd+tab / cmd+space guards exist only in tests** (input.py:248-287); "cmd+tab" was delivered 6
  times on 09-30/10-01.
- Foreground press doesn't raise the window and doesn't check what's under the point (driver.py:1550-1556, 1024).
- `ax_keyboard`: 8-char chunks drop silently on any non-US character ("hello wörld" types "rld"); US keycodes only;
  `fn` dropped; results always True.
- mechanism ignored for select and focus; `ferb watch` step `scroll_window_targeted` crashes (device/watch.py:357-381).
- No AX messaging timeout; many synchronous AX/NSWorkspace calls on the event loop (one hung app stalls the daemon);
  PID typing holds the global GUI lock for the whole text (3,097 chars ≥ 62 s).
- Dead: the marks path, `has_secure_field`, `settable`, `Profiles.prefers/untried`, test-only input guards.

### Shell
- Profile (`sandbox.py:152-253`): deny-list over `(allow default)`; FERB state, FERB.app, Trash, guarded folders
  always denied; Keychain service, LaunchServices, Apple Events denied; network only via the proxy.
- **Process groups survive task end**: `kill()` returns once the shell's own `exit_code` is set (pty_proc.py:215) and
  `end_task`/`kill_all` skip finished processes (manager.py:270, 279), so `cmd &` children live on.
- **Escape-hatch gaps**: `shell.send` has no grant check (an approved `bash` keeps the lift for anything typed later);
  a write lift's allow comes after never-read write denies (lifting `write:/Library` reopens `/Library/Keychains`);
  FERB's own launchd plist is protected only by a liftable rule (rule 10).
- Localhost ports that start listening after a command starts bypass the proxy; no signal rules (a command can kill
  the daemon/proxy); suspected: contained processes inherit FERB.app's Accessibility/Screen Recording grants (a
  `python -c` could post keyboard events into a terminal without the terminal-typing ask) — needs measuring.
- Host records live in memory until forget/close; a crash loses in-flight records. Shell logs never purged (142 MB).
- `ShellManager._procs` grows forever; a 3,001-byte line sent in canonical mode is dropped while reporting
  `sent:true`; `shell.kill` reports kind OPEN_TAB (counted reversible).
- sandbox-exec is deprecated; FERB fails closed if it stops working; semantics could change silently.

### Files
- fd-anchoring, RENAME_EXCL trash moves with dev/inode checks, APFS S1 copies, never-overwriting restore: sound.
- `fs.edit` CRLF → LF; `atomic_text` writes 0600 and loses mode, xattrs, hard links; non-UTF-8 and permission errors
  escape as exceptions (marked "outcome unknown"); `fs.trash` on a symlink trashes the target; `_restore_copy` ignores
  the protection lists; `fs.write overwrite` onto a folder raises; fs I/O runs on the event loop.

### Decision (shell deletes) — implementation notes
- Seatbelt `file-write-unlink`; rename semantics need measuring (likely unlink on source + create on target).
- Superseded by decision 4 (visible-folder scope + snapshots): hidden folders, `~/Library` (minus protection lists),
  temp and the workspace stay writable, so tool caches and coding agents' homes work without a list.
- Would break: git in repos not named in the task (index.lock renames), `sed -i`/`perl -pi` there, `git config
  --global`, global installs under `~` (pipx, uv tool, cargo install, nvm, pyenv), Codex CLI and Claude Code run in
  FERB's shell (write `~/.codex`, `~/.claude` by rename). Overwriting with `> f` is not unlink.

## Depends on
- notes/orchestration.md: the structured `folders` task field (decision 4), pause and wait-for-owner (tab groups:
  closed tab → ask; pause keeps the group).
- notes/gateway.md consumes the facts produced here.
- notes/storage.md decision 5: extension minimum version in the upgrade path.

## Measurements (step 0, 2026-10-30)
- **Sandbox semantics, measured with `sandbox-exec` on scratch folders:**
  - `deny file-write-unlink` blocks unlink, rmdir, renaming a file out of or within the folder, and renaming onto an
    existing file; it still allows creating files, moving a new file in, truncating and appending.
  - So decision 4 ("no changes in visible folders") needs `deny file-write*`, which blocked all ten operations.
  - Exception for hidden folders works with a later `allow file-write*` regex on any path component starting with "."
    (last match wins): `.git/index.lock` creation and its rename onto `.git/index` were allowed.
- **Shell commands and FERB's permissions:** commands are spawned with `subprocess.Popen` and no responsibility
  disclaim (pty_proc.py:64-66), so they run with the grants of the app that started them. Measured from a granted
  parent: a child under a FERB-like sandbox **could post input events** (a 1 px cursor move, restored) and read the
  screen. Adding `(deny mach-lookup (global-name "com.apple.windowserver.active") (global-name
  "com.apple.windowserver"))` blocked both (no cursor move; screen capture returned nothing) while git, python and
  ordinary commands kept working. **Decision (technical, keeps Full Disk Access for the shell):** add that rule to the
  shell profile, so no script can type into a terminal or capture the screen around the always-ask rules. (The
  owner's own run of the probe was in Terminal, so it showed Terminal's grants, not FERB's.)

## Fix list
- [ ] One extension protocol bump covering tab groups, card fields secret, and daemon→extension signing/pairing
      (manifest, JS/Python protocol and minimum versions together).
- [ ] Shell: localhost ports opened after a command starts must not bypass the proxy; deny signals to processes
      outside the task's group (a command can't kill the daemon or proxy).
- [ ] Mac: PID typing releases the GUI lock between chunks.
- [ ] Browser: remove page-detectable markers (`ferb-armed` event, `data-ferb-*` attributes) where possible.
- [ ] Investigate the top failure causes: profile not matched/connected (110), ScreenCaptureKit "Failed to start
      stream" (38).
- [ ] One browser list from installed apps (bundle ids) used by Mac sources, policy, profiles and AX re-read.
- [ ] Secret-field rule (password, one-time-code, cc-number/cc-csc/cc-exp) applied to type, press and fill, in the
      browser and Mac; secret values never captured; boxes masked.
- [ ] Structural facts reported: form submit + action host/method, field types, focused element/app that received
      input, secure field, page URL; best-effort page amounts.
- [ ] Stable P19 identity: browser page URL + backendDOMNodeId/path; Mac bundle + window + element path.
- [ ] Browser: connection-independent tab identity (no orphans); outbox flushed after welcome; read errors = failed;
      mask `list_tabs`/dialog URLs; remember the chosen window; closed-shadow `deepActive`; sign daemon→extension and
      accept `paired` only during pairing.
- [ ] Mac: staleness check by window id + bounds + hit element (fixes multi-window); AX messaging timeout and time
      budget; AX calls off the event loop; terminal detection fails closed; key presses check secure fields; one
      foreground path that raises the window and checks the hit target; remove the ax_keyboard route or fix it;
      mechanism honoured or rejected for select/focus; fix `ferb watch`; remove marks and dead helpers.
- [ ] Key-combo guard (decision 2).
- [ ] Tab groups (decision 3), replacing connection-scoped tab ownership.
- [ ] Per-task folder read lift (decision 1), shared by the escape hatch and file tools.
- [ ] Shell: kill the whole process group at task end regardless of exit; prune `_procs`; `shell.send` respects
      escape grants; never-read write denies after any lift; FERB's launchd plist never liftable; correct `shell.kill`
      kind; durable host records; purge shell logs; report partial/dropped sends.
- [x] Measured sandbox rename semantics (see Measurements).
- [ ] Shell profile denies the window server (blocks event posting and screen capture from commands); measured.
- [ ] Shell scope + snapshot (decision 4): sandbox denies writes/unlink/rename in visible folders except named
      ones; snapshot before each command.
- [ ] Files: preserve mode/xattrs/newlines on write and edit; catch decode/OS errors as plain errors; trash acts on the
      link; restore honours protection lists; fs I/O in a thread.
- [ ] One protection check shared by shell and file tools, with the lift rule of gateway decision 2 and decision 1 here.
