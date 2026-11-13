# Rust rewrite status

One row per part of the Python daemon. States: **not started**, **in progress**, **ported** (code and tests exist),
**verified** (matches Python-made fixtures, ENG-3), **live** (the owner's live check passed on an installed build).
Lines are Python lines at `63369cb3`. Update the row in the commit that changes it; the story is in `JOURNAL.md`.

## Layout

One line per crate. **Portable** crates build for iOS and Android (`rust/gate.sh portable`) and are the foundation
the phone apps share; they reach the Mac's own parts only through traits.

| Crate | What it is | Builds for |
|---|---|---|
| `ferb-py` | What Python's standard library would have produced: `json.dumps`, `datetime`, `str`/`dict` behavior, `\d` | portable |
| `ferb-core` | Shared types, ids, errors, timing, the event bus; `paths` and `config` (Unix only) | portable |
| `ferb-sanitize` | Redaction and origins | portable |
| `ferb-rpc` | Wire frames, roles and methods, run tokens (any target); the Unix-socket server and FERB.app check (Unix only), with the code-signature calls and the peer's pid and audit token behind `Security` and `set_peer_lookup` | portable |
| `ferb-identity` | The `Secret` type, owner-secrets rules, origins, seals, handle files, the helper's request/reply protocol behind `Launcher` | portable |
| `ferb-platform-macos` | Everything that touches Apple frameworks or macOS tools: Security framework, audit tokens, `codesign` and the helper process, Chrome's folder | Mac only (empty elsewhere) |
| `ferb-store` | The Mac's SQLite database: migrations and typed tables | Mac only (not in the portable set: bundled SQLite needs the Android NDK's C compiler) |
| `ferb-native-host` | The browser's native-messaging relay (program) | Mac only |
| `ferbd` | The daemon host (program) | Mac only |

## Stage 1: the Mac engine at parity (ENG-1)

| Part | Python | Lines | Rust crate | State |
|---|---|---|---|---|
| Store: connection, migrations, checksums | `ferb/store/db.py`, migrations | 240 | `ferb-store` | verified (journal 3) |
| Store: typed tables (spans, results, history, retention, clean-up, salvage) | `ferb/store` | ~1,550 | `ferb-store` (Python's JSON and times: `ferb-py`) | verified (journal 11) |
| Redaction and origins | `ferb/sanitize` (not `images.py`), `ferb/core/origin.py` | 2,118 | `ferb-sanitize` | verified and hardened (journal 4, 13) |
| RPC wire, roles, app-only check | `ferb/daemon/server.py`, `auth.py`, `app_peer.py` | ~900 | `ferb-rpc`; Security framework and audit token: `ferb-platform-macos` | verified offline; signed-app check unproven (journal 5) |
| Browser native-messaging relay | `ferb/native_host` | 73 | `ferb-native-host` | verified offline; no browser run yet (journal 6) |
| Core types, ids, errors, timing | `ferb/core` | ~850 | `ferb-core` | verified (journal 10) |
| Config, paths, bus | `ferb/config.py`, `paths.py`, `bus.py` | 490 | `ferb-core` | verified; ten config shapes refused (journal 10) |
| Secrets helper caller, owner secrets | `ferb/identity` | 826 | `ferb-identity`; `codesign` and the helper process: `ferb-platform-macos` | verified with a fake helper (journal 12) |
| What the models receive: tool schemas, answer schemas, prompt assembly, error text | (fixtures only) | - | `rust/fixtures/catalog` | frozen (journal 9) |
| Prompt context, structured answers, answer schemas | `ferb/core/context.py`, `structured.py`, `orchestrator/schemas.py`, `runtimes/schema_shape.py` | ~1,500 | `ferb-context` | verified (journal 17); outcome models go with orchestration |
| Policy: confirmations, protection, payments, destinations | `ferb/policy` | 4,666 | `ferb-policy` | in progress |
| Gateway, questions, stop, resources, registry | `ferb/daemon/gateway.py` and state files | ~6,000 | - | not started (needs policy) |
| RPC handlers (106 methods), daemon composition | `ferb/daemon/main.py`, `areas/` | ~9,700 | `ferbd` | not started; recordings of the Python daemon done (journal 23) |
| Files, recovery | `ferb/act/fs` | (in 21,723) | `ferb-fs` | verified (journal 18); Mac renderers and Trash locator not written |
| Shell, sandbox, proxy | `ferb/act/shell` | (in 21,723) | `ferb-shell` | verified, live sandbox tests included (journal 20) |
| Browser hands and extension bridge | `ferb/act/browser` | (in 21,723) | `ferb-browser` | protocol, pairing, bridge verified (journal 21); tool verbs in progress |
| Mac hands: AX, input, capture, recording, hearing | `ferb/act/mac` and `ferb/act/*.py` | (in 21,723) | - | not started |
| Model runtimes: Codex, Claude, APIs, tool bridge | `ferb/runtimes`, `ferb/cli/mcp.py` | 4,876 | `ferb-runtimes` | in progress |
| Orchestration: planner, workers, checker, plans | `ferb/orchestrator`, `ferb/decide` | 15,817 | - | not started |
| Time: timers, schedules, waits | `ferb/clock`, `orchestrator/schedules.py` | ~2,000 | `ferb-clock` | verified (journal 19); tz database to settle |
| Memory: embeddings, search, import parsers | `ferb/memory` (4 files) | ~1,770 | `ferb-memory` | verified (journal 22); needs SQLite 3.53.4+ for trigram tables |
| Memory: items, profile, learning, distill, playbooks, import job | `ferb/memory` (rest) | ~4,300 | - | not started |
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
