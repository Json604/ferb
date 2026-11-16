# Fixtures: what the Python build produced, frozen

Each folder here holds what the Python daemon (`ferb/`, removed from the repository by ENG-11) produced for
invented inputs, and the script that recorded it (`make.py` and its helpers). The Rust crates' tests replay these
files and must match them byte for byte (ENG-3). Some folders were never Python-recorded (`sanitize-shapes`,
`chat-entries/engine_*.json`, `phone`): their own scripts or tests say how they were made.

**They are frozen contracts** (ENG-17). A fixture changes only through a named difference: a Python bug fixed in the
port (ENG-6) or a deliberate change, each listed in `engine/JOURNAL.md` and named in the test that differs. A fixture is
never regenerated from the Rust code.

**The scripts are history.** They import the `ferb` Python package and its test helpers, which exist only at
commit P, the last commit before the Python code was removed: `kb/config.toml` `historical` records it, and
`git rev-parse "$(git log --diff-filter=D -1 --format=%H -- ferb/cli/__init__.py)^"` names it too. At P the folders
have their old names (`rust/` is today's `engine/`, `ferb/` is `engine/runtime/`: PROJ-510). To run one again, at P and
never in this checkout:

```sh
git worktree add /tmp/ferb-at-p P                    # P: the commit above
cd /tmp/ferb-at-p && uv sync --group dev
uv run python rust/fixtures/<part>/make.py           # writes into /tmp/ferb-at-p/rust/fixtures/<part>
# compare with this checkout's engine/fixtures/<part>; copy a file back only as a named difference (above)
git worktree remove /tmp/ferb-at-p
```

`engine/AGENT-BRIEF.md` describes how a fixture script is written; the scripts' own rules (invented data only, injected
time and ids, nothing real started) still hold for any new one.
