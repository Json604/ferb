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

## Rulings (step 4, technical)
- Codex workers keep Codex's automatic compaction (long tool loops need it); only supervisor runs have it off.
- `worker_type` is validated per call rather than an enum, so the planner schema stays stable for the P20 cache.
- A Claude sign-out button in the app is UI-side work; `ferb setup --claude-logout` exists meanwhile.
  (Step 12b: Settings → AI → Sign out of Claude runs it.)

## Target design
1. One small runtime table (name, factory, limits, price, key or sign-in) replacing the five lists and the
   `Capabilities` display record; availability means "model set and signed in".
2. Codex: app-server path only; explicit MCP tool and startup timeouts; one timeout; version check and catalog
   write once at start; item gate records tool and server; stderr always drained; provider-neutral core schemas (decision 1).
3. Planner caching via a stable per-role cache key (P20), together with the prompt ordering in
   notes/orchestration.md.
4. All adapters kept and fixed (decision 1), behind the one runtime table and the same gateway path.

## Fix list
- [x] Owner-facing errors in app words, plain cause first (no `ferb …` commands, config.toml or raw provider errors
      like "anthropic HTTP 400: output_config…"): catalog.py, codex_stream.py, claude_sdk.py, api.py, *_api.py.
      (Step 12b: sign-in and key fixes point at Settings → AI; model-limit errors name the cause and the models FERB
      knows; a provider HTTP refusal reads "it refused the request FERB sent (HTTP 400: …)" (`api.http_cause` from
      the status code); Codex account errors, the Claude sign-in's failure text, the Jev and identity errors and setup's
      browser line no longer name commands. Tests: test_api_runtime.py, test_claude_login.py, test_codex_streaming.py,
      test_runtime_selection.py.)
- [x] Remove the Codex exec path, `workdir`, `model_instructions_file`, duplicate MODEL constant, duplicate `check_final`.
      (Step 3: exec path and `workdir` removed. Step 4: `codex_exec.MODEL` and the providers' `DEFAULT_MODEL` gone,
      defaults live only in the runtime table; codex_stream no longer runs `check_final`, every caller's one check
      in core/structured.py does.)
- [x] Explicit `tool_timeout_sec` and `startup_timeout_sec` in the Codex MCP config. (Step 4: 120 s start, 7 days per
      call, codex_exec.MCP_*_TIMEOUT_S with the reasons; FERB's stall watchdog and stop end hangs.)
- [x] Remove the doubled 900 s start/events timeouts: the planner call gets 90 s (notes/orchestration.md
      decision 11); other non-worker calls keep the 180 s silence guard and one overall limit; workers none.
      (Step 4: catalog.call_limit, 90 s / 900 s / none, one deadline across start and answer; applied by every
      adapter (Codex, Claude, direct APIs), not only Codex. The plain reply on a planner timeout is step 8.)
- [x] Version check and locked catalog once at daemon start. (Step 4: `_prepare_codex` at start; a run checks only if
      that didn't happen; the locked catalog is rewritten only when Codex's own catalog file changes.)
- [x] Item gate records the tool and server; server→client request reported as a violation; stderr drained always.
      (Step 4: violation data `item` {kind, tool, server} / {kind: serverRequest, method}; the stderr reader reads
      to EOF whatever happens, an unreadable line fails the catalog check closed.)
- [x] Claude run token via the 0600 token file (`FERB_RUN_TOKEN_FILE`). (Step 4: owner-only temp folder, removed on
      cancel.)
- [x] Claude available only with a model and FERB sign-in; `runtimes.check` reads limits; remove the stale marker on
      sign-out; blank `CLAUDE_CODE_OAUTH_TOKEN` in runs. (Step 4: no fallback to ~/.claude; `ferb setup
      --claude-logout` runs Claude Code's `auth logout` in FERB's home and removes the marker; the app has no
      button for it yet (ui/ untouched).)
- [x] `worker_type` as an enum of available runtimes in the planner schema; clear error otherwise. (Step 4: validated,
      not an enum: the planner's repair loop is told "worker_type 'x' isn't one of the worker types available now:
      …"; the dispatcher is told once and asked again; nothing blames config.toml. Worker types moved into the
      planner's fixed Layer 1. An enum would change SUPERVISOR_SCHEMA per call, which P20 keys on.)
- [x] `ferb runtimes use` validates the model; the RPC can set one. (Step 4: against the runtime's documented limits.)
- [x] ApiRuntime: unknown tool → tool error, not a violation. (Step 4: the other calls of the turn still run.)
- [x] Remove `native_tools_contained`, `Registry.unregistered`, the doctor's dead "unregistered" check (or enforce).
      (Step 4: removed with the whole `Capabilities` record.)
- [x] haiku-5-5 price entry. (Step 4: $0.10/$0.01/$0.125/$0.50, 5x for prompts over 100k tokens, pricing page
      2026-10-30.)
- [x] Investigate: concurrent auth.json refresh (token rotation race); disable Codex auto-compaction; give workers
      FERB's base instructions instead of Codex's coding-agent prompt. (Step 4, see "Step 4 findings".)
- [x] One runtime table. (Step 4: ferb/runtimes/catalog.py: name, factory, limits, billing, sign-in kind,
      availability = model set AND signed in / key present; every former list reads it, `ferb setup --key` too.
      Prices stay per model in prices.py, shared by Codex and the OpenAI API.)
- [x] Provider-neutral core schemas (Anthropic union-parameter limit; SUPERVISOR_SCHEMA ~46 nullable fields).
      (Step 4: ferb/runtimes/schema_shape.py. Core schemas unchanged. Anthropic API and Claude SDK: nullable object →
      list of at most one item, nested nullable scalar → optional property while the 24-optional budget lasts,
      the rest stay unions (≤ 16); answers mapped back before FERB's check; SUPERVISOR_SCHEMA 42 unions → 11 unions
      + 24 optional. Gemini: an enum holding null → anyOf. Offline test over every core schema.)
- [x] Fix each adapter: Claude SDK (token file, availability, model, steering replay order, usage across steered
      turns, sign-in marker), Anthropic/OpenAI/Gemini API (unknown tool → tool error; Gemini call ids; prices).
      (Step 4: all listed; Claude cost across steered turns fixed; Gemini calls without ids get FERB-local ids,
      only Gemini's own ids are sent back. Replay order: see findings.)
- [ ] Live test each adapter on real calls, one by one (OpenAI offline only).
- [ ] P20 planner cache: owner runs the injection eval; on if it passes.

## Step 4 findings (2026-10-30)
- **auth.json refresh race: not confirmed, not changed.** Live DB (read-only): 0 `auth_required` / `refresh_token_reused`
  / `invalid_grant` errors in ~9,500 Codex runs; 3 `account_unavailable (code -32603)` planner turns (10-15 02:42,
  10-16 02:42, 10-19 03:08 local), a Codex internal error on `account/read` with `refreshToken: false`, not a rotated
  token. `_save_refresh` already writes a refreshed copy back only when the source is unchanged. If it ever shows,
  the fix is one shared refresh under a lock before a run starts.
- **Auto-compaction: disabled for supervisor runs** (`model_auto_compact_token_limit` set out of reach; the key is in
  the pinned 0.157.1 binary). FERB already fits those one-turn prompts to the window, so compaction could only
  rewrite FERB's context. Workers keep Codex's default (a long tool loop compacts rather than fails): owner's call
  whether workers should fail on overflow instead.
- **Worker base prompt: replaced.** thread/start's `baseInstructions` is FERB's worker system prompt for workers too
  (was Codex's coding-agent prompt plus FERB's text as developer instructions). Needs the live check below.
- **Claude usage across steered turns: confirmed and fixed.** Claude Code 2.1.283's own field descriptions:
  `total_cost_usd` is the running total across turns of a streaming-input session ("read the latest result rather
  than summing"); `usage` is per turn (main loop only). FERB summed the running total; it now reports the
  difference per result. Tokens were already per turn.
- **Claude steering replay order: no bug found offline.** FERB waits for the replay (by UUID) of every correction
  before it accepts a result, so a stale result before or after the replay is skipped, and merged corrections need
  one result (tests/test_claude_steering.py). Claude Code also reports `queued_turn_count` on results ("> 0 means at
  least one more result follows"), but the pinned SDK 0.2.160 drops that field when it parses a result, so FERB can't
  read it. Unknown offline: whether Claude Code replays a queued message when it receives it or when its turn
  starts; if on receipt mid-turn, a result for a turn that didn't see the correction could be accepted. Live check.
- **Gemini parallel same-tool calls: no collision inside FERB's loop** (results are paired by position), but both got
  the same id (the tool name). Now FERB-local unique ids; only Gemini's own ids go back.
- **Claude SDK structured output and Anthropic's limits.** The bundled Claude Code marks tools strict when a request
  carries structured outputs (`requestCarriesStructuredOutputs` in its code), so the Anthropic reshaping is applied
  to Claude runs too. Open: whether FERB's MCP tool schemas then count toward the 24-optional/16-union limits in a
  Claude worker run with an output schema.

## Live checks for step 13, per adapter (from step 4)
- **Codex:** (1) a worker run with FERB's base instructions in place of Codex's: FERB tools listed and called, typed
  answer returned, no `instructionSources`; compare one task's quality and tokens with before. (2) A planner prompt
  over ~245k tokens shows no `contextCompaction` item (compaction really off) and an over-window prompt comes back as
  an overflow. (3) An owner confirmation left unanswered for > 60 s in a worker: the call is not abandoned
  (`tool_timeout_sec`). (4) Daemon start journals no `runtime.codex_problem`; a run starts without a `codex --version`
  process. (5) A planner turn is cut at 90 s (step 8 words the reply).
- **Claude SDK (owner's subscription, FERB's own sign-in):** (1) without a model set, `ferb runtimes` shows Claude
  unusable and the planner isn't offered it; `runtimes.check` fails naming the model. (2) Planner turn on
  SUPERVISOR_SCHEMA (reshaped) answers and a task plan streams. (3) A worker with an output schema and many FERB tools
  runs (MCP tool schemas vs the optional/union limits). (4) `ps` during a worker run shows no run token. (5) Steer a
  worker twice: one result, the cost of the run equals Claude's final `total_cost_usd`. (6) `ferb setup
  --claude-logout` signs out and Claude becomes unusable.
- **Anthropic API (owner's key):** (1) planner turn on SUPERVISOR_SCHEMA accepted (no "too many parameters with union
  types"), answer mapped back, a plan streams. (2) A worker calling a made-up tool name gets the tool error and goes
  on. (3) Prices on `claude-haiku-5-5` usage spans.
- **Gemini API (owner's free key):** (1) planner turn on SUPERVISOR_SCHEMA and a worker on WORKER_SCHEMA accepted
  (no "too many states"); (2) a worker making two parallel calls to one tool gets both results paired correctly.
- **OpenAI API:** offline only (no key).
