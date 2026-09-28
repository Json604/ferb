# Spike 2 — Claude Agent SDK with the `ferb mcp` stand-in as the only tool source

Status: done (2026-09-28), 9/9 tests pass against real sessions (twice).

Environment: `claude-agent-sdk` 0.2.160 (bundles its own Claude Code CLI; pinning the SDK pins the CLI),
Python 3.12, spike 1's MCP server. Default model reported by the CLI: `claude-opus-5-5`.

Run the tests (real sessions, costs a little — about $0.01–0.02 per session):

    uv run --python 3.12 --no-project --with claude-agent-sdk==0.2.160 --with httpx \
      --with pytest --with pytest-xdist pytest -n 8 -q test_spike.py

## Result

Clean lockdown with options alone; no sandbox fallback needed (unlike Codex).

## Lockdown recipe (`sdk_run.py`)

- `tools=[]` → `--tools ""`: removes every built-in tool (Bash, Read, Write, Edit, WebFetch, Agent/Task, Skill, …).
- `allowed_tools=["mcp__ferb__get_ticket"]` + `permission_mode="dontAsk"`: anything not pre-allowed is denied,
  never prompted.
- `setting_sources=[]` — **must be explicit**: `None` makes the SDK pass `user,project`, loading `~/.claude`
  settings, hooks and project `CLAUDE.md`.
- `strict_mcp_config=True`, `plugins=[]`, `agents=None`, `skills=None`.

## Evidence

- A recording proxy on `ANTHROPIC_BASE_URL` (records request bodies only, never headers) shows every request
  to the API carries exactly one tool: `mcp__ferb__get_ticket`. The CLI's `init` message agrees.
  This proxy is a cleaner source of truth than Codex's trace log: it sees the actual API request.
- System prompt ~3.5k chars; no mention of subagents, skills, slash commands or instruction files.
  Per-session overhead ~2k input tokens (vs ~25–40k for Codex).
- Shell, file write, subagent, read-outside-workspace and network attempts: no tool call other than the MCP one,
  no files created, canary never returned, local HTTP server never hit.
- Controls: with `Bash` added, the marker is created; with `setting_sources=["project"]`, a canary in the
  workspace's `CLAUDE.md`/`AGENTS.md` reaches the model. With `[]` it does not.

## Auth

`apiKeySource: "none"` → the bundled CLI uses the Claude subscription login (OAuth from the local Claude
login), no API key. An `ANTHROPIC_API_KEY` in the environment would take over; the runtime should set auth
explicitly rather than inherit it (decide in build step 4).

## Notes for build step 4

- `init` still *lists* built-in agents, skills, slash commands and two built-in plugins (`agents-md`,
  `telemetry`). None is reachable without the Agent/Skill tools (proxy confirms), and with `setting_sources=[]`
  the `agents-md` plugin did not inject `AGENTS.md`. The allowlist check should use the request (proxy), not `init`.
- `telemetry@builtin` means the CLI reports usage telemetry to Anthropic; not a tool, not model-visible.
- Proxy gotcha: forward decoded bytes (`iter_bytes`) when dropping `content-encoding`, or the CLI gets gzip
  it can't parse ("JSON Parse error: Unrecognized token").
