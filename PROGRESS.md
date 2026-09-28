# FERB progress

Sequence: spikes (ARCHITECTURE.md §17, in order), then build steps (§18, in order).

## Done

| Unit | Date | Result | Notes |
|------|------|--------|-------|
| Spike 1 — Codex exec + MCP, native tools off | 2026-09-28 | Works with a sandbox fallback | `spikes/01_codex_mcp/FINDINGS.md`. Codex can't remove `apply_patch`, and code-mode models keep `exec` + subagents, so workers are pinned to `gpt-5.5` and run under the read-only sandbox. §5, §7, §19 and test 21.1 updated to match. |
| Spike 2 — Claude Agent SDK + MCP, native tools off | 2026-09-28 | Works cleanly (options only) | `spikes/02_claude_sdk/FINDINGS.md`. `tools=[]`, `dontAsk`, explicit `setting_sources=[]` (None loads user+project settings). Proxy on `ANTHROPIC_BASE_URL` shows the API receives only the MCP tool. Auth: subscription login. ~2k tokens overhead. |
| Spike 3 — CDP pipe to `ferb-identity`; `fill_secret` | 2026-09-28 | Works; Google needs a seeded session | `spikes/03_cdp_identity/FINDINGS.md`. Pipe CDP + origin-bound `fill_secret` proven on a practice site (refusals incl. iframe, focus stealing). Google refuses automated sign-in (remote debugging sets `navigator.webdriver`); identity profile seeded from the owner's FERB-only Chrome profile and Gmail works over the pipe. §0, §8 (incl. `doctor` re-verify), §9.1, §10.1, §19, test 21.20 updated. |
| Spike 4 — Chromium extension in Brave + Chrome | 2026-09-28 | Works; background input needs focus emulation | `spikes/04_extension/FINDINGS.md`. Native messaging round trip, background tab + sanitized snapshot, trusted click/typing in a background tab (incl. Google Search, not flagged), extension-side CDP allowlist. Needs: focus emulation (site then sees `visible`; owner decision), input-readiness probe, `onStartup`, version check in `hello`, host outside TCC folders. §8/§9.2/§9.4 edits proposed. |
| Spikes 5, 7, 11 — Safari | 2026-09-28 | **Skipped by the owner** (doesn't use Safari) | Safari control parked. Spike 5 work (built, not run against Safari) is on branch `spike-5-safari` with its FINDINGS.md. ARCHITECTURE.md edits for Safari watch-only proposed. |
| Spike 6 — pyobjc probes + TCC checks | 2026-09-28 | Works; AXManualAccessibility gone | `spikes/06_pyobjc/FINDINGS.md`. Frontmost app, window titles without activating, no-prompt TCC checks (Automation via `ctypes`). Chrome 154/Brave 151 build the web AX tree on demand (first query may be empty ~0.2 s); `AXManualAccessibility` unsupported → §9.2 edit proposed. TCC grants belong to the responsible app, not FERB. |
| Spike 8 — launchd agent + socket from CLI | 2026-09-28 | Works | `spikes/08_launchd/FINDINGS.md`. Start/ping/restart-after-crash/stop/stale socket, owner-only socket with peer-uid check. The launchd process has **none** of the terminal's TCC grants (they attach to the binary launchd runs); CLI must treat EOF as daemon-unavailable. |
| Spike 9 — system audio | 2026-09-28 | **Skipped by the owner** | Deferred to the voice block (§17, §20). |
| Spike 10 — profiles | 2026-09-28 | Works | `spikes/10_profiles/FINDINGS.md`. Local State discovery + §9.5 resolver (ambiguity never first-match). `--profile-directory` hands off to the running instance (real Chrome FERB profile too); unknown folder silently creates a profile → guard. External Extensions: every profile gets it disabled (8192) until accepted, Chrome and Brave (own data dir). `Local State` written lazily. |
| Spike 12 — pairing end to end (Chrome only) | 2026-09-28 | Works | `spikes/12_pairing/FINDINGS.md`. Token via the extension's own pairing page; install_id→folder binding; signed + sequenced messages; unpaired install can't send events or receive actions (21.16). Found: native host must exit when the daemon drops; never start/kill the owner's browser implicitly (one harness-started Chrome was force-quit — fixed). |
| Spike 13 — MV3 worker restart | 2026-09-28 | Works | `spikes/13_mv3_restart/FINDINGS.md`. Worker killed after/before the effect → `unknown` → reconciled done/failed from the re-announced state; generation +1; old-gen commands refused; duplicate ids ignored even across restarts (`storage.session`). Reconnect: tab event ~0.1 s, alarm ≤30 s. Lab: spare Chrome, CDP only as test equipment. |
| Spike 14 — owner-input detection | 2026-09-28 | Works | `spikes/14_owner_input/FINDINGS.md`. Listen-only event tap + private marker on FERB's own events (idle timer unusable: FERB's input resets it). Detection ~0.6 ms median; yield ≤ one action step (20 ms steps → ≤25 ms). |
| **Build step 1** — core, store, bus, clock, daemon + socket + auth, journal, `ferb logs`, `ferb trace` | 2026-09-28 | Done | 35 tests (3 runs green), `mypy --strict` clean on `ferb/core`. Span.task_id, bus event classes, 1 MiB message cap added to the contract first. Ported from spike 8: socket perms, stale socket, second daemon refused, EOF. Found: shutdown hung on open connections (fixed: server closes them). |
| **Build step 2** — sanitize, policy (rules, intent, exposure, dataflow, autonomy, approvals, engine), ledger, `ferb stop` | 2026-09-28 | Done | 95 tests (3 runs green). Safety tests 21.3, 21.7, 21.8, 21.9, 21.10, 21.11 and partial 21.2/21.5. Contract first: intent derived only from the owner's message at plan time (10.2), internal-id redaction exemption (10.6), new critical topics (6), engine.py + config.py (3). Added: stop survives restart (10.5). |
| **Build step 3** — device (scan, profile, permissions, install, selftest), `ferb setup [--update]`, `doctor`, `uninstall` | 2026-09-28 | Done, installed on this Mac | 118 tests (3 runs green) incl. a real launchd test. Spike 15 measured a launcher: section 8 step 6 kept. Real setup: dedicated Python 3.12.13 in `~/.ferb/python`, `com.ferb.daemon` running; owner granted Accessibility + Screen Recording (Input Monitoring granted too); `ferb doctor` all green after `--update`. Fixed after the first real run: self-test read the frontmost (Brave) window title → now reads the Dock. |
| **Build step 4** — runtimes (Codex exec, Claude Agent SDK) with native tools contained, `ferb mcp` shim, gateway, `act/fs`, `ferb chat` (reply/clarify, one worker) | 2026-09-28 | Done | 147 offline tests (3 runs green) + 6 real (`pytest -m real`: 21.1 for Codex and Claude, 123 s). Supervisor + worker = Codex (`[runtimes]` in config.toml); Claude fully built and tested, a config switch away. Codex supervisor: no FERB tools, read-only sandbox, gpt-5.5, chat history re-sent each turn (`--ephemeral`; resume would need recorded sessions in `~/.codex`). Tool-list check per run: Codex from its trace, Claude via a localhost proxy recording tool names only. Pasted/quoted chat content wrapped as untrusted data (§10.2). Time header per model turn, footer per tool result. Codex pinned in `~/.ferb/node`. |
| **Build step 5** — `act/browser`: `cdp_identity` + `identity/` (Keychain, origin-bound fill, OTP), identity seeding in `ferb setup`, doctor session check | 2026-09-29 | Done except the real session check (blocked: FERB's Google session is signed out, needs a re-seed) | 176 offline tests (3 runs green) incl. real headless Chrome on local pages + a temporary keychain: pipe only (no TCP port), 21.3, 21.4 (lookalike, scheme, cross-origin iframe, focus stealing), 21.5 (password nowhere in results/db/logs after a real sign-in), 21.13 (stale refs), intent-gated submits, per-task tabs, `ferb stop` closes FERB's browser. Real chat via the installed daemon: Codex → `browser.open`/`snapshot` on example.com → done (24 s). OTPs from Gmail's Atom feed, filled straight into the field (never shown to a model). |

## Next

**Build step 6** — Chromium extension + native host + `ext_bridge` + profiles/pairing + site resolver.
**Before it:** the Brave pairing check (ask the owner for a moment first), and the step 5 items below.

## Rules for build steps (owner)

- Use the spike findings and port their tests where relevant, but write production code fresh in `ferb/`;
  never import from `spikes/`.

## Open items carried forward

- Automation (Apple Events): owner decision — ask on first use (no apps requested at setup).

- Build step 7: mark every FERB input event; GUI actions check owner input before each step (per keystroke);
  listen-only tap needs Accessibility or Input Monitoring on the daemon's interpreter.

- Build steps 1/3: daemon runs on a dedicated Python under `~/.ferb` (owner decision, §8 step 6); build step 3
  first measures a stable launcher executable (§18 step 3).

- Build step 3: `ferb doctor` checks TCC for the launchd-started daemon, not the terminal (grants belong to the
  responsible app); Automation status only checkable while the target app runs.
- Owner decision: notifications (section 10.2 "Sending… Undo") vs CLAUDE.md's CLI-only interface.

- Build step 6 (extension): record command id as seen before executing and tag created resources with it;
  `hello` re-announces owned resources; alarm (30 s) + event reconnect; native host exits when the daemon connection drops; extension queues outgoing
  messages until reconnected; read `lastError` in `onDisconnect`; register `runtime.onStartup` from the first version; `hello` carries the protocol
  version and the daemon refuses a mismatch; input-readiness probe before dispatched input; treat
  `executeScript` null/`error` results as failures; install `ferb-native-host` outside TCC-protected folders.
- Spike 12 (Chrome only; owner's rule: Brave is the owner's main browser, never touch its profile): measure
  native-host manifest location for the owner's *default* Chrome data folder.
- **Before build step 6: short Brave pairing check** in ONE temporary Brave profile, created and deleted in the
  same session (never the owner's own Brave profile). Ask the owner first so they can pick a moment when they're
  not mid-work (it opens a Brave window and needs the unpacked extension loaded into that temporary profile).


- FERB.app (later block): harden the Keychain item's ACL to FERB's signed binary once FERB.app exists (today any process
  of this user can read it via the `security` tool without a prompt).
- Codex worker is a stopgap (§19): pinned to `gpt-5.5`, ~25–40k tokens of overhead per run.
- Codex loads MCP tools lazily (`tool_search`): the model-visible FERB tools are a subset of the offered list.
  21.1 is tested as "offered == allowlist, seen ⊆ allowlist" for Codex (§21.1 wording approved and applied).
- Build step 8: task tree is in memory until then; outcomes are journaled with `verified: false` (verification
  lands with step 8).
- Upgrading Codex or the Claude SDK: rerun `pytest -m real tests/test_real_runtimes.py` (costs real model usage).
- **Step 5 open (owner):** FERB's Google session is signed out (atom feed 401; myaccount shows signed-out).
  Likely cause: a headless visit to Gmail's web UI during step 5 probing (Google showed the account chooser, then
  the session was gone). Needs a re-seed (`ferb setup --seed-identity`, Chrome quit) and the owner's choice of
  headless vs. a real (off-screen/background) window for FERB's browser. Then run
  `pytest -m real tests/test_identity_real.py` and an owner-assisted OTP check (an email with a code to FERB).
- FERB's Chrome is started in its own session: if the daemon is killed (not stopped), Chrome may outlive it; the
  next launch then fails with "another copy using its profile". Handle on daemon start (step 8 reconciliation).
