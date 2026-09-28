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

## Next

All spikes done or skipped (5, 7, 9, 11 skipped by the owner). Next: **build step 1** — `core`, `store`, `bus`,
`clock`, daemon + socket + `auth`, minimal journal, `ferb logs`, `ferb trace`. Foundation step: show the plan
(files, key types and functions, tests) and wait for the owner's approval before building.

## Pinned external tools

- **Codex CLI 0.157.1**, installed repo-locally by `npm install` (root `package.json`, exact version); FERB runs
  `node_modules/.bin/codex`, never the global one. **Upgrading Codex means rerunning the spike 1 tests first**
  (`spikes/01_codex_mcp`, command in its FINDINGS.md) and only then bumping the pin in both `package.json` and
  `CODEX_VERSION` in `codex_run.py`. The tests refuse to run on any other version.
- **claude-agent-sdk 0.2.160** (bundles the Claude Code CLI, so this pins the CLI too). Same rule: **upgrading
  means rerunning the spike 2 tests first**, then bumping `SDK_VERSION` in `sdk_run.py`. Moves into the
  project's `pyproject.toml` as an exact pin when build step 1 creates it.

## Open items carried forward

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

- Build step 5: when porting spike 3's tests, use a local test login page for `fill_secret` tests instead of an
  external site (the practice site went down mid-spike).

- Build step 4 (runtimes) must implement the tool-list allowlist check (§7). Claude: from the actual API request
  via a recording proxy (spike 2 `proxy.py`). Codex: from the trace log (spike 1 `tools_in_trace`, including
  `tool_search` outputs and nameless hosted tools).
- Build step 4: launch the Claude runtime with `CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` and `DISABLE_TELEMETRY=1`
  (both verified in CLI 0.2.160 to remove the built-in `telemetry` plugin; traffic to non-API hosts not observed
  directly, so the verification is the plugin's absence).
- Build step 5: harden the Keychain item's ACL to FERB's signed binary once FERB.app exists (today any process
  of this user can read it via the `security` tool without a prompt).
- Build step 4: set runtime auth explicitly (an inherited `ANTHROPIC_API_KEY` would override the subscription login).
- Codex worker is a stopgap (§19): pinned to `gpt-5.5`, ~25–40k tokens of overhead per run.
