# CLAUDE.md

You're building FERB, a personal agent runtime for macOS. `ARCHITECTURE.md` is the design contract; your job
is to implement it faithfully, one step at a time, so I can review each piece before the next is built on it.

## Workflow

The work is a fixed sequence: first the spikes in ARCHITECTURE.md section 17 (in order), then the build steps
in section 18 (in order). `PROGRESS.md` tracks where we are — create it if it doesn't exist, and keep it updated.

Each unit of work (one spike or one build step) goes like this:
1. Read `PROGRESS.md`, pick the next unit, and state in a few lines what you'll build and how you'll know it's done.
2. Build it, with tests passing.
3. Update `PROGRESS.md` and commit.
4. Report: what was built, anything uncertain, anything in `ARCHITECTURE.md` the work contradicts.
5. Stop and wait for me. Don't start the next unit on your own.

When I reply "continue", start the next unit from step 1. If I reply with feedback instead, apply it to the
current unit first.

Build steps 1–4 in section 18 are the foundation everything else rests on, so for those four, after step 1
show me the plan (files, key types and functions, tests) and wait for my approval before building. For every
other unit, go straight from the plan to building.

## Rules

- `ARCHITECTURE.md` is the source of truth. If something in it turns out to be wrong or impossible, stop and
  tell me with a proposed edit rather than working around it; the file gets updated before the code.
- The `vibed/` folder holds old versions of FERB built with an approach I've abandoned. Leave it out of
  everything you read, search, or reuse, so the old approach doesn't leak into this one.
- Keep the scope to the current unit. Build what the contract describes, as simply as it allows.
- Tests should prove the contract's promises (safety rules, real decision logic, boundaries between parts),
  not chase coverage. Trivial code doesn't need tests.
- Use throwaway accounts and a spare browser profile for anything touching real sites; my real accounts stay
  out until the section 21 safety tests pass. Never put secrets in code, logs, or commits.
- Ask before anything destructive or hard to undo outside the working tree.