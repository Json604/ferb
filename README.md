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

Chrome and Brave need FERB extension **0.14.0** (protocol 10). After pulling it, run `ferb setup --update` to
republish the managed bundle; a managed install reloads it when idle, and an older one is refused with a reload notice.
Prepare its managed folder with `ferb browsers extension install`;
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
content. Limits: `browser.download_max_mb` (100) per file, `browser.download_task_max_files` (20) and
`browser.download_task_max_mb` (500) per task; refused ones are reported, still without a dialog. The files are FERB's
working copies: after `retention.download_days` (30), or when the folder passes `browser.download_total_mb` (2048), they
move to the Trash. Your own downloads, in other tabs or in a tab once its task is over, are never touched. What can't be
intercepted: a page script that saves a file through a link it never adds to the page (FileSaver style), a popup that
downloads the moment it opens, and a download already held when you press Cancel on the "started debugging" bar; the
browser handles those itself (it may ask where to save) and the task is told when FERB saw it.

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
Secrets are redacted; OTPs use a sealed input path. Full trace output remains private data: share deliberately.
Shell output is redacted incrementally before being written to owner-only logs; paging can reach the whole log.
An oversized unfinished record may be replaced by an explicit redaction marker.

Retention (`[retention]` in `config.toml`): `trace_days` (default 30) is how long a finished task's detailed trace is kept,
and `decision_days` (default 30) how long Jev decisions are kept. A task's story (`ferb trace TASK`), the task list,
chat, daily summaries and memory are kept; the story is written when the task ends, and a purge never removes a
task's steps before its story exists. After `trace_days`, `--steps`, `--full` and `--json` have no steps to show.

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
