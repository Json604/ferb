# Runtimes: decisions and fix list

Area: `ferb/runtimes/*`, the runtime interface in `ferb/core` (interfaces.py, types.py), runtime selection in
`ferb/daemon/main.py` (`_runtime_for`, `_model_for`, `_build_runtime` ~4538-4600, `runtimes.check/use`),
`ferb/cli/runtimes.py`, `ferb/cli/mcp.py` (the shim), `ferb/registry.py`.
Basis: a read of every line (2026-10-30), read-only DB counts. File:line refs are to commit c6621c3 (one report
cites 68dc95b for the same files).

## Owner decisions (2026-10-30)
1. **Keep every adapter and fix each** (owner: "all major options so people can choose and try anything"): Codex,
   Claude Agent SDK, and FERB's own loop over the Anthropic, OpenAI and Gemini APIs. Core output schemas become
   provider-neutral so any adapter can run any role. Afterwards each is tested on real calls one by one: Codex,
   Claude SDK (owner's subscription), Anthropic API (owner's key), Gemini API (owner's free key). The OpenAI API has
   no live test: the owner has no key, so it is checked offline only until one exists.
2. **Planner caching (P20) on after the live injection test passes**: the owner runs
   tests/security/test_planner_injection_real.py once after the rebuild; if it passes, caching is the default. It
   works with the smaller prompts (notes/orchestration.md, notes/prompts.md): the prompt shrinks, and caching reuses
   its stable part.

## What exists today (verified)

- **Usage is all Codex**: model.usage spans — codex ~9,500 (gpt-5.6-sol 6,142; gpt-5.5 2,452; older 733); anthropic
  API 5 (10-28); Claude SDK 7. OpenAI and Gemini APIs never used. Every role defaults to codex (main.py:4538-4545).
- **Codex adapter** (codex_exec.py 490, codex_stream.py 665): every production call passes a schema, so every run
  uses the app-server path (codex_exec.py:318). Containment layers: 21 disabled features, locked model catalog,
  read-only sandbox + approval never + `_isolated` check (fails loudly), trace-log tool-catalog scan (fails closed
  and loudly if the trace format changes), item gate (kills the run on an unexpected item type). A kill after an
  extra tool appears is confirmed live (10-01, 10-11, 10-13, 10-29) but happens after the request reached OpenAI;
  the gateway stays the hard boundary.
- **Planner caching is built but off**: P20 (`_codex_cache_role`, main.py:4592-4599) needs `[runtimes]
  codex_planner_cache = true`, off "until the owner passes the live injection eval"
  (tests/security/test_planner_injection_real.py); the installed config doesn't set it. Without it each planner
  call is a new app-server and a new ephemeral thread, so a new cache key: 2.0% cached input on 300 planner turns
  (workers 91.9%).
- **Dead Codex exec path** (~200 lines: `_cmd`, exec branches of start/events/cancel, `_reap`, `_watch_trace`,
  `model_instructions_file`); `workdir` ignored by the app-server path (codex_stream.py:224).
- **No explicit MCP `tool_timeout_sec`/`startup_timeout_sec`** (codex_exec.py:289-292); a long FERB tool call (an
  owner confirmation, a long shell command) can be abandoned by Codex (handled: `server.abandon`, identical retry gets
  the result). One worker had no FERB tools loaded (10-13), suspected startup timeout.
- **Supervisor timeouts doubled**: 900 s for start and another 900 s for events (codex_stream.py:247, 540).
- **Per call**: `codex --version` subprocess and a rewrite of the locked catalog file.
- **Version pin 0.157.1** in package.json, package-lock.json, CODEX_VERSION, INPUT_CHARS_SOURCE; a mismatched binary
  fails every run loudly. Checkout installs run `npm install @openai/codex@0.157.1` without the lockfile.
- **Claude SDK adapter**: `tools=[]`, dontAsk, `setting_sources=[]`, strict MCP config. Run token in argv via
  `--mcp-config` (claude_sdk.py:115-116 → SDK subprocess_cli.py:667-689), visible in `ps`; the shim already supports
  `FERB_RUN_TOKEN_FILE`. Without FERB's sign-in marker it uses the owner's own `~/.claude` login and billing (settings,
  hooks, CLAUDE.md and MCP servers are not loaded); the marker exists today. Tool check only at ResultMessage and
  skipped on error/cancel/stall (late detection, not a hole: no built-in tools exist). ToolListProxy is an HTTP
  forwarder that only records offered tool names.
- **Claude with no model fails every call** (`limits_for(None)`, claude_sdk.py:72-74) while `runtimes.check` passes
  and the catalog marks Claude always available (catalog.py:57), so it is offered to the planner as a worker type.
  The live config sets no Claude model.
- **ApiRuntime** (FERB's own loop for Anthropic/OpenAI/Gemini): tools checked every turn, run through the gateway,
  native schema enforcement, retries. SUPERVISOR_SCHEMA (~46 nullable/union parameters, OpenAI-strict style) is
  rejected by the Anthropic API ("too many parameters with union types"; 4 spans), so the Anthropic API can't be the
  planner today. A made-up tool name kills the run as a violation (api.py:219-223) instead of returning a tool error.
- **Selection**: `worker_type` is a free string (schemas.py:55, 74, 155); an unknown one fails with a message blaming
  config.toml (main.py:4560-4561). `ferb runtimes use --model` saves any string; the RPC can't set a model.
  `native_tools_contained` and `Registry.unregistered` are never enforced (display only); the whole `Capabilities`
  record only feeds `ferb runtimes` output. Runtime names are listed in five places.
- **Prices**: `claude-haiku-5-5` missing from PRICES (cost "unknown").

## Other confirmed bugs
- Codex item gate records only "mcpToolCall", not the tool (codex_stream.py:592): the 3 live kills can't be diagnosed.
- A Codex server→client request ("disallowed client action") is overwritten by a generic transport error
  (codex_stream.py:391-393, 427-428).
- If `_watch_catalog` exits on an error, nothing drains stderr under RUST_LOG=trace; Codex blocks until the stall
  watchdog fires (rare).
- Suspected: concurrent Codex runs each refresh their own copy of auth.json (token rotation race); Codex
  auto-compaction never disabled (36 planner prompts over 244.8k tokens); Codex's coding-agent base prompt stays in
  worker runs; Claude steering replay order and usage double-count across steered turns; Gemini parallel same-tool
  calls collide.

## Target design
1. One small runtime table (name, factory, limits, price, key or sign-in) replacing the five lists and the
   `Capabilities` display record; availability means "model set and signed in".
2. Codex: app-server path only; explicit MCP tool and startup timeouts; one timeout; version check and catalog
   write once at start; item gate records tool and server; stderr always drained; provider-neutral core schemas (decision 1).
3. Planner caching via a stable per-role cache key (P20), together with the prompt ordering in
   notes/orchestration.md.
4. All adapters kept and fixed (decision 1), behind the one runtime table and the same gateway path.

## Fix list
- [ ] Remove the Codex exec path, `workdir`, `model_instructions_file`, duplicate MODEL constant, duplicate `check_final`.
      (Step 3: exec path (`_cmd`, exec branches of start/events/cancel, `_reap`, `_watch_trace`, `model_instructions_file`) and the `workdir` parameter removed; every run is an app-server run. Left: duplicate MODEL constant, duplicate `check_final`.)
- [ ] Explicit `tool_timeout_sec` and `startup_timeout_sec` in the Codex MCP config.
- [ ] Remove the doubled 900 s start/events timeouts: the planner call gets 90 s (notes/orchestration.md
      decision 11); other non-worker calls keep the 180 s silence guard and one overall limit; workers none.
- [ ] Version check and locked catalog once at daemon start.
- [ ] Item gate records the tool and server; server→client request reported as a violation; stderr drained always.
- [ ] Claude run token via the 0600 token file (`FERB_RUN_TOKEN_FILE`).
- [ ] Claude available only with a model and FERB sign-in; `runtimes.check` reads limits; remove the stale marker on
      sign-out; blank `CLAUDE_CODE_OAUTH_TOKEN` in runs.
- [ ] `worker_type` as an enum of available runtimes in the planner schema; clear error otherwise.
- [ ] `ferb runtimes use` validates the model; the RPC can set one.
- [ ] ApiRuntime: unknown tool → tool error, not a violation.
- [ ] Remove `native_tools_contained`, `Registry.unregistered`, the doctor's dead "unregistered" check (or enforce).
- [ ] haiku-5-5 price entry.
- [ ] Investigate: concurrent auth.json refresh (token rotation race); disable Codex auto-compaction; give workers
      FERB's base instructions instead of Codex's coding-agent prompt.
- [ ] One runtime table.
- [ ] Provider-neutral core schemas (Anthropic union-parameter limit; SUPERVISOR_SCHEMA ~46 nullable fields).
- [ ] Fix each adapter: Claude SDK (token file, availability, model, steering replay order, usage across steered
      turns, sign-in marker), Anthropic/OpenAI/Gemini API (unknown tool → tool error; Gemini call ids; prices).
- [ ] Live test each adapter on real calls, one by one (OpenAI offline only).
- [ ] P20 planner cache: owner runs the injection eval; on if it passes.
