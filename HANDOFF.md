# FERB handoff: end state on 2026-11-13, for the Rust rewrite

This is the end state of the Python/Swift codebase as the last coordinating agent left it, written for the agent that
converts this repo to mainly Rust. Read `AGENTS.md` first (the rules still bind you), then this file, then `kb/MAP.md`.


---

## 1. Where things stand

| Item | State |
|---|---|
| `main` | Pushed through `e71a92bf`. Local commits `c2a1942e` and `6a274cc8` (the Developer ID switch, below) were going through the push gate when this was written. Run `git log origin/main..main` to see what's unpushed; push with `tools/kb/run gate push`, then `git push`. |
| Installed FERB.app on the owner's Mac | Build `202611121958` (from `e71a92bf`), self-signed with "FERB Release Signing". |
| Extension | `extensions/chromium` 0.19.10, published to the Chrome Web Store by the owner. `MIN_EXTENSION_VERSION` is unchanged. |
| Owner's data | Backed up to `~/FERB-archive-2026-11-13/.ferb/` (2.2 GB: database via SQLite online backup, integrity ok, 725 tasks / 32,211 spans / 3,649 journal rows; memory, evidence, logs, workspace, config). Personal data: never commit it. The owner plans a fresh install of the new FERB. |
| Python suite | About 4,970 tests passing (`nice -n 10 uv run pytest -q`). Known flaky under load: `tests/test_downloads.py::test_a_navigation_that_becomes_a_download_returns_its_facts` and `tests/test_execution_runs.py` restart timing (T-15 lists the nondeterministic files). |
| Swift tests | 741 passing (`ui/mac/FERBNotch`, `swift test` after `source scripts/swiftpm-anonymous.sh`). |
| kb | `tools/kb/run kb check` ok: 14 areas, ~1,140 decisions, 83 tasks, ~478 live checks. |

## 2. What FERB is

A personal assistant that works across the owner's Mac in the background while they keep working. It does tasks like a
person would, perceives what happens on the Mac, and gets better at repeated tasks. Agents (Codex, Claude, API models)
interpret and decide. Code acts, exposes facts and enforces boundaries (AGENTS.md rules 3–7).

## 3. The parts, and what each becomes

| Part | Path | Size | Language now | In the rewrite |
|---|---|---|---|---|
| Daemon: orchestrator (planner, workers, checker), gateway (confirmations, sink rule, redaction), tools, sensing, memory, store | `ferb/` | ~87k lines, 314 files | Python 3.12 + PyObjC | **Rust**: the main conversion |
| Tests (safety suite, end-to-end with fake model through the real daemon/gateway) | `tests/` | ~86k lines | pytest | Port the *safety guarantees* (TEST-500..509), not line by line |
| Mac app (notch UI, setup tour, Settings, updater) | `ui/mac/FERBNotch` | ~57k lines | Swift/SwiftUI | **Stays Swift** (the owner's side; `ui/AGENTS.md`). It talks to the daemon only through RPC, so keep the RPC contract. |
| Agent stub that hosts the daemon | `ui/mac/FERBAgent` | small | Swift | Stays; it will launch the Rust daemon instead of the bundled Python |
| Browser extension | `extensions/chromium` | ~330 KB JS | JavaScript (MV3) | **Stays JS** (protocol below) |
| Native messaging host (browser ↔ daemon relay) | `ferb/native_host/main.py` | small | Python | Rust binary |
| Secrets helper | `ferb/identity/secret_helper.swift`, pinned bytes in `assets/secret-helper/` | small | Swift | Stays (REL-404 pin; see §7 for the Developer ID change) |
| Runtime prompts | `ferb/orchestrator/prompts/*.md`, `ferb/memory/*.md`, `ferb/abilities/**` | — | Markdown | **Reuse as-is**: executable input, governed by the `prompts` area decisions |
| Build, sign, notarize, DMG, appcast | `scripts/` | — | zsh + Python | Keep the flow; replace the Python-bundling parts |
| Knowledge and gate | `kb/`, `tools/kb/` | — | TOML + Python tool | Keep. It is the source of truth for *why* |
| Site | `site/` | — | static | Unchanged |
| Relay (Cloudflare worker) | `relay/` | — | JS | Unchanged |

Never read or reuse `vibed/` (abandoned versions). `spikes/` are historical experiments.

## 4. Contracts the Rust daemon must keep exactly

1. **App ↔ daemon RPC.**
   - **Where:** `ferb/daemon/main.py` and `ferb/daemon/server.py` (a Unix socket in `~/.ferb/run/ferb.sock`), plus `ferb/daemon/areas/*.py` (methods per area).
   - **Roles:** `ferb/daemon/auth.py`. Methods per role (`cli`, `mcp_shim`, `native_host`); APP_ONLY methods answer only FERB.app's own process (`ferb/daemon/app_peer.py`, DMN-23).
   - **Chat events:** the app depends on the event stream shape: `ack`, `plan`, `plan_delta`, `reply`, `step`, `outcome`, `merged`, … with `request_id` and `at` on every event (APP-578).
   - **Event order:** a `reply` event ends the turn in the app (`DaemonConnection.swift:419-431`); any later event of that turn is ignored (`:356`). ORCH-638 relies on this.
2. **Extension protocol.**
   - **Where:** `extensions/chromium/protocol.js`, `ferb/act/browser/*`.
   - **Signing:** messages are HMAC-signed with the pairing key, nonce and seq (HAND-222).
   - **Versions:** `MIN_EXTENSION_VERSION` and the protocol number change together with the manifest (AGENTS.md "Development").
3. **Model runtimes and the tool bridge.**
   - **Where:** `ferb/runtimes/` (Codex via `codex exec` / streaming, Claude via the Claude Agent SDK, Anthropic / OpenAI / Google APIs).
   - **One gateway:** tools always run through FERB's gateway. The models reach FERB's tools through the `ferb` program as an MCP bridge (`ferb/cli/mcp.py`), with scoped run tokens.
   - **Claude Agent SDK:** it is Python (`claude-agent-sdk==0.2.174`). In Rust, drive the `claude` CLI directly or keep a thin adapter (RUN-* decisions).
4. **Storage.**
   - **Where:** `~/.ferb/ferb.db` (SQLite, WAL), 62 append-only migrations in `ferb/store/migrations/` (the gate refuses edits to old ones).
   - **Layout:** memory, evidence, logs, workspace, config (`config.toml`), `service.json`.
   - **Upgrades:** STORE-403 says upgrading from any older build must keep working. The owner plans a fresh install, but keep the schema and migrations readable so the archive in §1 can be opened and a friend's install upgrades.
5. **Shell sandbox.**
   - **Where:** `ferb/act/shell/sandbox.py` builds the `sandbox-exec` profile.
   - **What it does:** it denies the window server (HAND-243), the Keychain (SecurityServer), app-bundle executables (HAND-429), visible-folder writes (HAND-203/244), and signals outside its own sandbox (HAND-217).
   - **Port:** the profile text, not the Python around it.
6. **Mac input and perception.**
   - **Where:** `ferb/act/mac/` (AX, CGEventPostToPid, SkyLight window-addressed pointer events, ScreenCaptureKit captures, Vision OCR redaction) and `ferb/sense/`.
   - **Rust:** the `objc2` family of crates covers these frameworks.
   - **Read first:** HAND-518 (light AX walks for masking) and the background-input findings in §8.
7. **Redaction (rule 7).**
   - **Where:** `ferb/sanitize/`.
   - **What it does:** secrets, OTPs and cards never reach a provider, trace, log or the database. Only FERB's own API keys are matched by value (GATE-572/573).
   - **Port the corpus tests with it.**

## 5. Source of truth and process (unchanged by the rewrite)

- **The owner's decisions are in `kb/`.**
  - **Before editing a file:** run `tools/kb/run kb show <file>`.
  - **Changing a decision:** owner decisions are verbatim and never edited; a change is a new decision that supersedes the old one. Your own rulings are `by = "agent"`.
  - **Commit trailers,** in one final paragraph: `Decisions:`, `Tasks:`, `Gate-change:` (when `kb/owner-words.lock` changes), `Co-Authored-By:`.
- **Checks run through git hooks** (`kb/gate.toml`), and a push requires `tools/kb/run gate push`.
  - **Never bypass them.**
  - **The gate is language-specific:** it runs pytest, mypy on `ferb/core` and the Swift tests. **The gate itself must be updated for Rust** (cargo test / clippy) as part of the conversion, as a recorded Gate-change.
- **Personal data:** `kb/local/personal.txt` (untracked) lists the owner's personal strings, including the legal name on the Developer ID certificate; the `personal` gate refuses them in tracked files. Never put the name, email or account details in the repo.
- **Never** `git stash`, force-push, or reset others' work; check `git diff --cached` before committing (parallel agents share the checkout).

## 6. The owner's working preferences (from many sessions; apply them)

- **"Presentation is everything."** Any owner-facing text (replies, cards, status lines, errors) is FERB speaking in one voice: plain, whole, said once, with no internal labels or audit text in chat. Before changing such text, rebuild the exact text on real cases and read it as the owner would.
- **The owner audits code.** Cite `file:line`. Say "inferred" for anything unchecked. Correct a wrong claim as soon as you find it.
- **Diagnose first, then present a solution, then build.** Discuss design choices before building; owner decisions are the owner's (AGENTS.md rules 14, 18).
- **No prompt patching for single tasks.** The owner rejected adding prompt lines for each task that went slightly wrong ("takes us to the old prompt junking"). Fix shared mechanisms instead, or leave it.
- **Don't over-engineer.** Simplest behavior; FERB says plainly what it did; the user changes it by asking.
- **Background first, the screen when needed** (2026-11-13). After a live experiment on the owner's real apps, the owner rejected app-specific background-input tricks: "lets just igve ferb scree when it neds. case not all can be in background, but we make sure most is in bg".
- **Terminal typing:** asks first with the exact command; one Yes covers that terminal app for the rest of the task (GATE-585). The 30-minute "don't ask" never covers it.
- **Safety, not at UX's cost.** False-alarm questions are "dumb questions". Payments keep their questions (two per Amazon order is accepted: "2 questiosn are better htan accidental paymetn").
- **Live acceptance is the owner's.** The owner runs real prompts and answers FERB's questions; offline tests never prove a live workflow (rule 15).

## 7. In-flight work

### Developer ID switch (T-27): built, not yet signed for real or installed

- **The owner's Apple setup is done:**
  - "Developer ID Application" certificate for team `8N7RNGH974` in the login keychain, valid to 2027-12-11;
  - `.p12` backup exported;
  - notarytool profile `ferb-notary` saved and validated.
- **Teaching guide:** `~/Desktop/FERB-Developer-ID-Guide.md`, outside the repo. **Technical plan:** `kb/plans/developer-id.md`.
- **Built:**
  - **Signing (`c2a1942e`):** `SIGN_IDENTITY=devid` (REL-421) picks the identity by SHA-1 for the team in `scripts/developer-team.txt` (REL-419, owner). It checks every Mach-O file and each piece against the team (REL-422), notarizes and staples the app and the DMG, and `release-appcast.sh` refuses unstapled images.
  - **Migration (`6a274cc8`):** the first launch of a team-signed build over a self-signed install:
    - clears FERB's own TCC entries once (APP-902, owner);
    - moves the service from the LaunchAgent to SMAppService, with the LaunchAgent as fallback (DMN-587, owner; APP-905);
    - moves the login item (APP-904);
    - shows "macOS asks for FERB's permissions once more" in the permission beats.
  - **The switch update asks first:** via `sparkle:minimumAutoupdateVersion` (APP-903, owner; REL-423). Only builds that contain this commit can ask.
  - **App-only pin:** DMN-588 (owner) superseded DMN-34 (accept the release leaf and the team during the switch, then the team only). T-9's pin is still unbuilt.
- **Not done:**
  - the first real `devid` build and notarization (commands in REL-421's commit message and in the build builder's notes: `SIGN_IDENTITY=devid NOTARY_KEYCHAIN_PROFILE=ferb-notary scripts/build-app.sh --arch arm64 build/app-arm64`, then `scripts/build-dmg.sh …`);
  - live checks LIVE-941..946;
  - committing `scripts/developer-id-first-build.txt` after the first appcast run.
- **Owner's open question:** publish one more self-signed "bridge" release carrying the ask-first code before the Developer ID release, or just tell friends to use Check for Updates… / download the new DMG?
- **For the rewrite:** the Developer ID removes the Python-bundle signing pain. A Rust daemon is a few Mach-O files, so notarization gets simpler; keep `devid` mode and the migration logic.

### Other open items

- **T-1096 (inferred):** with the service in SMAppService, `FERB_APP_BUILD` may stay at the registered build (launchd keeps the loaded definition); this affects `salvage.this_build`.
- **T-1095:** plan-level wait steps on wait.for conditions. The owner and the coordinator agreed to skip it unless a real case needs it.
- **The owner's open questions** in `tools/kb/run kb next`: T-408 preview frame-rate cap, T-1020, T-1044, T-1045, T-1050, T-1057, T-1061..1064, T-1067, T-1068, T-1093. None blocks the rewrite.
- **Live checks pending on the owner** (`kb/live.toml`): the newest are LIVE-929..946 (step checklist, waits, fs.look, extension 0.19.10 typing/paste, payments, terminal per-task Yes, paused row, stop + new task, Developer ID).

## 8. What this round learned (keep it in the rewrite)

- **The planner sees everything.** Conversation (last 20 exchanges whole plus a summary), live facts, front-window pictures *per message* (ORCH-637), earlier results by id (`earlier_results`).
- **Waiting is code, not AI.** wait.for (ORCH-631..633, ORCH-639) sleeps with no model calls. It wakes on named conditions, on the task's own commands ending, and returns at once after a failed action.
- **The front-window capture is fast only with light AX walks.** HAND-518 reads only what masking needs, and walks alongside the shot (VS Code: 0.62 s to 0.13 s per walk).
- **Background input on real apps.**
  - **Where it's written up:** session notes, summarized in §6.
  - **Measured:** the owner works in full-screen Spaces, so FERB's apps sit on another Space. Keys to a process worked in Blender, WhatsApp search, System Settings and Cursor. Window clicks into AppKit or Catalyst windows on another Space failed, and a Finder click switched the owner's Space. Blender routes keys by its last-seen pointer position.
  - **Decision:** the owner chose the foreground Yes as the fallback rather than per-app tricks.
- **Payments.** Code raises the payment question for same-site POST forms on pages showing money (GATE-582). A payment with no readable amount goes back to the worker once to state it, and the card shows it with code's check (GATE-584).
- **A stop or pause plus a new task in one planner answer does both** (ORCH-638).

## 9. Build, run, release (today's commands)

- **Dev:**
  - `uv sync --group dev`.
  - Tests: `nice -n 10 uv run pytest -q` (never two full suites at once; load causes timeouts), `uv run mypy ferb/core`, `git diff --check`.
- **App tests:** `cd ui/mac/FERBNotch && source ../../../scripts/swiftpm-anonymous.sh && swift test` (SwiftPM must never use the owner's keychain).
- **Release build (self-signed):** `source scripts/swiftpm-anonymous.sh; SIGN_IDENTITY=release scripts/build-app.sh --arch arm64 build/app-arm64`.
  - **Check it:** `CFBundleVersion`, `codesign --verify --deep --strict`, and that the secrets helper's CDHash is unchanged (`f9a54c2e2f7d191beae459b922061a0b5a4d17a6`; unchanged means no Keychain prompts).
- **Install, only with the owner's go and FERB quit:** move the old app to the Trash, `ditto build/app-arm64/FERB.app /Applications/FERB.app`, `open` it.
- **Extension store zip:** `uv run python -c "from ferb.act.browser.autoinstall import package; from pathlib import Path; package(Path('extensions/chromium'), Path('build/store'))"`.
- **Real-model test runner** (developer toolbox): `scripts/question_run.py TASKS --runtime codex --out ~/ferb-runs/x.json`. It sends real messages; FERB's questions wait for the owner.
- **Signing keys:** "FERB Release Signing" (SHA-1 in `scripts/release-cert.sha1`), the Developer ID certificate, the notary profile and the Sparkle EdDSA key are all in the owner's login keychain. Never commit, print or move private keys (AGENTS.md "Signing").
