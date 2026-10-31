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

5. **Sound is part of background-first** (owner, 2026-10-30): FERB's own tabs are muted by default unless the task
   needs to listen; FERB stopping or changing playback in a tab the owner is using counts as a disturbance fact; a
   general "keep a tab playing while the owner switches away" ability is later.

6. **Bot checks: workers try visible checks with the bot-check ability** (owner, 2026-10-27, recorded 2026-10-31):
   workers use `ferb/abilities/browser_verification` for a visible "verify you are human" check; `browser_handoff` is
   the fallback. 2026-10-31 (visa run cd80f4ee): the worker never read the ability and gave up at the check, while the
   10-30 runs with the same task text read it and got through; nothing made the ability reach the worker. Owner:
   code states the fact and names the ability; no extra prompt wording about what counts as a bypass.

7. **Download limits** (owner, 2026-10-31; replaces the owner's 2026-10-09 values of 512 MB per download and 50 files /
   4 GB per task, recorded until now only in the old plan, notes/docs-cleanup.md item 3):
   - **No per-file size cap.** A download isn't hard to undo, so FERB doesn't ask either.
   - **Per task: 300 files and 32 GB.**
   - **The 10 GB free-disk floor stays.**
   - **Shell downloads count against the same per-task budget and the free-disk floor** (one shared path): curl, brew,
     pip, git clone, huggingface-cli, docker pull. When a shell command would push free space under the floor, or take
     the task over its byte budget, it is stopped and the worker is told plainly why. It is a fact, not a judgment.
   - Unchanged from 2026-10-09: FERB's downloads folder keeps to 20 GB (its oldest files to the Trash past it, until
     decision 8 below), 50 tabs per task, no time-based clearing, a browser download never opens a Save dialog.

8. **The downloads folder's cap asks; Yes deletes for good** (owner, 2026-10-31; replaces the move to the Trash, which
   freed no space): when `~/.ferb/workspace/downloads` goes over its 20 GB cap, FERB tells the owner and asks, showing
   how much would go and how many files (oldest first, enough to get back under the cap). On Yes it deletes those
   files permanently; on No, or with no answer, it does nothing and doesn't ask again until the folder grows past the
   cap by a further meaningful amount (threshold: a technical ruling; simple, no nagging). The question goes through
   the existing owner-question path, follows orchestration decision 21 for how it appears, and isn't tied to a task;
   it is the permanent-deletion confirmation of rule 5. Only files inside that folder are deleted, never through a
   link; each deletion is journaled with counts and bytes only, never file names.

9. **An app that comes forward by itself is let be** (owner, 2026-10-31; live: the Slack uninstall's background
   "File > Find" press made Finder come to the front): when an app brings itself to the front in answer to FERB's
   background action, FERB doesn't put the owner's app back and doesn't ask. The worker is told, as a fact, and the
   trace shows it, so the worker can avoid that route. FERB makes it rare by preferring routes that open no window
   (Spotlight `mdfind` instead of Finder's Find window, reading AX attributes instead of opening windows, AppleScript
   that only reads, a quiet reopen); lessons reach the how-notes and playbooks (memory step 9b) so the same app and
   route isn't repeated. Code never scans AppleScript for "activate" (rule 4): the after-call fact covers it.

10. **"When you're back at your Mac" is the Mac coming back into use** (owner, 2026-10-31; orchestration decision 17
    clarified): lid opened / wake from sleep, screen unlocked, login, or restart (the daemon starting after boot).
    Input idleness doesn't count: watching a film without touching anything is still being at the Mac. Kept: at most
    once per the schedule's own rule, and away for days gives one run. The paused-task mention (orchestration decision
    13) uses the same signal; held banners follow decision 21 (a banner only when the notch can't show).

11. **A sound-change fact never blames FERB for the owner's own action** (owner, 2026-10-31; refines decision 5): a
    change is not recorded when Chrome says the owner muted or unmuted the tab (`mutedInfo.reason == "user"`) or when
    the owner's own keyboard or mouse input was under way at that moment (the input tap's timing). Only changes FERB
    could have caused are recorded: a structural fact, not a judgment.

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

## Rulings (step 5a, technical)
- Late localhost listeners: a command connects directly only to FERB-sandboxed listeners that existed when it
  started; anything later goes through the proxy (closes the bypass; clients that ignore proxy settings can't reach
  a server started in the same command).
- Visible folders include files directly in `~` and tool folders like `~/go`; a task that needs one names it
  (the plan line), and snapshots cover mistakes.
- After a failed snapshot the next attempt waits 60 s (a hanging tmutil can't add its timeout to every command);
  each result still states that no snapshot was taken.
- `shell.send` into a command started with a lift is refused.

## Rulings (step 5b, technical)
- Secure fields: a key press is refused only when it would put text into the focused password field (a character
  key without ⌘/⌃, Space, or a paste ⌘V, by key or by the Paste menu item); Return, Tab, Escape, arrows and other
  shortcuts still go (submitting a login FERB didn't type isn't typing a password).
- Shortcut guard: ⌥⌘H (hide others) hides the owner's apps, so it is refused like the screen-wide combos; ⌘M stays an
  in-app shortcut. The guard also covers a menu item whose key equivalent is ⌘Q/⌘H (mac.menu, a press on the item, a
  click on it). Screen-wide also: ⌃Space, ⌃⌘Q (lock), ⇧⌘Q (log out).
- `ax_keyboard` removed (live: 23 uses, none confirmed a change; nothing needs it); `focus`/`select` take `ax` only.
- P19 identity (Mac): `mac.act` = the snapshot window's window-server id + the element's AX role path, role,
  subrole, label (its sibling index only when unlabelled), never the snapshot id or the value; `mac.click` /
  `mac.pointer` = the shot's window id + the element the capture's walk found under the point; with no element there
  or one larger than 400 pt, the point on a 24 pt grid of the window. Window titles are left out (a chat window's
  title changes after a send, which would let a repeat escape); a collision only asks the owner.
- Staleness (pointer route): window id + bounds, then the element the app's hit test finds at the point against the
  one the capture's walk found there (same, inside it, or same role at the same frame); when either is unknown, the
  captured window's own walk is compared. `ax` and `foreground` clicks keep their own right-before checks (the hit
  element bound to the window; the point reaching the window), not this content gate.
- Responsiveness numbers: AX messaging timeout 2 s (macOS default 6 s), set globally and on each app element; a walk
  stops after 8 s or 60,000 elements, marked `budget`, and then can't tell about password fields (captures fall back
  to text recognition, L13).
- Foreground element input (press, scroll) is refused when FERB can't tell which window holds the element (no
  window, or two at one place with one title); the point is checked for FERB windows before that.
- ScreenCaptureKit: captures are serialized in-process and a capture failing to start (SCStreamErrorDomain -3811)
  is tried once more after 0.3 s (a read). Live data: the 38 failures came singly, between successes on the same
  window seconds apart, with no other FERB capture span overlapping; the cause isn't visible from code.

## Rulings (step 5c, technical)
- Tab groups: one group per task tree per window (the task's next tab opens in the window its group is in, so normally
  one); grey, labelled "FERB · <first words of the request>" (about 30 characters, redacted), collapsed when made and
  never expanded by FERB (a tab added to a collapsed group keeps it collapsed). A popup in a popup-type window can't
  be grouped and stays ungrouped (still FERB's). A tab opened from a borrowed owner tab is released like its opener,
  so it isn't grouped either. When the last tab FERB opened leaves a group, the owner's tabs dragged into it are
  ungrouped (left open) and the group is forgotten. An owner who renames the group makes its tabs theirs at the next
  hello (the title check that guards against reused ids).
- Result tab: `browser.keep_open` (existing) is how a worker marks the result: the tab leaves the group, open. "Keep
  the tabs open": `_TabDriver.keep_tabs_open(task)` (step 8c wired the owner's words: the planner's `keep_tabs_open`
  on the task, or in a correction's task, reaches both drivers through BrowserPort.keep_tabs_open); pause: the daemon-supplied
  `task_paused` (default False; step 8) makes `release` close nothing.
- Owner's stop: `browser.owner_stopped` {task_id, tab, profile, borrowed, part: "tab" | "group"} when a tab FERB holds is
  removed without FERB closing it (part "group" when no other FERB-opened tab of that tree is left in the profile);
  `browser.owner_took` when the owner moved a FERB tab out of its group (the extension lets go of it and sends
  `taken`). Both critical bus topics; step 8 turns them into "ask whether to continue".
- Tab identity: the extension keeps its records in chrome.storage.local (survive worker restarts and extension reloads;
  cleared on `runtime.onStartup`, a new browser session, whose reused ids mean nothing). Each hello lists the records
  still valid (tab open, still in its titled group); the daemon re-adopts its tabs (new generation, element ids stale),
  drops ones not listed, adopts listed tabs of a running task it doesn't know (after its own restart), and closes
  (FERB-opened) or releases (borrowed) tabs of ended tasks. The extension's `close` closes only a tab its records hold
  as FERB-opened. A self-reload waits only for work in flight (a command, a held download, a gesture, an attached
  debugger while a connection could drive it), not for open task tabs.
- Signing: welcome and cmd are {type, seq, body, mac}, HMAC-SHA256(pairing key, "<nonce>|<seq>|<type>|<body>") with the
  hello's fresh nonce and a per-connection counter; an assembled command's parts are each verified and the envelope's
  MAC again. Unsigned replies (refused, pairing_required) change no page. A command that fails verification is dropped
  without a reply (echoing its unverified id could fail a real command early), so the daemon times out: unknown, the
  safe side.
- Native host: the daemon takes a connection only when the native host's parent process runs a browser app FERB knows
  (`peer_refusal`, the .app's CFBundleIdentifier through libproc), and a paired profile's hello only from that
  browser's own app. Same-uid limit: any process of the owner's user can start the browser binary with its own
  profile, or read a profile's pairing key from its extension storage; same-uid processes stay trusted.
- Reads: list_tabs, snapshot, wait, screenshot(_part), record_frame, frame_origin, read_source, window_secrets, find,
  result_part, ping, point, and dialog without a decision: an error, a timeout or a dropped connection is a known
  failure (DriverError), never OutcomeUnknown.
- Outbox: messages wait unsigned until the welcome and are signed when posted (at most 5000 kept while refused; the
  oldest go).
- P19 identity: backend node id first (stable while the node lives, so a re-snapshot of the same page matches), DOM path
  when the accessibility pass gave none (screenshot points, a tab without the debugger). A re-render that replaces the
  node escapes the identity; a shifted DOM path can collide (only asks the owner).

## Rulings (regression 2026-10-31, macOS 27.2, technical)
- **Regression:** after the update to macOS 27.2 (Darwin 27.2.0) FERB refused every local port ("FERB couldn't
  inspect the local TCP sockets"); 6 shell/proxy tests failed on clean 41ef7dff. Cause, measured: the kernel's
  socket lists (`net.inet.{tcp,udp}.pcblist[_n]`) now hold only the caller's own sockets for an ad-hoc-signed Python
  (FERB's ~/.ferb/venv, the checkout .venv, Homebrew's) and for any program it starts, so `netstat -anv` exits 0 and
  prints nothing. The same netstat from a terminal, or exec'd in place of that Python, lists everything; an
  ad-hoc-signed copy of netstat lists nothing. Apple's rule isn't documented; it behaves like Local Network privacy's
  attribution of a tool to the third-party program that ran it.
- **Source now (ferb/act/shell/sockets.py):** libproc in FERB's own process, no subprocess (`proc_listpids`,
  `proc_pidinfo PROC_PIDLISTFDS`, `proc_pidfdinfo PROC_PIDFDSOCKETINFO`, SDK `struct socket_fdinfo`, 792 bytes;
  an answer of any other size is no reading). It sees every process of the owner's user and none of another
  user's or root's. The proxy route (`LocalAccess`) uses only it, about 2 ms per reading: the exact connected
  server socket must belong to an inspectable process, so a root or other-user server is refused.
- **Every user's listeners for the profile route:** the shell profile's direct ports (a contained server's port,
  a pinned port) need every listener on that port, or another user's listener on the same port number (other
  address family) would be reachable around the proxy. `sockets.complete()` adds what `/usr/bin/nettop -L 1 -n -x
  -J state` lists for processes libproc can't inspect, each unnamed (pid None: outside, never pinned). nettop is
  Apple's, entitled, and still lists every user's sockets when FERB starts it. It counts only when it lists every
  listener libproc found (a nettop that sees less, the way netstat does now, is no reading: nothing direct, only the
  proxy) and only when every line parses (a program name with a line break makes it unreadable, never misread).
  About 9 ms per profile.
- **Accept wait:** a connection the server hasn't accepted yet is in no descriptor table, only counted in its
  listener's queue (libproc, measured). `LocalAccess.connected` re-reads while a visible listener on that port has a
  queued connection, at most `ACCEPT_WAIT` (30 s, the proxy's own connect timeout), then refuses; with nothing queued
  on a visible listener it refuses at once. netstat had named a queued connection after its listener.
- Sockets shared by several processes are listed once per process, and each must pass (stricter than netstat's one
  last-user pid).
- **FERBNotch:** macOS 27.2 never calls `updateTrackingAreas` in a test window (measured: plain NSView too, on or
  off screen, NSApp running or not, after add, move, resize, scroll), so `LatestSourceHoverTests.testFullNotchSource…`
  found no tracking area. `PreviewHoverTrackingView` now adds or removes its inVisibleRect area itself when it joins a
  window, changes frame, or its clip view scrolls. The pill's hover in the app already ran through the panel's
  mouse-moved monitors and `PreviewHoverRouter`; whether the live app gets AppKit's call wasn't measurable without
  moving the owner's pointer (other overrides: PanelCoordinator, FerbIndicator, PanelResizeOverlay).

## Rulings (step 12d, technical)
- The shell's egress proxy listened with socketserver's default backlog of 5. macOS answers a connect past a full
  backlog with a reset, so whenever the accept loop fell behind (the process busy, a burst of connections) a client got
  `ConnectionResetError` before the proxy saw it: the cause of the flaky
  `test_many_unauthenticated_connections_neither_starve_a_client_nor_the_daemon` (reset at the flood's connect, ~1 run
  in 3 under full-suite load), and a real weakness: a package manager's parallel downloads, or a flood, could reset a
  legitimate client the same way. Measured with one busy thread in the process, 8 runs of the test each: backlog 5,
  8 of 8 reset; backlog 128, none (25 of 25 alone as well). Ruling: the proxy, and the Claude runtime's tool-list relay
  (ferb/runtimes/proxy.py, same default), listen with `socket.SOMAXCONN` (the kernel caps it at kern.ipc.somaxconn,
  128 here). The test is unchanged.

## Rulings (step 14b, technical: decision 5, sound)
- Muting is the browser's tab mute (`chrome.tabs.update({muted})`), never the page's media: the page is untouched and
  can't tell. A tab FERB opens (`open`, with or without a task) is muted before anything loads; a popup a FERB tab opens
  (`adopt_tab`, not borrowed) too. The owner's tabs (borrowed, handed over, or never FERB's) are never muted or
  unmuted: the extension's `mute_tab` refuses them (`not_ferbs`) as `close` does, and the daemon never sends it for an
  attached tab.
- "Needs to listen": `audio.listen` on a tab, or `browser.record with_audio`, unmutes that FERB tab before capture and
  it stays unmuted for the rest of the task (voice decision 5 unchanged: hearing is still the whole browser app,
  unmuted). The worker's own `browser_media unmute` in a FERB tab unmutes the tab too ("play this for me": the owner
  wants to hear it), and `mute` mutes it again; the tool description says FERB's tabs are muted until then. Results
  carry `tab_muted` (and `tab_sound_error` when the browser refused), never failing the call.
- A tab that becomes the owner's gets its sound back, only when FERB's extension muted it (`mutedInfo.reason` extension
  with FERB's id): `release_tab` keep_open (the result tab, "keep the tabs open") and a tab moved out of its group
  (`taken`). Closed tabs need nothing.
- Disturbance fact: the extension sends a `sound` tab event when a tab's `audible` or mute changes (browser facts, tab
  id, whether it is FERB's; no URL or title). For a tab that isn't FERB's, the disturbance sampler records a
  `tab_sound` change for each active task with an acting call (any capability not marked `observation`) in flight or
  ended within SOUND_NEAR_S = 5 s (Chrome drops `audible` about 2 s after sound stops). No FERB call near: the owner's
  own play/pause, nothing kept. Attribution stays "temporal correlation only". The merged checker gets them in
  `how_facts` as `sound_changes_in_other_tabs_near_ferb_calls`; the trace lists them with the near calls. Sensing
  paused: dropped like every owner-tab event.
- Not built (decision 5 says later): keeping a tab playing while the owner switches away.

## Rulings (step 12e, technical: decision 7, download limits, and the research's bugs)
- **One budget per task tree, browser and shell together.** `Downloads` (ferb/act/browser/downloads.py) owns it: the
  bytes of files the tree's tabs took (`downloads` rows) + transfers still arriving + what its shell commands received
  through the egress proxy (`shell_downloads`, migration 0059, one row per tree, aged with the operational records,
  so a daemon restart mid-task doesn't start the tree afresh). Only received bytes (`bytes_in`) count; what a command
  sends doesn't. Browser admission and streaming see the shell's bytes, and the shell sees the browser's.
- **The 300-file count is the browser's only.** A shell command has no file count: a `brew install` or `pip install`
  fetches many parts, and the proxy sees connections, not files. The byte budget and the floor cover the shell.
- **How a shell download is stopped (the floor during long commands):** while any FERB command runs, every
  `WATCH_S` = 1 s (ferb/act/shell/manager.py) the bytes each command received since the last look go to the daemon
  (`_shell_received`): counted, then `Downloads.shell_stop` answers why it must stop: past the tree's budget, or free
  disk under the floor (or free space unreadable, as for the browser). Then that command's whole group is stopped, its
  results carry `stopped_by_ferb` (FERB's words; `is_error`), and the journal gets `shell.download_stopped`. Only a
  command that received bytes in that second is ever stopped: a command that isn't downloading never is, even under
  the floor (unzipping or building isn't a download; decision 7 covers downloads). At a 1 GB/s link a command can
  pass either limit by about one second of traffic. A process's last bytes (after the final look) still count, from
  its final proxy record.
- **No total time for a browser download, only a stall:** a transfer fails when no byte arrives for `STALL_S` = 60 s
  (a part that doesn't answer within it, or parts that keep coming back empty), reason "stalled"; a slow download that
  keeps moving is never cut. The old 10-minute ceiling (with 72,000-byte parts) capped a download at a few GB.
- **Download parts of 512 KiB, sent whole** (protocol.js DOWNLOAD_PART, was 72,000 bytes): about 7× fewer round
  trips. The reply (~700,000 base64 characters) is never split into 100,000-character result parts. Direction checked:
  Chrome's 1 MB native-messaging cap is for messages from the host to Chrome; from the extension to the host Chrome
  allows 64 MiB, and download data only travels that way (extension → native host → daemon). FERB's own host
  (`MAX_MESSAGE`) and the daemon's socket take 1 MiB per message, so 512 KiB keeps a margin.
- **The tab cap counts every open FERB tab.** `browser.task_max_tabs` (50) now counts the tabs FERB opened for the task
  tree that are still open, in every browser driver: the worker's, a link's, a page's popup, and new tabs opened from
  the owner's borrowed tab, plus opens in flight; never the owner's borrowed tab itself, a tab handed over (kept
  open) or a closed one. A close frees a place. Past the cap FERB refuses with a fact: the worker's open is refused, a
  link isn't opened (no `open_plain` hand-off to the browser, whose download would skip every limit and land in
  ~/Downloads), and a page's popup is closed while still blank (`popup_refused`, protocol 20); the opener's next
  result says so.
- **Nothing dropped silently.** A page's saves past the hold caps (20 per tab, 100 in all) are still dropped (never the
  browser's), but every one is reported: the first of a document at once, then those that follow within the minute in
  one event at its end with `count` (content.js and background.js both), so a flood is a few facts, not one per
  save. Still unreported, because nobody can be told: a tab no task drives, or no daemon connected.
- **Decimal GB everywhere** (as the owner and Finder count): `browser.download_task_max_gb` (32),
  `browser.download_total_gb` (20), `browser.download_min_free_gb` (10, unchanged). The binary MB keys are gone: a
  one-time journaled config upgrade (`download-limits-2026-10-31`, ferb/config.py) removes `download_max_mb`, moves
  `download_task_max_files` 50 → 300, and drops `download_task_max_mb` / `download_total_mb`; an owner's own MB value
  (anything but 2026-10-09's) carries over to its GB key, converted. An old config that still sets a removed key is
  never read.
- **The copy to ~/Downloads becomes a move.** `browser_downloads` tells the worker to move the file (mv, not cp)
  where the owner asked, else ~/Downloads, and name that path: same destination as before, no second copy on disk.
  FERB's working copy is then gone from its folder (its record reads "already gone" after the next retention pass).
  The shell's scope rule (decision 4) still applies to the destination folder.
- **Known limitation:** past 20 GB the downloads folder moves its oldest files to the Trash, which frees no space until
  the Trash is emptied. Left as it is: permanent deletion is the owner's call.
- Extension protocol 20, extension and minimum 0.18.0 (one bump): download parts, `popup_refused`, drop counts.

## Rulings (step 12e part 2, technical: Mac background/focus, decisions 8-11)
- **Foreground right after the owner's Yes (live 2026-10-31, clean-up task: Terminal never came forward and every
  foreground action after the Yes was refused).** `mac.act` and `mac.click` refused a foreground route whenever one of
  FERB's own apps was in front (`_own_front`), before activating the target: right after a Yes in the notch FERB *is*
  in front. That pre-activation refusal is gone (and `_own_front` with it). What keeps input out of FERB's own app is
  unchanged: `_real` waits for the owner to be quiet, activates the target, and posts each event only while the target
  is the front app (`PointerInterrupted` otherwise); `_own_point` refuses a real click/scroll wherever a FERB window
  would take it, before bringing anything forward and again right before each event; every Mac tool still refuses to
  target FERB's apps (`app()`).
- **One foreground question per app, not per spelling** (live: asked about "Terminal", then "pid:9040").
  `confirm.foreground_need` keys its Need by `confirm.app_key`: the running app's bundle id (casefolded) when the name,
  `pid:N` or bundle id names exactly one running app (`apps.find`, the drivers' own lookup), else the argument without
  case or invisible marks. The question still shows the app as the worker wrote it. A Yes therefore covers every
  spelling and every retry in the task (rule 5).
- **Self-activation reaches the worker (decision 9).** The disturbance sampler already recorded what changed during an
  on-screen call (span output `screen`, the checker's `how_facts` `screen_changes_during_ferb_calls`); the worker
  never saw it. Its tool result now carries `screen_changed_during_call` (`disturbance.told`: `front_app` {before,
  after} by name, else `focused_window_changed`; `fullscreen`; `space_changed`; `put_back: false`), on success and on
  failure. Nothing is restored or asked. Which calls are sampled is unchanged (capabilities with `on_screen`: Mac
  input, menu, open, AppleScript, shell commands, the browser tools that can raise a window); `mac.applescript` is
  sampled, so a script whose app activates itself is told the same way, with no scan of the script (rule 4).
- **Background launch without hiding (fix 4).** `apps.launch` uses `open -g -b` (was `open -g -j -b`). Reasoned, not
  measured on the owner's Mac (no app was launched): `-j` launches hidden, and a hidden app's windows are ordered out,
  so ScreenCaptureKit's window capture and pointer events posted to the window can't reach them (Accessibility reads
  either way); `-g` alone doesn't activate the app. Not added: `NSRunningApplication.unhide()` for an app the owner
  hid (it changes the owner's arrangement; its activation behaviour is unmeasured).
- **Windowless running app (fix 4).** `mac.open reopen=true` sends the core `reopen` Apple Event (kAEReopenApplication,
  what a Dock click sends) to the running app's pid without `kAECanSwitchLayer`, never interacting, no reply awaited
  (`apps.reopen`), only when the app shows no window; then waits for one as before. Its "no window" result names this
  and the app's New Window menu item (mac_menu, an AX press). An app that comes forward anyway is decision 9's fact.
- **Open a file or address in an app, in the background (fix 5).** `mac.open` takes `path` (absolute or `~/…`) or
  `url` (http/https only: other schemes start programs or send mail; argument validity) and opens it with
  `NSWorkspace.openURLs(_:withApplicationAt:configuration:)`, `activates = false` (`apps.open_in`, waits up to 15 s
  for macOS's confirmation). Through the gateway like any call: the file is the capability's `file_paths` (read), so
  the protection lists ask or refuse as for the file tools; a `url` is described as an `OPEN_TAB` to its site, so the
  address checks (no-site refusal, S4's private-data sink, known destinations) apply as for `browser.open`. The
  terminal rule (D31): when the app is a terminal (a script opened in Terminal runs) or opens files with their default
  apps (Finder, System Events) and that default is a terminal app (a `.command`), `terminal_input` returns the file
  with its exact contents, the gateway asks, and right before opening the driver re-hashes it (`open_refusal`): a
  changed file isn't covered. A `path` Launch Services counts an application (`NSURLIsApplicationKey`: an .app, an
  applet) is refused (a where-boundary): opening it would start a program the shell wrote outside the shell's sandbox;
  installed apps open by `bundle_id`. The shell's `lsopen` denial (sandbox.py) is untouched: `mac.open` runs in the
  daemon, not in a sandboxed command, and mac.applescript's profile still denies launches and document opens.
- **Window-free routes in the tool description (fix 6).** One sentence in `mac.open`'s description: finding and
  reading need no window (mdfind through the shell, mac_snapshot's Accessibility reads). Recorded in notes/prompts.md.
- **`_bring_forward` and the notch click (fix 7).** The input tap counts every unmarked event as the owner's,
  including a click in the notch. But `_real` first waits for the owner to be quiet (`RealInput.quiet`, 1.5 s since
  their last input) and only then activates; `_bring_forward` gives up only on input after that, so the Yes click
  itself can't make it give up; a mouse move right after it can. Left as it is (moving the mouse is real owner
  activity that a foreground click would fight); measured in step 13.
- **Presence signal (decision 10, `ferb/sense/watchers/presence.py`).** `MacPresence` replaces the input tap's
  idle/active as the "back at your Mac" signal: out of use while the Mac is asleep, its displays are asleep, its screen
  is locked or its login session is switched away (NSWorkspace will-sleep/did-wake, screens-did-sleep/wake,
  session resign/become active; distributed `com.apple.screenIsLocked`/`screenIsUnlocked`; one thread of FERB's with
  its own run loop, measured: the old workspace watcher received 106 `NSWorkspaceDidWakeNotification`s on that kind of
  thread, 10-11 → 10-31). Every 5 s the same thread also reads the session's lock and console state
  (`CGSessionCopyCurrentDictionary`) and the main display's sleep (`CGDisplayIsAsleep`), so a missed notification
  costs seconds. Back = the moment none of those holds after one did; journaled `mac.back_in_use` {signal}. A daemon
  start in a new login session (the session's unique id, kept as a digest in `daemon.started`) is a return ("login":
  restart or login), unless the Mac is locked then (its unlock is the return); a start in the same session (an update,
  a crash) isn't, and the current stretch begins at the last `mac.back_in_use`. Started and stopped with sensing (Pause
  FERB: unknown, counted as present, as before). Timers, `Schedules._book`, "at most once per its rule", "away for days
  = one run" and the paused-task mention are unchanged; only the signal moved. `OwnerInput` keeps only input timing
  (yielding, the trail's idle/active records); `[sense] idle_after_s` now only drives those records.
- **Held banners (decision 21).** `_notify` holds a non-urgent banner while the Mac is out of use (displays asleep,
  locked, asleep, switched away), not after input quiet time, and delivers it when the Mac is back. A film watched
  without input gets its banners at once.
- **Sound facts (decision 11).** The extension's `sound` event carries `muted_reason` (Chrome's `mutedInfo.reason`;
  protocol 21, extension and minimum 0.19.0, one bump). The daemon drops the `muted` part of a change whose reason is
  "user". The sampler records nothing when the owner's input is under way or ended within OWNER_NEAR_S = 3 s
  (Chrome's audible flag trails a pause by about 2 s), from the tap's last unmarked event on the daemon's clock;
  without the tap, input can't be told and only the reason applies.
- **Downloads cap (decision 8).** `Downloads.over_cap` (the oldest files whose deletion brings the folder back under
  `browser.download_total_gb`, by recorded size) after each taken file and in the hourly retention pass; nothing moves
  to the Trash any more. The question is one of FERB's own task-less questions (the memory forget's path:
  `_ask_owner`, kind `downloads:over_cap`, capability `downloads.delete`, Yes/No only, kept in `memory_questions` so a
  restart asks it again), shown as decision 21 says (the notch opens; a banner when it can't). **Re-ask threshold
  (ruling):** after a No, or while the question waits, it is asked again only once the folder is a further 10% of the
  cap (2 GB of 20 GB) past the size it was asked at; the newer question replaces the waiting one. After a Yes the next
  pass over the cap asks afresh. Yes deletes exactly the files the question counted that are still there, through the
  open folder (`O_NOFOLLOW`), regular files only, never a link (a link or a missing file is recorded "gone"); rows stay
  with `kept` deleted. Journal: `downloads.cap_asked` {files, bytes, total_bytes, cap_bytes}, `downloads.deleted`
  {files, bytes}, `downloads.cap_declined`: no names.
- **A file that can run when opened (12e review, finding 1).** The rule: opening a file must not start code the shell
  wrote outside the shell's sandbox without the owner's Yes on its exact contents. Before, only a terminal app, or
  Finder/System Events handing the file to one, counted; but some default openers run the file themselves (on this
  Mac a `.jar` opens in Java Launcher and runs, a `.py` can open in Python Launcher, a `.pkg` in Installer, a
  `.mobileconfig` in ProfileHelper, a `.shortcut` in Shortcuts), so those opened unasked. **Ruling:** the file's Launch
  Services type decides, a structural fact about the file, not an app list (`apps.runs_when_opened`): it conforms to
  `public.executable`, `public.script`, an installer package (`com.apple.installer-package-archive`, `-package`,
  `-meta-package`), a configuration profile (`com.apple.mobileconfig`, `com.apple.configprofile`), or a shortcut or
  workflow (`com.apple.shortcut`, `com.apple.shortcuts.workflow-file`, `com.apple.automator-workflow`), or the path is
  a package directory (`isFilePackageAtPath`). An unknown type (a dynamic `dyn.` one: an extension macOS doesn't know)
  or an unreadable one counts as runnable (fail toward asking, like `is_application`). Checked on this Mac: `.jar`
  (com.sun.java-archive), an executable with no extension and `.js` conform to executable; `.py`, `.sh`, `.command`,
  `.scpt` to script; `.pkg`, `.mobileconfig`, `.shortcut` to theirs; `.txt`, `.pdf`, `.html`, `.rtf`, `.zip`, a plain
  folder and a non-executable file with no extension don't. It applies whatever app receives the file (`mac.open`
  `_opens_to_run`), and to every file FERB's input would open by its default app (`_open_files`: a Finder double-click,
  an AX press, ⌘O or File > Open on a selection, a file a Finder script names). Such an open goes through the existing
  terminal question (D31) with the file's path, hash and text ("a file that can run when opened"), and the Yes covers
  exactly those bytes, re-hashed right before the open. So opening a script in an editor through `mac.open` asks too
  (FERB can't tell which apps run what they open), and so does a package document like an `.rtfd`. The app-bundle
  refusal stays. tests/test_mac_focus.py `test_a_file_whose_type_runs_when_opened_asks_in_any_app`.

## Step 13 must measure (step 12e part 2)
- Whether `open -g` (no `-j`) puts a launched app's first window behind the owner's front window, on the current
  Space, without a front-app or focus change (TextEdit, Notes, Reminders, Calendar).
- Whether a window screenshot and pointer input work for an app launched with `-g`, and (for comparison, on a
  disposable app) whether they failed for a hidden (`-j`) launch; whether `unhide()` activates.
- The `reopen` Apple Event without `kAECanSwitchLayer`: which apps open a window without coming forward (TextEdit,
  Notes, Finder); whether macOS asks for Automation permission for it.
- `NSWorkspace` open with `activates = false`: a file in TextEdit and an address in Safari stay behind the owner's app.
- Presence: lid close/open, screen lock/unlock, display sleep/wake, fast user switch, restart each journal one
  `mac.back_in_use` with the right signal; whether distributed lock notifications reach FERB's thread or only the
  5-second read catches them.
- `_bring_forward`: a foreground click right after a Yes in the notch, with the owner's hand still on the mouse.
- Sound: the owner pausing their own music by click or space bar while a FERB call runs records nothing.

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
- [x] Sound is part of background-first (decision 5): FERB's own tabs muted unless the task needs to listen; FERB
      stopping or changing playback in a tab the owner is using is a disturbance fact. (Step 14b: protocol 19,
      extension and minimum 0.17.0: `open`/`adopt_tab` `muted`, `mute_tab`, sound back on handover, `sound` tab events;
      `_TabDriver.tab_sound` used by audio.listen, browser.record with_audio and browser.media mute/unmute;
      `DisturbanceSentinel.call_started/call_ended/sound_changed`, `how_facts`
      `sound_changes_in_other_tabs_near_ferb_calls`. See Rulings (step 14b); tests/test_tab_sound.py. The owner reloads
      the extension once in step 13.)
- [ ] macOS 27.2: AppKit stopped calling `updateTrackingAreas` in the test process (2026-10-31). The source pill now
      reconciles its own tracking area; the other overrides (PanelCoordinator.swift:85, FerbIndicator.swift:94,
      PanelResizeOverlay.swift:68) are checked live in step 13 (hover-to-open, resize) and get the same pattern if
      the live app is affected.
- [x] Bot-check fact (decision 6): when a page shows a bot-check widget (a structural fact, e.g. the challenge frame's
      origin, reported by the extension/snapshot like a field's kind), the tool result says so and names the
      `browser_verification` ability. No prompt line about bypasses (owner).
      (Step 9b: `PageSnapshot.frame_urls` from what the extension already reports (each child frame's address, each frame
      element's `src`; `ext_bridge.frame_urls`); `snapshot.bot_check` matches them against `CHALLENGE_FRAMES` and the
      snapshot result starts with `bot_check` {widgets, frames, ability: browser_verification, read: "abilities.read
      name=browser_verification"}. No protocol change, so no extension reload; no prompt line. tests/test_perception_9b.py.)
- [x] One extension protocol bump covering tab groups, card fields secret, and daemon→extension signing/pairing
      (manifest, JS/Python protocol and minimum versions together). (5c: protocol 18, extension and minimum 0.16.0,
      `tabGroups` permission; the `rest`/`show_tab` version gates removed. The owner reloads the extension once.)
- [x] Shell: localhost ports opened after a command starts must not bypass the proxy; deny signals to processes
      outside the task's group (a command can't kill the daemon or proxy). (5a: direct only to ports whose listeners
      were all contained at command start, plus pinned ports; anything later only through the proxy, which now
      checks contained listeners too and gets NO_PROXY empty; `(deny signal)(allow signal (target same-sandbox))`,
      measured: own children/setsid descendants yes, other programs and other FERB commands no.)
- [x] Mac: PID typing releases the GUI lock between chunks. (5b: `RealInput.to_process(release_every=8)`; every
      post runs off the event loop.)
- [x] Browser: remove page-detectable markers (`ferb-armed` event, `data-ferb-*` attributes) where possible.
      (5c: hook event names and the linking/stamp attributes are random per service-worker start (protocol.js
      FERB_TOKEN); the linking attribute leaves the page right after each snapshot. Remains: the page hook's patched
      `window.open`/`click`/`dispatchEvent` are visible as non-native (toString) in a task tab while FERB drives it, the
      popup's blank `about:blank#ferb-…` URL, attributes visible to a MutationObserver during a snapshot, the 1280×900
      viewport and focus emulation in FERB's own window.)
- [x] Investigate the top failure causes: profile not matched/connected (110), ScreenCaptureKit "Failed to start
      stream" (38). (5b: ScreenCaptureKit part done, see Rulings; live check in step 13 whether -3811 survives the
      retry. 5c: read-only span counts: 46 "no profile called 'chrome scaler' / 'chrome:scaler'" (only "chrome/x" and
      quoted forms resolved), 2 'Brave Browser' (a browser's name), 15 "select a profile" (no profile given: the
      worker's choice, left), 23+16 "paired but not connected"/old-protocol refusals (before the 10-26 wake fix and
      old extensions). Fixed: `profiles.resolve` takes "<browser>[ :/“]<name>", the browser's label/app name, and
      compares without invisible marks; `profiles.running` used `pgrep -x` (16-char process names: Chrome Beta/Canary,
      Edge channels never looked running). Drops: daemon restarts and MV3 worker deaths; tabs no longer go stale on
      them (readoption).)
- [x] One browser list from installed apps (bundle ids) used by Mac sources, policy, profiles and AX re-read.
      (5b: `ferb/act/mac/browsers.py`: known bundle ids/vendors/names + Launch Services http/https handlers, cached
      60 s; replaces ax_browser, confirm.BROWSER_APPS, profiles' list and scan.py's table; names compared without
      invisible marks. sense/batches.py's name→bundle map is not a browser test and stays.)
- [x] Secret-field rule (password, one-time-code, cc-number/cc-csc/cc-exp) applied to type, press and fill, in the
      browser and Mac; secret values never captured; boxes masked. (5b: Mac done: key presses that enter text and
      the Paste item refuse into a secure field, `accessibility.secure_element`. 5c: content.js `fieldKind` is the
      one rule (cards by autocomplete, inputs and expiry selects); a text-entering press is refused in content.js,
      background.js (`entersText`, also where focus lands) and ext_bridge (`enters_text`); a login code is never
      sealed into a card field; `field_kind` reported.)
- [x] Structural facts reported: form submit + action host/method, field types, focused element/app that received
      input, secure field, page URL; best-effort page amounts. (5c browser: `_TabDriver.act_facts` before the act
      (ActionSpec.facts) and the page's own submit events after it (ToolResult.facts); new field `frame_host` for an
      input in a cross-origin frame (what "bank page" can mean structurally); amount = the clicked control's label
      amount, else exactly one structured page amount (price metadata, JSON-LD); merchant = JSON-LD seller or
      og:site_name. JS fetch() submissions fire no submit event: only network observation could see them, later.) (5a: the shared shape `ActionFacts` in
      ferb/core/types.py, recorded on the call's span; fs tools report writes/deletes. Browser/Mac reporting: 5b/5c.
      5b: Mac tools report target role/subrole, secure field, received_by (read after input), terminal_target and
      page URL in a browser: `ActionResult.observed` → `ToolResult.facts`; new ActionFacts fields target_role,
      target_subrole, page_url. 6b: the gateway raises questions from them (notes/gateway.md Rulings (step 6b)); new
      field `tab_chain` (the first task tab of the chain, from `_Tab.opener`), for one payment per tab chain.)
- [x] Stable P19 identity: browser page URL + backendDOMNodeId/path; Mac bundle + window + element path. (5b: Mac
      done, capability `ledger_args` read by gateway `hashed_args`, see Rulings. 5c: `BrowserAct.ledger_args` and its
      click content use `element_identity`: page origin+path, child frame's, role, backend node id else DOM path.)
- [x] Browser: connection-independent tab identity (no orphans); outbox flushed after welcome; read errors = failed;
      mask `list_tabs`/dialog URLs; remember the chosen window; closed-shadow `deepActive`; sign daemon→extension and
      accept `paired` only during pairing. (5c: see Rulings (step 5c); plus the native host's starter checked by app
      bundle, `_pair` rolled back on error, pointer.js reads `disabled`, one `maskUrl` (mask.js), one key table.)
- [x] Mac: staleness check by window id + bounds + hit element (fixes multi-window); AX messaging timeout and time
      budget; AX calls off the event loop; terminal detection fails closed; key presses check secure fields; one
      foreground path that raises the window and checks the hit target; remove the ax_keyboard route or fix it;
      mechanism honoured or rejected for select/focus; fix `ferb watch`; remove marks and dead helpers. (5b: see
      Rulings; `observed_change`/`ensure_window`/`window_named` fixed for 2+ windows; `ferb watch`'s step was already
      gone; removed marks, `has_secure_field`, `settable`, `describe`, AX keyboard, `Profiles.prefers/untried/seed`.)
- [x] Key-combo guard (decision 2). (5b: `input.screen_wide`/`quit_or_hide` in `driver._guarded_combo` for every
      route; ⌘Q/⌘H through `LaunchedApps.quit_now/hide_now`, only the task tree's own launched app, quit only with no
      unsaved work.)
- [x] Tab groups (decision 3), replacing connection-scoped tab ownership. (5c: see Rulings (step 5c).)
- [x] Per-task folder read lift (decision 1), shared by the escape hatch and file tools. (5a: one key per rule per
      tree, `confirm.read_lift_key`; 6b: both are Need producers of the gateway's one step, `_lift_needs` and
      `_escape_need`.)
- [x] Shell: kill the whole process group at task end regardless of exit; prune `_procs`; `shell.send` respects
      escape grants; never-read write denies after any lift; FERB's launchd plist never liftable; correct `shell.kill`
      kind; durable host records; purge shell logs; report partial/dropped sends. (5a: after the leader exits the
      group is signalled only if every member is contained and the id isn't another command's; sends into a lifted
      command refused; both FERB LaunchAgents are FERB_SERVICE, never lifted; kind `stop_process`; first connection
      per host journaled `shell.destination`; logs purged after evidence_days; over-long canonical lines refused.)
- [x] Measured sandbox rename semantics (see Measurements).
- [x] Shell profile denies the window server (blocks event posting and screen capture from commands); measured.
      (5a: shell profile only; mac.applescript's unchanged.)
- [x] Shell scope + snapshot (decision 4): sandbox denies writes/unlink/rename in visible folders except named
      ones; snapshot before each command. (5a: `ToolContext.folders` from `Gateway.task_folders`, empty until step 8's
      planner field; non-hidden files directly in ~ count as visible too; a failed snapshot is retried at the next
      command and stated in the result.)
- [x] Files: preserve mode/xattrs/newlines on write and edit; catch decode/OS errors as plain errors; trash acts on the
      link; restore honours protection lists; fs I/O in a thread. (5a: APFS clone then rewrite, else mode+xattrs
      copied; a hard-linked file is rewritten in place; `ferb restore` checks only the never-lifted rules.)
- [x] Download limits (decision 7): no per-file cap; 300 files / 32 GB per task tree; the 10 GB floor; shell downloads
      under the same budget and floor. (12e: see Rulings (step 12e); tests/test_download_limits.py,
      tests/test_downloads.py.)
- [x] Browser downloads end on a stall (60 s without a byte), never on total time; 512 KiB parts sent whole. (12e)
- [x] Tab cap counts every open FERB tab of the tree and refuses past 50 (worker open, link, popup), never a silent
      hand-off to the browser. (12e)
- [x] Dropped page saves are all reported, with a count. (12e)
- [x] One unit for download limits: decimal GB; MB keys upgraded once, journaled. (12e)
- [x] The worker moves a download to where the owner wants it instead of copying it. (12e)
- [x] Known limitation, owner's call: the 20 GB folder cap moves files to the Trash, which frees no space until it is
      emptied. (Decision 8, step 12e part 2: the owner is asked; Yes deletes the oldest for good.)
- [x] Foreground right after the owner's Yes in the notch goes through; one foreground question per app however it is
      spelled. (12e part 2; tests/test_own_apps.py, tests/test_foreground_asks.py.)
- [x] An app that comes forward by itself during a call is told to the worker, never put back (decision 9).
      (12e part 2: `screen_changed_during_call`; tests/test_mac_focus.py.)
- [x] Background launch without `-j`; `mac.open` reopen for a windowless app; `mac.open` path/url opened without
      activation through the gateway's file, address and terminal checks. (12e part 2; tests/test_mac_focus.py.)
- [x] "Back at your Mac" from wake / unlock / login / restart, not input idleness; held banners while the Mac is out
      of use (decision 10). (12e part 2: `MacPresence`; tests/test_when_active.py, test_schedules.py,
      test_execution_runs.py.)
- [x] Sound facts skip the owner's own mute and input-coincident changes (decision 11; protocol 21 / 0.19.0).
      (12e part 2; tests/test_mac_focus.py.)
- [ ] Step 13 measurements above (Step 13 must measure (step 12e part 2)).
- [x] One protection check shared by shell and file tools, with the lift rule of gateway decision 2 and decision 1 here.
      (5a: `Protection.fs_rules`; `fs_reads_never_read` gone.)
