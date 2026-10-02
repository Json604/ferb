# FERB

FERB is a personal agent runtime for macOS. The owner's goal, in their words: "an agent that can control my Mac the
way I can, and do things building on top of that base, and keep learning with doing." You talk to it in a terminal
(`ferb chat`). For each message it replies, asks one question, or runs **one worker** (Codex by default, the Claude
Agent SDK as an alternative) that works your Mac apps and browser tabs **in the background**, so you keep using the
Mac while it works. Every tool call the worker makes goes through one gateway that decides, from what the call
really does, whether it runs, is held for undo, or needs your yes. What works per app is learned from doing, not
written into the code.

- **Version:** 0.20.1 (see [CHANGELOG.md](CHANGELOG.md)). Built and tested by one
  person on one Mac. Expect rough edges.
- **Platform:** macOS only, Apple silicon only (FERB pins the arm64 Codex binary). Developed on macOS 26.
- **Interface:** the CLI, plus push-to-talk. No GUI, menu-bar item or web dashboard. FERB.app (for push-to-talk)
  runs in the background with no window, Dock icon or menu-bar item.

[CAPABILITIES.md](CAPABILITIES.md) lists what works today and how each item is verified. [CODE_TOUR.md](CODE_TOUR.md)
walks the flows through the code. [ARCHITECTURE.md](ARCHITECTURE.md) is the design contract. The docs map is at the
end of this file.

See [the 0.20.0 pointer and continuity notes](docs/POINTER_CONTINUITY_020.md) for current behavior, validation and limits.
Plain `ferb chat` resumes the last conversation across CLI and daemon restarts; `--new` starts another, and
`--resume ID` returns to an existing one. Browser screenshot targets now preserve exact coordinates and support dragging.

## Requirements

| Needed for | Requirement |
|---|---|
| Everything | macOS on Apple silicon; [uv](https://docs.astral.sh/uv/) (setup installs a dedicated Python 3.12 and FERB's venv) |
| The default runtime | Node.js with `npm` (setup installs the pinned Codex 0.157.1 into `~/.ferb/node`); Codex signed in for your macOS user. The model is pinned in `ferb/runtimes/codex_exec.py` (`MODEL`) |
| The Claude runtime (optional) | Claude signed in for your macOS user (the Agent SDK is a Python dependency; FERB clears `ANTHROPIC_API_KEY` for its runs) |
| Browser control (optional) | Google Chrome and/or Brave, with the FERB extension (`extensions/chromium`, version 0.9.0) loaded unpacked in each profile you pair |
| FERB's own accounts (optional) | A Chrome profile that holds only FERB's Google account |
| Jev decisions (optional) | A Jev API key (shadow mode by default; ARCHITECTURE.md §5) |
| Push-to-talk (optional) | Xcode command-line tools (`swiftc`) and a code-signing certificate named "FERB Local Code Signing" in your login keychain |

## Install and setup

```sh
git clone <this repo> && cd FERB
uv sync                  # development environment (.venv)
uv run ferb setup
```

`ferb setup` runs four steps (`ferb/cli/setup.py:setup`): it installs FERB's dedicated Python, its venv (a copy of
this checkout), the pinned Codex, the native-messaging host and the LaunchAgent `com.ferb.daemon`, then starts the
daemon; scans the Mac's apps (Info.plist only) into a device profile; asks for **Accessibility** and **Screen
Recording** (plus Input Monitoring if FERB's watcher of your own input can't start), checked by the daemon itself and
granted to the interpreter path setup prints, not to your terminal; and runs a self-test through the daemon.
Everything else (folder access, your email address, AppleScript automation per app) is asked the first time a task
needs it. After changing the code, `ferb setup --update` reinstalls FERB into its venv and restarts the daemon.

**Browsers.** Pair each profile FERB may use: `ferb browsers pair [--browser chrome|brave] "<profile name>"`. If the
extension isn't installed there yet, pairing walks you through Load unpacked (Chrome only allows installing an
unpacked extension by hand), waits until the browser has it, then pairs. `ferb browsers` lists profiles and whether
they're connected. `[gates] owner_profiles` changes unpaired-profile guidance; it does not enforce a pairing gate.
After an extension update, reload
the unpacked extension in each paired profile: the daemon refuses extensions older than 0.9.0 at hello.

**Push-to-talk.** `ferb setup --voice` builds and signs `~/.ferb/FERB.app`, asks once for Microphone and Speech
Recognition, and turns push-to-talk on. Hold Right Option, speak, let go (a hold under 0.3 s is a tap and does
nothing; Right Option plus another key is a shortcut). FERB answers out loud; say "yes", "no", a number, or "stop"
during a countdown. `ferb voice` shows what it heard; `ferb voice off` / `on` switch it. Speech recognition runs on
the Mac; audio is never saved; a Bluetooth headset's microphone is never opened.

**FERB's own identity.** `ferb setup --identity [--profile "<name>"]` verifies a Chrome profile holds exactly FERB's
Google account, records FERB's email address and pairs that profile as FERB's identity. FERB's passwords live in the
macOS Keychain (service `ferb`) and are filled into pages by the extension; the model never sees them.

**Jev.** `ferb setup --jev` stores the key in the Keychain. Jev runs in shadow mode (its answers are logged next to
FERB's own decisions) until you enable a question with `ferb decide enable`.

## Talking to FERB

```sh
ferb chat
```

```
hi, who are you and what can you do?
in your workspace create notes.md with the line 'one', then append 'two'
what apps take the most space on my mac, and how can I free some up?
in TextEdit, make a new document and write a two-line haiku about rain
search WhatsApp for 'syllabus' and tell me the latest message that mentions it
in my <profile> profile, open wikipedia.org and tell me today's featured article title
remind me next friday at 5 pm to call mom
every morning at 9, summarize the files in ~/Documents/inbox and write summary.md in your workspace
remember that my dentist is Dr. Rao at Smile Clinic
```

- Each message gets exactly one of: a reply, one clarifying question (your next message continues the request), or
  a task. A task is one worker run with every tool its identity allows; its report is your answer.
- **Ctrl-C** stops the running task ("stopped by you"); a message held for undo is cancelled too. Closing chat does
  not stop tasks: when one finishes you get a macOS notification and chat shows the answer next time.
- Pasted text arrives whole and is treated as data, never as instructions. A message that is entirely a paste asks
  whether it's your own request. `[policy] pasted_text_is_data = false` turns the rule off.
- When FERB needs your OK it asks inline: `[y/N]` for an action, a numbered list for "Which Rahul?", and for a
  worker's own question (`owner_ask`) a number or your own words (Enter skips). A question expires after 10 minutes.
- `ferb chat --verbose` also shows the task, tool calls, policy decisions and checks. `--speak` says replies aloud.
- `ferb trace` shows what the latest task did, step by step (every tool call with its declared effect, reason and
  result); `ferb trace <first characters of an id>` for another; `ferb timeline` lists recent tasks.

## What it can do today

Read and operate Mac apps in the background through Accessibility (snapshots of what's really on screen, presses,
typing, in-place edits, menus, scrolling, pop-ups, sheets and dialogs); work in your paired Chrome/Brave profiles or
in FERB's own browser inside a minimized window (real mouse and keyboard input through a debugger
session, scrolling of containers, dialogs, uploads); run shell commands and drive terminal programs;
read and write files in its workspace and folders you allow; email from FERB's own address; reminders, timers and
scheduled tasks; a metadata-only record of your activity with daily summaries; memory of what you tell it; and
push-to-talk. [CAPABILITIES.md](CAPABILITIES.md) has the full list with how each item is verified and its limits.

## How a request runs

1. The supervisor model (`prompts/supervisor.md`) answers with `reply`, `clarify` or `task` (goal, identity `owner`
   or `ferb`, the targets it understood, optional success conditions). The only check in code is the identity floor:
   a `ferb` task never gets your Mac, browsers or folders.
2. One worker (`prompts/worker.md`) runs with every tool its identity allows right now. Mac apps and browser tabs are
   worked in the background; the worker never activates an app.
3. Every acting call declares an `effect` — `navigate`, `edit`, `send`, `pay` or `destroy` — and a `why`. The gateway
   raises the declaration to the control's floor (a button labelled Send is a send whatever the worker said; Return
   in a message box is a submit; a security page is `destroy`). Navigate and edit run. A send asks once per target
   unless your own message named it. Send/post/reply/call/mailbox-send kinds are held 10 s for undo; form submits
   have no undo hold. Pay and destroy ask every time. Careful mode also confirms action kinds outside `trusted_kinds`.
4. The worker's summary is your answer. Success conditions are checked by code where possible and by a read-only
   verifier run otherwise; an action whose outcome is unknown is re-read, never retried blindly.

## Safety model

See [identities (§3)](ARCHITECTURE.md#3-identities-and-scopes),
[the action gate (§5)](ARCHITECTURE.md#5-the-gate-at-the-action), and
[testing (§14)](ARCHITECTURE.md#14-testing).

- **Two identities.** `owner` tasks use your Mac, paired profiles and shell and never hold credentials; `ferb` tasks
  use only FERB's own browser and accounts. Your profiles are used only when your words point at them.
- **One gateway.** Every model tool call goes `ferb mcp` → gateway → policy engine. Runtimes' native tools are off
  and the tool list the model saw is checked on every run.
- **The effect gate.** Declared effect raised to a code floor; floors only raise. Sends to people you didn't name
  ask once; pay, permanent deletion and account/security changes ask every time; sends, posts and calls are held
  10 s for undo; a second message to the same person in one task asks; contacts are never guessed.
- **Untrusted content.** Pasted text never counts as your words. A worker that read a web page, email, message or a
  download loses shell, AppleScript and file-write for the rest of its run. Data read from your accounts may only go
  to a target of the request, somewhere you named, or somewhere you approved.
- **Your screen stays yours.** Background first; the foreground only when a call passes `allow_foreground=true`, once
  per stretch of work, with your first key or click held back and your app put back afterwards.
- **Secrets.** FERB's passwords and one-time codes are sealed to the extension; password fields are never read or
  typed; screenshots never touch disk.
- **Guards.** The same failing call 3×, loops, 4 h / 50 worker runs / 1,000 tool calls per task; `ferb stop` halts
  everything and survives restarts; every tool call is a redacted trace span.

## Owner-verified testing

Offline tests prove contracts; they can't prove that nothing moved on your screen. Two things do:

- `ferb watch` runs each kind of interaction FERB uses — 13 Mac steps on TextEdit and Calculator, 12 browser steps
  in a throwaway Chrome with its own data folder — with the target in the background, and after each one asks what
  you saw. Your answer is recorded next to macOS's own notifications and a frontmost-app poll
  (`~/.ferb/watch/<timestamp>.json`). `--mac`, `--browser`, `--only M4,M9`, `--dry-run`. The owner's three runs on
  2026-10-02: 0 of 25 steps moved anything (`docs/PLATFORM_MACOS.md`, "Owner-verified runs").
- [docs/OWNER_ACCEPTANCE.md](docs/OWNER_ACCEPTANCE.md) lists whole tasks to give FERB through `ferb chat`, what should
  happen, and what you check. Sections B–H have not been run yet.
- `ferb stats [--days N]` reads task outcomes, tool-error reasons and model-run counts straight from the database.

## Command reference

Chat and most admin commands talk to the daemon over `~/.ferb/run/ferb.sock` (owner-only).
`stats` reads the database directly; `watch` runs probes locally. Setup, doctor, uninstall and device commands
also perform local work; `daemon run` starts the service, and `mcp` relays worker calls.

| Command | What it does |
|---|---|
| `ferb chat [--verbose] [--speak]` | Talk to FERB |
| `ferb logs [--tasks] [--trail] [--tools] [--browsers] [--policy] [--actions] [--timers] [--voice]` | Live stream from the daemon's bus, optionally filtered |
| `ferb trace [task] [--full]` | What a task did, step by step (default: the latest) |
| `ferb watch [--mac] [--browser] [--only IDS] [--dry-run]` | The owner-verified background test |
| `ferb stats [--days N]` | Task outcomes, tool errors and model runs from `~/.ferb/ferb.db` (works without the daemon) |
| `ferb voice [on\|off]` | Push-to-talk: watch it live, or turn it on / off |
| `ferb timeline [--since YYYY-MM-DD]` | The journal: chats, task outcomes, daily summaries |
| `ferb stop` / `ferb resume-tasks` | Emergency stop (revoke tokens, block actions, cancel held sends; survives restarts) / lift it |
| `ferb undo [hold_id]` | Cancel a pending send / post / call during its undo window |
| `ferb autonomy [careful\|balanced\|hands-off]` | Show or set the autonomy level |
| `ferb pause` / `ferb resume` | Stop / restart every activity watcher |
| `ferb memory list [--proposed] [--all]`, `show [subject]`, `add [--kind K] [--subject S] [--key KEY] <content>`, `confirm <id>`, `edit <id> <content>`, `retract <id>`, `forget [--keep-sources] [--yes] <id>` | Memory |
| `ferb reminders [list\|cancel] [id]` | Pending reminders and timers |
| `ferb schedules [list\|cancel\|pause\|resume] [id]` | Scheduled and recurring tasks |
| `ferb browsers`, `browsers pair [--browser chrome\|brave] <profile>`, `browsers unpair …` | Browser profiles, pairing and connection status |
| `ferb device [show\|scan]` | This Mac's device profile |
| `ferb ps` / `ferb kill <id>` | Background processes FERB started |
| `ferb decide stats`, `decide enable <question>`, `decide disable <question>` | Jev's answers next to FERB's decisions |
| `ferb setup [--update] [--identity [--profile P]] [--jev] [--codex-login] [--voice]` | Install / update / identity profile / Jev key / FERB's own Codex sign-in / push-to-talk |
| `ferb doctor` | Check the installation, permissions (from inside the daemon), runtimes, browsers, identity, watchers; run the self-test |
| `ferb uninstall [--purge]` | Remove the LaunchAgent and native-host files; `--purge` also deletes `~/.ferb` (asks you to type `delete`) |
| `ferb daemon run` | Run the daemon in the foreground (development) |
| `ferb mcp` | Internal: the stdio MCP shim runtimes use to reach FERB's tools |

## Configuration

`~/.ferb/config.toml` must be owner-only (FERB refuses a config other users can read). Keys FERB reads
(`ferb/config.py`; the daemon re-reads `[policy]` when the CLI changes it, other edits need `ferb setup --update`):

| Section | Key | Default | Meaning |
|---|---|---|---|
| `[runtimes]` | `orchestrator`, `worker` | `"codex"` | Runtime per role: `codex` or `claude` |
| | `claude_model` | SDK default | Model for the Claude runtime |
| `[policy]` | `autonomy` | `"balanced"` | `careful` (confirm every non-navigate action), `balanced`, `hands-off` |
| | `trusted_kinds` | `[]` | Action kinds `careful` mode no longer confirms |
| | `trusted_accounts` | `[]` | Site + profile pairs where you approved an account FERB can't verify (written by FERB) |
| | `pasted_text_is_data` | `true` | Pasted text is data, never instructions |
| `[fs]` | `owner_roots` | `[]` | Your folders FERB may use (added when you say yes to a folder question) |
| | `untrusted_roots` | `["~/Downloads"]` | Folders whose files count as untrusted content |
| `[privacy]` | `exclude_apps` | password managers | Never recorded, captured or acted on |
| | `exclude_domains` | `[]` | Sites never recorded in the trail |
| | `exclude_paths` | `["~/.ssh", "~/Library/Keychains", "~/.ferb"]` | Never recorded, even inside a watched folder |
| `[sense]` | `window_titles` | `true` | Record (redacted) window titles |
| | `watch_folders` | `[]` | Folders whose file events are recorded |
| | `idle_after_s` | `120` | Seconds without input before you count as idle |
| | `trail_retention_days` | `30` | Raw trail rows kept; a day is purged only after its summary exists |
| `[mac]` | `snapshot_image` | `true` | `mac.snapshot` also returns the window image with each control numbered |
| `[voice]` | `push_to_talk`, `mic`, `sounds`, `requirement` | `false`, `"auto"`, `true`, set by setup | Push-to-talk switch, microphone choice (never Bluetooth), start/stop sounds, FERB.app's code signature |
| `[browser]` | `prefer_for_actions` | `["brave", "chrome"]` | Browser order when a site is open in several |
| | `extension_ids` | FERB's extension id | Extension ids the native host accepts |
| `[identity]` | `browser`, `profile` | set by `ferb setup --identity` | FERB's own Chrome profile |
| `[mailbox]` | `address` | set by `ferb setup --identity` | FERB's own email address (Gmail) |
| `[gates]` | `owner_profiles` | `false` | Changes unpaired-profile guidance only; pairing is not gated in code |
| `[jev]` | `enabled` | `[]` | Jev questions allowed to act |
| `[device]` | `id` | generated | This Mac's device id |

## Permissions, privacy, where data lives

macOS permissions go to FERB's dedicated interpreter: Accessibility (read and operate app windows), Screen Recording
(screenshots of one window), Input Monitoring (only if the owner-input watcher can't start without it; used to hear
the push-to-talk key and to step aside while you use the Mac), Microphone and Speech Recognition (FERB.app only),
Automation (asked per app on first use). Full Disk Access is not needed.

The activity trail records metadata only: the frontmost app, redacted window titles, tab URLs and titles from paired
profiles, idle/active transitions, app installs and file events in listed folders, each row marked owner or FERB.
Never keystrokes, clipboard or screen contents; excluded apps, domains, paths and private tabs are dropped before
anything is written. `ferb pause` stops every watcher (and real input) until `ferb resume`. Raw rows live 30 days;
each day first gets a permanent summary. `ferb memory forget <id>` is a true purge of an item, its versions, sources
and index. Nothing is sent anywhere except to the model provider you configured (and Jev, if set up), after
redaction.

| Path | Contents |
|---|---|
| `~/.ferb/ferb.db` | SQLite (WAL): tasks, spans, journal, trail, memory, timers, schedules, ledger, pairings |
| `~/.ferb/config.toml` | Configuration (owner-only) |
| `~/.ferb/workspace/` | FERB's own working folder |
| `~/.ferb/memory/` | Read-only markdown export of active memory |
| `~/.ferb/device/` | The device profile (JSON, local git history) and `apps/<bundle>.json`, the learned per-app profile of which background mechanisms landed |
| `~/.ferb/watch/` | `ferb watch` results and detector logs |
| `~/.ferb/logs/`, `run/` | Daemon logs; the socket and runtime scratch |
| `~/.ferb/python`, `venv`, `node`, `bin` | The dedicated interpreter, FERB's venv, the pinned Codex, `ferb-native-host` |
| `~/.ferb/codex-home/` | FERB's own Codex settings and sign-in (`ferb setup --codex-login`) |
| `~/.ferb/FERB.app` | The push-to-talk app |
| `~/Library/LaunchAgents/com.ferb.daemon.plist`; Chrome/Brave `NativeMessagingHosts/` | The LaunchAgent; the native-host manifest |
| macOS Keychain, service `ferb` | FERB's own secrets and the Jev key |

`ferb uninstall` removes the LaunchAgent and native-host files; `--purge` also deletes `~/.ferb` and offers to delete
the Keychain items. Remove macOS permission entries and the browser extension yourself.

## Troubleshooting

Start with `ferb doctor`; each problem comes with a hint.

| Symptom | Likely cause / fix |
|---|---|
| `ferb chat` can't connect | Daemon not running: `ferb setup --update`; logs in `~/.ferb/logs/` |
| "Unavailable: … paired but not open" | Open that browser profile; the extension reconnects within 30 s |
| Mac control unavailable | Accessibility / Screen Recording not granted to FERB's interpreter |
| A permission was just granted but still shows denied | Screen Recording applies after a restart: `ferb setup --update` |
| "extension 0.6.x … FERB needs 0.9.0" | Reload the unpacked extension in that profile |
| A "started debugging this browser" bar in your own browser window | FERB is working in a tab of that profile; it goes when the tab closes. FERB's own window is minimized, so it shows nothing there |
| An app ignores every background way | The result says what was tried; the worker may retry with `allow_foreground=true`, which moves your screen once |
| Everything is blocked | Emergency stop: `ferb resume-tasks` |
| Nothing is recorded / no real input | Watchers paused: `ferb resume` |
| `ferb doctor` says "installed code differs" | `ferb setup --update` |

## Development

```sh
uv sync --group dev
uv run pytest                 # offline suite; real tests are deselected
uv run pytest -m real         # real models (Codex only), apps and browsers: TextEdit, Calculator, a throwaway Chrome
uv run mypy ferb/core
```

Real tests are locked down (each test daemon keeps only the tools it needs); Claude variants run only with
`-m real_claude`. `tests/test_mac_background_real.py` and `tests/test_browser_lab_real.py` assert the frontmost app
never changes. `FERB_HOME=/tmp/ferb-dev uv run ferb daemon run` runs a second daemon side by side with the installed
one; use the same `FERB_HOME=/tmp/ferb-dev` for its chat and admin commands.
`python -m ferb.act.mac.probe <bundle> [--seed]` probes a running TextEdit, Calculator or Finder for which
background mechanisms work and can seed its learned profile. Codex in development comes from the repo's
`node_modules` (`npm install`).

## Docs map

| File | What it is |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | The design contract. Code follows it; sections are numbered and other docs cite them |
| [CLAUDE.md](CLAUDE.md) | How to pick the repo up cold: where things are, the rules that shape every change, how to work |
| [PROGRESS.md](PROGRESS.md) | Where we are, history of every unit built, what's next |
| [CHANGELOG.md](CHANGELOG.md) | Versions, with the commits each covers |
| [CAPABILITIES.md](CAPABILITIES.md) | What works today, how each item is verified, known limits |
| [CODE_TOUR.md](CODE_TOUR.md) | The flows through the code, by `path:function` |
| [FILE_MAP.md](FILE_MAP.md) | Every folder and file, one line each |
| [docs/REWORK_PLAN.md](docs/REWORK_PLAN.md) | The 2026-10-02 rework: the rule on hardcoding and the phases (platform study, fixes, drivers, one worker, learning) |
| [docs/DESIGN_ONE_WORKER.md](docs/DESIGN_ONE_WORKER.md) | The chat turn, the one worker, the effect-class gate, `owner_ask`, verification, the acceptance set |
| [docs/PLATFORM_MACOS.md](docs/PLATFORM_MACOS.md) | How Mac apps can be driven in the background: Accessibility, events, Spaces, TCC; probe results on this Mac; the owner-verified runs; the per-primitive rung order the driver implements |
| [docs/PLATFORM_BROWSER.md](docs/PLATFORM_BROWSER.md) | Chromium in the background: MV3, `chrome.debugger`, focus emulation, trusted input, windows and tabs; probe results; the extension design |
| [docs/OWNER_ACCEPTANCE.md](docs/OWNER_ACCEPTANCE.md) | Tasks the owner gives FERB and checks with their own eyes (`ferb watch` first, then sections B–H) |

## Picking this repo up as an agent

Read [CLAUDE.md](CLAUDE.md) first: it holds the rules (the owner's eyes are the verifier; hardcode platform facts and
safety floors only; background first; one worker gated at the action; the floors that never move; what experiments
may touch), the working loop, and what's next. Then `PROGRESS.md` for where things stand, and the docs above for the
area you're changing. Never read or reuse `vibed/`.

## Known limitations

- macOS only, Apple silicon, one Mac, one user. The CLI is the only interface.
- Browsers: Chrome and Brave through the unpacked extension (no store listing). Safari and Firefox aren't supported.
  While FERB works in one of your own profiles, that profile's visible windows show a "started debugging" bar.
- Pixel clicks posted to an app in the background reach only windows on the current desktop; the real mouse needs
  `allow_foreground=true`. Electron apps and same-desktop occluded windows haven't been probed by the new driver.
- Owner-requested OTP login uses hidden CLI input (`browser.login_code`); the code is sealed to the selected origin and never sent to the model. Password sign-in and CAPTCHAs still require the owner. Payments retain their approval floor.
- Email from FERB's address is text only. Scheduled tasks run only while the daemon runs (a missed run happens once
  on wake, or is skipped if too late). No monthly schedules.
- Prompt injection is reduced (pasted text as data, exposure, data flow, the effect floor), not eliminated. A
  network-fetching shell command exposes the worker like a web page does, so it loses the shell afterwards.
- Learning by doing (procedure memory: how a task was done, reused next time) is designed but not built.
- Browser snapshots use Chromium Accessibility first, backed by DOM targets. `browser.screenshot` and `browser.point` resolve missing visible controls in the exact task tab; the normal action policy still gates the click.
- Only the Mac and browser drivers have recorded real test runs after the 0.17/0.18 rework; the legacy safety suite was last run for
  real at 0.1.0 and has been re-run offline since.

Browser routing defaults to FERB's own profile. Personal profiles require explicit direction such as "in my Brave"
or "my Work profile"; a site, phone number or saved address is not permission. Disconnected profiles report a
connection error; FERB no longer opens a visible wake tab to force a connection. See
[0.20.1 browser routing notes](docs/BROWSER_DEFAULT_0201.md).
