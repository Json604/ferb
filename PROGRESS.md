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

## Next

Spike 6 — pyobjc: frontmost app, window title, AX tree of Chrome with and without `AXManualAccessibility`;
TCC status checks.

## Pinned external tools

- **Codex CLI 0.157.1**, installed repo-locally by `npm install` (root `package.json`, exact version); FERB runs
  `node_modules/.bin/codex`, never the global one. **Upgrading Codex means rerunning the spike 1 tests first**
  (`spikes/01_codex_mcp`, command in its FINDINGS.md) and only then bumping the pin in both `package.json` and
  `CODEX_VERSION` in `codex_run.py`. The tests refuse to run on any other version.
- **claude-agent-sdk 0.2.160** (bundles the Claude Code CLI, so this pins the CLI too). Same rule: **upgrading
  means rerunning the spike 2 tests first**, then bumping `SDK_VERSION` in `sdk_run.py`. Moves into the
  project's `pyproject.toml` as an exact pin when build step 1 creates it.

## Open items carried forward

- Build step 6 (extension): register `runtime.onStartup` from the first version; `hello` carries the protocol
  version and the daemon refuses a mismatch; input-readiness probe before dispatched input; treat
  `executeScript` null/`error` results as failures; install `ferb-native-host` outside TCC-protected folders.
- Spike 10: measure native-host manifest locations for the owner's *default* Chrome and Brave profiles.

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
