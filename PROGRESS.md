# FERB progress

Sequence: spikes (ARCHITECTURE.md §17, in order), then build steps (§18, in order).

## Done

| Unit | Date | Result | Notes |
|------|------|--------|-------|
| Spike 1 — Codex exec + MCP, native tools off | 2026-09-28 | Works with a sandbox fallback | `spikes/01_codex_mcp/FINDINGS.md`. Codex can't remove `apply_patch`, and code-mode models keep `exec` + subagents, so workers are pinned to `gpt-5.5` and run under the read-only sandbox. §5, §7, §19 and test 21.1 updated to match. |

## Next

Spike 2 — Claude Agent SDK session with the `ferb mcp` shim as the only tool source.

## Open items carried forward

- Build step 4 (runtimes) must implement the tool-list allowlist check from the request trace (§7); the spike's
  `tools_in_trace` parser is the reference, including `tool_search` outputs and nameless hosted tools.
- Codex worker is a stopgap (§19): pinned to `gpt-5.5`, ~25–40k tokens of overhead per run.
