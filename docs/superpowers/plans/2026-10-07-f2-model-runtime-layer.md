# F2 Model Runtime Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every model call in FERB goes through one FERB-defined interface, with adapters for the Codex CLI, the Claude Agent SDK and the direct Anthropic, OpenAI and Google APIs. The interface is selectable per role, keeps a stable cacheable prefix, enforces and checks typed answers, reports normalized token usage and price, and keeps API keys in the Keychain (D32, P20).

**Architecture:** `ModelRequest` carries FERB's instructions and the context as ordered parts, with the count of leading parts that repeat across calls. `assemble_parts` builds it, and context blocks marked `cache=True` define the prefix. CLI adapters (Codex, Claude) render it as text, as today. One shared `ApiRuntime` runs the tool loop for the direct APIs: every tool call goes through FERB's gateway via `GatewayBridge`. A small per-provider class translates requests, responses, cache markers, typed output and usage into each provider's native form. FERB checks every final answer against the requested schema in one place. Usage is normalized into one `Usage` record and priced from a dated table.

**Tech Stack:** Python 3.12, httpx 0.28.1 (no provider SDKs), jsonschema 4.26.0, pytest with `httpx.MockTransport`, the pinned Codex CLI 0.157.1, claude-agent-sdk 0.2.160.

**Spec:** `docs/0.27_PLAN.md` — D32 (critique section, "Universal model runtimes"), P20 and its F2 note, the "Final build order" F2 line, and the D32 registry note in `docs/0.27_DECISIONS.toml`.

## Global Constraints

- Python 3.12; `uv sync --group dev`; before every commit run the focused tests, then `uv run pytest -q`, `uv run mypy ferb/core` and `git diff --check`.
- D32: "FERB defines one model interface (context blocks, tools, output schema; tool calls, structured results, token usage) … selectable per role (orchestrator, worker, checker). Everything model-specific lives in the adapter … Tools always run through FERB's gateway whatever the adapter. Each adapter passes the same contract and scenario tests; a capability table records what each supports. API keys live in the Keychain."
- P20: stable content first, changing content last. F2 adds cache markers and a cache-routing key that stays the same across one role's calls.
- Provider HTTP goes through httpx only; no provider SDK dependencies. Provider-hosted tools (web search, code execution, file search, computer use) are never enabled: FERB's gateway tools are the only tools.
- API keys live only in the Keychain: service `ferb`, account `api/<provider>` (`anthropic`, `openai`, `google`). Never put them in config, command lines, logs, spans, the journal, error text or tests' stored output (CLAUDE.md rule 14).
- No task, turn or token budgets (CLAUDE.md rule 6 and the context-budget docstring). The only exception is a per-reply output ceiling where an API requires one (Anthropic `max_tokens`), and a cut-off reply is an error, never a partial answer.
- Code never judges intent (rule 6). Adapters translate; they don't decide.
- Live tests are owner-run: `-m real` (Codex), `-m real_claude`, and the new `-m real_api` (direct APIs, billed per token). None run under the default `uv run pytest -q`.
- Do not install or restart the daemon. The owner runs `ferb setup --update`. Never read or search `vibed/`.
- Commit messages cite D32 and/or P20 and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Provider facts (verified 2026-10-07 from official docs; the sources are in each provider module's docstring)

- **OpenAI Responses API** (`POST https://api.openai.com/v1/responses`, `Authorization: Bearer`):
  - Request fields:
    - `input` items: `{"role": "developer"|"user", "content": [{"type": "input_text", "text": …}]}`.
    - Function tools: `{"type": "function", "name", "description", "parameters", "strict"}`.
    - Typed output: `text.format = {"type": "json_schema", "name", "schema", "strict": true}`.
    - Effort: `reasoning.effort`.
    - `store: false`.
  - Every output item is replayed on the next call. Reasoning items carry `encrypted_content` by default when `store` is false.
  - Tool calls and results:
    - The model emits `{"type": "function_call", "call_id", "name", "arguments": "<JSON string>"}`.
    - FERB replies with `{"type": "function_call_output", "call_id", "output": str | [input_text, input_image]}`.
  - Usage: `input_tokens` (includes cached), `input_tokens_details.cached_tokens`, `input_tokens_details.cache_write_tokens`, `output_tokens` (includes reasoning), `output_tokens_details.reasoning_tokens`.
  - Caching on GPT-5.6 and later (all gpt-6.x):
    - Explicit `prompt_cache_breakpoint: {"mode": "explicit"}` on `input_text` blocks; `instructions` can't carry one, so FERB's instructions go in a developer message.
    - At most 4 cache writes.
    - `prompt_cache_key` is optional.
  - Caching on gpt-5.5: implicit only, and `prompt_cache_key` steers routing.
- **Anthropic Messages API** (`POST https://api.anthropic.com/v1/messages`, `x-api-key`, `anthropic-version: 2023-06-01`):
  - Caching:
    - Breakpoints: `cache_control: {"type": "ephemeral"}` on system and content blocks, plus a top-level automatic one that moves forward. At most 4 in total.
    - 20-block lookback.
    - Minimum cacheable length: 512 tokens on the 5.x models, 4,096 on Haiku 4.5.
  - Typed output: `output_config.format = {"type": "json_schema", "schema"}`. It is GA and works together with tools.
  - `tool_choice`: only `auto` and `none` on Opus 5.5, Sonnet 5.5 and Fable 5.1; `any` and `tool` return 400.
  - Effort: `output_config.effort` (Haiku 4.5 doesn't support it).
  - Thinking can't be disabled on the 5.x models. Assistant content, thinking blocks included, is passed back unchanged.
  - Usage: `input_tokens` covers only tokens after the last breakpoint; also `cache_creation_input_tokens`, `cache_read_input_tokens`, `output_tokens`.
  - `max_tokens` is required.
- **Gemini API** (`POST https://generativelanguage.googleapis.com/v1beta/models/<model>:generateContent`, `x-goog-api-key`):
  - Request fields:
    - `systemInstruction`, `contents`.
    - `tools[].functionDeclarations[]` with `parametersJsonSchema` (JSON Schema; mutually exclusive with `parameters`).
    - `generationConfig.responseMimeType: "application/json"` and `responseJsonSchema`; with tools this works only on Gemini 3 (gemini-3.8-flash, gemini-3.1-pro-preview).
    - `generationConfig.thinkingConfig.thinkingLevel` (`low`/`medium`/`high`; `minimal` is unsupported on 3.8-flash and 3.1-pro).
  - Function calls and thought signatures:
    - Function-call parts carry a `thoughtSignature` that must be passed back exactly. FERB passes the whole model content back.
    - Replies use `functionResponse: {name, id, response, parts?}`.
  - Caching is implicit: at least 4,096 tokens on 3.x models; hits show in `usageMetadata.cachedContentTokenCount`.
  - Usage: `promptTokenCount` (includes cached), `candidatesTokenCount` (excludes thoughts), `thoughtsTokenCount`, `toolUsePromptTokenCount`.
- **Codex 0.157.1** sends `prompt_cache_key` = its per-process session id, and no setting changes it. Measured: three identical `codex exec` calls cached 6,656 / 6,656 / 13,824 of 14,840 tokens (gpt-5.5) and 0 / 0 / 0 (gpt-6-sol). It also loads `~/.agents/skills` unless the Codex process gets its own `HOME` (verified: with `HOME` set to an empty directory, nothing loads and the answer still comes back).

## Review Focus

- A provider answers with something that isn't the requested JSON (a refusal, or a reply cut off at the output limit): the run ends with an error naming why, never a partial "final". Tests: Task 2 (`check_final`), and Tasks 7–9 (`test_cut_off_reply_is_an_error`).
- The model sends tool arguments that aren't a JSON object: FERB returns an error result to the model without calling the gateway, and the run continues. Tests: Task 6 (`test_bad_arguments_never_reach_the_gateway`) and the Tasks 7–9 contract case.
- A network failure or provider 5xx/429 mid-run: retried with backoff, then a clear error. No hang. Tests: Task 6 (`test_retries_then_succeeds`, `test_network_failure_after_retries_is_an_error`).
- The runtime chosen for a role has no API key, or isn't installed: the owner gets a message naming the fix (`ferb setup --key <provider>`), and the planner never offers unavailable runtimes as worker types. Tests: Task 6 (`test_missing_key_names_the_fix`) and Task 10 (`test_worker_types_list_only_available_runtimes`).
- A tool result with an image (e.g. `mac.screenshot`): it reaches the model in that provider's native form. Test: the Tasks 7–9 contract scenario `test_image_results_reach_the_model`.

---

### Task 1: One request shape with a cacheable prefix

**Files:**
- Modify: `ferb/core/context.py` (`ContextBlock`, new `assemble_parts`, `assemble_context` becomes a join)
- Modify: `ferb/core/types.py` (new `ModelRequest`)
- Modify: `ferb/core/interfaces.py:26-32` (`AgentRuntime.start`)
- Modify: `ferb/runtimes/codex_exec.py:148` and `ferb/runtimes/claude_sdk.py:56` (`start` signature)
- Modify: `ferb/orchestrator/worker.py:162-171` (`_run` builds the request)
- Modify: `ferb/orchestrator/supervisor.py:371-394` (`_supervise` marks blocks) and `:761-767` (`_context` marks blocks)
- Modify: `ferb/memory/distill.py:150-157`
- Modify: `tests/conftest.py` (`Script`, `FakeRuntime.start`, `run_direct`)
- Test: `tests/test_model_request.py`

**Interfaces:**
- Produces:
  - `ContextBlock(text, reference, protected=False, cache=False)`.
  - `assemble_parts(blocks, *, budget_tokens=DEFAULT_BUDGET_TOKENS, reserved_tokens=0) -> tuple[list[str], int]`, returning the parts, budget report last, and the number of cacheable leading parts.
  - `assemble_context(...) -> str`, the same text as before.
  - `ModelRequest(system: str, parts: tuple[str, ...], cacheable: int = 0)` with a `.context` property (`"\n\n".join(parts)`).
  - `AgentRuntime.start(task, request: ModelRequest, run_token) -> RunHandle`.
  - `Script.requests: list[tuple[str, str, ModelRequest]]` in tests.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_model_request.py
"""One request shape for every runtime: instructions, ordered context parts, and the cacheable prefix (D32, P20)."""
from ferb.core.context import ContextBlock, assemble_context, assemble_parts
from ferb.core.types import ModelRequest
from tests.conftest import chat, install, sup


def test_parts_keep_order_and_count_the_cacheable_prefix() -> None:
    parts, cacheable = assemble_parts([ContextBlock("facts", "a", protected=True, cache=True),
                                       ContextBlock("history", "b", protected=True, cache=True),
                                       ContextBlock("memory", "c"),
                                       ContextBlock("message", "d", protected=True)])
    assert parts[:4] == ["facts", "history", "memory", "message"]
    assert parts[-1].startswith("Context budget: ")
    assert cacheable == 2


def test_an_omitted_cache_block_ends_the_cacheable_prefix() -> None:
    parts, cacheable = assemble_parts([ContextBlock("long " * 400, "a", cache=True),
                                       ContextBlock("history", "b", protected=True, cache=True)], budget_tokens=60)
    assert parts[0].startswith("[Context omitted") and cacheable == 0


def test_assemble_context_is_the_joined_parts() -> None:
    blocks = [ContextBlock("one", "a", protected=True), ContextBlock("two", "b")]
    assert assemble_context(blocks) == "\n\n".join(assemble_parts(blocks)[0])
    assert assemble_context([]) == "\n\n".join(assemble_parts([])[0]) and assemble_context([]).startswith("\n\n")


def test_model_request_context_joins_parts() -> None:
    assert ModelRequest("sys", ("a", "b"), 1).context == "a\n\nb"


def test_supervisor_request_marks_device_facts_and_history_cacheable(daemon) -> None:
    script = install(daemon, [sup("reply", "first"), sup("reply", "second")])
    chat(daemon, "an earlier message")
    chat(daemon, "the new message")
    request = [r for _, key, r in script.requests if key == "supervisor"][-1]
    assert request.parts[0].startswith("This Mac: ") and request.parts[1].startswith("Conversation so far")
    assert request.cacheable == 2
    assert request.system                                 # FERB's instructions travel separately from the context
```

- [ ] **Step 2: Run them to see them fail**

Run: `uv run pytest tests/test_model_request.py -q`
Expected: FAIL with `ImportError: cannot import name 'assemble_parts'`.

- [ ] **Step 3: Implement `cache` and `assemble_parts` in `ferb/core/context.py`**

Replace the `ContextBlock` dataclass and `assemble_context` with:

```python
@dataclass(frozen=True)
class ContextBlock:
    text: str
    reference: str
    protected: bool = False
    cache: bool = False      # repeats across this role's calls; the cacheable prefix ends after the last leading one (P20)


def assemble_parts(blocks: list[ContextBlock], *, budget_tokens: int = DEFAULT_BUDGET_TOKENS,
                   reserved_tokens: int = 0) -> tuple[list[str], int]:
    """Reserve prompts/tools first; omit whole optional blocks, keeping retrieval references visible. Returns the context
as ordered parts (the budget report last) and how many leading parts repeat across calls (adapters cache them).

Protected source text can exceed the configured budget. Reporting that fact is preferable to changing it.
The budget is a context allocation, not a task/output/token-spend limit.
"""
    if budget_tokens <= 0 or reserved_tokens < 0:
        raise ValueError("context budget must be positive and reserve nonnegative")
    active = [block for block in blocks if block.text]
    kept = {index for index, block in enumerate(active) if block.protected}

    def render() -> list[str]:
        parts = [block.text if index in kept else f"[Context omitted; fetch full text: {block.reference}]"
                 for index, block in enumerate(active)] or [""]
        body = "\n\n".join(parts)
        used = count_tokens(body) + reserved_tokens
        report = {"budget_tokens": budget_tokens, "reserved_tokens": reserved_tokens,
                  "content_tokens_estimate": count_tokens(body), "method": token_method(),
                  "over_budget": used > budget_tokens,
                  "omitted_blocks": len(active) - len(kept)}
        return parts + ["Context budget: " + json.dumps(report)]

    for index, block in enumerate(active):
        if block.protected:
            continue
        kept.add(index)
        if count_tokens("\n\n".join(render())) + reserved_tokens > budget_tokens:
            kept.remove(index)
    parts = render()
    # Header/omission references also consume context. Report their unavoidable overhead truthfully.
    if count_tokens("\n\n".join(parts)) + reserved_tokens > budget_tokens and '"over_budget": false' in parts[-1]:
        parts[-1] = parts[-1].replace('"over_budget": false', '"over_budget": true', 1)
    cacheable = 0
    for index, block in enumerate(active):
        if not (block.cache and index in kept):
            break
        cacheable += 1
    return parts, cacheable


def assemble_context(blocks: list[ContextBlock], *, budget_tokens: int = DEFAULT_BUDGET_TOKENS,
                     reserved_tokens: int = 0) -> str:
    """The assembled context as one text (`assemble_parts` joined)."""
    return "\n\n".join(assemble_parts(blocks, budget_tokens=budget_tokens, reserved_tokens=reserved_tokens)[0])
```

`render()` returns `[""]` for an empty context, so `assemble_context([])` keeps its old leading `"\n\n"`.

- [ ] **Step 4: Add `ModelRequest` to `ferb/core/types.py`** (next to `RunHandle`)

```python
@dataclass(frozen=True)
class ModelRequest:
    """What FERB sends any model runtime (D32): its instructions and the context in order, stable parts first (P20)."""
    system: str
    parts: tuple[str, ...]
    cacheable: int = 0                       # parts[:cacheable] repeat across this role's calls; adapters cache them

    @property
    def context(self) -> str:
        return "\n\n".join(self.parts)
```

- [ ] **Step 5: Change the runtime interface and the two CLI adapters**

In `ferb/core/interfaces.py`, import `ModelRequest` from `ferb.core.types` and change `AgentRuntime.start` to:

```python
    async def start(self, task: Task, request: ModelRequest, run_token: str) -> RunHandle: ...
```

In `ferb/runtimes/codex_exec.py`, change `start(self, task: Task, system: str, context: str, run_token: str)` to `start(self, task: Task, request: ModelRequest, run_token: str)`. Make the first line of the body `system, context = request.system, request.context`; everything else stays the same. Do the same in `ferb/runtimes/claude_sdk.py` (`client.query(f"{request.system}\n\n{request.context}")`). Import `ModelRequest` in both.

- [ ] **Step 6: Build the request in `worker._run`**

In `ferb/orchestrator/worker.py`, replace the `ctx_text = assemble_context(...)` line and the `rt.start` call with:

```python
        parts, cacheable = assemble_parts(ctx, budget_tokens=self.context_budget_tokens,
                                          reserved_tokens=count_tokens(system) + count_tokens(json.dumps(tool_schemas)) +
                                                          count_tokens(json.dumps(output_schema)) + 1024)
        request = ModelRequest(system, tuple(parts), cacheable)
        self.spans.update_input(parent_span, {"context": request.context, "system_prompt": system,
                                             "context_budget_tokens": self.context_budget_tokens})
        run = await rt.start(task, request, token)
```

Import `assemble_parts` from `ferb.core.context` and `ModelRequest` from `ferb.core.types`. Remove the `assemble_context` import if it is no longer used there.

- [ ] **Step 7: Mark the cacheable blocks in the supervisor**

In `ferb/orchestrator/supervisor.py` `_supervise`, add `cache=True` to the device-facts block and to the history block:

```python
        blocks = [ContextBlock(_env_text(env), "actions.search and current device facts", cache=True),  # same on every turn
                  ContextBlock(recent, "chat.history", protected=True, cache=True),                     # grows only at its end
```

The snapshot rewrite a few lines below recreates non-protected blocks and would drop `cache`. Change it to `dataclasses.replace`:

```python
        blocks = [block if block.protected else dataclasses.replace(
                      block, reference=block.reference + "; original planner snapshot: task.evidence kind=context.source "
                                                         "query=planner-context") for block in blocks]
```

(`import dataclasses` at the top.) In `_context(self, tree)`, add `cache=True` to the first two blocks: the owner's complete original request and the task specification. Both repeat across one task's worker, dispatch and checker calls.

- [ ] **Step 8: Distiller and test doubles**

In `ferb/memory/distill.py` `_model`, replace the `assemble_context` call and `rt.start(task, system, ctx, "")` with:

```python
        parts, cacheable = assemble_parts([ContextBlock(ctx, f"daily evidence {day}", protected=True)],
                                          reserved_tokens=count_tokens(system))
        run = await rt.start(task, ModelRequest(system, tuple(parts), cacheable), "")
```

In `tests/conftest.py`:
- Add `self.requests: list[tuple[str, str, Any]] = []` to `Script.__init__`.
- Change `FakeRuntime.start` to `async def start(self, task: Task, request: ModelRequest, run_token: str) -> RunHandle:`. It appends `(self.mode, key, request.context, self.tools)` to `self.script.contexts` as before, and also `(self.mode, key, request)` to `self.script.requests`.
- In `run_direct`, call `rt.start(task, ModelRequest("", (prompt,), 0), token)`.

Then run `grep -rn "\.start(task\|rt\.start(" ferb tests` and update any remaining caller the same way.

- [ ] **Step 9: Run the tests**

Run: `uv run pytest tests/test_model_request.py tests/test_prompt_order.py tests/test_history_cap.py tests/test_hardening_context.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: all pass. The context text every existing test sees is unchanged.

- [ ] **Step 10: Commit**

```bash
git add ferb/core/context.py ferb/core/types.py ferb/core/interfaces.py ferb/runtimes/codex_exec.py ferb/runtimes/claude_sdk.py ferb/orchestrator/worker.py ferb/orchestrator/supervisor.py ferb/memory/distill.py tests/conftest.py tests/test_model_request.py
git commit -m "F2 (D32, P20): one request shape for every runtime, with the cacheable prefix marked

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Typed answers are checked once, for every runtime

**Files:**
- Create: `ferb/core/structured.py`
- Modify: `ferb/orchestrator/worker.py` (`_run`, after the final event)
- Modify: `ferb/memory/distill.py` (`_model`, after the run)
- Modify: `pyproject.toml` (direct dependency `jsonschema==4.26.0`; dev dependency `types-jsonschema`)
- Test: `tests/test_structured_results.py`

**Interfaces:**
- Consumes: `ModelRequest` (Task 1).
- Produces: `check_final(text: str, schema: dict[str, Any] | None) -> str`. It returns `text` unchanged, or raises `ValueError("the answer …")`.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_structured_results.py
"""Whatever a provider promises, FERB checks each typed answer against the schema it asked for (D32)."""
import json

import pytest

from ferb.core.structured import check_final
from ferb.memory.distill import DISTILL_SCHEMA
from ferb.orchestrator import schemas
from tests.conftest import install, sup

SCHEMA = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"], "additionalProperties": False}


def test_a_matching_answer_passes_unchanged() -> None:
    assert check_final('{"ok": true}', SCHEMA) == '{"ok": true}'


@pytest.mark.parametrize("text,why", [("not json", "not JSON"), ('{"ok": "yes"}', "at ok"), ('{}', "top level"),
                                      ('{"ok": true, "extra": 1}', "top level")])
def test_a_wrong_answer_names_the_problem(text: str, why: str) -> None:
    with pytest.raises(ValueError, match=why):
        check_final(text, SCHEMA)


def test_no_schema_means_free_text() -> None:
    assert check_final("plain words", None) == "plain words"


def test_every_ferb_output_schema_is_strict_compatible() -> None:
    """OpenAI strict mode and Anthropic output_config need additionalProperties false and every property required."""
    def problems(node, path="$"):
        out = []
        if isinstance(node, dict):
            if node.get("type") == "object" or "properties" in node:
                if node.get("additionalProperties") is not False:
                    out.append(f"{path}: additionalProperties")
                if set(node.get("properties") or {}) - set(node.get("required") or []):
                    out.append(f"{path}: required")
            for k, v in node.items():
                out += problems(v, f"{path}.{k}")
        elif isinstance(node, list):
            for i, v in enumerate(node):
                out += problems(v, f"{path}[{i}]")
        return out
    found = {n: problems(getattr(schemas, n)) for n in dir(schemas) if n.endswith("_SCHEMA")}
    found["DISTILL_SCHEMA"] = problems(DISTILL_SCHEMA)
    assert not {n: p for n, p in found.items() if p}


def test_a_worker_answer_that_breaks_the_schema_is_not_used(daemon) -> None:
    from tests.conftest import FakeRuntime, chat, outcome, task
    script = install(daemon, [sup("task", "Checking.", task("check something"))],
                     {"worker": json.dumps({"status": "done"})})      # WORKER_SCHEMA also requires summary, artifacts, evidence

    def make(role, *, mode, tool_names, schema):
        rt = FakeRuntime(script, mode, tool_names)
        if mode == "worker":
            rt.output_schema = schema                         # the test double carries the schema a real runtime would
        return rt
    daemon.daemon.supervisor.make_runtime = make
    out = outcome(chat(daemon, "check something"))
    assert out["status"] != "done"
    errors = daemon.run(_worker_errors(daemon))
    assert any("doesn't match the schema" in (e or "") for e in errors)


async def _worker_errors(daemon):
    return [r["error"] for r in daemon.daemon.db.conn.execute("SELECT error FROM spans WHERE kind = 'worker'").fetchall()]
```

The last test reaches the existing "worker's report wasn't usable" path. `FakeRuntime` normally has no `output_schema` attribute, so existing tests' scripted answers (for example `sup()`'s, which omit fields `SUPERVISOR_SCHEMA` requires) are not checked. That matches today. Don't add a schema to `install()`'s fake. If `spans` stores errors under another column name, read `ferb/store/migrations` and use it.

- [ ] **Step 2: Run them to see them fail**

Run: `uv run pytest tests/test_structured_results.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'ferb.core.structured'`.

- [ ] **Step 3: Implement `ferb/core/structured.py`**

```python
"""Owns checking a model's typed answer against the schema FERB asked for. Must not repair, guess or fill fields.

Every runtime's final answer passes here once, whatever the provider promised about enforcing the schema (D32)."""

import json
from typing import Any

import jsonschema


def check_final(text: str, schema: dict[str, Any] | None) -> str:
    """The answer unchanged when it is JSON matching schema; ValueError naming the first problem otherwise."""
    if not schema:
        return text
    try:
        value = json.loads(text)
    except ValueError as e:
        raise ValueError(f"the answer is not JSON ({e.msg})") from None
    error = jsonschema.exceptions.best_match(jsonschema.Draft202012Validator(schema).iter_errors(value))
    if error is not None:
        where = "/".join(str(p) for p in error.absolute_path) or "the top level"
        raise ValueError(f"the answer doesn't match the schema at {where}: {error.message[:200]}")
    return text
```

Add `"jsonschema==4.26.0"` to `[project] dependencies` and run `uv add --group dev types-jsonschema` so strict mypy on `ferb/core` passes.

- [ ] **Step 4: Use it in `worker._run` and the distiller**

In `worker._run`, after the loop and before `return final`, add `final = check_final(final, output_schema)`. `output_schema` is already read there with `getattr(rt, "output_schema", None)`. The `ValueError` lands in `_run_worker_once`'s existing `except (ValidationError, ValueError, Stalled)` branch.

In `distill._model`, before `parsed = json.loads(final)`, add `final = check_final(final, DISTILL_SCHEMA)`.

- [ ] **Step 5: Run the tests**

Run: `uv run pytest tests/test_structured_results.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS. If an existing test fails because a runtime double now carries a schema its scripted answer breaks, fix the scripted answer, never the check, and list each such change in the report.

- [ ] **Step 6: Commit**

```bash
git add ferb/core/structured.py ferb/orchestrator/worker.py ferb/memory/distill.py pyproject.toml uv.lock tests/test_structured_results.py
git commit -m "F2 (D32): every runtime's typed answer is checked against its schema in one place

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Normalized token usage and dated prices

**Files:**
- Create: `ferb/runtimes/usage.py`, `ferb/runtimes/prices.py`
- Modify: `ferb/runtimes/codex_exec.py` (the `turn.completed` usage event), `ferb/runtimes/claude_sdk.py` (the `ResultMessage` usage event)
- Modify: `ferb/memory/distill.py` (record usage), `ferb/daemon/main.py:668` (wire the distiller's usage into spans)
- Test: `tests/test_usage_prices.py`

**Interfaces:**
- Produces:
  - `Usage` (frozen dataclass): `model, input_tokens, cached_input_tokens, cache_write_tokens, output_tokens, reasoning_tokens, cost_usd, cost_basis, billing`.
  - `from_openai(model, usage, billing) -> Usage`, `from_codex(model, usage) -> Usage`, `from_anthropic(model, usage, billing, provider_cost=None) -> Usage`, `from_gemini(model, usage_metadata, billing) -> Usage`.
  - `cost_usd(model, input_tokens, cached, cache_write, output, day=None) -> float | None`.
  - `Price`, and `PRICES: dict[str, list[Price]]`.
  - `Distiller(..., record_usage=Callable[[str, dict], None])`.
  - Usage events carry `dataclasses.asdict(Usage)`. The keys `input_tokens` and `cached_input_tokens` keep `ferb baseline` working.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_usage_prices.py
"""Every runtime reports usage the same way; cost comes from a dated price table, never guessed (D32)."""
import asyncio
import json
import sys
from pathlib import Path

import pytest

from ferb.core.types import RunHandle
from ferb.runtimes.codex_exec import CodexRuntime
from ferb.runtimes.usage import cost_usd, from_anthropic, from_codex, from_gemini, from_openai


def test_openai_usage_counts_cached_inside_input() -> None:
    u = from_openai("gpt-6.1-sol", {"input_tokens": 5100, "input_tokens_details": {"cached_tokens": 3000,
                    "cache_write_tokens": 2000}, "output_tokens": 50, "output_tokens_details": {"reasoning_tokens": 20}},
                    billing="api")
    assert (u.input_tokens, u.cached_input_tokens, u.cache_write_tokens, u.output_tokens, u.reasoning_tokens) == \
           (5100, 3000, 2000, 50, 20)
    # 100 fresh × $2 + 3000 cached × $0.10 + 2000 written × $2.50 + 50 out × $10, per 1M tokens
    assert u.cost_usd == pytest.approx((100 * 2 + 3000 * 0.10 + 2000 * 2.50 + 50 * 10) / 1e6)
    assert u.cost_basis == "price_table" and u.billing == "api"


def test_anthropic_input_adds_cache_reads_and_writes() -> None:
    u = from_anthropic("claude-sonnet-5-5", {"input_tokens": 100, "cache_creation_input_tokens": 2000,
                       "cache_read_input_tokens": 3000, "output_tokens": 50}, billing="api")
    assert (u.input_tokens, u.cached_input_tokens, u.cache_write_tokens) == (5100, 3000, 2000)
    assert u.cost_usd == pytest.approx((100 * 2 + 3000 * 0.20 + 2000 * 2.50 + 50 * 10) / 1e6)


def test_claude_sdk_cost_comes_from_the_sdk() -> None:
    u = from_anthropic("claude", {"input_tokens": 10, "output_tokens": 5}, billing="subscription", provider_cost=0.0123)
    assert u.cost_usd == 0.0123 and u.cost_basis == "provider" and u.billing == "subscription"


def test_gemini_output_includes_thoughts_and_input_includes_tool_prompt() -> None:
    u = from_gemini("gemini-3.8-flash", {"promptTokenCount": 5000, "cachedContentTokenCount": 3000,
                    "candidatesTokenCount": 30, "thoughtsTokenCount": 20, "toolUsePromptTokenCount": 100}, billing="api")
    assert (u.input_tokens, u.cached_input_tokens, u.output_tokens, u.reasoning_tokens) == (5100, 3000, 50, 20)


def test_codex_usage_is_an_api_equivalent_estimate() -> None:
    u = from_codex("gpt-5.5", {"input_tokens": 14840, "cached_input_tokens": 6656, "cache_write_input_tokens": 0,
                               "output_tokens": 60, "reasoning_output_tokens": 9})
    assert u.billing == "subscription" and u.cost_basis == "price_table" and u.cost_usd is not None


def test_unknown_model_has_no_cost() -> None:
    assert cost_usd("some-new-model", 1000, 0, 0, 10) is None
    assert from_openai("some-new-model", {"input_tokens": 1}, billing="api").cost_basis == "unknown"


def test_prices_change_on_their_date_and_long_prompts_cost_more() -> None:
    assert cost_usd("gemini-3.8-flash", 1_000_000, 0, 0, 0, day="2026-12-31") == pytest.approx(0.75)
    assert cost_usd("gemini-3.8-flash", 1_000_000, 0, 0, 0, day="2027-01-01") == pytest.approx(1.50)
    assert cost_usd("gpt-6-sol", 300_000, 0, 0, 0) == pytest.approx(300_000 * 2 * 2.0 / 1e6)   # above 272K: 2x input


@pytest.mark.anyio
async def test_codex_adapter_yields_normalized_usage(tmp_path: Path) -> None:
    line = json.dumps({"type": "turn.completed", "usage": {"input_tokens": 100, "cached_input_tokens": 40,
                                                           "output_tokens": 7, "reasoning_output_tokens": 2}})
    proc = await asyncio.create_subprocess_exec(sys.executable, "-c", f"print({line!r}, flush=True)",
                                                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
    rt = CodexRuntime(Path("/unused"), "worker", tmp_path, tmp_path, timeout_s=None)
    run = RunHandle("t", rt.name, "t", {"proc": proc, "violation": None, "seen": set()})
    usage = [e.data async for e in rt.events(run) if e.kind == "usage"][0]
    assert usage["input_tokens"] == 100 and usage["cached_input_tokens"] == 40 and usage["billing"] == "subscription"
```

- [ ] **Step 2: Run them to see them fail**

Run: `uv run pytest tests/test_usage_prices.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'ferb.runtimes.usage'`.

- [ ] **Step 3: Implement `ferb/runtimes/prices.py`**

```python
"""Owns FERB's dated table of provider prices (USD per 1M tokens). Must not guess: an unlisted model has no price.

Checked 2026-10-07 against the official pages:
- OpenAI: https://developers.openai.com/api/docs/pricing and /api/docs/models/<id>; prompts above 272K tokens cost 2x input
  (cached and written too) and 1.5x output for the whole request; GPT-5.6+ cache writes cost 1.25x input.
- Anthropic: https://platform.claude.com/docs/en/about-claude/pricing (5-minute cache writes 1.25x input).
- Google: https://ai.google.dev/gemini-api/docs/pricing (gemini-3.8-flash doubles on 2027-01-01; 3.1-pro-preview above
  200K tokens: 2x input, 1.5x output).
Re-check against a billing statement before relying on totals; update by adding a dated entry, never editing an old one."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Price:
    input: float                     # uncached input
    cached: float                    # read from the provider's cache
    cache_write: float               # written to the cache (equal to input where writes aren't billed extra)
    output: float                    # generated tokens, reasoning included
    long_above: int | None = None    # prompt size above which the long-context factors apply to the whole request
    long_input: float = 1.0          # multiplies input, cached and cache_write
    long_output: float = 1.0
    since: str = "2000-01-01"        # ISO date this price starts


PRICES: dict[str, list[Price]] = {
    "gpt-6-astra": [Price(10.00, 1.00, 12.50, 50.00, 272_000, 2.0, 1.5)],
    "gpt-6.1-sol": [Price(2.00, 0.10, 2.50, 10.00, 272_000, 2.0, 1.5)],
    "gpt-6-sol": [Price(2.00, 0.20, 2.50, 10.00, 272_000, 2.0, 1.5)],
    "gpt-6-luna": [Price(0.10, 0.01, 0.125, 0.50)],
    "gpt-5.5": [Price(5.00, 0.50, 5.00, 30.00, 272_000, 2.0, 1.5)],
    "claude-fable-5-1": [Price(10.00, 0.25, 12.50, 50.00)],
    "claude-opus-5-5": [Price(4.00, 0.20, 5.00, 20.00)],
    "claude-sonnet-5-5": [Price(2.00, 0.20, 2.50, 10.00)],
    "claude-haiku-4-5": [Price(1.00, 0.10, 1.25, 5.00)],
    "claude-haiku-4-5-20251001": [Price(1.00, 0.10, 1.25, 5.00)],
    "gemini-3.8-flash": [Price(0.75, 0.075, 0.75, 3.75), Price(1.50, 0.15, 1.50, 7.50, since="2027-01-01")],
    "gemini-3.1-pro-preview": [Price(2.00, 0.20, 2.00, 12.00, 200_000, 2.0, 1.5)],
}
```

- [ ] **Step 4: Implement `ferb/runtimes/usage.py`**

```python
"""Owns one usage record for every runtime (D32): tokens in, cached, written, out, reasoning, and the cost basis.

Providers count differently: OpenAI and Gemini input includes cached tokens, Anthropic's input_tokens is only what follows
the last cache breakpoint; Gemini's candidates exclude thoughts. Each from_* turns one provider's numbers into the same
meaning. Subscription runtimes (Codex, Claude Agent SDK) report an API-equivalent estimate, labelled as such."""

from dataclasses import dataclass
from datetime import date
from typing import Any

from ferb.runtimes.prices import PRICES


@dataclass(frozen=True)
class Usage:
    model: str
    input_tokens: int                # every prompt token, cached or not
    cached_input_tokens: int         # read from the provider's cache
    cache_write_tokens: int          # written to the cache by this call
    output_tokens: int               # every generated token, reasoning included
    reasoning_tokens: int
    cost_usd: float | None           # None when the model has no listed price
    cost_basis: str                  # "provider" (the runtime reported it) | "price_table" | "unknown"
    billing: str                     # "api" (billed per token) | "subscription" (cost is the API-equivalent estimate)


def cost_usd(model: str, input_tokens: int, cached: int, cache_write: int, output: int, day: str | None = None) -> float | None:
    entries = [p for p in PRICES.get(model, []) if p.since <= (day or date.today().isoformat())]
    if not entries:
        return None
    p = max(entries, key=lambda e: e.since)
    long = p.long_above is not None and input_tokens > p.long_above
    fi, fo = (p.long_input, p.long_output) if long else (1.0, 1.0)
    fresh = max(input_tokens - cached - cache_write, 0)
    return (fresh * p.input * fi + cached * p.cached * fi + cache_write * p.cache_write * fi + output * p.output * fo) / 1e6


def _make(model: str, inp: int, cached: int, write: int, out: int, reasoning: int, billing: str,
          provider_cost: float | None = None) -> Usage:
    if provider_cost is not None:
        return Usage(model, inp, cached, write, out, reasoning, float(provider_cost), "provider", billing)
    cost = cost_usd(model, inp, cached, write, out)
    return Usage(model, inp, cached, write, out, reasoning, cost, "price_table" if cost is not None else "unknown", billing)


def _int(d: dict[str, Any], key: str) -> int:
    return int(d.get(key) or 0)


def from_openai(model: str, usage: dict[str, Any], billing: str) -> Usage:
    details = usage.get("input_tokens_details") or {}
    out = usage.get("output_tokens_details") or {}
    return _make(model, _int(usage, "input_tokens"), _int(details, "cached_tokens"), _int(details, "cache_write_tokens"),
                 _int(usage, "output_tokens"), _int(out, "reasoning_tokens"), billing)


def from_codex(model: str, usage: dict[str, Any]) -> Usage:
    """Codex's turn.completed usage (OpenAI meaning: input includes cached, output includes reasoning)."""
    return _make(model, _int(usage, "input_tokens"), _int(usage, "cached_input_tokens"),
                 _int(usage, "cache_write_input_tokens"), _int(usage, "output_tokens"),
                 _int(usage, "reasoning_output_tokens"), "subscription")


def from_anthropic(model: str, usage: dict[str, Any], billing: str, provider_cost: float | None = None) -> Usage:
    read, write = _int(usage, "cache_read_input_tokens"), _int(usage, "cache_creation_input_tokens")
    return _make(model, _int(usage, "input_tokens") + read + write, read, write, _int(usage, "output_tokens"), 0,
                 billing, provider_cost)


def from_gemini(model: str, meta: dict[str, Any], billing: str) -> Usage:
    thoughts = _int(meta, "thoughtsTokenCount")
    return _make(model, _int(meta, "promptTokenCount") + _int(meta, "toolUsePromptTokenCount"),
                 _int(meta, "cachedContentTokenCount"), 0, _int(meta, "candidatesTokenCount") + thoughts, thoughts, billing)
```

- [ ] **Step 5: Emit normalized usage from the CLI adapters**

In `codex_exec.py` `events`, replace `yield RuntimeEvent("usage", dict(ev.get("usage", {})))` with:

```python
                        yield RuntimeEvent("usage", asdict(from_codex(self.model, dict(ev.get("usage", {})))))
```

In `claude_sdk.py`, replace the `ResultMessage` usage line with:

```python
                yield RuntimeEvent("usage", asdict(from_anthropic(self.model or "claude", dict(m.usage or {}),
                                                                  billing="subscription",
                                                                  provider_cost=m.total_cost_usd)))
```

(`from dataclasses import asdict`; import the `from_*` functions.)

- [ ] **Step 6: The distiller's model calls are traced too**

In `ferb/memory/distill.py`:
- Add a keyword parameter `record_usage: Callable[[str, dict[str, Any]], None] = lambda runtime, data: None` to `Distiller.__init__` and store it.
- In `_model`'s event loop, add `elif e.kind == "usage": self.record_usage(rt.name, dict(e.data))`.

In `ferb/daemon/main.py`, where the `Distiller` is built (`:668`), pass `record_usage=self._record_distill_usage` and add:

```python
    def _record_distill_usage(self, runtime: str, data: dict[str, Any]) -> None:
        span = self.spans.start("model.usage", task_id=None, parent_id=None, input={"runtime": runtime, "role": "distill"})
        self.spans.finish(span, output=data)
```

If `self.spans.start` needs another argument (e.g. `device_id`), copy how `worker._run` starts its `model.usage` span.

- [ ] **Step 7: Run the tests**

Run: `uv run pytest tests/test_usage_prices.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS. If a test asserted the old raw usage keys, update it to the normalized keys and list it in the report.

- [ ] **Step 8: Commit**

```bash
git add ferb/runtimes/usage.py ferb/runtimes/prices.py ferb/runtimes/codex_exec.py ferb/runtimes/claude_sdk.py ferb/memory/distill.py ferb/daemon/main.py tests/test_usage_prices.py
git commit -m "F2 (D32): one usage record for every runtime, priced from a dated table

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Capability table, per-role selection and `ferb runtimes`

**Files:**
- Modify: `ferb/core/types.py` (`Capabilities` replaces `RuntimeFeatures`)
- Modify: `ferb/core/interfaces.py` (`AgentRuntime.capabilities`)
- Modify: `ferb/runtimes/codex_exec.py`, `ferb/runtimes/claude_sdk.py`, `tests/conftest.py` (`FakeRuntime`)
- Create: `ferb/runtimes/catalog.py`, `ferb/cli/runtimes.py`
- Modify: `ferb/daemon/main.py:1551-1569` (`_make_runtime` → `_runtime_for` + `_build_runtime`; checker role; models)
- Modify: `ferb/orchestrator/worker.py:130` (verifier runs ask for the `checker` role)
- Modify: `ferb/cli/__init__.py` (the `runtimes` command)
- Test: `tests/test_runtime_selection.py`

**Interfaces:**
- Consumes: `Usage` (Task 3), for nothing beyond imports.
- Produces:
  - `Capabilities(tools, cache, cache_key, structured_output, reasoning_effort, image_input, continuation, live_messaging, billing, native_tools_contained=True)`.
  - `RuntimeInfo(name, default_model, capabilities: Callable[[str], Capabilities], available: Callable[[Path], tuple[bool, str]])`, and `CATALOG: dict[str, RuntimeInfo]`.
  - `ROLES = ("orchestrator", "worker", "checker")`.
  - Daemon: `_runtime_for(role) -> str`, `_model_for(name) -> str | None`, `_build_runtime(name, role, *, mode, tool_names, schema)`.
  - Config: `[runtimes] orchestrator|worker|checker = "<runtime>"` and `[models] <runtime> = "<model>"` (`runtimes.claude_model` still read for Claude).

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_runtime_selection.py
"""A capability table per runtime; the orchestrator, worker and checker each pick their runtime and model (D32)."""
import io
from pathlib import Path

from ferb.cli import runtimes as runtimes_cli
from ferb.config import Config
from ferb.core.types import Capabilities
from ferb.runtimes.catalog import CATALOG, ROLES
from tests.conftest import RunningDaemon


def test_every_catalog_runtime_declares_its_capabilities() -> None:
    for name, info in CATALOG.items():
        caps = info.capabilities(info.default_model)
        assert isinstance(caps, Capabilities) and caps.native_tools_contained, name


def test_checker_role_falls_back_to_the_worker_runtime(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    d.config.set("runtimes", "worker", "claude")
    assert d._runtime_for("checker") == "claude"
    d.config.set("runtimes", "checker", "codex")
    assert d._runtime_for("checker") == "codex" and d._runtime_for("worker:default") == "claude"
    assert d._runtime_for("worker:codex") == "codex"            # a planner-chosen worker type still wins


def test_model_comes_from_config(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    assert d._model_for("codex") is None
    d.config.set("models", "codex", "gpt-6-sol")
    assert d._model_for("codex") == "gpt-6-sol"


def test_verifier_runs_ask_for_the_checker_role(daemon: RunningDaemon) -> None:
    from tests.conftest import chat, done, install, sup, task
    roles: list[str] = []
    install(daemon, [sup("task", "Checking.", task("check a file"))], {"worker": done("checked")})
    inner = daemon.daemon.supervisor.make_runtime

    def spy(role, **kw):
        roles.append(role)
        return inner(role, **kw)
    daemon.daemon.supervisor.make_runtime = spy
    chat(daemon, "check a file")
    assert "checker" in roles and "worker:default" in roles


def test_runtimes_use_writes_config_and_show_lists_roles(tmp_path: Path) -> None:
    home = tmp_path / "home"
    out = io.StringIO()
    assert runtimes_cli.use(home, "orchestrator", "claude", None, out) == 0
    assert Config(home).get("runtimes", "orchestrator") == "claude"
    assert runtimes_cli.use(home, "planner", "claude", None, out) == 1         # unknown role
    assert runtimes_cli.use(home, "worker", "nope", None, out) == 1           # unknown runtime
    out = io.StringIO()
    assert runtimes_cli.show(home, out) == 0
    text = out.getvalue()
    assert all(role in text for role in ROLES) and "codex" in text and "claude" in text
```

`test_verifier_runs_ask_for_the_checker_role` relies on the scripted flow: `task(...)` gives one success condition, the worker reports `done`, and `Script.step` answers the `.verify` run itself. That is the path most lifecycle tests already take.

- [ ] **Step 2: Run them to see them fail**

Run: `uv run pytest tests/test_runtime_selection.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'ferb.runtimes.catalog'`.

- [ ] **Step 3: `Capabilities` replaces `RuntimeFeatures` in `ferb/core/types.py`**

```python
@dataclass(frozen=True)
class Capabilities:
    """What a model runtime supports (D32's capability table): facts for planning and for `ferb runtimes`."""
    tools: Literal["direct", "deferred", "none"]      # how FERB's gateway tools reach the model
    cache: Literal["prefix", "markers"]               # automatic on an identical start, or FERB marks the cacheable prefix
    cache_key: bool                                   # FERB keeps one cache-routing key per role
    structured_output: Literal["native", "tool"]      # the provider enforces the schema, or a final-answer tool carries it
    reasoning_effort: bool
    image_input: bool                                 # image tool results reach the model
    continuation: bool
    live_messaging: bool
    billing: Literal["subscription", "api"]
    native_tools_contained: bool = True               # must be True to be registered
```

Run `grep -rn RuntimeFeatures ferb tests` and replace every use. `AgentRuntime` gets `capabilities: Capabilities` instead of `features`.
- `CodexRuntime.capabilities = Capabilities("direct", "prefix", False, "native", True, <image>, False, False, "subscription")`.
- `ClaudeRuntime.capabilities = Capabilities("direct", "prefix", False, "native", False, True, True, True, "subscription")`.
- `FakeRuntime.capabilities = Capabilities("direct", "prefix", False, "native", False, False, False, False, "api")`.

For Codex's `<image>`, use `True` only if a test, spike or trace shows an MCP image result reaching Codex: run `grep -rn "ImageContent\|image" spikes/*/README* spikes/*/*.md tests/test_real_runtimes.py`. Otherwise use `False` with a comment "unverified".

- [ ] **Step 4: Create `ferb/runtimes/catalog.py`**

```python
"""Owns the list of model runtimes FERB can run and their capability facts (D32). Must not choose a runtime for a task.

Each entry: its default model, its capabilities for a model, and whether it can run on this Mac (with the fix if not).
The direct-API runtimes join in Task 10 of the F2 plan."""

from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from ferb.core.types import Capabilities

ROLES = ("orchestrator", "worker", "checker")


@dataclass(frozen=True)
class RuntimeInfo:
    name: str
    default_model: str
    capabilities: Callable[[str], Capabilities]
    available: Callable[[Path], tuple[bool, str]]     # (usable on this Mac, how to fix when not)


def _codex_available(home: Path) -> tuple[bool, str]:
    from ferb.core.errors import Unsupported
    from ferb.runtimes.codex_exec import codex_binary, codex_home
    try:
        codex_binary(home)
    except Unsupported:
        return False, "run `ferb setup --update`"
    return (True, "") if codex_home(home) is not None else (False, "run `ferb setup --codex-login`")


def _codex_caps(model: str) -> Capabilities:
    from ferb.runtimes.codex_exec import CodexRuntime
    return CodexRuntime.capabilities


def _claude_caps(model: str) -> Capabilities:
    from ferb.runtimes.claude_sdk import ClaudeRuntime
    return ClaudeRuntime.capabilities


CATALOG: dict[str, RuntimeInfo] = {
    "codex": RuntimeInfo("codex", "gpt-5.5", _codex_caps, _codex_available),
    "claude": RuntimeInfo("claude", "", _claude_caps, lambda home: (True, "")),
}
```

`codex_binary(home)` without a repo checks the installed `~/.ferb/node`, which is what the daemon uses. `MODEL` in `codex_exec.py` stays `"gpt-5.5"` and must equal the catalog default; add `assert CATALOG["codex"].default_model == MODEL` to the test file.

- [ ] **Step 5: Split `_make_runtime` in `ferb/daemon/main.py`**

Replace the body of `_make_runtime` with a lookup and a build:

```python
    def _runtime_for(self, role: str) -> str:
        """The runtime a role uses: a planner-chosen worker type, else [runtimes] orchestrator / worker / checker."""
        if role.startswith("worker:") and role != "worker:default":
            return role.partition(":")[2]
        if role == "orchestrator":
            return str(self.config.get("runtimes", "orchestrator", "codex"))
        worker = str(self.config.get("runtimes", "worker", "codex"))
        return str(self.config.get("runtimes", "checker", worker)) if role == "checker" else worker

    def _model_for(self, name: str) -> str | None:
        model = self.config.get("models", name) or (self.config.get("runtimes", "claude_model") if name == "claude" else None)
        return str(model) if model else None

    def _make_runtime(self, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any]) -> Any:
        return self._build_runtime(self._runtime_for(role), role, mode=mode, tool_names=tool_names, schema=schema)

    def _build_runtime(self, name: str, role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any]) -> Any:
        if name not in self.registry.runtimes:
            raise FerbError(f"unknown runtime {name!r} in config.toml [runtimes]")
        model = self._model_for(name)
        shim = [sys.executable, "-m", "ferb.cli.mcp"]
        if name == "codex":
            from ferb.device.install import repo_root
            from ferb.runtimes.codex_exec import MODEL, CodexRuntime, codex_binary
            workdir = self.home / "workspace" if mode == "worker" else self.home / "run" / "supervisor"
            return CodexRuntime(codex_binary(self.home, repo_root()), mode, workdir, self.home,  # type: ignore[arg-type]
                                shim_cmd=shim, tool_names=tool_names, output_schema=schema, model=model or MODEL,
                                timeout_s=None if mode == 'worker' else 900)
        from ferb.runtimes.claude_sdk import ClaudeRuntime
        if self._proxy is None:
            self._proxy = ToolListProxy()
        return ClaudeRuntime(mode, self._proxy, str(self.home), shim_cmd=shim, tool_names=tool_names,  # type: ignore[arg-type]
                             output_schema=schema, model=model)
```

In `ferb/orchestrator/worker.py:130`, ask for the checker role on verifier runs:

```python
            role = "checker" if task.role == "verifier" else "worker:" + tree.spec.worker_type
            rt = self.make_runtime(role, mode="worker", tool_names=tools, schema=schema)
```

- [ ] **Step 6: Create `ferb/cli/runtimes.py` and wire the command**

```python
"""`ferb runtimes`: the capability table, what each role uses, and switching a role's runtime or model (D32).
Reads and writes config.toml directly (the daemon reads it on every call); no daemon needed."""

from pathlib import Path
from typing import TextIO

from ferb.config import Config
from ferb.runtimes.catalog import CATALOG, ROLES


def show(home: Path, out: TextIO) -> int:
    cfg = Config(home)
    print(f"{'runtime':<10}{'usable':<8}{'model':<20}{'tools':<9}{'cache':<9}{'key':<5}{'typed':<8}{'effort':<8}"
          f"{'images':<8}billing", file=out)
    for name, info in CATALOG.items():
        model = str(cfg.get("models", name) or info.default_model or "(default)")
        ok, fix = info.available(home)
        c = info.capabilities(model)
        print(f"{name:<10}{'yes' if ok else 'no':<8}{model:<20}{c.tools:<9}{c.cache:<9}{'yes' if c.cache_key else 'no':<5}"
              f"{c.structured_output:<8}{'yes' if c.reasoning_effort else 'no':<8}{'yes' if c.image_input else 'no':<8}"
              f"{c.billing}" + (f"   ({fix})" if fix else ""), file=out)
    worker = str(cfg.get("runtimes", "worker", "codex"))
    uses = {"orchestrator": str(cfg.get("runtimes", "orchestrator", "codex")), "worker": worker,
            "checker": str(cfg.get("runtimes", "checker", worker))}
    print("\n" + "\n".join(f"{role}: {uses[role]}" for role in ROLES), file=out)
    return 0


def use(home: Path, role: str, runtime: str, model: str | None, out: TextIO) -> int:
    if role not in ROLES:
        print(f"Unknown role {role!r}; roles: {', '.join(ROLES)}.", file=out)
        return 1
    if runtime not in CATALOG:
        print(f"Unknown runtime {runtime!r}; runtimes: {', '.join(CATALOG)}.", file=out)
        return 1
    cfg = Config(home)
    cfg.set("runtimes", role, runtime)
    if model:
        cfg.set("models", runtime, model)
    print(f"The {role} now uses {runtime}" + (f" with {model}" if model else "") + " from its next call.", file=out)
    return 0
```

In `ferb/cli/__init__.py`, add a `runtimes` subparser following the `memory` pattern:
- `ferb runtimes` shows the table.
- `ferb runtimes use ROLE RUNTIME [--model MODEL]` switches a role.

Dispatch both to `runtimes_cli.show(ferb_home(), sys.stdout)` / `runtimes_cli.use(ferb_home(), args.role, args.runtime, args.model, sys.stdout)` before any daemon connection is made; neither needs the daemon. `ferb_home` comes from `ferb.paths`.

- [ ] **Step 7: Run the tests**

Run: `uv run pytest tests/test_runtime_selection.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add ferb/core/types.py ferb/core/interfaces.py ferb/runtimes/catalog.py ferb/runtimes/codex_exec.py ferb/runtimes/claude_sdk.py ferb/daemon/main.py ferb/orchestrator/worker.py ferb/cli/runtimes.py ferb/cli/__init__.py tests/conftest.py tests/test_runtime_selection.py
git commit -m "F2 (D32): capability table, per-role runtime and model selection, ferb runtimes

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The Codex adapter keeps the owner's personal setup out

**Files:**
- Modify: `ferb/runtimes/codex_exec.py` (`DISABLED`, `start` env)
- Test: `tests/test_codex_isolation.py`

**Interfaces:**
- Produces: `CODEX_USER = "codex-user"`, an owner-only empty directory under FERB's home used as the Codex process's `HOME`.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_codex_isolation.py
"""Codex runs with its own HOME so the owner's ~/.agents/skills never load (D32 note; verified live 2026-10-07)."""
import asyncio
import stat
from pathlib import Path

import pytest

import ferb.runtimes.codex_exec as cx
from ferb.core.types import Identity, ModelRequest, Task


def test_skill_dependency_install_is_disabled() -> None:
    assert "skill_mcp_dependency_install" in cx.DISABLED


@pytest.mark.anyio
async def test_codex_process_gets_ferbs_own_home(tmp_path: Path, monkeypatch) -> None:
    seen: dict = {}

    async def fake_exec(*cmd, **kw):
        seen.update(kw)
        raise RuntimeError("stop before running anything")
    async def no_version(binary):
        return None
    monkeypatch.setattr(cx, "check_version", no_version)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", fake_exec)
    rt = cx.CodexRuntime(Path("/x/codex"), "supervisor", tmp_path / "work", tmp_path)
    task = Task("t", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
    with pytest.raises(RuntimeError):
        await rt.start(task, ModelRequest("sys", ("ctx",), 0), "")
    home = Path(seen["env"]["HOME"])
    assert home == tmp_path / cx.CODEX_USER and home.is_dir()
    assert stat.S_IMODE(home.stat().st_mode) == 0o700 and not any(home.iterdir())
```

- [ ] **Step 2: Run them to see them fail**

Run: `uv run pytest tests/test_codex_isolation.py -q`
Expected: FAIL. `skill_mcp_dependency_install` is not in `DISABLED`, and `CODEX_USER` doesn't exist.

- [ ] **Step 3: Implement**

In `ferb/runtimes/codex_exec.py`, append `"skill_mcp_dependency_install"` to `DISABLED` (Codex 0.157.1 has it switched on: it installs MCP dependencies for skills). Add, below `SUPERVISOR_SETTINGS`:

```python
# Codex finds the owner's personal skills through HOME (~/.agents/skills), even with skill_search disabled and its own
# CODEX_HOME. An empty, owner-only HOME of its own keeps them out (verified 2026-10-07: no skill loads, auth unaffected,
# because auth lives in CODEX_HOME). FERB's MCP shim is unaffected: it gets FERB_HOME explicitly.
CODEX_USER = "codex-user"
```

In `start`, after `env = {**os.environ, "RUST_LOG": "trace"}`, add:

```python
            user_home = self.ferb_home / CODEX_USER
            user_home.mkdir(mode=0o700, parents=True, exist_ok=True)
            env["HOME"] = str(user_home)
```

Update the module docstring's containment paragraph: one sentence saying Codex runs with its own empty HOME and why.

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_codex_isolation.py tests/test_unit1_speed_voice.py -q`, then `uv run pytest -q`.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ferb/runtimes/codex_exec.py tests/test_codex_isolation.py
git commit -m "F2 (D32): Codex runs with its own HOME, so the owner's personal skills never load

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

The controller re-runs the live probe afterwards: no `failed to load skill` and no owner skill names in Codex's trace.

---

### Task 6: The direct-API runtime core and Keychain keys

**Files:**
- Create: `ferb/runtimes/keys.py`, `ferb/runtimes/api.py`
- Create: `tests/support/runtime_fakes.py`
- Test: `tests/test_api_keys.py`, `tests/test_api_runtime.py`

**Interfaces:**
- Consumes:
  - `ModelRequest` (Task 1), `Usage` (Task 3), `Capabilities` (Task 4).
  - `mcp_name` from `ferb.cli.mcp`.
  - `Unsupported` from `ferb.core.errors`.
- Produces:
  - `keys.PROVIDERS = ("anthropic", "openai", "google")`.
  - `keys.account(provider) -> str`, `async keys.api_key(provider) -> str | None`, `keys.has_api_key(provider) -> bool`.
  - In `api.py`:
    - `ToolSpec(name, description, schema)`, `ToolCall(id, name, arguments, arguments_error="")`, `ToolOutcome(call, text, is_error, image=None)`, `Turn(calls, final, usage, stop="")`.
    - `ProviderError(Exception)`.
    - The `Provider` protocol: `name`, `model`, `capabilities`, `begin(request, tools, schema, effort, cache_key) -> dict`, `http(state) -> (url, headers, body)`, `auth(key) -> dict`, `parse(state, data) -> Turn`, `add_results(state, turn, results) -> None`.
    - The `Bridge` protocol: `tools(run_token) -> list[dict]`, `async call(run_token, name, arguments) -> dict`.
    - `GatewayBridge(gateway, tokens)`.
    - `inline_refs(schema) -> dict`.
    - `ApiRuntime(provider, *, mode, role, tool_names, output_schema, bridge, key, transport=None, sleep=asyncio.sleep, retries=4, backoff_s=1.0, timeout_s=600.0)`.
  - In tests: `FakeBridge`, `Scripted`, `FakeProvider` in `tests/support/runtime_fakes.py`.

- [ ] **Step 1: Write the shared test fakes**

```python
# tests/support/runtime_fakes.py
"""Test doubles for the direct-API runtimes: a scripted HTTP transport, FERB's gateway as a bridge, a minimal provider."""
import asyncio
import json
from typing import Any

import httpx

from ferb.core.types import Capabilities
from ferb.runtimes.api import ProviderError, ToolCall, Turn
from ferb.runtimes.usage import Usage

FS_LIST = {"name": "fs.list", "description": "List a folder.",
           "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}}


class FakeBridge:
    def __init__(self, tools: list[dict[str, Any]] | None = None, result: dict[str, Any] | None = None) -> None:
        self._tools = tools if tools is not None else [FS_LIST]
        self.result = result or {"content": {"entries": ["a.txt"]}, "footer": "", "is_error": False, "image": None}
        self.calls: list[tuple[str, dict[str, Any]]] = []

    def tools(self, run_token: str) -> list[dict[str, Any]]:
        return self._tools

    async def call(self, run_token: str, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        self.calls.append((name, arguments))
        return self.result


class Scripted:
    """Replays (status, json body) responses in order; records each request's headers and JSON body.
    A response of None blocks until the test cancels the run."""
    def __init__(self, responses: list[tuple[int, Any] | None]) -> None:
        self.responses = list(responses)
        self.requests: list[tuple[dict[str, str], dict[str, Any]]] = []

    async def _handle(self, request: httpx.Request) -> httpx.Response:
        self.requests.append((dict(request.headers), json.loads(request.content)))
        item = self.responses.pop(0)
        if item is None:
            await asyncio.Event().wait()
        status, body = item  # type: ignore[misc]
        return httpx.Response(status, json=body)

    def transport(self) -> httpx.MockTransport:
        return httpx.MockTransport(self._handle)


class FakeProvider:
    """A provider whose wire format is FERB-made: {"calls": [[id, name, args]], "text": ..., "usage": n}."""
    name = "fake"
    model = "fake-model"
    capabilities = Capabilities("direct", "prefix", True, "native", True, True, False, False, "api")

    def begin(self, request, tools, schema, effort, cache_key) -> dict[str, Any]:
        return {"body": {"system": request.system, "parts": list(request.parts), "cacheable": request.cacheable,
                         "tools": [t.name for t in tools], "schema": schema, "effort": effort, "cache_key": cache_key,
                         "results": []}}

    def http(self, state):
        return "https://provider.test/v1", {"content-type": "application/json"}, state["body"]

    def auth(self, key: str) -> dict[str, str]:
        return {"x-key": key}

    def parse(self, state, data) -> Turn:
        if data.get("cut"):
            raise ProviderError("fake: the reply was cut off at the output limit")
        calls = [ToolCall(c[0], c[1], c[2] if isinstance(c[2], dict) else {},
                          "" if isinstance(c[2], dict) else "not a JSON object") for c in data.get("calls", [])]
        return Turn(calls, None if calls else data.get("text"),
                    Usage(self.model, data.get("usage", 1), 0, 0, 1, 0, None, "unknown", "api"))

    def add_results(self, state, turn, results) -> None:
        state["body"]["results"].append([[r.call.id, r.text, r.is_error, r.image] for r in results])
```

- [ ] **Step 2: Write the failing tests**

```python
# tests/test_api_keys.py
"""Provider API keys live in the Keychain (service ferb, account api/<provider>) and nowhere else (D32, rule 14)."""
import os
import stat
from pathlib import Path

import pytest

from ferb.runtimes import keys


@pytest.fixture
def fake_security(tmp_path: Path, monkeypatch):
    script = tmp_path / "security"
    script.write_text("#!/bin/sh\n"
                      'if [ "$5" = "api/openai" ]; then [ "$6" = "-w" ] && echo "sk-test-123"; exit 0; fi\n'
                      "exit 44\n")
    script.chmod(script.stat().st_mode | stat.S_IEXEC)
    monkeypatch.setenv("PATH", f"{tmp_path}{os.pathsep}{os.environ['PATH']}")


@pytest.mark.anyio
async def test_reads_the_key_from_the_keychain_account(fake_security) -> None:
    assert await keys.api_key("openai") == "sk-test-123"
    assert await keys.api_key("anthropic") is None


def test_presence_check_never_reveals(fake_security) -> None:
    assert keys.has_api_key("openai") and not keys.has_api_key("google")


def test_unknown_provider_is_refused() -> None:
    with pytest.raises(ValueError):
        keys.account("someone")
```

```python
# tests/test_api_runtime.py
"""The direct-API runtime: tools only through FERB's gateway, retries, errors without the key, cancellation (D32)."""
import asyncio
import json

import httpx
import pytest

from ferb.core.errors import Unsupported
from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.api import ApiRuntime, inline_refs
from tests.support.runtime_fakes import FakeBridge, FakeProvider, Scripted

TASK = Task("t.worker", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
REQ = ModelRequest("instructions", ("facts", "history", "message"), 2)


def runtime(script: Scripted, *, mode="worker", bridge=None, key="sk-secret", sleeps=None, **kw) -> ApiRuntime:
    async def key_source(provider):
        return key
    async def sleep(s):
        (sleeps if sleeps is not None else []).append(s)
    return ApiRuntime(FakeProvider(), mode=mode, role="worker", tool_names=["fs.list"], output_schema=None,
                      bridge=bridge or FakeBridge(), key=key_source, transport=script.transport(), sleep=sleep, **kw)


async def run_all(rt, request=REQ, token="tok") -> list:
    run = await rt.start(TASK, request, token)
    return [e async for e in rt.events(run)]


@pytest.mark.anyio
async def test_tool_call_goes_through_the_bridge_and_back() -> None:
    script = Scripted([(200, {"calls": [["c1", "fs_list", {"path": "/tmp"}]]}), (200, {"text": "done"})])
    bridge = FakeBridge()
    events = await run_all(runtime(script, bridge=bridge))
    assert [e.kind for e in events] == ["usage", "tool_call", "usage", "final"]
    assert bridge.calls == [("fs.list", {"path": "/tmp"})]                    # mcp name mapped back to the capability
    assert script.requests[1][1]["results"][0][0][:3] == ["c1", '{"entries": ["a.txt"]}\n', False]
    assert events[-1].data == {"text": "done"}                                # no tools_seen: tools are never deferred


@pytest.mark.anyio
async def test_request_carries_the_cacheable_prefix_and_role_key() -> None:
    script = Scripted([(200, {"text": "ok"})])
    await run_all(runtime(script, mode="supervisor"), token="")
    headers, body = script.requests[0]
    assert body["parts"][:3] == ["facts", "history", "message"] and body["cacheable"] == 2
    assert body["cache_key"] == "ferb:worker:supervisor" and body["effort"] == "low" and body["tools"] == []
    assert headers["x-key"] == "sk-secret"


@pytest.mark.anyio
async def test_unoffered_tool_is_a_violation() -> None:
    script = Scripted([(200, {"calls": [["c1", "shell_run", {"command": "id"}]]})])
    bridge = FakeBridge()
    events = await run_all(runtime(script, bridge=bridge))
    assert events[-1].kind == "violation" and bridge.calls == [] and len(script.requests) == 1


@pytest.mark.anyio
async def test_bad_arguments_never_reach_the_gateway() -> None:
    script = Scripted([(200, {"calls": [["c1", "fs_list", "not an object"]]}), (200, {"text": "ok"})])
    bridge = FakeBridge()
    events = await run_all(runtime(script, bridge=bridge))
    assert bridge.calls == [] and events[-1].kind == "final"
    result = script.requests[1][1]["results"][0][0]
    assert result[2] is True and "not a JSON object" in result[1]


@pytest.mark.anyio
async def test_retries_then_succeeds() -> None:
    sleeps: list[float] = []
    script = Scripted([(429, {"error": {"message": "slow down"}}), (503, {}), (200, {"text": "ok"})])
    events = await run_all(runtime(script, sleeps=sleeps))
    assert events[-1].kind == "final" and sleeps == [1.0, 2.0]


@pytest.mark.anyio
async def test_network_failure_after_retries_is_an_error() -> None:
    async def boom(request):
        raise httpx.ConnectError("down")
    rt = runtime(Scripted([]), retries=2)
    rt.transport = httpx.MockTransport(boom)
    events = await run_all(rt)
    assert events[-1].kind == "error" and "network error" in events[-1].data["message"]


@pytest.mark.anyio
async def test_errors_name_the_status_never_the_key() -> None:
    script = Scripted([(401, {"error": {"message": "bad key sk-secret was rejected"}})])
    events = await run_all(runtime(script))
    message = events[-1].data["message"]
    assert events[-1].kind == "error" and "401" in message and "sk-secret" not in message


@pytest.mark.anyio
async def test_cut_off_reply_is_an_error() -> None:
    events = await run_all(runtime(Scripted([(200, {"cut": True})])))
    assert events[-1].kind == "error" and "cut off" in events[-1].data["message"]


@pytest.mark.anyio
async def test_missing_key_names_the_fix() -> None:
    with pytest.raises(Unsupported, match="ferb setup --key fake"):
        await runtime(Scripted([]), key=None).start(TASK, REQ, "tok")


@pytest.mark.anyio
async def test_cancel_stops_a_slow_call() -> None:
    rt = runtime(Scripted([None]))
    run = await rt.start(TASK, REQ, "tok")
    collected: list = []
    async def consume():
        async for e in rt.events(run):
            collected.append(e)
    consumer = asyncio.create_task(consume())
    await asyncio.sleep(0.05)
    await rt.cancel(run)
    await asyncio.wait_for(consumer, 2)
    assert not any(e.kind == "final" for e in collected)


def test_inline_refs_resolves_local_definitions() -> None:
    schema = {"type": "object", "properties": {"a": {"$ref": "#/$defs/A"}}, "$defs": {"A": {"type": "string"}}}
    assert inline_refs(schema) == {"type": "object", "properties": {"a": {"type": "string"}}}
```

- [ ] **Step 3: Run them to see them fail**

Run: `uv run pytest tests/test_api_keys.py tests/test_api_runtime.py -q`
Expected: FAIL with `ModuleNotFoundError` for `ferb.runtimes.keys` / `ferb.runtimes.api`.

- [ ] **Step 4: Implement `ferb/runtimes/keys.py`**

```python
"""Owns the provider API keys FERB keeps in the Keychain (D32). Must not log, store or return them anywhere else.

Service "ferb", account "api/<provider>". `ferb setup --key <provider>` stores one (the `security` tool prompts for it, so
it is never an argument); runtimes read it at call time and the daemon adds it to its redactor."""

import asyncio
import subprocess

from ferb.identity.secrets import SERVICE

PROVIDERS = ("anthropic", "openai", "google")


def account(provider: str) -> str:
    if provider not in PROVIDERS:
        raise ValueError(f"unknown provider {provider!r}")
    return f"api/{provider}"


async def api_key(provider: str) -> str | None:
    proc = await asyncio.create_subprocess_exec("security", "find-generic-password", "-s", SERVICE, "-a",
                                                account(provider), "-w", stdout=asyncio.subprocess.PIPE,
                                                stderr=asyncio.subprocess.DEVNULL)
    out, _ = await proc.communicate()
    key = out.decode().strip() if proc.returncode == 0 else ""
    return key or None


def has_api_key(provider: str) -> bool:
    """Presence only: no -w, so the value is never read."""
    return subprocess.run(["security", "find-generic-password", "-s", SERVICE, "-a", account(provider)],
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0
```

- [ ] **Step 5: Implement `ferb/runtimes/api.py`**

```python
"""Owns FERB's own tool loop for direct provider APIs (D32). Must not choose targets, authorize actions or offer tools the
run token doesn't cover.

One loop for every provider: ask the model, run each tool call through FERB's gateway (GatewayBridge, the same path as the
MCP shim), send the results back, until the model answers. A small Provider class per API translates requests, responses,
cache markers, typed output and usage. A tool the run wasn't offered stops the run (violation), as with Codex and Claude.
No turn or token budget; stop and the stall watchdog end a run. Keys come from the Keychain at start and never appear in
errors. Provider-hosted tools are never enabled."""

import asyncio
import json
import secrets
from dataclasses import asdict, dataclass
from typing import Any, AsyncIterator, Awaitable, Callable, Literal, Protocol

import httpx

from ferb.cli.mcp import mcp_name
from ferb.core.errors import FerbError, Unsupported
from ferb.core.types import Capabilities, ModelRequest, RunHandle, RuntimeEvent, Task
from ferb.runtimes.usage import Usage

RETRY_STATUS = frozenset({408, 409, 429, 500, 502, 503, 504, 529})


class ProviderError(Exception):
    """A provider failure, worded without the key or the request."""


@dataclass(frozen=True)
class ToolSpec:
    name: str                        # as the model sees it (dots become underscores)
    description: str
    schema: dict[str, Any]


@dataclass(frozen=True)
class ToolCall:
    id: str
    name: str
    arguments: dict[str, Any]
    arguments_error: str = ""        # set when the model's arguments weren't a JSON object


@dataclass(frozen=True)
class ToolOutcome:
    call: ToolCall
    text: str
    is_error: bool
    image: dict[str, str] | None = None     # {"data": base64, "mime": "image/png"}


@dataclass(frozen=True)
class Turn:
    calls: list[ToolCall]
    final: str | None
    usage: Usage
    stop: str = ""


class Provider(Protocol):
    name: str
    model: str
    capabilities: Capabilities
    def begin(self, request: ModelRequest, tools: list[ToolSpec], schema: dict[str, Any] | None, effort: str | None,
              cache_key: str) -> dict[str, Any]: ...
    def http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]: ...
    def auth(self, key: str) -> dict[str, str]: ...
    def parse(self, state: dict[str, Any], data: dict[str, Any]) -> Turn: ...
    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None: ...


class Bridge(Protocol):
    def tools(self, run_token: str) -> list[dict[str, Any]]: ...
    async def call(self, run_token: str, name: str, arguments: dict[str, Any]) -> dict[str, Any]: ...


class GatewayBridge:
    """FERB's gateway in-process: the tools a run token covers, and each call validated and traced by the gateway."""
    def __init__(self, gateway: Any, tokens: Any) -> None:
        self.gateway, self.tokens = gateway, tokens

    def tools(self, run_token: str) -> list[dict[str, Any]]:
        return self.gateway.tools_for(self.tokens.validate(run_token))  # type: ignore[no-any-return]

    async def call(self, run_token: str, name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        return await self.gateway.call(self.tokens.validate(run_token), name, arguments)  # type: ignore[no-any-return]


def inline_refs(schema: dict[str, Any]) -> dict[str, Any]:
    """The schema with local #/$defs references written out (not every provider accepts $defs). A reference to a
    definition already being expanded becomes a plain object, so recursive models can't loop."""
    defs = schema.get("$defs", {})

    def walk(node: Any, stack: tuple[str, ...]) -> Any:
        if isinstance(node, dict):
            ref = node.get("$ref")
            if isinstance(ref, str) and ref.startswith("#/$defs/"):
                name = ref.removeprefix("#/$defs/")
                if name in stack or name not in defs:
                    return {"type": "object"}
                return walk(defs[name], stack + (name,))
            return {k: walk(v, stack) for k, v in node.items() if k != "$defs"}
        if isinstance(node, list):
            return [walk(v, stack) for v in node]
        return node
    return walk(schema, ())  # type: ignore[no-any-return]


@dataclass
class _Run:
    state: dict[str, Any]
    key: str
    token: str
    names: dict[str, str]                       # model-visible tool name -> capability name
    client: httpx.AsyncClient
    cancelled: bool = False
    inflight: "asyncio.Future[Turn] | None" = None


class ApiRuntime:
    def __init__(self, provider: Provider, *, mode: Literal["supervisor", "worker"], role: str,
                 tool_names: list[str] | None, output_schema: dict[str, Any] | None, bridge: Bridge | None,
                 key: Callable[[str], Awaitable[str | None]], transport: httpx.AsyncBaseTransport | None = None,
                 sleep: Callable[[float], Awaitable[None]] = asyncio.sleep, retries: int = 4, backoff_s: float = 1.0,
                 timeout_s: float = 600.0) -> None:
        self.provider, self.name, self.capabilities = provider, provider.name, provider.capabilities
        self.mode, self.role, self.tool_names, self.output_schema = mode, role, list(tool_names or []), output_schema
        self.bridge, self.key, self.transport, self.sleep = bridge, key, transport, sleep
        self.retries, self.backoff_s, self.timeout_s = retries, backoff_s, timeout_s

    async def start(self, task: Task, request: ModelRequest, run_token: str) -> RunHandle:
        key = await self.key(self.name)
        if not key:
            raise Unsupported(f"no {self.name} API key in the Keychain: run `ferb setup --key {self.name}`")
        tools: list[ToolSpec] = []
        names: dict[str, str] = {}
        if self.mode == "worker" and self.bridge is not None and self.tool_names:
            offered = {t["name"]: t for t in self.bridge.tools(run_token)}
            for capability in self.tool_names:
                if capability in offered:
                    t = offered[capability]
                    tools.append(ToolSpec(mcp_name(capability), str(t["description"]), inline_refs(t["input_schema"])))
                    names[mcp_name(capability)] = capability
        state = self.provider.begin(request, tools, self.output_schema, "low" if self.mode == "supervisor" else None,
                                    f"ferb:{self.role.partition(':')[0]}:{self.mode}")
        client = httpx.AsyncClient(timeout=self.timeout_s, transport=self.transport)
        return RunHandle("run-" + secrets.token_hex(6), self.name, task.id,
                         {"run": _Run(state, key, run_token, names, client)})

    async def events(self, run: RunHandle) -> AsyncIterator[RuntimeEvent]:
        r: _Run = run.state["run"]
        try:
            while True:
                r.inflight = asyncio.ensure_future(self._call(r))
                try:
                    turn = await r.inflight
                except asyncio.CancelledError:
                    if r.cancelled:
                        return
                    raise
                except ProviderError as e:
                    yield RuntimeEvent("error", {"message": str(e)})
                    return
                yield RuntimeEvent("usage", asdict(turn.usage))
                if not turn.calls:
                    if not turn.final:
                        yield RuntimeEvent("error", {"message": f"{self.name}: the model ended without an answer"})
                        return
                    yield RuntimeEvent("final", {"text": turn.final})
                    return
                results = []
                for call in turn.calls:
                    capability = r.names.get(call.name)
                    if capability is None:
                        yield RuntimeEvent("violation", {"unexpected_tools": [call.name]})
                        return
                    yield RuntimeEvent("tool_call", {"tool": call.name, "arguments": call.arguments})
                    results.append(await self._execute(r, capability, call))
                self.provider.add_results(r.state, turn, results)
        finally:
            await r.client.aclose()

    async def _execute(self, r: _Run, capability: str, call: ToolCall) -> ToolOutcome:
        if call.arguments_error:
            return ToolOutcome(call, f"FERB refused: the tool arguments were not a JSON object ({call.arguments_error})",
                               True)
        assert self.bridge is not None
        try:
            out = await self.bridge.call(r.token, capability, call.arguments)
        except FerbError as e:
            return ToolOutcome(call, f"FERB refused: {e}", True)
        text = json.dumps(out["content"], ensure_ascii=False) + "\n" + str(out.get("footer", ""))
        return ToolOutcome(call, text, bool(out.get("is_error")), out.get("image"))

    async def _call(self, r: _Run) -> Turn:
        url, headers, body = self.provider.http(r.state)
        headers = {**headers, **self.provider.auth(r.key)}
        delay = self.backoff_s
        for attempt in range(self.retries + 1):
            try:
                resp = await r.client.post(url, headers=headers, json=body)
            except httpx.TransportError as e:
                if attempt < self.retries:
                    await self.sleep(delay)
                    delay *= 2
                    continue
                raise ProviderError(f"{self.name}: network error ({type(e).__name__})") from None
            if resp.status_code in RETRY_STATUS and attempt < self.retries:
                await self.sleep(_retry_after(resp) or delay)
                delay *= 2
                continue
            if resp.status_code != 200:
                raise ProviderError(self._error_text(resp, r.key))
            try:
                data = resp.json()
            except ValueError:
                raise ProviderError(f"{self.name}: the reply was not JSON") from None
            return self.provider.parse(r.state, data)
        raise ProviderError(f"{self.name}: still failing after {self.retries} retries")

    def _error_text(self, resp: httpx.Response, key: str) -> str:
        try:
            err = resp.json().get("error") or {}
        except (ValueError, AttributeError):
            err = {}
        detail = (err.get("message") or err.get("type") or err.get("status") or "") if isinstance(err, dict) else str(err)
        text = f"{self.name} HTTP {resp.status_code}" + (f": {str(detail)[:300]}" if detail else "")
        return text.replace(key, "[key]") if key else text

    async def continue_(self, run: RunHandle, message: str) -> None:
        raise Unsupported("direct-API runs are one-shot; the supervisor re-sends the chat history each turn")

    async def cancel(self, run: RunHandle) -> None:
        r: _Run = run.state["run"]
        r.cancelled = True
        if r.inflight is not None and not r.inflight.done():
            r.inflight.cancel()


def _retry_after(resp: httpx.Response) -> float | None:
    try:
        return min(float(resp.headers.get("retry-after", "")), 60.0)
    except ValueError:
        return None
```

In `test_network_failure_after_retries_is_an_error`, the test swaps the transport before `start`. Because `start` builds the client from `self.transport`, that works.

- [ ] **Step 6: Run the tests**

Run: `uv run pytest tests/test_api_keys.py tests/test_api_runtime.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add ferb/runtimes/keys.py ferb/runtimes/api.py tests/support/runtime_fakes.py tests/test_api_keys.py tests/test_api_runtime.py
git commit -m "F2 (D32): FERB's own tool loop for direct APIs, tools only through the gateway, keys from the Keychain

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Anthropic Messages API adapter and the shared contract suite

**Files:**
- Create: `ferb/runtimes/anthropic_api.py`
- Create: `tests/test_runtime_contract.py` (scenarios plus the Anthropic case; Tasks 8 and 9 add their cases)
- Test: same

**Interfaces:**
- Consumes: `ApiRuntime`, `Provider`, `ToolSpec`, `ToolCall`, `ToolOutcome`, `Turn` and `ProviderError` (Task 6); `from_anthropic` (Task 3); `Capabilities` (Task 4).
- Produces:
  - `AnthropicProvider(model=DEFAULT_MODEL)`, with `DEFAULT_MODEL = "claude-sonnet-5-5"`.
  - The contract case dict pattern `CASES[provider] = {...}` used by Tasks 8 and 9.

- [ ] **Step 1: Write the contract suite with the Anthropic case**

```python
# tests/test_runtime_contract.py
"""The same contract for every direct-API adapter (D32): typed answers, tool round trips through FERB's gateway,
violations, normalized usage, errors without the key, retries, images, cut-off replies, and P20 cache placement.
Each provider adds one case: its native responses and the checks on its native requests."""
import json

import pytest

from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.anthropic_api import AnthropicProvider
from ferb.runtimes.api import ApiRuntime
from tests.support.runtime_fakes import FakeBridge, Scripted

SCHEMA = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"], "additionalProperties": False}
TASK = Task("t.worker", None, "chat", "dev", Identity.OWNER, set(), "mixed", "r", [], None)
REQ = ModelRequest("FERB instructions", ("device facts", "history", "memory", "message"), 2)
IMAGE = {"content": {"shot": 1}, "footer": "", "is_error": False, "image": {"data": "aGVsbG8=", "mime": "image/png"}}


def anthropic_tool(name, args):
    return {"content": [{"type": "tool_use", "id": "toolu_1", "name": name, "input": args}], "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5}}


CASES = {
    "anthropic": {
        "provider": lambda: AnthropicProvider("claude-sonnet-5-5"),
        "final": {"content": [{"type": "text", "text": '{"ok": true}'}], "stop_reason": "end_turn",
                  "usage": {"input_tokens": 100, "cache_creation_input_tokens": 2000, "cache_read_input_tokens": 3000,
                            "output_tokens": 50}},
        "tool": lambda name, args: anthropic_tool(name, args),
        "cut": {"content": [{"type": "text", "text": '{"ok": tr'}], "stop_reason": "max_tokens", "usage": {}},
        "usage": {"input_tokens": 5100, "cached_input_tokens": 3000, "cache_write_tokens": 2000, "output_tokens": 50},
        "key_header": ("x-api-key", "sk-secret"),
    },
}


def check_anthropic_first(body: dict) -> None:
    assert body["system"][0] == {"type": "text", "text": "FERB instructions", "cache_control": {"type": "ephemeral"}}
    content = body["messages"][0]["content"]
    assert [b["text"] for b in content] == ["device facts", "history", "memory", "message"]
    assert content[1]["cache_control"] == {"type": "ephemeral"} and "cache_control" not in content[2]
    assert body["output_config"]["format"] == {"type": "json_schema", "schema": SCHEMA}
    assert body["max_tokens"] > 0 and body["cache_control"] == {"type": "ephemeral"}


def check_anthropic_result(body: dict, image: bool) -> None:
    result = body["messages"][-1]["content"][0]
    assert result["type"] == "tool_result" and result["tool_use_id"] == "toolu_1"
    kinds = [c["type"] for c in result["content"]]
    assert kinds == (["text", "image"] if image else ["text"])
    assert body["messages"][-2]["role"] == "assistant"                      # the model's turn went back unchanged


CASES["anthropic"]["check_first"] = check_anthropic_first
CASES["anthropic"]["check_result"] = check_anthropic_result


def make(case, script, mode="worker", bridge=None, schema=SCHEMA) -> ApiRuntime:
    async def key(provider):
        return "sk-secret"
    async def sleep(s):
        pass
    return ApiRuntime(case["provider"](), mode=mode, role="worker", tool_names=["fs.list"], output_schema=schema,
                      bridge=bridge or FakeBridge(), key=key, transport=script.transport(), sleep=sleep)


async def run(rt, token="tok"):
    handle = await rt.start(TASK, REQ, token)
    return [e async for e in rt.events(handle)]


@pytest.fixture(params=sorted(CASES))
def case(request):
    return CASES[request.param]


@pytest.mark.anyio
async def test_typed_answer_with_cache_placement(case) -> None:
    script = Scripted([(200, case["final"])])
    events = await run(make(case, script, mode="supervisor"), token="")
    assert events[-1].kind == "final" and json.loads(events[-1].data["text"]) == {"ok": True}
    headers, body = script.requests[0]
    assert headers[case["key_header"][0]].endswith(case["key_header"][1])
    case["check_first"](body)


@pytest.mark.anyio
async def test_tool_call_goes_through_ferb_and_back(case) -> None:
    bridge = FakeBridge()
    script = Scripted([(200, case["tool"]("fs_list", {"path": "/tmp"})), (200, case["final"])])
    events = await run(make(case, script, bridge=bridge))
    assert [e.kind for e in events] == ["usage", "tool_call", "usage", "final"]
    assert bridge.calls == [("fs.list", {"path": "/tmp"})]
    case["check_result"](script.requests[1][1], image=False)


@pytest.mark.anyio
async def test_image_results_reach_the_model(case) -> None:
    script = Scripted([(200, case["tool"]("fs_list", {"path": "/tmp"})), (200, case["final"])])
    await run(make(case, script, bridge=FakeBridge(result=IMAGE)))
    case["check_result"](script.requests[1][1], image=True)


@pytest.mark.anyio
async def test_unoffered_tool_is_a_violation(case) -> None:
    bridge = FakeBridge()
    script = Scripted([(200, case["tool"]("shell_run", {"command": "id"}))])
    events = await run(make(case, script, bridge=bridge))
    assert events[-1].kind == "violation" and bridge.calls == [] and len(script.requests) == 1


@pytest.mark.anyio
async def test_usage_is_normalized(case) -> None:
    events = await run(make(case, Scripted([(200, case["final"])]), mode="supervisor"), token="")
    usage = events[0].data
    assert {k: usage[k] for k in case["usage"]} == case["usage"] and usage["billing"] == "api"


@pytest.mark.anyio
async def test_cut_off_reply_is_an_error(case) -> None:
    events = await run(make(case, Scripted([(200, case["cut"])]), mode="supervisor"), token="")
    assert events[-1].kind == "error" and not any(e.kind == "final" for e in events)


@pytest.mark.anyio
async def test_provider_error_never_contains_the_key(case) -> None:
    events = await run(make(case, Scripted([(401, {"error": {"message": "key sk-secret is invalid"}})]), mode="supervisor"),
                       token="")
    assert events[-1].kind == "error" and "401" in events[-1].data["message"]
    assert "sk-secret" not in events[-1].data["message"]
```

- [ ] **Step 2: Run it to see it fail**

Run: `uv run pytest tests/test_runtime_contract.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'ferb.runtimes.anthropic_api'`.

- [ ] **Step 3: Implement `ferb/runtimes/anthropic_api.py`**

```python
"""Owns the Anthropic Messages API translation for FERB's tool loop (D32). Must not choose targets or add tools.

Verified 2026-10-07 (https://platform.claude.com/docs/en/build-with-claude/prompt-caching, /structured-outputs,
/agents-and-tools/tool-use/define-tools, /build-with-claude/effort):
- cache: breakpoints on the system block and on the last cacheable context part (P20), plus the top-level automatic
  breakpoint that follows the growing tool loop: 3 of the 4 allowed;
- typed answers: output_config.format json_schema (GA, works with tools); tool_choice stays "auto" because "any"/"tool"
  return 400 on Opus 5.5, Sonnet 5.5 and Fable 5.1;
- effort: output_config.effort (not supported by Haiku 4.5); thinking stays adaptive (disabling it returns 400) and the
  assistant's content, thinking blocks included, goes back unchanged;
- max_tokens is required: MAX_TOKENS is a per-reply ceiling, and a reply cut off there is an error, never a partial answer."""

from typing import Any

from ferb.core.types import Capabilities, ModelRequest
from ferb.runtimes.api import ProviderError, ToolCall, ToolOutcome, ToolSpec, Turn
from ferb.runtimes.usage import from_anthropic

URL = "https://api.anthropic.com/v1/messages"
VERSION = "2023-06-01"
DEFAULT_MODEL = "claude-sonnet-5-5"
MAX_TOKENS = 32000
NO_EFFORT = ("claude-haiku-4-5",)
CACHE = {"type": "ephemeral"}


class AnthropicProvider:
    name = "anthropic"
    capabilities = Capabilities("direct", "markers", False, "native", True, True, False, False, "api")

    def __init__(self, model: str = DEFAULT_MODEL) -> None:
        self.model = model

    def begin(self, request: ModelRequest, tools: list[ToolSpec], schema: dict[str, Any] | None, effort: str | None,
              cache_key: str) -> dict[str, Any]:
        content: list[dict[str, Any]] = [{"type": "text", "text": p} for p in request.parts]
        if 0 < request.cacheable <= len(content):
            content[request.cacheable - 1]["cache_control"] = CACHE
        body: dict[str, Any] = {"model": self.model, "max_tokens": MAX_TOKENS, "cache_control": CACHE,
                                "messages": [{"role": "user", "content": content}]}
        if request.system:
            body["system"] = [{"type": "text", "text": request.system, "cache_control": CACHE}]
        if tools:
            body["tools"] = [{"name": t.name, "description": t.description, "input_schema": t.schema} for t in tools]
            body["tool_choice"] = {"type": "auto"}
        output: dict[str, Any] = {}
        if schema:
            output["format"] = {"type": "json_schema", "schema": schema}
        if effort and not self.model.startswith(NO_EFFORT):
            output["effort"] = effort
        if output:
            body["output_config"] = output
        return {"body": body}

    def http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]:
        return URL, {"anthropic-version": VERSION, "content-type": "application/json"}, state["body"]

    def auth(self, key: str) -> dict[str, str]:
        return {"x-api-key": key}

    def parse(self, state: dict[str, Any], data: dict[str, Any]) -> Turn:
        content = data.get("content") or []
        stop = str(data.get("stop_reason") or "")
        if stop == "max_tokens":
            raise ProviderError("anthropic: the reply was cut off at the output limit")
        if stop == "refusal":
            raise ProviderError("anthropic: the model refused to answer")
        state["body"]["messages"].append({"role": "assistant", "content": content})
        calls = [ToolCall(b["id"], b["name"], b["input"] if isinstance(b.get("input"), dict) else {},
                          "" if isinstance(b.get("input"), dict) else "not a JSON object")
                 for b in content if b.get("type") == "tool_use"]
        text = "".join(b.get("text", "") for b in content if b.get("type") == "text")
        return Turn(calls, None if calls else text, from_anthropic(self.model, data.get("usage") or {}, billing="api"), stop)

    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None:
        blocks = []
        for r in results:
            content: list[dict[str, Any]] = [{"type": "text", "text": r.text}]
            if r.image:
                content.append({"type": "image", "source": {"type": "base64", "media_type": r.image["mime"],
                                                            "data": r.image["data"]}})
            blocks.append({"type": "tool_result", "tool_use_id": r.call.id, "content": content, "is_error": r.is_error})
        state["body"]["messages"].append({"role": "user", "content": blocks})
```

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_runtime_contract.py -q`, then `uv run pytest -q`.
Expected: PASS (7 scenarios × 1 case).

- [ ] **Step 5: Commit**

```bash
git add ferb/runtimes/anthropic_api.py tests/test_runtime_contract.py
git commit -m "F2 (D32, P20): Anthropic Messages API adapter with cache markers; the shared runtime contract suite

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: OpenAI Responses API adapter

**Files:**
- Create: `ferb/runtimes/openai_api.py`
- Modify: `tests/test_runtime_contract.py` (add the `openai` case and an OpenAI-only test)

**Interfaces:**
- Consumes: Task 6 types; `from_openai` (Task 3).
- Produces: `OpenAIProvider(model=DEFAULT_MODEL)` with `DEFAULT_MODEL = "gpt-6.1-sol"`, `EXPLICIT_CACHE = ("gpt-6", "gpt-5.6")`, and `capabilities_for(model) -> Capabilities`.

- [ ] **Step 1: Add the OpenAI case to the contract suite**

Append to `tests/test_runtime_contract.py`:

```python
from ferb.runtimes.openai_api import OpenAIProvider


def openai_tool(name, args):
    return {"status": "completed", "output": [{"type": "function_call", "id": "fc_1", "call_id": "call_1", "name": name,
                                               "arguments": json.dumps(args)}], "usage": {"input_tokens": 10}}


def check_openai_first(body: dict) -> None:
    developer, user = body["input"]
    assert developer["role"] == "developer"
    assert developer["content"][0]["prompt_cache_breakpoint"] == {"mode": "explicit"}
    assert [b["text"] for b in user["content"]] == ["device facts", "history", "memory", "message"]
    assert user["content"][1]["prompt_cache_breakpoint"] == {"mode": "explicit"}
    assert "prompt_cache_breakpoint" not in user["content"][2]
    assert body["text"]["format"] == {"type": "json_schema", "name": "ferb_result", "schema": SCHEMA, "strict": True}
    assert body["store"] is False and body["prompt_cache_key"] == "ferb:worker:supervisor"
    assert body["reasoning"] == {"effort": "low"} and "instructions" not in body


def check_openai_result(body: dict, image: bool) -> None:
    items = body["input"]
    assert any(i.get("type") == "function_call" and i["call_id"] == "call_1" for i in items)   # replayed output
    out = items[-1]
    assert out["type"] == "function_call_output" and out["call_id"] == "call_1"
    if image:
        assert [c["type"] for c in out["output"]] == ["input_text", "input_image"]
        assert out["output"][1]["image_url"] == "data:image/png;base64,aGVsbG8="
    else:
        assert isinstance(out["output"], str)


CASES["openai"] = {
    "provider": lambda: OpenAIProvider("gpt-6.1-sol"),
    "final": {"status": "completed", "output": [
        {"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "enc"},
        {"type": "message", "id": "msg_1", "role": "assistant", "content": [{"type": "output_text", "text": '{"ok": true}'}]}],
        "usage": {"input_tokens": 5100, "input_tokens_details": {"cached_tokens": 3000, "cache_write_tokens": 2000},
                  "output_tokens": 50, "output_tokens_details": {"reasoning_tokens": 20}}},
    "tool": openai_tool,
    "cut": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "output": [], "usage": {}},
    "usage": {"input_tokens": 5100, "cached_input_tokens": 3000, "cache_write_tokens": 2000, "output_tokens": 50},
    "key_header": ("authorization", "Bearer sk-secret"),
    "check_first": check_openai_first,
    "check_result": check_openai_result,
}


@pytest.mark.anyio
async def test_openai_malformed_arguments_return_an_error_result() -> None:
    bad = {"status": "completed", "output": [{"type": "function_call", "id": "fc_1", "call_id": "call_1",
                                              "name": "fs_list", "arguments": "{not json"}], "usage": {}}
    bridge = FakeBridge()
    script = Scripted([(200, bad), (200, CASES["openai"]["final"])])
    events = await run(make(CASES["openai"], script, bridge=bridge))
    assert bridge.calls == [] and events[-1].kind == "final"
    assert "not a JSON object" in script.requests[1][1]["input"][-1]["output"]


def test_gpt_5_5_uses_the_routing_key_without_breakpoints() -> None:
    state = OpenAIProvider("gpt-5.5").begin(REQ, [], SCHEMA, None, "ferb:orchestrator:supervisor")
    blocks = [b for item in state["body"]["input"] for b in item["content"]]
    assert not any("prompt_cache_breakpoint" in b for b in blocks)
    assert state["body"]["prompt_cache_key"] == "ferb:orchestrator:supervisor"
```

- [ ] **Step 2: Run it to see it fail**

Run: `uv run pytest tests/test_runtime_contract.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'ferb.runtimes.openai_api'`.

- [ ] **Step 3: Implement `ferb/runtimes/openai_api.py`**

```python
"""Owns the OpenAI Responses API translation for FERB's tool loop (D32). Must not choose targets or add tools.

Verified 2026-10-07 (https://developers.openai.com/api/reference/resources/responses/methods/create,
/api/docs/guides/prompt-caching, /structured-outputs, /reasoning, /function-calling):
- FERB's instructions go in a developer message (top-level `instructions` can't carry a cache breakpoint);
- cache: on GPT-5.6 and later (all gpt-6.x), explicit breakpoints on the instructions and on the last cacheable context
  part (P20), within the 4 writes allowed; on gpt-5.5 and earlier caching is implicit and prompt_cache_key steers routing,
  so the key stays the same for every call of one role (FERB measured per-process keys defeating Codex's cache);
- stateless (store false): every output item, reasoning included, is replayed on the next call;
- typed answers: text.format json_schema strict (FERB's schemas are strict-compatible); tools stay non-strict because
  FERB's tool schemas have optional fields;
- function_call arguments arrive as a JSON string; results go back as function_call_output, with images as input_image."""

import json
from typing import Any

from ferb.core.types import Capabilities, ModelRequest
from ferb.runtimes.api import ProviderError, ToolCall, ToolOutcome, ToolSpec, Turn
from ferb.runtimes.usage import from_openai

URL = "https://api.openai.com/v1/responses"
DEFAULT_MODEL = "gpt-6.1-sol"
EXPLICIT_CACHE = ("gpt-6", "gpt-5.6")
BREAKPOINT = {"mode": "explicit"}


def capabilities_for(model: str) -> Capabilities:
    explicit = model.startswith(EXPLICIT_CACHE)
    return Capabilities("direct", "markers" if explicit else "prefix", True, "native", True, True, False, False, "api")


class OpenAIProvider:
    name = "openai"

    def __init__(self, model: str = DEFAULT_MODEL) -> None:
        self.model = model
        self.capabilities = capabilities_for(model)
        self.explicit = model.startswith(EXPLICIT_CACHE)

    def _text(self, text: str, mark: bool) -> dict[str, Any]:
        block: dict[str, Any] = {"type": "input_text", "text": text}
        if mark and self.explicit:
            block["prompt_cache_breakpoint"] = BREAKPOINT
        return block

    def begin(self, request: ModelRequest, tools: list[ToolSpec], schema: dict[str, Any] | None, effort: str | None,
              cache_key: str) -> dict[str, Any]:
        items: list[dict[str, Any]] = []
        if request.system:
            items.append({"role": "developer", "content": [self._text(request.system, True)]})
        items.append({"role": "user", "content": [self._text(p, i == request.cacheable - 1)
                                                  for i, p in enumerate(request.parts)]})
        body: dict[str, Any] = {"model": self.model, "input": items, "store": False, "prompt_cache_key": cache_key}
        if tools:
            body["tools"] = [{"type": "function", "name": t.name, "description": t.description, "parameters": t.schema,
                              "strict": False} for t in tools]
        if schema:
            body["text"] = {"format": {"type": "json_schema", "name": "ferb_result", "schema": schema, "strict": True}}
        if effort:
            body["reasoning"] = {"effort": effort}
        return {"body": body}

    def http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]:
        return URL, {"content-type": "application/json"}, state["body"]

    def auth(self, key: str) -> dict[str, str]:
        return {"authorization": f"Bearer {key}"}

    def parse(self, state: dict[str, Any], data: dict[str, Any]) -> Turn:
        if data.get("status") == "incomplete":
            reason = (data.get("incomplete_details") or {}).get("reason", "unknown")
            raise ProviderError(f"openai: the reply is incomplete ({reason}); it was cut off")
        output = data.get("output") or []
        state["body"]["input"].extend(output)            # replay every output item, reasoning included
        calls = []
        for item in output:
            if item.get("type") != "function_call":
                continue
            try:
                args = json.loads(item.get("arguments") or "{}")
                error = "" if isinstance(args, dict) else "not a JSON object"
            except ValueError:
                args, error = {}, "not a JSON object"
            calls.append(ToolCall(str(item["call_id"]), str(item["name"]), args if not error else {}, error))
        parts = [c for item in output if item.get("type") == "message" for c in item.get("content", [])]
        if any(c.get("type") == "refusal" for c in parts):
            raise ProviderError("openai: the model refused to answer")
        text = "".join(c.get("text", "") for c in parts if c.get("type") == "output_text")
        return Turn(calls, None if calls else text, from_openai(self.model, data.get("usage") or {}, billing="api"),
                    str(data.get("status") or ""))

    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None:
        for r in results:
            text = ("Tool error: " + r.text) if r.is_error else r.text
            out: Any = text
            if r.image:
                out = [{"type": "input_text", "text": text},
                       {"type": "input_image", "image_url": f"data:{r.image['mime']};base64,{r.image['data']}"}]
            state["body"]["input"].append({"type": "function_call_output", "call_id": r.call.id, "output": out})
```

The test `test_openai_malformed_arguments_return_an_error_result` checks for "not a JSON object" in the output text. The runtime's error text says "the tool arguments were not a JSON object (not a JSON object)", which contains it.

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_runtime_contract.py -q`, then `uv run pytest -q`.
Expected: PASS (7 scenarios × 2 cases, plus the 2 OpenAI-only tests).

- [ ] **Step 5: Commit**

```bash
git add ferb/runtimes/openai_api.py tests/test_runtime_contract.py
git commit -m "F2 (D32, P20): OpenAI Responses API adapter with explicit cache breakpoints and a per-role cache key

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Gemini API adapter

**Files:**
- Create: `ferb/runtimes/google_api.py`
- Modify: `tests/test_runtime_contract.py` (add the `google` case and a thought-signature test)

**Interfaces:**
- Consumes: Task 6 types; `from_gemini` (Task 3).
- Produces: `GoogleProvider(model=DEFAULT_MODEL)` with `DEFAULT_MODEL = "gemini-3.8-flash"`.

- [ ] **Step 1: Add the Google case**

Append to `tests/test_runtime_contract.py`:

```python
from ferb.runtimes.google_api import GoogleProvider


def google_tool(name, args):
    return {"candidates": [{"content": {"role": "model", "parts": [
        {"functionCall": {"name": name, "args": args, "id": "fc1"}, "thoughtSignature": "sig1"}]},
        "finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 10}}


def check_google_first(body: dict) -> None:
    assert body["systemInstruction"] == {"parts": [{"text": "FERB instructions"}]}
    assert [p["text"] for p in body["contents"][0]["parts"]] == ["device facts", "history", "memory", "message"]
    assert body["generationConfig"]["responseMimeType"] == "application/json"
    assert body["generationConfig"]["responseJsonSchema"] == SCHEMA
    assert body["generationConfig"]["thinkingConfig"] == {"thinkingLevel": "low"}


def check_google_result(body: dict, image: bool) -> None:
    model_turn, reply = body["contents"][-2], body["contents"][-1]
    assert model_turn["parts"][0]["thoughtSignature"] == "sig1"              # passed back exactly as received
    response = reply["parts"][0]["functionResponse"]
    assert response["name"] == "fs_list" and response["id"] == "fc1"
    assert ("parts" in response) == image
    if image:
        assert response["parts"][0]["inlineData"] == {"mimeType": "image/png", "data": "aGVsbG8="}


CASES["google"] = {
    "provider": lambda: GoogleProvider("gemini-3.8-flash"),
    "final": {"candidates": [{"content": {"role": "model", "parts": [{"text": '{"ok": true}'}]}, "finishReason": "STOP"}],
              "usageMetadata": {"promptTokenCount": 5000, "cachedContentTokenCount": 3000, "candidatesTokenCount": 30,
                                "thoughtsTokenCount": 20, "toolUsePromptTokenCount": 100}},
    "tool": google_tool,
    "cut": {"candidates": [{"content": {"role": "model", "parts": [{"text": '{"ok'}]}, "finishReason": "MAX_TOKENS"}],
            "usageMetadata": {}},
    "usage": {"input_tokens": 5100, "cached_input_tokens": 3000, "cache_write_tokens": 0, "output_tokens": 50},
    "key_header": ("x-goog-api-key", "sk-secret"),
    "check_first": check_google_first,
    "check_result": check_google_result,
}


def test_google_tools_use_json_schema_parameters() -> None:
    from ferb.runtimes.api import ToolSpec
    state = GoogleProvider().begin(REQ, [ToolSpec("fs_list", "List.", {"type": "object"})], None, None, "k")
    declaration = state["body"]["tools"][0]["functionDeclarations"][0]
    assert declaration == {"name": "fs_list", "description": "List.", "parametersJsonSchema": {"type": "object"}}
```

- [ ] **Step 2: Run it to see it fail**

Run: `uv run pytest tests/test_runtime_contract.py -q`
Expected: FAIL with `ModuleNotFoundError: No module named 'ferb.runtimes.google_api'`.

- [ ] **Step 3: Implement `ferb/runtimes/google_api.py`**

```python
"""Owns the Gemini API (generateContent) translation for FERB's tool loop (D32). Must not choose targets or add tools.

Verified 2026-10-07 (https://ai.google.dev/api/generate-content, /gemini-api/docs/generate-content/function-calling,
/structured-output, /thinking, /caching, /thought-signatures):
- function declarations use parametersJsonSchema (JSON Schema; mutually exclusive with the OpenAPI-subset `parameters`);
  FERB inlines $defs first because the docs don't promise $ref support there;
- typed answers: responseMimeType application/json + responseJsonSchema; together with tools this works on Gemini 3 only
  (gemini-3.8-flash, gemini-3.1-pro-preview), so DEFAULT_MODEL is a Gemini 3 model;
- function-call parts carry a thoughtSignature that must go back exactly: FERB appends the model's whole content unchanged;
- caching is implicit (Gemini 3: at least 4,096 tokens), helped by the stable-first order (P20); hits are reported in
  usageMetadata.cachedContentTokenCount;
- thinkingLevel "low" for planner-type calls ("minimal" is unsupported on 3.8-flash and 3.1-pro).
Google now also promotes a newer Interactions API; generateContent remains documented and is what this adapter uses."""

from typing import Any

from ferb.core.types import Capabilities, ModelRequest
from ferb.runtimes.api import ProviderError, ToolCall, ToolOutcome, ToolSpec, Turn
from ferb.runtimes.usage import from_gemini

BASE = "https://generativelanguage.googleapis.com/v1beta/models"
DEFAULT_MODEL = "gemini-3.8-flash"
STOPPED = {"SAFETY", "RECITATION", "PROHIBITED_CONTENT", "BLOCKLIST", "SPII", "MALFORMED_FUNCTION_CALL"}


class GoogleProvider:
    name = "google"
    capabilities = Capabilities("direct", "prefix", False, "native", True, True, False, False, "api")

    def __init__(self, model: str = DEFAULT_MODEL) -> None:
        self.model = model

    def begin(self, request: ModelRequest, tools: list[ToolSpec], schema: dict[str, Any] | None, effort: str | None,
              cache_key: str) -> dict[str, Any]:
        body: dict[str, Any] = {"contents": [{"role": "user", "parts": [{"text": p} for p in request.parts]}]}
        if request.system:
            body["systemInstruction"] = {"parts": [{"text": request.system}]}
        if tools:
            body["tools"] = [{"functionDeclarations": [{"name": t.name, "description": t.description,
                                                        "parametersJsonSchema": t.schema} for t in tools]}]
        config: dict[str, Any] = {}
        if schema:
            config["responseMimeType"] = "application/json"
            config["responseJsonSchema"] = schema
        if effort:
            config["thinkingConfig"] = {"thinkingLevel": effort}
        if config:
            body["generationConfig"] = config
        return {"body": body}

    def http(self, state: dict[str, Any]) -> tuple[str, dict[str, str], dict[str, Any]]:
        return f"{BASE}/{self.model}:generateContent", {"content-type": "application/json"}, state["body"]

    def auth(self, key: str) -> dict[str, str]:
        return {"x-goog-api-key": key}

    def parse(self, state: dict[str, Any], data: dict[str, Any]) -> Turn:
        candidates = data.get("candidates") or []
        if not candidates:
            reason = (data.get("promptFeedback") or {}).get("blockReason", "no candidates")
            raise ProviderError(f"google: no answer ({reason})")
        first = candidates[0]
        finish = str(first.get("finishReason") or "")
        if finish == "MAX_TOKENS":
            raise ProviderError("google: the reply was cut off at the output limit")
        if finish in STOPPED:
            raise ProviderError(f"google: the model stopped ({finish})")
        content = first.get("content") or {"role": "model", "parts": []}
        state["body"]["contents"].append(content)        # thought signatures go back exactly as received
        parts = content.get("parts") or []
        calls = []
        for p in parts:
            fc = p.get("functionCall")
            if not fc:
                continue
            args = fc.get("args")
            ok = isinstance(args, dict) or args is None
            calls.append(ToolCall(str(fc.get("id") or fc["name"]), str(fc["name"]), args or {} if ok else {},
                                  "" if ok else "not a JSON object"))
        text = "".join(p.get("text", "") for p in parts if "text" in p and not p.get("thought"))
        return Turn(calls, None if calls else text, from_gemini(self.model, data.get("usageMetadata") or {}, billing="api"),
                    finish)

    def add_results(self, state: dict[str, Any], turn: Turn, results: list[ToolOutcome]) -> None:
        parts = []
        for r in results:
            response: dict[str, Any] = {"name": r.call.name, "response": {"error" if r.is_error else "result": r.text}}
            if r.call.id != r.call.name:
                response["id"] = r.call.id
            if r.image:
                response["parts"] = [{"inlineData": {"mimeType": r.image["mime"], "data": r.image["data"]}}]
            parts.append({"functionResponse": response})
        state["body"]["contents"].append({"role": "user", "parts": parts})
```

In `parse`, write the `args` expression with explicit parentheses: `(args or {}) if ok else {}`.

- [ ] **Step 4: Run the tests**

Run: `uv run pytest tests/test_runtime_contract.py -q`, then `uv run pytest -q`.
Expected: PASS (7 scenarios × 3 cases, plus the provider-specific tests).

- [ ] **Step 5: Commit**

```bash
git add ferb/runtimes/google_api.py tests/test_runtime_contract.py
git commit -m "F2 (D32): Gemini API adapter (JSON-schema tools, thought signatures passed back, implicit cache)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Wire the API runtimes into FERB: selection, keys, availability, check

**Files:**
- Modify: `ferb/runtimes/catalog.py` (add `anthropic`, `openai`, `google`)
- Modify: `ferb/registry.py` (register the three runtimes)
- Modify: `ferb/daemon/main.py` (`_build_runtime` API branch, `_api_key` with redaction, availability-filtered worker types at `:419` and `:1248`, the `runtimes.check` RPC)
- Modify: `ferb/daemon/auth.py:30-40` (`runtimes.check` for the CLI role)
- Modify: `ferb/cli/__init__.py`, `ferb/cli/setup.py` (`ferb setup --key PROVIDER`; doctor line per role)
- Test: `tests/test_runtime_wiring.py`

**Interfaces:**
- Consumes: Tasks 4, 6, 7, 8 and 9.
- Produces:
  - `CATALOG` entries `anthropic`/`openai`/`google`; `registry.runtimes` entries pointing at the provider modules.
  - Daemon: `_runtime_available(name) -> bool` (cached 60 s), `_api_key(provider)`, RPC `runtimes.check {"runtime"} -> {"ok": bool, "detail": str}`.
  - `setup.key_setup(env, provider) -> int`.

- [ ] **Step 1: Write the failing tests**

```python
# tests/test_runtime_wiring.py
"""The direct-API runtimes join FERB: chosen per role, keys from the Keychain and redacted, offered only when usable."""
import json

import pytest

from ferb.runtimes.api import ApiRuntime
from ferb.runtimes.catalog import CATALOG
from tests.conftest import RunningDaemon


def test_catalog_lists_all_five_runtimes() -> None:
    assert set(CATALOG) == {"codex", "claude", "anthropic", "openai", "google"}
    assert CATALOG["openai"].capabilities("gpt-5.5").cache == "prefix"
    assert CATALOG["openai"].capabilities("gpt-6.1-sol").cache == "markers"


def test_api_runtime_is_built_for_its_role(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    d.config.set("runtimes", "orchestrator", "openai")
    d.config.set("models", "openai", "gpt-6-sol")
    rt = d._make_runtime("orchestrator", mode="supervisor", tool_names=[], schema={"type": "object"})
    assert isinstance(rt, ApiRuntime) and rt.provider.model == "gpt-6-sol" and rt.role == "orchestrator"


def test_worker_types_list_only_available_runtimes(daemon: RunningDaemon, monkeypatch) -> None:
    d = daemon.daemon
    monkeypatch.setattr(d, "_runtime_available", lambda name: name in ("codex", "anthropic"))
    assert d.supervisor.worker_types() == ["default", "codex", "anthropic"]


def test_a_key_read_for_a_call_is_redacted_everywhere(daemon: RunningDaemon, monkeypatch) -> None:
    import ferb.runtimes.keys as keys
    async def fake_key(provider):
        return "sk-live-abc123"
    monkeypatch.setattr(keys, "api_key", fake_key)
    d = daemon.daemon
    assert daemon.run(d._api_key("openai")) == "sk-live-abc123"
    assert "sk-live-abc123" not in json.dumps(d.redact({"text": "echo sk-live-abc123"}))
```

If `d.redact` returns a different shape than a dict, adapt the assertion to compare its string form. `_api_key` must add the key to the redactor before returning it. (F1's tools-not-loaded re-run never triggers for API runtimes: they report no `tools_seen`, which Task 6's `test_tool_call_goes_through_the_bridge_and_back` pins.)

- [ ] **Step 2: Run them to see them fail**

Run: `uv run pytest tests/test_runtime_wiring.py -q`
Expected: FAIL. `CATALOG` has only two entries, and `_api_key` doesn't exist.

- [ ] **Step 3: Catalog and registry entries**

In `ferb/runtimes/catalog.py`, add:

```python
def _api_available(provider: str) -> Callable[[Path], tuple[bool, str]]:
    def check(home: Path) -> tuple[bool, str]:
        from ferb.runtimes.keys import has_api_key
        return (True, "") if has_api_key(provider) else (False, f"run `ferb setup --key {provider}`")
    return check


def _anthropic_caps(model: str) -> Capabilities:
    from ferb.runtimes.anthropic_api import AnthropicProvider
    return AnthropicProvider.capabilities


def _openai_caps(model: str) -> Capabilities:
    from ferb.runtimes.openai_api import capabilities_for
    return capabilities_for(model)


def _google_caps(model: str) -> Capabilities:
    from ferb.runtimes.google_api import GoogleProvider
    return GoogleProvider.capabilities


CATALOG.update({
    "anthropic": RuntimeInfo("anthropic", "claude-sonnet-5-5", _anthropic_caps, _api_available("anthropic")),
    "openai": RuntimeInfo("openai", "gpt-6.1-sol", _openai_caps, _api_available("openai")),
    "google": RuntimeInfo("google", "gemini-3.8-flash", _google_caps, _api_available("google")),
})
```

Each default model must equal its provider module's `DEFAULT_MODEL`. Add that assertion to the test file for all three.

In `ferb/registry.py` `build()`, add after the Codex/Claude lines:

```python
    r.runtimes["anthropic"] = "ferb.runtimes.anthropic_api"
    r.runtimes["openai"] = "ferb.runtimes.openai_api"
    r.runtimes["google"] = "ferb.runtimes.google_api"
```

- [ ] **Step 4: Daemon: build, keys, availability, check**

In `ferb/daemon/main.py` `_build_runtime`, before the Claude branch, add:

```python
        if name in ("anthropic", "openai", "google"):
            from ferb.runtimes.api import ApiRuntime, GatewayBridge
            from ferb.runtimes.catalog import CATALOG
            provider = _provider(name, model or CATALOG[name].default_model)
            return ApiRuntime(provider, mode=mode, role=role, tool_names=tool_names,  # type: ignore[arg-type]
                              output_schema=schema, bridge=GatewayBridge(self.gateway, self.tokens), key=self._api_key)
```

Add the module-level helper:

```python
def _provider(name: str, model: str) -> Any:
    if name == "anthropic":
        from ferb.runtimes.anthropic_api import AnthropicProvider
        return AnthropicProvider(model)
    if name == "openai":
        from ferb.runtimes.openai_api import OpenAIProvider
        return OpenAIProvider(model)
    from ferb.runtimes.google_api import GoogleProvider
    return GoogleProvider(model)
```

And the methods:

```python
    async def _api_key(self, provider: str) -> str | None:
        """From the Keychain at call time; added to the redactor so an echo never reaches a trace or the journal."""
        from ferb.runtimes import keys
        key = await keys.api_key(provider)
        if key:
            self._add_secret_value(key)
        return key

    def _runtime_available(self, name: str) -> bool:
        from ferb.runtimes.catalog import CATALOG
        now = time.monotonic()
        hit = self._available_cache.get(name)
        if hit is None or now - hit[0] > 60:
            hit = (now, name in CATALOG and CATALOG[name].available(self.home)[0])
            self._available_cache[name] = hit
        return hit[1]
```

Initialise `self._available_cache: dict[str, tuple[float, bool]] = {}` in `__init__`.

At `:419`, change the worker types to:

```python
        self.supervisor.worker_types = lambda: ['default', *[n for n in self.registry.runtimes if self._runtime_available(n)]]
```

At `:1248`, list the same filtered names in the environment facts.

Add the RPC `runtimes.check`. Register it in the dispatch table next to `jev.reload`, and add `"runtimes.check"` to `METHODS[Role.CLI]` in `ferb/daemon/auth.py`:

```python
    async def _runtimes_check(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """One tiny typed call through a runtime: proves the key, model and network work (costs a fraction of a cent)."""
        from ferb.core.types import Identity, ModelRequest, Task
        name = str(params.get("runtime"))
        self._available_cache.pop(name, None)
        schema = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"],
                  "additionalProperties": False}
        try:
            rt = self._build_runtime(name, "orchestrator", mode="supervisor", tool_names=[], schema=schema)
            task = Task("runtime-check", None, "chat", self.device_id, Identity.OWNER, set(), "mixed", "runtime-check", [], None)
            run = await rt.start(task, ModelRequest('Answer {"ok": true}.', ("Check.",), 0), "")
        except Exception as e:
            return {"ok": False, "detail": f"{type(e).__name__}: {e}"}
        detail = ""
        try:
            async for ev in rt.events(run):
                if ev.kind == "final":
                    return {"ok": json.loads(ev.data["text"]).get("ok") is True, "detail": "answered"}
                if ev.kind in ("error", "violation"):
                    detail = str(ev.data)
        finally:
            await rt.cancel(run)
        return {"ok": False, "detail": detail or "no answer"}
```

Use whatever attribute holds the device id elsewhere in `main.py` (e.g. `self.config.get("device", "id")`) if `self.device_id` doesn't exist.

- [ ] **Step 5: `ferb setup --key` and doctor**

In `ferb/cli/__init__.py`, add to the setup parser:

```python
    sp.add_argument("--key", choices=["anthropic", "openai", "google"],
                    help="store a provider's API key in the Keychain and check it answers (direct-API runtimes)")
```

Dispatch it next to `--jev`: `if args.cmd == "setup" and args.key: return setup_cmd.key_setup(env, args.key)`.

In `ferb/cli/setup.py`, add:

```python
def key_setup(env: Env, provider: str) -> int:
    """The key goes straight into the Keychain (`security` asks for it; never an argument, never in chat or config),
    then the daemon makes one tiny typed call with it."""
    import subprocess
    from ferb.identity.secrets import SERVICE
    from ferb.runtimes.keys import account
    env.say(f"Paste your {provider} API key when `security` asks (twice; nothing is shown):")
    if subprocess.run(["security", "add-generic-password", "-U", "-s", SERVICE, "-a", account(provider), "-w"]).returncode:
        env.say("The key wasn't stored.")
        return 1
    try:
        c = env.connect()
        out = c.call("runtimes.check", {"runtime": provider})
        c.close()
    except (NotRunning, FerbError) as e:
        env.say(f"Stored. The daemon couldn't check it yet: {e}")
        return 1
    env.say(f"{provider} answered; use it with `ferb runtimes use ROLE {provider}`." if out.get("ok")
            else f"Stored, but the check failed: {out.get('detail')}")
    return 0 if out.get("ok") else 1
```

In `doctor`, add one `Check` per role. For each role in `ROLES`, use the configured runtime name; if `CATALOG[name].available(inst.home)` is `(False, fix)`, add `Check(f"runtime {role}", "warn", f"{name} not usable", fix)`; otherwise `Check(f"runtime {role}", "ok", name)`.

- [ ] **Step 6: Run the tests**

Run: `uv run pytest tests/test_runtime_wiring.py tests/test_runtime_selection.py -q`, then `uv run pytest -q` and `uv run mypy ferb/core`.
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add ferb/runtimes/catalog.py ferb/registry.py ferb/daemon/main.py ferb/daemon/auth.py ferb/cli/__init__.py ferb/cli/setup.py tests/test_runtime_wiring.py
git commit -m "F2 (D32): direct-API runtimes selectable per role; keys from the Keychain, redacted; only usable runtimes offered

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Live scenario tests and documentation

**Files:**
- Modify: `pyproject.toml` (marker `real_api`)
- Modify: `tests/test_real_runtimes.py` (fixture params for the three API runtimes; API-specific expectations)
- Create: `tests/test_runtime_cache_real.py` (live cache check, `real_api`)
- Modify: `ARCHITECTURE.md` (model runtimes section), `README.md` (`ferb runtimes`, `ferb setup --key`, `[runtimes]`/`[models]`), `CHANGELOG.md` (F2 entry), `docs/0.27_DECISIONS.toml` (D32 and P20 tests and notes), `docs/OWNER_ACCEPTANCE.md` (F2 pending live checks)

**Interfaces:**
- Consumes: everything above.
- Produces: nothing new in code.

- [ ] **Step 1: Add the marker**

In `pyproject.toml` `[tool.pytest.ini_options]`, change `addopts` to `"-m 'not real and not real_claude and not real_api'"`. Add to `markers`:

```toml
           "real_api: the direct-API runtime variants of real tests (billed per token); only when the owner runs -m real_api",
```

- [ ] **Step 2: Extend the live runtime tests**

In `tests/test_real_runtimes.py`:
- Add `pytest.param("anthropic", marks=pytest.mark.real_api)`, `pytest.param("openai", marks=pytest.mark.real_api)` and `pytest.param("google", marks=pytest.mark.real_api)` to the `rt_daemon` fixture params.
- In the fixture, `pytest.skip(f"no {name} key")` when `name in ("anthropic", "openai", "google")` and `not has_api_key(name)`.
- `expected_worker_tools` and the `seen` assertions apply only to Codex and Claude (they report `tools_seen`). For the API runtimes, assert instead that every `tool_call` event's tool is one of `{t.replace('.', '_') for t in FS_TOOLS}`.
- Keep the shell/file-write/subagent native-path assertions for Codex and Claude only; API runtimes have no native tools.

- [ ] **Step 3: Live cache check**

```python
# tests/test_runtime_cache_real.py
"""Live (owner-run, billed): two calls with the same long stable prefix; the second reads the provider's cache (P20)."""
import asyncio

import pytest

from ferb.core.types import Identity, ModelRequest, Task
from ferb.runtimes.keys import has_api_key

pytestmark = pytest.mark.real_api
TASK = Task("cache-check", None, "chat", "dev", Identity.OWNER, set(), "mixed", "cache-check", [], None)
STABLE = "Reference notes for a cache check. " * 900                  # ~6k tokens: above every provider's minimum
SCHEMA = {"type": "object", "properties": {"ok": {"type": "boolean"}}, "required": ["ok"], "additionalProperties": False}


@pytest.mark.parametrize("name", ["anthropic", "openai", "google"])
def test_second_call_reads_the_cache(rt_daemon_for, name: str) -> None:
    if not has_api_key(name):
        pytest.skip(f"no {name} key")
    d = rt_daemon_for(name)
    cached = []
    for i in range(2):
        rt = d.daemon._build_runtime(name, "orchestrator", mode="supervisor", tool_names=[], schema=SCHEMA)
        async def once():
            run = await rt.start(TASK, ModelRequest('Answer {"ok": true}.', (STABLE, f"call {i}"), 1), "")
            return [e async for e in rt.events(run)]
        events = d.run(once(), timeout=120)
        cached.append(next(e.data["cached_input_tokens"] for e in events if e.kind == "usage"))
        d.run(asyncio.sleep(3))
    assert cached[1] > 0, cached
```

Provide `rt_daemon_for` as a fixture in this file. It builds a `RunningDaemon` in a temp home with `SystemClock()`, the way `rt_daemon` does in `tests/test_real_runtimes.py`, and stops it at teardown. The Keychain is the owner's login keychain, shared by the temp-home daemon.

- [ ] **Step 4: Documentation**

- `ARCHITECTURE.md`: add a "Model runtimes (F2, D32)" section covering:
  - `ModelRequest` and the cacheable prefix (`assemble_parts`, `ContextBlock.cache`, which blocks are marked);
  - the five runtimes and the capability table (`ferb runtimes`);
  - per-role selection (`[runtimes]`, `[models]`, checker falls back to worker);
  - `ApiRuntime` and `GatewayBridge` (tools only through the gateway; violation on unoffered tools; retries; no budgets);
  - typed answers checked in `ferb/core/structured.py`;
  - normalized `Usage` and the dated price table;
  - keys in the Keychain (`api/<provider>`, redacted);
  - Codex's own HOME;
  - what remains (Codex's per-process cache key; use an API runtime for the orchestrator to cache across calls).
- `README.md`: document `ferb runtimes`, `ferb runtimes use ROLE RUNTIME [--model M]` and `ferb setup --key PROVIDER`, and that API runtimes bill per token.
- `CHANGELOG.md`: add `## 0.27.0-dev — 2026-10-07 — F2 model runtime layer` above the F1 entry, in the F1 entry's style. It covers the commit range from this plan's commit, with one bullet per task, describing concrete behavior.
- `docs/0.27_DECISIONS.toml`:
  - D32: `tests = ["tests/test_runtime_contract.py", "tests/test_api_runtime.py", "tests/test_runtime_selection.py", "tests/test_usage_prices.py", "tests/test_codex_isolation.py"]`, and update its note to say the skills issue is fixed by Codex's own HOME.
  - P20: add `"tests/test_model_request.py"` to its tests and amend its note: markers and keys are done for the API runtimes; Codex's per-process key remains.
- `docs/OWNER_ACCEPTANCE.md`: add "0.27 F2 — pending live checks":
  1. `ferb runtimes` shows the table.
  2. After `ferb setup --update`, a Codex worker run's trace has no owner skill loads.
  3. For each provider the owner adds a key for: `ferb setup --key P` answers, `ferb runtimes use orchestrator P` works for a few chat turns, and `ferb baseline` / the per-turn cache numbers improve.
  4. `uv run pytest -m real_api` (owner-run).

- [ ] **Step 5: Run the tests**

Run: `uv run pytest tests/test_decision_coverage.py -q`, `uv run pytest -q` (the real_api tests are deselected), `uv run mypy ferb/core` and `git diff --check`.
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add pyproject.toml tests/test_real_runtimes.py tests/test_runtime_cache_real.py ARCHITECTURE.md README.md CHANGELOG.md docs/0.27_DECISIONS.toml docs/OWNER_ACCEPTANCE.md
git commit -m "F2 (D32, P20): live runtime and cache tests for the API runtimes; docs, changelog, registry

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Live acceptance (controller with the owner, after the final review and `ferb setup --update`)

1. **Owner's skills stay out of Codex:** the controller re-runs the Codex probe. The trace shows no owner skill names and no `failed to load skill`.
2. **The capability table:** `ferb runtimes` lists five runtimes, what each role uses, and which ones are usable.
3. **API runtimes, only where the owner adds a key** (which providers is the owner's choice; API use is billed per token):
   - `ferb setup --key P`;
   - `ferb runtimes use orchestrator P`;
   - a few back-to-back chat turns, after which the controller reads the per-turn cached tokens from the trace;
   - then switch back with `ferb runtimes use orchestrator codex` if the owner prefers the subscription.
4. **Live tests:** the owner runs `uv run pytest -m real_api` for the providers with keys.
5. **Verdicts:** recorded with `ferb verdict`.

## Self-Review

- **Spec coverage (D32 and the F2 line):**
  - one interface → Tasks 1, 4, 6;
  - Codex/Claude adapters → Tasks 1, 3, 4, 5;
  - Anthropic/OpenAI/Google → Tasks 7–9;
  - selectable per role → Tasks 4, 10;
  - tool exposure → the capability table, and API tools are always direct (Task 6);
  - cache order, markers and key (P20) → Tasks 1, 7, 8, 9;
  - reasoning effort → Tasks 6–9 (low for supervisor-mode calls);
  - structured output → Tasks 2, 7–9;
  - context limit and image support facts → Task 4's capability table (`image_input`), with context limits in the dated provider facts. Using them in planning is F6, as the spec says "facts for planning";
  - tools always through the gateway → Task 6 (`GatewayBridge`) and Task 10;
  - contract and scenario tests → Tasks 7–9 (contract) and Task 11 (live scenarios for all five);
  - token and price reporting → Task 3;
  - keys in the Keychain → Tasks 6, 10;
  - the carry-forward (owner skills) → Task 5.
- **Not in F2, on purpose:**
  - Jev keeps its own typed client (it isn't an agent runtime); its accounting is F7's.
  - Codex's per-process cache key can't be changed from FERB. The remedy is an API runtime for the orchestrator, and that choice is the owner's.
- **Placeholder scan:** every code step has the code. Three steps tell the implementer to confirm a detail against existing code before using it, and each names where to look:
  - the `sup(...)` signature;
  - `spans.start` arguments;
  - the daemon's device-id attribute.
- **Type consistency:**
  - `ModelRequest(system, parts, cacheable)` and `assemble_parts -> (list[str], int)`;
  - `Usage` fields;
  - `Capabilities` field order: `tools, cache, cache_key, structured_output, reasoning_effort, image_input, continuation, live_messaging, billing`;
  - `Provider.begin(request, tools, schema, effort, cache_key)`;
  - `ToolCall(id, name, arguments, arguments_error)`;
  - `ToolOutcome(call, text, is_error, image)`;
  - `Turn(calls, final, usage, stop)`.

  These are used identically in Tasks 3–10.
- **Review Focus:** each of the five lines has its test in the owning task (Task 2, Task 6, Tasks 7–9, Task 10).
