# FERB

FERB is a personal assistant for macOS. Talk to it with `ferb chat`; its orchestrator assigns workers, answers their
questions from context and changes approach when needed. Agents interpret your request and decide what actions it
authorizes. They prepare a concrete hard-to-undo action, confirm its identity, and carry that confirmation across retries.

Version **0.26.1**; 0.27 is in development on `astra/0.27-upgrade` ([plan](docs/0.27_PLAN.md)). See the capability
ledger for measured behavior and pending owner acceptance.
The [changelog](CHANGELOG.md) records changes; [architecture](ARCHITECTURE.md) explains the implementation and limits.

## Setup

Requires macOS on Apple silicon, Python 3.12 through `uv`, Node/npm, and a signed-in Codex or Claude account.
The pinned runtime versions/model are in `package.json` and `ferb/runtimes/codex_exec.py`.

```sh
uv sync --group dev
uv run ferb setup
```

Setup installs a dedicated interpreter, environment, runtime and LaunchAgent under `~/.ferb`. It collects macOS
permissions and reports missing grants. `ferb setup --update` installs the current checkout and restarts the daemon;
run it when existing work can be interrupted. The development environment and installed daemon are separate.

Chrome and Brave need FERB extension **0.14.0** (protocol 10); downloads without a Save dialog need it. After pulling
it, run `ferb setup --update` to republish the managed bundle; a managed install reloads it when idle, and an older one
is refused with a reload notice. Prepare its managed folder with `ferb browsers extension install`;
`ferb browsers extension path` prints the path. Enable Developer mode in the browser's extensions page and Load
unpacked from `~/.ferb/extensions/chromium`, once per profile. Pair with:

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
content. Limits: `browser.download_max_mb` (100) per file and, per task, `browser.download_task_max_files` (20),
`browser.download_task_max_mb` (500) and `browser.task_max_tabs` (20 new tabs FERB opens from its pages); one download
at a time per tab (the next waits up to 20 s), each within 10 minutes. Refused ones are reported, still without a
dialog. A page that starts more saves at once than FERB holds (20 in a tab, 100 in all) has the rest dropped (neither
taken nor given to the browser), and the task is told. The worker sees the facts in its open/navigate result and with
`browser.downloads`. The files are FERB's working
copies: after `retention.download_days` (30), or when the folder passes `browser.download_total_mb` (2048), they move to
the Trash, so ask FERB to copy a file you want to keep where you want it. While a task drives one of your tabs,
downloads in that tab and in tabs opened from it go to FERB, not to your Downloads; tabs opened from your tab are
released to you, never closed, when the task ends. Your downloads in other tabs, and in your tab once the task lets go
of it, are never touched. FERB adopts only tabs it opened itself and new popups that the browser confirms a task tab
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

Optional: `ferb setup --voice` builds the background microphone app. Hold Right Option to talk; release to send.
`ferb chat --speak` speaks terminal replies. Pending questions accept free text, including corrections.
`ferb setup --jev` configures optional memory/activity enrichment; it does not decide task permissions.

## Everyday use

```sh
ferb chat                         # resumes the last conversation
ferb chat --new                    # separate conversation
ferb chat --resume ID
ferb trace                        # latest task: the story, about 10-20 lines
ferb trace TASK --steps            # one line per tool call
ferb trace TASK --full             # complete recorded inputs, results and decisions
ferb trace TASK --json             # machine-readable trace
ferb logs --tools                  # one live step line per tool call while a task runs
ferb stats
ferb doctor
```

FERB can read and operate native apps, browse through its extension, run shell commands and coding sessions,
work with files, send email from its own account, remember useful context, and create reminders or scheduled tasks.
Each new action task has explicit success conditions. Result inspection uses recorded evidence and fresh reads;
incomplete coverage goes back to the orchestrator. A delivered click or launched process does not prove completion.
The final check is read-only, enforced by code: it can look but not click, type, send or write.

A task ends in one of three results, each its own status:
- **done**: every condition was checked and confirmed. `ferb trace` heads the story `done ✓ verified`.
- **done, but not confirmed** (`done_unverified`): the work finished and the check found nothing wrong, but couldn't
  confirm everything from the background. The answer starts "Done, but not confirmed:" and lists each unconfirmed
  condition as "— unverified: <the real reason>", often with "To check: …" saying how you can check it yourself.
  `ferb trace` shows `done (not confirmed)`. A task with nothing to check against says "Done, but there was nothing to
  check it against."
- **failed**: the work didn't finish, a check found a condition false, or FERB judged that it failed. The answer gives
  the real reason; `ferb trace` shows `failed ✗`.

"Unverified" appears only beside a condition a check ran on; the status itself reads "not confirmed". If the work
stopped before any check, the answer says "I couldn't finish this task." and lists what is "Still to do". `ferb stats`
counts the three apart ("done N (x%), not confirmed N (y%), failed N (z%)"), `ferb baseline` records them as `done`,
`unverified` and `failed` with their percentages (`unverified` is the not-confirmed count), and `ferb chat --verbose`
shows `done ✓`, `done — not confirmed` or `failed ✗` before the answer.

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
replacing anything ("<name> (restored)<ext>" beside whatever is there now). Before bulk work an agent may take a local
disk snapshot (at most one per 15 minutes; FERB deletes its own after 7 days). FERB can't open a snapshot itself (that
needs admin rights); you can, from macOS Recovery or Time Machine. File changes FERB makes through shell commands or
AppleScript get no per-file copy.

Closing chat leaves tasks running. Ctrl-C stops the current task. `ferb stop` stops all tasks until
`ferb resume-tasks`. `ferb pause` / `ferb resume` pause/resume activity sensing.

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

Codex runs in FERB's own home and needs `ferb setup --codex-login` (there is no fallback to `~/.codex`); without it
the chat says to run that command, and `ferb runtimes` shows Codex as not usable. `ferb runtimes use` still saves the
choice for a runtime that is not usable yet and warns that the role will fail until it is.

The Codex and Claude runtimes use your subscriptions. The Anthropic, OpenAI and Google API runtimes bill per token
against the key you add; `ferb runtimes` labels each runtime's billing.

Use `ferb COMMAND --help` for options.

## Privacy and debugging

`~/.ferb/config.toml` holds configuration. `ferb.db` contains tasks, traces, reminders and memory.
`activity/today.jsonl` holds redacted daily activity; it rotates on the next read/write after the local date changes.
Memory retrieval, activity search and registry search supply relevant context each turn. Forgetting source-backed
memory removes its corresponding daily-log copies too. This is single-device context, not cross-device sync.

Sensing records app/window/tab metadata and configured folder events, not keystrokes, clipboard or continuous
screenshots. Exclusions and pause apply before storage. Task tools can explicitly capture windows for the model.
Those images, and one picture of an app's window when a worker starts on it, are kept in `~/.ferb/evidence` so the
check can look again. A secret field the accessibility tree shows is blacked out, or the image is kept as facts only.
Always facts only: `mac.screenshot` of a browser app's window (it shows your own tabs), a page with no known host,
messaging and mail apps, and recordings' frames (full screenshot redaction arrives with F5). A `browser.screenshot` of a
task's tab with a known host is kept as an image, unless the site is on your exclusion list.
Secrets are redacted; OTPs use a sealed input path. Full trace output remains private data: share deliberately.
Shell output is redacted incrementally before being written to owner-only logs; paging can reach the whole log.
An oversized unfinished record may be replaced by an explicit redaction marker.

Retention (`[retention]` in `config.toml`): `trace_days` (default 30) is how long a finished task's detailed trace is kept,
and `decision_days` (default 30) how long Jev decisions are kept. A task's story (`ferb trace TASK`), the task list,
chat, daily summaries and memory are kept; the story is written when the task ends, and a purge never removes a
task's steps before its story exists. After `trace_days`, `--steps`, `--full` and `--json` have no steps to show.
`evidence_days` (7) is how long captured images and their facts are kept; `recovery_days` (7) how long the copies of
files FERB changed, and FERB's own disk snapshots, are kept (a record of an item FERB moved to the Trash stays while the
item is there, at most 90 days); `download_days` (30) how long downloaded files stay in FERB's workspace before they
move to the Trash. Every `retention` days setting counts as at least 1 day (0 or less is read as 1), so nothing is
purged the moment it is kept.

Start with `ferb doctor` and `ferb trace TASK --full` when something fails. Doctor compares the installed code,
runtime prompts and extension resources with the checkout. Connection errors, provider errors, watcher failures,
planner/dispatch decisions and cleanup failures should remain visible. Background input depends on the app and OS;
no automated test proves that the owner experienced no disturbance.

## Development and documentation

```sh
uv run pytest -q                   # offline tests; no real models or personal apps
uv run mypy ferb/core
uv run pytest -m real              # explicit live run; disposable targets, requires observation
```

The [owner acceptance record](docs/OWNER_ACCEPTANCE.md) separates observed results from checks still pending.
Read [CLAUDE.md](CLAUDE.md) before making changes. Folder ownership, execution flow and troubleshooting are in
[ARCHITECTURE.md](ARCHITECTURE.md). [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) preserves required attribution.
Historical implementation documents are retained in Git rather than duplicated alongside the current design.

### Target recordings

For requested screen recordings, FERB can record specific task tabs (`browser.record`, `ferb_browser.record`) or a
native window selected by a fresh screenshot (`mac.record`). Captures stay bound to that tab/profile or window/PID,
including when you use another app. They never fall back to capturing the whole display or raising a window.
An unsupported or lost capture route returns a concrete error/partial result.

These are silent sampled videos: 2 captures/second by default, configurable from 0.2–10. They are useful for task
replays, not high-frame-rate video or audio recording. They need `ffmpeg` and `ffprobe` installed; FERB searches PATH
and the usual Homebrew/user binary folders. Browser capture requires extension 0.14.0 / protocol 10; native capture
requires macOS Screen Recording access and the target window to remain capturable.

`recording.stop` finalizes the MP4 plus a JSON receipt of the target and capture times/errors. `recording.inspect`
decodes chosen moments from the actual saved video into a contact sheet. FERB must inspect those pixels before
claiming the requested actions were recorded. Sampled inspection is evidence for the moments viewed, not proof
of every frame. See [owner acceptance](docs/OWNER_ACCEPTANCE.md) for the disposable two-tab check.

### Guided architecture walkthrough

Open [docs/walkthrough/index.html](docs/walkthrough/index.html) in your browser (or run
`open -g docs/walkthrough/index.html`). It works offline without a server or dependencies. Fourteen task journeys
explain the current HLD/LLD step by step, with source links and evidence limits, including the interactive Claude
Code trace. The page does not execute tasks. See [ARCHITECTURE.md](ARCHITECTURE.md) for full Mermaid sources.

## Understanding and validating 0.26

Start with [OWNER_GUIDE](docs/OWNER_GUIDE.md), the [capability ledger](docs/CAPABILITIES.md),
[acceptance checklist](docs/OWNER_ACCEPTANCE.md) and [incident audit](docs/AUDIT_0_26.md).
The [daemon RPC contract](docs/RPC.md) is the integration point for future clients.

Full owner requests and artifact text are preserved. Optional context omitted under an explicit token budget has
a visible fetch reference. Target/source ambiguity remains a question for the agent and owner; unsupported routes
return facts. Final answers show every failed/unverified condition before outcome facts.

Every task records desktop observations through cleanup. `ferb trace TASK --full` shows desktop changes and nearby
actions; `--json` exports a structured outcome. Sampling can miss short changes and reports unavailable fields; it
does not silently approve focus, block actions or restore an old foreground app.

```sh
uv run python scripts/check_doc_sources.py
```
