<p align="center"><img src="assets/logo/ferb-logo.svg" width="128" height="128" alt="FERB logo"></p>

# FERB

FERB is a personal assistant for your Mac. You ask it in plain words, from the notch at the top of your screen, and it
does the task the way a person would: in your apps, your browser, your files and the terminal. It works in the
background while you keep working, without taking your cursor, keyboard focus or Space. It remembers what you tell it
and gets better at work you ask for more than once.

AI models read your request and decide what to do. FERB's own code carries out the actions, keeps the facts and
enforces the limits: what may be touched, what is sent where, and what needs your Yes first.

Status: prerelease (0.27.0.dev1). It runs on macOS 14.2 or later. The Apple silicon build is in daily use; the Intel
build is made the same way but hasn't been tested on an Intel Mac yet.

## Install

1. Download `FERB-AppleSilicon.dmg` (M1 or later) or `FERB-Intel.dmg` from the
   [releases page](https://github.com/Json604/ferb/releases), open it and drag FERB to Applications. Open it from
   Applications, not from the disk image.
2. FERB is signed with its own certificate, not an Apple Developer ID, so macOS blocks the first open. Open System
   Settings → Privacy & Security and click **Open Anyway** next to the message about FERB. macOS remembers the choice.
3. The **Set Up FERB** window walks you through:
   - installing FERB's background service (macOS lists it under Login Items);
   - **Choose your AI**: sign in with ChatGPT or Claude, or paste an API key (see [AI models](#ai-models));
   - **Permissions**: **Allow all** asks for each one in turn (see [Permissions](#permissions));
   - **Browsers**: add the FERB extension from the Chrome Web Store in each browser profile, then pair it;
   - **FERB's own account** (optional): an email account and browser profile that are FERB's, not yours, for sign-ups
     and one-time codes;
   - **Done** opens the notch.

FERB updates itself: about once a day it checks the GitHub releases, verifies the download with FERB's update key and
certificate, and installs it when no task is running. You can turn this off in Settings → General.

To remove FERB, use Settings → Advanced → Uninstall. "Uninstall FERB and delete all its data" also removes FERB's
Keychain items.

## What FERB can do

- **Work in your Mac apps** through macOS Accessibility: read a window, click, type, use menus, in a specific app,
  without moving your cursor or bringing the app forward. If a task really needs the foreground, FERB asks first.
- **Browse** in Chromium browsers (Chrome, Brave, Edge, Chromium and their channels) through the FERB extension. It
  works in background tabs, grouped per task, in your paired profiles or in FERB's own. Downloads go to FERB's
  workspace, never through a Save dialog. A task may download up to 300 files and 32 GB in all (browser and terminal
  together), and FERB always leaves 10 GB free on the disk.
- **Run terminal commands and change files** inside a sandbox. FERB keeps a copy of a file before it changes it, moves
  files to the Trash instead of deleting them, and can take a disk snapshot before bulk work. Settings → Data →
  Restore undoes these.
- **See what you see**: with each message, FERB looks at your front window (a redacted screenshot), so "what does this
  mean?" works. "What FERB saw" under a message (and Settings → Data) shows those screenshots as FERB's AI got them.
- **Hear an app** during a task (a video, a call recording): the sound is transcribed on your Mac and deleted once
  transcribed.
- **Keep going over time**: reminders, schedules (with an end date if you give one), timers, watches ("tell me when
  …") and loops ("keep this tab playing until I say stop").
- **Answer in steps**: each part of your request gets its own row in the notch, posted once it's checked. FERB asks you
  only what, who or which, never how.
- **Remember**: facts you mention, even in passing, kept as a short profile and searched by meaning and keywords on your
  Mac. After a task it notes how the work was done, so the same kind of task goes faster next time. It can import what
  ChatGPT, Claude, Gemini, Claude Code and Codex CLI know about you (the last 3 months).
- **Dictation**: hold Right Command + Option to dictate a message. FERB always replies in text in the chat; it never speaks.
- **Sign in for you**: passwords, one-time codes and card details go through a sealed path; the model never sees them.
  FERB tries simple "verify you are human" checks itself and hands the tab to you when it can't.
- **Report a failure**: a failed task offers **Send to developer**: FERB prepares the task's record (secrets removed),
  you can view it, and **Send** delivers it to FERB's developer. No account or email needed.

## AI models

FERB runs its planner, workers and checker on the AI service you choose, per role, in Settings → AI:

| Runtime | How you connect it |
|---|---|
| Codex (OpenAI, the default) | Sign in with ChatGPT |
| Claude (Claude Agent SDK) | Sign in with your Claude subscription |
| Anthropic API | API key |
| OpenAI API | API key |
| Google Gemini API | API key |

Sign-ins are FERB's own (separate from your personal Codex or Claude Code), and API keys go into your login Keychain.
Whichever model you use, every action goes through FERB's own tools and checks. If a service reports a usage limit or a
lost sign-in, FERB tells you once and offers another signed-in one; it never switches by itself.

## Permissions

FERB needs all of these; setup isn't done until each is on.

| Permission | Why FERB needs it |
|---|---|
| Accessibility | To read and operate other apps' windows and controls in the background. |
| Screen Recording | To take the redacted screenshots it looks at, show the live preview of a task, and hear an app's sound. |
| Input Monitoring | To know when you are typing or using the mouse, so it stays out of your way, and to record what you type per field (never in password fields, secure input or terminals). |
| Full Disk Access | To work with files in your folders and run commands on them. The sandbox and protection lists still apply. |
| Microphone and Speech Recognition (one row in setup) | To turn your voice into text for dictation in the notch, and an app's sound into text during a task, on your Mac only. |
| Notifications | To tell you when something needs you and the notch can't show it. |
| Paste from Other Apps | To read what you copy, for FERB's activity history (the text is kept 7 days). |
| Automation (asked per app, when first needed) | To script an app FERB works with, such as reading a browser's current tab. |

## Privacy and security

**Your data stays on your Mac.** FERB keeps its records in `~/.ferb` (a local database and files). Saved passwords,
security answers and card details are in your login Keychain, never in the database. There is no FERB server.

**What leaves the Mac.** To understand and do a task, FERB sends the task's context to the AI service you chose, under
that service's terms: your messages, the relevant memory, page and app text, and redacted screenshots. Before anything
leaves, code removes secrets, one-time codes and card details; models see only a placeholder. Redaction covers those
classes of secret, not every private detail.

**What FERB records about you, and for how long.** You can change the periods in Settings → Privacy; Settings → About
shows the current table.

| What | Kept |
|---|---|
| Your tasks, their results, your chats with FERB, day summaries | Forever |
| What FERB remembers about you and what it learned | Until you forget it (Memory page) |
| Each task's full trace (every model and tool call) | 30 days after the task ends |
| Operational records (actions, confirmations, plans, timers) | 90 days after their task or timer ends |
| Activity on the Mac (apps, windows, pages, files, notifications) | 30 days, once that day's summary exists |
| Text read from apps on screen | 7 days |
| What you typed and copied (redacted) | 7 days |
| Screenshots, evidence and shell output | 7 days |
| Copies of files before FERB changed them, snapshots | 7 days |

Private (incognito) windows, sites and apps you exclude in Settings → Privacy, password fields and terminals are not
recorded.

**What FERB asks you first.** Code asks, once per task, showing the exact action, only for hard-to-undo things:
- paying money;
- account security changes;
- permanent deletion;
- sending as you to someone new, unless a standing permission you gave covers it;
- a command that turns off certificate checks;
- typing into any terminal (it shows the exact command, every time);
- input on a browser's own pages;
- sending your private data to a site or person you haven't approved, once the task has read outside content
  (a page, an email, a message);
- using your screen in the foreground.

Your answer carries across retries.

**What never needs asking, because FERB can't do it.** FERB can't change its own settings, audit records, protection
rules, sandbox or service, and its commands can't get around its network proxy, which records every site contacted.
Only your own words give instructions; what FERB reads (pages, mail, messages, files) is treated as data, never as
instructions.

**Paused.** Notices about what happens on your Mac and proactive offers need Jev, a paid filtering service, and are
switched off until there is a design without it. FERB still records activity, but it doesn't post notices.

## Current limitations

- Prerelease: many checks on a real Mac are still pending (`kb/live.toml`).
- No Apple Developer ID yet: the first open needs **Open Anyway**. The Intel build is untested on an Intel Mac.
- Commands and the browser can reach any site without asking. After FERB reads a malicious page, a command could send
  one of your files somewhere; it is recorded, not prevented.
- A post, webmail send or payment in a browser is checked when the worker declares it; code can't detect one it doesn't
  declare.
- The browser extension works in Chromium browsers only (no Safari or Firefox).
- Hearing is per app, so a browser is heard whole, and the sound must play for FERB to hear it. FERB can't recognise
  music by its sound.
- About once a month macOS asks again whether FERB may keep capturing the screen. Declining it stops screenshots, the
  front-window picture and hearing until you allow it again.
- A loop checks what it watches every 10 seconds, so a change that reverts within seconds can pass unseen.
- With the Claude runtime, Claude Code adds some text of its own to each request (including your Claude account's email
  address); FERB's trace shows only FERB's part.

## Development

Requirements: macOS, Python 3.12 through [uv](https://docs.astral.sh/uv/), Node/npm, and Xcode Command Line Tools for
the app.

```sh
git clone https://github.com/Json604/ferb.git
cd ferb
uv sync --group dev
nice -n 10 uv run pytest -q        # offline safety suite: no real models, apps or accounts
uv run mypy ferb/core
git diff --check
```

Live tests (`uv run pytest -m real`) use real models and disposable targets on surfaces you control, and need someone
watching; the live checks are in `kb/live.toml`.

- `uv run ferb setup --update` installs the checkout as a development background service (it refuses while FERB.app's
  service is registered). The checkout's `.venv` is separate from the installed `~/.ferb/venv`.
- `scripts/build-app.sh [--arch arm64|x86_64] [OUT_DIR]` builds FERB.app; `scripts/build-dmg.sh` builds both apps and
  disk images. Signing is described at the top of `scripts/signing.sh`.
- The notch app is in `ui/mac/FERBNotch` (`swift test` after `source scripts/swiftpm-anonymous.sh`).
- `ui/mac/FERBNotch/playground.sh` opens the intro playground: the real first-run intro with everything behind it faked
  (see `ui/mac/FERBNotch/PLAYGROUND.md`).
- To see what happened in a task: `ferb trace TASK --full` (or `--json`), `ferb stats`, `ferb doctor`, `ferb logs`. The
  `ferb` program is development plumbing, not a user interface.

### Releasing

1. If the browser extension changed: raise `version` in `extensions/chromium/manifest.json` (and
   `MIN_EXTENSION_VERSION` in `ferb/act/browser/ext_protocol.py` when FERB needs the new one), build the store zip with
   `uv run ferb browsers extension package --out build/store`, upload it with the listing text in
   `extensions/store/listing.md`, and wait until the store publishes it.
2. Set the release version in `ferb/__init__.py`, `pyproject.toml` and the `ferb` entry of `uv.lock` (no `.dev`;
   newer than the last release tag), with an owner decision for the release.
3. `SIGN_IDENTITY=release scripts/build-app.sh --arch arm64 build/next`, then
   `SIGN_IDENTITY=release scripts/build-dmg.sh build/next/FERB.app build/FERB-AppleSilicon.dmg` (the window picks the
   before/after Developer ID text from the signature).
4. `scripts/release-appcast.sh vX.Y.Z build/FERB-AppleSilicon.dmg arm64`. It first runs `tools/kb/run gate release`
   over the whole tree: every live check touched since the last tag needs the owner's pass or waiver (`kb/live.toml`),
   and every commit must have run the hooks (exceptions with a reason go in `kb/gate-allow.toml`). Then
   `scripts/release_checks.py` refuses a development or unbumped version, a tag that doesn't match, and an extension
   the Chrome Web Store doesn't serve yet. Signing the DMG with the Sparkle key makes macOS ask the owner for their
   login password (Keychain); it writes `build/appcast-arm64.xml`.
5. With the owner's go: `gh release create vX.Y.Z build/FERB-AppleSilicon.dmg build/appcast-arm64.xml --target <full
   commit sha>`. Installed copies update from `releases/latest/download/appcast-arm64.xml`, and the site's download
   is `releases/latest/download/FERB-AppleSilicon.dmg` (REL-414).
6. The site (`site/`, Vercel project `ferb-site`, https://ferb.kartikey.xyz) deploys from that folder:
   `cd site && npx vercel@<pinned version> deploy --prod`, on the owner's go.

## Where things are written down

- `kb/`: the owner's decisions and agents' rulings (`kb/decisions/`), the work list (`kb/tasks.toml`), the live checks
  (`kb/live.toml`) and terms, validated by `tools/kb/kb.py`; overview in `kb/MAP.md`, one file's context with
  `tools/kb/run kb show <file>`. Git hooks run the checks in `kb/gate.toml` on every commit and push.
- `AGENTS.md`: how to work on FERB, for every agent (`CLAUDE.md` imports it).
- `THIRD_PARTY_NOTICES.md`: attribution for bundled third-party code.

## License

Apache-2.0 (`LICENSE`).
