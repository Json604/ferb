# FERB

FERB is a personal agent runtime for macOS. You talk to it in a terminal (`ferb chat`). It plans the work, runs model
workers (Codex by default, Claude Agent SDK as an alternative), and every action they take goes through one gateway
that checks it against what you asked for. It can operate your Mac apps (in the background where it can), your
paired Chrome/Brave profiles, a shell, files, and a separate browser profile with FERB's own accounts. You can also
talk to it: hold Right Option, speak, let go (push-to-talk), and it answers out loud. It keeps a record of your
activity (metadata only), daily summaries, and a memory of facts you tell it.

- **Status:** v0.1 ("Block 1"). All 24 safety tests in [ARCHITECTURE.md §21](ARCHITECTURE.md) pass, offline and
  against real models, browsers and the Mac. Built and tested by one person on one Mac. Expect rough edges.
- **Platform:** macOS only, Apple silicon only (FERB pins the arm64 Codex binary). Developed and tested on macOS 26.
- **Interface:** the CLI only, plus push-to-talk. There is no GUI, menu-bar item or web dashboard. FERB.app (for
  push-to-talk) runs in the background with no window, Dock icon or menu-bar item.

[CAPABILITIES.md](CAPABILITIES.md) lists what FERB can do and how well tested each item is.
[CODE_TOUR.md](CODE_TOUR.md) walks through the code. [ARCHITECTURE.md](ARCHITECTURE.md) is the design contract.

## Requirements

| Needed for                     | Requirement                                                                                                                |
| ------------------------------ | -------------------------------------------------------------------------------------------------------------------------- |
| Everything                     | macOS on Apple silicon;[uv](https://docs.astral.sh/uv/) (setup uses it to install a dedicated Python 3.12 and FERB's venv)  |
| The default runtime            | Node.js with`npm` (setup installs a pinned Codex, 0.157.1, into `~/.ferb/node`); Codex signed in for your macOS user   |
| The Claude runtime (optional)  | Claude signed in for your macOS user (the Agent SDK is a Python dependency; FERB clears`ANTHROPIC_API_KEY` for its runs) |
| Browser control (optional)     | Google Chrome and/or Brave, with the FERB extension from`extensions/chromium` loaded unpacked                            |
| FERB's own accounts (optional) | A Chrome profile that holds only FERB's Google account                                                                     |
| Jev decisions (optional)       | A Jev API key (shadow mode only; see ARCHITECTURE.md §12)                                                                 |
| Push-to-talk (optional)        | Xcode command-line tools (`swiftc`) and a local code-signing certificate named "FERB Local Code Signing" in your login keychain (Keychain Access → Certificate Assistant → Create a Certificate, type Code Signing) |

## Install and setup

```sh
git clone <this repo> && cd FERB
uv sync                  # development environment (.venv)
uv run ferb setup
```

`ferb setup` runs four steps:

1. Installs FERB's dedicated Python, its venv (a copy of this checkout), the pinned Codex, the native-messaging
   host, and the LaunchAgent `com.ferb.daemon`. It then starts the daemon.
2. Scans the Mac's apps (Info.plist only) into a device profile.
3. Asks for the permissions macOS needs up front: **Accessibility** and **Screen Recording**, plus Input Monitoring
   if FERB's watcher of your own mouse and keyboard can't start. Permissions are checked by the daemon itself and
   must be granted to the interpreter path setup prints, not to your terminal.
4. Runs a self-test through the daemon.

Everything else is asked the first time a task needs it. That includes folder access, your email address for
"email me", and AppleScript automation per app.

After changing the code, `ferb setup --update` reinstalls FERB into its venv and restarts the daemon.

### Optional: browsers

Pair each profile you want FERB to use: `ferb browsers pair [--browser chrome|brave] "<profile name>"`.

- If FERB's extension isn't installed in that profile yet, pairing walks you through it. It opens the profile's
  extensions page, copies the extension folder (`extensions/chromium`) to your clipboard and shows the three clicks
  (Developer mode → Load unpacked → ⌘⇧G, paste, Select), waits until Chrome has it, then pairs. Chrome only allows
  installing an unpacked extension by hand (the command-line way was removed in Chrome 137). The manifest carries a
  fixed key, so the extension id matches the one the native host accepts.
- `ferb browsers` lists profiles: paired, connected, or "not open right now" (FERB opens its own profile when a task
  needs it).
- An installed but unpaired extension can't act or report anything. Pairing your real profiles is gated by
  `[gates] owner_profiles` in `config.toml` (see Configuration).

### Optional: push-to-talk

```sh
ferb setup --voice
```

This builds FERB.app into `~/.ferb/FERB.app`, signs it with your local certificate (so its permissions survive
rebuilds), asks once for **Microphone** and **Speech Recognition**, and turns push-to-talk on. Then:

- **Hold Right Option**, speak, let go. A soft sound plays once the hold passes 0.3 s; a quick tap, or Right Option
  plus another key (a shortcut), does nothing. Pressing it again cuts FERB off mid-sentence.
- FERB answers out loud. When it's waiting on a question, say "yes", "no" or a number; during a send or call
  countdown, say "stop" to cancel.
- `ferb voice` shows what FERB heard and said back, live. `ferb voice off` stops push-to-talk (FERB.app quits and
  FERB stops speaking); `ferb voice on` turns it back on.
- Speech recognition runs on your Mac. Audio is never saved. FERB never opens a Bluetooth headset's microphone (that
  drops the headset into call-quality audio); it uses the built-in mic instead.

### Optional: FERB's own identity

FERB can use a Chrome profile that holds **only** FERB's own Google account (never yours):

```sh
ferb setup --identity [--profile "<chrome profile name>"]
```

Setup verifies the profile holds exactly FERB's account, records FERB's email address, and pairs the profile as
FERB's identity. FERB's passwords are stored in the macOS Keychain (service `ferb`) and filled into pages by the
extension. The model never sees them.

### Optional: Jev

`ferb setup --jev` stores the Jev API key in the Keychain and checks that it answers. Jev runs in shadow mode:
its answers are logged next to FERB's own decisions until you enable a question with `ferb decide enable`.

## Quickstart

```sh
ferb chat
```

Example requests (see [CAPABILITIES.md](CAPABILITIES.md) for more):

```
hi, who are you and what can you do?
in your workspace create notes.md with the line 'one', then append 'two'
what apps take the most space on my mac, and how can I free some up?
remind me next friday at 5 pm to call mom
every morning at 9, summarize the files in ~/Documents/inbox and write summary.md in your workspace
open example.com in my <profile> profile and tell me what's on the page
make a note in Notes titled 'groceries' with milk and eggs
remember that my dentist is Dr. Rao at Smile Clinic
```

In chat:

- **Ctrl-C** stops the running task ("stopped by you"); a message held for undo is cancelled too.
- Closing chat (or `/quit`) does **not** stop tasks. When one finishes while chat is closed, you get a macOS
  notification, and chat shows the answer next time it opens.
- Pasted text arrives in one piece and is treated as data, never as instructions. If the whole message is a paste
  (or a paste with just "hey" before it), chat asks whether it's your own request. You can turn this rule off with
  `[policy] pasted_text_is_data = false`. Option-Enter starts a new line.
- A misspelled app name is read as the app it's close to ("salck" → Slack, and FERB says so); if it's close to
  several, FERB asks which.
- When FERB needs your OK, it asks inline (`[y/N]`, or a numbered choice). If chat is closed, a notification tells
  you to open it.
- `ferb chat --verbose` also shows plans, steps, tool calls and checks. `ferb chat --speak` also says FERB's replies
  out loud.
- To see what a task did afterwards, step by step: `ferb trace` (the latest task) or `ferb trace <first few
  characters of its id>`; `ferb timeline` lists recent tasks with their ids.

## Command reference

All commands talk to the daemon over `~/.ferb/run/ferb.sock` (owner-only permissions).

| Command                                                                                    | What it does                                                                                                    |
| ------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------- |
| `ferb chat [--verbose] [--speak]`                                                                  | Talk to FERB                                                                                                    |
| `ferb logs [--tasks] [--trail] [--tools] [--browsers] [--policy] [--actions] [--timers] [--voice]` | Live stream from the daemon's bus, optionally filtered                                                          |
| `ferb trace [task] [--full]`                                                             | What a task did, step by step: each step's goal and result, every tool call and what came back. Default: the latest task; a short id works |
| `ferb voice [on\|off]`                                                                   | Push-to-talk: watch what FERB heard and said (no argument), or turn it on / off                                |
| `ferb timeline [--since YYYY-MM-DD]`                                                     | The journal: chats, task outcomes, daily summaries                                                              |
| `ferb stop`                                                                              | Emergency stop: revoke tokens, block all actions, cancel held sends (survives restarts)                         |
| `ferb resume-tasks`                                                                      | Lift the emergency stop                                                                                         |
| `ferb undo [hold_id]`                                                                    | Cancel a pending send/post/call during its undo window                                                          |
| `ferb autonomy [careful\|balanced\|hands-off]`                                             | Show or set the autonomy level                                                                                  |
| `ferb pause` / `ferb resume`                                                           | Stop / restart every activity watcher; nothing is recorded while paused                                         |
| `ferb memory list [--proposed] [--all]`                                                  | Active memory items (or ones waiting for you, or everything)                                                    |
| `ferb memory show [subject]`                                                             | Every item about a subject, with history and sources                                                            |
| `ferb memory add [--kind K] [--subject S] [--key KEY] <content>`                         | Remember something you state (kinds: fact, preference, person, project, env)                                    |
| `ferb memory confirm <id>`                                                               | Activate an inferred item                                                                                       |
| `ferb memory edit <id> <content>`                                                        | Correct an item (new version; the old one stays as history)                                                     |
| `ferb memory retract <id>`                                                               | Stop using an item (kept as history)                                                                            |
| `ferb memory forget [--keep-sources] [--yes] <id>`                                       | Purge an item, its versions and its sources (asks first)                                                        |
| `ferb reminders [list\|cancel] [id]`                                                      | Pending reminders and timers                                                                                    |
| `ferb schedules [list\|cancel\|pause\|resume] [id]`                                         | Scheduled and recurring tasks                                                                                   |
| `ferb browsers`                                                                          | Browser profiles, pairing and connection status                                                                 |
| `ferb browsers pair [--browser chrome\|brave] <profile>`                                  | Pair a profile; walks you through installing the extension there first if it's missing                         |
| `ferb browsers unpair [--browser chrome\|brave] <profile>`                                | Forget a profile's pairing                                                                                      |
| `ferb device [show\|scan]`                                                                | This Mac's device profile (apps, scriptable apps, browsers);`scan` rescans                                    |
| `ferb ps` / `ferb kill <id>`                                                           | Background processes FERB started / stop one                                                                    |
| `ferb decide stats`                                                                      | Jev's answers next to FERB's decisions, per question                                                            |
| `ferb decide enable <question>` / `disable <question>`                                 | Let Jev's stricter answers act for one question, or go back to shadow mode                                      |
| `ferb setup [--update] [--identity [--profile P]] [--jev] [--codex-login] [--voice]`     | Install / update / identity profile / Jev key / sign in FERB's own Codex / set up push-to-talk                 |
| `ferb doctor`                                                                            | Check the installation, permissions (from inside the daemon), browsers, identity, and run the self-test         |
| `ferb uninstall [--purge]`                                                               | Remove the LaunchAgent and native-host files;`--purge` also deletes `~/.ferb` (asks you to type `delete`) |
| `ferb daemon run`                                                                        | Run the daemon in the foreground (development)                                                                  |
| `ferb mcp`                                                                               | Internal: the stdio MCP shim runtimes use to reach FERB's tools (needs`FERB_RUN_TOKEN`)                       |

## Configuration

`~/.ferb/config.toml` must be owner-only (FERB refuses a config other users can read). Setup and the daemon write
defaults for the privacy keys on first run, so you can see and edit them there. Keys FERB reads:

| Section        | Key                       | Default                                                              | Meaning                                                                                    |
| -------------- | ------------------------- | -------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| `[runtimes]` | `orchestrator`          | `"codex"`                                                          | Runtime for planning:`codex` or `claude`                                               |
|                | `worker`                | `"codex"`                                                          | Runtime for workers:`codex` or `claude`                                                |
|                | `claude_model`          | SDK default                                                          | Model for the Claude runtime                                                               |
| `[policy]`   | `autonomy`              | `"balanced"`                                                       | `careful` (confirm every non-reversible action), `balanced`, `hands-off`             |
|                | `trusted_kinds`         | `[]`                                                               | Action kinds`careful` mode no longer confirms                                            |
|                | `trusted_accounts`      | `[]`                                                               | Site + profile pairs where you approved an account FERB can't verify (written by FERB)     |
|                | `pasted_text_is_data`   | `true`                                                             | Pasted text is data, never instructions; `false` treats it as your own words              |
| `[fs]`       | `owner_roots`           | `[]`                                                               | Your folders FERB may use in your tasks (added when you say yes to a folder question)      |
|                | `untrusted_roots`       | `["~/Downloads"]`                                                  | Folders whose files count as untrusted content                                             |
| `[privacy]`  | `exclude_apps`          | password managers (1Password, Bitwarden, Passwords, Keychain Access) | Never recorded, never captured or acted on; you may edit or empty it                       |
|                | `exclude_domains`       | `[]`                                                               | Sites never recorded in the activity trail                                                 |
|                | `exclude_paths`         | `["~/.ssh", "~/Library/Keychains", "~/.ferb"]`                     | Never recorded, even inside a watched folder                                               |
| `[sense]`    | `window_titles`         | `true`                                                             | Record (redacted) window titles                                                            |
|                | `watch_folders`         | `[]`                                                               | Folders whose file events are recorded (nothing is watched unless listed)                  |
|                | `idle_after_s`          | `120`                                                              | Seconds without input before you count as idle                                             |
|                | `trail_retention_days`  | `30`                                                               | Raw activity rows kept; days are purged only after their daily summary exists              |
| `[mac]`      | `snapshot_image`        | `true`                                                             | Mac app snapshots also carry an image of the window with each control numbered               |
| `[voice]`    | `push_to_talk`          | `false`                                                            | Set by `ferb setup --voice` / `ferb voice on\|off`                                        |
|              | `mic`                   | `"auto"`                                                           | `auto` (your default input, built-in if that's Bluetooth), `builtin`, `default`; never a Bluetooth mic |
|              | `sounds`                | `true`                                                             | The start / stop sounds                                                                    |
|              | `requirement`           | set by setup                                                         | FERB.app's code signature; only that app may send speech to FERB                         |
| `[browser]`  | `prefer_for_actions`    | `["brave", "chrome"]`                                              | Browser order when a site is open in several                                               |
| `[identity]` | `browser`, `profile`  | set by`ferb setup --identity`                                      | FERB's own Chrome profile                                                                  |
| `[mailbox]`  | `address`, `provider` | set by`ferb setup --identity`                                      | FERB's own email address (Gmail)                                                           |
| `[gates]`    | `owner_profiles`        | `false`                                                            | Allow pairing your real browser profiles (turn on once the §21 safety tests pass for you) |
| `[jev]`      | `enabled`               | `[]`                                                               | Jev questions allowed to act (set by`ferb decide enable`)                                |
| `[device]`   | `id`                    | generated                                                            | This Mac's device id                                                                       |

The daemon re-reads `[policy]` settings when they change through the CLI. For other edits, run
`ferb setup --update` to restart it.

## Permissions and privacy

**macOS permissions** are granted to FERB's dedicated interpreter (the path `ferb setup` prints):

- Accessibility: read and operate app windows. Required for Mac control.
- Screen Recording: screenshots of a single app window. Required for screenshot clicks.
- Input Monitoring: only if FERB's watcher of your own mouse and keyboard can't start without it. FERB uses it to
  hear the push-to-talk key and to step aside while you're typing or moving the mouse.
- Microphone and Speech Recognition: for FERB.app only, asked by `ferb setup --voice`.
- Automation (AppleScript): asked per app, the first time a task needs it.
- Full Disk Access: optional; not needed for normal use.

**What's recorded (the activity trail):** metadata only. That means which app is in front, window titles
(redacted; switch off with `[sense] window_titles`), browser tab URLs and titles from paired profiles (redacted),
idle/active transitions, app installs, and file events in the folders you list. Every row records whether you or
FERB caused it.

**What's never recorded:** keystroke content, clipboard contents, or screen contents. Also skipped: apps on
`exclude_apps`, sites on `exclude_domains`, paths on `exclude_paths`, and incognito/private tabs. Excluded events are
dropped before anything is written, published or sent to a model. `ferb pause` stops every watcher until
`ferb resume`, and the pause survives restarts.

**Secrets:** FERB's own passwords live in the macOS Keychain and are filled by the extension; they never appear in
results, the database or logs. Password and one-time-code fields are never read or typed by workers. Screenshots
stay in memory and are never written to disk. A window showing a detectable password field is never captured.

**Where data lives:**

| Path                                             | Contents                                                                                                  |
| ------------------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| `~/.ferb/ferb.db`                              | SQLite (WAL): tasks, spans (traces), journal, activity trail, memory, timers, schedules, approvals ledger |
| `~/.ferb/config.toml`                          | Configuration (owner-only)                                                                                |
| `~/.ferb/workspace/`                           | FERB's own working folder (its files, not yours)                                                          |
| `~/.ferb/memory/`                              | Read-only markdown export of active memory, regenerated after each change (created on the first item)     |
| `~/.ferb/device/`                              | The device profile (JSON) with local git history                                                          |
| `~/.ferb/logs/`                                | Daemon stdout/stderr logs                                                                                 |
| `~/.ferb/run/`                                 | The daemon socket and the planner's scratch directory                                                     |
| `~/.ferb/python`, `venv`, `node`, `bin`  | The dedicated interpreter, FERB's venv, the pinned Codex, the native-host launcher                        |
| `~/.ferb/codex-home/`                          | FERB's own Codex settings and sign-in (`ferb setup --codex-login`), kept apart from your personal Codex  |
| `~/.ferb/FERB.app`                             | The push-to-talk app (built by `ferb setup --voice`)                                                      |
| `~/Library/LaunchAgents/com.ferb.daemon.plist` | The LaunchAgent                                                                                           |
| Chrome/Brave`NativeMessagingHosts/`            | The native-host manifest for the extension                                                                |
| macOS Keychain, service`ferb`                  | FERB's own secrets and the Jev key                                                                        |

Nothing is sent anywhere except to the model provider of the runtime you configured (and to Jev, if you set it
up). Content goes to them only after exclusions and redaction.

**Retention and forgetting:**

- Raw trail rows are kept for 30 days. Each day first gets a permanent summary in the journal. The purge refuses any
  day without its summary.
- `ferb memory forget <id>` is a true purge. It removes the item, its versions, its sources in the journal and trail,
  and the search index. Affected daily summaries are rebuilt or deleted, and the markdown export is regenerated.
- `ferb memory retract` only stops using an item and keeps it as history.

**Uninstall:**

- `ferb uninstall` removes the LaunchAgent and the native-host files.
- `ferb uninstall --purge` also deletes `~/.ferb`, after you type `delete`, and offers to delete FERB's secret
  Keychain items.
- Remove the Jev key yourself if you stored one: `security delete-generic-password -s ferb -a <account>`
  (`security find-generic-password -s ferb` lists them).
- macOS can't revoke permissions for you: remove the interpreter from System Settings → Privacy & Security.
- Remove the extension from each browser yourself.

## Safety model (summary)

The full contract is [ARCHITECTURE.md §10](ARCHITECTURE.md), with the safety tests in §21.

- **Two identities:** *owner* tasks use your Mac, your paired browser profiles and your shell, and never hold
  credentials. *FERB* tasks use only FERB's own browser profile and accounts, and never touch yours.
- **Whose browser:** your browser profiles are used only when your words point at them ("my browser", "my Brave",
  "my <profile> profile", "my school gmail in chrome", "the current tab", "my LinkedIn"). A plain "do X on YouTube"
  uses FERB's own browser. The browser or profile you name is the only one used; if it isn't connected, FERB says
  why instead of using another. FERB can take over a tab you already have open, and never closes it.
- **Your screen stays yours:** FERB works in Mac apps from the background where it can (Accessibility, then
  keystrokes sent to the app itself). When it has to bring an app forward, it does so once per stretch of work,
  holds back your first key or click so it never lands in FERB's app, steps aside while you use the Mac, and puts
  your app back. A browser that jumps forward when FERB opens it is sent back too.
- **One gateway:** every tool call from a model goes through `ferb mcp` → the daemon's gateway → the policy engine.
  Runtimes' built-in tools (their own shell, file writes, web access) are disabled and checked on every run.
- **Intent:**
  - The actions allowed, recipients, sites, apps, paths and amounts are fixed from your own words when the task is
    planned, before any untrusted content is read.
  - An action kind you didn't ask for is blocked.
  - An action on a target outside the request asks you.
  - Pasted or quoted text never contributes to the intent.
- **Untrusted content:** a worker that has read web pages, email or messages loses shell, AppleScript and file-write
  tools. Readers and actors are split, so raw untrusted text never reaches a worker that can act.
- **Data flow:** data read from your accounts may only go to destinations the request involves.
- **Confirmations are rare:**
  - Reversible actions just run.
  - Sends, posts, messages and calls you asked for are held about 10 s with an undo countdown.
  - Payments, permanent deletion (including `rm -rf`, `git push --force`) and account/security changes always ask,
    every time.
  - One approval covers the same target and action type for the rest of the request.
- **Contacts are never guessed:** a call or message must name a contact that was actually on screen. Several
  matches means FERB asks "Which …?". In messaging apps FERB presses the Send button (Return is refused, so the
  countdown and contact check always apply), and a second message to the same person in one task asks you first.
- **Exploring:** workers may look at other apps and sites, with a stated reason. Acting outside the plan's apps and
  sites needs your yes.
- **Runaway guards:** the same failing action 3×, loops, 4 h / 50 worker runs / 1,000 tool calls per task.
- **Emergency stop:** `ferb stop` halts everything immediately and survives restarts.
- **Traces:** every tool call is a span, redacted before it's stored. See `ferb trace` (the latest task) or
  `ferb trace <id>`.

## Troubleshooting

Start with `ferb doctor`. It checks the LaunchAgent and interpreter, each permission (as the daemon sees it), the
native-host manifests, paired browsers and their connections, FERB's identity profile and session, device-profile
drift, and runs the self-test. Each problem comes with a hint.

| Symptom                                                         | Likely cause / fix                                                                     |
| --------------------------------------------------------------- | -------------------------------------------------------------------------------------- |
| `ferb chat` can't connect                                     | Daemon not running:`ferb setup --update` (restarts it); logs in `~/.ferb/logs/`    |
| "Unavailable: the owner's browsers (… paired but not open …)" | Open that browser profile; the extension reconnects within 30 s                        |
| "Unavailable: FERB's own browser (not connected …)"            | Open FERB's Chrome profile; check`ferb doctor`                                       |
| Mac control unavailable                                         | Accessibility / Screen Recording not granted to FERB's interpreter (not your terminal) |
| A permission was just granted but still shows denied            | Screen Recording applies after the daemon restarts:`ferb setup --update`             |
| The extension shows as installed but unpaired                   | `ferb browsers pair --browser <chrome\|brave> "<profile>"`                            |
| "I can't use your Chrome “…” profile yet"                       | That profile isn't connected: `ferb browsers pair "<profile>"` (it walks you through installing the extension) |
| What did a task actually do?                                    | `ferb trace` (latest) or `ferb trace <id>`; `--full` for everything                 |
| Push-to-talk does nothing                                       | `ferb doctor` (push-to-talk line); `ferb voice on`; `ferb setup --voice` if FERB.app isn't set up |
| Push-to-talk is on and you want it off                          | `ferb voice off`                                                                     |
| `ferb doctor` says "installed code differs"                     | `ferb setup --update` (close `ferb chat` first)                                      |
| Everything is blocked                                           | An emergency stop is active:`ferb resume-tasks`                                      |
| Nothing is recorded                                             | Watchers paused:`ferb resume`                                                        |
| Device profile drift in doctor                                  | `ferb device scan`                                                                   |

## Development

```sh
uv sync
uv run pytest                 # offline suite (fakes for models, browsers and the Mac); real tests are deselected
uv run mypy ferb
```

- **Real tests** run against real models, browsers and the Mac: `uv run pytest -m real`.
  - They use the **Codex runtime only**.
  - Claude-runtime variants run only with `-m real_claude`, and only when you trigger them yourself.
  - Real tests are locked down: each test daemon keeps only the tools that test needs. Mac, shell and AppleScript
    tools are included only when the test is about them.
  - Browser tests need a spare Chrome profile with the extension loaded, e.g.
    `FERB_REAL_PROFILE="chrome:<spare profile>" uv run pytest -m real tests/test_ext_real.py`. The contacts test
    expects a Chrome profile named "FERB Test".
  - They point the native-host launcher at their own test daemon and give it back to the installed daemon afterwards.
- **Foreground daemon:** `FERB_HOME=/tmp/ferb-dev uv run ferb daemon run` runs a separate daemon with its own home,
  side by side with the installed one. See [CODE_TOUR.md](CODE_TOUR.md) for where to set breakpoints.
- **Codex in development** comes from the repo's `node_modules` (`npm install` in the repo); the installed daemon
  uses `~/.ferb/node`.
- **Workflow:** `ARCHITECTURE.md` is the source of truth, `PROGRESS.md` tracks the build, and `CLAUDE.md` holds the
  working rules for the coding agent that builds FERB.
- `spikes/` holds the feasibility experiments. Production code never imports from them.

## Known limitations

- macOS only, Apple silicon only; one Mac (no multi-device sync); single user.
- Browsers: Chrome and Brave through the extension. Safari is not supported yet. The extension is loaded unpacked
  (no Web Store listing), by hand in each profile (`ferb browsers pair` guides you).
- On macOS any new browser window can pull the browser to the front (a known Chromium problem). FERB avoids
  creating windows and takes you straight back when one does, but you may see a brief switch.
- Push-to-talk recognizes speech on your Mac (English); FERB's own name is often misheard, which doesn't matter
  since the key is the trigger. There's no wake word or always-listening mode yet.
- The planner model has its own judgment; FERB's instructions tell it you decide what's appropriate for your own
  work, which reduces but can't remove its caution.
- The CLI is the only interface.
- FERB never signs in as you. A signed-out page stops the task.
- Opening and reading apps/sites is never confirmed (only acting is). A worker may explore more than you expect.
  Each such look carries a stated reason in the trace.
- The runaway guards and a 4-hour limit per task are fixed.
- Prompt injection is reduced (intent matching, exposure, reader/actor split, data flow), not eliminated.
- Learned trust is manual. FERB doesn't yet offer to stop asking about an action type; `[policy] trusted_kinds` does
  it by hand.
- Scheduled tasks run only while the daemon is running. A missed run happens once when the Mac wakes, marked late,
  or is skipped if too late.
- Jev only runs in shadow mode until you enable individual questions.
- Real-run verification covers the items marked "verified" in [CAPABILITIES.md](CAPABILITIES.md). Everything else is
  tested offline only.

## Roadmap

Later blocks (see ARCHITECTURE.md §18–§19):

- Safari control (FERB.app, which already runs push-to-talk, will host the Safari extension).
- Listening to system audio (what's playing) as a tool; an always-on voice mode with a wake word.
- A GUI, designed separately.
- Read-only planner lookups for large trusted data (e.g. "is app X installed?" against the device profile). The
  planner never gets direct access to trail data.
- Embeddings behind memory recall, without changing its interface.
- Per-resource grants with process isolation for arbitrary code (needed for multi-user).
- More devices.
