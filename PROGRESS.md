# FERB progress

Sequence: spikes (ARCHITECTURE.md §17, in order), then build steps (§18, in order).

## Done

| Unit | Date | Result | Notes |
|------|------|--------|-------|
| Spike 1 — Codex exec + MCP, native tools off | 2026-09-28 | Works with a sandbox fallback | `spikes/01_codex_mcp/FINDINGS.md`. Codex can't remove `apply_patch`, and code-mode models keep `exec` + subagents, so workers are pinned to `gpt-5.5` and run under the read-only sandbox. §5, §7, §19 and test 21.1 updated to match. |
| Spike 2 — Claude Agent SDK + MCP, native tools off | 2026-09-28 | Works cleanly (options only) | `spikes/02_claude_sdk/FINDINGS.md`. `tools=[]`, `dontAsk`, explicit `setting_sources=[]` (None loads user+project settings). Proxy on `ANTHROPIC_BASE_URL` shows the API receives only the MCP tool. Auth: subscription login. ~2k tokens overhead. |

## In progress

Spike 3 — CDP over `--remote-debugging-pipe` to `ferb-identity`; login + `fill_secret` on a real site.
- Stage 1 done (2026-09-28): practice site + temporary keychain, 8/8 tests. `spikes/03_cdp_identity/FINDINGS.md`.
- Stage 2 waiting on the owner: FERB email account + Keychain item `-s ferb -a ferb/email/password`
  (owner creates it; the password never passes through chat).

## Pinned external tools

- **Codex CLI 0.157.1**, installed repo-locally by `npm install` (root `package.json`, exact version); FERB runs
  `node_modules/.bin/codex`, never the global one. **Upgrading Codex means rerunning the spike 1 tests first**
  (`spikes/01_codex_mcp`, command in its FINDINGS.md) and only then bumping the pin in both `package.json` and
  `CODEX_VERSION` in `codex_run.py`. The tests refuse to run on any other version.
- **claude-agent-sdk 0.2.160** (bundles the Claude Code CLI, so this pins the CLI too). Same rule: **upgrading
  means rerunning the spike 2 tests first**, then bumping `SDK_VERSION` in `sdk_run.py`. Moves into the
  project's `pyproject.toml` as an exact pin when build step 1 creates it.

## Open items carried forward

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
