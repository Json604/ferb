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
