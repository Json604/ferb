# Rust rewrite status

One row per part of the Python daemon. States: **not started**, **in progress**, **ported** (code and tests exist),
**verified** (matches Python-made fixtures, ENG-3), **live** (the owner's live check passed on an installed build).
Lines are Python lines at `63369cb3`. Update the row in the commit that changes it; the story is in `JOURNAL.md`.

## Stage 1: the Mac engine at parity (ENG-1)

| Part | Python | Lines | Rust crate | State |
|---|---|---|---|---|
| Store: connection, migrations, checksums | `ferb/store` | 1,794 | `ferb-store` | not started |
| Redaction and origins | `ferb/sanitize` | 2,118 | `ferb-sanitize` | not started |
| RPC wire, roles, app-only check | `ferb/daemon/server.py`, `auth.py`, `app_peer.py` | ~900 | `ferb-rpc` | not started |
| Browser native-messaging relay | `ferb/native_host` | 73 | `ferb-native-host` | not started |
| Core types, context, prompts, structured answers | `ferb/core` | 1,368 | - | not started |
| Config, paths, bus, registry | `ferb/*.py` | 704 | - | not started |
| Secrets helper caller, owner secrets | `ferb/identity` | 826 | - | not started |
| Policy: confirmations, protection, payments, destinations | `ferb/policy` | 4,666 | - | not started |
| Gateway, questions, stop, resources, RPC areas | `ferb/daemon` | 15,712 | - | not started |
| Files, recovery, shell, sandbox, proxy | `ferb/act/fs`, `ferb/act/shell` | (in 21,723) | - | not started |
| Browser hands and extension bridge | `ferb/act/browser` | (in 21,723) | - | not started |
| Mac hands: AX, input, capture, recording, hearing | `ferb/act/mac` and `ferb/act/*.py` | (in 21,723) | - | not started |
| Model runtimes: Codex, Claude, APIs, tool bridge | `ferb/runtimes`, `ferb/cli/mcp.py` | 4,876 | - | not started |
| Orchestration: planner, workers, checker, plans | `ferb/orchestrator`, `ferb/decide` | 15,817 | - | not started |
| Time: timers, schedules, waits | `ferb/clock` | 1,388 | - | not started |
| Memory, learning, embeddings, import | `ferb/memory` | 6,096 | - | not started |
| Perception | `ferb/sense` | 3,817 | - | not started |
| Device: install, permissions, service | `ferb/device` | 2,276 | - | not started |
| Internal commands the app calls, toolbox | `ferb/cli` | 2,442 | - | not started |
| Traces | `ferb/trace` | 759 | - | not started |
| Daemon host in FERB.app | `ui/mac/FERBAgent/main.c` | - | `ferbd` | not started |
| Build, sign, package without Python | `scripts/` | - | - | not started |

## Later stages

| Stage | What | State |
|---|---|---|
| 2 | History owned by the engine, shared by every client | not started |
| 3 | Phone role in the daemon; Android app (from the phone lab) | not started |
| 4 | iPhone app | not started |
| 5 | Closed-app notifications | not started |
