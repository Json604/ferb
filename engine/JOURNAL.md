# Rust rewrite journal

What was done and why, oldest first. One entry per slice of work: **What** changed, **Why** it was done that way,
**Evidence** (commands run and their results, file references), **Open** (what is unfinished or unverified), **Next**.
The state of every part is in `STATUS.md`; decisions are in `kb/decisions/engine.toml` (ENG-*); this file is the story.

Rules for entries: say what was checked and what was only inferred; a port is "verified" only against fixtures made
by the Python build (ENG-3) or a live check, never because it compiles.

---

## 2026-11-13 · 1 · The research was checked, and the plan changed

**What.** The migration research (a folder on the owner's Desktop, `FERB-Rust-Migration-Handoff-2026-11-13`, written
by another agent from a read-only review) was checked by six read-only agents against the code at `63369cb3`.

**Result.** Its facts about the code hold: several hundred file references checked, no false statement about
behavior, some misplaced line numbers, all 1,287 inventory hashes match. It corrects HANDOFF.md in three places: the
daemon host is C embedding Python (`ui/mac/FERBAgent/main.c`), there are 58 migration files (highest 0062), and Codex
runs only through `app-server` (`ferb/runtimes/codex_stream.py:289`).

**What the research got wrong or left out** (each found in the code):
- The Python Claude bridge is not needed: `ferb/runtimes/claude_sdk.py` only starts the bundled `claude` program in
  stream-json mode (no hooks, permission callbacks or resume). → ENG-2.
- Output that must match byte for byte is barely covered: prompts composed with `json.dumps`
  (`ferb/orchestrator/supervisor.py:1230-1258`), tool schemas from pydantic (`ferb/daemon/gateway.py:725`),
  `jsonschema` error text sent back to the model (`ferb/core/structured.py:24-27`), and redaction patterns that use
  lookaround, backreferences and possessive quantifiers (`ferb/sanitize/formats.py`, `labels.py`, `stream.py`), which
  the `regex` crate lacks. → ENG-3.
- Some "ports" are new design: pending questions live in memory (`ferb/daemon/questions.py:121`), history pages
  within one day only (`ferb/store/chat_history.py:123-129`), the phone library has no close call or panic guard.
- ffmpeg is not bundled today (`ferb/act/recording.py:43-51` finds it on the PATH).
- It puts the port, shared history, the phone role, two phone apps and push behind one cutover. → ENG-1.
- Its Windows/Linux placeholders go against PROJ-506; they are not built.

**Why the order changed (ENG-1).** A parity port on the unchanged schema can be compared with Python and rolled
back. Shared history, phones and push then land one at a time on a working engine.

**The owner's word.** PROJ-508: branch `rust`, the agent decides how, keep this journal.

## 2026-11-13 · 2 · Toolchain, workspace and gate

**What.**
- Installed Rust 1.100.0 with rustup (minimal profile, clippy, rustfmt) in the home folder; pinned in
  `rust/rust-toolchain.toml`.
- Created the workspace `rust/` with five crates: `ferb-store`, `ferb-sanitize`, `ferb-rpc`, `ferb-native-host`,
  `ferbd`. More are added when a port needs one.
- kb: new area `engine` (ENG, path `rust`), decisions PROJ-508 and ENG-1..4. Nothing existing was edited.
- Gate: `rust/gate.sh fmt` on commit, `lint` and `test` on push, only when `rust/**` changes. The Python and Swift
  checks are unchanged.

**Why.** The gate refused files under `rust/` until an area owned them, and once owned, none of the language checks
would have run on them.

**Evidence.** `cargo build` and `cargo fmt --check` pass on the skeleton; `tools/kb/run kb check` ok.

**Open.** The machine had 22 GB free at the start; `rust/target` is the thing to watch.

**Next.** First ports, each against Python-made fixtures: the store (open, migrations, checksums), redaction, the RPC
wire and roles, and the browser's native-messaging relay.

## 2026-11-13 · 3 · The store foundation: connection, migrations, checksums

**What.** `ferb-store` ports `ferb/store/db.py` except `swap`: `Db::open` (Python's pragmas in order, mode 0600),
`transaction`, the migration runner with every refusal and Python's messages, WAL truncate/checkpoint, incremental
vacuum, compact. `build.rs` embeds `ferb/store/migrations` (ENG-4).

**Why.** Statements are split with SQLite's own `sqlite3_complete`, line by line as `_split_sql` does, so the stored
schema text is identical. A build script was used instead of a new dependency.

**Evidence.** `rust/fixtures/store/make.py` (Python, temp folder) wrote `fresh_store.json`; the Rust test matches all
243 schema rows, 58 checksum rows, pragmas and mode. `cargo test -p ferb-store`: 14 passed, 1 ignored; the ignored
test (`-- --ignored`, needs `uv`) passed when run by hand: each side opens the other's store with nothing to apply.

**Open.**
- `swap` (`db.py:236`) not ported; upgrade tests on older stores (STORE-403, T-400/T-403) not ported.
- Bundled SQLite is 3.46 against Python's 3.53.4; the schema matches today, but FTS5 ranking and JSON behavior have
  to be compared when the search and memory ports land.
- Python refuses two statements on one line; Rust runs the first. No migration has such a line.
- Found in Python, not changed: `migrate` rolls back unconditionally and can hide the real error (`db.py:212`).

## 2026-11-13 · 4 · Redaction and origins

**What.** `ferb-sanitize` ports `ferb/sanitize` (all but `images.py`, which is Vision OCR) and `ferb/core/origin.py`.
Python's pattern text and Unicode tables are generated into `src/generated.rs` by the fixture script and translated
for `fancy-regex` in `rx.rs`.

**Why.** Rust's `\s`, `\w`, `$`, `(?i)` and verbose mode differ from Python's; translating Python's own pattern text
keeps one source, and Unicode answers come from Python 3.12's tables so a newer Rust changes nothing.

**Evidence.** `rust/fixtures/sanitize/make.py`: 8,208 invented cases (strings stored base64 so the gate's secrets
check finds nothing); `cargo test -p ferb-sanitize`: 15 passed, about 31,000 comparisons, none differ, including the
stream redactor on random chunk splits. The porting agent reports wider local runs to 43,002 cases with none
differing (not re-run by me).

**Open: differences from Python that remain.** Each must be closed or accepted before the gateway uses this crate:
- JSON integers beyond 64 bits parse as floats and are not checked. To fix (arbitrary-precision numbers).
- A `fancy-regex` runtime error panics. Fails closed, but would take the daemon down: needs a caught refusal.
- The `Redacted` mark (skip text already redacted) and `bytes` values are not ported; every call redacts.
- `high_entropy` depends on the platform's `log2` at the 3.0 line.
- No committed guard for the pathological-input timings of `tests/test_redaction_scaling.py`.
- Found in Python, ported as is: the short-text needle scan can miss an overlapping needle (`formats.py:291`);
  booleans between private-key lines are masked (`redact.py:346,395`).

## 2026-11-13 · 5 · RPC wire, roles and the FERB.app check

**What.** `ferb-rpc`: frames written as `json.dumps` writes them, requests refused in pydantic's words, the
role/method table (106 cli, 3 mcp_shim, 20 FERB.app-only), run tokens, a tokio Unix-socket server (folder 0700,
socket 0600, hello first, owner uid, ping only while starting, cancel when the caller leaves, shim watch, streams),
and the audit-token → SecCode → requirement check behind a trait. Handlers register through `Server::register_*`.

**Why.** ENG-1: the unchanged app, toolbox, tool bridge and native host must not notice the change. Python picks the
FERB.app-check mode from `sys.prefix`; Rust reads the executable's own path: inside an `.app` = bundled, under a
cargo `target` folder outside FERB's home = checkout (the only mode that accepts the development notch), anything
else = installed.

**Evidence.** `rust/fixtures/rpc/make.py` plays 44 scenarios on the real Python `Server`; `tests/golden.rs` gets
the same 134 frames from Rust. 49 tests pass. The ignored test drives the real `ferb/cli/client.py` against the Rust
server (passed when the porting agent ran it by hand).

**Open.**
- A positive signature check needs a signed FERB.app; only refusals are tested against the real framework.
- DMN-588's signer pin is not in `app_peer.py` either (T-9 is open); it is built with the host port.
- JSON edge cases not compared: NaN/Infinity, lone surrogates, nesting past 128, ids past 64 bits.
- Tests use short real waits (up to 200 ms); TEST-508 wants injected time.
- Found in Python, mirrored: a non-UTF-8 line drops the connection with no reply; any byte from a streaming client
  counts as "left". Found, not mirrored: `is_app` blocks Python's event loop (Rust runs it off the loop).

## 2026-11-13 · 6 · The browser's native-messaging relay

**What.** `ferb-native-host` ports `ferb/native_host/main.py`: std only, two threads, runnable directly as the
manifest's `path` so it stays the browser's direct child (`ferb/act/browser/ext_bridge.py:249-260`). It checks each
message against Python's JSON grammar and forwards the sender's bytes instead of rewriting them.

**Why.** Signed bodies (HAND-222) and lone surrogates cannot change if nothing is decoded, and a message never grows
past the daemon's line limit.

**Evidence.** `rust/fixtures/native-host/make.py` ran the Python relay on 44 cases, writing a case only when
Python's parser reads the raw and rewritten bytes as the same value. `cargo test -p ferb-native-host`: 7 passed.

**Open.** Not run against a real daemon or browser. Rust exits on a non-JSON daemon line where Python lingers.
Found in Python: a parse error kills the reader thread and leaves the relay alive and deaf (`main.py:53-57`); the
rewrite inflates a 1 MiB non-ASCII frame to about 3 MB, past the daemon's limit (`main.py:61`, `server.py:201`).

## 2026-11-13 · 7 · State after the first four ports

**Evidence.** From `rust/`: `cargo fmt --check` clean, `cargo clippy --workspace --all-targets --locked -D warnings`
clean, `cargo test --workspace --locked`: 85 passed, 2 ignored (the two that need `uv` and Python).

**Next.** The Python packages import each other in circles (`policy` ↔ `orchestrator` ↔ `daemon`), so the Rust
layering is its own: core types and ids, config and paths, the event bus, the typed stores (journal, spans,
confirmations, tasks), the secrets-helper caller; then policy and the gateway on top of them.

## 2026-11-13 · 8 · Pushing is blocked by GitHub, and stays blocked until the owner decides

**What.** `git push` of `rust` was refused by GitHub's push protection: it decodes the base64 in
`rust/fixtures/sanitize` and reports the invented key-shaped test values as secrets (10+ detections). The repo's
own gate passed. Commits stay local.

**Why not worked around.** The values have to look like real keys to test the detectors. Re-encoding them so the
scanner cannot see them was refused as going around a security check, and is now a rule in `AGENTS.md`. Allowing
them on GitHub, or excluding the folder from scanning, is the owner's choice (asked; open).

## 2026-11-13 · 9 · Fixtures for what the models receive

**What.** `rust/fixtures/catalog/make.py` starts the Python daemon in a temp home and writes `tools.json` (97
tools, 23 listings by grant, the MCP form), `answer_schemas.json` (86 schemas in each provider's form),
`prompt_static.json` (32 instruction texts, encoder cases, all 238 `json.dumps` sites), `repair_notes.json` (117)
and `validation_errors.json` (627 through the real `Gateway.call`). `README.md` maps each to file:line.

**Why.** ENG-3: pydantic's schemas and error text, `json.dumps` spacing and escaping and jsonschema's messages
decide prompt bytes and provider caches. Rust cannot regenerate them by default, so they are frozen.

**Evidence.** Two runs byte-identical; 1.92 MB; gate patterns clean.

**Open.** Not frozen: real facts, availability and profile text; the text around the other `json.dumps` sites; MCP
frames; where `ferb_browser.*` sits in the list when a profile is paired at start.
Found in Python: `browser.pdf`, `ferb_browser.pdf` and `audio.listen` validators leave a `ValueError` in the reply,
so it cannot be serialized.

## 2026-11-13 · 10 · Core types, paths, config and the bus

**What.** `ferb-core` ports `ferb/core` (types, ids, errors, timing, call_facts, interfaces), `ferb/paths.py`,
`ferb/config.py` and `ferb/bus.py`.

**Why.** tomlkit and `toml_edit` own whitespace differently, so `toml_edit` only locates tables, keys and values
and each edit changes the text the way tomlkit does; shapes not reproduced are refused before anything is written.

**Evidence.** `rust/fixtures/core/make.py`: 85 config cases byte for byte after every write, 5 bus scripts, timing
scripts, every type's JSON. 27 tests pass.

**Open.**
- Ten config shapes are refused (`Unsupported`): a table as a value, inline or dotted sections, sub-table removal.
  Three edits give the same values with a different layout from tomlkit.
- Type JSON is compared by key order and values, not `json.dumps` spacing.
- `SystemClock`/`FakeClock` are not here; `ferb-core::when` and `ferb_py::time` are two datetime types.
- Found in Python: an empty `FERB_HOME` means the current folder (`paths.py:10`); the C host reads it as unset.

## 2026-11-13 · 11 · The typed stores

**What.** `ferb-store` now ports the rest of `ferb/store`: spans, task results, chat history, retention, the
clean-up, salvage and `swap`. Other packages come in through seams (clock, ids, redactor, reversible kinds, card
sources, journal, TOML check, settings).

**Evidence.** `make_tables.py` ran the Python stores in temp folders; `tests/tables.rs` gets identical returns and
rows for 291 calls in 6 scenarios and 62 salvage steps. SQLite 3.46 (bundled) matched Python's 3.53.4 on every
ported query; FTS5 ranking is not used by these stores and is compared in the memory port.

**Open.** `json.loads` edge cases (NaN, integers past 64 bits, lone surrogates); `lower()`/`upper()` follow Rust's
Unicode tables; upgrade tests on older stores (STORE-403) still unported.
Found in Python (mirrored, to fix under ENG-6): `retention._with_descendants` never ends on a span parent cycle.

## 2026-11-13 · 12 · The secrets-helper caller and owner secrets

**What.** `ferb-identity` ports `ferb/identity`: the caller of the pinned Swift helper (unchanged), owner secrets,
configured secrets, origins, seals, the mail inventory, the Chrome profile check and `secrets.list`. Values are a
`Secret` type: zeroed on drop, printed as `<secret>`.

**Why.** The pin stays the requirement file in FERB's home, checked by `codesign` before every run, so tests run
the real check on an ad-hoc-signed fake helper and no test-only switch exists. The helper's environment is cut to
five variables (Python passes everything).

**Evidence.** `rust/fixtures/identity/make.py`: 33 helper cases, 87 owner-secret steps, 31 handle files; same
stdin bytes, refusals and `owner-secret-handles.json` bytes. 21 tests pass.

**Open.** Never run with the real helper or Keychain; the scrubbed environment is unproven there. serde_json's
scratch buffer is not zeroed. The `security` CLI branch (`secrets.py:80-84`) is not ported.
Found in Python (mirrored, to fix under ENG-6): the helper is verified, then run by path, which leaves a swap window.

## 2026-11-13 · 13 · Redaction hardened

**What.** `ferb-sanitize`: a `Py` value type (any-size integers, bytes, the Redacted mark) with Python's JSON
reading and writing; a pattern that cannot run hides the whole input (`[redacted: unreadable text]`) or returns
`Err(Unread)` and never panics; `high_entropy` fixtured at the 3.0 line; a scaling guard by instruction counts.

**Why.** serde_json's arbitrary precision would change every crate's numbers. The mark changes output
(`redact.py:377`). Python's `re` cannot fail, so hiding everything is the only safe answer when Rust's can.

**Evidence.** 8,706 tracked cases; 26 tests pass; three seeds at scale 8 (about 33,100 cases each), none differ.
Scaling: 6,912 runs, none slow.

**Open.** `formats`/`labels`/`known` read a pattern failure as "no match" unless wrapped in `checked`. The scaling
test takes 54 s. `value(&serde_json::Value)` cannot carry marks, bytes or big integers (use `Py`).
Found in Python (mirrored, to fix under ENG-6): a key body under a non-text key between BEGIN and END items is left
unredacted (`redact.py:395`). That build is installed today.

## 2026-11-13 · 14 · A shared foundation: portable engine crates, one Mac platform crate (ENG-5)

**What.** `ferb-platform-macos` holds what touches Apple frameworks or macOS tools: the Security framework, the
peer's pid and audit token, `codesign` and the helper process, Chrome's folder. `ferb-py` holds Python's own
behavior once. `ferb-rpc` takes the peer lookup by injection (`Server::set_peer_lookup`); `ferb-identity` runs the
helper through `helper::Launcher`. `gate.sh portable` checks the five engine crates for iOS and Android, and the
push gate runs it.

**Why.** The owner asked whether the rewrite was a straight translation or a shared foundation; the same engine
must serve the phone apps, and ports by separate agents had left Mac code in shared crates and three copies of
`repr(float)`.

**Evidence.** Whole workspace, run by the coordinator: `cargo fmt --check` clean, clippy with warnings refused
clean, `cargo test --workspace`: 158 passed, 3 ignored (they need `uv`); `./gate.sh portable` passes.

**Open.** `ferb-store` is not in the portable set (bundled SQLite needs the NDK for Android). `ferb-rpc::app_peer`
and `ferb-core::paths` both find the installed FERB.app.

## 2026-11-13 · 15 · How the work is run from here

**What.**
- `rust/PLAN.md`: what is left, in order, with sources, needs and traps, and how a new agent picks it up.
  `rust/AGENT-BRIEF.md`: the one brief every port agent gets. `AGENTS.md` has a section for the rewrite.
- ENG-6: a bug found in Python is fixed in the port, not mirrored. The owner asked why known bugs were being
  carried over; three mirrored ones are to be fixed next (redaction leak, retention cycle, helper swap window).
- Every part whose needs are met is launched at once, each agent in its own crate with its own build folder; a
  crate being ported sits in the workspace's `exclude` list. The owner asked why the work ran in small waves.
- Subagents run on Opus or below (owner's word); six agents started on the coordinator's model were stopped and
  relaunched.

**Running now (nine agents):** policy, prompt context and answers, files, shell, browser, runtimes, time, memory
(embeddings, search, import parsers), and recordings of the Python daemon's RPC and chat scenarios.

## 2026-11-13 · 16 · Three Python bugs fixed in the ports (ENG-6)

**What.**
- `ferb-sanitize`: a private key's lines spread across items are masked whatever holds them (a dict, a nested
  list, bytes, a node's text); booleans, None and floats between the lines are kept.
- `ferb-store`: the retention walk ends on a span parent cycle; without a cycle it is Python's walk step for step.
- `ferb-platform-macos`: the secrets helper is started suspended and resumed only once its running code and its
  file match the pin; nothing is written to it before that. macOS has no way to run an already-open file.

**Why.** ENG-6: known bugs are fixed in the port, not mirrored. One left a secret unredacted (rule 7).

**Evidence.** Whole workspace, run by the coordinator: format and clippy clean, 170 tests pass, 3 ignored.
The porting agent reports: each helper check, when removed, fails its test; two fresh corpora made by a patched
Python (12,679 and 20,598 cases) match Rust with no exceptions.

**Fixed against Python.** `redact.py:342,393,395` (named parity exceptions: walk 21, 22; py 134, 187),
`retention.py:190`, `helper.py:29-43`. A patch for the Python build that is installed today is at
`rust/fixtures/sanitize/python-redact-fix.patch` (applies cleanly; 146 tests pass with it in a throwaway worktree).
It is not applied: shipping a Python build is the owner's.

**Open.**
- A key line used as a dict key, and nodes inside dict values, are still not read as key lines.
- `rust/fixtures/sanitize/make.py` did not reproduce the tracked random cases on a re-run (from walk 70 / py 52
  on); the cause is not known. A fixture script must write the same bytes every run: to investigate.
- A cycle split across delete batches may still fail its foreign key (not tested).
- Not checked with a certificate pin or the real helper; the cost of the extra check is not measured.

## 2026-11-13 · 17 · Prompt context and structured answers

**What.** `ferb-context` ports `ferb/core/context.py` (o200k token counts, context assembly), `prompts.py`,
`structured.py`, the schemas and JSON readers of `ferb/orchestrator/schemas.py`, and
`ferb/runtimes/schema_shape.py`. It builds for iOS and Android and is in the portable check.

**Why.** These bytes reach a model (ENG-3): jsonschema's chosen error, Python's `repr` and `json.dumps` spacing go
back to the model as written, and a different token count moves what fits in the context.

**Evidence.** Against the frozen catalog: 86 of 86 schemas byte-identical in every provider form; 117 repair notes.
Own fixtures (`rust/fixtures/context/make.py`): 3,552 token strings, 1,497 assembly cases, 1,183 answers (839
invalid), 168 mapped-back answers. 18 tests pass, none differ. The porting agent reports wider runs to 21,122
strings and 5,775 answers with none differing (not re-run by the coordinator).

**Fixed against Python.** None.

**Open.**
- The pydantic outcome models (`SupervisorOutcome`, `TaskSpec`, `WorkerReport`, `CheckerOutcome`) with their
  validators and error text are not ported; they need Plan, When and Targets and go with orchestration.
- A lone surrogate escape reads as U+FFFD; past 9,990 nesting levels Rust says `TooDeep` where Python raises
  `RecursionError`; a schema keyword outside the implemented set is refused when the validator is made.
- `repr` exists here and in `ferb-sanitize`; both should move into `ferb-py`.

## 2026-11-13 · 18 · Files, recovery and the change record

**What.** `ferb-fs` ports `ferb/act/fs` (files, recovery, usage, look, changes) and the file-identity half of
`ferb/policy/protection.py`: one function per tool (`fs.list`, `fs.read`, `fs.write`, `fs.edit`, `fs.copy`,
`fs.move`, `fs.trash`, `fs.look`, `fs.disk_usage`, `recovery.snapshot`), plus Python's difflib and the DOCX reader.
macOS calls sit in `sys::mac`; ImageIO, PDFKit and NSFileManager are traits for `ferb-platform-macos`.

**Evidence.** `make.py` and `make_changes.py` ran the Python on invented trees in temp folders: 24 cases with 196
steps, 72 change scenarios, 202 difflib cases; the races Python's tests make (a link swapped in, a file changed
between read and write, hard links) replay at named hook points. 44 tests pass, none differ.

**Fixed against Python.** `changes.named()` returned a word and the path inside it in hash-seeded set order, so the
output changed between runs; Rust returns the word first.

**Open.**
- The macOS implementations behind `TrashLocator`, `Renderer` and `PdfText` are not written.
- Real `tmutil` is untested; PDF text and times outside UTC are unverified.
- Python's async wrappers are synchronous here: the daemon must run them off the event loop.
- A failed `index.json` write is swallowed where Python raises; DOCX parts that are not UTF-8 give no text.

## 2026-11-13 · 19 · Time: clocks, typed dates, timers, schedules, wait.for

**What.** `ferb-clock` ports `ferb/clock` and the calendar arithmetic and store of
`ferb/orchestrator/schedules.py`. Zones use `chrono-tz` with Python's fold=0 rule; the lookaround regex of
`schedules.py:408` and Python's `random.sample` are hand-written.

**Evidence.** `rust/fixtures/clock/make.py`: 3,789 recurrence cases (every DST change of 2026 and 2027 in 16 zones),
1,676 `when` cases, 273 context cases, the timer, schedule and tool stores, 52 wait scenarios. 26 tests, about
19,000 comparisons, none differ.

**Fixed against Python.**
- A crash between booking the next run and creating its task lost that run (`areas/schedules.py:659-687`):
  `claim_run` and `start_run` hand the run over in one transaction.
- A failure in `next_due_in` ended the timer loop for good; an empty day source crashed on an `assert`; a file first
  seen at monotonic 0.0 never counted as settled; `Timers.add` wrote its rows one by one.

**Open.**
- The built-in tz database is 2025b; Python reads the Mac's (2026f today). A zone whose rules changed between the
  two would give a different time: use the system database or pin and update it. To decide before cutover.
- Patterns a model writes with `regex: true` run on `fancy-regex`, not Python's `re`.
- Timer and schedule labels (they need `names.py`), the daemon handlers, `~user` expansion and Python's
  `local_zone()` fallback are not ported. Nothing was run on real time.

## 2026-11-13 · 20 · Shell: the sandbox profile, PTY, task processes and the egress proxy

**What.** `ferb-shell` ports `ferb/act/shell`: the Seatbelt profile builder, the PTY process with the PID-reuse
defense, the task-owned process manager, the eight `shell.*` tools, the egress proxy, local access and listeners,
and the hand-laid-out libproc socket structs. There is no uncontained mode (rule 11): a manager cannot be built
without a sandbox.

**Evidence.** `rust/fixtures/shell/make.py`: 2,188 checks, none differ, including 210 profile texts byte for byte
and 60 raw proxy exchanges; struct layouts match Python's ctypes field by field. 11 live tests under the real
`sandbox-exec`: writes outside the allowed folders are denied, app programs do not run, FERB's own state is closed,
signals stay inside the command, stop kills the group, network goes only through the proxy. 41 tests pass.

**Fixed against Python.** A nettop port written with a non-decimal digit made `int()` raise out of
`sockets.complete()`; Rust reads it as no listing, which fails closed.

**Open.**
- Unverified: a real sudo admin run (only its argv), `DirectReports` against the real `log stream`, the proxy's
  local-peer route with a real pinned server, other users' sockets.
- Differences: proxy replies carry no `Server:` header; the `log stream` watcher is a fixed `/bin/sh` script, not
  Python (exit 127, not 1, when `log` is missing); a symlink that vanishes while a path resolves is treated as a
  plain path where Python raises.
- Found in Python, mirrored: a head with bare-LF line endings is forwarded with `Connection: close` inserted after
  the body bytes; latin-1 header bytes are re-encoded as UTF-8. To fix under ENG-6.
- It uses doc-hidden helpers of `ferb-sanitize` (`parity::*`); they should move into `ferb-py`.

## 2026-11-13 · 21 · Browser hands, first half (not yet a workspace member)

**What.** `ferb-browser` has the protocol, both HMACs, the seal, pairing (byte-compatible with Python's stored
records), the Hub and Bridge, snapshots and the core of the tab driver, including the sealed fills. The tool verbs
and the remaining operations are being ported by a second agent; the crate stays out of the workspace until then.

**Evidence.** MACs and seals agree byte for byte with Python and with the unchanged extension's `protocol.js` run
under node. 8 transcripts through the Python tests' fake extension, 327 comparisons, none differ. 19 tests pass
(run by the porting agent; not re-run by the coordinator).

**Fixed against Python.** When a connection ends, only the commands sent on it become unknown; Python fails every
command in flight on that profile, including ones already sent on its newer connection.

**Open.** Everything in the second half. Python's `enters_text` and the extension's `entersText` disagree on six
key specs (for example `a+` and `Cmd+V`).

## 2026-11-13 · 22 · Memory: embeddings, search and the import parsers

**What.** `ferb-memory` ports `embeddings.py`, `search.py`, `import_sources.py` and `import_extract.py`, with the
word rules of `items.py` they need. Python 3.12's Unicode, HTML-entity and cp437 tables are generated into the
crate. Store-backed search sits behind the `store` feature; the pure parts build for iOS and Android.

**Evidence.** `rust/fixtures/memory/make.py` (fixed clock and zone, hash seed 0): 5,300 tokenizer cases, 504
embedded texts (vectors bit for bit), 95 store steps, 2,266 search helper cases, 1,698 SQLite comparisons, 30
export files, 384 reader cases; none differ. 10 tests pass, 1 ignored (the real model table, 308 texts; passed
when the porting agent ran it).

**Fixed against Python.**
- `embed_pending` stored vectors for facts deleted while their batch was being encoded, and nothing removed them
  (`search.py:364-409`); Rust rechecks inside the write.
- The Gemini HTML parsers were never closed, so trailing text holding an `&` was dropped
  (`import_sources.py:452,485`).
- `window_start` looped forever in January to March of year 1 (`import_sources.py:87-97`).

**Open.**
- SQLite: `unicode61` tables tokenize and rank identically on the bundled 3.46 and Python's 3.53.4, but the
  **trigram** tokenizer differs on text containing NUL (3.46 stops at it). That reaches `learning_titles`
  (migration 0030). The workspace needs a rusqlite whose bundled SQLite is 3.53.4 or newer before the learning
  port; rusqlite 0.42 exists (0.32 is pinned today).
- No zstd decoder yet: `.zst` Codex rollouts go through the `Unzstd` seam and are skipped without one.
- Zip members compressed with bzip2 or LZMA stop the import (Python reads them); zip64 untested.
- `save` and `said_at` go with the items part.
- The plan's "deletion-generation recheck" did not exist in Python; it is the first fix above.

## 2026-11-13 · 23 · Recordings of the Python daemon

**What.** `rust/fixtures/daemon/make.py` starts the real Python daemon in a fixed temp home with the test suite's
fake model and records raw socket lines both ways. `rpc_transcripts.json`: 12 scenarios calling all 106 owner
methods and the 3 tool-bridge methods, 61 of them also in a way that is refused. `chat_scenarios.json`: 28 chat
scenarios (a plain reply, a clarifying question, plans, confirmations answered Yes and No and stopped while
pending, pause and steer, stop plus a new task, a merged message, a schedule, a wait, a tool failure and repair, a
checker rejection, a restart mid-task) with every frame, all 136 prompts the model received, every gateway call
and the store tables. `README.md` maps it and says how a Rust test replays a scenario.

**Why.** Parts 8 and 12 of the plan are judged against these: the Rust daemon must send the same bytes to FERB.app
and the same prompts to the models.

**Evidence.** Three full runs byte-identical; every scripted answer validated against its schema; 3.6 MB.

**Open.**
- Recorded only as refusals or stand-ins: browser and pairing calls, `ps.*`, some secrets and conversation calls,
  runtime checks (signed out only), real Mac answers (permissions, clipboard, Automation).
- Frame order across connections is not fixed, only within each.
- Things the app relies on that are easy to miss: `ack` always first; `request_id` and `at` are stamped when a
  frame is queued; a merged message's stream is `ack`, `merged`, end; a restart ends open streams with
  `{"ended": true}` and the result comes later through `chat.status`; an unknown method answers
  `-32001 "not allowed for cli"`.
- Found in Python (to fix with the ports that own them): a file's 19-digit `mtime_ns` in a tool result is
  redacted as a card number when it passes the Luhn check, about one time in ten (`act/fs/files.py:51`); about 15
  handlers answer bad parameters with an internal error; the global stop tags FERB's own words "(from the task)";
  the trace stores a redacted copy of the system prompt, not what was sent.

## 2026-11-13 · 24 · Model runtimes and the tool bridge

**What.** `ferb-runtimes` ports `ferb/runtimes` (except `schema_shape`, which is in `ferb-context`) and
`ferb/cli/mcp.py`: Codex over its `app-server` JSON-RPC, Claude without the Python SDK (ENG-2: the same argv,
environment, `initialize` request, control answers and steering by uuid replay that the SDK and `claude_sdk.py`
write together), the Anthropic, OpenAI and Gemini API loop with SSE parsing and retries, the MCP tool bridge with
its token held as a `Secret`, and the runtime catalog.

**Evidence.** `rust/fixtures/runtimes/make.py` drove the Python adapters against scripted programs and local
servers through 619 cases (63 Codex, 25 Claude, 7 bridge, 100 API, about 425 table rows): every byte written and
every event yielded matches; API request bodies are byte-identical. A test with a shell-script `claude` and a
proxy test pass. 8 test groups pass.

**Fixed against Python.** None in the port. Decided here, to apply in the daemon: Python removes only the API key
from a provider's error words before keeping them; the Rust daemon also runs the redactor on them (rule 7). The
hook exists and is off by default so the fixture comparison stays exact.

**Open.**
- Not started: `keys.py`, the Claude sign-in flow (`claude_login.py`), `check.py`, the catalog factories.
- The `claude` program needs its own pin once the SDK is gone (for example the npm package locked like Codex's)
  and a start-time version check; Python only warns. A packaging item (plan part 13).
- `ferb-core`'s runtime trait has the wrong shape for real adapters: it should return what the adapter started,
  and events should carry errors. To change with the supervisor port.
- Differences: a line over 64 MiB is dropped whole (Python processes its tail as a new line); the proxy's 403 has
  an empty body; httpx's own error class names may differ from the three used here; the MCP bridge covers the
  handshake Python's library speaks today, not the newer request-envelope form.
- Never run against the real `codex` or `claude` programs or a provider: that is a run the owner makes.

## 2026-11-13 · 25 · The store bundles SQLite 3.54.1 (rusqlite 0.42)

**What.** The workspace moved from rusqlite 0.32 (libsqlite3-sys 0.30.1, SQLite 3.46.0) to rusqlite 0.42.0
(libsqlite3-sys 0.40.0, SQLite 3.54.1). `ferb-clock`, `ferb-fs` and `ferb-memory` named their own version and now
say 0.42. The only API change: `Statement::column_names()` returns an iterator, not a slice (four call sites).

**Why.** Python's daemon runs SQLite 3.53.4; FTS5's trigram tokenizer differs on text with a NUL (entry 22), which
reaches `learning_titles` (migration 0030).

**Evidence.** `ferb-store/tests/sqlite_version.rs` asserts the linked version is at least 3.53.4 and that the
trigram tokenizer skips a NUL and reads the text after it. `ferb-memory`'s FTS5 comparison no longer excepts
NUL cases: 0 of its 1,698 comparisons differ. No other fixture comparison changed (stored schema text and rows
match as before). Workspace: 311 tests pass, 4 ignored; fmt, clippy, `gate.sh portable` pass.

**Open.**
- Excluded crates `ferb-policy` and `ferb-browser` still name `rusqlite = "0.32"` (two lines each); they must move
  to 0.42 before they join, or two SQLite copies link.

## 2026-11-13 · 26 · Policy: when FERB asks, and what is protected

**What.** `ferb-policy` ports `ferb/policy` (4,666 lines): `decide`, the URL and form sink rule, the question
wording, provenance, known and approved destinations, confirmations, standing sends, payments, terminal input,
certificate checks, the protection lists and stored provenance. It carries Python-exact urllib, ipaddress, idna,
shlex and Unicode normalization from tables the Python build writes. It builds for iOS and Android; the stores are
Mac-side.

**Why.** These are the decisions about when FERB must ask before acting (rules 3 to 5). A drift here is a safety
bug, so the cases were harvested from the Python guard tests themselves.

**Evidence.** `rust/fixtures/policy/make.py` runs 455 guarding tests to collect inputs, then generated and
scripted cases: 10,769 in all, none differ, rows byte-identical. Scripted cases cover known versus approved
destinations, per-tree answers (the latest wins, an explicit No, one-run use), the terminal-app grant and its
exceptions (GATE-585/586), standing sends, payment amount and merchant changes, and the routine 30-minute window
and what it never covers. 16 test groups pass, re-run by the coordinator.

**Fixed against Python.** `tls.py:344-345`: `yarn config set enableStrictSsl` with no value raised IndexError.

**Open.**
- How answers combine (first answer wins, window expiry) lives in `gateway.py` and `questions.py`: the gateway port.
- Unverified: `targets::Requests`, `Contact`'s pydantic schema, `with_forms`, normalization beyond the sampled
  strings.
- Differences that fail closed or are listed: NaN amounts give text where Python raises; a regex backtrack stop
  reads as no further match; a redactor that cannot read its text counts as a hit.

## 2026-11-13 · 27 · Traces

**What.** `ferb-trace` ports `ferb/trace` (compose, lines, render, timing) and the trace output of
`ferb/cli/admin.py` (the span tree, `--full`, `--json`, the story). It is pure and portable: it takes the
`trace.get` shape as decoded JSON, which `ferb-store`'s `SpanStore::tree_json` produces.

**Evidence.** `rust/fixtures/trace/make.py` builds trees through the real Python span store and adds the 28
recorded chat scenarios: 71 trace cases, 59 story cases, 997 helper cases; 1,681 comparisons, byte-exact, none
differ. Whole workspace after it joined, run by the coordinator: 340 tests pass, 4 ignored; clippy clean; the
portable check passes.

**Fixed against Python.** `ferb trace` raised AttributeError (`admin.py:80`) on a span result that is JSON but not
an object (a number, a list, null, a string); Rust shows it as plain text.

**Also.** `serde_json`'s `float_roundtrip` feature is now on for the whole workspace: without it a recorded float
such as a cost can read back one digit off from what Python reads.

**Open.** A time that cannot be parsed shows `--:--:--` where Python raises; integers beyond 64 bits and NaN inside
a planner's result text are not read as Python reads them.

## 2026-11-13 · 28 · Browser hands, second half: the page operations and both tool families

**What.** `ferb-browser` is now a workspace member. Added to entry 21's protocol, pairing and bridge: every driver
operation (act with its structural facts, find, html, scroll, media, wait, web terminals, screenshots and their
masking, pdf, owner view, both upload routes, the OTP inbox, open tabs, cleanup leftovers), all 46 tools of
`browser.*` and `ferb_browser.*` with typed arguments and what each declares to the gateway, profiles, the
resolver and the install requests.

**Evidence.** New fixtures: 11 operation scenarios (303 results, 281 daemon frames), 1,130 key specs run through
the unchanged extension's `protocol.js` under node, money, mask and mime cases. Every tool is checked against the
frozen catalog (name, description including the ones that append saved secret names, scopes, facts). 27 tests
pass, re-run by the coordinator; nothing differs from Python except the three fixes below.

**Fixed against Python.**
- An older connection ending no longer publishes `browser.disconnected` while the profile's newer one is up.
- A hello with a version such as "0.19.²" is refused as too old instead of ending the connection.
- `enters_text` follows the extension, the unchanged other end: Python disagreed with it on 155 of 1,130 key specs
  (for example `Cmd+V`, `a+`, `é`).

**Open.**
- Not started: `downloads.py`'s `Downloads` (transfers, budgets, the disk floor, the over-cap question, sweep) and
  `uploads.py`'s staging; the macOS parts of `profiles.py`. They reach the drivers only through seams.
- Unverified tool runs: pdf (both routes), record, the success paths of pair and handoff, the `point` tool's
  display mapping.
- The upload package's zip bytes change between Python runs (a timestamp), so names, order and contents are
  compared instead of bytes.
- `ferb-identity`'s mailbox draws its tokens from system randomness, which a test cannot inject.
- Never run against a real browser.

## 2026-11-13 · 29 · Device setup and the internal commands FERB.app calls

**What.** `ferb-device` ports `ferb/device` (install, profile, scan, persona, devtools, extensions, the self-test's
words, the grant probe) and `ferb/cli` (the argument parser, the socket client, every `setup` path, `uninstall`,
sign-ins, the clean-up, `doctor`, the toolbox's daemon clients). A plist writer reproduces Python's
`plistlib.dumps` exactly. The one named difference is `Programs`: in Python mode it writes Python's bytes; in Rust
mode it names `ferbd` and `ferb-native-host`.

**Why.** FERB.app runs these commands, reads `service.json` and compares the plist's SHA-256 (DMN-514). The app is
unchanged in stage 1, so every command line, exit status, printed line and file must stay as it is.

**Evidence.** `rust/fixtures/device/make.py` drives the Python with the test suite's fakes in a fixed temp root
and records every file written, every external command it would run, every daemon call, printed line and exit
status: 55 scenarios, 2,457 events, 52 command lines, 35 toolbox runs; none differ. 18 tests pass, re-run by the
coordinator. No test runs `launchctl` or writes outside a temp folder.

**Fixed against Python.** An app whose Info.plist is not a dictionary raised AttributeError and broke the whole
software scan; Rust skips that app.

**Open.**
- No `ferb` program yet: its host needs parts that are not ported. Not started: the `trace`, `stats`, `runtimes`
  and dashboard commands' wiring (the trace output itself is in `ferb-trace`).
- **FERB.app tells a development checkout's service from its own by the arguments `-P -m ferb.daemon`**
  (`AppSetupPlan.isCheckout`). A Rust checkout's plist has no such arguments, so the app would replace it without
  asking. Stage 1 keeps the app unchanged: the Rust plist needs a marker the app already recognizes, or the app
  needs a one-line change. To settle in part 12.
- Moving the old `[setup.persona_pending]` sub-table is refused by `ferb-core`'s config writer (one of its ten
  refused shapes), so the old keys stay in `config.toml`.
- argparse's own usage and error text is not reproduced (the exit status is).
- Found in Python, mirrored because it is owner-facing wording: `setup --codex-logout` with no Codex installed
  prints its reason twice ("FERB's pinned Codex isn't installed: FERB's pinned Codex isn't installed: ...").

## 2026-11-13 · 30 · Perception

**What.** `ferb-sense` ports `ferb/sense`: the trail and its receipts, exclusions applied before anything is kept,
attribution, capture, today's activity, the watcher manager, the trail tools, the front window, software facts,
the daemon side of transcription and every watcher's mapping. The macOS sources (FSEvents, a listen-only event
tap, NSWorkspace notifications, AX observers on their own run-loop thread, the pasteboard, the secure-input check)
are in a `mac` module; each thread and callback drains its own autorelease pool. Jev's batcher is ported and stays
switched off; notices stay paused.

**Evidence.** `rust/fixtures/sense/make.py`: 969 comparisons across 13 areas, none differ. Behavior tests from
the Python scenarios: key text is never extracted under Secure Input, in a secure field or in a terminal; the tap
starts and stops with the Input Monitoring grant; a pause survives a restart; notifications are not read until
Full Disk Access. 23 tests pass, 2 ignored, re-run by the coordinator. The pure half builds for iOS.

**Fixed against Python.** `Path.resolve` raised on a symlink loop out of exclusions and the trail; a launch-job
plist whose root is a list failed the software watcher.

**Open.**
- All native code is unverified on an installed build (AX observers, the tap's key text, notifications, window
  list, pasteboard reads). The porting agent ran two passive read-only tests by hand; no prompts appeared.
- Composing the watchers is the daemon part.
- The plan's "recovery after an event-bus overflow" does not exist in `ferb/sense`; its consumers are synchronous.
- `ferb-clock::py::timedelta_us` overflows and panics in a debug build on very large values; to fix in the clock
  crate.

## 2026-11-13 · 31 · The Rust release is a fresh install (owner, STORE-421)

**What the owner said.** Asked whether the Rust release may require a fresh install with the old data kept only as
an archive: "yes ofc , tahts what i want". Only the owner and one friend run the Python build.

**What changes.**
- Not built: upgrade tests on real older databases for the Rust store (T-400 and T-403 stay as they are, for the
  Python build), the service and login-item moves from a Python install, config from older layouts.
- Unchanged: the schema and migration files (ENG-4), so the port stays checkable against Python and an archive
  stays readable.
- FERB.app may change with the Rust release. So the finding in entry 29 (the app recognizes a checkout's service by
  Python arguments) is closed by a one-line change in the app, not by a marker in the Rust service definition.
  ENG-1's "FERB.app unchanged" stays the rule for verifying the port, not a rule for the release.

## 2026-11-13 · 32 · Memory: items, profile, learning, distill, playbooks, the import job

**What.** `ferb-learning` ports the rest of `ferb/memory`: the shared save path and forget (`items.py`), the core
profile, learned nodes and episode cards, the daily summary and distill, playbooks, the Mac profile, the import
job, the journal, questions, export, the upgrade steps and the `memory.learned` tool. Model calls go through the
runtime trait with prompts composed byte for byte. The pure parts build for iOS and Android.

**Evidence.** `rust/fixtures/learning/make.py` runs the Python code in a temp home with a fake clock, sequential
ids and a scripted model: 15 scenarios (536 steps) comparing returns, events, composed prompts and every changed
table; 1,167 pure cases; the distill failing at each of its 28 writes and run again; the import failing at each of
its 116 writes and resumed. None differ apart from the fix. 8 test groups pass, re-run by the coordinator.

**Fixed against Python.** Import writes are atomic (`import_job.py:186-190`): a chunk's source row, saved facts,
done marks and progress commit together, and so do a run's closing writes. A restart at any write ends exactly as
the uninterrupted run does. Python's own restarts lost or doubled work in 64 of the 116 cases.

**Open.**
- `card_reader.py` (the separate reader process with its home check, MEM-418) is not ported. The Rust daemon can
  read cards on a read-only connection; whether a separate process is still wanted is decided with the daemon.
- Not checked: the dashboard's request rules, the embedder path of `Learning.search`, the daemon's redactor walk,
  the runtime's `retrying` event.
- Jev's "worth keeping" branch in distill is left out (Jev is off).
- Where Python iterates a set, Rust uses sorted order (the `on_joined` id list, `rollback` and `due` updates).
- The runtime trait has no `limits` or `model`; `calls::Made` carries them (to fold into `ferb-core` with the
  supervisor port).
- T-1044 (the owner's open question on retry limits) is untouched: the hourly retry and the two-failure card rule
  are ported as they are.

## 2026-11-13 · 33 · Orchestration's building blocks

**What.** `ferb-orchestrator` ports everything in `ferb/orchestrator` except `supervisor.py` and `worker.py`, plus
`ferb/decide`: the answer models (`SupervisorOutcome`, `TaskSpec`, `WorkerReport`, `CheckerOutcome`) with
pydantic's validation and exact error text; plans, the plan store and executor, parts, the task tree, the ledger,
continuity, conversations; loops and steering; evidence, verification, cards, attention, episodes; answer marks,
the reply stream, progress lines, probes. Everything owned by another part is a trait in `ports.rs`. The crate
builds for iOS; store-backed modules sit behind a feature.

**Why.** These bytes reach models, the app and the store (ENG-3), and the supervisor port is built on them.

**Evidence.** `rust/fixtures/orchestrator`: 5,372 model answers, valid and invalid (1,222 for `SupervisorOutcome`,
1,000 each for the other three), 2,803 composed-text cases, 513 store comparisons; none differ. 6 test groups pass,
re-run by the coordinator.

**Fixed against Python.** `probes.field_value` on a path with a non-decimal digit (`media.²`) raised ValueError
and crashed the probe read; Rust reads it as missing, so the probe trips.

**Open.**
- Not started: `MemoryRead` (now possible: the memory items part landed in entry 32); `steering.apply` as one
  function and `ExecutionState` (they are supervisor and gateway state).
- Unverified: `guards`, `stop`, steering's helpers, the evidence-capture path of `task.evidence`,
  `episodes::record`, the real fused search of `chat.history`.
- For the supervisor port: some malformed legacy answers raise TypeError or AttributeError in Python and escape the
  supervisor's `except ValidationError`. Rust marks them `raised()`; they must be handled as Python's crash path
  is, or fixed by name under ENG-6, not treated as an ordinary repair.
- Policy is a seam here; `ferb-policy` has no `site_key`, which the tests supply themselves.
- Helpers copied from `ferb-trace` and `ferb-sanitize` belong in `ferb-py`.

## 2026-11-13 · 34 · The gateway, first half (not yet a workspace member)

**What.** `ferb-gateway`: the tool catalog served from the frozen schemas, pydantic-exact argument checks,
admission, pending questions, ordered locks, stop, and the full `Gateway::call` pipeline on the store: the needs,
authorize (first answer wins, an explicit No is final, the 30-minute window, standing scopes, one-run Yeses), the
question cards, `owner.ask`, the undelivered result, the URL and form sinks, refused fills, tab-chain payments and
sending a missing amount back once. One `Tool` trait and registry, with adapters for `ferb-fs` and `ferb-clock` as
worked examples.

**Evidence.** All 23 listings and 97 tool rows byte for byte against the frozen catalog; 627 validation replies
plus 5 pre-validation refusals; `rust/fixtures/gateway/make.py` runs 45 scenarios through the real Python
`Gateway`, comparing every reply, event, question card, journal entry and table: none differ. 8 recorded daemon
file calls replay identically. 19 tests pass (run by the porting agent; the crate stays outside the workspace
until its second half lands).

**Fixed against Python.**
- `browser.pdf`, `ferb_browser.pdf` and `audio.listen` left a `ValueError` in the reply so it could not be
  serialized; the reply is now the error's text.
- A file's `mtime_ns` was redacted as a card number about one time in ten: file tools now declare the numbers they
  measured themselves, and the gateway leaves numbers at those places out of redaction. Text in the same result is
  still redacted. The rule is structural (where the number came from), not a list of field names.

**Open (a second agent is on these).** Hidden input (`hidden_input.py`) is not ported. No fixture is behind: the
escape hatch, the terminal's one Return, GATE-580's press-time re-check, pay pictures, probes,
`destination_allowed`. The real call hooks are a no-op seam. Redaction marks are not carried, so FERB's own text
may be redacted twice. The scenario test uses a fixed folder under `/tmp`.

## 2026-11-13 · 35 · Mac hands

**What.** `ferb-mac-hands` ports `ferb/act/mac`, `listen.py`, `recording.py`, `point.py`, the locks, the
after-action read, evidence, `ferb/device/permissions.py`, autorelease and `ferb/sanitize/images.py`: the 17
`mac.*`, `audio.listen` and `recording.*` tools. Pure logic builds anywhere; Apple calls are in `native/` (AX,
window list, ScreenCaptureKit, Vision, CGEvent and the optional SkyLight route by `dlopen`, pasteboard,
permission probes by raw FFI), each `unsafe` with its reason.

**Evidence.** About 600 pure cases (key combos, gestures, window and page masks, OCR region rules, the AX walk)
and 8 driver scenarios on the Python tests' own fakes (108 steps compared): none differ. Declarations match the
frozen catalog for all 17 tools. 20 tests pass, re-run by the coordinator.

**Fixed against Python.** `type_steps` gave a code-point count as the UTF-16 length, so a character outside the
BMP (an emoji) was sent as a lone surrogate; Rust sends the whole character.

**Open.**
- **The native layer has only been compiled.** It has never run against a real app. Four read-only tests exist and
  were not run (one reads the TCC database outside a temp folder). Real clicks, keys, captures and permissions are
  proven only by the owner on an installed build.
- Not fixture-checked: the run paths of `mac.open`, `mac.applescript`, `mac.secret`, `mac.record`,
  `mac.clipboard`, `audio.listen`, `recording.stop`/`inspect`.
- Not started: the native run-loop threads for AX observers and event taps here (perception has its own),
  `record_done`, `grant_target`, `open_pane`.
- Differences: the recording review sheet draws no time labels; a window name in Unicode digits returns nothing
  where Python raises; `accessBehavior` is not guarded for macOS older than 15.4.

## 2026-11-13 · 36 · Browser hands: downloads, upload copies, the Mac profile lookups

**What.** `ferb-browser` adds `downloads` (transfers, the per-tree budget shared with the shell, the disk floor,
the over-cap fact, sweep, cleanup), `staging` (upload copies through folder handles, with the source's identity
checked so a file swapped mid-copy is refused) and `macos` (Launch Services, `pgrep`, free space). pdf, record,
pair, handoff and the cursor point are now verified at the tool level. `ferb-identity`'s mailbox takes an injected
random source, so tokens match Python in tests.

**Evidence.** New fixtures: 21 upload scenarios (133 steps, including the link and folder-swap races), 18
download scenarios (245 steps), a 41-action tool transcript, Mac profile lookups on a fake platform. None differ
except the named fix. 36 browser tests pass. Whole workspace, run by the coordinator: 452 tests pass, 11 ignored;
format and clippy clean; the portable check passes.

**Fixed against Python.** In the unsupported-browser scan, an Info.plist holding a number where a list belongs
raised TypeError and broke the whole scan.

**Open.**
- The over-cap question itself (its text and the rule for asking again) is the daemon's code and not ported; only
  the fact is.
- Two tabs downloading at once: the order of calls and events is compared as a set, since the two schedulers
  differ; rows and outcome match exactly.
- The fixture folder is 3.85 MB. Never run against a real browser; `launch_detached` has never run.

**Process note.** This follow-up worked inside a crate that was already a workspace member, and its half-edited
files failed the tree-wide format check, which held four finished ports out of a commit for about an hour. A
follow-up on a member crate now works on a copy, or the crate leaves `members` while it is edited.

## 2026-11-13 · 37 · The worker

**What.** `ferb-worker` ports `ferb/orchestrator/worker.py`: one attempt (worker, checker, fix, re-check), the
shared model call with its silence watchdog and retry rules, every event kind, steering and stop, the failure
classes and the owner's words for each, usage where unknown cost stays unknown, and the live lines. Portable. Its
`ModelRuntime` / `EventSource` / `RunControl` traits are the runtime interface real adapters need (start returns
the run with its event stream and control; errors arrive as items): `ferb-runtimes`' adapters map onto it one for
one, and `ferb-core`'s older trait should give way to it.

**Evidence.** `rust/fixtures/worker/make.py` runs the real Python worker on the tests' fake runtime with recording
fakes for every seam: 69 attempts, 14 calls, 15 tool lines, 36 failure reasons; 2,815 comparisons, every seam call
in order, none differ. All 80 worker and checker requests recorded in `chat_scenarios.json` match (system text,
parts, tool names, schema); 6 of them up to an 8-byte step, because the daemon recorder masked two forms of the
temp root as one. A one-word mutation produced 12 differences, so the comparison does catch changes. 6 test groups
pass, re-run by the coordinator.

**Fixed against Python.** None.

**Open.**
- The glue from `ferb-runtimes`' adapters to `ModelRuntime` is unwritten (the daemon part).
- `deliver` is implemented here again because `ferb-orchestrator`'s version makes futures non-Send; one of the two
  should go.
- Cleanup when an attempt's future is dropped skips the steering and timing writes; stopping through the tree's
  stop words is the intended path.
- If the redactor raises inside the owner's words for a failure, Python propagates and Rust shows empty words.

## 2026-11-13 · 38 · The supervisor, first pass: what it composes (not yet a workspace member)

**What.** `ferb-supervisor` has everything `supervisor.py` composes for a model or the owner: every role's
instructions, the planner's context (conversation, live facts, tasks, open questions, the latest result, what
happened since), a turn's payload, the worker's and checker's context, the checker's report, the fix and re-check
blocks, the owner-facing lines and the chat line's arrival order. The flows answer `NotPorted` by name; a second
agent is porting the path from a message's arrival to its result.

**Evidence.** 32 of 32 frozen instruction texts; all 118 recorded model calls (system text and parts); 38 planner
calls, 37 turn payloads, 78 worker and checker contexts, 36 checker reports, 328 unit cases; none differ. (The
daemon recordings hold 118 model calls; entry 23 said 136, which counted 18 in the RPC scenarios.) Run by the
porting agent; the crate is outside the workspace until the flows land, and is not committed yet.

**Open.** No scenario replays end to end: frames, gateway calls and tables need the flows. Pause, steer, stop,
suspend, resume, restart, scheduled runs and waits come after the forward path.

## 2026-11-13 · 39 · The gateway, second half: it joins the workspace

**What.** `ferb-gateway` gains hidden input (a one-time secret, a saved one, by handle, login codes, the Mac admin
password for the escape hatch): the answer is taken out of the question once and wiped, and a cleanup that runs
even when the waiter leaves closes the question as used, declined or set aside. Also the real call hooks, FERB's
own words carried with the redactor's mark to the span store and events, a cancelled call's measured facts kept as
Python keeps them, and the real orchestrator ledger behind the `Ledger` seam with restart and reconcile.

**Why.** Rules 5 to 7: this is the one path every model tool call takes.

**Evidence.** `rust/fixtures/gateway`: 71 scenarios with 256 steps from the real Python `Gateway` (26 new), a
canary value checked against every stored byte in the hidden-input scenarios, and 25 hook cases; none differ.
Removing the marks, the cancel-time facts or the answer wipe each fails the right scenarios. Whole workspace
clippy clean; 23 gateway tests pass, re-run by the coordinator; the crate builds for iOS and is in `members`.

**Fixed against Python.** None new (entry 34's two stand).

**Open.**
- `Approval.answer` while a question is pending and `EscapeGrant.secret` are plain strings, not zeroized `Secret`s.
- A hidden input whose caller left reads the task's newest in-flight call; Python reads the calling context.
- `ferb-browser`'s `OwnerInput` seam passes (task, intent, origin) but the gateway needs the call's context: the
  daemon keeps it when wiring the two.
- The scenario test's per-run folder is under `/tmp` with a fixed length, because FERB clips long lines and a
  different path length clips them elsewhere.
- The real hooks are checked against fakes; the parts they drive belong to other crates.

## 2026-11-13 · 40 · The daemon's handlers (71 of 106 methods), and a recorder that opened a real browser

**What.** `ferb-daemon` holds the RPC handlers over the ported engines, each behind a `Deps` struct of seams.
- Group A, 26 methods: memory, memory import, schedules, reminders and timers, chat history, settings, retention,
  records; plus `shared` (Python's helpers and the one loop thread that holds the engines, as Python's live on one
  asyncio loop).
- Group B, 45 methods: browsers and pairing, runtimes with keys, checks and the Claude sign-in, about, seen,
  sensing, Jev (off: nothing reads a key), notices, intro, voice, egress, failure reports, the task preview, the
  disturbance sampler, environment, compose and desktop.

**Evidence.** Replayed against `rpc_transcripts.json`, the lines recorded from the real Python daemon: group A's 26
methods over 106 lines (18 byte for byte, 8 differing only by named fixes) plus 63 more lines from
`fixtures/daemon-a`; group B's 45 methods plus about 150 scenarios and 1,100 library comparisons from
`fixtures/daemon-b`. A crash before a scheduled run's task is recorded leads to redelivery and the run happens
once. 56 test groups pass, re-run by the coordinator; whole-workspace clippy clean.

**Fixed against Python.** About 20 handlers answered bad parameters with an internal error (-32603) and now refuse
in plain words (missing keys in `memory.*`, `schedules.change`, `reminders.cancel`; a bad `since`, `limit` or
`timeout_s`; `attention.ack` with an unknown id). `schedules.change` with an unknown action wrote a bogus
`schedule.<action>` journal row before crashing; Rust refuses before writing. `runtimes.check` answers
`ok: false` when the adapter's stream raises. A frozen preview frame keeps its `seq` and `age_ms`.

**What went wrong.** The browsers recorder (`fixtures/daemon-b/browsers.py`) opened the owner's real Brave three
times at `chrome://extensions` in the Default profile. `profiles.open_known_profile` binds the real launcher as a
default argument at import, so the stub on the launcher function never reached it. The recorder now stubs the
caller; `fixtures/daemon/make.py` had the same gap and now replaces the bound default (its fixtures re-ran byte
for byte). `AGENT-BRIEF.md` tells every recorder to find and replace each launch point first and to report
anything real it starts. The owner was told and answered that testing in real apps is fine as long as it is
reported.

**Open.**
- Not ported: `chat.*` and the task handlers (they need the supervisor), the composition (`main.py`'s
  constructors, the runtime factory wiring the adapters, a macOS desktop probe), schedule creation and editing
  from chat, the hourly memory loops, sleep and wait glue, the resource sampler.
- Verified only on refusal paths: `ps.*`, `secrets.saved`/`delete`, the notifications stream's delivery.
- Unverified: retention pass, persona with a real scan, the relay over https.

## 2026-11-13 · 41 · The supervisor's forward path (not yet a workspace member)

**What.** `ferb-supervisor::flow`: a message's path from arrival to result, on the real store and `ferb-worker`.
Intake with `ack` first, the planner call and its repair, routing (reply, clarify and its answer, a merged
message, one task, a plan), the plan event and step states, the worker attempt loop, the checker with accept,
fail, and findings with fix and re-check, the task's end, its episode record and result row.

**Evidence.** A replay harness runs all 28 recorded chat scenarios on a temporary store with a scripted runtime
and gateway: 1,964 comparisons. **9 replay fully** (every frame, model call and compared table matches the Python
daemon): plain_reply, clarify_then_answer, one_step_task, multi_step_plan, plan_repair, checker_rejects,
checker_findings_fix, second_message_merged, attachment_and_bad_params. Run by the porting agent; the crate stays
outside the workspace while the remaining paths are ported.

**The other 19 stop here.**
- At tool calls through the bridge to the gateway (5): one_step_with_tools, checker_skips_change_records,
  tool_failure_then_retry, wait_for_time. The gateway's records and `tool.called` frames are not replayed, so ids
  drift.
- At a question or approval for the owner (4): confirm_yes, confirm_no, confirm_stop_pending,
  worker_question_answered.
- At paths not ported (10): check_in_after_five; pause_steer_resume, steer_running_task, stop_by_chat,
  stop_plus_new_task; stop_by_task_stop, global_stop; restart_mid_task, restart_with_open_question;
  timer_set_and_fired.
- At the daemon's own RPCs (1): schedule_created_and_fired.

**Fixed against Python.** A malformed legacy planner answer whose validator raised TypeError (a `map` step whose
`inputs` or `batches` is not a list; a legacy `schedule` beside a non-list `schedules`) failed the whole turn; it
is now repaired like any invalid answer.

**Open.** Control (pause, steer, stop), waits for the owner, check-in, service-down, side entries, the summary
fold, several asks (the card), loops, conversations, wait and ask steps, scheduled runs, resume, restart: each
answers `NotPorted` by its Python name. Bus events the daemon merges into a stream are not merged here.

## 2026-11-13 · 42 · The daemon program runs (not yet a workspace member)

**What.** `ferbd` composes the crates into one program with three modes: `daemon`, `mcp` (the tool bridge the
model programs talk to) and the internal commands of `ferb-device`. It opens the store with salvage and config
upgrades, builds the redactor, journal, ledger and gateway, serves the RPC socket with the peer lookup and the
FERB.app check, takes the native-host channel, runs the timer loop and hourly retention, and builds the runtime
factory with its glue to `ferb-worker`'s `ModelRuntime`. Home rule: inside a `.app` only the account's own
`~/.ferb`; anywhere else `FERB_HOME` is honored.

**Evidence.** Tests start the real binary on temp homes: 69 of the 71 ported methods replay through it against
the recorded Python daemon (byte-exact or the named fixes); the catalog's 97 tools and 23 listings match by hash;
`ferbd mcp` lists a run token's tools; SIGTERM exits 0 and removes the socket; a second daemon on the same home
refuses; the stop marker and unknown-outcome marking survive a restart; the unchanged Python client gets a real
`fs.list` through the gateway. 18 tests pass (run by the porting agent). Test stand-ins sit behind a feature only
test builds enable; a plain build holds no trace of it (`strings`).

**Fixed against Python.** An exclusive `run/daemon.lock`: two simultaneous starts could both pass the socket check.

**Ran on this Mac.** Only `ferbd` on temp homes under `/tmp`. One smoke-test daemon outlived its shell (SIGHUP is
not handled); it was found, stopped and its folders removed. Checked by the coordinator: no `ferbd` process and no
temp home remain.

**Open.**
- Behind a "not available in this build" refusal: chat and task methods (the supervisor is not wired); the tools
  `mac.*`, `recording.*`, `audio.listen`, `fs.look`, `recovery.snapshot`, `abilities.*`, `trail.*`,
  `task.evidence`, `wait.for`, `sleep_until`, `skills.read`, `chat.history`, the memory tools, `secrets.list`,
  `about.read`, `conversation.read` and the 21 `ferb_browser.*`; the device setup environment; the egress proxy;
  the hourly memory loops; the resource sampler.
- Not ported at all: 19 RPC methods (`doctor.check`, `setup.permissions`, `reports.*`, `recovery.*`, `trace.get`,
  `verdict.set` and others) and the `logs.subscribe` stream.
- Seams that did not fit and were adapted in `ferbd` instead of fixed at the source: `ferb-gateway`'s `call`
  future is not Send, and it has no public way to ask FERB's own questions; nothing implements `ferb-shell`'s
  `Protection`; two `Shape` types (`ferb-runtimes`, `ferb-context`); `ferb-worker`'s `Error` cannot carry a
  provider error's overflow, account and image facts; `ferb-browser`'s `OwnerInput` lacks the call context;
  `ferb-core`'s old runtime trait; `ferb-device`'s installer needs the Rust service definition; the catalog's MCP
  listing envelope disagrees with the bridge's wire fixture.
- `ferbd` should handle SIGHUP.

## 2026-11-13 · 43 · The supervisor: control, recovery and the real gateway (not yet a workspace member)

**What.** Two agents in the same crate, each on its own files.
- Control and recovery: `flow::control` (stop, pause, continue and retry from a planner answer, alone or together
  with new work, ORCH-638; `task.stop`, where an empty list is refused; the global stop; steering; a correction
  typed on a card), `flow::owner_wait` (the one wait for the owner, the check-in, service-down), `flow::restart`,
  `flow::scheduled`.
- The rest of the forward path: the real `ferb-gateway` joined in place of the scripted one (`watch.rs`), so spans,
  ledger rows, ids and `tool.called` frames come from real code; change records from `ferb-fs` feed the checker;
  the bus merged into the chat stream in Python's wake order; the chat RPCs (`chat_rpc.rs`); a waiting worker's
  live line; timers set from a planner answer.

**Evidence.** `tests/replay.rs`: **23 of the 28 recorded chat scenarios replay fully**, 2,209 comparisons (9 before).
`tests/control_scenarios.rs`: 10 more recorded control scenarios, 8 full. The crate passes format, clippy and its
tests (run by the porting agents).

**Not yet replaying.** restart_mid_task and restart_with_open_question (restart is not replayed; `tree.rs` needs
resume support); schedule_created_and_fired (a schedule made in chat is not ported); timer_set_and_fired (stops
at the daemon's timer delivery); global_stop (needs the daemon's `stop.engage`).

**Fixed against Python.** GATE-581: the global stop's summary is FERB's own words, not marked "(from the task)".

**Open.** The summary fold, several asks and their card, checklist plans, loops, conversations, wait and ask
steps, `route_question`, memory and schedule side entries, `_resume`; unverified chat handlers (`chat.correct`,
`chat.attach`, `results.ack`); the chat methods' recorded lines are not replayed yet. `Gateway::call` is not Send
and the gateway cannot take FERB's own questions (being fixed at the source). The fixture folder is about 3.8 MB.

## 2026-11-13 · 44 · Seams fixed at their source

**What.** The interfaces `ferbd` papered over with adapters (entry 42) are fixed in the crates that own them, each
additively (old paths keep compiling; nothing removed).
- `ferb-gateway`: the call pipeline's future is `Send` (the policy's `Facts`, which holds plain `&dyn Fn` lookups,
  was held across the counting await; it now lives in a block before it), so no dedicated call thread is needed.
  FERB's own questions outside a task (`areas/memory.py` `_ask_owner`): `Gateway::ask_own(&OwnQuestion)`,
  `await_own`, `waiting`, `close_own`. A hidden input's typed answer is moved at `resolve` into a zeroizing
  `Secret` (`Approvals::resolve_sealed` / `take_sealed`), never left in the question's plain `answer`.
- One `Secret` type, in `ferb-py` (`ferb_py::secret`), re-exported as `ferb_identity::Secret`; `EscapeGrant.secret`
  holds it (`Option<Secret>`; `Clone` duplicates, `PartialEq` compares values).
- `ferb-shell`: `policy_lists::PolicyProtection` implements its `Protection` seam over ferb-policy's lists (feature
  `policy`, default; the shell owns the seam and may see the policy crate, not the reverse). `tools::declared`,
  `tools::describe`, `tools::run` and `TOOLS` give the eight tools' facts and runs; `ferb_gateway::adapters::shell::
  ShellTool` puts them behind the gateway's `Tool` trait.
- One `Shape` (ferb-context's); `ferb_runtimes::forms::{Shape, SchemaTooComplex}` re-export it and
  `forms::ContextForms` is the `Forms` seam over ferb-context.
- `ferb_worker::Error::Adapter(AdapterError)` carries a runtime error's class (`ProtocolError`, `AccountError`,
  `ProviderError`) with `overflow`, `account`, `image`, `empty`, `status`, `reason`, `code`; `account()`,
  `overflow()`, `image()`, `runtime()`, `is_runtime_unavailable()` read them as Python's `getattr` does.
  `ferb_runtimes::worker`: `From<RuntimeError> for ferb_worker::Error`, `WorkerRuntime`, `WorkerEvents`,
  `WorkerControl` (the glue entry 37 left unwritten).
- `ferb-browser`: `OwnerInput::ask_code_for` / `ask_secret_for` carry the call's whole `ToolContext` (defaults
  forward to the old methods); the tools call `owner_code_for` / `owner_secret_for`. `Tools::owner_family` and
  `Tools::ferb_family` build each family with its driver; `KeychainSecrets` puts FERB's configured secrets behind the
  FERB-identity driver.
- `ferb-device`: `RustPrograms::ferbd(built)` (`ferbd daemon`, the native host beside it) and a checkout install of
  the Rust programs (refused before any change while a binary isn't built).
- `ferb-learning`: `Made::from_model` and `MakeModelRuntime` take a worker `ModelRuntime` (its limits and model read
  from it).
- Times: `ferb_py::time::DateTime` is the stored and wire representation (it holds everything Python reads and
  writes); `ferb_core::when::{to_py, from_py}` convert losslessly or refuse by name (`NotRepresentable`).
- `ferb-py` now holds `text` (`is_space`, `is_digit`, `is_alnum`, `is_printable`, `lower`, `str_repr`,
  `bytes_repr`, `shorten`) and `url` (`hostname`), with their tables from `rust/fixtures/py/make_unicode.py`;
  ferb-sanitize, ferb-context, ferb-trace and ferb-orchestrator re-export them, and sanitize's generator no longer
  writes the six moved tables (re-run: the same bytes as the hand edit). `ferb_clock::py::timedelta_us` answers
  Python's `OverflowError` for a total past i128 instead of panicking, and a total past i64 microseconds (legal in
  Python, but no date can take it) is `date value out of range` instead of a silently wrapped number.

**The catalog's MCP listing was the wrong one.** `rust/fixtures/catalog/tools.json` recorded `Shim.list_tools`'s
`model_dump`, which still has `ttlMs`, `cacheScope` and `resultType`; mcp 2.2.0's `ServerRunner` sieves a spec
method's result by the connection's protocol version, so a 2025-06-18 client (Codex, Claude Code) receives
`{"tools": [...]}` only, as `rust/fixtures/runtimes/mcp.json` (a real run of `cli/mcp.py`) shows. `make.py` now
records the sieved form (`--only tools.json` regenerated that file alone; only `mcp_tools_list` changed). The Rust
bridge already wrote the wire form.

**Evidence.** Workspace format, clippy (`-D warnings`) and tests pass; `ferbd` and `ferb-supervisor` (outside the
workspace) pass clippy unchanged. New tests: the call future's `Send` (compile-time), FERB's own questions
asked, answered, set aside and cancelled; a hidden answer sealed and taken once; the shell's facts and `describe`
against the catalog; `PolicyProtection` against the policy lists; the runtime error conversion and steering; the
browser families and the old `OwnerInput` still answering; a checkout install of `ferbd` with a recording runner,
and Python's `bundle_install` scenario replayed with the Rust programs (same commands, same files, only the
definition, its hash and the launcher differ); the datetime conversions on the core and store fixtures' times;
Python's whitespace table equal to `is_py_space` on every character.

**Not done.** `ModelRuntime` / `EventSource` / `RunControl` stay in ferb-worker: their error type is the worker's
`Error`, whose `Validation` variant holds ferb-orchestrator's pydantic `ValidationError` over ferb-context's `Py`,
so they can't sit in ferb-core without moving those down too; `ferb_core::interfaces::AgentRuntime` is still used
(ferb-runtimes' `CoreRuntime`, ferb-learning) and is not deprecated. `Approval.answer` stays a `String` for
ordinary answers (only the hidden kinds are sealed). Nothing in `ferbd` was changed: its adapters can now go.

**Left for later removal.** `ferb_runtimes::forms::Shape::mapped` (now on ferb-context's `Shape`, ignoring its way
back), the task-keyed `OwnerInput::ask_code` / `ask_secret` and `Driver::owner_code` / `owner_secret`,
`ferb_core::interfaces::AgentRuntime`, ferb-sanitize's `parity` re-exports of the moved helpers.

## 2026-11-13 · 45 · The daemon program composes every tool; first runs against the real Mac

**What.** `ferbd`: all 97 tools have real adapters and none refuses (the catalog test asserts it): the Mac driver
over the native layer, the task-identity browser tools, memory and trail tools, waits, `fs.look` with an
ImageIO/PDFKit renderer, `task.evidence`, `chat.history`. The 20 remaining RPC methods and `logs.subscribe`; the
egress proxy, the Claude tool-list relay, the memory loops, the resource sampler (ported); `setup --service`,
`uninstall`, `doctor`; SIGHUP handled.

**Evidence.** 55 tests pass (run by the porting agent). Through the binary: `abilities.*`, `skills.read`,
`memory.read` (14 of 14 calls) match Python fixtures; `fs.look`'s four decoded pictures are byte-identical to
Python's JPEGs; the 20 RPC methods replay against the recordings with four named fixes.

**Ran on the owner's Mac (the owner allows it when reported).**
- Passive reads, four runs: running apps listed; 501 AX elements read from the frontmost window (VS Code; Brave
  gave none); one window screenshot captured, masked and deleted with its temp home; the pasteboard's change count
  only. No permission prompt (the terminal's app already had the grants).
- A typing test in TextEdit, not completed. Run 1: the daemon's running-app list was stale and called TextEdit not
  running, and the test's guard sent it SIGKILL; only the test's own empty document was open. Run 3: `mac.open`
  with `activates=false` brought TextEdit to the front, so the agent stopped as instructed and typed nothing; it
  closed the document and quit TextEdit. Nothing was left behind (checked: no TextEdit, no `ferbd`, no temp home).
- The guard that kills a process is against the rule that a test closes only what it created; it is to be removed.

**Found by running for real.**
- `mac.open` with `activates=false` brings the app forward (rule 10 broken).
- The daemon's running-app list goes stale.
- A frontmost-window capture answers NoTarget when the app has several windows and no id is given.
- Vision's first text recognition takes 51 to 104 s and failed once at 51 s; Python's takes 81 s on the same Mac.

**Open.**
- Never run for real: the Mac input routes (typing, clicks), recordings, `audio.listen`, `browser.record`, the
  task-identity browser tools' success paths, pairing, handoff, `recovery.snapshot` with the real `tmutil`.
- Not started: the disturbance sentinel, previews and change-record hooks; the owner-input tap; `setup --cleanup`;
  the weekly Mac-profile refresh.
- Seams: `ferb-mac-hands` futures are not Send; `ferb-daemon`'s timer sleepers are private; the gateway's
  `question_sites` is private, so `secrets.list` shows no linked sites; `Learning.embedder` cannot be set later.

## 2026-11-13 · 46 · The supervisor replays every recorded scenario

**What.** The supervisor's third pass: a shutdown suspends a task tree and a restart resumes it in place
(`tree.rs` `RunOpts`, `restart.rs`); scheduled runs (`run_scheduled`); schedules made and changed from chat
(`flow/schedules_chat.rs`: `_schedule_create`, `_schedule_problems`, `_schedule_scopes`,
`_schedule_edit_from_chat`); `_resume`; `tasks.list`; and, behind a `daemon` feature so the phone build stays
clean, the implementations of the daemon's `Tasks`, `Owner`, `Questions` and schedule seams
(`flow/daemon_seams.rs`).

**Evidence.** **All 28 recorded chat scenarios replay fully** (2,271 comparisons: frames, model calls, gateway
calls, store tables), on the real gateway. The two restart scenarios shut the supervisor down and build a new one
on the same store: completed steps do not run again and `seq` keeps rising. The timer and schedule scenarios run
`ferb-daemon`'s real handlers on a real `ferb-rpc` server with the supervisor plugged in. The 10 control scenarios
replay fully. 20 recorded chat and task request lines match through `chat_rpc::register` on a real server.
Format and clippy clean; the iOS check passes (run by the porting agent).

**Fixed against Python.** `conversations.list` with an unknown id answered an internal error; it is refused in
plain words.

**Open.**
- Of the Python class's 282 methods: 172 ported, 21 partly, 89 not. The missing groups, each answering
  `NotPorted` by name: the summary fold, several asks and their card, the checklist for plans of three or more
  steps, loops, delegated conversations, wait and ask steps, `route_question`, the memory and forget side entries,
  the runtime offer, the preview, the front window at send, attention. None is reached by the 28 scenarios; new
  recordings are needed to judge them.
- The supervisor's own task questions still live in a second store sharing the gateway's id counter: `ask_own`
  lacks a task question's `part`, `runtime_switch` and `options_only`.
- Noted by the agent: the owner's installed Python FERB daemon was running throughout (untouched).

## 2026-11-13 · 47 · The Mac hands on the real Mac: background open, a late app list, and typing into TextEdit

**What.** Entry 45's findings checked against Python on this Mac. `mac.open` now waits for an app it launched to
show on the running-app list (`wait_listed`) before recording it as the task's; the open arguments are named
constants (`launch_args`, `OPEN_ACTIVATES`); `mainloop::run_with` makes the main-thread turn testable; the
real-machine check no longer kills anything (it skips itself when TextEdit is already running, quits through ⌘Q,
and a guard asks only the pids it started to quit).

**Evidence.** Through `ferbd` on the real Mac: `mac.open` in the background, a `mac.snapshot` of the text area,
`mac.act type` on the pid route ("typing delivered in the background; a value change was observed"), the line read
back through AX (34 characters), safe quit. The system's front app (asked through `lsappinfo`, not the daemon's
flags) stayed the owner's Brave throughout and the cursor did not move. With no main-thread turn a launched
TextEdit stayed off NSWorkspace's list for 6 s; with turns it was listed within 0.25 s. A cold Vision load took
44.6 s; a first request made during the warm-up waited for it and succeeded. 5 tests on fakes and a mainloop test;
workspace clippy clean (run by the porting agent).

**Ran on the owner's Mac.** TextEdit opened with an empty temp document and asked to quit, about seven times
(two Python control runs, two Rust probes, three through `ferbd`); two screenshots of the owner's Brave window,
kept only in a temp home and deleted with it; AX reads of VS Code and TextEdit; Vision on blank in-memory images.
Nothing left behind (checked by the coordinator: no TextEdit, no `ferbd`).

**Fixed against Python.** `tools.py:348` looks for a launched app once, right after the open and before the main
run loop's turn lists it, so the app could go unrecorded: no quit at release, and ⌘Q refused as "not an app FERB
opened". Rust waits for it as long as for its window.

**Corrections to entry 45.** "`mac.open` brings the app forward" was wrong: the owner was switching apps under a
before/after comparison of the daemon's `isActive` flags; both builds leave the app in the background. "NoTarget
with several windows and no id" is Python's rule (`driver.py:913-923`), ported as it is. The coordinator had
reported both to the owner as bugs from one run.

**Open.** `ferbd` does not wire the front-window capture at send (L12: no `NativeFront` implementation). Vision's
error text is dropped in both builds, so the one failure at 51 s is unexplained. Python treats a window's missing
layer as 1, Rust as 0. Never run for real: pointer routes, foreground input, recordings, audio.

## 2026-11-13 · 48 · The supervisor's fourth pass: the summary, cards, the checklist, memory side entries

**What.** `ferb-supervisor::flow` gains `fold.rs` (the running summary of the conversation), `card.rs` (several
asks in one answer and their card, parts, the checklist), `answer.rs` (answers routed in chat, `route_question`),
`memory_side.rs` (the memory and forget side entries) and `results.rs` (earlier results, unbacked parts); ask and
wait steps; the checker's runtime trouble; the planner's last call. New `Daemon` seam methods, all with defaults.

**Evidence.** `rust/fixtures/supervisor/make_more.py` records 14 new scenarios from the real Python daemon, with a
tripwire on every way Python starts a program (only FERB's own card reader may start) and two runs writing the
same bytes. All 14 replay fully in `tests/replay_more.rs`, on a harness shared with `replay.rs`. The earlier 28
and 10 still replay. Format, clippy and the iOS check pass (run by the porting agent).

**Bugs in the earlier Rust port, fixed here.** A correction typed on a confirmation's card was refused and the
gateway's open checks were never set aside (it looked only in the supervisor's own question store); `keep_recent`
read only the latest summary's covered entries.

**Found in Python, not fixed yet.** The forget question in chat says "(from the task) FERB deletes:", marking
FERB's own words as the task's (the same kind as GATE-581).

**Open.**
- About 45 of 282 methods still answer `NotPorted`: loops (about 25: `ferb-orchestrator`'s `LoopRunner` holds its
  host and store as plain references, so a loop cannot run inside the supervisor's tasks, which move between
  threads), delegated conversations, attention, the preview, `mention_paused`, big-job statements and standing
  scopes, `_browse_as`, the front window at send, the image-refused rerun, inventory walks.
- About 8 ported but unverified, among them `runtime_offer` and `answer_runtime_switch`.
- The supervisor's own task questions stay in a second store: `OwnQuestion` lacks `task_id`, `part`,
  `runtime_switch`, `options_only`, `from_task`, `corrects`, `exact`.
- A card row's `seq` read the real wall clock in Python; the recorder and the Rust card take the injected clock.
- The fixture folder is 5.1 MB.

## 2026-11-13 · 49 · The supervisor joins the daemon program: a message goes through the real binary

**What.** `ferbd`'s `compose/supervisor.rs` is the real supervisor: `flow::Daemon` implemented over the gateway,
run tokens, the live task records, the bus, the change record, schedules from chat and the runtimes; the
gateway's question router and the runtime offer plugged in; the daemon's handlers on `SupervisorTasks` and
`GatewayQuestions`; `chat_rpc` registered; start (recover after restart, mark unknown, resume) and shutdown
(suspend) in the lifecycle; `stop.engage`'s hooks. The workarounds for seams fixed in entry 44 are deleted
(the calls thread, the hand-written worker glue).

**Evidence.** A test-only scripted model (absent from a plain build: the `strings` check) drives recorded
scenarios through the started binary over its socket, its tool calls arriving as the tool bridge's with the run's
token. **27 of the 28 base scenarios replay fully** (2,322 comparisons), the two restarts as real process stops
and starts; 13 of the 14 newer ones. A smoke run with the unchanged Python client: `ping`, `about.get`,
`tasks.list`, `chat.status`, and `chat.send "hello"` answered `ack` then a reply; SIGTERM exits 0. 57 tests pass
(run by the porting agent).

**Differences.**
- confirm_stop_pending: one bridge reply differs. Python closes the connection with no line; Rust answers -32603
  "CancelledError". `ferb-rpc` has no handler outcome that closes without a reply. Every frame, model call and
  table row matches.
- remember_then_forget does not replay through the binary: chat capture (`_remember`) is not ported in
  `ferb-daemon`.
- The scenario replay takes the recorder's values for what a real daemon reads from the machine (environment,
  live facts, running apps, file identity, activity); those reads are unverified on the real Mac.

**Ran on the owner's Mac.** Only `ferbd daemon` on temp homes. One interrupted test run left daemons that the
agent killed itself (its own processes) and whose homes it removed. Checked by the coordinator: none running.

**Open.** Refused still: `intro.preview`, `conversations.change resume` (both wait on the supervisor's last pass),
the store clean-up preview, the hearing probe. Not started: `_remember`, the disturbance sentinel, the front
window at send, the memory push, literal checks, the task-browser tab drivers' composition, standing permissions
(the gateway exposes no public store for them). `ferb-rpc`'s `RunTokens` cannot take an injected token source.

## 2026-11-13 · 50 · The supervisor's loops; one question store

**What.** Loops in the supervisor (`flow/loops.rs`: pacing, rounds, pause and stop, statuses, waking on events) on
a thread-safe form of `ferb-orchestrator`'s runner (`LoopRunner<'a, R, H>` with default type parameters, so the
old name is unchanged, and `LoopRunnerShared`). FERB's own task questions moved out of the supervisor's second
store into the gateway's (`OwnQuestion` gained the task question's fields and an `OwnEvent`), so one store holds
every question and no id counter is shared by hand. Also `mention_paused`, `owner_stopped_part`,
`preview_status`, findings on a not-done reason, the runtime offer and its answer, the event-wait step.

**Evidence.** 7 new recordings from the Python daemon replay fully (loop_steer_finish, loop_pause_stop,
mention_paused, owner_closed_tab, reason_findings, runtime_switch_tap, event_wait): 59 scenarios in all. Two
recorder runs wrote the same bytes. Format, clippy and the iOS check pass for the supervisor and
`ferb-orchestrator`; `ferb-gateway`'s tests pass (run by the porting agent).

**Fixed against Python.** The forget question in chat said "(from the task) FERB deletes:"; FERB's own heading is
no longer tagged as the task's (the same kind as GATE-581).

**Open.** About 14 of 282 methods still answer `NotPorted`: delegated conversations (they need the same
thread-safe form `run_conversation` lacks), a big job's plan statement and standing scope, `_browse_as`, the
front window at send, the start-of-task screenshot, the image-refused rerun, inventory walks, the intro's
preview, attention, a correction updating the worker's goal. About 6 ported but unverified (probes, `_look_only`,
sites named elsewhere). `ferbd` must implement the new seams (`loop_rest`, `loop_notice`, `probe_cap`,
`probe_read`, `changes_reset`) and switch to the gateway's question store. The method counts were made by hand.
The fixture folder is 5.6 MB.

## 2026-11-13 · 51 · FERB.app with the Rust engine: the build, the host, no Python (ENG-7)

**What.**
- `scripts/build-app-rust.sh` (helpers in `scripts/rust-parts.sh`; `scripts/build-app.sh` is unchanged and the
  Python build still builds) makes a FERB.app whose nested `com.ferb.daemon` app runs `ferbd` as its executable,
  with `ferb-native-host` in `Helpers/` and a new, separate `ferb` program for the internal commands.
- It carries the 30 runtime Markdown files byte for byte, the extension, the embedding model, Codex, Claude Code
  (newly pinned in `rust/packaging`, the same bytes as the Python SDK's bundled program) and the pinned secrets
  helper; every pin is read from the Rust sources, never from Python files.
- `ferbd/src/host.rs` carries what `main.c` guaranteed: only `daemon`, `mcp` and `grant-probe`; only the account's
  own `~/.ferb`; a rebuilt environment; the real executable path. The internal commands, some of which take
  paths, are no longer accepted by the program that holds FERB's macOS permissions.
- A development home (`FERB_DEV_HOME`) and a dry run (`FERB_DEV_DRY_RUN`) are honored only by ad-hoc-signed code
  (read from the signature's flags); a certificate-signed build refuses them.
- FERB.app, three Swift changes: `isPackaged` no longer requires a Python interpreter; `isCheckout` also knows a
  Rust checkout's service (`ferbd daemon` from outside any `.app`), which closes entry 29's finding; one test.

**Evidence.** The arm64 bundle is 662 MB against the Python build's 709 MB (Claude Code 274, Codex 273, `ferbd` 57).
No Python file, folder or libpython link in it. `codesign --verify --deep --strict` passes and every Mach-O file is
signed (ad hoc). The agent's identifier, entitlements and Info.plist equal the installed app's. From inside the
bundle on a temp home: wrong modes and a foreign home exit 64; the daemon answers `ping` and `about.get`; the
bridge answers `initialize`; `ferb ps` reaches the daemon; SIGTERM exits 0. `ferb setup --service` as a dry run
writes the service definition and records `launchctl` without running it. An x86_64 bundle runs under Rosetta.
Tests: Swift 742; pytest 61 (packaging and release checks); `ferbd` host 3, services 6 (run by the porting agent).

**Ran on the owner's Mac.** Cargo, Swift and npm builds (npm registry reads for the Claude Code pin);
`rustup target add x86_64-apple-darwin`; the bundled daemon for a few seconds at a time on temp homes, removed.
Read-only `codesign` reads of `/Applications/FERB.app`. Nothing installed or registered; the owner's running
daemon untouched. Left in place: `build/rust-app-arm64/FERB.app` and `build/rust-app-x86_64/FERB.app`.

**Open.**
- Unproven until an installed, certificate-signed build: release and Developer ID signing, notarization, whether
  macOS permissions attach to the `ferbd` agent, SMAppService registration.
- On a development home the tool bridge refuses, because model programs pass `FERB_HOME` but not `FERB_DEV_HOME`.
- Claude Code's start-time version check has not run inside `ferbd` against the real program.
- `release-appcast.sh` has not run on a Rust disk image.

## 2026-11-13 · 52 · The supervisor's last gaps

**What.** Delegated conversations, on a shared-store form of `ferb-orchestrator`'s conversation runner (added
beside the old one, a Send future); a big job's plan statement and standing scope, restated by a correction; the
front window captured at send, the image-refused rerun, the start-of-task screenshot; inventory walks;
`_browse_as`; the intro's preview; attention; a correction's new goal and conditions reaching the worker and the
checker (they ride on the tree, since a Rust `Task` does not change).

**Evidence.** 6 new recordings from the Python daemon (conversation_handover, conversation_change, stated_scope,
front_window, preview_attention, browse_as_corrected): **65 recorded scenarios replay fully** in all (28 + 10 +
27). A script comparing the Python class's methods (read with `ast`: 258, not the 282 counted by grep earlier)
with citations in the Rust: 246 ported, 2 kept as `NotPorted` only under their old signatures (`preview`,
`route_question`: the working versions are `preview_events` and `route_worker_question`), 10 not cited anywhere
and probably inlined (`_targets`, `_waits_for`, `finish_episodes`, `stop_episodes`, `_fixing_step`,
`_told_not_done` and four more), not verified. Format, clippy and the iOS check pass for both crates (run by the
porting agent).

**Fixed against Python.** `conversations.change` with an unknown id answered an internal error; it is refused in
plain words.

**Open.**
- No recording behind: probes (arming, reads, `_look_only`), sites named elsewhere, `asked_only_about`,
  `displaced_question`, inventory walks, resuming a conversation whose task no longer runs, a capture that times
  out. A probe recording needs the recorder to record the gateway's probe reads.
- The ten uncited methods need checking one by one.
- `turn::Arrival.seen` changed type (one breaking change for `ferbd`). `ferbd`'s standing-scope and
  named-destination seams are no-ops and must write the tables.
- If the store fails while forgetting a conversation's trail copies, Rust journals `conversation.forget_failed`
  where Python would raise.
- The fixture folder is about 6.3 MB.

## 2026-11-13 · 53 · The daemon program's wiring closed; the whole engine in one checked workspace

**What.**
- `ferbd`: chat capture (`areas_a::remember`: `_remember`, `_until`, `_save_login_name`); a sleep woken by its
  timer instead of a one-second poll; the owner's presence and input tap feeding timers, notices and foreground
  input; episode cards read in-process on a read-only connection (MEM-418 is a path boundary, the reader opens
  only its own home, not a requirement for a separate process); the weekly Mac-profile refresh;
  `setup --cleanup`; the front window captured at send over its own window list (a window's missing layer counts
  as not normal, Python's rule); the disturbance sentinel over a native desktop probe; literal checks; the hearing
  probe; standing permissions and named destinations written through `ferb-policy`'s stores; the supervisor's new
  seams; `intro.preview` and `conversations.change resume` no longer refuse.
- `ferb-rpc`: a handler outcome that closes the connection with no line (Python's behavior when a call is
  cancelled by stop), and an injectable token source for `RunTokens`.
- `ferb-supervisor` and `ferbd` are workspace members: nothing is outside the gate any more. The debug profile
  optimizes `sha2`, because the daemon hashes its own executable at start and unoptimized that took over 30 s and
  hung a start.

**Evidence.** Through the running binary: all 28 base and 27 newer recorded scenarios (55), confirm_stop_pending
now with no differing line. Whole workspace, run by the coordinator: format clean, clippy with warnings refused
clean, **684 tests pass, 14 ignored**, the engine crates build for iOS and Android.

**Ran on the owner's Mac.** `ferbd` on temp homes; two runs of a front-window capture (one redacted picture kept
in the temp home's evidence store and deleted with it; only key names printed) and a disturbance sample around
`/usr/bin/true` in the sandbox. One manual start hung (the hash above); the agent killed its own process. Checked
by the coordinator: no `ferbd` left.

**A mistake in the commits.** `be6e8f3c` (the packaging) swept in a half-finished snapshot of `ferbd` that called
code not yet committed in `ferb-daemon` and `ferb-rpc`, so that commit alone does not build. This commit makes the
tree whole. A snapshot of a crate another agent is editing is not to be committed again.

**Open.**
- Not started: a task's live previews (they need a `try_hold` on the gateway's locks), the memory push
  (`_learning_candidates`), the persona scan behind the weekly refresh, embedding vectors for the prompt's chat
  and memory search.
- loop_steer_finish can differ in id order through the binary: on a multi-threaded runtime a steer can wake the
  loop before the reply is written. It passed in this run; the fix belongs in the supervisor.
- `ferb-sense`'s presence has no setter for "present since"; `ferb-mac-hands`' window list still defaults a
  missing layer to 0; the gateway's `probe` future is not Send and `question_sites` is private.

## 2026-11-13 · 54 · The old FERB removed; the Python suite leaves the push gate (owner, TEST-510)

**The owner's words.** "install it, you can quit/delete the old ferb, im not usig it anyways rn"; and, on the
gate: "remove the python test. havent we migrated, so why do we even need it?"

**Done on the owner's Mac.** The Python FERB's service unloaded (`launchctl bootout`); `~/.ferb` (2.2 GB) and the
LaunchAgent definition moved to `~/FERB-archive-2026-11-13-before-rust/`; `/Applications/FERB.app` moved to the
Trash. The sign-ins for Codex and Claude lived in that folder, so the new install asks for them again; Keychain
items are untouched.

**The gate.** Pushing the engine failed three runs in a row, each on a different timing-sensitive test outside
the Rust work: a Swift window-Space test, `test_local_direct.py` (T-22 already calls it unreliable on a busy Mac),
then a connection-flood test in `test_egress_proxy.py`. Each passes alone; the four Rust checks passed every
time. Every push of this branch ran the 25-minute Python suite because the packaging touched `scripts/` and
`tests/`. The coordinator also ran the gate twice per attempt (by hand, then again through the pre-push hook),
which doubled the wait. `kb/gate.toml` no longer has the Python suite as a push check; the quick Python checks at
commit stay while the Python sources remain.

**Next.** Push, rebuild FERB.app with the finished engine, install it, then setup with the owner present.

## 2026-11-13 · 55 · The first installs; the engine's open list closed; owner browsing wired

**Installs on the owner's Mac (the owner's word: "install it, you can quit/delete the old ferb").**
- First install: the ad-hoc-signed build. The tour's permissions beat showed Accessibility "Added · turn it on"
  and three rows "Restart needed". Cause: macOS ties a grant to the code's signature, and an ad-hoc build is a
  different program from the FERB signed with the release certificate (and from every other ad-hoc build). The
  coordinator had told the packaging agent not to use the owner's signing identities, which is right for tests and
  wrong for an installed build.
- The owner asked why not the Developer ID they already have. Second install: `SIGN_IDENTITY=devid` with the saved
  notary profile. Apple accepted the bundle on the first submission; Gatekeeper reports "Notarized Developer ID",
  team 8N7RNGH974; the ticket is stapled. This is T-27's first real Developer ID build. Rebuilds of the team keep
  the grants.
- The build script needed one fix first: it looked for `ferbd`'s own lock file after the crate had joined the
  workspace (9fc1b35e).
- APP-907 (owner): "also remove the timer from tour". The waiting indicator draws the spinner and no elapsed time.

**The open list of entry 53, closed by one agent.**
- Live previews over the gateway's locks (`Gateway::try_hold`; 7 scenarios from the Python tests' cases).
- The memory push and query vectors (`areas_a/candidates.rs`; 7 queries against the real Python memory); the
  learning store takes its embedder after it is built.
- The persona scan (4 cases on a fake home with `mdls` stubbed), the start-time step that waits on Full Disk
  Access, presence's "present since".
- A correction that wakes a loop: Python's recorded order is the loop round's span, then `chat.reply`, then the
  round's worker, so a hand-over makes the turn wait until the woken loop has started its round and the round wait
  until the reply is recorded. 100 of 100 runs through the binary replay.
- Six new recordings for probes, login sites named in another chat, the displaced question, an inventory walk, a
  conversation resumed after its task ended, a capture that times out: 33 newer scenarios replay in process and
  through the binary. Of the ten uncited supervisor methods five were inlined (now cited) and `_wait_cut`,
  `_capture_after_outcome`, `stop_episodes` and `finish_episodes` are ported.
- Seams: the window list's missing layer, a Send `probe`, public `question_sites` (so `secrets.list` shows linked
  sites), file tools on a blocking thread, time conversions through `to_py`/`from_py`, dead adapters removed.

**Found and fixed in the Rust port.**
- `ferbd`'s owner-browser driver never chose a profile (it always refused "no browser profile is paired") and had
  no owner input for login codes or secrets: every tool on the owner's own browser refused. It now uses
  `ferb-browser`'s resolver and the gateway's hidden-input questions. **The installed build predates this.**
- `hide_code_reply` never applied `asked_only_about` or `displaced_question`.
- Setup's scan wrote a naive time with microseconds where Python writes a local time with its offset.

**Evidence.** Whole workspace (run by the porting agent): 702 tests pass, 15 ignored; format, clippy and the
portable check clean. Coordinator: format clean.

**The commit gate's slow check.** `kb check` spends about 125 of 128 s in `symbol_exists` (tools/kb/kb.py:285-304),
parsing a Python file again for each of 1,185 `file.py::symbol` entries (`supervisor.py` alone 237 times). It is
not the fixtures. Caching each file's names would bring it to seconds and stop checking nothing; not done here.

**Open.** Unverified on the real Mac: the persona scan, previews of real windows and tabs, the resolver with a
paired browser. The owner's setup on the Developer ID build is in progress.

## 2026-11-14 · 56 · The first live run, debugged: the browser connection, the in-app service, Accessibility's alert

**What.**
- `ferbd` refused every native-host connection: `compose/tools/browser.rs` composed the relay's parent as `|_fd| None`
  and its app as `|_pid| None`, so `ferb-browser` `bridge.rs:1189-1191` answered every browser "FERB couldn't tell
  which app started this browser connection". Now `peer_seams()` reads them as Python does (`ext_bridge.py:131-160,
  249-260`): LOCAL_PEERPID, libproc's parent, the outermost `.app`/`.app.bundle`'s Info.plist (`ferb-browser`
  `macos.rs` `native_host_parent`, `app_bundle`; `rules.rs` `outermost_bundle`).
- The pairing's wake page was `""` (`wake_url: String::new`) and both drivers had no `Wake` (HAND-403's start of a
  profile that isn't running). `WakePage` serves Python's `WakeServer` page; `ProfileWake` runs `open_known_profile`.
- HAND-406: the hub's `record_refusal` and the socket's `on_native_refused` were never set, so no refusal was
  journaled. Now through `_Repeats` (`ferb-daemon` `Repeats`, first use in `ferbd`), flushed at shutdown.
- The daemon's own log (DMN-592, `src/log.rs`): start, socket serving, every call answered (ms since the process
  started), slow start steps, stop, FERB.app-only verdicts, each bounded journal problem, a redacted panic with a
  backtrace (also `daemon.unhandled`). Python wrote only redacted tracebacks there (checked: the archived Python
  `daemon.err.log` is 84 lines of tracebacks, `daemon.out.log` empty).
- SENSE-409 (fixed against Python): ferb-sense's Accessibility elements exist only while `AXIsProcessTrusted` says
  FERB is trusted.
- DMN-591 (FERB.app): launchd's "spawn failed" for FERB's job is read from `launchctl print`; the in-app registration
  is made again once, then the LaunchAgent; after every restart or start the app makes.
- `setup.permissions` reads Automation for all apps at once through the shared check (`rpc.py:505-510`).
- FERB's own browser attaches (`IdentityAttach`: `ferb_browser.*` in the registry; `identity.check` can say paired).
- REL-424: `FERB_NOTARIZE=0` with `SIGN_IDENTITY=devid` signs for this Mac only; DMGs and appcasts still refuse it.

**Why (the owner's three complaints).**
- "Didn't start from the startup sound, settings said FERB isn't running": not the Rust start (it answers every call
  0.4-0.5 s after launchd spawns it, three runs). In the first run FERB.app registered its own service (DMN-587,
  never run live before) and launchd failed it every 5 s ("The specified path is not a bundle", error 128); the app
  waited 30 s, fell back to the LaunchAgent (23:35:14) and only then started the tour. Reproduced on 2026-11-14 with the
  exact cause: the in-app job resolves its program through the Background Task Management record of the user
  LaunchAgent it replaced (same label); once macOS dropped that record, launchd failed every start ("copy_bundle_path
  (91943C9A…) record not found", EX_CONFIG), after FERB.app's restart-to-check `kickstart -k` had already reported
  success, so the tour hung at Input Monitoring.
- "Pairing opened all browsers but not the pairing page": every connection was refused (above), so the page's token
  never arrived; the empty wake address opened a window in every profile and nothing could close it. The pairing
  page opens and closes itself by design (`pair.js`).
- "Mac control permission midway": macOS's own Accessibility alert (`universalAccessAuthWarn`), raised by the
  untrusted daemon's first Accessibility call from its watchers (`TCCAccessRequest kTCCServiceAccessibility` from the
  daemon's pid, then `TCCAccessSetInternal client=com.ferb.daemon`). Python's watchers make the same call
  (`base.py:focused_app`); its builds kept their grant across releases, so it rarely showed. Not Automation: the
  daemon's only Apple Events during pairing were `ask=false` checks.

**Evidence.** Tests: `ferbd` `tests/native_host.rs` (a relay whose parent is a compiled program in a fake
`Google Chrome.app` is taken; one from a fake `com.example.other` and one from no app are refused and journaled; a
foreign origin is a "host" refusal); `lifecycle` asserts the log lines; wake page, identity attach, Automation
concurrency, trust cache, bundle lookup units; Swift `ServiceSpawnFailureTests` (6); pytest for REL-424. Workspace:
format, clippy and tests clean (counts in the coordinator's run).

**Ran on the owner's Mac.** Two Developer ID builds without notarization, installed at `/Applications/FERB.app`;
`ferb setup --service` once (00:12), FERB.app launched once by me (00:19:21; the owner then went through the tour
himself and quit it at 00:23:24), `launchctl submit -l com.ferb.daemon` once (00:36); `launchctl bootout` of
`com.ferb.daemon` three times; `~/.ferb` removed three times. FERB's native-host manifest and launcher written by
hand (Full Disk Access isn't granted, so FERB's own writer is gated, DMN-507/508) in Brave's and Chrome's
`NativeMessagingHosts`, removed at the end. One `chrome-extension://jaclnph…/pair.html` (no token) opened in Brave
Default to wake the extension (it did not load: that copy's folder had moved with the old `~/.ferb`, inferred).
Brave's FERB extension (`jaclnph…`, unpacked) then connected and was taken: relay parent Brave pid 12972,
`browsers.list` `unpaired_connections {"brave": 1}`, no refusal (00:17:52 and again on the final build 00:41:11).
A relay run from a shell was refused and journaled `browser.connection_refused {"reason": "peer"}` (00:16:40).
`setup.permissions` answered in 0.05-0.09 s with Brave and Chrome running. My `ferb setup --service` at 00:12 left
the LaunchAgent record that the 00:19 registration bound to: it caused the owner's hang at 00:21:30 (not a stop
of mine; FERB.app's `launchctl kickstart`, pid 78147). Left: the final build installed, no service, no `~/.ferb`.

**Open.**
- Pairing end to end is unproven: profile discovery needs Full Disk Access (`browsers.pair` → "no brave profile in
  folder 'Default'", `setup.browsers` → `waiting: full_disk_access`).
- SENSE-409 live: the owner granted Accessibility at 00:21, so the after-fix start (00:36) was trusted and can't show
  the alert's absence; only the cache is tested.
- DMN-591's repair is tested on fakes only; whether a fresh registration always gets its own record is unproven. The
  root fix is that `setup --service --app-service` should not leave a LaunchAgent record the registration can bind
  to; macOS gives no API to see or drop that record.
- The audit's other ten mismatches (`scratchpad/app-daemon-audit.md` 4-13), and: browser downloads, uploads, tab
  trail, owner-tab ownership and sound hooks are not composed in `ferbd` (the hub's `on_download`, `on_tab`, `owned`,
  `paused`, `on_sound`, `on_tab_closed` stay defaults; `Downloads`/`Uploads` are never built).
- The new files (`ferbd/src/log.rs`, `ferbd/tests/native_host.rs`, `ServiceSpawnFailureTests.swift`) are untracked:
  kb sees DMN-591's tests once the Swift file is added.

## 2026-11-14 · 57 · Every seam ferbd fills wired or named; one way to run the service; the service loop proven

**What.**
- The audit's items 4-13 (scratchpad `app-daemon-audit.md`): `chat.status.paused` reads sensing's pause; banners stay
  off while a `logs.subscribe` stream shows the topic (`areas_c::logs::Watchers`, `_log_subscriptions`) or a chat is
  attached to the tree, and a task's request is read back (`request_of`); the human-check handoff has the router's tabs,
  banners and the supervisor's waiting flag (`Supervisor::worker_waiting`); `update.prepare` waits for FERB's running
  shell processes and a transcription (rpc.py:97, DMN-401) and quits the hearing instances; `chat.attach` starts the
  Mac scan and `chat.status.persona` reads it; Input Monitoring is asked once with the first request (`AskOnce`); the
  notch's forget answer has its follow-up line; an edited LaunchAgent is put back at start (DMN-514); data set aside is
  told once (DMN-10); the Screen Recording probe's window rule is a tested pure function (it already read a missing
  layer as 0).
- Browser `Downloads` and `Uploads` built (sweep, finalize_stale) and composed as Python's `_build_browsers` and
  `_build_sense`: the hub's `on_download`, `on_tab_closed`, `on_tab` (the trail), `paused`, `owned`, `on_sound`; each
  driver's downloads, uploads, clipboard, Quartz image masking, native secrets/capture/place (through the desktop area),
  upload files, profiles and FERB's inbox. The drivers had no seams at all before (`set_seams` was never called): a tab
  picture with a secret field failed ("no image library") and uploads were refused.
- The sweep (two read-only agents classified every seam; four agents and the coordinator wired them in disjoint files).
  New: `compose/desktop_seams.rs` (desktop.py's `Seams` over the parts: `_task_end`, `_task_paused`/`_resumed`,
  `_tree_finished`'s upload copies, the native captures), `compose/consumers.rs` (the rest of `_wire_events`: the day's
  activity, conversation observation, a question's banner, GATE-562's refused-value notice, ORCH-312), environment
  facts from the browser area (hub, profiles, profile state, unpaired note, the browsing question, developer tools),
  the supervisor's `grant_in_force`, `withdraw_lapsed`, `asked_origins`, `resumed_outcome`, `declined`, `ran_cards`,
  `has_route`, `identity_pending`, `signals`, the core profile in every prompt, `[fs]` owner and untrusted roots
  (`~/Downloads` and `uploads/outside` were missing), the global stop's browser/hearing/shell hook, secret-helper
  failures journaled (HAND-410), the owner's fills sealed through the signed helper, perception and its native
  watchers, files/app-install/scripted-tab watchers, the shared attribution, learner spans and usage, the shell's
  escape journaling, local refusals and reports, the browser tools' terminal input, point, payment picture and press,
  tab picture and protected saves, the owner's contacts and username holds.
- The guard: `tests/seams_wired.rs` reads the composition's source and fails on any placeholder-shaped value outside
  `crate::off::off("id", …)`, on an off id `seams-off.txt` doesn't list (or one it lists and no code uses), and on a
  trait default (flow::Daemon, Reads, notify::Listeners and Desk, Tool for BrowserTool and MacTool) neither overridden
  nor listed with a reason. `seams-off.txt`: 36 off ids, 19 defaults, 22 plain data fields, each with why.
- DMN-593 (owner): the Rust FERB.app registers its service only through SMAppService; no LaunchAgent fallback, no
  self-signed migration (APP-902/904/905 superseded; DMN-594 the agent ruling, superseding DMN-591). From FERB.app,
  `setup --service` never writes `~/Library/LaunchAgents/com.ferb.daemon.plist`.
- An updated agent's build is read from its bundle (`host.rs` `bundle_build`): an SMAppService job keeps the
  environment of its first registration across an update, so `FERB_APP_BUILD` stayed 202611131934 on later builds.
- The stop: sensing's stop took 3 s on every restart (`NativeThread`'s drop waited up to 2 s for presence's 5 s run-loop
  turn, then the tap's). Its run loop is woken now and a thread still inside a step is left to end on its flag, as
  Python's daemon threads are; each slow part of a stop is a log line (`stop: … took … ms`).

**Why.** The owner's first setup failed on do-nothing seams no recording exercised; the owner: "FIGURE ALL THIS
background/ui mismatch and fix it ... ensure its a general fix". DMN-593 is the owner's answer on the service.

**Evidence.** New tests: `tests/wiring.rs` (4, through the binary), `tests/seams_wired.rs` (3), `tests/owner_facts.rs`
(3), `tests/sensing_wired.rs` (5), a relay-connected stop under 1 s (`native_host.rs`), unit tests for update blockers,
AskOnce, log watchers, LaunchAgent restore, the window rule, the bundle's build, native-thread and presence stops,
DMN-593's installer and Swift tests (748 Swift tests, 0 failures). Workspace: see the report; the recorded scenario
`conversation_handover` differs in about a third of runs since the day's-activity consumer was wired (none of 8 runs
without it, 3 of 8 with only it): not resolved.

**Ran on the owner's Mac** (01:46-03:20; the app's two disappearances at 01:46 and 02:31 were the owner
quitting it, not crashes). First in-app registration from a state with no LaunchAgent record: app opened 01:46:10,
daemon spawned by launchd 01:46:26, every call answered 1.19 s after its start; BTM item `8.com.ferb.daemon`
(team 8N7RNGH974, generation 1); no plist written. Update 202611131934 → 202611132052 (quit, `rm -rf`, `ditto`, open):
the same registration (generation 3), the new binary running 6 s after the app opened, no prompt. Five `kickstart -k`:
back in 1.2, 4.5, 4.5, 4.7, 4.5 s (launchd's ThrottleInterval 5 s spaces respawns); each stop spent ~3 s in
sensing. Update → 202611132105: back 8 s after open, the daemon logging its own build; five restarts: 0.8, 2.6, 2.5,
2.5, 2.5 s, sensing's stop ~2.1 s (an Accessibility walk held the thread's drop). Update → 202611132139 with FERB.app
left open throughout (03:17:58): the new build's daemon 6 s after open; five restarts back in 0.89, 0.74, 0.62, 0.81,
0.74 s, each stop 250-310 ms (accessibility's thread 120-173 ms of it); `launchctl print`: program
`Contents/Helpers/FERB.app/Contents/MacOS/FERB`, the job's own environment still the first registration's build;
`service.json` service "app"; no plist in `~/Library/LaunchAgents`. Permissions before and after the updates unchanged
(accessibility, input monitoring, clipboard granted; screen recording and Full Disk Access denied as before). Left: the
202611132139 build installed, FERB.app quit, the service booted out (its registration stays), `~/.ferb` removed, no
NativeMessagingHosts file of FERB's, nothing of FERB's running.

**Why the in-app registration failed tonight.** At 23:34:43 the first Developer ID build registered while
`2.com.ferb.FERB`, the background-item record the self-signed builds had made (no team, disposition disabled), was
still the app's record, and the old app-migration code unregistered and registered the main-app login item first;
launchd then refused the job every 5 s ("The specified path is not a bundle: Contents/Helpers/FERB.app/…",
`copy_bundle_path(05D44A95…)`, EX_CONFIG), and after a same-label LaunchAgent had been installed and removed the job's
record was gone ("record not found"). From a state with no same-label LaunchAgent the same registration worked the
first time. Which record `05D44A95…` named isn't in the log. A normal install never meets it; an update from a
self-signed build with its LaunchAgent could, which DMN-593 removes from the product.

**Open.**
- `conversation_handover` (above).
- DMN-587's text still says an ad-hoc build keeps the LaunchAgent; DMN-594 registers ad-hoc Rust builds in-app
  (unverified live). LIVE-943 and LIVE-946 test removed behavior.
- Not composed by design: Jev (off), FERB.app as the sending client's pid at send.

## 2026-11-14 · 58 · A chat's frames in Python's order however the stream's task is woken

**What.**
- `conversation_handover` differed in about a third of runs through the program, always the same way: the chat
  stream's line 9 was the hand-over's `reply` where Python sent the thread read's bus events. The whole Rust stream
  of failing runs was ack, plan, subtask, step, paused, needs_you, approval.requested, step running, progress, reply,
  step done, subtask done, verification, **tool.called, action.observed**, outcome: the read's two bus events came
  after the turn's own events instead of before the reply. In process (replay_more) it never failed (0 of 30 with the
  old code).
- Cause (traced with temporary logging of every poll of the turn's task): the bus events ride the tree's line
  (`chat.rs` subscription, `worker.tell_tree`), `run_tree` forwards that line into the turn's queue, and the turn's
  stream takes whatever its queue holds when its task is polled, grouped by `as_python_wakes` (bus group after the
  turn's group). The tree runs inside the same task, so its line reaches the turn's queue one poll after it was told.
  The `tokio::task::yield_now()` after `conversation.read` (`conversations.rs`) was meant to let the stream take the
  read's events before the turn goes on; but its one `Pending` often came before `run_tree` had forwarded them, and
  the next poll forwarded them and ran the worker and its check to the end without another `Pending` (the program's
  scripted model answers at once). The stream then took step, tool.called, progress, action.observed, reply, step,
  subtask, verification as one batch and put the two bus events last. Python (`conversations.py:299`
  `ConversationRunner.run`, `supervisor.py` `_conversation_step`): `await self.turn(...)` reads the thread through the
  gateway; Python's loop runs the chat's queue readers at the gateway call's own awaits, so the read's bus events
  reach the chat before the worker's first model call. Whether the yield worked depended on thread timing; wiring the
  day's activity consumer only shifted that timing (with temporary logging on, it failed 4 of 4 times with the
  activity consumer off).
- Fix: `Inner::python_wakes_here(tree_id)` (`tree.rs`) puts a wake mark on the tree's line; the stream ends a burst
  at each mark whenever it takes the batch (`chat.rs` `as_python_wakes` splits, `one_burst` groups; the mark goes no
  further; a resumed run's `resumed_event` drops it). Used after `conversation.read` and before a step's question is
  asked (`owner_wait.rs`, the same yield-as-boundary pattern), in place of the two `yield_now`s.
- Python's consumer order (`compose.py` `_wire_events`): plans, activity, conversations, attention, bus, approval
  banner, GATE-562, `_question_record`, ORCH-312. Rust had the bus first, then plans, the turn's bus,
  `_question_record`, then activity and conversations. Now `consumers::wire_first` adds plans, activity and
  conversations before the bus (each takes its parts once they exist, as before), so a conversation's
  `conversation.paused` reaches the bus before the trail event that caused it, as in Python. The day's activity row is
  written inside the publish again (the thread of entry 57 drew `when` from the clock later than Python).

**Evidence.** `conversation_handover` through the program: 100 of 100 (new ignored test
`a_conversation_turn_keeps_pythons_order_every_time`; the loop test now shares `every_time`). Its 100 runs ran while
two other agents built and tested (the coordinator measured a load average of 155 around then). Every recorded
scenario once: ferb-supervisor `replay` (2 tests), `control_scenarios` (12), `replay_more` (33), ferbd `chat_replay`
(the 28 and the 33 through the program).
Whole workspace on a copy of HEAD with only these changes (another agent's half-made browser edits in the shared
checkout did not compile): format and clippy clean, `cargo test --workspace` three times in a row, 742 passed, 0
failed, 16 ignored each time, `./gate.sh portable` clean. Six tests of ferb-platform-macos `tests/identity.rs` were
left out of those three runs (see Open).

**Open.**
- From 04:39 the Mac could not start a copied signed program: syspolicyd (started on demand at 04:39:45) logged
  "Unable to initialize qtn_proc: 3" without end, and a copy of `/bin/echo` in any folder, inside the sandbox or out,
  stayed at `_dyld_start` until killed. A program cargo had just built still ran. So ferb-platform-macos `identity.rs`'s
  six tests that run a copy of the signed helper (`owner_secrets_flows_match_python_step_by_step`,
  `configured_secrets_match_python`, `a_value_travels_only_on_stdin`, `the_helper_is_called_and_read_as_python_does`,
  `helper_seal_gives_the_nonce_and_the_ciphertext`, `the_pinned_code_swapped_for_itself_still_runs`) hung for 10
  minutes and failed once their helpers were stopped. That comes from the Mac's state, not from this change (no
  identity code changed); they need running again once syspolicyd works, which may take a restart of the Mac or of the
  daemon.
- The consumers Python `_spawn`s (the approval banner, GATE-562's notice, ORCH-312's pause and resume) run in Python
  at the publisher's next await; Rust spawns them on another worker at once. No recording reaches them with an order
  that depends on it; their journal rows could interleave differently from Python's.
- `_question_record` still runs before the approval banner and GATE-562 (Python: after); both are only a spawn at
  publish time, so nothing observable differs.
- Other yields that stand for Python's suspension points remain where they order tasks rather than a chat's frames
  (`loops.rs` with `SteerHold`, `control.rs` a woken loop, `tree.rs` the episode capture).
- A bus event about a resumed tree that a chat turn's subscription puts on that tree's line reaches
  `resumed_event`, which publishes it as `task.resumed_event` (no `type`); not seen in any recording, not changed.

## 2026-11-14 · 59 · The first real tasks on the Rust engine: a frozen app, steps stalled behind one question, calls that ran for nobody, the owner's profile ignored

**What.**
- The gateway: a call queued behind its tree's open question (one FERB question per tree) waits on the owner (the
  stall clock stops) and is cancelled when its attempt ends (`queue_behind_question`, `forget_calls`; `Inner.queued`).
- FERB.app: a message's text goes into its text view on the next main-queue turn, measured meanwhile with a detached
  layout (`NativeMessageText.swift` `stage`/`applyPending`), so an Accessibility notification can't re-enter SwiftUI's
  update.
- ferb-browser: a task browsing in the owner's chosen profile can't name another one (`picked_only`,
  `Tasks::browses_as_owner`, read from the live tree in `ferb-supervisor/src/flow/browse.rs`).
- The shop-link question rebuilt as `rust/fixtures/gateway/tab_link.json` (`make_tab_link.py`): Python asks it too.

**Why.** The owner ran real tasks with real Codex on build 202611132139 and reported: "it shifted the screen focus …
ferb is stuck", then "i entered a profile in chrome BUT it anyways used the one in brave". The app froze at 03:57:08
(an attribute-graph cycle through `NSHostingView.accessibilityChildren` from `MessageTextView.update`), 0.6 s after
step cameras' confirmation was published, and stayed frozen until the coordinator force-quit it at 04:04 on the
owner's word. Steps claude and blender queued confirmations behind that question with the stall clock running, so
each attempt was stopped after 180 s and retried; at the owner's answer (04:03:44) the queued calls of the stopped
attempts ran: RETURN and ctrl+c in Terminal, an owner.ask for Blender. The 03:56:28 "focus shift" was the owner's
hover opening the notch with focus (WindowServer "key thief", frontmost stayed Code); the Space switches at
03:56:53 and 03:57:00 were the owner-approved foreground typing in Terminal; Blender's launch tried to come forward
and macOS refused it. In task 6b2b3266 the owner's pick (chrome/Profile 15, saved 04:15:26) lost to the worker's
`"profile": "brave/Work"` because a named profile is resolved before the pick.

**Evidence.** Spans and journal of tasks a5255913, 6b2b3266, 254ed34c (read-only copies, since deleted); the unified
log of FERBNotch and WindowServer; the app's sample. Tests: ferb-gateway
`a_call_queued_behind_another_steps_question_waits_on_the_owner_and_ends_with_its_attempt` (fails without the fix),
`the_shop_link_case_replays_as_python_ran_it`; ferb-browser `tests/owner_pick.rs`; Swift `MessageTextStagingTests` (6).

**Fixed against Python.** `gateway.py:1716-1732` holds `_confirming` while asking with the stall clock running for
queued calls, and their calls survive their attempt. `resolver.py:48-60` returns a named profile before the owner's
pick (APP-777 says the pick is always used). The Swift hang is in the app the Python build shares.

**Corrected.** The coordinator first told the owner there were two unapproved focus shifts and a three-minute model
silence; both were wrong, as above.

**Ran on the owner's Mac.** Read-only reads and backups of `~/.ferb/ferb.db`; `sample` of the daemon and the app;
`log show`; the frozen FERB.app force-quit and reopened at the owner's request. Load average reached about 155 from
three agents' builds between 04:14 and 04:31: slow page reads and a capture timeout then may be artefacts.

**Open.**
- The shop-link question is Python's too (GATE-541; GATE-568 decided, not built, T-1075).
- `browser.tabs` labels the owner's other tabs as pages the task read (both builds).
- Progress answers come only from status facts (`supervisor.md:201`); the owner wants what the task has so far,
  marked not verified.
- Leftover-tab lines are one per tab and cleanup doesn't wake a disconnected profile (both builds); whether the
  keepalive ran live is unverified.
- bad_mac repeats (242) from install 232158e2: sender unidentified.
- Which outside client observed FERB.app is unknown; UI cost under load not measured.
- The owner's four questions on profile choice (refuse, redirect or ask; planner-chosen owner identity; reading
  other profiles' tabs; the facts wording).
- No test drives `Inner::browses_as_owner` in a live supervisor.

## 2026-11-14 · 60 · Pairing leaves no tab; a relay's input is not a call

**What.**
- HAND-520 (fixed against Python): pairing loads a profile that isn't connected with no window
  (`--no-startup-window`, as the drivers' wake does) and opens only the self-closing pairing page. `WakePage`,
  `wake_url` and `close_wake` are gone. The daemon log names each browser command it runs, token left out.
- APP-908: only input on a microphone counts as a call (`MicrophoneUse.swift` `InputUse`, `microphoneInputs`); each
  sound played is logged with its file.

**Why.** The owner's first setup: "it didn't close the tabs it opened … close the tabs it opened to pair when
paired", and "no sound whatsoever". Every profile kept a "FERB" wake page: Python closes that page only after a
pairing that succeeds (its recording opened 7 and closed 2), and live the close got no answer. The system log shows
every sound "stayed silent: PairIt - AudioRelay is using the microphone": that relay runs input on a tap naming no
device, with the microphone off. The coordinator's guess (a resumed tour) was wrong.

**Evidence.** `b_browsers` replays Python's recording with the one named difference; new test
`pair_all_leaves_each_profile_with_only_the_owners_tabs`. Swift `testInputThatIsntAMicrophoneIsntACall`,
`testARelaysInputDoesntSilenceTheFirstLaunchClip`. On this Mac the new reader returns no calling process; the old
one returned AudioRelay.

**Open.**
- Why `wake_close` got no answer live is unverified (about 10 s per profile: inferred, no reply); commands sent
  right after pairing should be checked live.
- FERB plays to the default output (Audio Relay here) while macOS's interface sounds use the system output device:
  the owner's call.
- Live checks of both fixes wait for the next installed build.

## 2026-11-14 · 61 · What a running task has so far; one message for what cleanup left; a dropped profile woken to close its tabs; pairing's visible fallback; FERB reads its own diagnostics

**What.**
- ORCH-640: progress questions are answered from the task's records. `supervisor.md` sends the planner to
  `task.evidence` (worker reports, one being checked included, else tool results) and says what it has, not yet
  checked. The status row's `last_report` shows the latest report's first line; the running worker's latest words,
  redacted and kept in memory only, ride the row while its run runs. Recorded prompts are compared through the named
  change list `rust/fixtures/catalog/prompt_changes.json`.
- HAND-521: a cleanup's leftovers are one message: tabs grouped by profile, up to three titles (two per profile when
  there are several), the rest counted, one "close them if they're still open".
- HAND-522: before closing, cleanup wakes each disconnected profile holding the task's tabs with no window and waits
  up to 35 s for it to connect and re-adopt (`driver.rs` `wake_for_cleanup`, `bridge.rs` `settled`).
  `browser.task_profile_disconnected` / `_reconnected` are journaled while tasks hold tabs.
- HAND-523: with no paired connection 10 s after the windowless start, pairing opens the profile's window with FERB's
  wake page and the pairing page again. The wake page (`areas_b/browsers/wake.rs`) closes itself on every outcome by
  asking FERB's local server; `browsers.pair_fallback` and `browsers.pair_abandoned` are journaled.
- DMN-595: `about.diagnostics` (planner lookup and every worker) returns the running build, each profile's
  connection, permissions, FERB's own log lines and its own journal rows by kind: redacted, labelled ferb, bounded to
  16,000 characters, read-only. The catalog's one named addition is `ferb-gateway/src/catalog_additions.json`.
- ferbd tests: `wait_ready` waits until the stand-ins socket takes a connection (its file exists from `bind()`, a
  moment before `listen()`): the "control: Connection refused" failures seen under whole-workspace load.

**Why.** Owner, task 254ed34c: asked "whats the list so far???" and FERB said it couldn't give the names, while two
`task.evidence` lookups would have returned the 31-agent list (`last_report` was always empty: `supervisor.py:2155`
reads a report's `summary`, which no report has). The same answer ended in seven one-line tab notices, its tabs
stayed open after the profile dropped, and the owner asked for the old pairing method as a fallback ("the pairing and
closing them after paired is priority") and for FERB to see its own errors.

**Evidence.** `progress_answers`, `running_records`, `texts::several_leftover_tabs_are_one_grouped_message`,
`cleanup_wake` (3), `browser_keepalive`, `b_browsers::a_windowless_start_that_gets_no_answer_falls_back_and_leaves_no_tab`,
`diagnostics` (2), the catalog tests.

**Fixed against Python.** The empty `last_report`; one line per tab (`leftovers.py:28-52`); the wake page closed only
after a pairing that succeeded.

**Additions (not in Python).** The cleanup wake; the task-profile and pairing-fallback journal rows; the worker's
in-run words in the status row; `about.diagnostics` (`PLANNER_TOOLS` 6 → 7, token reserves grow by its row).

**Open.**
- Live: a real model reading `task.evidence` and `about.diagnostics`; a real browser's reconnect within 35 s; the
  wake page closing itself on Chrome, Brave, Edge and Arc.
- Why the live profile dropped mid-task is unknown; the keepalive runs in the composed daemon.
- The runtime check's `PLANNER_TOOLS` copy (`ferb-daemon/src/areas_b/runtimes/mod.rs`) still lists six; ORCH-516's
  rule still names five.
- Owner's: a length limit on the worker's latest words; which of FERB's own records diagnostics shows (profile
  names, memory rows); redacted log lines reaching the provider; log retention; grouping apps left running.

## 2026-11-14 · 62 · The sink rule covers the owner's files: browser uploads, AppleScript addresses, plain shell sends

**What.** GATE-587 (owner) with GATE-588.
- After outside content was read, a browser upload of the owner's file to a site the owner neither named nor approved
  asks the form's question, keyed `(sink:new_destination, host)`, once per site per task; a Yes carries to the submit
  (`Gateway::file_send_needs`, `owner_file`).
- `mac.applescript` records the email addresses and phone numbers in its string literals as recipients
  (`script_recipients`), so an address found only outside asks.
- `ferb-policy/src/sends.rs` `file_sends` reads plain file-send shapes from the command's shlex words (curl, wget,
  httpie, scp, rsync, sftp, nc, ssh, socat); a command sending an owner file to a host found only in outside content
  asks before it runs. `destination_allowed` also refuses a host after a No under that key.

**Why.** The owner believed "it cant send something to a webpage if the webpage says so"; `outside_destinations.rs`
measured three routes around it (shell upload, browser upload then Send, AppleScript mail). The owner: "ok we can
buil it", choosing the narrow shell version.

**Evidence.** `outside_destinations.rs` (25 tests: a Yes and a No per route, contrasts, documented limits),
`ferb-policy/tests/sends.rs`, `ferb-mac-hands/tests/script_recipients.rs`. Python parity holds apart from one named
difference (5 of 490 generated `decide` cases word an AppleScript recipient "recipient"). On the archived history
(4–13 Nov, 274 task trees): narrow shell 0 questions of 614 commands; AppleScript 0 of 27; uploads 3 questions in 1
task; the full shell variant would have asked 109 times in 23 tasks. Whole workspace: 794 pass, 0 fail.

**Ran on the owner's Mac.** Read-only, immutable reads of both archive databases for counts (no personal content
kept); an independent red-team review of a wider link rule (P1/P2/P3), not built: its verdict, exact rule text,
25 tests to add and eleven holes that exist today (F1–F11: a Yes approving a whole host for good, prefill-then-click,
values in shell addresses, typing without submit, and others) are in the session report and wait for the owner's
decisions.

**Open.**
- Uploads written in scripts or one-liners, cloud CLIs and variable hosts are not caught (recorded by the proxy).
- Workspace and task-made files are not asked about.
- Owner's: the application-task case (3 questions), the workspace exception, how long each Yes lasts.
- The link rule: lock the tightened version, close prefill-then-click (F3), scope of a Yes (F1).

## 2026-11-14 · 63 · The link rule tightened as the review gave it; one answer for a file on every site; FERB says what it can't apply; pairing's fallback shown; profiles by email; apps grouped; one lookup list

**What.**
- GATE-589..592: the URL sink lets a private value go back to the same host it was read on, exactly (C1–C5: https or
  the source's scheme, the default or the source's port, no userinfo, one source unit holding every id-shaped piece,
  no nested address from elsewhere), and lets through an address read as written, before FERB wrote any of its
  values, in a page, mail, app or non-FERB file, or in the owner's words (B1–B4). Otherwise GATE-528's page rule
  applies (now with C1 and exact hosts), then the question. F3, smallest form: an address's query and fragment values
  count as typed at submit. `confirm.rs` `SameHost`, `LinkFacts`; `gateway.rs` `link_facts`; `spans.rs`
  `call_arguments`. The review's optional uppercase clause is left out (it breaks the locale-change shape).
- GATE-593..595: a file question offers "Send to every site in this task" (one file by identity, one tree, in
  memory); the answer lists the sites.
- PRM-355/356: `about.read section=answers`, computed at call time from the gateway's choice table
  (`ferb-gateway/src/answers.rs`); one prompt line (about 75 tokens); "Nothing was applied" for non-choice words.
- APP-909/HAND-524: the `fallback` pairing event and its one line on the setup screen and in Settings → Browsers.
  DMN-596: diagnostics names profiles with `account_hint`. HAND-525: apps left running grouped by reason. ORCH-641:
  `PLANNER_TOOLS` held once in `ferb-orchestrator/src/lookups.rs`.
- kb tasks T-1098 (second last: an independent full-device security audit) and T-1097 (last: the owner's navigable
  map of the code), both in PLAN.md after stage 5.

**Why.** The owner's answers of 2026-11-14 to journal 62's open list. On the link rule the owner said "im trusting
you with the safety here ... confirm and then lets lock this": an independent red-team review on the coordinator's
model found the coordinator's proposal unsafe as worded (FERB-written records laundering an address; "any occurrence"
turning attacker text into proof; sibling hosts) and gave the rule built here. The owner kept the cleanup wait at 35 s
and a Yes's host-wide, permanent scope.

**Evidence.** `ferb-gateway/tests/link_rule.rs` (38: every attack the review listed, the measured pass shapes, nine
documented holes), 5 tests added to `outside_destinations.rs`, `answers.rs` guards, `unsupported_answers.rs` (2),
`b_browsers` fallback event, `diagnostics`, grouping texts, Swift PairProgress (2). Whole workspace: 847 pass, 0 fail,
16 ignored; Swift 758, 0 failures; portable ok. Replay through the code of the 24 link questions in the archive:
today's rule asks 14, the new rule 9 (7 values carried to another site in addresses FERB built, 2 addresses seen
only in shell output).

**Corrected.** The coordinator told the owner the rule would remove about 9 of the still-asked questions (a hand
estimate); the code replay gives 5. The archive holds 24 such questions, not 25.

**Named differences from Python.** Parity: 8 of 524 URL-sink cases (`LINK_RULE_CHANGES`); scenario `tab_link` (the
shop `/dp/<id>` now opens unasked); `b_runtimes` (3 planner builds and 3 grants gain about.diagnostics); prompt
change PRM-355. Two comment-only decision tags in `tests/test_value_provenance.py` drop the superseded GATE-541/542.

**Open.**
- Live: the three-choice card, the pairing line on a real fallback, a real model using the answers lookup.
- The words path for file grants (needs the owner's change to GATE-512); grants are lost at a restart.
- Link holes documented as tests: a plain-word space on the same host, a recipient prefilled by the address (F2),
  the matching link in a list, reversed or split values, a value in a shell command's address, typing without
  submitting, any value to a named or approved host, the owner's open-tab addresses, a file an earlier task wrote.
  These are T-1098's starting list.

## 2026-11-14 · 64 · Pairing opens the profile's window with FERB's wake page, and nothing windowless

**What.** HAND-526 (owner; supersedes HAND-520, HAND-523, HAND-524, APP-909): a profile that isn't connected is paired
as Python paired it: its window with FERB's local wake page, the load time (2 s), then the pairing page; a connected
profile gets only the pairing page. The wake page closes itself on every outcome (`areas_b/browsers/wake.rs`,
unchanged). Removed: the windowless first attempt, `pair_quiet_wait_s`, `PairWait`, the `browsers.pair_fallback` and
`browsers.pair_abandoned` rows, `step: "fallback"`, the stream's `fallback` event, FERB.app's fallback line and its
tests. The drivers' ordinary wake and the cleanup wake are unchanged (owner: "it queitly start and does what it needs
to do. SO DONT REMOVE OR CHANGE THAT").

**Why.** Owner, live on build 202611141614 (fresh install, browsers left running): "that fallback worked, but the
minizmied one shifted my focus anyways. SO lets just ahve the proven working method. REMOVE the minized one and eveyr
other code it added". From the system log: the six `--no-startup-window` commands (22:39:29) moved nothing; the six
pairing pages handed to profiles with no window (22:39:31) were followed by Brave and Chrome making themselves
frontmost and a Space switch. A quiet start with no address opens no window, which is why mid-task wakes never moved
focus.

**Evidence.** `b_browsers`: Python's recording replays with its commands (7 wake pages, 2 `wake_close`);
`pair_all_leaves_each_profile_with_only_the_owners_tabs`; `the_wake_page_closes_itself_however_the_pairing_ends`.

**Fixed against Python.** The wake page closes itself on every outcome (Python: only after a pairing that succeeded).

**Open.** The visible method opens a browser window by design and keeps no front app. Live: pairing on the next build.

## 2026-11-14 · 65 · A stop reaches a waiting plan; a wait names an event FERB publishes and reads its task's recorded end; refused hellos name their sender

**What.**
- Corrected drift: the Rust `cancel_tree` did not wake the plan run as `supervisor.py:6130` does, so a stop never
  ended a step parked on an event. A stop and a continue now wake the wait; the wait registers its wake-up before it
  checks anything.
- ORCH-642: a plan's wait or loop event must be in `ferb-orchestrator/src/waits.rs` `WAIT_EVENTS` (`task.finished`,
  `timer.fired`) with only that event's fields matched; otherwise the planner is told the names through the repair
  path. Guards: every bus topic is sorted into `WAIT_EVENTS` or `NOT_WAITED`; every topic published by name is on the
  bus. A plan's own structural checks run for every plan.
- ORCH-643: a wait on another task reads that task's recorded status at its start and on every wake (ended before
  the wait began: continue at once; any end but done: the step fails with that fact).
- ORCH-644: a waiting step's own line ("Waiting for “…” to finish"), shown by FERB.app's checklist
  (`StepChecklist.swift`, `StepState.line`); `waiting_steps` in the planner's task facts while a step waits (what,
  since when, minutes, continue or stop).
- ORCH-645: an open browsing-profile question says what its answer settles, so a plan built meanwhile doesn't ask
  again.
- HAND-530: a hello refused for its MAC or as a replay records its sender (extension origin, browser process,
  connection, counters, the profile's accepted connection).
- No prompt changes (owner: "where are you addin it?? the systme promtp again?").

**Why.** Live on build 202611141614, tree 15737cce: the planner waited on an invented `task_completed` event and the
plan sat for over 45 minutes; the owner's "cancel them all" (twice) and six presses of Stop task did not end it
("ok so waiting takss cant be cacnelled currnelrty??? FIX THIS"); FERB answered "I've asked it to stop; it's still
finishing what it was doing" with nothing running; the plan asked for a browser profile the owner had picked 20 s
into its turn. Chrome Profile 15's `bad_mac` repeated 1,119 times with no sender recorded.

**Evidence.** `ferb-supervisor/tests/stop_waits.rs` (3: the live plan shape, an event wait alone, a time wait; the
event-wait tests fail without the wake-up), `task_waits.rs` (5), orchestrator `waits` (3 + guard),
`ferb-browser/tests/refused_sender.rs`, Swift `testAStepWaitingForAnotherTaskSaysWhatItWaitsFor`.

**Fixed against Python.** A plan's own checks run when no step lists inputs (since ORCH-625 Python skipped them);
an unknown wait event is refused (Python waited forever too).

**Named differences.** `plan_repair` (one more problem named, budget +42); `event_wait` replays with `timer.fired`;
the refusal's sender facts.

**Not changed.** The link question "Open www.amazon.in? The link includes “asus vivobook oled …”" is the locked rule
working as written: the phrase counted as private only because Google's results page, read in the signed-in
profile, echoed FERB's own query (T-1075). A proposal ("FERB's own words echoed back are not private") is with an
independent review, not built.

**Open.**
- Owner's: an upper bound on waits (none added); the link-rule adjustment after its review.
- Which extension context sent the bad_mac hellos (leading theory: the previous install's key kept in the
  extension's storage, since the store was wiped and the browsers were not); `pair_failed` decided from a refusal
  in the same instant as a pairing that succeeded; a re-key needs an extension protocol change.
- The Tasks list shows the last finished step's text while the task waits.
- A test run hung 1 h 40 min on six copied `ferb-secrets` helpers that never started (syspolicyd restarting, as in
  journal 58); long test commands now run under a timeout.

## 2026-11-14 · 66 · FERB.app's chat: each question waits only on its own answer; a row's picture detaches; the plan folds

**What.**
- APP-910: one question waits only on its own answer (`NotchModel.sendingAnswers`, a set of approval ids). One
  app-wide `sendingAnswer` flag (since 2026-10-13) had disabled every open question's buttons and Send, and silently
  dropped a typed answer's Return, while any one answer's call was out.
- APP-912 (owner): a card row's or checklist line's preview opens the main preview's short view (`TaskShortView`)
  with its Detach window for that step (`PreviewSubject`), through the one path `TaskActivityWindows` already used.
- APP-911 (owner): the plan message folds by the same rule and component as answers (`CollapsibleText`);
  `FirstReplyView` had never been wrapped in APP-725's fold.

**Why.** Owner, live: "why are htese unselectable??? but when i clsoe and open chat again, they are visible then";
"i wnat these row live preview circles to expan the smae as the below main one"; "why diens it follow the show more
logic we have for other thigs??? IT SHOUDL FOLLOW IT". Four questions were open at once (a plan's three asks and a
link check).

**Evidence.** Swift 760 tests, 0 failures: `QuestionAnswerInFlightTests` (fails with 8 assertions on the old code),
additions to `TaskActivityWindowsTests` and `FoldClipTests`. Not a Rust wire difference; the Python build's app had
the flag too.

**Open.** Why an answer's call stayed out for more than 15 s (before the daemon read it; nothing records where).
`TranscriptList.setEnvironment` refreshes built rows on only four keys (a row built disabled would stay dimmed;
nothing triggers it today). The owner's eyes on all three on the next build.

## 2026-11-15 · 67 · A part's quote is compared as words

**What.** ORCH-646: a plan part's quote passes when its letters-and-digits words, in order and unbroken, are words
the owner typed (`ferb-supervisor/src/words.rs` `said_as_words`, tried after `quoted`'s character comparison in
`flow/turn.rs`). Punctuation, quote styles, line breaks and marks a paste carries are not words; words the owner
didn't type, or in another order, are still refused.

**Why.** Owner, live on build 202611142038 (02:27): "i pasted some takss but it rejected after a loooong thignking".
The pasted text came from the coordinator's terminal reply and carried its quote bar and line breaks between words
("give me the \n  ▎ link."); the planner quoted the plain words; `quoted` (items.py:157, compared by characters)
refused three parts, the one repair gave the same quotes, and the request ended after two minutes as "something broke
inside FERB (PlanRejected…)". The check is the Python build's, unchanged since the first port.

**Evidence.** `ferb-supervisor/tests/quote_words.rs` (the live paste's shapes pass; other words, another order, an
added sentence and an empty quote are refused). Whole workspace: 861 pass, 0 fail, 16 ignored.

**Fixed against Python.** `quoted` compares characters, so one pasted mark between two of the owner's words refused
the owner's own request.

**Open.** Memory's `remember` quote check still compares characters (`ferb-memory` `contains_words`): the same paste
would refuse a remembered preference. The same run asked "Which Chrome profile should I use for the Google Sheet?"
as a plan step on a fresh install with no profile picked: not examined.

## 2026-11-15 · 68 · Quotes compared as words everywhere; a waiting task says so; the browsing question for owner-identity tasks; a cancelled app call ends

**What.**
- MEM-430 (supersedes ORCH-646): the one quote check, `ferb-memory` `terms::quoted`, accepts a part by Python's
  character match or when its words, in order and unbroken, are the owner's (`said_as_words`, moved from
  ferb-supervisor). Every acceptance check uses it: plan parts, remembered items and their end dates, schedules and
  reminders, side entries, imports. What is saved as the owner's words is still their exact clause; a forget's clause
  check stays Python's.
- ORCH-647: when every live step of a tree is a waiting wait step, its live line (`task.progress`) is the wait's own
  line (tree.rs `say_waiting`), so FERB.app's Tasks list no longer shows the last finished step's words.
- ORCH-648: a tree the planner gives identity "owner" whose task names sites, and whose request words name none of
  the owner's profiles (`names_profile`), goes through `browse_as`: without a pick FERB's one browsing question is
  asked before the plan runs (answer saved); with a pick the tree is `as_owner`. The no-pick fact says FERB asks once
  itself and a plan never asks (`NOT_PICKED_TAIL`).
- APP-913: `DaemonRPC.call` closes its connection when its Swift task is cancelled; a call of 3 s or more is logged
  by phase (queued, connect and hello, reply), method name only.

**Why.** The owner's live checks on build 202611142038 ("all passed. works, we can move forward") left journal 65–67's
open items: the Tasks list showed a finished step's words for a waiting tree; on the fresh install the plan asked
"Which Chrome profile should I use for the Google Sheet?" as a step (02:29:53) with `browsing.choose` never asked and
nothing saved, and asked again at 02:39:33; an answer's call had stayed out over 15 s on 11-14.

**Evidence.** ferb-memory `quote_words.rs`, ferb-learning `capture_words.rs`, supervisor `task_waits.rs` (+3) and
`owner_browsing.rs` (4), daemon `b_browsers` names_profile, ferb-rpc `a_call_held_open_on_one_connection_…`, Swift
`testACancelledCallClosesItsConnectionAtOnce`.

**Fixed against Python.** items.py:157 compares characters; plan_executor.py:47 told a wait's state, never a line;
supervisor.py:3004 asked the browsing question only for identity "ferb".

**Named differences.** 0 of 18 (memory) and 0 of 1,116 (learning) recorded quote cases flip. `event_wait`,
`wait_step`: the task-level wait progress frames are left out before comparing. `login_sites`: an unrecorded
`browsing` call is answered `{}`. daemon-b environment: three `_facts` cases carry the new no-pick fact.

**Not a drift.** FERB.app opens one connection per call, and Python's server, like the Rust one, serves each
connection's calls in turn: the daemon cannot hold an answer behind a preview. The coordinator's hypothesis was wrong.

**Open.**
- Owner's: remembered preferences as the owner's words for a profile; whether `picked_only` stays a refusal.
- A plan built before the pick exists can still add its own ask step; owner-identity tasks with no sites still browse
  where the worker names.
- Where the 22:47 answer's 15 s went: not determined; the new rpc log line will show it next time.

## 2026-11-15 · 69 · Stage 2 begins: the chat as entries, projected in the engine and folded back to FERB.app's transcript

**What.** ENG-8: `ferb-core::chat_entry`, the entry contract (kinds, states, `pos`, `seq`, `rev`, placement, typed
bodies; portable). ENG-9: `ferb-supervisor/src/flow/entries.rs` `ChatProjector`, a pure port of FERB.app's reducer
that turns frames and events into entry changes or live state. `NotchCore/EntryFold.swift` folds entries into
`ConversationArchive`. Nothing is wired; no schema, no RPC, no runtime change.

**Why.** Stage 2 (ENG-1): one conversation for every client, computed once in the engine. Today FERB.app writes the
history rows (`chat_history`, last writer wins, paged inside one day), a stream's frames reach only the connection
that sent the message, `chat.send` has no idempotency key, and "shown" is one global flag. The design (a
`chat_entries` table written by the engine where it emits frames; the journal stays the model-facing record) was
chosen over deriving from the journal (append-only; plan steps deleted after 90 days) and over moving the app's
opaque rows (a Swift shape as the cross-platform contract).

**Evidence.** 71 recorded scenarios turned into timelines give `rust/fixtures/chat-entries/scenarios.json` (not
Python-recorded: there is no Python counterpart). `ferb-supervisor/tests/chat_entries.rs` (8: golden bytes; one
message per ack, no live frame stored, `seq` only grows, `pos` set once, each revision changes something).
`EntryFoldTests`: 71 of 71 transcripts equal the app's own reducer's, with a sensitivity test and a legacy test.
The agent corrected 13 points of the coordinator's frame mapping against the code (step states are live only; the
engine never emits an `error` frame; notices are never revised; and others).

**Seen once, not explained.** In the coordinator's combined workspace run, ferbd `chat_replay`
`the_more_scenarios_through_the_program` failed once (which scenario was not captured) and passed three times alone
afterwards.

**Open.**
- App bugs found by the fold, not fixed: every stream frame after a turn's first answer is dropped
  (`Conversation.swift:72-90`, `NotchSession.swift:226`: in `conversation_change` three of four replies never show);
  a one-ask outcome's `unconfirmed` is read as `note`.
- Questions are never closed by a restart in the entries (wiring's job).
- The schema (slice 3) waits for stage 1 recorded as cut over and the owner's word on the shared conversation.

## 2026-11-15 · 70 · A chat's frames and a step's answer in Python's order however the threads run

**What.**
- The order of a chat's frames (ENG-3). A worker's tool call runs on its connection's task, beside the chat's; it
  tells the tree's line `tool.called`, its progress line and `approval.requested`, and the stream's reads cut that
  run wherever they happened to fall. ferb-worker `bursts.rs`: whatever one poll of a producer (the tree's work, a
  worker's tool call) tells a tree's line goes onto it as one message; `run_tree` forwards it between two wake marks.
  A poll ends where the work suspends, as a Python coroutine's run does (gateway.py:2256: publish, hooks.started,
  the question; `_chat_send`'s two readers, chat.py:360/402).
- An answer given in chat: the answering turn holds the waiting step (`answer_holds`, the loops' `SteerHold`) until
  its reply is recorded, so `chat.reply` precedes `plan.answer` as in Python.
- ORCH-647: a stopped or ended tree tells no wait line (`say_waiting`).
- `a_wait_beside_an_open_question_stays_quiet` now runs its plan (it had been refused for two terminal steps and
  passed without testing anything); ferbd's `plan_waits` waits for the wait step's waiting state.

**Why.** `the_more_scenarios_through_the_program` failed once in the coordinator's combined run (journal 69) and
passed three times alone.

**Evidence.** A reader lagged 30 ms per message (a temporary patch, removed) changed `route_question`,
`correct_pending_check`, `conversation_change` and `conversation_resume` before the fix; after it all 61 scenarios
pass with 30 and 150 ms lag. `answer_in_chat` differed once in 9 natural runs (journal row 23: `plan.answer` where
Python has `chat.reply`); with a 300 ms pause it differed every time before the hold and never after. Unfiltered
`chat_replay`: 30 of 30. `task_waits` `a_stopped_task_says_no_wait_line_after_the_stop` fails without the guard.
The coordinator's runs on the combined tree: `cargo test --workspace --no-fail-fast` three times, 884 passed, 0
failed, 16 ignored each; clippy and portable clean; Swift 766, 0 failures.

**Corrected drift.** Both orders were Python's by its single loop and the Rust engine's only by timing.

**Open.**
- Which of the two the original failure was is unknown (it was not captured).
- The harness's 30 s timeouts fail under load averages above 100 (16 CPU hogs): `plan_repair`,
  `check_in_after_five`, `plan_parts_checklist` gave empty lines. Not an order.
- `probe_watch`'s harness still sleeps 500 ms.

## 2026-11-15 · 71 · Every reply of a turn shows; the owner's decisions on the shared conversation and the cutover

**What.**
- APP-914: other words FERB says after a turn's answer (a reply while its task runs, then the task's end; more
  replies to one message) are a later answer: in FERB.app a `ConversationReply` with its own words (`said`), placed
  after the newest turn, with no quote line right under its turn. `receiveAfterAnswer` takes a turn's frames after
  its session ended (`Conversation.swift:77` dropped them once `finishBackendWork` cleared `activeTaskID`);
  `ConversationArchive.finish` no longer overwrites an answered turn with other words. `ChatProjector` mirrors it
  (a task's end after a reply `follows` it); `EntryFold` draws answers that follow.
- APP-915: the app reads an outcome's `note` and never `unconfirmed`. ORCH-606 (owner, 2026-11-11: one fixed short
  line) renamed the field on purpose; journal 69's "app bug B2" was not one, and the coordinator was wrong to call it
  so.
- ENG-10 (owner): one conversation kept on the Mac; phones fetch it like from a server and store what they fetched;
  FERB.app's behaviour and UI for earlier messages stay as they are, and turns started on a phone appear in it.
- ENG-11 (owner): stage 1 is cut over. Schemas above 0062 may be added; the Python daemon leaves the repo in its own
  commit.
- PLAN: the phone apps' look is `~/Desktop/ferb-glass-lab`, the same on iPhone and Android (owner; to be recorded
  as a decision when stage 3 starts). For the Mac app it stays undecided ("i like the settings window part of it not
  all").

**Why.** Journal 69's open items, and the owner's answers of 2026-11-15. On the dropped replies the owner asked why
they had never met it: it needs more than one reply to one message, which their recent use (one answer, or one
task's result) never produces; the recorded cases are delegated conversations. The owner: "ok fix them if you know
it".

**Evidence.** `EntryFoldTests` 71 of 71 with each later answer's words compared, and a sensitivity check that
catches a dropped later answer; `testEveryReplyOfATurnAndItsTaskEndAreShownInOrder`; `ArchiveUpgradeTests` (a later
answer saved and read back; an older reply row read as before). Swift 766, 0 failures. The golden's one change:
`conversation_handover`'s task end `follows` the handover reply.

**Open.**
- Not live-checked; the new regression test was not run against the old code (reasoned from it).
- A handover now shows two bubbles saying nearly the same thing (left for the owner's eyes).
- "Conversation pause: <id>" (`supervisor.py:3369` and its port) shows a raw id against ORCH-519: to fix.
- An older app build reading a row saved with `said` shows the turn's first answer there.

## 2026-11-15 · 72 · Stage 2's store, wiring and calls: the conversation read like from a server

**What.**
- ENG-12: migration `0063_chat_entries.sql` (the first schema above 0062, allowed by ENG-11): `chat_streams`,
  `chat_entries`, `chat_submissions`, `chat_clients`, `chat_client_marks`, and the backstop's watermark;
  `chat_history` rows carried over as `legacy` entries with their bytes (`chat_history` stays). ferb-store
  `chat_entries.rs`: pages, changes, cursors, idempotent submissions, forward-only marks, tombstones, and the
  read-only archive import (idempotent; not run, not wired to run). A stream's own entries start at pos 2^32 so
  imported older history always goes below them and `pos` is never renumbered. All six tables are kept forever, as
  `chat_history` is.
- ENG-13: `ferb-supervisor/src/flow/chat_log.rs` feeds `ChatProjector` from one queue on the producers' own paths;
  the ack is written in one transaction with its `chat.ack` journal row and the submission row; a writer task drains
  the rest; the bus is read by the daemon's last events consumer; projectors are rebuilt from stored entries after a
  restart, with the journal backstop and a start-up pass (cut work `interrupted`, open questions expired).
- ENG-14: `chat.entries.page|changes|get|days|watch|mark`, and `chat.send` with `client_id`/`submission_id` (a
  repeat returns the same ack and starts no turn; a caller without them gets today's bytes; a client id with no
  session writes in the current conversation, not "default").
- ORCH-649: a conversation change is said by who it is with ("Paused the chat with Robin."), never its id.
- T-1099 (owner): the phone apps get a sign-in button for Claude, Codex or an API key, as the Mac has.

**Why.** ENG-10, ENG-11; journal 71's raw-id item.

**Evidence.** All 61 scenarios through the binary keep frames, model calls and Python's tables byte-equal. 7 new
tests through the real daemon (a repeated submission; two clients at the same instant; a second connection watching
a turn live; a watcher catching up exactly after reconnecting; `reset_required` and `cursor_ahead`; long bodies and
`max_bytes` cuts; forward-only marks), 13 store tests, 3 projector tests. The coordinator's runs: 908 passed, 0
failed, 16 ignored, twice; Swift 766, 0 failures; clippy and portable clean. The method table is Python's 106 plus
the 6 named additions.

**What the entries comparison does and does not prove.** The stored entries equal the golden strictly on 33
scenarios. On 28 they are compared with named differences (`chat_replay.rs` `ENTRY_DIFFERENCES`); for 26 of those
the difference is `Order`: the golden places a stream's frames at the stream's end (the recordings keep each
connection's order, not the order across connections), so those scenarios are compared as the same entries in their
final states WHATEVER THEIR ORDER. So for scenarios with two streams or a question beside a stream, the engine's
stored order (`pos`, which is what a phone displays) is not checked against anything: it is the order of commits,
by construction, and unverified against what FERB.app shows for the same moments.

**Corrected drift.** Draining frames synchronously moved `confirm_yes`'s confirmation id ahead of the question
record; fixed by queuing only on the producers' paths and reading the bus last.

**Open.**
- An order check for the 26 scenarios (at least structural: a message before its plan before its answers; an answer
  after what it answers; a question between its task's plan and end), and the comparison with FERB.app's order when
  its change lands.
- FERB.app's one change (turns a phone started); the phone role; the archive import's run with the owner present.
- A notice cleared by a forget, or a tombstone, is not pushed live to watchers (they see it in `changes`).
- Not live-checked.

## 2026-11-15 · 73 · The stored order checked against the engine's own interleaving; FERB.app shows the turns another device sent

**What.**
- ENG-15: seven order rules for every conversation's stored entries (`ferb-supervisor/tests/support_entries`): `seq`
  strictly grows in commit order and `pos` never moves; a message before its request's plan, answers, errors and
  questions; a plan before its task's answers and its workers' questions; a question before the answer that told its
  task's end; `answers` and `follows` name lower positions; and catch-up from any cut gives exactly what changed since.
  Checked on the wired log's trace after every scenario the program replays and on the pure projector over invented
  interleavings. The engine's real interleaving of the 61 replayed scenarios is kept in
  `fixtures/chat-entries/engine_*.json` (not Python-recorded), and `EntryFoldTests` drives FERB.app's reducer in that
  order. Every entry of a turn carries its message's `client_id`; `chat.attach` adds the `entries` capability.
- APP-916, APP-917, APP-918: FERB.app watches `chat.entries.watch` and applies only other clients' turns, through
  EntryFold's per-entry functions, saved through its own history; one writer per turn (`sentElsewhere`); the cursor
  moves after the history write; `chat.send` carries the install's `client_id` and a `submission_id`, with one resend
  if the link drops before any frame. Without the capability the app behaves exactly as before.

**Why.** Journal 72: on 26 scenarios the stored order was compared with nothing. ENG-10: FERB.app keeps its behaviour
and shows a phone's turns.

**Evidence.** Rules: 61 of 61 replayed scenarios; 1,826 of 1,826 invented timelines; a sensitivity test per kind of
break. App order: 61 of 61 transcripts equal when FERB.app's reducer is driven in the engine's order, 26 of them in
an order the golden doesn't have; two runs wrote identical fixture files. `EntryWatchTests` (10): the 71 transcripts
unchanged with the watch connected; a foreign turn live, after a relaunch, interleaved by `pos`, answered from the
Mac; no duplicate; an old daemon unchanged. Rule 4 needs no exception: APP-580 decides where an answer shows, not its
`pos`.

**Seen once.** `correct_pending_check` swapped a journal row's and a span's ids under two parallel replays while the
trace was written inside the drain's transaction; not reproduced in five solo runs, nor after the trace moved past
the commit.

**Named additions.** `chat.attach`'s `entries` capability. The golden's recorded multi-lane timelines break rule 4
(an artifact of their order) and are not used as orders.

**Open.**
- Not live-checked: a second client against the real daemon is the owner's check (a script on the socket with the
  `cli` role; the toolbox has no command for it).
- For the owner's eyes: a foreign message arrives quietly; the app waits up to 3 s at connect for the catch-up;
  turns from before the first watch are not imported.
- One Swift run printed CoreData errors about the owner's AddressBook store: some test reaches the Contacts service
  (which one is not looked into; tests must not touch the owner's data: TEST-508).

## 2026-11-15 · 74 · The redactor measured against the public scanners' lists and widened; labels followed by code stay readable; personal identifiers masked by layout and check digit

**What.** All in `ferb-sanitize`, as hand-written Rust-only layers applied with the ported rules (`generated.rs`
untouched; the Python-recorded fixtures unchanged):
- GATE-596: `shapes.rs`: 53 vendor token shapes, webhook URLs, Telegram tokens, AWS key pairs, WIF, PuTTY and JWK
  private members, URL `;param=` credentials as named kinds; labels in XML, attribute pairs, `=>`, `:=`, setters;
  command-line passwords; `.netrc`/`.pgpass`; Kubernetes Secret data; labelled seed phrases; password hashes;
  environment/version suffixes, listed glued names, `…_PASS`/`…_PW`; password words in other languages.
- GATE-597 (owner: "yes to 2 and 3"): publishable keys (Stripe `pk_`, Razorpay key ids, Mapbox `pk.`), public keys
  and certificates are not masked.
- GATE-598 (owner: "yes"): after a secret label, what is structurally not a value stays readable: a comparison, a
  type annotation, a placeholder, an environment or config lookup, markup, a publishable key. A quoted literal
  compared with a label is masked (new).
- GATE-599 (owner: "run a worlwide PII info reaserhca nd then mkae it like the maskign list"): `pii.rs`, 59
  identifier kinds from about 40 countries: 26 found on their own (issuer layout plus check digit), 33 only after a
  label; contact data and technical identifiers are not masked.
- `kb/gate-allow.toml`: ten `[[secret]]` exceptions for the invented key-shaped values of
  `rust/fixtures/sanitize-shapes` (the repo's own commit check flags them; nothing was re-encoded).

**Why.** The owner asked whether passwords and API keys in a repo are masked before a model reads them, then: "just
make sure this seed regex file is good and inclusive of almost all secrets there can be of a shape. if not update
adn justify why". The coordinator's first answer (15 common shapes, all masked) was true and flattering.

**Evidence.** `rust/fixtures/sanitize-shapes` (NOT Python-recorded): of 238 secret shapes taken from gitleaks,
trufflehog, GitHub secret scanning and detect-secrets, 126 were masked before and 238 after; the stream agrees. The
ten false positives of the ported label rule are now negatives that stay readable. Identifiers: 70 of 70 invented,
check-valid ids masked; 28 look-alikes readable (phones, timestamps, tracking numbers, ISBNs, amounts, coordinates).
Over the repo's 2,227 text files (85 MB): 367 values that were masked are now visible, every one code or a type
(read in full); the new rules mask 4 + 10 more values, all invented test data. Python-recorded cases that flip: one
(`token2` is a secret now) of 8,706. CPU: about 0.8–1.0 ms per KB with or without the new layers (85.0/87.9 s and
66.9/68.8 s over the 85 MB); the coordinator's timing of the existing redactor: 0.4 ms for 2 KB, 8–12 ms for 20 KB,
37 ms for 60 KB. Whole workspace on the merged tree: 921 passed, 0 failed, 16 ignored; clippy and portable clean.

**From the code, for the owner's questions.** The owner's chat message is redacted before any model sees it and the
raw text is discarded (`flow/chat.rs`). The hidden-input path already fills any value into a field without a model
seeing it (`Gateway::ask_secret`, the browser's `secret` action, `D_SECRET`), with `remember` to save it: masking
identifiers does not break form filling. No per-value "don't mask this" exists; `Redactor::configure`'s exclusions
only add masking.

**Open.**
- The check-digit rules other than the few tested against published specimens are validated only against the
  author's own generator: an independent verification against specifications and python-stdnum is running.
- Still through: an unlabelled password that looks like a word; an unlabelled seed phrase or 64-hex key; a secret
  only base64-wrapped; positional arguments; PKCS#12 blobs; a Kubernetes Secret as JSON; bare SIN, Taiwan and South
  African ids; passports and accounts with no label.
- Owner's: an owner-granted "don't mask this" for identifiers (designed, not built) and its scope; whether a saved
  id fills on any site; contact data and technical ids left unmasked (recommended); a sealed handle minted when a
  chat message is redacted so an id is not typed twice.

## 2026-11-15 · 75 · Personal identifiers checked against python-stdnum: narrow rules widened, two look-alikes refused, Norway and CLABE added

**What.** GATE-600 refines GATE-599, in `ferb-sanitize/src/pii.rs`: Portugal NIF takes any first digit but 0;
Ireland PPS ignores an old-format ninth W, T or X; Israeli ids are padded from 5–8 digits; the Steuer-ID gets its
repetition rule; the Personalausweis's nine printed characters count after its label; NIR reads 2A/2B only as the
department; IBAN has the registry's 89 lengths; PAN takes K; the voter id gets its Luhn digit; an Aadhaar VID never
starts 0 or 1. Label-only, shape-only kinds for Singapore's M series, Taiwan's 8/9 and Sri Lanka's 12 digits; new
kinds for Norway's fødselsnummer and Mexico's CLABE. An IPv4 address after a label is never a value. The broken
Hebrew passport label is fixed and about 40 labels added. The header now says the ported card rule masks IMEIs and
plain South African and Emirates ids as cards. The corpus generator's unrealistic values are corrected.

**Why.** Journal 74's open item. An independent review rebuilt every rule and compared it with python-stdnum 2.2
and published specimens: no checksum algorithm was wrong; some rules were too narrow (494313650, 6433435FT and
3933742-3 are valid and were missed) and two look-alikes passed (WKH1186263, 36554266806). 41 of the 59 kinds were
verified against an independent source.

**Evidence.** stdnum as the oracle: 300 valid numbers per changed kind, all accepted and masked after their label;
every check-character mutation agreed (pt_nif 2,700; ie_ppsn 6,600; il_id 2,700; de_steuer_id 2,700; fr_nir 2,667;
iban 2,670; pan 4,500; in_epic 2,700; aadhaar_vid 3,300; no_fnr 5,400). Corpus: 333 of 340 positives masked (7
declared limits), 88 of 88 identifiers; 138 of 139 negatives intact (the IMEI is declared). Repo text (2,271 files,
88.7 MB): new maskings only in the changed test and corpus files. Python-recorded cases that flip: 0. Workspace: 922
passed, 0 failed, 16 ignored; clippy and portable clean.

**Open.**
- Belgium's 2000+ variant and Norway's "not born yet" rule need a clock the redactor does not have.
- No oracle for HKID, Emirates, Saudi, AU Medicare, 8-digit TFN, UK driving licence, Singapore M series, Taiwan
  8/9, Sri Lanka 12-digit, CLABE (one published example), the Personalausweis.
- GitHub's push protection refused commit 70d94eae for the corpus's invented key-shaped values (over 60 detections);
  the commits are local until the owner chooses how to allow them. No value was altered and no scanner
  configuration added.

## 2026-11-15 · 76 · Ready to remove the Python code: trace, stats and the extension zip in Rust, release tooling off Python, kb paths read as history

**What.** The preparation ENG-16 asks for before ENG-11's deletion; the deletion itself is T-1111 (not run).
- `ferb trace [TASK] [--full|--json]`: the toolbox calls `trace.get` and writes ferb-trace's `trace_output`
  (ferb-device now depends on ferb-trace). `ferb logs` prints a tool step as ferb-trace's step line (it printed a
  placeholder). `ferb stats [--days N]`: `ferbd/src/stats.rs`, the database read-only, time injected (`Now`).
  `ferb browsers extension [path|install|package] [--out]`: `ferbd/src/extension_cmd.rs` over ferb-browser's
  `package` and ferb-device's `install_extension`. `ferb daemon run` and `ferb mcp` name `ferbd daemon`/`ferbd mcp`;
  `runtimes` and `memory dashboard` stay unported (T-1112).
- Release tooling: `scripts/sign-app.sh` (was `build-app.sh --sign-only`); `build-dmg.sh` builds with
  build-app-rust.sh; dmg_layout.py and make_logo.py carry PEP 723 pins with `scripts/*.py.lock` (`uv run --script
  --locked`); release-appcast.sh refuses an app carrying Python and reads only the Rust pins; release_checks.py names
  the Rust version homes; REL-401's cap (720/750 MB) is `ferb_rust_size_cap` in build-app-rust.sh.
  `scripts/tests`: 65 stdlib unittest tests ported from four pytest files (dropped: those of the Python build itself).
- kb: `historical` in kb/config.toml (PROJ-509); inactive until the deletion names its commit.
- Tests found or written for 17 decisions whose only tests were Python (REL-2, REL-401, REL-419, REL-423, REL-424,
  APP-525, ORCH-527, GATE-2, 219, 223, 504, 531, 533, 536, 544, HAND-238, 241); 98 others are named by T-1100..T-1110.
- Records: ENG-16 (owner), ENG-17, PROJ-509, REL-425..428 (REL-427/428 supersede REL-410/REL-3). Headers of the
  generated tables and `rust/fixtures/README.md` say how a fixture script runs at the commit before the removal.

**Why.** ENG-16 (owner): "port eveyrhig thats needed to be"; the kb keeps all its history.

**Evidence.** `ferb trace`: 220 of 220 recorded outputs (story, full, json, plain) byte-equal through the program's
dispatch. Step lines: 6 of 6 equal to ferb/cli/logs.py's. `ferb stats`: 20 of 20 Python-recorded cases
(`rust/fixtures/stats`) equal, with the exit status. Extension: manifest 0.19.10 at or above all six minimums,
protocol.js 21, the key gives FERB_EXTENSION_ID; `package`'s lines and refusals equal Python's for the same inputs (the zip's bytes were not compared with Python's).
`scripts/tests`: 65 run, 6 skipped (need a built bundle), on /usr/bin/python3 3.9.6, 3.12 and 3.14. kb, simulated
after the deletion (files hidden, `historical` = HEAD): 0 path errors (1,602 decision paths and 24 area paths read at
that commit), 98 coverage gaps, all named by tasks. The Python suite passes (two Python tests changed to the
deliberate script changes: the appcast's fake app has no Python; `ferb_release_constants ... python` is refused).

**Open.**
- Tags in ferb-gateway for the coordinator: GATE-219, GATE-220 (scenarios.rs), GATE-536, GATE-540, GATE-537
  (link_rule.rs). TEST-504 is a gap: the Rust tests have no per-test timeout, memory cap, model-call or network guard.
- The not-running line still says `ferb daemon run` (client.rs; mcp_bridge.rs's copy reaches a model): T-1112.
- Not run: build-app-rust.sh, a signed or notarized build, `ferb browsers extension install` (writes to the
  browsers' folders), stats on a real database.
- `ferb stats` errors are one stderr line where Python printed a traceback; its repeat count reimplements
  `repeat_stats` on a read-only connection (ferb-store has no read-only `Db`).

## 2026-11-15 · 77 · The owner's identifier choices: "don't mask this" for one value, saved IDs on any site, IDs typed in chat kept sealed

**What.** Only for the personal identifiers `pii.rs` masks (GATE-599); passwords, keys, tokens, OTPs and card details
are untouched (rule 7). GATE-601 records the owner's words; GATE-602..604 are the mechanisms.
- GATE-603: `OwnerSecrets` keeps identifiers in one record, `"any site"` (kind `"identifier"`), never linked to a
  site. `reveal` falls back to it when the target's own record has no entry of that name (a site's own entry wins);
  card fields and whole-browser targets still refuse. `secrets.list` names them as `identifiers`. A value typed into
  the hidden input with `remember` goes there only when `ferb_sanitize::identifier_kind` finds it whole as a bare
  identifier (issuer layout and check digit, no card, secret format or secret label) and the field is no card
  field; anything else keeps its site exactly as before.
- GATE-604: `chat.send` seals each identifier of the owner's own text that the redactor masks and no other rule
  covers (`Redactor::identifiers`) through a new `Daemon::seal_identifier` seam (ferbd: `OwnerSecrets::
  seal_identifier` on the helper, the wait marked blocking). Names are the kind ("PAN", then "PAN 2"); the same
  value however written reuses its entry. The text stays masked; the ack carries `sealed: [{kind, name}]`; the
  journal gets `identifier.sealed` (names) or `identifier.seal_failed` (kind only); the planner gets one more fact
  line (`intake::sealed_note`). No prompt or catalog change.
- GATE-602: `secrets.unmask {name}` / `secrets.mask {name}` (app_only, `IDENTIFIER_ADDITIONS` in the method table)
  mark an any-site identifier `shown` in the handle file (the value stays in the Keychain); `secrets.saved` rows of
  kind identifier carry `shown`; a delete or a new value under the name ends the grant. The redactor holds the shown
  values' SHA-256 (letters and digits, upper-cased) in memory, loaded at start off the start-up path and after each
  change, and leaves only a `pii.rs` span visible when it matches; a span any other rule masks stays masked. Mask
  and delete clear the redactor's set first, so a Keychain failure leaves everything masked.
- FERB.app: the ack's `sealed` is kept on the turn; the notice says the ID was kept sealed and fills on any site
  (GATE-501's "didn't save it" stays for everything else) with a "Let the AI see “PAN”" link; Settings → Data →
  Saved secrets shows "ID, any site" rows with Let the AI see / Hide from the AI. Playground answers both calls.

**Why.** The owner's answers (GATE-601): "a. always, b. filled on any site, c. yes leave them unmaskwed, d. yes lets
add that".

**Follow-up: the sink rule for a saved ID (GATE-605).** "Filled on any site" answered being asked to type the ID
again, not where the owner's data may go. A site's own saved entry is filled only at its exact place
(`OwnerSecrets::reveal`), which is why its sealed fill never went through the sink (the form sink reads
`typed_in_tab`, which never holds a sealed value); the any-site fallback had lost that protection.
- `Gateway::identifier_sink` (in `floor_needs`): a `browser.secret` whose `secret` names an ID the page's place would
  fill from the any-site record (`OwnerSecrets::fills_from_any_site`, names file only) asks the form sink's question
  (`sink:new_destination`, the form's key for the host: once per site per task, its Yes carries to the submit) when
  outside content was read and the site is neither approved nor named (GATE-537). The card names the ID by its saved
  name ("This fills your saved ID “PAN” into a page on …"), never the value: one `saved` branch in `wording.rs`.
- At the fill, `ask_secret` re-checks the exact frame's site (`identifier_cleared`): a frame of another site, or a
  site not cleared, is refused and nothing is filled.
- Apps: the sink has no app form, so `entry_for` gives an `app:` place only its own entries; a value typed for an app
  stays the app's; `secrets.list` names identifiers only for a web place (before: for every place but "any site").
- The hidden input's save card for a web page says "… usable only at this destination (or any site, if it's an ID
  number)" and "for x.test only (or any site, if it's an ID number)": a named difference from Python's card in
  `scenarios.rs` (`any_site_clause`); an app's card is unchanged.
- Message entries (ENG-8): `MessageBody.sealed` ({kind, name}), absent when empty, so stored and golden bodies read
  and write the same bytes; the projector takes it from the ack.
- `chat.send` is the `cli` role's only (ferb-rpc `auth.rs:84`, not app_only): `cli` is proven by the socket peer's uid;
  a model's tools come in as `mcp_shim` (tools.call only); the shell's sandbox denies the daemon's socket by path and
  every outbound unix socket (ferb-shell `sandbox.rs:512-517`, `:535`; wired at ferbd `compose/tools/shell.rs:154`);
  Claude runs with `--tools ""` (ferb-runtimes `claude/transport.rs:53`); Codex runs read-only without network and
  its item gate stops a run on any item not FERB's (`codex/stream.rs:793-811`, `:1385-1401`). New test:
  `only_the_owners_connection_may_send_a_chat_message` (ferb-rpc).

Evidence (follow-up): ferb-gateway `outside_destinations.rs` 5 new (asks once per site naming “PAN”, value absent from
the card, Yes carries to the submit; a No fills nothing and isn't asked again; a named site fills unasked; a site's own
password still fills unasked; an embedded frame of another site refused), `scenarios.rs` with the named card
difference, ferb-identity (app gets no fallback, listing for an app has no identifiers, the any-site flag),
ferb-supervisor `chat_entries.rs` (sealed stored, absent when none), ferb-rpc (above). Runs in the shared tree: core,
identity, policy, gateway, supervisor, sanitize, rpc 368 passed, 0 failed; ferb-daemon and ferbd 191 passed, 0 failed,
5 ignored (chat_replay's recorded scenarios unchanged); clippy on the touched crates and `cargo fmt --check` clean;
`./gate.sh portable` clean.

**Evidence.** New tests: ferb-sanitize `tests/identifiers.rs` (5: one value shown and another of its kind masked,
written either way; a mark from before a change not trusted; card, password label and grant; `identifier_kind` by
shape only; the message's identifiers; contact data and technical ids visible), ferb-identity `tests/identifiers.rs`
(3), ferb-gateway `scenarios.rs` (a remembered PAN kept for any site, a password and a card-field value for their
site, the PAN filled on another site with no question), ferb-daemon `b_small.rs` (the RPCs through the real server),
ferb-rpc golden (table = Python's + stage 2 + these 2), ferb-supervisor `planner.rs` (the note), Swift
`RedactedHistoryTests` and `AppScreensTests`, ferbd `chat_replay.rs` (through the real program: an identifier its test
Keychain can't keep is only masked, the ack has no `sealed`, the journal says `identifier.seal_failed {"kind": "PAN"}`
and holds no value). Runs: sanitize, identity, gateway, rpc, daemon 281 passed, 0 failed; supervisor 94 passed;
ferbd `chat_replay` 10 passed (the 61 recorded scenarios unchanged), run in a worktree of HEAD plus only this change;
Swift focused 24, 0 failures; clippy (workspace) and portable clean. Python-recorded fixtures: no change in bytes
(every new field is absent when empty).

**Chosen differently from the brief.** The grant lives on the saved entry (Keychain value, `shown` flag in the
handle file), not a keyed hash in the database: no migration and no key to keep, and a hash of an Aadhaar or PAN is
guessable from its small space unless keyed. Consequence: only a saved identifier can be shown, which the chat path
guarantees (every identifier typed in chat is saved first). In the hidden input, label-only kinds (passport, bank
account) keep their site: a page sets the field label, so it can't decide what fills everywhere.

**Unverified.** Not live-checked (a real Keychain save from chat, the app's link and Settings buttons). ferbd's
other test binaries were not run on this change (the disk was full; in the shared tree another agent's unfinished
change kept ferbd from compiling). No test drives `chat.send` through a store that succeeds (the program's test
Keychain always refuses); the success path is covered piecewise (identity, supervisor note, Swift). The start-up load
of shown values is untested. The seam's `block_in_place` assumes the daemon's multi-thread runtime (a current-thread
one calls the helper inline, holding that thread for the helper's run). The stored message entry now carries
`sealed`, but FERB.app's and the phones' folds from entries don't read it yet, so a turn folded from entries shows
the old notice. GATE-605: the gateway tests use a stand-in `browser.secret` (the real driver's frame origin is not
exercised); `mac.secret`'s app rule is tested in ferb-identity only.

**Questions for the owner.**
- (Answered by GATE-605: a new site that came from outside content asks once per task; the card says an ID number
  is kept for any site.)
- An ID the owner pastes that is someone else's (from a document) is kept too, as "PAN 2": fine, or keep only ones
  the owner typed?
- A shown ID is shown everywhere FERB reads it (pages, files), and the model can then type it like any text.
- Notice and Settings wording ("kept it sealed as “PAN”, so I can fill it in on any site", "Let the AI see") is
  for the owner's eyes.

## 2026-11-15 · 78 · Image redaction after hours of running: Vision's compiled models were purged; retry on the CPU, then a fresh child process, and every failure recorded

**What.** HAND-531. `ImageRedactor` (ferb-mac-hands `ocr.rs`) now takes a list of attempts and tries them in order:
Vision as it chooses its devices (`native::vision::NativeVision`), Vision with every compute stage on the CPU
(`CpuVision`: the device Vision lists as `MLCPUComputeDevice` for each stage), then FERB's own program in a
short-lived child (`vision_child::ChildVision`: `ferb read-image-text`, image on stdin, JSON regions or the child's
fault on stdout, nothing on disk, stderr discarded, killed after 150 s, one child at a time; a live preview's frame
fails at once with `ferb.busy` instead of queueing). No attempt reads it: withheld, with the same text
(`UNAVAILABLE`). Each failure is a `Fault` (step, NSError domain and code, the underlying NSError's domain and code;
FERB's own `ferb.*` domains for decode, start, timeout, exit, reply, busy, rules, paint), never a message, the image
or text read. ferbd's `MacImages` (tools/mac.rs) journals `image.redaction_failed` (withheld) and
`image.redaction_recovered` (a later attempt read it, `read_by`) with `use` (tool, preview, front_window, recording,
warm) and the faults, bounded like the previews' notes (first at once, then one per 600 s per kind/use/faults with
`repeats`). `preview.failed` gains `faults` (task_preview's sanitizer error is now `Unmasked {error, facts}`; the
shown text and `error` are unchanged), passed from the sanitizer through a task-local set in `compose/previews.rs`.
`MacImages::warm` reads a blank picture with the in-process attempts and journals it, for the coordinator to call
when the Mac comes back into use.

**Why / cause.** Proven from the owner's Mac: (1) the unified log shows, for the installed daemon (pid 35788), every
recognition from 16:50:57 to 16:52:10 followed within 10 ms by `[com.apple.TextRecognition] E5RT Error:
e5rt_execution_stream_operation_create_precompiled_compute_operation_with_options call failed (Code: 13)`; no other
process on the Mac logged that error between 02:00 and 17:30. (2) The daemon's Vision model cache,
`~/Library/Caches/com.ferb.daemon/com.apple.e5rt.e5bundlecache/26B123/*`, was written 02:14:37-02:15:16 (the start's
warm-up) and every file in it was removed at 03:41:08 (the bundle folders' change time; they hold 0 bytes now),
while the Mac was locked (the redactions that worked were 02:19-02:46; unlock at 04:35). (3) Other programs' e5rt
caches on this Mac were emptied within seconds to minutes of being written today (duetexpertd 20:08:03 → 20:08:38;
my test process 20:21:09 → 20:21:48), and CacheDelete reported 940 MB free on the Data volume at urgency 3 at
20:21:40 (`df`: 96% used). Inferred, not proven: macOS purged those compiled-model files for disk space (who deleted
them at 03:41 isn't in the log), and the long-running process keeps using its record of the compiled model, so each
request fails creating its compute operation from the missing files, for the rest of the process's life; a fresh
process compiles again (30-65 s here) and works. Sleep or lock is not the cause by itself: `pmset -g log` shows no
sleep today. Python did the same in-process call per image (images.py:81-106: new request, no device choice, no
retry, no subprocess), so the Python daemon would fail the same way.

**Evidence.** ferb-mac-hands `tests/image_fallback.rs` (10 + 1 by hand): CPU retry reads and paints, child reads when
both fail, all fail → withheld with three faults and the same text, read-but-unpainted → withheld with no text in any
fault, first success runs alone, the child's reply round trip and refusals, the child side's own retry, a real child
over pipes (`/bin/sh`), exit / garbage / missing program, a preview doesn't queue behind a running child while other
images wait; by hand: a hung child killed at its limit. ferbd: `compose::tools::mac` journaling test (bounded, repeats,
recovered `read_by`, no content), `compose::previews` test (preview.failed carries `faults`, `error` unchanged).
Real Vision, by hand on this Mac: `tests/native.rs a_child_process_reads_an_image` passed (the child, the
`read_image_text` example, answered in 65.2 s cold; `CpuVision` in the test process returned `Ok([])`; 178.6 s in
all). A second manual pair of child runs: the first failed in 6 s (`__objc2.missingError` 0, with
`e5rt_e5_compiler_compile failed (Code: 11)` in the log), the next read the image in 59.5 s. Runs: ferb-mac-hands +
ferb-daemon 111 passed, 0 failed; ferbd 100 passed, 1 failed (`seams_wired`: `browser.rs:1065 ..Default::default()`,
another agent's open change, not this one); clippy on the three crates clean; fmt clean for them.

**Unverified.** Live: whether the CPU attempt in the long-running daemon recovers after a purge (its compiled model
is purged too, then it fails fast like the first) and how long the child takes per image when caches keep being
purged (here 6-65 s, compiling each time); not wired yet: the child's command in `ferb` (`run::cli`), the span record
of tool calls, the warm-up on unlock (listed for the coordinator). The by-hand test writes Vision's own cache under
~/Library/Caches (read_image_text, native-<hash>), as any Vision run does.

**Open for the owner.** The Mac is nearly out of disk (CacheDelete: 940 MB free); freeing space is what stops macOS
from purging the compiled models. The child limit (150 s) and the new journal kinds' retention (kept, not in
`OPERATIONAL_KINDS`) are the agent's choices awaiting the owner's word.

## 2026-11-15 · 79 · Background first in browser tasks: what a page brings forward goes back down and is told; sign-in popups that start blank are the task's; browser input is sampled

**What.** The owner's live report on task 0821c105 (HAND-532, his words kept): an IIT Madras dashboard check ran in
FERB's minimized Chrome window, and at its Google sign-in "the chrome window popped up" and took his focus. Extension
0.19.11 (protocol 21, minimum 0.19.0 kept, HAND-537) and the engine:
- HAND-533: FERB's minimized window, or a popup window FERB claimed for a task tab, found shown within 3 s of a page
  in it opening a tab or showing a dialog is minimized again at once (background.js `keepDown`, `putDown`,
  `surfacingTab`, `windows.onFocusChanged`); one found shown with no such cause (Dock, Window menu, `show_tab`) is the
  owner's and is left until it is minimized again. The owner's own windows only become facts. The page hook refuses
  `window.focus`/`self.focus`, fullscreen (`requestFullscreen`, webkit forms) and picture-in-picture while FERB drives
  the tab (protocol.js `ferbPageHook`, content.js `onSurface`). Each event is a `surfaced` fact on the responsible
  tab's next reply; the bridge takes it out of any reply (`Bridge::keep_surfaced`, known fields only, 10 per tab) and
  the tab's next result tells the worker once (`window_shown`, `surfaced.rs::told`, via `tools.rs::news`). FERB never
  activates another app (HAND-212 stands).
- HAND-534: a page's `window.open(url, "_blank", "noopener")` in a driven tab returns null and goes the way a link's
  new tab goes (the daemon opens a background tab); content.js opens it natively when FERB doesn't.
- HAND-535: browser.act, navigate, media, secret, login_code, fill_secret and fill_otp declare on_screen, so the
  disturbance sentinel samples around them (DMN-5) and a front-app change is the call's `screen_changed_during_call`
  fact. `BrowserTool::facts` (ferbd) now takes on_screen from the verb (before, every fact came from the frozen
  catalog, so a verb's own on_screen was never read).
- HAND-536: a new tab whose first address is about:blank in a popup-type window, within 3 s of a task tab's input,
  is checked by the browser's openerId and claimed as the task's popup, and its window goes down at once.

**Why / cause.** Known from Chromium's source (main, read 2026-11-15): every new tab or window a page opens is followed
by its window's `Show()` (`browser_navigator.cc` ScopedBrowserShower; `BrowserWebContentsDelegate::AddNewContents`
passes kShowWindow and `AddWebContents` sets user_gesture true); `BrowserView::Show` on a window that isn't visible (a
minimized one) shows it activated (`NativeWidgetMac::Show(kNormal)` → `kShowAndActivateWindow` →
`[NSApp activateIgnoringOtherApps:YES]` and `makeKeyAndOrderFront:`); on a visible one it calls `Activate()`; a popup's
contents are focused too. Known from the site: app.onlinedegree.iitm.ac.in/auth/login loads FirebaseUI 3.5.2 with
`signInFlow: 'popup'` (Firebase 7.14.2). Inferred, not proven: FirebaseUI opened Google's sign-in as a popup window
(named target and a size) whose first address was blank, so HAND-443 (http(s) first addresses only) never claimed
it; Chrome showed and activated it; it completed with the already signed-in account and closed itself, and the opener
went on to the dashboard (the url wait was met at 19:52:25; no `popup` event, "nothing changed within 0.5 s" at the
opener). Whether macOS granted Chrome the activation (cooperative activation since macOS 14) or only ordered the
window front isn't known: the sampler sampled no browser call (on_screen_calls 2, browser.open only) and the change it
recorded at 19:53:08 was the owner going to VS Code. The probe page (below) confirms it by hand.

**Evidence.** ferb-browser `tests/extension_background.rs` (8 tests: the extension's own JavaScript under node with
fake Chrome APIs and a fake clock, `tests/extension/harness.js`; with the popup's minimize and the owner's-window
rule taken out, the two tests on them failed), `tests/background_first.rs` (2: a fake extension's reply carrying `surfaced` through the hub, the bridge
and browser.navigate to `window_shown`, told once; the facts' shapes and words), `tests/tools_catalog.rs` (the catalog
comparison names HAND-535's difference); ferbd `compose::tools::browser` unit test (on_screen from the verb, every
other fact the catalog's). ferb-browser 59 passed, 0 failed; ferbd 126 passed, 0 failed (chat_replay 266 s); clippy
-D warnings clean on both; `gate.sh fmt` clean. The workspace's lint and test weren't run here (other agents' work
in progress; the disk was nearly full). Python's extension
tests (node, fake Chrome): 98 passed after one assertion in tests/test_downloads_extension.py changed for HAND-534.

**Changed against Python (deliberate).** HAND-535's on_screen for seven browser verbs (Python's catalog has false);
the extension's new behaviour is the extension's own (0.19.11), shared by both daemons.

**Unverified.** Everything live: no real browser ran. Whether the minimize after a popup or tab comes soon enough to
spare the owner the flash; whether Chrome's activation is granted on this macOS; that FirebaseUI's sign-in still
completes with its popup minimized (it ran in a background tab before); that a renderer-opened tab really un-minimizes
FERB's window (source says yes). Not covered: Chrome's own UI that no extension reaches (permission and sign-in
bubbles, the external-protocol prompt and the app it launches, passkey/WebAuthn sheets, FedCM's dialog), a page's
`opener.focus()` across origins, and the keyboard focus Chrome took (FERB doesn't give it back, HAND-212).

**Open for the owner.** Publish extension 0.19.11 to the Chrome Web Store (until then nothing of the extension's half
runs; HAND-535's sampling and facts work with any extension). Then LIVE-947 with the probe page.

## 2026-11-15 · 80 · The Python daemon leaves the repo

**What.** ENG-11 and ENG-16, prepared in journal 76; at `9d3f84bc` (P).
- Removed (674 files): `ferb/**/*.py` (314), `tests/` (350), `pyproject.toml`, `uv.lock`, `.python-version`,
  `scripts/build-app.sh`, `codex_standin.py`, `question_run.py`, `question_compare.py`, the two `python_client.rs`
  tests, `ui/mac/FERBAgent/main.c` (the C host embedding Python; ferbd's `host.rs` replaced it). Two ignored Rust tests
  that ran the Python through uv (`a_rust_seal_opens_in_python`, `python_and_rust_open_each_others_store`). The
  Python build's parts of the scripts: `ferb_size_report`, `ferb_trim_app`, `FERB_UV_ARCH`, the interpreter's
  signing and entitlements (`com.ferb.python`).
- Kept in `ferb/` (97 files, what the engine and FERB.app read): the role, supervisor, memory, decide and abilities
  Markdown, the memory dashboard files, the 59 migrations, `identity/secret_helper.swift`. All
  `rust/fixtures/**/make.py` stay as history (ENG-17); `spikes/` stays as AGENTS.md says (five of its experiments
  import `ferb` and no longer run).
- kb: `historical` names P and every removed root (PROJ-509); the gate's three Python checks and the note about the
  hand-run suite are retired; T-1111 is done and deleted; T-1113 lists the leftovers below. AGENTS.md, README.md,
  HANDOFF.md and AGENT-BRIEF.md follow the Rust rewrite (owner rules 1–18 unchanged); the store listing and the relay
  README name the Rust homes.

**Evidence.** `kb check: ok (15 areas, 1234 decisions, 99 tasks, 479 live checks, 44 terms)`, no path problem (1,602
decision paths and 24 area paths read at P). Rust: fmt, clippy and the iOS/Android check clean; 984 passed, 0
failed, 15 ignored. `scripts/tests`: 65 run, 6 skipped (need a built bundle). The bundle's inputs: Resources/ferb
assembled by `ferb_rust_runtime_markdown` plus the extension, migrations, helper source and packaging inputs, 129
files, the same SHA-256 at P and after; `git diff 9d3f84bc` is empty for those paths and for every engine source,
manifest and Cargo.lock (only test files changed under `rust/crates`). No full app build was run.

**Open.** T-1113: ferb-device's installer still carries the Python build's paths (`Programs::Python`, the venv, uv)
and `setup.rs:903` says a checkout runs `uv run ferb`; THIRD_PARTY_NOTICES.md names the Python files its adapted code
came from (it ships in FERB.app, so it was left byte for byte); `ui/mac/FERBNotch/PLAYGROUND.md` and
`com.ferb.daemon.plist`'s comment name build-app.sh (left: the Swift gate and the bundle). Comments in the
migrations, `protocol.js` and the Swift sources name Python files as where a rule came from (bytes that ship or are
checksummed: unchanged).

## 2026-11-15 · 81 · A pairing page nobody answered is opened again inside the wait

**What.** The owner's live report (a fresh install's Pair all over seven profiles at once, 22:47): six profiles
paired within 6 s; Chrome "Profile 2" got its wake page (22:47:35.943) and its pairing page (22:47:38.774), nothing
answered, and `browsers.pair_failed {"why": "the pairing page wasn't answered"}` came at 22:48:39 with no
`browser.connection_refused`; Try again paired it in 0.2 s (pairing page 22:48:43.454, paired 22:48:43.663). HAND-538
(agent): while the one-time token is still unused and nothing says the extension is off, gone or refused, the page
is opened again for the same token at 10 s and at 25 s of the wait (`PAIR_REOPEN_AT_S`), exactly as the first
(`open_pairing` unchanged: the wake page first when the profile isn't connected, its load time, then the pairing
page), only inside the wait, so at most two re-opens. `pair_one` (Pair all, a single pairing, a task's browser_pair)
and `identity_pair` share it (`Cx::open_pairing_held`, `Cx::reopen_if_due`, `PairOpens` in
`ferb-daemon/src/areas_b/browsers/mod.rs`). Each round reads the clock once, so the wait's end and a re-open are judged
at the same moment. The timeout, its failure text, the `hint` and the journal rows are Python's; no row was added.
Every wake page is held until the wait ends and closes itself as before (HAND-526). After a re-open, once the
profile is connected, its extension is asked to close each pairing page by its exact address (`wake_close`): a page
that never ran pair.js keeps its address with the token and would stay; one that ran dropped the token from its
address and closed itself, so it matches nothing. A pairing that opened one page asks nothing more (Python's calls).

**Why / cause.** Proven: the first page went unanswered for 60 s and a second open of the same flow paired at once;
nothing was refused. Not proven which of these lost the first page; read in the extension (not changed):
`chrome.runtime.onMessage` takes `pairToken` and calls `send("pair")`; `send` queues every message in `outbox` while
the native port is down (`background.js:266-270`), and the outbox is flushed only on a `welcome`
(`background.js:454`), which an unpaired install never gets: the daemon answers its hello with `pairing_required`
(`ferb-browser/src/bridge.rs:1544`) and the extension sets `gen = null`. So a token handed over in a moment the port
is down (the native host not up yet, or retrying after a disconnect: 1, 2, 4… s, the 30 s alarm) waits in the outbox
for good, while the page itself closes (inferred from Chrome's documented behaviour: the `sendMessage` callback runs once the
listener returns without replying). The next
page, with the port up, posts `pair` at once. The other way is a page Chrome opened before the profile's extension was
loaded (seven launches in the same instant), which never runs pair.js. Either way a second page is what worked, so
the daemon reopens; the extension's outbox is worth fixing in the extension itself (posting a `pair` once the port is
up, as a `hello` is).

Pair all's simultaneous start was left as it is (one `pair_one` per profile in a JoinSet): six of seven paired within
6 s that way, the failure isn't shown to be the crowding, and staggering would add the 2 s load wait per profile
(about 14 s for seven) to every Pair all.

**Evidence.** `tests/b_browsers.rs::an_unanswered_pairing_page_is_opened_again_inside_the_wait` (HAND-538; the fake
hub moves the injected clock 10 s per lookup, so a wait's rounds are known steps): the second page answered pairs
after one re-open, two pairing pages with one token, each after its wake page; the first answered opens one page and
asks only Python's wake_close; never answered in 60 s opens three pages (no fourth) and ends with Python's
`no_answer` reply and its one `pair_failed` row; a 20 s wait opens two, an 8 s wait one; FERB turned off in that
profile as the first page opens ends at once with one page and Python's row. Once paired, every tab FERB opened is
gone and the owner's tab is left. With the re-open disabled the test fails (2 launches where 4 were expected). The
fixture test `with_full_disk_access_every_call_answers_writes_and_launches_as_python` keeps Python's commands byte for
byte: every recorded wait (timeout_s 0, or 5 answered at once on a still clock) ends before the first re-open (named
there). ferb-daemon: 68 passed, 0 failed (b_browsers 7 passed, five runs); clippy -p ferb-daemon --all-targets
-D warnings clean; my files rustfmt-clean. The workspace's `gate.sh fmt` and `lint` fail on other agents' files in
progress (ferb-store, ferb-supervisor, ferb-gateway's `type_complexity`, `b_window_shown.rs`), not on these.
`kb check`: only other agents' pending citations (HAND-546, HAND-547, STORE-423, STORE-424).

**Fixed against Python.** HAND-538: Python opened one pairing page, and a page missed waited out the whole timeout;
the re-opens, and closing a never-run pairing page after a re-open, are the deliberate difference.

**Unverified.** Everything live: no real browser ran. Which of the two ways lost the first page on the owner's Mac;
that Chrome keeps a never-loaded chrome-extension:// page's address with its token (so `wake_close` finds it);
`identity.pair`'s re-open has no test of its own (same helper). A page answered late together with a re-opened one
could deliver the used token a second time: the bridge then notes `unknown_token` for that install until its next
hello (a transient `problem` and a `browser.connection_refused` row), inferred from `bridge.rs:1498-1506`, not seen.
Pairing pages that never ran stay open when the wait times out (nobody can close them: the profile isn't connected),
as before.

## 2026-11-15 · 82 · The owner is told when a browser window came up; image-redaction rows pruned; an image's faults on its call's span

**What.**
- HAND-545 (owner: "i recommend notify too"), HAND-546. When a task's page brought a browser window up (extension
  0.19.11's `surfaced` facts where a window came up, not a refused request) the browser part publishes
  `browser.window_shown` {task_id: the tree, tab, site, shown} (`Driver::window_shown`, from `tools.rs::news`). The
  gateway publishes the same topic when a sampled browser call ends with its own browser process at the front
  (`came_forward`: the screen fact's front app pid equals the new `Tool::background_pid`, which ferbd's BrowserTool
  answers from the tab's bridge) and the result carried no `window_shown`. ferbd's consumer hands it to
  `Notifier::tell_window_shown`: the line is written from the facts by `notify::window_shown_line` (no model, no
  prompt) and posted through `tell_owner` as notice `window-shown-<tree>-<site>` (once per task and site; refs
  `task:<tree>`): a quiet FERB message in today's chat (APP-622's notices: no focus, no full chat; one notification
  sound; a banner only when no FERB client shows notices), kept in `notices.list` and the chat's entries. The task's
  record gets an `owner.notice` span (notice id, text, site, facts; `desktop::fact_span`). No Swift or extension change.
  The lines: "A pop-up window opened for a moment on <site>; I minimized it." (with "and took your keyboard focus"
  before the semicolon when it did; "; I couldn't minimize it." when it stayed); "My browser window came up for a
  moment when <site> opened a new tab / showed a dialog; I minimized it."; "Your browser window came to the front when
  <site> …; I left it as it was."; "Google Chrome came to the front while I was working on <site>." Without a site:
  "a page", or no place. Never "sign-in": a pop-up is named by its window type only.
- HAND-547 (owner: "ok agreed"): `image.redaction_failed` and `image.redaction_recovered` join OPERATIONAL_KINDS
  (53 kinds, sorted) and go with `preview.failed`; the child reader's 150 s limit stays.
- HAND-548: `ImageSanitizer::sanitize_noted` (defaulted) returns the span's notes with the answer; MacImages fills
  them (`span_notes`: faults, then `read_by` or `withheld`); `redact_image` returns them as a third part and the
  gateway records them on the tool.call span's output as `image_redaction`, as `timing` is (HAND-436). What the model
  receives is unchanged.
- Bus: `browser.window_shown` is a telemetry topic and in waits.rs NOT_WAITED; ferbd's stand-ins gain a `publish`
  control for the wiring test.

**Evidence.** ferb-gateway `tests/span_notes.rs` (2: faults on the span and nowhere in the model's reply, withheld
and recovered; the call's own browser at the front publishes once, another app or an explained one doesn't);
ferb-daemon `tests/b_window_shown.rs` (2: every line from its facts, refused/unknown say nothing, no "sign"; once per
task and site, published `attention.notice` without banner, `chat.notice` journaled, refs `task:<tree>`); ferb-browser
`background_first.rs` (the published event: tree, tab, site, only the window that came up, once); ferbd
`tests/wiring.rs::a_window_that_came_up_is_told_once_and_kept_in_the_tasks_record` (through the real program: one
notice in `notices.list` for two publishes, an `owner.notice` span, one fallback banner recorded) and
`compose::tools::mac` span_notes unit test; ferb-store `tests/retention_kinds.rs` and `tables.rs` (Python's kinds plus
exactly the two). Runs: ferb-store, ferb-core, ferb-orchestrator, ferb-gateway, ferb-browser 265 passed, 0 failed, 1
ignored; clippy -D warnings clean on those five; `gate.sh fmt` and `portable` clean; the new ferb-daemon and ferbd
ferb-daemon + ferbd 194 passed, 0 failed, 5 ignored and clippy -D warnings clean on all seven touched crates, run in a
worktree at HEAD with only these files (another agent's open ComputerAction change in ferb-core/ferb-mac-hands kept
the shared tree from compiling at the time). `seams-off.txt` names `Tool::background_pid`'s default for MacTool.

**Changed against Python (deliberate).** The bus's telemetry list gains `browser.window_shown` and OPERATIONAL_KINDS
the two image kinds; both named in their fixture tests.

**Unverified / open.** Live: LIVE-955 (the line in the notch with the real extension, its timing against the
popup's flash, and that the notification sound is wanted for it). The front-app case needs the bridge's
`browser_pid` (the relay's parent); an unknown pid tells nothing. A front-app change the owner made themselves into
the task's own browser during a call would read as the browser coming up (structure can't tell). The new test files
are untracked: kb sees their `decision:` tags once they are added.

**Follow-up (2026-11-16, HAND-560, after journal 85's HAND-549).** The Mac's `came_forward` fact reaches the same
notice. `window_shown_line`'s front-app line (`notify.rs::front_app_line`) now reads the fact's fields: the app, what
FERB did there (`action`, added to the Mac fact by `tools.rs::input_action`: click, type for text/keys/values,
scroll, drag, else use) or the site, and `put_back` / `previous` / a plain `why`. The lines: "WhatsApp came to the
front when I clicked in it; I put Notes back." / "… when I typed in it …" / "… when I scrolled in it; I left it in
front." (put_back false, no why) / "WhatsApp came to the front while I was working in it; I couldn't put Notes back:
another app is in front now." / "Google Chrome came to the front while I was working on app.example.edu; I put Brave
Browser back." (a browser call; without `put_back` it ends after the site as before). A line with no site is once per
task and app (`window-shown-<tree>-app:<app>`). The gateway merges the sampler's `screen_changed_during_call` into a
tool's own instead of replacing it (`merge_told`; the tool's fields win), so the Mac tool's put_back, why,
raw_input_asks and next survive. A browser call whose own browser is in front after it now puts the app that was in
front back (`Tool::put_back`, ferbd's BrowserTool through `MacTools::put_back`, made public: the same activation and
the same check that the browser is still in front, so an owner who switched meanwhile is left alone); the result is
in the published fact and the worker's. Reading of HAND-549's second yes as covering browsers (HAND-532 said FERB
doesn't switch back) is the coordinator's, to be confirmed with the owner. The topic keeps its name
(`browser.window_shown`): a rename touches the bus fixture test, waits, the browser, the gateway, ferbd and five tests
for no behaviour. Evidence: ferb-daemon `b_window_shown.rs` (+2: the Mac and browser strings; once per task and
app), ferb-gateway `span_notes.rs` (+2: the merged fact; put back, and the owner-switched answer told as it is),
ferb-mac-hands `element_actions.rs` (+2: the put-back the browser uses, skipped after a switch; the action words; the
HAND-549 test's fact gains `action`). Unverified: live; the ferbd conversion from the sampler's {pid, bundle_id, name}
to the Mac's RunningApp has no test of its own; whether the owner caused the browser to come forward can't be told
from the sampler's facts (only "someone switched since" is).

## 2026-11-15 · 83 · Settings → Stored data: every table viewable, a phrase found, a turn deleted so the planner no longer reads it

**What.** The owner's request (STORE-422, his words kept): "where is this exact word written??? i wanna see the db
too … add a storage/db viewer in setting … i wann see all teh data", and "delete in db by user if they want".
- Viewer (STORE-423): `ferb-store/src/data_view.rs` reads on a read-only connection of its own (`open_read_only`:
  `SQLITE_OPEN_READ_ONLY`, `query_only`): `tables` (every `sqlite_master` table, its rows, columns, a group for a
  person, `internal` for full-text indexes, their shadow tables and SQLite's own), `rows` (newest first by rowid,
  pages of up to 200, each text cut in SQL at up to 2 000 characters with its full length in `cut`, a blob as its
  size, the page stopped at 500 000 characters, an optional `equals`/`starts_with` filter on a real column), `row`
  (one row, each cell up to 200 000 characters and the row up to 600 000), `cell` (the rest in pieces of up to
  200 000), `search` (LIKE with `\` escaping, ASCII case ignored, also the phrase as Python's `json.dumps` stores it
  so accents and quotes inside FERB's JSON are found; 20 rows per table; the important groups first; 8 s, then
  `stopped`). Table names only from `sqlite_master`, columns only the table's own, values as parameters.
  `ferb-supervisor/src/flow/data_rpc.rs` serves them as `data.tables|rows|row|cell|search` on blocking threads, plus
  `data.context` (`Inner::conversation_now`: the `_conversation_blocks` the planner's next turn reads) and
  `data.delete`; dispatched from `chat_rpc.rs`, app_only on the cli role (`auth.rs` `DATA_ADDITIONS`, the golden).
- Delete (STORE-424): `flow/data_delete.rs`. `plan` reads what a turn (a request id; a task resolves to its turn)
  holds; `apply` runs inside `ChatLog::rewrite` (new: the log's write transaction after everything queued; the
  tombstoned entries go to the watchers and their conversations' projectors are dropped): the journal's conversation
  rows naming the request or its tasks (`data_view::CONTENT_KINDS`, tested equal to `CONVERSATION_KINDS`,
  `SELF_POSTS`, `chat.plan` and `chat.summary`) and every running summary of that chat from the first that folded one
  in (a later summary carries the earlier one's text); the journal's delete trigger empties chat_fts (secure-delete)
  and the recall vectors; `forgotten_refs` gets `journal:<id>`; chat entries become tombstones (`chat_entries::
  tombstone`: body `{}`, new seq); chat_history's turn (`$.requestID`) and its replies (`$.turnID`); tasks,
  task_results, cards, names, task_context (a later task's `prior_task` set NULL), plans and steps; spans of the tasks
  and of the planner calls (`input.request_id`) with their children (recursive). Then, in its own transaction, a
  later `model.*` call whose input quotes one of the turn's texts (≥ 24 characters with a space, once or twice
  JSON-escaped) gets the attention forget's `CONTEXT_REMOVED` as its context. Refused while the turn's stream is
  open or a tree of it runs. `data.deleted` is journaled with the request id, task ids and counts only. Memory:
  the existing `memory.forget` (`confirm: true`).
- Fresh conversation (STORE-425): `chat.attach {new: true}` already made a new current conversation; FERB.app now
  asks for it (`NotchModel.startFreshConversation`, keeps the new id for `chat.send`).
- FERB.app: Settings → Stored data (`SettingsTab.stored`, in Manage FERB): a search field, "Start a fresh
  conversation…", the groups with their tables (internal ones behind a switch), under What the AI was given "Next
  turn: what FERB would read now" and "Model calls"; a table's rows newest first with Load more; a row in full
  (`StoredData.readable`: JSON indented, long texts such as `context` shown as written, selectable, Show more); Delete
  this turn… / Forget this memory… with an alert listing what goes and what stays. A deleted turn leaves the chat the
  app holds (`ConversationArchive.removeTurns`). Models in `NotchCore/StoredData.swift`; the playground answers the
  calls from a small invented database (`PlaygroundStoredData`).

**Why.** The planner's conversation is read from the journal (`turn.rs` `conversation_view` → `entries_of
(CONVERSATION_KINDS + SELF_POSTS)` and `chat.summary`), so deleting a turn must delete those rows; `0001` keeps the
journal append-only for updates but leaves delete for "forget = purge", and `0054`'s trigger takes the index copies.
Rule 6: no generic row delete; the journal's other rows, the action ledger, confirmations, standing permissions, known
destinations and recovery copies are shown, not deletable here (Uninstall's delete everything covers them).

**Evidence.** ferb-store `tests/data_view.rs` (5: tables and groups equal `sqlite_master`, paging and cursor,
a 250 000-character cell cut and read back whole in pieces, search incl. `café said "no"` stored as `é`/`\"`,
`%` literal, budget 0 stops, bad names/columns refused, the connection can't write). ferb-supervisor
`tests/data_delete.rs` (2, through the real RPC server after the recorded `plain_reply` turn ran through the real
supervisor): a non-app connection refused all seven calls (`-32005`); tables/rows/row/search through the wire; the
planner's context held "hi there" and the answer before; after `chat.attach new` the new conversation's context had
neither; preview changed nothing; after the delete the context of `s1` had neither, journal rows and chat_fts gone,
2 tombstones with `{}` bodies and last_seq +2, chat_history empty, the turn's spans gone, the later quote blanked,
`data.deleted` without the words, a second delete "nothing stored". ferb-rpc auth unit test and the golden (+7).
Runs: ferb-store, ferb-rpc, ferb-supervisor 189 passed, 0 failed; clippy -D warnings clean on the three (run with
`-A clippy::type_complexity` because another agent's open ferb-gateway change trips it); `gate.sh portable` clean;
Swift `StoredDataTests` 8 passed; whole `swift test` 786 executed, 0 unexpected failures.

**Unverified / open.** Live: LIVE-956. ferbd was not compiled (another agent's open ferb-mac-hands change doesn't
build); `data.*` reach ferbd through `chat_rpc::register`, which it already calls. Not covered by a test: the
"still running" refusals, a summary chain, task tables' rows (the recorded turn had no task). Copies a delete
leaves: the journal's audit rows naming the turn (e.g. `chat.ack`, `task.*`), screenshots until they age out,
memories and learned cards built from it (counted in the preview; Memory forgets them; learning reads the
`journal:` tombstones), a later call's paraphrase of it, the WAL until its checkpoint, FERB.app's unsent-history
outbox, a phone's copy until it reads the tombstone. EntryWatch keeps the old conversation id until FERB.app
reconnects after a fresh conversation. The new files are untracked: kb sees their tags and paths once added.

**Addition: the owner's three answers (STORE-426, "yes yes yes to all.").**
- What FERB learned from a deleted turn goes with it (STORE-428). The link used is what FERB records: a memory item
  whose every `memory_sources` row is one of the turn's journal rows or a `trail` row of its tasks; a learned card
  whose every `learning_sources` ref is `journal:<row>` of the turn or `episode:<task>` of its tasks. After the
  turn's transaction, the new `Daemon::forget_learned` seam (ferbd → `areas_a::memory::forget_turn_learning`) runs
  Memory's own forget for each item (`forget_memory`, sources kept: the turn's are gone) and `forget_nodes` for the
  cards (with what was derived from them), then `settle_forgets`. One with other sources is kept; the preview lists
  both with their words (`will_delete.memories`/`learned`, `keeps_memories`/`keeps_learned`). Not linkable: a memory
  FERB recorded from another turn that says the same thing, or one with no recorded source; those stay.
- The tasks' `episodes` rows go (their how-notes by the foreign key), and their evidence: each row's file under
  `<home>/evidence` (home = the database's folder), only plain-name relative paths whose folder resolves inside it,
  never followed (a link there is removed itself, never its target); a file that can't go keeps its row
  (`screenshots_kept`). `data.deleted` carries the counts, never paths.
- A fresh conversation (STORE-427): `chat.attach {new}` writes a notice entry "New conversation: FERB doesn't read the
  messages above." (ref `conversation:<id>`, origin ferb) into the new stream through `ChatLog::rewrite`, not
  journaled; FERB.app adds the same line to its chat (`addNotice`, saved) and restarts its entry watch on the new id.
- Tests: ferb-supervisor `tests/data_delete.rs` now 4 (a turn with a two-task tree: tasks, results, context, spans
  with a child, task.answer, episode, three summaries of which the 2nd and 3rd go, four evidence rows: a file and a
  link removed, the link's target outside kept, a `../` path refused and kept, a data-only row removed; the memory
  port asked for exactly the only-from-it item and card, the shared ones kept and named; the audit row and trail
  kept; the fact without names. The marker entry. `busy` refusals and a finished turn not open). ferb-daemon
  `tests/a_turn_learning.rs` (the real forget path: item and its memory_fts copy gone, card and its derived card gone
  with tombstones, the others kept). Swift: the preview's words, the marker in the chat, the watch following the new
  conversation.
- Runs: ferb-store, ferb-rpc, ferb-supervisor, ferb-daemon and ferbd, 69 test binaries (chat_replay included): 390
  passed, 0 failed, 5 ignored; clippy -D warnings clean on the five (with `-A clippy::manual_contains -A
  clippy::type_complexity` for another agent's open ferb-gateway change); Swift 790 executed, 0 unexpected failures.
  The entry's "Unverified" items on screenshots, memories, the entry watch, the refusals, summary chains and task
  rows are covered by the above; still live-only: LIVE-956.

## 2026-11-15 · 84 · A stopped task's summary said once more, not three times: a task's end goes to the turn that ran it

**What.** APP-919: a task's end is filed on the turn of the session that ran it (`Conversation.swift`
`recordAnswer`), else on the turn its request names, else on the task's NEWEST turn (`DaemonConnection.swift`
`receiveOutcome`), then on any other turn of it still waiting; an earlier turn the task ran under before a later
message restarted it keeps its answer. `ChatProjector` mirrors both (`handle_outcome`, `receive_outcome`). ENG-18:
`scenarios.json` gains its first invented timeline, `stop_beside_new_work_two_turns`, after the 71 recorded ones;
the order rule for plans (rule 4, `support_entries`) takes a task's first plan, since a resumed task has one per turn.

**Why.** Owner, live 2026-11-15 23:24: "i just got triple replies for the latest assignment task, why??? FIX". Task
3a0cc55b ran under two turns (B85892C6, then 0BEBF605 after "start 3 parallel agents"). "stop all the tasks. list
where all the files … are" (b5dfa157) stopped it beside new work (ORCH-638): `control.rs` `then_start` journaled
`chat.reply` (told_task; no frame) and the new task's plan frame carried the stop's words first (`tree.rs`
`said_first`: plan text and first receipt, as Python's recording `stop_plus_new_task` does). The stopped task's
`outcome` came on 0BEBF605's stream with `told_in` b5dfa157. Copies, as read from the owner's chat_history and
chat_entries (read-only) and the code:
1. 95361593's first bubble (receipt + plan line; `plan` stores the same words, not drawn while a `planLine` exists,
   `TurnCardView.swift:18`): ORCH-638's design.
2. 0BEBF605's answer: APP-825's design (the told end filed on its own turn).
3. A later answer of B85892C6 at the bottom (reply 3E1DA219; entry …320 `follows` …307): the bug.
   `receiveOutcome` → `handleOutcome` → `recordAnswer`, whose `turns.first { id == receipt.id || backendTaskID ==
   task }` (`Conversation.swift:131`, since 2026-10-13) matched B85892C6 first; it already had its answer ("Stopped,
   as you asked. Sites contacted: 1 site", from the restart), so `finish` made the summary a later answer (APP-914,
   61b2908a). The projector copied the lookup (`entries.rs` `handle_outcome`'s `first`), hence …320 and …321.
Before APP-914 the same lookup overwrote B85892C6's answer in place (a copy up the chat, no new bubble): the
lookup predates the Rust build; APP-914 made it a bubble at the bottom. Nothing in the engine's frames or their
order differs from Python's.

**Evidence.** ferb-supervisor `chat_entries`: `a_stopped_task_two_turns_ran_tells_its_end_once_on_the_turn_that_ran_it`
(both orders of the new plan and the stopped end) and `a_result_with_no_request_ends_the_newest_turn_of_its_task`
fail on the old `entries.rs` (2 answers with the summary, as the owner's store; the end on r1) and pass on the new;
`a_history_saved_with_the_extra_answer_…` reads an old store back whole. Golden: the 71 recorded scenarios
byte-identical (the diff is the header line, the last recorded scenario's closing comma and the new scenario).
EntryFoldTests: 72 of 72 transcripts equal; `testAStoppedTaskTwoTurnsRanTellsItsEndOnceOnTheTurnThatRanIt`,
`testAResultWithNoRequestEndsTheNewestTurnOfItsTask`, `testAHistorySavedWithTheExtraCopyIsReadBackWhole`.
On the old app code (a worktree with only this diff, the two Swift lines put back) the first two fail (the summary
said again by a reply; the end on the first turn) and the scenario's fold differs; on the new, EntryFold, EntryWatch,
StopToldOnce and ArchiveUpgrade tests: 28, 0 failures. Runs: worktree `gate.sh fmt` and `lint` clean, ferb-supervisor
98 passed, 0 failed; ferbd `chat_replay` 10 passed, 2 ignored (`engine_*.json` unchanged); Swift on the shared
checkout 790 tests, the only failures EntryWatch's scenario count (fixed since) and SpaceRuleTests on another agent's
new StoredDataPage.swift alerts.

**For the owner.** By ORCH-638 and APP-825 the stopped task's summary still shows twice when a stop comes with new
work: on the stopped task's own turn and as the first lines of the new message's bubble. In this case the two turns
are next to each other. Whether one of them should be shorter (e.g. the stopped turn saying only that it stopped)
is the owner's call.

**Open.** Not live-checked. The engine's replay through the real program (`chat_replay`, `engine_*.json`) has no
such scenario (its timelines are Python recordings). A saved history keeps its old third copy (installs are wiped).

## 2026-11-15 · 85 · An element's own AX actions in mac.act (context menus in the background); an app that raises itself is put back and asks next time

**What.**
- HAND-551: `mac.act` gains `action: "menu"` (AXShowMenu) and `action: "perform"` with a new optional `name` (any
  other action the element advertises now, matched by its listed name; `perform` naming press/cancel/menu is that
  action, `driver::normalized`). A snapshot lists every advertised action (`ax::action_shown`): Python's table by
  FERB's word, an app's own NSAccessibilityCustomAction by the name in its `Name:…\nTarget:…` string, any other `AX…`
  action in snake case; white space and control characters folded, cut at 48 characters, 16 per element, once each.
  An action the element doesn't list, or a name two actions share, is refused with a plain fact
  (`act::advertised`). `perform` goes through every check a press does (files, cmd+Q/cmd+H items, run controls,
  paste and terminal items; terminal_input asks as for a press). ferb-core's `ComputerActionKind` gains `Menu` and
  `Perform` and `ComputerAction` an optional `name` (absent from the JSON when None, so Python's types fixture holds).
- HAND-552: `menu` waits for the owner to be quiet (`RealInput::quiet`, as foreground input does; nothing held
  back), performs AXShowMenu, finds the menu (`ax::context_menu`: the element's AXMenu child, the app's one open
  menu, or the menu holding the app's focused element; 8 looks 0.2 s apart), walks it as a new snapshot generation
  (`MacDriver::listed`, the snapshot's listing taken out of `snapshot`) and returns its elements in `facts.elements`;
  the result's `next` is `tools::MENU_NEXT`. AXShowMenu that the app didn't answer (kAXErrorCannotComplete) counts as
  done when a menu is found open. An AXMenu inside a window is a `menu` container in every snapshot.
- HAND-553: a pointer step refused because the target is in front while the owner is active now says, when the
  app wasn't in front at the call's first look, "<app> came to the front by itself when it got FERB's <kind> (it
  wasn't in front before), and the owner is using the Mac, so the rest of the pointer input stopped"
  (`driver::came_forward_stop`); the owner's text stands otherwise; when it asks is unchanged.
- HAND-549 (owner: "yes and yes"; supersedes HAND-212), HAND-550 (what HAND-212 kept), HAND-554 (how): every Mac input
  call but AppleScript and owner-allowed foreground input reads the front app before and after (`MacTools::run`,
  `after_input`). If the target came forward, the app in front before goes back in front (`apps.activate`,
  attributed to FERB through `on_activate`) unless another app is in front by then or it quit; the result carries
  `screen_changed_during_call` {front_app, by, put_back, why, raw_input_asks, next} (a PointerInterrupted gets the
  same sentence appended); the driver's `came_forward` seam publishes `browser.window_shown` {task_id, site "",
  shown [{cause front_app, app, put_back, previous, input}]} for HAND-546's notice (ferbd `tools/mac.rs`). Raw
  input (`tools::raw_input`: mac.click by pointer, mac.pointer but hover, mac.act by process events) records
  `raw_input_background` in the app's profile: failed when it came forward, landed when it stayed behind; the last
  one decides. `confirm::foreground_need` asks the existing foreground question (same key and card, once per app per
  task) before raw input to an app whose last record is failed (`Apps::comes_forward_on_input`, defaulted false;
  ferbd's `MacApps` reads the driver's records through `Profiles::shared`, one instance per folder).
- Catalog: `rust/fixtures/catalog/tool_changes.json` (new, by hand) holds the deliberate changes to Python's tools;
  ferb-gateway's `catalog::Change` applies them at load; `apply_changes`/`revert_changes`/`changes_shift` for the
  tests (gateway catalog and validation, ferbd catalog_mcp, ferb-supervisor `prompt_changes::tool_shift` for the
  token reserve). The three HAND-551 changes: the action enum plus `name` (schema), one description sentence, and
  pydantic's literal_error text. The description sentence: "The actions an element lists in the snapshot are done
  in the background: menu opens its context menu (the result lists the menu's items with ids to press; cancel
  closes it), and perform with name does any other listed action exactly as listed, an app's own ones included. A
  context menu is opened this way, not by a right-click at a point." `tools::D_ACT` follows.

**Why.** Live 2026-11-15: FERB worked WhatsApp through Accessibility for two minutes, then needed a document bubble's
menu; the snapshot listed `menu` but mac.act couldn't do it, the worker right-clicked, WhatsApp came to the front,
the owner lost focus, and the refusal blamed the owner. The owner then answered "yes and yes" to: tell at once and
ask before more clicks to that app (remembered per app), and put the previous app back right away.

**Evidence.** ferb-mac-hands `tests/element_actions.rs` (15 + 1 ignored by-hand test, on fakes over the real
`ax::Ax` walk: the live case's bubble lists [press, scroll_to_visible, cancel, menu]; menu opens it, the items come back
with ids, the next snapshot shows `open: [menu]`, Delete pressed by AX; cancel closes it; the owner typing makes menu
wait and yield; Download performed by its Name: string; an unlisted name, an unadvertised menu and a perform without
name refused with nothing performed; the context menu's Quit (cmd+Q) item guarded for press and perform; FERB's notch
refused; describe kind and ledger identity equal a press's plus action/name; a self-raising right-click: the stop's
text, Notes activated again, one `shown` fact, profile failed; an app the owner uses keeps the old text, no put back;
an app that stays behind changes nothing and clears the record; an AX menu that raised the app is put back without a
raw-input record; owner-allowed foreground typing is never put back), `tests/driver.rs` (description with the
change applied), ferb-policy `tests/raw_input.rs` (raw input to a recorded app asks with the foreground key; menu,
perform, press, hover, AX click and other apps don't), ferb-gateway `tests/catalog.rs` (listings byte for byte with
the changes taken out; the three changes, once each) and `validation.rs` (627 cases, mac.act's with the change).
Runs: ferb-core, ferb-policy, ferb-gateway, ferb-mac-hands 228 passed, 0 failed, 7 ignored; ferbd catalog_mcp,
chat_replay (the token reserve moved by the change's bytes), wiring 17 passed, 2 ignored; ferb-supervisor and
ferb-daemon in the first run passed (290 passed with ferbd, its 2 replay failures being the reserve, since fixed).
clippy -D warnings clean on ferb-core, ferb-policy, ferb-gateway, ferb-mac-hands, ferbd, ferb-supervisor;
`gate.sh fmt` and `portable` clean. The tag lines citing HAND-212 in ferb-gateway call_hooks.rs/hooks.rs and
ferb-daemon b_disturbance.rs now cite HAND-550.

**Changed against Python (deliberate).** A snapshot lists actions Python's table dropped (an app's own, other AX
ones); mac.act's schema, description and literal_error text (tool_changes.json, named in the catalog tests); an
AXMenu inside a window is a menu container (Python left its items uncontained); HAND-212's let-it-be is replaced
(owner).

**Unverified / open.** Nothing ran on a real app. The owner's checks: in WhatsApp (or TextEdit) ask FERB to open a
message's or document's context menu and pick an item: the app should stay behind and the items come back in one
call; a right-click into an app that raises itself should put the owner's app back, give one notch line, and the
next raw click in that app should ask. Whether AXShowMenu on a background app takes key focus or activates the app
is not known from code or probes here (the menu tracking's keyboard capture is my reading of AppKit; the by-hand
ignored test `element_actions::by_hand_textedit_context_menu` prints the front app before and after). Where macOS
puts a context menu in AX (element child, app child or only via focus) is covered by three lookups, not observed. The
notice line is HAND-546's (`window_shown_line`): for these facts it says "<app> came to the front while I was working
in it." and is posted once per task tree (site ""), not per app; the owner's wording ("… when I clicked in it; I put
<previous> back.") needs `window_shown_line` to read `put_back`/`previous`, the other agent's open file. The gateway
writes the sampler's `screen_changed_during_call` over the tool's own when the sampler saw a change (only when the
put-back failed, or a key-window change remains). The owner's own switch to the raising app during FERB's
sub-second call reads as the app's (the input tap doesn't say which app input went to). `kb lock` also locked
HAND-545, HAND-547 and STORE-422, which were unlocked. HAND-532's "(HAND-212 stands)" now conflicts in part with
HAND-549 for browser tasks: the browser path is unchanged here.

## 2026-11-16 · 86 · Five bugs from the live assignment task: a memory note's sentence, the checker's own question, pagers, a local PDF's profile, a pause read as a failure

**What.** From a read-only comparison of tree 3a0cc55b (2026-11-15 22:52-23:25):
- MEM-431 (`ferb-learning` text.rs `sentence_end`, `clause_span`): the owner's words saved for a fact also end where a
  sentence ends (a period followed by space or the end, not closing an abbreviation by shape: single letters with
  periods, or one to four letters before a capital or a number). Refines MEM-420 ("never a period"), recorded as a
  new agent decision because existing decisions aren't edited: whether MEM-420 should be marked superseded is the
  coordinator's call.
- ORCH-650 (`ferb-supervisor` store.rs `step_answers`, attempt.rs `answered_in_step`, worker_context.rs
  `step_answers_block`): the verifier's context gets a block of FERB's questions to the owner in this step
  (plan.question/plan.answer, loop.*, paired by question id) with the owner's answers; a check that asks again, in the
  same words, a question answered in this step is not asked: journal `plan.question_answered_before`, the next
  attempt gets `owner_continuation`'s line, counted as a failed attempt (the check-in bounds it). Prompt files unchanged.
- HAND-561 (`ferb-shell` manager.rs `NON_INTERACTIVE`, `command_env`): shell commands inherit PAGER, GIT_PAGER and
  GH_PAGER=cat, GIT_TERMINAL_PROMPT=0, GH_PROMPT_DISABLED=1; a command's own assignment wins. AppleScript's and the dev
  tools' environment (`clean_env`) unchanged.
- HAND-562 (`ferb-browser` driver.rs `in_use_profile`, tools.rs `pdf`, profiles.rs `NO_CHOICE`): when the resolver
  alone can't choose, browser.pdf html_path prints in the owner profile of the task's newest open tab; with none it
  says to open one with browser.open profile=. The resolver's shared refusal names the profile argument with an
  example (first available, browser/folder). No catalog change: a `profile` argument on browser.pdf was tried and
  dropped, because ferb_browser.pdf's schema is byte-identical and tool_changes.json's text swap would change both.
- RUN-465 (`ferb-worker` worker.rs `classify`, ports.rs `WorkerHost::paused`, supervisor host.rs, words.rs): a run
  whose tree was stopped or is paused ends cancelled with the stop's words or "paused by you" / "paused by FERB to ask
  you", whatever error the cut left; no failure in its span.

**Why.** Live: the saved roll number carried "THEY SHOULNT MENTION AI …" into always-on memory (memory_items seq 7);
the HCI step's checker asked the owner the same question three times (journal 98-112); `git log` sat at "Press
RETURN" and `git push` asked for a username; browser.pdf html_path failed five times with "select a profile from the
task context"; the owner's pause at 23:02 recorded step.search's run as failed "runtime finished without an answer".

**Evidence.** New tests: ferb-learning `tests/clause_sentences.rs` (2), ferb-shell `tests/live.rs`
`a_command_sees_no_pager_and_no_terminal_prompt_unless_it_sets_one` (real sandbox-exec), ferb-worker `tests/replay.rs`
`a_paused_run_ends_as_paused_never_as_a_failure`, ferb-browser `tests/cleanup_wake.rs`
`a_local_print_goes_to_the_profile_the_task_already_works_in`, ferb-supervisor `tests/control_scenarios.rs`
`the_check_after_an_answer_reads_it` and `a_question_answered_in_the_step_is_not_asked_again`. Fixture comparisons
with the differences named: ferb-learning pure_rules (no recorded case flips), ferb-worker worker_runs
(`stop_records_no_failure`), ferb-browser profiles (`without_the_argument`), and the supervisor's and ferbd's replays
through one shared file, `ferb-supervisor/tests/common/named_diffs.rs` (support_control, replay/harness.rs,
ferbd chat_replay). Runs: ferb-shell, ferb-learning, ferb-worker, ferb-browser, ferb-gateway, ferb-supervisor all
passed; ferbd chat_replay 10 passed, 2 ignored; `gate.sh fmt` and `lint` clean.

**Fixed against Python.**
- items.py `_clause_span` never cut at a period: a fact took every following sentence up to the next comma (MEM-431).
- supervisor.py `_verifier`/`_context` never gave the checker the owner's answer to its own question (ORCH-650).
- manager.py's environment set only TERM=dumb (HAND-561).
- tools.py `BrowserPdf._run` opened the print page with no profile (HAND-562); profiles' NeedsOwner text named no
  argument.
- worker.py classify recorded a paused run as failed, and checked the stop only for a ValueError (RUN-465). Recorded
  changes this makes: a stopped or paused worker span has no error; a paused run's span is cancelled "paused by …";
  a stopped check's episode step reads "checked nothing — 1 condition unknown" with "⚠ 1 check left conditions
  unknown" (`stop_while_checking`); a paused run's episode step reads "stopped: paused by …".

**Open / unverified.** Nothing ran live. The owner's existing memory item (seq 7) still holds the long text: code
doesn't rewrite it; forgetting or correcting it is the owner's. MEM-431's abbreviation shape keeps a sentence ending
in a word of four letters or fewer before a capital ("I met Raj. He …") joined, as before. ORCH-650's dedup matches
the same words only: the live checker reworded its question each time, so there it is the context block that should
stop the repeat (a model's judgment, not verified). Check-in answers ("Keep going") and runtime-switch answers are in
the block too. HAND-562 picks the newest live tab's profile when the task has tabs in several. I reformatted
`fixtures/catalog/tool_changes.json` (another agent's untracked file) once, same content, while trying and removing
a change. `cargo fmt -p ferb-supervisor` ran once over the whole crate, which can touch whitespace in files other
agents are editing.

## 2026-11-16 · 87 · The repository's folders renamed: engine/, engine/runtime/, apps/, extension/

> From this entry on, paths are the new layout: `rust/` is `engine/`, `ferb/` is `engine/runtime/`, `ui/` is
> `apps/`, `extensions/chromium` is `extension/`, `extensions/store` is `extension-store/`. Entries 1-86 keep the
> names they were written with.

**What.** PROJ-510 (owner, 2026-11-16: "yes do it i agree" to the proposed layout), one mechanical change at
`30905328`:
- `git mv`: `rust` → `engine` (crates, fixtures, packaging, gate.sh, the toolchain pin, PLAN, STATUS, JOURNAL,
  AGENT-BRIEF), `ferb` → `engine/runtime` (prompts, memory files, abilities, decide prompts, migrations, the secret
  helper's Swift source), `ui` → `apps` whole (apps/mac, apps/AGENTS.md, apps/docs, apps/output, apps/.gitignore),
  `extensions/chromium` → `extension`, `extensions/store` → `extension-store` (PROJ-512: the listing and artwork
  stay out of the folder FERB.app copies and the store zip packs). New `apps/android/README.md` and
  `apps/ios/README.md` (stages 3 and 4; the lab code is outside the repo). The empty untracked `ui/android` and
  `ui/iphone` folders were removed; `engine/target` (38 GB), `apps/mac/FERBNotch/.build` and `build` moved with
  their folders (a rename, nothing copied).
- Where the build reads: `scripts/build-app-rust.sh`, `rust-parts.sh` (pins, runtime Markdown list, model pin),
  `release-appcast.sh`, `sparkle-keys.sh`, `swift-test-gate.sh`, `secret_helper.py` (its SOURCE; the temporary
  path the slices are compiled at keeps `ferb/identity/secret_helper.swift`, so the same code still builds the same
  bytes), `rust_notices.py`, `make_logo.py`, `make_dmg_background.py`, the scripts' tests; `ferb-store/build.rs`
  (migrations from `../../runtime/store/migrations`); tests that read the prompts, migrations, extension and
  fixtures from the repository.
- Where FERB.app WRITES and what the engine looks for at run time: unchanged. The bundle keeps
  `Contents/Resources/ferb` and `Contents/Resources/extensions/chromium`; `~/.ferb/extensions/chromium` stays.
  ferbd's checkout lookup changed: `home.rs` `package_folder` finds a checkout by `engine/runtime/orchestrator/
  prompts` (`CHECKOUT_PACKAGE`, `checkout_root`), and `setup_env.rs` `sources()` reads the bundle's layout inside
  FERB.app (`Resources/ferb/...`, `Resources/extensions/chromium`) and the checkout's (`engine/runtime/...`,
  `extension/`) elsewhere: before, one `package_folder().parent()` served both because the two layouts matched.
  `setup.rs` builds a checkout's notch from `apps/mac/FERBNotch`.
- kb: `kb/config.toml` `moved` (PROJ-511): a record's path under a moved folder that names nothing tracked now is
  read at its new place, before `historical`; `kb show` of a file at its new place lists the records naming the old
  one, `kb show` of an old path says where it is now, `kb show ID` prints each moved path, the generated
  `.claude/rules` use the new places. Validated: a list of {from, to, why}, no globs, no duplicate `from`, `to`
  tracked now, no chain or cycle. Records keep their words and paths; area paths (configuration) name the new
  folders, the engine area as `engine/*`, `engine/crates`, `engine/fixtures`, `engine/packaging` so
  `engine/runtime` stays with its areas. Gate paths follow; the kb tool's test check runs both test files.
- Docs: AGENTS.md (outside the owner's rules), README, STATUS (layout), PLAN, AGENT-BRIEF, fixtures/README,
  apps/AGENTS.md, PLAYGROUND.md, the store listing, the relay README, the Developer ID plan. HANDOFF.md keeps the
  layout it was written in, with a note mapping the old names.

**Why.** The Python daemon is gone (ENG-16) and the phone apps come next (ENG-1 stages 3 and 4): folder names that
say what they hold, with the phone apps beside the Mac app.

**Left on purpose.** Owner rule 13 in AGENTS.md names `ui/` twice: rules are kept word for word, so T-1115 asks the
owner. JOURNAL entries 1-86, the Python column of STATUS, the PLAN's Python sources and every comment naming a
Python file keep the old names (history at `9d3f84bc`). Frozen fixtures and their generated Rust tables
(`generated.rs`, `tables.rs`, `unicode_*.rs`, `mime_table.rs`: "Do not edit" headers) and the goldens' "Generated by
rust/crates/..." header strings (in `chat_entries.rs` and `chat_replay.rs`, which write them) keep their bytes.
Python-recorded layouts in tests (`install::code_hash`'s sibling `extensions/chromium`, the `$REPO/ferb/...`
placeholders) are the Python install's, unchanged. `ArchiveUpgradeTests` names `ui/mac/...` at a release tag, where
it was. Migrations, THIRD_PARTY_NOTICES.md, the app's ThirdPartyNotices.txt and Sounds/CREDITS.md ship or are
checksummed: unchanged.

**Fixed against Python / named differences.** ferb-device `cli_fixtures`: the recorded checkout's notch build ran
`<repo>/ui/mac/FERBNotch/build.sh`; the test maps that one recorded folder to `apps/mac/FERBNotch` (`notch_moved`,
`// decision: PROJ-510`), everything else compared as recorded.

**Evidence.** `engine/gate.sh fmt`, `lint` (clippy, all targets) and `portable` clean; `gate.sh test`: 1,036 passed, 0
failed, 16 ignored (the first run stopped at ferb-device `cli_fixtures` until the named difference above). Swift: 790
tests, 3 skipped, 0 failures. `scripts/tests`: 65 run, 6 skipped. kb tool: `test_kb_historical.py` 7, new
`test_kb_moved.py` 8, all passed. `kb check: ok (15 areas, 1264 decisions, 101 tasks, 481 live checks, 44 terms)`
(before: 1261, 100, 481; plus PROJ-510..512 and T-1115); without `moved` the same tree has 1,273 path errors, with it
none, and 1,397 decision paths resolve through it. Every one of the 1,635 tracked files at `30905328`, at its new
place, keeps its area and its governing decisions (kb.py at `30905328` on a worktree against kb.py now), except 15
that gain only PROJ-510/511/512. Bundle inputs: the runtime Markdown as each tree's own `rust-parts.sh` lists it,
the extension, the plists, notices, icon, sounds, banner, pinned helper, both npm locks, the migrations, the
helper's source and pin and Cargo.lock, assembled at the bundle's paths from `git archive 30905328` and from this
tree: 159 files, identical SHA-256 lists. Two clean ad hoc builds (`SIGN_IDENTITY=- FERB_UPDATES=off`, arm64, the
same output folder): `30905328` in a temporary worktree, then this tree. Same 207 entries; the 171 plain files and 9
links identical, `ferb-native-host`, `ferb-secrets`, Codex, Claude Code and Sparkle identical. Different: the three
build-numbered plists and `CodeResources`; `ferbd` and `ferb` (their strings differ only in the checkout paths this
entry changes: `engine/runtime` + `orchestrator/prompts`, `apps/mac/FERBNotch`, `extension`; the bundle's
`Contents/Resources/ferb`, `ferb/identity/secret_helper.swift` and `extensions/chromium` are in both; no repository
path is embedded, panic locations are `crates/...`); `FERBNotch` in 34 bytes (its UUID and nine 16-bit immediates;
no string differs) from the source folder's path: a rebuild of this tree reproduces it, and putting back the one
comment changed in its sources does not remove it.

**Open / unverified.** Nothing installed or run live; the checkout-only paths in ferbd (a checkout's package folder, the installer's
sources, a checkout's notch build) are covered by the tests above, not run on a real checkout setup. That the nine
FERBNotch immediates that follow the source path are behaviour-neutral is inferred (same strings, same size, same
symbols), not proven. Build caches hold absolute paths: the moved `apps/mac/FERBNotch/.build` failed (`no
XCFramework found at .../ui/mac/...`) and was deleted (swift test rebuilt it); `build/rust-app-arm64/.work/swift`
and `.work/cargo` were deleted for the same reason (the app and the model, node and sign caches kept). A cargo
cache whose artifacts are newer than the sources ran a stale ferb-store build script once here (building
`30905328` then this tree into one folder): a build folder used before this change should drop its `.work/cargo`.
`engine/gate.sh lint` and `portable` now also run when only `engine/runtime/` changes (`engine/**`). T-1115:
the owner's answer on rule 13.
