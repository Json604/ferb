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
