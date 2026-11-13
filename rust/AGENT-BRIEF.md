# Brief for an agent porting one part

You port one part of the Python daemon (`ferb/`) to Rust, in one crate, checked against what the Python code
produces. Read, in order: `AGENTS.md` (all of it; "The Rust rewrite" is the method, the 18 rules bind behavior),
`rust/PLAN.md` (your part's sources, needs and traps; the traps are audit findings: check each against the code),
`rust/STATUS.md` (the crate layout), the last entries of `rust/JOURNAL.md`. House style: `rust/crates/ferb-store`
with `rust/fixtures/store/make_tables.py`, and `rust/crates/ferb-rpc` for async code with injected seams.

## The work

1. Read every Python file of your part fully. `tools/kb/run kb show <file>` gives the decisions that govern it;
   grep `tests/` for the tests that guard it. Those tests' cases are your scenario list.
2. Write `rust/fixtures/<part>/make.py` (run as `cd <repo> && uv run python rust/fixtures/<part>/make.py`). It runs
   the real Python code on invented data in a temporary FERB home (`FERB_HOME` and `HOME` in a temp folder; see
   `tests/conftest.py` and `rust/fixtures/catalog/make.py`), with clocks and ids injected, and records inputs and
   everything produced: returns, stored rows, bytes written, text composed. It must write the same bytes on every
   run. Reuse the Python tests' fakes by import.
3. Port. Same results, same stored bytes, same text for the same inputs. Python-exact helpers are in
   `rust/crates/ferb-py` (`json.dumps`, times, `str`, `repr` pieces); Python's `re` features missing from the
   `regex` crate (lookaround, backreferences, possessive quantifiers, Unicode classes) need `fancy-regex` or a
   hand-written matcher, verified by fixture (see `rust/crates/ferb-sanitize/src/rx.rs`).
4. Rust tests replay the fixtures and assert no difference. Tests use temp folders only; no real time (`sleep`)
   where a fake clock can do it; no network beyond 127.0.0.1.
5. What belongs to another part comes in through a named trait or parameter (a seam); list every seam. Anything
   that calls an Apple framework or a macOS-only tool goes in a `#[cfg(target_os = "macos")]` module (each
   `unsafe` with a SAFETY comment) or is left as a trait for `ferb-platform-macos`; the rest compiles anywhere.
6. A bug you find in the Python: fix it in Rust, name the one deliberate difference in its test, report it.
7. Cite only active decisions (`// decision: ID`): `tools/kb/run kb show ID` says when one is superseded and by
   which; the commit gate refuses a superseded citation.

## Rules

- Work only inside your crate `rust/crates/<crate>` and your fixture folder `rust/fixtures/<part>`. Never edit
  `rust/Cargo.toml`, another crate, `ferb/`, `tests/`, `extensions/`, `ui/`, `kb/`, `assets/`, `scripts/`.
- Your crate is not a workspace member yet. Its `Cargo.toml` names everything itself (version "0.28.0", edition
  "2021", license "Apache-2.0", `publish = false`; dependencies with explicit versions; workspace crates by
  `path = "../ferb-core"` and so on). Every cargo command:
  `. ~/.cargo/env && export CARGO_TARGET_DIR=<repo>/rust/target/agents/<crate> && cargo <cmd> --manifest-path <repo>/rust/crates/<crate>/Cargo.toml`.
- If your brief sends you into a crate that is already a workspace member, keep every file formatted and compiling
  at each step (`cargo fmt -p <crate>` after every edit session): the commit gate checks the whole tree, and a
  half-edited member blocks every other commit.
- Never commit, stash or reset. Never read `vibed/`. Never touch `~/.ferb`, `~/.codex`, `~/.claude`, the Keychain,
  the real secrets helper, a real browser or its folders, a model provider, or files outside temp folders.
- A fixture script runs real Python code, and that code can start real programs. Before the first run, find every
  place your part can launch something (`grep -n "Popen\|subprocess\|open -\|osascript\|launchctl"` in the modules
  you drive) and replace each. A stub on a module function does not reach a caller that bound the real one as a
  default argument at import: replace the default too (`fn.__defaults__`), or stub the caller. A recorder once
  opened the owner's real browser three times through exactly that gap. If a run starts anything real, stop, say so
  first in your report, and say what was left behind.
- Invented data only. Fixtures must pass the commit gate and GitHub's push protection: no personal strings and no
  key-shaped values in tracked files unless your part is about detecting them; keep a fixture folder under ~3 MB.
- Never print a secret value in an error, a `Debug` impl or a log.
- Finish with `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` (all with the manifest
  path above), all passing; if your brief says the crate is portable, also `cargo check --target aarch64-apple-ios`.
  Then delete your build folder (disk is short).
- Port all of your part. If you cannot finish something, say exactly what: never leave a stub that looks done.

## Final report (at most 500 words)

Modules and public API; test results (the exact output tail) with fixture case counts and how many differ; three
lists: ported and verified, ported but unverified, not started; every Python behavior not reproduced; seams; bugs
fixed against Python; and a ready-to-paste entry for `rust/JOURNAL.md` (What / Why / Evidence / Fixed against
Python / Open).
