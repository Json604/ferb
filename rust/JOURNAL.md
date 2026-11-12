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
