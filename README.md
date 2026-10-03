# FERB

FERB is a personal agent runtime for macOS. The owner's goal, in their words: "an agent that can control my Mac the
way I can, and do things building on top of that base, and keep learning with doing." You talk to it in a terminal
(`ferb chat`). The orchestrator assigns workers to read and act, resolves their questions, and changes approach
when a worker cannot continue. Setup collects access once. The agents interpret your request and ask about an
unrequested final outcome only after the work is ready for review. “Send Rahul the deck” already authorizes sending;
“get the deck ready for Rahul” calls for a final choice before sending.

- **Version:** 0.22.1 (see [CHANGELOG.md](CHANGELOG.md)). Background click fix in this checkout; installation and full-task acceptance pending. [Pointer evidence](docs/BACKGROUND_POINTER_0221.md).
- **Platform:** macOS only, Apple silicon only (FERB pins the arm64 Codex binary). Developed on macOS 26.
- **Interface:** the CLI, plus push-to-talk. No GUI, menu-bar item or web dashboard. FERB.app (for push-to-talk)
  runs in the background with no window, Dock icon or menu-bar item.

[CAPABILITIES.md](CAPABILITIES.md) lists what works today and how each item is verified. [CODE_TOUR.md](CODE_TOUR.md)
walks the flows through the code. [ARCHITECTURE.md](ARCHITECTURE.md) is the design contract. The docs map is at the
end of this file.

See [the 0.22 control model](docs/ORCHESTRATED_CONTROL_022.md) for current behavior, validation and limits.
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

`ferb setup` installs FERB's dedicated Python/environment, pinned Codex, native host and LaunchAgent; scans apps;
collects Accessibility, Screen Recording, Input Monitoring and Full Disk Access; offers Automation for running
scriptable apps; then runs daemon checks. Grants belong to the interpreter path setup prints. Browser pairing and
account sign-in complete setup. New apps or changed binaries may still require macOS grants.

`ferb setup --update` reinstalls this checkout and restarts the installed daemon. A pre-0.22 update offers permission
collection once, recording `setup.permissions_version=22`. Missing/skipped grants remain visible in doctor.

**Browsers.** Pair each profile FERB may use: `ferb browsers pair [--browser chrome|brave] "<profile name>"`. If the
extension isn't installed there yet, pairing walks you through Load unpacked (Chrome only allows installing an
unpacked extension by hand), waits until the browser has it, then pairs. `ferb browsers` lists profiles and whether
they're connected.
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
  a task. Workers use the installed registry; the orchestrator handles recovery and unresolved questions.
- **Ctrl-C** stops the running task ("stopped by you"); active recovery is cancelled too. Closing chat does
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

1. Each turn searches relevant memory, today's activity log and the installed action registry, alongside durable
   conversation context and current connection facts.
2. The orchestrator assigns a worker (`default`, `codex` or `claude`). Reading and acting share one workflow.
   The worker has the installed registry; `actions.search` helps discover capabilities.
3. If an attempt cannot continue, its report goes back to the orchestrator. The model changes worker or approach,
   retaining artifacts and uncertain-action evidence. There is no fixed task duration or action count.
4. Worker questions go through the orchestrator first. It answers from context when possible; only unresolved
   questions reach you. Result checks can send incomplete work back for repair.
5. FERB reports the result and evidence. Closing chat leaves work running; explicit stop cancels it.

## Execution and privacy

The gateway authenticates and validates tool calls, serializes resources, records execution and redacts results.
It does not classify your wording into app/action permissions, remove tools after web reads, delay sends for undo,
or force repeated payment confirmations. Agents decide authorization from the full request.

FERB uses its own browser by default; your profiles are used when you direct it, including “scaler Gmail.” Correct
tab/process ownership, fresh screenshots, actual OS permissions, sealed secret input, privacy exclusions and stop
remain enforced. Pages and logs are data, not new instructions. Real model judgment needs owner acceptance.

See the [0.22 contract](docs/ORCHESTRATED_CONTROL_022.md) and [architecture](ARCHITECTURE.md).

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
| `ferb undo [hold_id]` | Legacy hold compatibility; new tasks do not create automatic send holds |
| `ferb autonomy [careful\|balanced\|hands-off]` | Legacy setting; does not restore per-action gates |
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
| `[policy]` | `autonomy` | `"balanced"` | Legacy setting; current authorization is agent-directed |
| | `trusted_kinds` | `[]` | Legacy action-policy preference; no effect on current execution |
| | `trusted_accounts` | `[]` | Legacy approved account pairs; not a current execution gate |
| | `pasted_text_is_data` | `true` | Pasted text is data, never instructions |
| `[fs]` | `owner_roots` | `[]` | Legacy folder metadata; ordinary OS access now applies |
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
| `[setup]` | `permissions_version` | set by setup | Permission collection migration version |
| `[jev]` | `enabled` | `[]` | Optional enrichment; not action permission or worker-recovery routing |
| `[device]` | `id` | generated | This Mac's device id |

## Permissions, privacy, where data lives

macOS permissions go to FERB's dedicated interpreter: Accessibility (read and operate app windows), Screen Recording
(screenshots of one window), Input Monitoring (used to hear
the push-to-talk key and to step aside while you use the Mac), Microphone and Speech Recognition (FERB.app only),
Automation for selected running apps and Full Disk Access are offered at setup.

The activity trail records metadata only: the frontmost app, redacted window titles, tab URLs and titles from paired
profiles, idle/active transitions, app installs and file events in listed folders, each row marked owner or FERB.
Never keystrokes, clipboard or screen contents; excluded apps, domains, paths and private tabs are dropped before
anything is written. `ferb pause` stops every watcher (and real input) until `ferb resume`. Raw rows live 30 days;
each day first gets a permanent summary. `ferb memory forget <id>` is a true purge of an item, its versions, sources
and index. Nothing is sent anywhere except to the model provider you configured (and Jev, if set up), after
redaction.

| Path | Contents |
|---|---|
| `~/.ferb/activity/today.jsonl` | Redacted what/where/when activity; resets on the next read/write after the local date changes |
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
| [docs/REWORK_PLAN.md](docs/REWORK_PLAN.md) | The 2026-10-02 rework: historical phases; 0.22 supersedes its control rules |
| [docs/DESIGN_ONE_WORKER.md](docs/DESIGN_ONE_WORKER.md) | Historical 0.18 effect-gate contract; superseded by ORCHESTRATED_CONTROL_022 |
| [docs/PLATFORM_MACOS.md](docs/PLATFORM_MACOS.md) | How Mac apps can be driven in the background: Accessibility, events, Spaces, TCC; probe results on this Mac; the owner-verified runs; the per-primitive rung order the driver implements |
| [docs/PLATFORM_BROWSER.md](docs/PLATFORM_BROWSER.md) | Chromium in the background: MV3, `chrome.debugger`, focus emulation, trusted input, windows and tabs; probe results; the extension design |
| [docs/OWNER_ACCEPTANCE.md](docs/OWNER_ACCEPTANCE.md) | Tasks the owner gives FERB and checks with their own eyes (`ferb watch` first, then sections B–H) |

## Picking this repo up as an agent

Read [CLAUDE.md](CLAUDE.md) first: owner verification, agent-directed decisions, background operation, authorized
experiments and development boundaries. Then read `PROGRESS.md` and [the current contract](docs/ORCHESTRATED_CONTROL_022.md).
This session stays on `astra/version`; no push or installed update without authorization. Never read `vibed/`.

## Known limitations

- macOS on Apple silicon, one user per installation; no cross-device synchronization or graphical frontend.
- Chrome/Brave require the paired extension; native fallback depends on actual app/OS capabilities. A visible
  debugger indicator can remain in a profile's windows. No automatic visible wake tab is opened.
- Native gestures and background delivery need owner verification on each relevant app. Earlier WhatsApp scrolling
  and YouTube last-comment acceptance do not prove every task or gesture works.
- OTP input uses the hidden sealed path; CAPTCHA and unavailable credentials can require owner input.
- FERB mailbox email is text-only; monthly schedules and procedure memory are not implemented.
- Context search is currently lexical. Agent authorization/recovery judgment and the scaler Gmail → coding-session
  workflow need live owner acceptance after installing 0.22. Files and shell run with the macOS user's OS access.
