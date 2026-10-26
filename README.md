# FERB

FERB is a personal assistant for macOS. Talk to it from its notch app (FERB.app); `ferb chat` is the same
conversation in a terminal, for development. Its orchestrator assigns workers, answers their questions from context
and changes approach when needed. Agents interpret your request and decide what actions it
authorizes. They prepare a concrete hard-to-undo action, confirm its identity, and carry that confirmation across retries.

Checkout version **0.27.0.dev1**, on `main` ([plan](docs/0.27_PLAN.md)). F5–F10 passed combined checkout
validation; owner live acceptance remains pending. The launch-week fixes (L1–L28, 2026-10-13/14), learning by
intent (L29, 2026-10-21) and the 2026-10-23 performance plan (with FERB's resource log, L30) are integrated here and
not yet installed or live-tested. The running installation is separate from this checkout. Earlier F0–F4 owner
verdicts remain in the acceptance record.
The [changelog](CHANGELOG.md) records changes; [architecture](ARCHITECTURE.md) explains the implementation and limits.

## Setup

Requires macOS 14.2 or later, on Apple silicon (`FERB-AppleSilicon.dmg`) or an Intel Mac (`FERB-Intel.dmg`; built the same way but not yet tested on an Intel Mac), and a signed-in Codex or Claude account. Install the app from the
disk image, or from source.

### FERB.app (disk image)

1. Open `FERB-AppleSilicon.dmg` (M1 or later) or `FERB-Intel.dmg`; the disk image's first-open note repeats step 2. Drag FERB to Applications. Open it from Applications, not from the disk image.
2. FERB is signed with its own certificate, not an Apple Developer ID, so macOS stops it the first time. Once:
   open System Settings → Privacy & Security, scroll to Security and click **Open Anyway** next to the message about
   FERB. The button is there for about an hour after the blocked attempt; if it's gone, open FERB again. macOS asks
   for your login password, then FERB opens and macOS remembers the choice
   ([Apple](https://support.apple.com/guide/mac-help/mh40616/mac)).
3. FERB opens its **Set Up FERB** window. **Set Up FERB** installs the background service from inside the app (a
   minute; a copy opened from the disk image is refused with how to fix it). Then: **Choose your AI** (Sign in with
   ChatGPT, which opens your browser and keeps the login in FERB's own Codex home `~/.ferb/codex-home`; Sign in with
   Claude, which opens your browser and keeps the login in FERB's own Claude home `~/.ferb/claude-home` (Claude Code
   keeps the credential in your Keychain); or an API key, which goes straight into your Keychain), **Permissions**
   (**Allow all** asks for each in turn: Accessibility, Screen Recording, Full Disk Access, Input Monitoring, Paste
   from Other Apps, Notifications, Microphone and Hearing app sound; each row's **Grant** opens the right System
   Settings pane and ticks itself when it's on, and FERB restarts its background service by itself when you come back
   from a switch that needs it; a row FERB's pane doesn't list offers **Show FERB to drag in**; Full Disk Access shows
   FERB in Finder to drag into the list), **Browsers** (**Add to Chrome** opens FERB's Web Store page in that profile;
   turn it on there, then **Pair all**, or **Pair** one profile; later, Settings → Browsers or a task asks), **Getting
   to know you** (save, edit or skip what FERB found about the apps you use) and **Done**, which opens the notch.
4. Closing the window doesn't stop a service install; **Set Up FERB…** in FERB's menu bar shows the window again. An
   installation set up before this window existed is asked once, at launch, whether FERB may send notifications.
   `ferb setup --codex-login` and `ferb setup --claude-login` sign in from a terminal (the `ferb` command).

In Privacy & Security, FERB appears as **FERB**: the background service (Accessibility, Screen Recording, Full Disk
Access, Automation) is FERB's agent app inside FERB.app, and dictation's Microphone and Speech Recognition belong to
FERB.app itself. FERB's voice helper inside FERB.app appears as **FERB Voice** until it merges into FERB.app: setup
asks once for its Speech Recognition, so FERB can turn an app's sound into text during a task (on this Mac only), and
push-to-talk, when you turn it on, adds its Microphone.

After setup, FERB.app opens at login by itself (Settings → General → **Open FERB at login**, on by default once; turn
it off there or in System Settings → General → Login Items). FERB uses macOS's own login item; if macOS refuses it for
FERB's self-made signature, FERB adds `~/Library/LaunchAgents/com.ferb.FERB.login.plist`, which opens FERB in the
background at login, and Settings says so. The background service starts at login either way.

FERB uses the same `~/.ferb` home as a source install, so an existing installation keeps its data; the app asks
before it replaces one, from its menu too. After you replace FERB.app with a newer build, the next launch updates the
background service quietly (and so does a launch that finds it not running); the setup window opens only if that
fails or a required permission is missing. Every release is signed with
the same FERB certificate, so an update keeps FERB's macOS permissions and its secret helper's Keychain access (the
two-build check on a second macOS user is pending).

FERB updates itself. About once a day it asks GitHub's public release list for a newer version (nothing about you is
sent), downloads it, checks it with FERB's own update key and certificate, and installs it when no task is running;
the background service restarts, a loop you asked for carries on, and the notch says "Updated to <version>". If you
quit FERB while a task runs and an update waits, FERB asks first (**Keep FERB Open** or **Quit and Install**).
Settings → General → **Install updates automatically** turns this off; FERB then shows "Update available — Install"
in its menu and in Settings.

The app is the product; the `ferb` command is for development. Setup links it at `~/.local/bin/ferb` and never edits
your shell profile; to use it, add `export PATH="$HOME/.local/bin:$PATH"` to `~/.zprofile`. `scripts/build-dmg.sh` builds both apps and both disk images;
`scripts/build-app.sh [--arch arm64|x86_64] [OUT_DIR]` builds one app (an Intel app builds on Apple silicon under Rosetta) and prints its size by part; `SIGN_IDENTITY=release` signs with FERB's release certificate (creating and backing it up: the top of `scripts/signing.sh`); a disk image holds only a release-signed or Developer ID app (`FERB_DEV_DMG=1` makes a test image); set `SIGN_IDENTITY` to a Developer ID (its name or
SHA-1 hash) and the notary credentials described in `scripts/signing.sh` to sign, notarize and staple the same bundle
(`scripts/build-app.sh --sign-only build/app-arm64/FERB.app` re-signs a built app without rebuilding it).

### From source

Requires Python 3.12 through `uv` and Node/npm. A `git clone` install never involves Gatekeeper.
The pinned runtime versions/model are in `package.json` and `ferb/runtimes/codex_exec.py`.

```sh
uv sync --group dev
uv run ferb setup
```

Setup installs a dedicated interpreter, environment, runtime and LaunchAgent under `~/.ferb`. It collects macOS
permissions and reports missing grants. `ferb setup --update` installs the current checkout and restarts the daemon;
run it when existing work can be interrupted. The development environment and installed daemon are separate.
When FERB's database still holds run cards from a build before the 2026-10-15 card fix (`e0e344b`), the next update
also cleans it once, with the daemon stopped. It removes the source rows that card builder copied (gigabytes after a
week of use), then compacts the file. That takes a few minutes (about two on a store of 9 GB). Setup says "Cleaning
FERB's database…", how far it is at each tenth, then how much it removed and the size before and after. Ctrl-C or
quitting FERB.app stops it safely, and the next update finishes it. It waits for the stopped daemon's process to
exit (up to 10 s) and never runs while a FERB daemon still answers; setup then says why, and the next update does
it. Compacting needs free disk at least the
database's size; without it, the rows are still removed and a later update compacts. `ferb doctor` shows the
database's size and what is left: the clean-out, or only the compaction.
Updates check existing grants in the running daemon and acknowledge them even when an older setup version was
recorded. Only missing or unknown grants are reported as deferred; updating does not open permission dialogs.

### Browser extension

FERB's browser hands are its extension, **0.15.11** (protocol 17; the daemon accepts 0.15.11 or later, the first that
reports a terminal in a page, D31); the daemon refuses an older one with a reload notice. `ferb setup` (and
`--update`) handles it for every Chromium browser it finds (Chrome and its Beta, Dev and Canary channels, Chromium,
Brave, Edge, Vivaldi, Arc, Opera):
- it registers FERB's native host in each browser's folder;
- it asks each browser to install it from FERB's Chrome Web Store listing (documented for
  Chrome and Chromium; written but unconfirmed for Brave and Edge; not possible for Vivaldi, Arc and Opera). The
  browser installs it at its next start and asks you to turn it on in each profile; setup says which browsers to
  reopen. Setup never restarts a browser, approves its prompt or edits its preferences;
- it reports, per profile, whether the extension is installed, enabled and paired.

The listing went live on 2026-10-20 (id `plbfkifcmgkgnlfgikdeaaglgncdhkma`, built in) and carried 0.15.6 from
2026-10-22; 0.15.11, which this build needs, was uploaded on 2026-10-25. Until a profile's store copy updates to it, the
daemon refuses that copy with the reload notice (the browser updates store extensions on its own; Load unpacked works at
once). 0.15.7 (typing and clicking on a page another extension put a frame into) and
0.15.8 (each video's own id and each tab's sound, which a loop watches), 0.15.9 (where the pointer lands) and 0.15.10 (a
first snapshot already says whether a video plays) aren't on the listing until 0.15.10 is uploaded;
until then a profile gets them only by Load unpacked. With the store copy those facts are absent: a loop's probe on
one is refused (its field is missing), and the loop runs on its 30-minute backup check until a round picks another. A profile that already has
the unpacked copy is offered the store copy too; setup's report and `ferb browsers pair` use the copy that is turned
on (the unpacked one when both are), so keep one turned on.
In browsers without the store request, setup prints the Load-unpacked steps: prepare the managed folder with `ferb browsers extension install` (`ferb browsers
extension path` prints it), enable Developer mode in the browser's extensions page and Load unpacked from
`~/.ferb/extensions/chromium`, once per profile. `[browser] store_extension_id = ""` in `~/.ferb/config.toml` turns
the store request off. `ferb browsers extension package --out DIR` builds the store zip. After pulling a new bundle, run `ferb setup --update` to republish it; a managed install reloads
it when idle.

Pairing happens on your click or your Yes, never by itself. Once FERB is turned on in a profile, **Pair all** (setup's
Browsers step, or Settings → Browsers) pairs every such profile at once, and **Pair** pairs one: a blank FERB page
opens in that profile and closes itself. Later, "pair my Work profile" in the chat pairs it with no question; a task
that needs an unpaired profile you didn't name asks you once ("pair your Chrome profile “Work” with FERB…"). FERB's own profile is paired by `ferb setup --identity`.

In a profile that holds both copies, switching from the paired copy to the other also needs a pairing. From a
terminal:

```sh
ferb browsers pair --browser chrome "scaler"
ferb browsers
```

For an existing repository-loaded extension, Load unpacked from the managed folder without removing the existing
extension; its pinned ID preserves pairing. Managed installations reload newer local bundles when idle.
“Paired” records a known installation; “connected” means that installation currently has a live native connection.
A closed browser is disconnected. An old version or lost pairing has a separate diagnostic.

FERB uses its own browser by default. Personal profiles are used when you request them, including “scaler Gmail”
or “my current Brave tab.” Set up FERB's own account with `ferb setup --identity [--profile NAME]`.

A download in a tab FERB is driving for a task never opens a Save dialog: FERB takes the file into
`~/.ferb/workspace/downloads/<id>-<name>` and the task gets facts (path, size, SHA-256, source URL), labeled outside
content. Limits (your decision of 2026-10-09): `browser.download_max_mb` (512) per file and, per task (one request,
all its attempts), `browser.download_task_max_files` (50), `browser.download_task_max_mb` (4096) and
`browser.task_max_tabs` (50 new tabs FERB opens from its pages); one download at a time per tab (the next waits up to
20 s), each within 10 minutes. A download that would leave less than `browser.download_min_free_gb` (10 GB, as macOS shows it; space macOS can
purge doesn't count) free on the disk is refused: by its declared size before
any byte moves (with no declared size, when the disk is already under the floor), and again before each part is
written; the reason says how much is free. Refused ones, and any FERB couldn't check against these limits (for example
while `config.toml` is half-edited), are reported, still without a dialog. A page that starts more saves at once than
FERB holds (20 in a tab, 100 in all) has the rest dropped (neither taken nor given to the browser), and the task is
told. The worker sees the facts in its open/navigate result and with `browser.downloads`. The files are FERB's working
copies and stay until you delete them; only when the folder passes `browser.download_total_mb` (20480) do the oldest
move to the Trash, so ask FERB to copy a file you want to keep where you want it. Where your `config.toml` still holds
the values FERB wrote in F4 (100, 20, 500, 2048 and 20 tabs), the daemon replaces them with these once, at its first
start with this version, and removes `retention.download_days`; a value you set yourself stays, and `ferb timeline`
shows each change (`config.upgraded`).
While a task drives one of your tabs, downloads in that tab and in tabs opened from it go to FERB, not to your
Downloads; tabs opened from your tab are released to you, never closed, when the task ends. Your downloads in other
tabs, and in your tab once the task lets go of it, are never touched.
FERB adopts only tabs it opened itself and new popups that the browser confirms a task tab
opened (through FERB's own page hook); a tab that already existed is never claimed, and a new tab that your own page
opens is never touched. The only new tabs FERB looks at are ones at its own popup address, `about:blank#ferb-<token>`:
it attaches long enough to ask the browser who opened the tab and lets go at once unless it was a task tab. What can
still reach the browser itself (it may ask where to save): a download held when you press Cancel on the "started
debugging" bar or when the extension's service worker crashes; a new tab FERB didn't open or claim (a popup opened
with `noopener`, a named or empty target, a script's click on a link, a form posted to a new tab, a link in a frame
FERB's script isn't in); a popup FERB took longer than 1.5 s to adopt; a type the browser shows but your profile is set
to download (such as "download PDFs"); a save a page makes before FERB's script is in it; pages' own save pickers
(`showSaveFilePicker`); while the daemon is down or restarting, a download in a tab a task was driving (after a
restart that tab stays set up for FERB until the extension reconnects) and a link to a new tab from it, which your
browser opens itself after about a second (also when the daemon is just slow to answer); in your own profile, a held download FERB doesn't start taking within 30 s;
and, when a task lets go of your tab, downloads from it FERB hasn't started taking yet. The task is told when FERB saw
it. A page can tell FERB is driving it and can opt itself out of
capture; its downloads then behave as usual. A popup a task page opens shows `about:blank#ferb-<token>` for a moment
before the page loads it.

A file FERB uploads through a browser is FERB's own copy: FERB copies it to
`~/.ferb/workspace/uploads/<task>/<random>/<its name>` (an APFS clone, instant on the same disk, else a byte copy;
under `uploads/outside/` when the original is outside content, so the copy keeps that label) and gives the browser the
copy, so the browser reads only FERB's workspace, never your Desktop, Documents or Downloads folder, and macOS has no
reason to ask you whether it may. The site sees the original name and the original is never changed. The copy goes
when the task ends; one used in your own tab, or in a tab FERB kept open for you, stays until that tab closes (the
browser reads the file again when the form is submitted) or until FERB next starts. So after `ferb setup --update`, a
form still open in such a tab no longer has its file to send.

Optional: `ferb setup --voice` builds the background microphone app. Hold Right Option to talk; release to send.
Push-to-talk recognizes speech on this Mac only: where on-device recognition isn't available it says so and doesn't
listen.
`ferb chat --speak` speaks terminal replies. Pending questions accept free text, including corrections.
`ferb setup --jev` configures optional memory/activity enrichment; it does not decide task permissions.

F7 development adds passive app/Accessibility events, redacted typed fields and clipboard text (seven-day content
retention), notification observation under Full Disk Access, and installed-software facts. Typed text is captured per
field again since the launch week; it is never captured from password fields (shown passwords included), while
Secure Input is on, or in a terminal (terminal apps and terminals inside other apps, e.g. VS Code's). Notification
triage stays in shadow until measured with you; now-playing and two-app Accessibility acceptance are still pending.
See the
[F7 checklist](docs/OWNER_ACCEPTANCE.md#027-f7-perception--pending-owner-acceptance) before treating these as live-proven.

## Everyday use

The app is the product: the notch app's source is in `ui/` (`ui/mac/FERBNotch`; a development build with `./build.sh`
there, the packaged app with `scripts/build-app.sh`). It uses the same local daemon socket as the CLI. See the
[RPC contract](docs/RPC.md) for its events and reconnect behavior. In its chat:
- a short acknowledgement ("Got it.", "On it.") appears about 0.75 s after you send, as a small bubble on the plan
  line that follows when FERB has a plan. While the task works, only the "N active tasks" row shows it's running; the
  steps are in View tasks › Inspect. If FERB loses its AI service mid-task, a line under the plan says so at once
  ("Your Mac seems to be offline: I'll carry on when it's back.") and again when it reconnects, while the task carries
  on (`ferb chat` prints both and the steps);
- text in the chat can be selected by dragging and copied with Command-C, even while FERB isn't the active app;
- an answer to an earlier message that arrives after a newer one appears at the bottom, quoting your question, and
  the earlier message says "Answered below";
- FERB's questions appear inline and you can keep typing; reply in your own words ("yes", "the payment one"), or use
  the question's own buttons; a hidden field for a password or code takes focus only when you click it;
- notices (something FERB noticed on your Mac) appear as FERB messages with a small "noticed" tag; outside text they
  repeat is shown as a quote with where it came from;
- a verified answer shows a small check mark;
- FERB steps one layer back, but stays on screen, while its file picker, another app's menu over the notch, or (pinned)
  an app you bring forward under the chat needs the space: it sits just behind that window or under that menu, so a
  screenshot or a clipboard app still shows it. Click it, hover the notch or use the shortcut to bring it to the front;
  clicking the other app sends it back again.

FERB.app posts FERB's notifications itself, with FERB's name and icon: a question while the notch is closed, a reminder,
an alert from something FERB watches, a task's answer that arrives while the notch is closed, and a loop pausing or
backing off. Clicking one opens the notch at that question or task. Focus modes apply. Each kind can be switched off
(config.toml `[banners]`, Settings → Notifications). Until you allow notifications for FERB (in Set Up FERB…'s
Permissions step, or System Settings → Notifications → FERB), and whenever FERB.app isn't running, FERB posts them
through macOS's script notifications (shown as Script Editor), as before. The menu bar icon shows FERB's state: ready,
working, needs you, paused, or "FERB isn't running — Restart"; its menu has Open FERB, Pause FERB (nothing is recorded
until Resume), Settings… and Quit.

FERB's pointer (L33): when FERB is about to act on something in a window you can see, a small arrow glides to the spot
and fades after about a second and a half. It is only a picture: it is click-through, never takes focus and never moves
your own mouse. There is none for a window you can't see (minimized, another Space, or behind another window at that
spot), none where FERB can't know the exact spot (a page's pinned header, an inner scrolling list, a zoomed page), and
none for an action FERB would refuse. FERB opens its own tabs in the background, so web work shows a pointer only in a
tab your window shows, for example when you ask FERB to work "on this page". Each task gets one pointer and keeps its
shade across its steps; several tasks at once get one each. With Reduce Motion on, it fades instead of gliding. Turn it
off with the settings switch "Show FERB's pointer". Pointing at web pages needs extension 0.15.9; with an older one the
pointer shows for native apps only. The pointer shows where FERB aims from its last look; a page's own layout can
mislead it, nothing in FERB reads it to decide anything, and the trace records what FERB actually touched.

The app's history keeps the message as FERB stored it (secret values replaced), never the raw text, and never your
unsent draft. Native dictation uses the microphone button or right Command + Option and places the transcript in the
composer for review. The separate voice app is optional. Live microphone/device-switch acceptance remains pending.

```sh
ferb chat                         # resumes the last conversation
ferb chat --new                    # separate conversation
ferb chat --resume ID
ferb setup --persona               # review/refresh your local app-use profile; no reinstall or permission dialogs
ferb trace                        # latest task: the story, about 10-20 lines
ferb trace TASK --steps            # one line per tool call
ferb trace TASK --full             # complete recorded inputs, results and decisions
ferb trace TASK --json             # machine-readable trace
ferb logs --tools                  # one live step line per tool call while a task runs
ferb stats                         # outcomes, errors, model runs; FERB's own CPU and memory per day
ferb doctor
```

FERB can read and operate native apps, browse through its extension, run shell commands and coding sessions,
work with files, send email from its own account, remember useful context, and create reminders, scheduled tasks
and watches ("tell me when …").
Each new action task has explicit success conditions. Result inspection uses recorded evidence and fresh reads;
incomplete coverage goes back to the orchestrator. A delivered click or launched process does not prove completion.
The final check is read-only, enforced by code: it can look but not click, type, send or write.
FERB's instructions to its agents say that work you hand to a named agent or person (a Codex Astra session, say, but
not an app FERB uses as a tool, such as WhatsApp) is done by that one: FERB starts it, helps it and checks its result,
and asks you before doing that work itself. Each quality you ask of the result is its own condition, the answer says
who did each part, a lasting choice the request doesn't need (such as which submission counts as final) is asked
about first, and everything FERB tells you says "you" and "your".

Everyday replies default to a short, natural answer: the result first, facts you need to use it, and material gaps
or a next step when needed. Detailed evidence stays in the report/trace. Ask for detail when you want it; requested
documents and substantive answers retain their full depth. There is no word cap or automatic answer truncation.
When a task needs your answer, its question appears before the remaining-work details.

Conversational replies appear progressively as a provisional draft in chat and the native app. The final reply
replaces that draft; a failed or disconnected stream is marked interrupted. You can keep typing in terminal chat
while the draft grows. Action confirmations and task results still wait for execution and verification. Secret
redaction applies across fragments, and only the final reply is spoken by `--speak`.

On first opening chat or the app, missing setup discovery runs in the background. FERB reads local app-use metadata
and shows the proposed profile with its evidence. Save it, edit it in your own words, or skip; nothing becomes a
personal preference without your confirmation. Updates also offer the missing review. Confirmed preferences shape
later answers; installed apps alone do not establish your occupation or interests.

A task ends in one of three results, each its own status:
- **done**: every condition was checked and confirmed. `ferb trace` heads the story `done ✓ verified`.
- **done, but not confirmed** (`done_unverified`): the work finished and the check found nothing wrong, but couldn't
  confirm everything from the background. The answer starts "Done, but not confirmed:" and lists each unconfirmed
  condition as "— unverified: <the real reason>", often with "To check: …" saying how you can check it yourself.
  `ferb trace` shows `done (not confirmed)`. A task with nothing to check against says "Done, but there was nothing to
  check it against."
- **failed**: the work didn't finish, a check found a condition false, or FERB judged that it failed. The answer gives
  the real reason; `ferb trace` shows `failed ✗`.

Every answer is shown in full; there is no separate short summary. For an unfinished task the answer starts with
FERB's status line, then FERB's question if it needs your answer, what the check couldn't confirm and why, why work
stopped, what was confirmed and the files. There is no "Verified." text prefix (the app shows a check mark). When the
task reached any website, the answer ends with "Sites contacted: …", built by code from FERB's records. A correction
you send after an answer ("too LinkedIn-ish") re-plans the same task with the new goal.

"Unverified" appears only beside a condition a check ran on; the status itself reads "not confirmed". If the work
stopped before any check, the answer says "I couldn't finish this task." and lists what is "Still to do". `ferb stats`
counts the three apart ("done N (x%), not confirmed N (y%), failed N (z%)"), `ferb baseline` records them as `done`,
`unverified` and `failed` with their percentages (`unverified` is the not-confirmed count), and `ferb chat --verbose`
shows `done ✓`, `done — not confirmed` or `failed ✗` before the answer.

Every check also judges FERB's own report against the records: what was done, who did each part (FERB itself, or the
agent, app or person you named) and with what. A report the records contradict fails the task, and the answer says
"What I reported didn't match the records: …"; when the check can't settle it, the task is at most not confirmed
("I couldn't confirm my report matches the records: …"). A confirmed answer is the report the check judged: the review
after cleanup can lower the status, never rewrite that answer.

Within a task, code refuses to repeat an action whose outcome is unknown, such as a send that was cut off, or a press
the app didn't answer in time (it may still have acted). The worker looks for evidence that it happened; if that
doesn't settle it, you get a question that starts "FERB: an earlier attempt at <tool> <target> may already have
happened (outcome unknown). Trying again could make it happen twice." (after several: "FERB: 2 earlier attempts at …
may already have happened (outcomes unknown). Trying again could make it happen up to 3 times."), with the call that
had the unknown outcome, then "What the worker found: …", and offers Yes or No. Your Yes allows exactly one more
attempt. A later request that continues the task sees the uncertain action and is told to check before retrying.

Before FERB overwrites or edits one of your files with its file tools, it keeps a copy (an APFS clone) for
`retention.recovery_days` (7 days). Items its file tools delete go to the Trash, and FERB remembers where each came from
while it is still there. `ferb restore` lists what it can put back, newest first, with an id; `ferb restore ID` puts one
back. A copy goes back in place only if the file is gone or unchanged since FERB wrote it; if you changed it since, your
version stays and the earlier one is saved beside it as "<name> (before FERB)<ext>". A trashed item moves back without
replacing anything ("<name> (restored)<ext>" beside whatever is there now). FERB moves items into and out of the Trash
only between folders it holds open: if what landed in the Trash isn't what FERB checked, it stays there and
`ferb restore` says so instead of moving it (look at it in the Trash and move it yourself if it's yours). Finder's
"Put Back" isn't offered for items FERB trashed; use `ferb restore`. Before bulk work an agent may take a local
disk snapshot (at most one per 15 minutes; FERB deletes its own after 7 days). FERB can't open a snapshot itself (that
needs admin rights); you can, from macOS Recovery or Time Machine. File changes FERB makes through shell commands or
AppleScript get no per-file copy.

Closing chat leaves tasks running. Ctrl-C clears the current line; a second Ctrl-C on an empty line closes chat.
Escape on an empty line stops the most recent task; `/stop` stops a named task or all tasks. `ferb stop` stops all tasks until
`ferb resume-tasks`. `ferb pause` / `ferb resume` pause/resume activity sensing, tasks' desktop observations
included.

Task-created tabs normally close after completion. When a page should remain useful—such as a class you asked FERB
to join—the worker uses the browser's keep-open operation. Cleanup releases FERB's debugger without closing the page.
Borrowed owner tabs stay open. Cleanup results and errors are recorded so the final report reflects what survived.

| Command | Purpose |
|---|---|
| `ferb logs`, `ferb timeline` | Live events and recorded task history |
| `ferb ps`, `ferb kill ID` | Managed background processes |
| `ferb reminders`, `ferb schedules` | List and manage timers or scheduled tasks |
| `ferb memory list`, `show`, `add`, `edit`, `forget` | Inspect and manage memory |
| `ferb device show`, `ferb device scan` | Device capabilities |
| `ferb watch --dry-run` | Preview owner-supervised interaction checks |
| `ferb voice on`, `ferb voice off` | Toggle push-to-talk |
| `ferb decide stats` | Inspect optional enrichment decisions |
| `ferb verdict TASK pass\|fail [--note TEXT]` | Record your acceptance verdict on a task; `ferb trace --json` shows it |
| `ferb restore`, `ferb restore ID` | What FERB can put back (files it overwrote or edited, items it moved to the Trash, disk snapshots), and put one back |
| `ferb runtimes` | The model runtimes (Codex, Claude, Anthropic, OpenAI, Google), what each can do, which are usable, and what each role uses |
| `ferb runtimes use ROLE RUNTIME [--model M]` | Make `orchestrator`, `worker` or `checker` use a runtime from its next call (`[runtimes]` and `[models]` in `config.toml`; the checker follows the worker unless set) |
| `ferb setup --key PROVIDER` | Store an Anthropic, OpenAI or Google API key in the Keychain (`security` asks for it) and make one tiny test call |
| `ferb baseline` | Save a read-only metrics snapshot to `~/.ferb/baselines/` for comparing milestones |
| `ferb regress [SET]` | Run a regression task set (default: the built-in smoke set) and print the verdict lines for you |

Codex runs in FERB's own home and needs `ferb setup --codex-login` (there is no fallback to `~/.codex`). `ferb setup`
offers it when it's missing, FERB.app's menu has **Sign in to Codex…**, and `ferb doctor` fails until it's done;
without it the chat says how to sign in, and `ferb runtimes` shows Codex as not usable. Account-check errors distinguish
confirmed login failures from an unavailable account check; repeating a request cannot repair an invalid login.
Refreshed credentials are shared after startup checks, before model work begins. `ferb runtimes use` still saves the
choice for a runtime that is not usable yet and warns that the role will fail until it is.

The Codex and Claude runtimes use your subscriptions. The Anthropic, OpenAI and Google API runtimes bill per token
against the key you add; `ferb runtimes` labels each runtime's billing. `[runtimes] codex_planner_cache = true` in
`config.toml` lets the Codex planner reuse its cached context (off by default until its injection eval passes; see
[owner acceptance](docs/OWNER_ACCEPTANCE.md), "Planner prompt caching").

Use `ferb COMMAND --help` for options.

### Delegated conversations (F9 implementation; live acceptance pending)

Ask FERB to research opportunities and continue selected conversations. Research produces concrete contact routes;
FERB shows the exact account, app/site, thread, contacts, topic and hand-over condition for approval, including
platform automation terms. Proposed standing sends are shown separately. Replies stay within that task, with
ordinary confirmation, private-data and unknown-outcome checks. A checked turn sleeps while waiting for a matching
reply and resumes across daemon restarts. The owner can take over in chat or with:

```sh
ferb conversations list
ferb conversations show CONVERSATION_ID
ferb conversations pause CONVERSATION_ID
ferb conversations takeover CONVERSATION_ID
ferb conversations resume CONVERSATION_ID
ferb notifications rules
ferb notifications unmute RULE_ID
ferb notifications label EVENT_ID --expected yes --note "This was useful"
```

Owner typing in the approved app/window pauses further input. If a source lacks a window ID, the pause covers the
app; an already-sent input cannot be recalled. The gateway pins identity and target app/site, checks concrete
account/recipient fields when supplied, and blocks untargeted input/shell in conversation turns. Generic GUI controls
still require fresh account/contact reads and a checker: their metadata alone cannot prove the selected account.
Incoming text never changes the approved task. A lead is handed over with evidence and action receipts; an unknown
send stays unknown until observed or its retry is explicitly approved.

Ordinary activity can produce a sourced owner notice; it cannot create a task. Jev is an optional third-party triage
service run by typesafe.ai (`api.typesafe.ai`; keys at https://console.typesafe.ai/keys): with a Jev key (`ferb setup --jev`), FERB sends it batches of redacted
activity (app and window names, tab addresses and titles, typed and copied text, notifications, apps installed or
removed, file paths in watched folders); without one, nothing goes there. When Jev flags a moment, FERB considers it once per app, with your relevant memory, the recent chat,
running tasks and the
notices already told, and posts what happened and what it means in chat. A macOS banner is used only when no FERB
client is showing notices. Posting a notice counts as telling it, so it isn't repeated. Notification triage remains in
shadow pending measured owner acceptance. Chat can save an owner-quoted app/chat/topic mute; `ferb notifications`
shows rules and labelled measurements.

### The FERB window and Settings (L34)

The notch stays the quick way in. For detail, choose **Open FERB Window** in FERB's menu bar menu (or press ⌘, for
Settings). The window has a sidebar:

- **Tasks:** every task, newest first, with FERB's answer; **Inspect** opens its full record; **Pass**/**Fail** give
  your verdict; **Stop** stops a running task; **Stop everything** stops FERB until you press **Resume FERB**.
- **Schedules & Reminders:** each schedule's rule, next run and last result, with Pause, Resume and Cancel; pending
  reminders with Cancel.
- **Noticed:** everything FERB noticed and told you; outside text it quotes is shown as a quote.
- **Memory:** what FERB thinks it learned (Confirm or Not right) and what it knows (**Forget…** shows exactly what
  goes before anything is deleted).
- **Settings:** General (shortcuts, FERB's pointer, sounds), AI (the setup window's Choose your AI, Check connection,
  Sign out of ChatGPT), Permissions and Browsers (the setup window's steps), Notifications (which kinds FERB posts),
  Privacy (Pause recording, window titles, the apps and sites FERB ignores, how long records are kept) and Advanced
  (logs, Check FERB, **Export a diagnostic file**, **Send diagnostics to developer**, Install command-line tool,
  Uninstall).

Settings change `~/.ferb/config.toml` only through the app (or the CLI), never through FERB itself: the protection
lists, secrets, models and the feedback address stay in `config.toml`. **Export a diagnostic file** writes
`~/.ferb/reports/FERB-diagnostic-<date>-<time>.txt` with secrets removed and shows it in Finder. **Send diagnostics
to developer** writes the same file (with recent setup, pairing and watcher events) and opens a new email in your Mail
app with it attached; nothing goes until you press Send there.

## Privacy and debugging

`~/.ferb/config.toml` holds configuration. `ferb.db` contains tasks, traces, reminders and memory.
`activity/today.jsonl` holds redacted daily activity; it rotates on the next read/write after the local date changes.
Memory retrieval, activity search and registry search supply relevant context each turn. Forgetting source-backed
memory removes its corresponding daily-log copies too. This is single-device context, not cross-device sync.

Sensing records app/window/tab metadata, configured folder events (paths), apps installed or removed, typed text per
field and clipboard text (both
redacted, kept seven days), never continuous screenshots. Exclusions and pause apply before storage. On macOS 15.4
and later, FERB reads the clipboard only when macOS lets it without asking: set FERB to Allow in System Settings →
Privacy & Security → Paste from Other Apps (setup offers to let macOS ask once so FERB appears there; until then
`ferb doctor` says clipboard capture is off). FERB never triggers macOS's paste alert in the background.
With every message you send, FERB captures your front window (the topmost window that isn't FERB's own): the app,
its title, a browser's front tab, and one redacted screenshot of that window. The planner sees it with your message
and can answer a question about your screen from it; the worker for that message starts from the same picture. The
hosted model therefore sees your front window, redacted, with every message. An app on your exclusion list is not
looked at or named; a site on it is still seen, under an "excluded" label. Later pictures need an explicit screenshot
request. Browser windows, task-tab screenshots and recording frames all go to the model redacted: password and code
fields (shown passwords included) are painted over, and on-device Vision OCR masks classified secrets before an image
reaches a model, evidence file or recording. When FERB can't locate some of a window's secret fields (a browser
profile that isn't paired, a page Accessibility can't read), the fields it did locate are still painted, the rest of
the picture is masked by the OCR only, and the picture says so; an image is withheld only when masking or redaction
fails. OCR can miss unreadable/obscured text; this is class-based redaction,
not a guarantee that all private text is removed. A browser's URL is read from the window actually in front, or not
sent.
`browser.secret` and `mac.secret` ask through hidden CLI input and fill only the exact origin/app and element.
`mac.secret` refuses every browser FERB knows and any app macOS lists as opening web links: a web page's password goes
through the browser connection, tied to that site's address, never to all of Chrome or Safari.
They can offer to remember a specific item in Keychain; the prompt names the handle and bound destination. Models
receive only handles and fill facts. Login codes, including recognized split numeric boxes, use sealed browser fills.
A choice you pick (a "1" for SMS) is never treated as a secret. A code typed into an ordinary question that asks for a
code is hidden for that answer, but only codes entered through the hidden input are remembered as secrets.
Full trace output remains private data: share deliberately.
**Send to developer** (L35): under an answer whose task FERB recorded as failed or interrupted, or whose turn FERB
recorded as failed (it couldn't start or finish it), a button writes one text file,
`~/.ferb/reports/FERB-report-<date>-<task>.txt` (owner-only, kept 30 days): a summary (versions, AI service and model,
status, reason, times), your request and FERB's answer, the task's full trace as `ferb trace TASK --full` shows it,
and FERB's own records of requests that failed inside it or answered slowly (those during the task and the latest 20).
FERB's raw error log never goes in. Every part passes FERB's secret redaction: the trace's records value by value
before they are written out, then each part as text, a saved secret matched also where it shows escaped (inside JSON
or a Python repr). Redaction covers secret classes, so the trace's other private text (pages, file names, messages) is
in the file. FERB then opens a new email in your own Mail app, to `[feedback] address` in `config.toml` (default
`ktky.attri@gmail.com`; empty turns it off), with the file attached. Open the file to see exactly what you would send.
A report leaves the Mac only when you press Send in Mail: FERB never sends it. Without a Mail account, FERB shows the
file in Finder and the address to send it to. An answer the notch itself marked interrupted (its task stopped running
with no answer from FERB) has no button: FERB has no record of how it ended.
Shell output is redacted incrementally before being written to owner-only logs; paging can reach the whole log.
FERB's shell and its AppleScript run inside a macOS sandbox built from `[protection]`. It can't read the credential
stores or FERB's own files, can't write the never-write places, can't use the Keychain, can't open apps or URLs or
start background jobs, and can't send Apple Events to other apps. Neither can anything it starts, a coding agent
included.

Its only way to the internet is FERB's egress proxy, which records each host, port and byte count per command (never
the page, path or query). Shell and browser reach any site without asking. The full answer lists "Sites contacted":
every site the shell reached, the addresses the task opened in its browser tabs, and the pages those tabs loaded
(clicked links, and the landing page of a redirect); not the requests a page makes on its own, a native app's network
use, or pages loaded while sensing is paused (`ferb pause`).
Output from a command that reached a site is labelled with that site. Programs that ignore the proxy settings
get no network, and `ssh` doesn't work. FERB's shell can't write any Trash folder: only FERB's own Trash tool and
`ferb restore` move things in and out.

Existing local listeners are blocked by default. To approve one, add an entry such as
`local_ports = [{port = 11434, executable = "/absolute/path/to/server"}]` under `[protection]`. HTTP proxy clients use
normal localhost URLs through the authenticated proxy, which checks the listening and connected process executable and
records the actual address/executable. A command can also connect directly (psql, redis-cli) when every listener on
that port runs the pinned executable as the command starts; those connections are recorded from the kernel's own
reports (a lower-bound count, no bytes). Servers started inside FERB's containment still work.

`mac.applescript` runs under a second profile with the same file, network and Keychain rules but keeps Apple Events,
so a script you have it run can drive apps you have authorized FERB to automate (Finder, for example).

Known gaps:
- A server the shell starts on `0.0.0.0` is reachable from the LAN.
- A hardlink made before FERB is a separate name.
- Local executable pins trust that executable's scripts/plugins. OS process snapshots have an exec race; an approved
  local service may itself fetch outside FERB's proxy.
- Certificate service `trustd` is denied by default. A Go TLS command may request the exact-command `trustd` escape;
  approving it permits certificate URL fetches outside the proxy for that process tree.

`shell.run`/`shell.start` may name one `escape` rule from `shell.rules`. FERB shows the exact command and working
folder and asks once for that launch; a No is final for that command in the task. `admin` uses hidden password input,
then starts the exact command inside the same sandbox under sudo. Read lifts do not lift writes; write lifts do not
lift file contents. FERB state, its proxy requirement and the question before terminal input are never lifted.
Both worker identities can use shell, Mac and clipboard tools; account-bearing tools remain identity-bound, and
checkers remain observation-only. `shell.send` records input as FERB's; `mac.clipboard` records the actual change.

`fs.disk_usage` reports metadata sizes without opening file contents. `ferb stats` reports shell startup-to-output/exit
latency, destinations/refusals, confirmation counts and escapes. Daily summaries include a security digest. Test
regressions enable a send allowlist (`ferb regress SET --allow-send EXACT_RECIPIENT`, or `"SITE @ACCOUNT"` for a
post) and clear it afterward.

If the sandbox can't run, those tools are off and `ferb doctor` says why. `shell.rules` lists the rules.
Before each shell command FERB takes a local disk snapshot (at most one per 15 minutes; a recent one is reused). If
none can be taken the command still runs, and `ferb doctor` shows the last failure.

FERB itself asks you, in chat, before some steps run. FERB's own checks start with `FERB:` and offer Yes or No. A
task's question always reads "Question from the task: …". A task's option, and a line of a plan, reply, question or
answer the task wrote, that starts with "FERB" (ignoring case, spaces and punctuation) has "(from the task)" in front.
Letters from another alphabet that only look like "FERB" aren't caught in those chat lines. Answer in chat in your own
words: a plain "Yes" or "No" answers one of FERB's checks only when FERB can tell which question it is for; if it could
answer more than one, FERB asks which. FERB asks only for these (Enter counts as No in the terminal):
- money: a payment, always;
- account security: a change to a password, two-factor or recovery setting, or deleting the account, always;
- a permanent delete of something you didn't name (a folder shows its count, size and a sample);
- sending as you to someone new: a message, post or call to someone you didn't name, or to a contact it can't name
  ("Rahul" names "Rahul Verma"; typed in lower case, only the full name counts), unless a standing permission covers it;
- private data going somewhere you didn't name: sending to a place you haven't named after the task read your own
  files and outside content, a link carrying one of your actual private values, or a recipient, number or amount that
  only outside content (a page, an email, a message, text you pasted) supplied;
- installing a tool you named that isn't installed (asked once);
- an escape-hatch command, and a retry of a step whose outcome is unknown;
- typing into a terminal: every time, showing the exact command (below).

A big job's plan doesn't wait: FERB states the plan, its estimate and any standing permission in the plan line and
starts; reply to change it. You're asked once per task: your answer carries across its retries, and a No stops that
step for the task (when you yourself ask FERB to retry or continue, it asks that one again once). A question stays
open while its task waits; it never expires. A recipient or place you named, chose in an answer, or said Yes to is
known from then on and isn't asked about again. A scheduled run doesn't take such a step, since nobody is at chat to
answer, except a send inside a standing permission recorded with the schedule.

Typing into any terminal (terminal apps and terminals inside other apps, e.g. VS Code's) is enforced by code: FERB
always asks you first, showing the exact command, every time, with no standing permission; a No types nothing. A Yes
covers that one input and the one Return after it ("FERB wants to type "codex" into the terminal in Code, then press
Return there"). What counts as a terminal: a terminal app (Terminal, iTerm2, Warp, Ghostty, Alacritty, kitty,
WezTerm, Hyper, Tabby, Termius, Rio, Contour, any you add to `[protection] terminal_apps`, and any app whose
Info.plist declares itself a shell); an xterm.js terminal inside an app or a page (VS Code, Cursor, Windsurf,
JupyterLab, a cloud shell), by the classes xterm.js puts on its own elements; a native text area whose identifier or
label names a terminal, console or shell (JetBrains IDEs' Terminal tool window, Xcode's console, Nova); and any
typing in Zed, whose terminal panel shows Accessibility nothing. The same question comes before a paste or a drop
into a terminal, a hidden secret into one, a terminal app's menu commands, an editor's Terminal or Run menu items and
run buttons (with the selection and file they would run), an AppleScript for a terminal app or for an app that runs
code or opens files (System Events, Finder, Script Editor, Automator, Shortcuts), and opening a file that runs in a
terminal (a `.command`, shown in full). A click into a terminal needs no question. Not recognized: a custom terminal panel that exposes none of these facts, and
editor keybindings that run code.

For a scheduled campaign (posts at set times), say the exact site and handle when you create the schedule, for
example "every day at 9, post the next launch update on x.com from @handle": FERB records a schedule's standing
permission only for destinations in words you typed ("X" isn't "x.com"), and its confirmation lists what it recorded
and what each run will still ask about. A correction to a run can narrow that permission but never widen it, and
cancelling or pausing the schedule ends it.
An oversized unfinished record may be replaced by an explicit redaction marker.

To be told when something happens ("tell me when ~/Downloads/report.pdf exists", "alert me whenever the battery is at
or below 10%"), FERB sets up a watch: a schedule that checks on its interval and survives restarts and
`ferb setup --update` (work you ask for in the same message, such as "turn on Low Power Mode", runs now as its own
task; "tell me when the agent you started is done" is that task's own monitoring, not a watch). A watch that ends once
it happens stops itself after a confirmed check finds it, and that answer says FERB stopped checking; a standing one
("whenever …") tells you once each time the condition becomes true. A met condition comes as a "FERB alert"
notification, as a reminder does, even with the app open; a check with nothing new to tell stays out of your chat.
`ferb schedules` lists each watch with its kind; cancel one there or by asking.

To have something kept true or repeated ("keep this Instagram tab playing until I say stop", "watch PR 12's checks and
loop until they pass"), FERB makes a loop (L31) and says so in its plan line, with one short line for how it ends
("⟳ Until you say stop", or "⟳ Until: “the checks pass”"). A round does the work and checks it,
then picks one exact fact that would show the job needs doing again (a tab's playing state, a command's running
state). Between rounds no model runs: FERB reads that fact every 10 seconds and starts another round only when it
changes, when something you named happens, when you reply with a correction, or after 30 minutes without one. A loop
with no end you stated runs until you say stop (Stop on the task in the notch, or "stop" in chat), and survives
restarts and updates without a message. If a fix keeps not lasting, FERB waits longer between rounds (2 minutes,
doubling up to an hour) and tells you once. It pauses and asks only when a round is blocked, needs you, or an action's
outcome is unknown; the question comes as a card with buttons. If you close the tab a loop keeps going, it asks "The
<site> tab you asked me to keep going was closed. Stop this task?" (Stop / Keep watching). The notch lists a loop as "Keeping: … · fixed N times · last X min ago" with Stop.
While a loop keeps one of your own tabs going, FERB holds it (its debugger bar stays) but between rounds the tab is
yours: a download you start there goes to your Downloads, a link you open in a new tab is your own tab, and with
extension 0.15.10 its file inputs open your file picker (with an older extension they stay FERB's until the loop
ends). FERB's own tabs stay in its minimized window.

FERB keeps a log of its own resource use (L30). Every 5 minutes the daemon writes one row of numbers to its database:
its CPU and memory, those of the processes it started (and theirs) and of FERB.app, the database's size, and counts of
what it did in those minutes (activity events by source, perception batches, Jev calls, model runs, tool calls, tasks
running, and the three busiest processes it started, by program name). Never content, arguments or paths. When FERB
stops, it writes the minutes since the last row. `ferb stats` (`--days N`, default 7; it reads the database, so it works
with FERB stopped) shows a line for each of the last N local days, today included (its other sections count back
N × 24 hours from now): the hours covered (with the Mac awake); the daemon's CPU, its average and busiest window (the
one with the most CPU seconds; windows are 5 minutes, shorter at a start or stop, and longer after a sample that
failed, since the next row covers its minutes too) with the time, as a share of one core (Activity Monitor's % CPU);
its memory, average and peak; the CPU of FERB's other processes and of FERB.app; and the database's size. Then come
the five busiest windows of the period, each with what FERB was doing. Apps FERB opens (`open`) belong to macOS, not
to FERB, so their use isn't in FERB's numbers.

Retention (`[retention]` in `config.toml`): `trace_days` (default 30) is how long a finished task's detailed trace is kept,
and `decision_days` (default 30) how long Jev decisions are kept. A task's story (`ferb trace TASK`), the task list,
chat, daily summaries and memory are kept; the story is written when the task ends, and a purge never removes a
task's steps before its story exists. After `trace_days`, `--steps`, `--full` and `--json` have no steps to show.
`evidence_days` (7) is how long captured images and their facts are kept; `recovery_days` (7) how long the copies of
files FERB changed, and FERB's own disk snapshots, are kept (a record of an item FERB moved to the Trash stays while the
item is there, at most 90 days). `resource_days` (30) is how long the resource log's rows are kept. Downloaded files
have no time limit (see downloads above). Every `retention` days setting counts as at least 1 day (0 or less is read as
1), so nothing is purged the moment it is kept.

Start with `ferb doctor` and `ferb trace TASK --full` when something fails. Doctor compares the installed code,
runtime prompts and extension resources with the checkout. Connection errors, provider errors, watcher failures,
planner/dispatch decisions and cleanup failures should remain visible. Background input depends on the app and OS;
no automated test proves that the owner experienced no disturbance.

## Current limitations

- Nothing from the launch week (L1–L33) is live-tested yet; the pending checks are in
  [owner acceptance](docs/OWNER_ACCEPTANCE.md).
- FERB.app is signed with FERB's own certificate, not a Developer ID: the first open needs **Open Anyway**, and
  whether macOS runs the bundled Python from a downloaded (quarantined) disk image under launchd, and keeps the
  permissions across two builds, are untested live. Developer ID signing and notarization come later. The app is about 581 MB on Apple silicon and 609 MB on Intel (downloads 171 MB and 190 MB). The Intel app is tested on a real Intel Mac by the owner ("Yes, I have one"); that check is pending.
- The Chrome Web Store listing isn't live yet; until it is, each profile needs Load unpacked. Brave and Edge may ignore
  the store install request; Vivaldi, Arc and Opera always need Load unpacked.
- Trust, record, recover: after reading a malicious page, the shell could send one of your files somewhere. It is
  recorded, not prevented, until the planned per-task folder scope.
- A private value that is a plain word (an email's local part, a username) in a link no longer asks; outside text
  inside FERB's own records (window titles, notification previews) no longer counts as outside content.
- A browser post, webmail send or payment is checked only when the worker declares it (`effect`); code can't detect an
  undeclared one.
- The hosted model sees your front window, redacted, with every message. Redaction covers secret classes, not all
  private text.
- With the Claude runtime, Claude Code adds text FERB didn't write to every request: a reminder carrying your Claude
  account's email and an environment message (working folder, OS, model, date). Neither is a secret, and `ferb trace`
  shows FERB's own request, not these additions.
- Hearing is per app, so a browser is heard whole; the sound must play for FERB to hear it.
- On macOS 15 and later, macOS asks again about once a month whether FERB may keep capturing the screen (a prompt
  saying FERB "is requesting to bypass the system private window picker"). FERB captures your front window with every
  message and uses screen capture to hear apps, so it shows up regularly; declining it stops screenshots, the
  front-window picture and hearing until you allow FERB again in System Settings → Privacy & Security → Screen
  Recording.
- A watch checks on the schedule interval (the planner uses every 15 minutes or more): no battery, power or file event
  wakes it sooner.
- Perception notices are paused: they need the paid Jev service, and a Jev-free approach is still to be designed.
  Without a working Jev key FERB still records activity but posts no notices; notification triage is shadow only.
- A watch only tells you; to have FERB keep something going, ask for a loop ("keep … going", "loop until …"). A loop
  can only watch what an observation tool reads exactly (a field of a tab snapshot, a shell session's state); it adds
  no wake-up of its own, so it does not keep the Mac awake, and it reads every 10 seconds, so a change that reverts
  within seconds can pass unseen.
- The terminal chat shows no live progress line.

## Development and documentation

```sh
uv run pytest -q                   # offline tests; no real models or personal apps
uv run mypy ferb/core
uv run pytest -m real              # explicit live run; disposable targets, requires observation
```

A test that runs past 300 s, or a test process whose memory grows past 4 GB, ends the run with every thread's
traceback (`tests/support/runaway.py`). `FERB_TEST_TIMEOUT_S` and `FERB_TEST_MAX_RSS_MB` change the limits (0 turns one
off); a test that really takes longer carries `@pytest.mark.ferb_timeout(seconds)`.

The [owner acceptance record](docs/OWNER_ACCEPTANCE.md) separates observed results from checks still pending.
Read [CLAUDE.md](CLAUDE.md) before making changes. Folder ownership, execution flow and troubleshooting are in
[ARCHITECTURE.md](ARCHITECTURE.md). [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) preserves required attribution.
Historical implementation documents are retained in Git rather than duplicated alongside the current design.

### Target recordings

For requested screen recordings, FERB can record specific task tabs (`browser.record`, `ferb_browser.record`) or a
native window selected by a fresh screenshot (`mac.record`). Captures stay bound to that tab/profile or window/PID,
including when you use another app. They never fall back to capturing the whole display or raising a window.
An unsupported or lost capture route returns a concrete error/partial result.

These are sampled videos: 2 captures/second by default, configurable from 0.2–10. They are useful for task replays,
not high-frame-rate video. With `with_audio`, FERB transcribes the app's sound on this Mac and attaches a redacted,
time-aligned transcript; the video itself keeps no audio track and the sound is deleted once transcribed. They need
`ffmpeg` and `ffprobe` installed; FERB searches PATH and the usual Homebrew/user binary folders. Browser capture
requires extension 0.15.11 / protocol 17; native capture requires macOS Screen Recording access and the target window
to remain capturable. A frame whose secret fields can't be painted over is skipped, and the result says how many.

### Hearing during tasks

A worker can listen to one app, or the browser showing a task's tab, for as long as it chooses (`audio.listen`), and
can play, pause, mute or unmute a page's video first (`browser.media`). Each video keeps its own id while the page is
open (extension 0.15.8), and the tab list says which tabs play sound, so a loop keeping a video playing watches that
exact video or tab. FERB's voice helper transcribes the sound on
this Mac with Apple's on-device speech recognition; there is no cloud fallback, so where on-device recognition isn't
available the tool says so. Only the redacted transcript reaches the model, labelled as outside content; the sound
file is deleted as soon as it is transcribed. The sound has to actually play, so FERB may briefly unmute or play a tab
you can hear. A tab is heard through its whole browser (every tab of that browser that makes sound). Setup asks once
for Speech Recognition; `ferb doctor` reports it. Listening ends with the task; there is no always-on hearing.

`recording.stop` finalizes the MP4 plus a JSON receipt of the target and capture times/errors. `recording.inspect`
decodes chosen moments from the actual saved video into a contact sheet. FERB must inspect those pixels before
claiming the requested actions were recorded. Sampled inspection is evidence for the moments viewed, not proof
of every frame. See [owner acceptance](docs/OWNER_ACCEPTANCE.md) for the disposable two-tab check.

### Guided architecture walkthrough

Open [docs/walkthrough/index.html](docs/walkthrough/index.html) in your browser (or run
`open -g docs/walkthrough/index.html`). It works offline without a server or dependencies. Fourteen task journeys
explain the current HLD/LLD step by step, with source links and evidence limits, including the interactive Claude
Code trace. The page does not execute tasks. See [ARCHITECTURE.md](ARCHITECTURE.md) for full Mermaid sources.

### Releasing (owner)

Once: `scripts/sparkle-keys.sh` makes FERB's update key (the private half stays in your login Keychain) and writes
`scripts/sparkle-public-key.txt` (commit it); `scripts/sparkle-keys.sh --export ~/Documents/FERB-keys/update-key`
backs the private key up outside the checkout. Keep that backup offline: without it no later release can update
existing installs.

Each release: build and sign both images with your release certificate (`SIGN_IDENTITY=release scripts/build-dmg.sh`),
then `scripts/release-appcast.sh v<version> build/FERB-AppleSilicon.dmg arm64` and
`scripts/release-appcast.sh v<version> build/FERB-Intel.dmg x86_64`, and publish all four files as one release:
`gh release create v<version> build/FERB-AppleSilicon.dmg build/FERB-Intel.dmg build/appcast-arm64.xml build/appcast-x86_64.xml`.
Every installed FERB finds it through `releases/latest`. A build with `FERB_UPDATES=off` never checks.

SwiftPM fetches Sparkle anonymously: `scripts/swiftpm-anonymous.sh` (sourced by the build scripts) turns git's credential
helper off, so no build asks for your keychain. Source it yourself before a bare `swift build` or `swift test` in
`ui/mac/FERBNotch`.

## Understanding and validating 0.26

Start with [OWNER_GUIDE](docs/OWNER_GUIDE.md), the [capability ledger](docs/CAPABILITIES.md),
[acceptance checklist](docs/OWNER_ACCEPTANCE.md) and [incident audit](docs/AUDIT_0_26.md).
The [daemon RPC contract](docs/RPC.md) is the integration point for future clients.

Full owner requests and artifact text are preserved. Optional context omitted under an explicit token budget has
a visible fetch reference. Target/source ambiguity remains a question for the agent and owner; unsupported routes
return facts. Final answers show every failed/unverified condition before outcome facts.

Every task records desktop observations through cleanup, except while `ferb pause` is on (a read under way when you
pause is dropped). `ferb trace TASK` lists each change near one of FERB's on-screen actions (a tool that can change the
screen, such as opening an app or a tab, clicking, or running a command; FERB's own cleanup counts) and each time an app
came to the front, or the Space changed, with none in progress or ended within the last half second: it says when
FERB's last on-screen action ended and which processes FERB had started were running, for example "· 16:36:02 Google
Chrome came to the front — no FERB on-screen action in progress (the last ended 4.2 s earlier); running: codex exec …
(started by this task)". When this task made none it says "(none in this task; the last of any FERB task ended N s
earlier)", or "(none yet)" when none has ended since FERB started; times under 10 s have one decimal. A process another
FERB task started shows only its id, "(started by another FERB task)". FERB never says who caused such a change: you,
another app, a process FERB started (a coding agent acts with its own permissions) or a late effect of FERB's own
action are all possible, and FERB neither instructs nor restricts another agent's tools (your decision of
2026-10-09). A paused span reads "desktop not observed (paused) until …". `ferb trace TASK --full` shows every change;
`--json` exports a structured outcome. Sampling can miss short changes and reports unavailable fields; it does not
silently approve focus, block actions or restore an old foreground app.

```sh
uv run python scripts/check_doc_sources.py
```

### Memory and learning (F8 checkout; owner live acceptance pending)

`ferb memory dashboard` prints a private localhost link; leave that terminal open while reviewing memories.
It shows identity, facts, entity summaries, past tasks, procedures, lessons and rollups, with source IDs and history.
Confirm a pending identity proposal, procedure or memory, edit a fact, forget a memory and its derived records
(FERB's own lines that hold its words, and what tasks given it did with it, go too, also what a task still running
at the forget records later; a paraphrase in other words can survive, and so can what such a task recorded before a
crash), or roll back a consolidation batch. Forgetting a learned item removes FERB's naming of it and what it
states from the replies and stored turns that were shown it (never from other tasks, even with the same goal); a
procedure's steps shown to a worker, records deleted only because they
were built on it, a lesson's scope labels (app, site, intent) and an entity summary's multi-line text (unless
recorded exactly) can survive.
In chat or the notch, "forget my …" asks a Yes/No question naming each item that goes. The link carries a temporary bearer token in its fragment; it is not a public site.
When FERB guesses something about you that you didn't say in those words (from chat, or from its nightly look at your
day), it asks at once: "Keep this? …" with Keep, Edit and Drop. Keep makes it what FERB remembers, so an older version
of the same fact ends, even one you said yourself; Edit asks for your own words and saves them as yours, ending it
the same way; Drop deletes the guess (your messages stay) and FERB never suggests it again. If you say the fact
yourself while the guess waits, the question closes and FERB keeps your words. The nightly look saves what you said in
so many words as your own, after checking the words against what you typed (never pasted text).
The guesses FERB made before it started asking appear once in the notch as one list, "Review N older guesses", each
with Keep, Drop or a field for your own words.

`ferb memory search 'query' [--app BUNDLE_ID] [--person NAME]` searches derived evidence; `memory.learned` gives
workers the same read-only records, each candidate with what it says, with full paged bodies. A worker's `memory.read` query is ranked the same way. Search ranks by what items say (never JSON field names),
skips words most memories share, and gives recent memories a small boost; what you said yourself counts as always
recent, so at equal relevance it comes before FERB's guesses. Each planner turn gets the 60 most relevant memories.
Each task worker and each step of a plan gets the 20 memories most relevant to your request and its goal; a plan step
also sees the whole task's goal. The sites, apps and people a task names also count on their own, for its workers and
for the planner while it shows that task, so a note about a site isn't buried under long past tasks.
Passwords and security answers you save stay in your Mac's Keychain, one login per site: on a login page FERB lists
what's already saved for that site (names only, never values) and asks only for what's missing. A sign-in page at
another address (the visa site's atlasauth.b2clogin.com) counts for the site you typed for the task when you typed
just one (exactly that site, with or without "www.", never other addresses under it), so it shows the username you
gave for the site from the first sign-in; a login saved there keeps counting for that site in later tasks, and its
secrets fill only on the page they were saved on. The hidden-input question names the account, and whose it is when it
comes from another site: "For atlasauth.b2clogin.com only (usvisascheduling.com account: Asha Rao): …". Tell FERB a
site's username while it works ("the username is …"), or answer its "What username?", and it is remembered for that
site, when you typed the site (in that message or for the task; never a site only a page, a pasted email or FERB named;
then FERB says "Not saved as your username for <site>: you didn't name that site"), and offered on its login page;
forgetting or retiring that memory removes it from the login record too, and a new username you give for the same
fact replaces it there. A password saved for a whole browser before 2026-10-24 can't be used and is listed as "ask
again". A saved answer that is an ordinary dictionary word (a city, a food) is hidden wherever that word appears, in any
case, but never inside longer words; FERB tells you so once, in a line at the end of that task's answer. Any other
answer (a password, a word the dictionary lacks, a word in Chinese, Japanese, Korean or Thai) is hidden wherever it
appears, even glued to other text. None of your answers is hidden inside FERB's own ids. "Remember that …" saves as
yours the whole sentence you typed it in (only a line break or a pasted part ends it, never a dot, "!" or "?"; so "I
don't [paste] like tea" saves "like tea"); FERB's rewording, like any inferred fact, waits for your Keep (FERB asks in
chat and the notch; or `ferb memory confirm ID`) before it reaches a prompt. Things you say with a date ("I'm in Pune
until the 25th") stop being used after that day; when you say something is over, FERB retires it, or asks you when it
only thinks so. Observed patterns not yet seen on three days are labelled unconfirmed hints. Only owner statements or
confirmed identity proposals enter always-loaded identity (`ferb memory identity 'rule' --key NAME`). A pending identity
digest is shown in chat at most once per day: the app shows Confirm and Not right; in the terminal `/memory confirm ID`
records an explicit confirmation.

The sleep service runs incrementally when no task is active: it reads again only cards whose inputs changed, gives
each finished task an intent label (the learning model names what kind of task it was, reusing a kind it has seen
before when the goal is the same; a task's card read again keeps its label), and learns by intent: once a kind of task
has three verified runs, one model call gets its runs (failed ones too, whatever tools they used: a procedure's
example runs first, then the newest, as many as fit the model's window), its procedure and lessons, to write or
revise them; from then on every change to its runs (a new run, or a verdict) brings another call. A lesson revised
this way keeps your helpful/harmful ratings. A procedure you forgot or turned down is never learned again for that
kind of task. A model call that failed is tried again on the next pass; an answer outside the expected form, a call
whose every proposed procedure was refused (its job says why), or input too large for the window, waits until what it
was about changes (that kind of task's runs, or for a label the task's card) or `ferb memory consolidate`, which
requests a pass.
Procedures contain text tool templates and expect lines and cite verified episodes. A step some runs skipped is
optional and must reproduce the runs that cite it; every other step must reproduce every run. Procedures become
learned after three verified successes across two slot values (L36: app versions are not a requirement).
The planner is shown every procedure it may use, whatever words you use, as data: its kind of task's name and
description and its slots (the most recently used kinds first, as many as fit its window); it picks one by meaning
and gives this request's value for each slot. Code checks each value against the slot's type: if any value is
missing or of the wrong type, the values are noted in the task's trace and left out, and the procedure is used with
its blanks; otherwise the run's receipt keeps them (L29). The worker then starts from that procedure,
as data: its steps with this request's values filled in (a blank without a value is left for the worker to fill from
your request), the lessons learned from that kind of task, and the tool calls of its latest verified runs, newest first,
as many as fit the model's window (L29). Procedures still require step checks and final verification.
Outside-derived action/destination steps are quarantined until confirmed (`ferb memory confirm <id>`): a recipient or
contact, an account, an amount, or a path (a file written or moved, a command's working folder) found only in outside
content (a page or message), and the site a send goes to when it names no recipient (a post, a form declared to send) if
only outside content named it; a target you typed or answered, or one found in neither (FERB's or the model's choice, or
an outside value code can't match), is not outside. The site a step only runs on never counts, and neither does a step
of a kind FERB records no target provenance for (the reversible kinds: opening or navigating a site, a tab or an app,
focusing, the Trash, timers, recovery points, stopping a command), so a procedure of navigating, typing and clicking on
a site FERB read needs no confirm (plan 2026-10-23 Task 2); a send, typing or click recorded without provenance (before
F3) still holds it. Reading an outside page alone does not quarantine a procedure. A revision of a procedure is the same
procedure: its runs keep counting (receipts belong to its key), and the confirm you gave by hand carries over unless the
revision adds a step built on outside content that the version you confirmed didn't have (then `ferb memory consolidate`
names it; if the hourly pass revised it, `ferb timeline` shows `memory.procedure_requarantined`). A learned procedure
remembers the app versions it was learned on; on a newer version it still runs, checking each step; two consecutive
failures mark it suspect. A suggestion you turned down ("Not right") is not proposed again.
Runs whose app versions FERB couldn't read add nothing to that range; with no range there is no new-version check.
Steps are compared with their runs without the ids that change every run (tabs, page elements, shell processes,
windows, FERB's ids) and with `~/` paths expanded, so repeats are learned (L36); typed text, checksums and other values
must still be equal. A learned step names those ids and temporary files by kind, for the worker to find in its own run,
never an earlier run's. A successful run counts as using its procedure only when it followed every required step; a run
that fails counts against it either way, so one that breaks partway is still marked suspect, except a run you stopped or
said No in. `ferb stats` shows the procedures stored, learned, used and refused.

Keyword and Unicode title search work immediately. Optional local embeddings require the `memory` package extra
(not in FERB.app); without it, the embedding commands say so before downloading anything.
`ferb memory eval-export` exports real historical queries for owner relevance labels; no labels are invented.
`ferb memory embeddings prepare --manifest FILE` downloads public model artifacts pinned by a full repository
commit and every file's SHA256. The JSON manifest declares `repository`, `revision`, `files` (path to SHA256),
`dimensions` (256 or 512) and an optional model-specific `query_prefix`. It contains no private text.
`evaluate --manifest FILE --dataset FILE` compares keyword and hybrid reciprocal rank on those labelled queries;
`adopt --manifest FILE --evaluation ID` only enables a model whose measured hybrid score improved. The model loads
from verified local files, with remote model code disabled. No model is selected or downloaded by default before
that evaluation. `status` exposes adoption and reports. Real task with/without-memory acceptance remains separate
from retrieval scores; the paired report helper preserves actual trace IDs and missing owner verdicts.

### Release measurement and owner checks

`ferb release-checklist` prints the packaged 20-check catalogue covering all eleven target-task categories and the
38 security scenarios. It executes nothing and records no verdict. Prepare the named owner-controlled surfaces,
use the Codex runtime and test-mode send allowlist, and record real traces and `ferb verdict` results. The full
security pass includes five runs per attack case and its benign twin; P0 cases 1–7 are the milestone live subset.
Notification triage remains in shadow; its target check measures observation and labels without enabling actions.
Third-party outreach still needs the owner's selected contacts, accepted platform terms and concrete send scope.

After owner installation and representative use, capture `ferb baseline`, then compare the two saved files with
`ferb baseline --compare BEFORE.json AFTER.json`. The JSON includes denominators, percentage-point changes, timings,
cache rate and missing measurements. Different task mixes and overlapping all-time snapshots are explicit limits;
aggregate improvement does not stand in for owner acceptance. With/without-memory comparisons use matched real runs.

For owner-supervised live checks, `ferb test-mode on` refuses all declared send actions; add repeated
`--allow-send EXACT_RECIPIENT` only for owner-controlled destinations. A post has no recipient, so allow it as
`--allow-send "SITE @ACCOUNT"` (for example `"x.com @ferb_test"`), or a bare `"SITE"` for a post that declares no
account: such a post passes only on a page of that host from that declared account, never a payment. An entry that is
a URL is refused. `ferb test-mode` inspects the active setting
and `ferb test-mode off` disables it. Manual mode persists across daemon restart. `ferb regress --allow-send EXACT_RECIPIENT` temporarily sets this gate and
restores the prior manual setting afterward. This gate covers send-class tools and declared GUI purposes; it is not
a semantic classifier for arbitrary GUI input or shell programs.

The [baseline comparison report](docs/0.27_BASELINE_COMPARISON.md) preserves the saved F0 measurements and
marks the new installed-run results unavailable until owner acceptance. `ferb release-checklist` includes
the complete prepared procedures from the 38-case security catalogue alongside the target-task checks.

## License

FERB is licensed under the [Apache License 2.0](LICENSE). Retained third-party attributions are in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
