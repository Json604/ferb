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
| Policy: confirmations, protection, payments, destinations | `ferb/policy` | 4,666 | `ferb-policy` | verified on 10,769 cases (journal 26) |
| Gateway, questions, stop, resources, registry | `ferb/daemon/gateway.py` and state files | ~6,000 | `ferb-gateway` | verified on 71 scenarios, hidden input included (journal 34, 39) |
| RPC handlers (106 methods), daemon composition | `ferb/daemon/main.py`, `areas/` | ~9,700 | `ferb-daemon`, `ferbd` | 71 of 106 methods verified against the recordings (journal 40); chat, tasks and the composition not started |
| Files, recovery | `ferb/act/fs` | (in 21,723) | `ferb-fs` | verified (journal 18); Mac renderers and Trash locator not written |
| Shell, sandbox, proxy | `ferb/act/shell` | (in 21,723) | `ferb-shell` | verified, live sandbox tests included (journal 20) |
| Browser hands and extension bridge | `ferb/act/browser` | (in 21,723) | `ferb-browser` | verified: protocol, bridge, operations, 46 tools, downloads, upload copies (journal 21, 28, 36); no real browser run |
| Mac hands: AX, input, capture, recording, hearing | `ferb/act/mac` and `ferb/act/*.py` | (in 21,723) | `ferb-mac-hands` | logic verified on fakes (journal 35); native layer compiled only |
| Model runtimes: Codex, Claude, APIs, tool bridge | `ferb/runtimes`, `ferb/cli/mcp.py` | 4,876 | `ferb-runtimes` | verified against scripted programs and servers (journal 24); sign-in, keys, checks not started |
| Orchestration: answer models, plans, ledger, loops, steering, evidence, cards | `ferb/orchestrator` (not supervisor, worker), `ferb/decide` | ~7,300 | `ferb-orchestrator` | verified (journal 33) |
| Orchestration: the worker and checker | `ferb/orchestrator/worker.py` | 796 | `ferb-worker` | verified (journal 37) |
| Orchestration: the supervisor | `ferb/orchestrator/supervisor.py` | 7,681 | `ferb-supervisor` | 59 recorded scenarios replay fully (journal 46, 48, 50); about 14 of 282 methods (delegated conversations, plan statements, attention, intro preview) not ported |
| Time: timers, schedules, waits | `ferb/clock`, `orchestrator/schedules.py` | ~2,000 | `ferb-clock` | verified (journal 19); tz database to settle |
| Memory: embeddings, search, import parsers | `ferb/memory` (4 files) | ~1,770 | `ferb-memory` | verified (journal 22); needs SQLite 3.53.4+ for trigram tables |
| Memory: items, profile, learning, distill, playbooks, import job | `ferb/memory` (rest) | ~4,300 | `ferb-learning` | verified (journal 32); card reader process not ported |
| Perception | `ferb/sense` | 3,817 | `ferb-sense` | verified on fixtures (journal 30); native sources unverified on a build |
| Device: install, service, profile, scan | `ferb/device` | 2,276 | `ferb-device` | verified on recorded flows (journal 29); permissions are in `ferb-mac-hands` |
| Internal commands the app calls, toolbox | `ferb/cli` | 2,442 | `ferb-device` | verified (journal 29); no `ferb` program yet |
| Traces | `ferb/trace`, trace output of `ferb/cli/admin.py` | 759 | `ferb-trace` | verified (journal 27) |
| The daemon program | `ferb/daemon/main.py` | 748 | `ferbd` | runs with the supervisor joined: 40 recorded scenarios replay through the real binary, restarts included (journal 42, 45, 47, 49); no real model or browser run yet |
| Daemon host in FERB.app | `ui/mac/FERBAgent/main.c` | - | `ferbd` (`src/host.rs`) | built and run from inside the bundle on a temp home (journal 51); permissions unproven until installed |
| Build, sign, package without Python | `scripts/` | - | `scripts/build-app-rust.sh` | builds an ad-hoc-signed FERB.app with no Python, arm64 and x86_64 (journal 51); release signing and notarization not run |

## Later stages

| Stage | What | State |
|---|---|---|
| 2 | History owned by the engine, shared by every client | not started |
| 3 | Phone role in the daemon; Android app (from the phone lab) | not started |
| 4 | iPhone app | not started |
| 5 | Closed-app notifications | not started |
