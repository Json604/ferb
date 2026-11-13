# Fixtures: what a model receives that Python libraries shape (ENG-3)

Made by `make.py` from the Python build at the commit it was run on:

    cd <repo> && uv run python rust/fixtures/catalog/make.py

It starts a real `ferb.daemon.main.Daemon` in a temporary home (`FERB_HOME` and `HOME` both inside a temp folder,
`TZ=UTC`, `FakeClock` at 2026-01-01T09:00:00Z, `SequentialIds`), with the switches of `tests/conftest.py`
`RunningDaemon` and the same stand-ins as its autouse fixtures. No runtime starts, no network, no Keychain, the
`codex` program is never run. Two runs give identical bytes. Temp paths are written as `/FX`.

Reading the fixtures from Rust: **key order is contract**. Every schema and catalog keeps Python's dict order, which
is what `json.dumps` writes. Read with `serde_json` `preserve_order`; a `BTreeMap` loses it. Each schema also has
`*_forms`: the SHA-256 and byte length of its two serializations (below), so a port can check bytes without the file
storing each schema twice.

Library pins that produced these bytes (`uv.lock`): pydantic 2.13.5, pydantic-core 2.46.5, jsonschema 4.26.0,
mcp 2.2.0, CPython 3.12 (`pyproject.toml` allows pydantic `>=2,<3`; only the lock pins it).

## The two JSON serializations that matter

| Python call | separators | non-ASCII | where |
|---|---|---|---|
| `json.dumps(x)` (defaults) | `", "` and `": "` | escaped `\uXXXX`, astral as surrogate pairs, **0x7f escaped too** | prompts, RPC lines, token counts |
| `json.dumps(x, ensure_ascii=False)` | `", "` and `": "` | raw UTF-8 (U+2028/2029 raw) | tool results, most "data" blocks |
| `json.dumps(x, ensure_ascii=False, separators=(",", ":"))` | compact | raw | `schema_shape.stated`; also what httpx `json=body` writes for every provider API request (`ferb/runtimes/api.py:504`; httpx's encoder, inferred from httpx, not traced here) |

Other Python behaviour a Rust encoder does not give by default (all in `prompt_static.json` → `json_forms`):
floats print as `repr` (`1e+16`, `1e-07`, `2.0`, `-0.0`, `5e-324`); integers of any size print in full; dict keys
that are `int`/`float`/`bool`/`None` become `"1"`, `"1.5"`, `"true"`, `"null"` (`supervisor.py:7139` writes int keys);
`sort_keys=True` over mixed key types raises `TypeError`; tuples are lists; `default=str` writes `str(value)`.

## tools.json

- **Produced by** `Gateway.tools_for` (`ferb/daemon/gateway.py:717-733`): for each capability in `self.caps`
  (the registry's dict, insertion order) that the grant covers, `cap.Input.model_json_schema()` (pydantic) plus, in
  this order: `effect` (when `_acts(cap)`, `gateway.py:712`), `destination` (when `generic_input` and the model has no
  field of that name), `why` (always). The row is `{"name", "description", "input_schema"}`. Registration:
  `ferb/registry.py` (`build` and the `add_*` functions), called from `ferb/daemon/main.py:162-163,414`,
  `ferb/daemon/areas/desktop.py:322-351`, `browsers.py:55,302`, `compose.py:106,175-186`, `memory.py:56`.
- **Consumers**
  - RPC `tools.list` (`ferb/daemon/areas/chat.py:181`): `{"tools": tools_for(grant)}`, sent as `json.dumps(msg)`
    (defaults) by `ferb/daemon/server.py:452`.
  - The tool bridge `ferb/cli/mcp.py:75-79`: name → `mcp_name` (`.` → `_`), `input_schema` → `inputSchema`; nothing
    else changes (asserted in `make.py`). `mcp_tools_list.planner_example` is one full `tools/list` result as the
    model programs receive it: the mcp library's handler result carries `ttlMs`, `cacheScope`, `resultType`, which its
    runner sieves out for the 2025-06-18 protocol Codex and Claude Code negotiate (`rust/fixtures/runtimes/mcp.json`).
    The dump flags (`by_alias`, `exclude_none`) were read from the mcp package; the JSON-RPC envelope's exact bytes are
    in the runtimes fixture, not here.
  - The provider APIs `ferb/runtimes/api.py:256-261`: `ToolSpec(mcp_name, description, inline_refs(input_schema))`.
    `inline_refs` (`api.py:152`) only changes schemas with `$defs`: `mac.act`, `mac.click`, `mac.pointer`,
    `wait.for` (their `api_schema` is stored). Envelopes: OpenAI `{"type":"function","name","description",
    "parameters","strict":false}` (`openai_api.py:92`), Anthropic `{"name","description","input_schema"}`
    (`anthropic_api.py:102`), Gemini `{"functionDeclarations":[{"name","description","parametersJsonSchema"}]}`
    (`google_api.py:74`). `for_gemini` changes no tool schema today (`gemini_schema_same`).
  - Token reserve: `json.dumps(tool_schemas)` defaults (`ferb/orchestrator/worker.py:252`, `supervisor.py:3987`);
    `listings.*.json_default_sha256` is the hash of exactly that string.
  - The `actions.search` tool (`ferb/act/catalog.py:11-20`): `actions_search_row` per tool.
- **Per tool**: `name`, `mcp_name`, `description`, `input_schema` (contract, with order), `added_by_gateway`, `acts`,
  `scopes`, `identities` (sorted here; sets in Python), `facts` (every capability attribute the gateway reads with
  `getattr`/`hasattr`; a method is written `"<method>"`: its presence is the fact, its result depends on arguments).
  `risk` is the `RiskLevel` value.
- **`listings`**: the ordered names each grant gets. The catalog differs by grant, never the schema of a tool:
  identity (`cap.identities`), scopes (`cap.scopes <= grant.scopes`), `observe_only` (the checker, and the planner:
  only `observation = True` tools), `grant.tools` (the planner: `PLANNER_TOOLS`, `supervisor.py:170`, issued at
  `supervisor.py:3702`). Worker grants are issued at `worker.py:407` with the task's scopes; there is one listing
  per single scope to test the filter.
- **Incidental**: the order of `scopes`/`identities` lists, `facts` naming, the `*_forms` helpers.
- **Not constant**
  - Descriptions are properties read at listing time. `ferb_browser.fill_secret`, `fill_otp`, `otp_candidates` append
    FERB's saved secrets' names (`ferb/act/browser/tools.py:1212,1240,1261`, `_accounts`); `dynamic_descriptions`
    holds the form with one invented secret. `*.downloads` fills `{total}` from the download limits
    (`tools.py:1146`; 20 here).
  - Which tools exist depends on the Mac and its state: `mac.applescript` and all `shell.*` only when
    `sandbox-exec` containment is available (`desktop.py:316-342`; it was on the machine that made this);
    `ferb_browser.*` only while FERB's own browser profile is attached (`browsers.py:44-66`). Here it was attached
    after start with a stub driver, so those 21 tools are **last**; where they sit when a profile is already paired at
    start was not checked. `*.record` tools need the recording manager (always given by the daemon).
  - pydantic versions change schema text (titles, `anyOf` for optionals, `default` placement, `$defs` naming).

## answer_schemas.json

- **Produced by** the constants in `ferb/orchestrator/schemas.py` (hand-written dicts, no pydantic),
  `worker_schema(...)` (`schemas.py:281`, called per run by `Supervisor._report_roles`, `supervisor.py:6328-6353`: all
  64 flag sets are stored by name with hashes, the seven named ones in full), the attention schema built inside
  `Supervisor.attention` (`supervisor.py:5506-5509`, taken from the real method), and the memory/intro/check schemas
  (`ferb/memory/profile.py:63`, `distill.py:62`, `import_extract.py:48,56`, `mac_profile.py:21`, `playbooks.py:32`,
  `ferb/daemon/areas/intro.py:34`, `ferb/runtimes/check.py:30`). 86 entries.
- **Per provider** (`ferb/runtimes/schema_shape.py`):
  - Codex: core, as `outputSchema` (`codex_stream.py:453`). OpenAI API: core, in
    `text.format {"type":"json_schema","name":"ferb_result","schema":…,"strict":true}` (`openai_api.py:95`).
  - Anthropic API and Claude Code: `for_anthropic` (`schema_shape.py:175`), in `output_config.format`
    (`anthropic_api.py:106`) / `output_format` (`claude_sdk.py:213`). `anthropic.wire` is stored where it differs
    (`mapped`): SUPERVISOR (49 unions → 14 unions, 24 optionals), WORKER_STEP, WORKER_LOOP, WORKER_ALL, CHECKER,
    DISTILL. No schema is refused today.
  - Gemini: `for_gemini` (`schema_shape.py:225`) as `responseJsonSchema` (`google_api.py:79`); same as core for all.
  - Stated form (the fallback, RUN-427): `stated(schema)` (`schema_shape.py:289`) appended to the instructions
    (`claude_sdk.py:221` joins with `"\n\n"`; `api.py:286`); the planner on the API loop gets `PLANNER_ANSWER` and the
    `ferb_answer` tool with `inline_refs(core)` instead (`api.py:284`, constants in `answer_rules.py`).
  - `Refusals.key` (`schema_shape.py:271`): SHA-256 of `json.dumps([provider, model or "", wire], sort_keys=True,
    separators=(",", ":"))`; one example.
- **Contract**: core and wire schemas with key order; the stated text. **Incidental**: `complexity_*`, `where`.

## prompt_static.json

- **Produced by** `Supervisor._system` (`ferb/orchestrator/supervisor.py:1230-1258`), called on the real supervisor
  with its six inputs replaced by fixed values (`facts`, `live_facts`, `availability().text`, `identity_context`,
  `skills_catalog`, `abilities_catalog`). `assembly` lists the steps; `load_prompt` (`ferb/core/prompts.py:10`) reads
  the file as UTF-8 and `.strip()`s it. 16 calls × 2 variants: `minimal` (empty facts and profile, no skills, the real
  ability catalog from `ferb/abilities/catalog.py:50-69`) and `rich` (non-ASCII, quotes, control characters, floats,
  big integers, nulls, nesting through both `json.dumps` calls at 1255 and 1257).
- **Consumers**: `_supervise` (`supervisor.py:3622`, `live=False`), `_run_worker_once` (`worker.py:438`), attention
  (`5534`), the question router (`7067`), the room estimate (`3986`).
- `prompt_files`: SHA-256 of every Markdown file under `ferb/orchestrator/prompts` (raw and stripped) and which
  placeholders it holds, so a test can tell "the Markdown changed" from "the assembly drifted".
- Code-composed pieces: `runtime_tools_blocks` (`worker.py:409-423`: `load_prompt(runtime_tools.md, {tool_names})` +
  `"\n\n"` + `clock.context.time_block`, `ferb/clock/context.py:72`), `tool_result_footers` (`clock/context.py:34`),
  `context_budget_reports` (`ferb/core/context.py:359`; both `token_method` strings), `wrap_untrusted`
  (`ferb/sanitize/untrusted.py:115`), `catalog_lines`, `json_forms` (10 invented values × 7 parameter sets, plus
  `default=str`), and `json_dumps_call_sites`: every `json.dumps` in `ferb/` read from source with `ast` (238 calls:
  file, line, function, keyword arguments), so a moved or changed call shows as a fixture diff.
- **Contract**: `system` bytes; the code-composed texts. **Incidental**: `what`, hashes.

### Every prompt-feeding `json.dumps` call site

Only two parameter sets reach a model from these files: defaults, and `ensure_ascii=False`. No `sort_keys`, no
`indent`, no custom separators (the one `indent=2` writes a file).

`ferb/orchestrator/supervisor.py`, **defaults**: 1255, 1257 (`_system` catalogs); 1565, 1585, 1599, 1617
(`_problems`, plan-repair text); 1644 (schedule facts); 2592 (`input_paths`); 3561 (plan repair); 3773 (questions
block); 3959 (`earlier_results` problem); 4064 (timeline entry); 5518, 5519, 5521 (attention); 6378 (missing
evidence notes, becomes `tree.reason`); 6887, 6896, 6907, 6943, 6949, 6963 (`_context` blocks); 7059 (question
options); 7139 (condition layers, **int keys**), 7147 (worker report), 7152 (worker types), 7157 (code checks).
**`ensure_ascii=False`**: 789 (front window); 1448, 1575 (quotes in refusals); 2194 (task state); 4368 (since
block); 5708 (probe exposure); 6584, 6616 (findings); 6939, 6953 (`_context`); 7035 (schedule block); 7143, 7151
(`_verifier`); 7207, 7312, 7313, 7325, 7327, 7329 (changed-file lines); 7244 (code facts).
Not model text (my reading of the call, not traced further): 2179, 3986, 3987 (token counts); 3592
(`planner_sources`, `ensure_ascii=False`); 5204, 5589, 5774 (`step_context`, stored, read back with `json.loads`);
5234 (`ensure_ascii=False, indent=2`, a file); 7043 (a hash key).

`ferb/orchestrator/worker.py`: 252 twice, defaults (token reserve); 496, defaults (span output, a record).
`ferb/core/context.py`: 359, defaults (`Context budget:` line). `ferb/clock/context.py`: none (f-strings only).
There is no `ferb/orchestrator/context.py`. `ferb/cli/mcp.py`: 97, `ensure_ascii=False`
(`json.dumps(content) + "\n" + footer`, every tool result; the API loop does the same at `ferb/runtimes/api.py:470`).

Others that reach a model: `ferb/runtimes/schema_shape.py:77` (`ensure_ascii=False`, a mapped-back answer), `:295`
(compact, `ensure_ascii=False`); `ferb/runtimes/api.py:347` (`ensure_ascii=False`); `claude_sdk.py:418,443`
(defaults); `google_api.py:203` (`ensure_ascii=False`); `ferb/orchestrator/evidence.py:192,194,360,410`
(`ensure_ascii=False`; 360 also `default=str`), `conversation_tools.py:39`, `loops.py:134,138,409`
(`ensure_ascii=False`); `ferb/memory/mac_profile.py:41` (`ensure_ascii=False, indent=1`). `evidence.py:360` and
`conversation_tools.py:39` page the text by **Python string index** (code points), not bytes.

## repair_notes.json

- **Produced by** `check_final` (`ferb/core/structured.py:15-27`): `json.loads`, then
  `jsonschema.exceptions.best_match(Draft202012Validator(schema).iter_errors(value))`; the text is
  `the answer doesn't match the schema at <path joined by "/" or "the top level">: <error.message[:200]>` or
  `the answer is not JSON (<JSONDecodeError.msg>)`. `repair_note` (`schema_shape.py:298`) wraps it.
- **Consumers**: the API loop (`api.py:351-406`), Claude (`claude_sdk.py:426-448`), learning calls
  (`ferb/memory/calls.py:55`), the connection check (`ferb/daemon/areas/runtimes.py:547`); the worker's own two
  messages are in `worker_side` (`worker.py:441-446`).
- 117 cases over 18 schemas, 17 valid examples, 13 `mapped_back` (Anthropic wire answer → `Shape.answer_text`,
  `schema_shape.py:69`, → core text → check), 9 `unfenced`.
- **Contract**: `error`, `repair_note`, `core_answer`. A port must reproduce: which error `best_match` picks
  (jsonschema's relevance order), jsonschema's messages with **Python `repr`** of the value (`None`, `True`,
  `'text'`, `['a', 'b']`, `{'k': 1}`), CPython's JSON error words, and the cut at 200 characters (characters, not
  bytes).

## validation_errors.json

- **Produced by** `Gateway._build` (`ferb/daemon/gateway.py:1130-1150`): `why`, `effect` and (generic input) the
  declared `destination` are popped, then `cap.Input.model_validate`; the reply is `{"error": "invalid arguments",
  "details": e.errors(include_url=False)}`; an invalid `destination` gets `loc` prefixed with `"destination"`
  (`Destination`, `gateway.py:250`). `_replier` (`gateway.py:1017`) redacts it and adds the footer.
- **Consumers**: `model_text` is what the model reads (`mcp.py:97`, `api.py:470`); `rpc_content_json` is the content
  on the daemon socket.
- 627 cases over 97 tools, each through the real `Gateway.call`; only arguments pydantic refuses were sent, so no
  tool ran. Six tools have none (`browser.profiles`, `browser.downloads`, `ferb_browser.downloads`, `mac.apps`,
  `shell.list`, `shell.rules`): no required argument, and pydantic ignores unknown ones. `before_validation`: the five
  refusals of `_admit` (`gateway.py:1066-1097`).
- **Contract**: `content` (pydantic's `type`, `loc`, `msg`, `input`, `ctx`) and `model_text`. **Depends on**
  pydantic-core's message catalogue.
- **Found in Python**: a `model_validator` that raises `ValueError` (`browser.pdf`, `ferb_browser.pdf`,
  `audio.listen`) leaves the exception object in `details[].ctx.error`, and `json.dumps` of the reply raises
  `TypeError` (5 cases, `model_text_error`). What the model then receives was not traced (the RPC server answers an
  internal error by type name, `server.py:327-329`; inferred).

## Not frozen, and why

- The real `facts()`, `live_facts()`, availability text and the Tier 1 profile: they depend on the Mac, the config,
  the installed runtimes and memory; each is its own port with its own fixture.
- Every prompt-feeding `json.dumps` site other than `_system`'s two: they sit inside turns that need a task tree,
  plan and history. Their parameters are listed above and `json_forms` fixes the encoder; the surrounding text of
  each block is not covered.
- `wrap_untrusted`'s id is `secrets.token_hex(6)`, new on every call; fixed to `0123456789ab` here.
- Tool descriptions and the catalog's membership and order as described under tools.json.
- The mcp library's JSON-RPC frames and the provider request bodies around the schemas (httpx).
- `Refusals` is time-dependent memory; only its key function is fixed.
- The other prompt Markdown (`ferb/memory/*.md`, `ferb/decide/prompts/*.md`, abilities' `SKILL.md`) is loaded with
  `load_prompt` alone; abilities' bodies are referenced by SHA-256.
