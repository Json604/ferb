# Spike 1 — Codex worker with MCP as the only tool source

Status: done (2026-09-28), 7/7 tests pass against real Codex.

Environment: codex-cli 0.157.1 (ChatGPT login), mcp Python SDK 2.x, Python 3.12.

Run the tests (real Codex runs, costs tokens):

    npm install   # once, at repo root: pinned Codex
    uv run --python 3.12 --no-project --with pytest --with pytest-xdist pytest -n 6 -q test_spike.py

## Result

Codex can be used as a worker whose only usable tools are FERB's MCP tools, **but only by combining three layers**:
disable what can be disabled, pin a non-code-mode model, and run the read-only sandbox to make the one
non-removable native tool (`apply_patch`) inert.

## Lockdown recipe (`codex_run.py`)

- `--ignore-user-config --ignore-rules --strict-config --ephemeral`; `--strict-config` rejects unknown `-c` keys, so a
  renamed lockdown key fails loudly instead of silently doing nothing.
- `--disable` shell_tool, unified_exec, shell_snapshot, multi_agent, multi_agent_v2, apps, plugins, remote_plugin,
  tool_suggest, skill_search, hooks, browser_use, browser_use_external, in_app_browser, computer_use,
  image_generation, view_image, goals, sleep_tool, code_mode_host; `-c web_search="disabled"`.
- `-s read-only`, `-c approval_policy="never"`.
- `-m gpt-5.5` (see "code mode" below).
- MCP: `mcp_servers.ferb.default_tools_approval_mode="approve"` — without it, `approval_policy="never"` fails every
  MCP call ("MCP tool call requires approval, but approval policy is never").

## What the model sees (from the request in the `RUST_LOG=trace` log)

With the recipe on `gpt-5.5`: `list_mcp_resources`, `list_mcp_resource_templates`, `read_mcp_resource`,
`tool_search` (MCP tools are deferred behind it), `request_user_input`, `apply_patch`. No shell, no JS runner,
no subagents, no web.

- `apply_patch`: comes from the model catalog (`apply_patch_tool_type`), present on every model, no flag removes it.
  Under `workspace-write` it wrote files; under `read-only` it fails ("writing is blocked by read-only sandbox").
- `request_user_input`: fails closed in exec (no user).
- `read_mcp_resource` & co: only reach configured MCP servers (only `ferb`).
- Native reads: with no shell and no JS runner there is no native tool that reads files or the network, so the
  "reads limited to workspace" and "no network" properties hold because no reader exists; tests probe both
  (canary file outside the workspace; local HTTP server must receive no request).
  Codex has `permissions.filesystem` / deny-read settings, not needed today; revisit if a reading tool appears.

## Code-mode models can't be locked down this way

Every gpt-5.6 / gpt-6 catalog entry is `tool_mode = code_mode_only` with `multi_agent_version = v2`:
- all tools (including MCP and `apply_patch`) are nested under `exec`, a JS runner in a V8 isolate;
  `--disable code_mode_host` makes code mode fail closed, which also cuts off MCP;
- `collaboration.spawn_agent` / `send_message` / … stay present with `multi_agent` and `multi_agent_v2` disabled.
Hence the pinned `gpt-5.5`. Risk: when OpenAI retires it, the allowlist test fails and the Codex worker must be
unregistered until a new recipe is found (the contract's rule, section 7).

## Other notes

- `codex debug prompt-input` shows messages only, not tools; the trace log is the authoritative tool list.
  Model-visible tools come from two places in it: each request's `"tools":[...]`, and `tool_search` results
  (logged as `output=[...]`), which load MCP tools as namespace `mcp__<server>` (so `mcp__ferb.get_ticket`).
  Hosted tools like `tool_search` have a `type` but no `name`; the parser must not drop nameless entries.
- Baseline cost: ~25–40k input tokens per exec turn, mostly Codex's own instructions.
- mcp SDK 2.x renamed `FastMCP` → `MCPServer` (`mcp.server.mcpserver`).
