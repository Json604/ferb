"""Writes the fixtures for everything a model receives that Python libraries shape (ENG-3): tool schemas from
pydantic, answer schemas and their per-provider forms, the roles' instructions, jsonschema's error text and the
gateway's argument-validation replies. README.md beside this file is the map for the porter.

Run from the repo root:  uv run python rust/fixtures/catalog/make.py  (`--only tools.json` writes that file alone)

A real `ferb.daemon.main.Daemon` is started in this process in a temporary home (FERB_HOME and HOME both point into a
temporary folder, so nothing reads or writes ~/.ferb or the Keychain), with the switches tests/conftest.py's
`RunningDaemon` uses. No model runtime is started and nothing is sent anywhere. Only calls whose arguments pydantic
refuses are sent through the gateway, so no tool runs. All data is invented.

Output is the same bytes on every run: time is a FakeClock, ids are sequential, the random id of the untrusted-data
wrapper is fixed, and every temporary path is written as /FX.
"""

import ast
import asyncio
import hashlib
import importlib.metadata
import json
import os
import shutil
import sys
import tempfile
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace
from typing import Any

REPO = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
# The MCP protocol version Codex and Claude Code negotiate with FERB's tool bridge (rust/fixtures/runtimes/mcp.json).
MCP_WIRE_VERSION = "2025-06-18"
ROOT = Path(tempfile.mkdtemp(prefix="ferbfx-"))          # short: the daemon's socket path is limited to 104 bytes
os.environ["FERB_HOME"] = str(ROOT / "home")
os.environ["HOME"] = str(ROOT / "user")
os.environ["TZ"] = "UTC"
time.tzset()
(ROOT / "user").mkdir()
tempfile.tempdir = str(ROOT)
sys.path.insert(0, str(REPO))

import jsonschema  # noqa: E402
import pydantic  # noqa: E402
import pydantic_core  # noqa: E402

import ferb.act.browser.ext_bridge as ext_bridge  # noqa: E402
import ferb.act.browser.profiles as profiles  # noqa: E402
import ferb.core.context as core_context  # noqa: E402
import ferb.daemon.reports as reports  # noqa: E402
import ferb.runtimes.codex_exec as codex_exec  # noqa: E402
import ferb.sanitize.images as images  # noqa: E402
import ferb.sanitize.untrusted as untrusted  # noqa: E402
import ferb.sense.network as network  # noqa: E402
from ferb.act.catalog import search_actions  # noqa: E402
from ferb.cli import mcp as mcp_shim  # noqa: E402
from mcp_types import methods as mcp_methods  # noqa: E402
from ferb.clock import context as clock_context  # noqa: E402
from ferb.clock.clock import FakeClock  # noqa: E402
from ferb.core.ids import SequentialIds  # noqa: E402
from ferb.core.prompts import load_prompt  # noqa: E402
from ferb.core.structured import check_final  # noqa: E402
from ferb.core.types import Identity, Task  # noqa: E402
from ferb.daemon import gateway as gw  # noqa: E402
from ferb.daemon.auth import RunGrant  # noqa: E402
from ferb.daemon.main import Daemon  # noqa: E402
from ferb.orchestrator import schemas as S  # noqa: E402
from ferb.orchestrator import supervisor as sup_mod  # noqa: E402
from ferb.orchestrator.tree import TaskRecord  # noqa: E402
from ferb.runtimes import answer_rules, schema_shape  # noqa: E402
from ferb.runtimes.api import inline_refs  # noqa: E402

# The same stand-ins tests/conftest.py's autouse fixtures install: nothing depends on this Mac's browsers, network,
# OCR, report relay or installed Codex.
profiles.locate = lambda bundle_id: None  # type: ignore[assignment]
profiles._located = {}
profiles.has_app = lambda bundle_id: True  # type: ignore[assignment]
profiles.handlers = lambda: []  # type: ignore[assignment]
network.has_route = lambda: True  # type: ignore[assignment]
reports.RELAY_URL = ""
ext_bridge.peer_refusal = lambda app, browser=None: None  # type: ignore[assignment]
images.recognize_text = lambda _image: []  # type: ignore[assignment]
codex_exec.bundled_catalog = lambda ferb_home: None  # type: ignore[assignment]   # never runs the codex program


async def _no_codex(ferb_home: Any, binary: Any) -> None:
    return None
codex_exec.prepare = _no_codex  # type: ignore[assignment]                        # nor `codex --version`
FIXED_WRAP_ID = "0123456789ab"
untrusted.secrets = SimpleNamespace(token_hex=lambda n: FIXED_WRAP_ID)  # type: ignore[attr-defined]


def sha(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def forms(value: Any) -> dict[str, Any]:
    """The two serializations FERB's own code applies to a schema: json.dumps defaults (RPC line, token counts) and
    compact with ensure_ascii=False (httpx's `json=` for the provider APIs, and schema_shape.stated)."""
    default = json.dumps(value)
    compact = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return {"default_sha256": sha(default), "default_len": len(default.encode()),
            "compact_sha256": sha(compact), "compact_len": len(compact.encode())}


def write(name: str, value: Any) -> int:
    text = json.dumps(value, ensure_ascii=False, indent=1) + "\n"
    for real in sorted({str(ROOT), str(ROOT.resolve()), os.path.realpath(ROOT)}, key=len, reverse=True):
        text = text.replace(real, "/FX")
    (HERE / name).write_text(text, encoding="utf-8")
    return len(text.encode("utf-8"))


def simple(value: Any) -> Any:
    """A capability attribute as JSON; sets sorted (their order is not a contract), enums by value."""
    if callable(value):
        return "<method>"
    if hasattr(value, "value") and not isinstance(value, (str, int, float, bool)):
        return simple(value.value)
    if isinstance(value, (set, frozenset)):
        return sorted(simple(v) for v in value)
    if isinstance(value, (list, tuple)):
        return [simple(v) for v in value]
    if isinstance(value, dict):
        return {str(k): simple(v) for k, v in value.items()}
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    return f"<{type(value).__name__}>"


# every capability attribute ferb/daemon/gateway.py and call_hooks.py read with getattr/hasattr
CAP_FACTS = ("risk", "reversible", "observation", "generic_input", "probe_safe", "origin", "describe_in_thread",
             "creates_new_only", "recovery_reason", "target_args", "file_paths", "method_args", "ledger_args",
             "noted_targets", "on_screen", "precheck", "locks", "unavailable", "point", "terminal_input",
             "browser_page", "real_path", "preview_target", "ledger_key")


class StubBrowser:
    """Stands for FERB's own browser profile so the ferb_browser.* tools register (Daemon.use_identity_browser);
    no method of it is ever called: only argument validation runs."""
    name = "ext:stub"
    identity = Identity.FERB

    def __init__(self) -> None:
        self.saved: list[Any] = []
        self.secrets = SimpleNamespace(specs=lambda: self.saved)

    async def shutdown(self) -> None:
        return None


# ---------------------------------------------------------------------------------------------------------------
# tools.json
# ---------------------------------------------------------------------------------------------------------------

def grant(identity: Identity, scopes: set[str], *, observe_only: bool = False, tools: frozenset[str] = frozenset(),
          max_calls: int = 0, task_id: str = "task-fx", worker: str = "w-fx") -> RunGrant:
    return RunGrant(task_id, worker, frozenset(scopes), identity, float("inf"), observe_only, tools, max_calls)


async def tools_fixture(d: Daemon) -> dict[str, Any]:
    g = d.gateway
    assert g is not None
    every = set().union(*(c.scopes for c in g.caps.values()))
    full = {t["name"]: t for ident in (Identity.OWNER, Identity.FERB) for t in g.tools_for(grant(ident, every))}
    assert set(full) == set(g.caps), "a tool no identity can list"
    searched = {row["name"]: row for row in search_actions(g.caps, "")}
    tools = []
    for name, cap in g.caps.items():                       # registry order: the order every listing follows
        listed = full[name]
        raw = cap.Input.model_json_schema()
        api = inline_refs(listed["input_schema"])
        gemini = schema_shape.for_gemini(api).wire
        row: dict[str, Any] = {
            "name": name, "mcp_name": mcp_shim.mcp_name(name), "description": listed["description"],
            "input_schema": listed["input_schema"], "input_schema_forms": forms(listed["input_schema"]),
            "added_by_gateway": [k for k in listed["input_schema"].get("properties", {})
                                 if k not in raw.get("properties", {})],
            "acts": g._acts(cap),
            "scopes": sorted(cap.scopes), "identities": sorted(i.value for i in cap.identities),
            "facts": {k: simple(getattr(cap, k)) for k in CAP_FACTS if hasattr(cap, k)},
            "actions_search_row": searched[name],
            "api_schema_same": api == listed["input_schema"], "gemini_schema_same": gemini == api}
        if api != listed["input_schema"]:
            row["api_schema"] = api
        if gemini != api:
            row["gemini_schema"] = gemini
        tools.append(row)

    def listing(g_: RunGrant) -> dict[str, Any]:
        out = g.tools_for(g_)
        return {"grant": {"identity": g_.identity.value, "scopes": sorted(g_.scopes), "observe_only": g_.observe_only,
                          "tools": sorted(g_.tools), "max_calls": g_.max_calls},
                "names": [t["name"] for t in out], "count": len(out), "json_default_sha256": sha(json.dumps(out)),
                "json_default_len": len(json.dumps(out))}
    planner_tools = frozenset(sorted(n for n in sup_mod.PLANNER_TOOLS if n in g.caps))
    planner = grant(Identity.OWNER, set(sup_mod.PLANNER_SCOPES), observe_only=True, tools=planner_tools,
                    max_calls=sup_mod.PLANNER_LOOKUPS)
    listings = {
        "worker_owner_all_scopes": listing(grant(Identity.OWNER, every)),
        "worker_ferb_all_scopes": listing(grant(Identity.FERB, every)),
        "checker_owner_all_scopes": listing(grant(Identity.OWNER, every, observe_only=True)),
        "checker_ferb_all_scopes": listing(grant(Identity.FERB, every, observe_only=True)),
        "planner_lookups": listing(planner),
        "no_scopes_owner": listing(grant(Identity.OWNER, set())),
    }
    for scope in sorted(every):
        listings["owner_scope:" + scope] = listing(grant(Identity.OWNER, {scope}))

    # The tool bridge: the real Shim.list_tools over the gateway's listing (its daemon call replaced by the listing)
    os.environ["FERB_RUN_TOKEN"] = "fixture-token-not-a-secret"
    shim = mcp_shim.Shim()

    async def mcp_list(g_: RunGrant) -> dict[str, Any]:
        async def rpc(method: str, params: Any = None) -> dict[str, Any]:
            assert method == "tools.list"
            return json.loads(json.dumps({"tools": g.tools_for(g_)}))       # as it crosses the socket
        shim._rpc = rpc  # type: ignore[method-assign]
        result = await shim.list_tools(None, None)
        dumped = result.model_dump(by_alias=True, mode="json", exclude_none=True)
        # What the model programs receive: mcp's ServerRunner._serialize sieves a spec method's result by the
        # connection's protocol version (mcp/server/runner.py), so the handler's ttlMs, cacheScope and resultType
        # never reach a 2025-06-18 client (Codex and Claude Code negotiate it: rust/fixtures/runtimes/mcp.json, a
        # real run of cli/mcp.py). ferb/cli/mcp.py sets no cache hints.
        return mcp_methods.serialize_server_result("tools/list", MCP_WIRE_VERSION, dumped)
    mcp_all = await mcp_list(grant(Identity.OWNER, every))
    for t, src in zip(mcp_all["tools"], g.tools_for(grant(Identity.OWNER, every))):
        assert t == {"name": mcp_shim.mcp_name(src["name"]), "description": src["description"],
                     "inputSchema": src["input_schema"]}, t["name"]
    mcp_names = [mcp_shim.mcp_name(n) for n in g.caps]
    assert len(set(mcp_names)) == len(mcp_names)
    # descriptions that are not constants: three ferb_browser tools list FERB's saved secrets (names, never values)
    stub = d.identity_browser
    stub.saved = [SimpleNamespace(ref=SimpleNamespace(key="ferb/example/password"), account_hint="ferb@example.test",  # type: ignore[union-attr]
                                  allowed_origins=("https://accounts.example.test", "https://example.test"))]
    with_secret = {n: c.description for n, c in g.caps.items() if c.description != full[n]["description"]}
    stub.saved = []  # type: ignore[union-attr]
    return {
        "about": "See README.md. Order of `tools` and of every `names` list is Python's registry order (contract).",
        "dynamic_descriptions": {
            "note": ("Descriptions are read when a listing is made. These change with FERB's saved secrets "
                     "(ferb/act/browser/tools.py _accounts); `tools` holds the form with none saved. browser.downloads "
                     "and ferb_browser.downloads fill {total} from the download limits (20 when unset)."),
            "with_one_invented_secret": with_secret},
        "versions": {"pydantic": pydantic.VERSION, "pydantic_core": pydantic_core.__version__},
        "constants": {"EFFECTS": list(gw.EFFECTS), "EFFECT_FIELD": gw.EFFECT_FIELD, "EFFECT_HELP": gw.EFFECT_HELP,
                      "DESTINATION_FIELD": gw.DESTINATION_FIELD, "DESTINATION_HELP": gw.DESTINATION_HELP,
                      "DESTINATION_SCHEMA": gw.DESTINATION_SCHEMA, "WHY_HELP": gw.WHY_HELP,
                      "CHECKER_READ_ONLY": gw.CHECKER_READ_ONLY, "LOOKUPS_SPENT": gw.LOOKUPS_SPENT,
                      "SAME_LOOKUP": gw.SAME_LOOKUP, "PLANNER_TOOLS": sorted(sup_mod.PLANNER_TOOLS),
                      "PLANNER_SCOPES": sorted(sup_mod.PLANNER_SCOPES), "PLANNER_LOOKUPS": sup_mod.PLANNER_LOOKUPS},
        "tool_count": len(tools), "tools": tools, "listings": listings,
        "mcp_tools_list": {
            "note": ("Shim.list_tools's result as the model programs receive it: mcp's model_dump(by_alias=True, "
                     "mode='json', exclude_none=True) sieved for protocol " + MCP_WIRE_VERSION + " "
                     "(mcp_types.methods.serialize_server_result, as mcp's ServerRunner does). "
                     "For every listing each tool is exactly {name: mcp_name, description, inputSchema: input_schema} "
                     "(asserted here for the full owner listing); the planner's listing is written out as the example."),
            "full_owner_listing_sha256": sha(json.dumps(mcp_all)), "full_owner_listing_count": len(mcp_all["tools"]),
            "envelope_keys": [k for k in mcp_all], "planner_example": await mcp_list(planner)},
        "abilities": {"catalog": d.registry.abilities.catalog(),
                      "read": [{**{k: v for k, v in d.registry.abilities.read(a["id"]).items() if k != "body"},
                                "body_sha256": sha(d.registry.abilities.read(a["id"])["body"]),
                                "body_file": f"ferb/abilities/{a['id']}/SKILL.md"}
                               for a in d.registry.abilities.catalog()]},
    }


# ---------------------------------------------------------------------------------------------------------------
# answer_schemas.json
# ---------------------------------------------------------------------------------------------------------------

async def attention_schema(d: Daemon) -> dict[str, Any]:
    """The attention call's schema is built inside Supervisor.attention (supervisor.py, before its make_runtime call):
    taken from the real method by stopping it at make_runtime."""
    class Taken(Exception):
        pass
    got: dict[str, Any] = {}
    sup = d.supervisor
    assert sup is not None
    keep = sup.make_runtime

    def take(role: str, *, mode: str, tool_names: list[str], schema: dict[str, Any], **kw: Any) -> Any:
        got["schema"] = schema
        raise Taken
    sup.make_runtime = take  # type: ignore[assignment]
    try:
        await sup.attention([], [], [])
    except Taken:
        pass
    finally:
        sup.make_runtime = keep  # type: ignore[assignment]
    return got["schema"]


async def named_schemas(d: Daemon) -> dict[str, tuple[dict[str, Any], str]]:
    from ferb.daemon.areas.intro import SUGGEST_SCHEMA
    from ferb.memory.distill import DISTILL_SCHEMA
    from ferb.memory.import_extract import RECALL_SCHEMA, VERIFY_SCHEMA
    from ferb.memory.mac_profile import MAC_SCHEMA
    from ferb.memory.playbooks import SCHEMA as PLAYBOOK_SCHEMA
    from ferb.memory.profile import PROFILE_SCHEMA
    from ferb.runtimes.check import IMAGE_SCHEMA
    out: dict[str, tuple[dict[str, Any], str]] = {
        "SUPERVISOR_SCHEMA": (S.SUPERVISOR_SCHEMA, "ferb/orchestrator/schemas.py; the planner (supervisor.py make_runtime 'orchestrator')"),
        "WORKER_SCHEMA": (S.WORKER_SCHEMA, "schemas.py worker_schema(); a one-step worker run"),
        "WORKER_STEP_SCHEMA": (S.WORKER_STEP_SCHEMA, "worker_schema(inputs, proposals, parts)"),
        "WORKER_CONVERSATION_SCHEMA": (S.WORKER_CONVERSATION_SCHEMA, "worker_schema(conversation)"),
        "WORKER_WATCH_SCHEMA": (S.WORKER_WATCH_SCHEMA, "worker_schema(watch)"),
        "WORKER_LOOP_SCHEMA": (S.WORKER_LOOP_SCHEMA, "worker_schema(loop)"),
        "WORKER_ALL_SCHEMA": (S.WORKER_ALL_SCHEMA, "worker_schema(all six flags)"),
        "CHECKER_SCHEMA": (S.CHECKER_SCHEMA, "the final checker (verifier.md)"),
        "FIX_SCHEMA": (S.FIX_SCHEMA, "the fix turn (worker.md + roles/fix.md)"),
        "RECHECK_SCHEMA": (S.RECHECK_SCHEMA, "the recheck (recheck.md)"),
        "SUMMARY_SCHEMA": (S.SUMMARY_SCHEMA, "the running chat summary (summary.md)"),
        "QUESTION_SCHEMA": (S.QUESTION_SCHEMA, "a worker question's router (question.md)"),
        "ATTENTION_SCHEMA": (await attention_schema(d), "built inside Supervisor.attention (attention.md); no constant"),
        "PROBE_SCHEMA": (S.PROBE_SCHEMA, "ferb/orchestrator/probes.py; part of the loop worker's schema"),
        "PROFILE_SCHEMA": (PROFILE_SCHEMA, "ferb/memory/profile.py"),
        "DISTILL_SCHEMA": (DISTILL_SCHEMA, "ferb/memory/distill.py"),
        "RECALL_SCHEMA": (RECALL_SCHEMA, "ferb/memory/import_extract.py"),
        "VERIFY_SCHEMA": (VERIFY_SCHEMA, "ferb/memory/import_extract.py"),
        "MAC_SCHEMA": (MAC_SCHEMA, "ferb/memory/mac_profile.py"),
        "PLAYBOOK_SCHEMA": (PLAYBOOK_SCHEMA, "ferb/memory/playbooks.py SCHEMA"),
        "SUGGEST_SCHEMA": (SUGGEST_SCHEMA, "ferb/daemon/areas/intro.py"),
        "IMAGE_SCHEMA": (IMAGE_SCHEMA, "ferb/runtimes/check.py (the connection check)"),
    }
    # every report schema _report_roles can ask for: the 64 flag sets of worker_schema, by name
    flags = ("inputs", "proposals", "conversation", "watch", "loop", "parts")
    for bits in range(64):
        on = {f: bool(bits >> i & 1) for i, f in enumerate(flags)}
        name = "worker_schema(" + ",".join(f for f in flags if on[f]) + ")"
        out[name] = (S.worker_schema(**on), "schemas.py worker_schema, as Supervisor._report_roles calls it")
    return out


def provider_forms(schema: dict[str, Any], full: bool) -> dict[str, Any]:
    entry: dict[str, Any] = {"core_forms": forms(schema), "complexity_core": list(schema_shape.complexity(schema))}
    if full:
        entry["core"] = schema
    # Codex (codex_stream.py 'outputSchema') and the OpenAI API (openai_api.py text.format) take the core schema
    entry["codex"] = {"same_as_core": True, "sent_as": "thread start params 'outputSchema': core"}
    entry["openai"] = {"same_as_core": True,
                       "envelope": {"format": {"type": "json_schema", "name": "ferb_result", "schema": "<core>",
                                               "strict": True}}}
    try:
        shape = schema_shape.for_anthropic(schema)
        entry["anthropic"] = {"mapped": shape.mapped, "complexity_wire": list(schema_shape.complexity(shape.wire)),
                              "wire_forms": forms(shape.wire),
                              "envelope": {"format": {"type": "json_schema", "schema": "<wire>"}}}
        if full and shape.mapped:
            entry["anthropic"]["wire"] = shape.wire
    except schema_shape.SchemaTooComplex as e:
        entry["anthropic"] = {"refused": str(e), "falls_back_to": "stated"}
    try:
        wire = schema_shape.for_gemini(schema).wire
        entry["gemini"] = {"same_as_core": wire == schema, "wire_forms": forms(wire),
                           "keywords_outside_documented": sorted(schema_shape.keywords(wire) - schema_shape.GEMINI_KEYWORDS)}
        if full and wire != schema:
            entry["gemini"]["wire"] = wire
    except schema_shape.SchemaTooComplex as e:
        entry["gemini"] = {"refused": str(e), "falls_back_to": "stated"}
    text = schema_shape.stated(schema)
    entry["stated"] = {"text_sha256": sha(text), "text_len": len(text.encode()),
                       "prefix": text[:text.index("\n") + 1], "rest": "json.dumps(core, ensure_ascii=False, separators=(',', ':'))"}
    tool = inline_refs(schema)
    entry["answer_tool_schema_same_as_core"] = tool == schema
    return entry


async def answer_schemas_fixture(d: Daemon) -> tuple[dict[str, Any], dict[str, dict[str, Any]]]:
    named = await named_schemas(d)
    out: dict[str, Any] = {}
    for name, (schema, where) in named.items():
        out[name] = {"where": where, **provider_forms(schema, full=not name.startswith("worker_schema("))}
    refusal = schema_shape.Refusals.key("anthropic", "invented-model-1", schema_shape.for_anthropic(S.FIX_SCHEMA).wire)
    return {
        "about": "See README.md. 'core' keeps Python's key order (contract: it is what json.dumps writes).",
        "versions": {"jsonschema": importlib.metadata.version("jsonschema")},
        "constants": {"ANTHROPIC_MAX_UNIONS": schema_shape.ANTHROPIC_MAX_UNIONS,
                      "ANTHROPIC_MAX_OPTIONAL": schema_shape.ANTHROPIC_MAX_OPTIONAL,
                      "WRAPPED_NOTE": schema_shape.WRAPPED_NOTE, "STATED_FOR_S": schema_shape.STATED_FOR_S,
                      "GEMINI_KEYWORDS": sorted(schema_shape.GEMINI_KEYWORDS),
                      "ANSWER_TOOL": answer_rules.ANSWER_TOOL, "ANSWER_TOOL_DESC": answer_rules.ANSWER_TOOL_DESC,
                      "PLANNER_ANSWER": answer_rules.PLANNER_ANSWER, "FOLLOW_UP": answer_rules.FOLLOW_UP,
                      "NOT_RUN_WITH_ANSWER": answer_rules.NOT_RUN_WITH_ANSWER,
                      "ANSWER_NOT_TAKEN": answer_rules.ANSWER_NOT_TAKEN,
                      "WRONG_CALLS_CAP": answer_rules.WRONG_CALLS_CAP},
        "stated_example": {"schema": "FIX_SCHEMA", "text": schema_shape.stated(S.FIX_SCHEMA)},
        "refusal_key_example": {"provider": "anthropic", "model": "invented-model-1", "schema": "FIX_SCHEMA (wire)",
                                "key": refusal},
        "schema_count": len(out), "schemas": out,
    }, {name: schema for name, (schema, _) in named.items()}


# ---------------------------------------------------------------------------------------------------------------
# repair_notes.json
# ---------------------------------------------------------------------------------------------------------------

def example(node: Any) -> Any:
    """A smallest value that fits a schema in FERB's dialect (the first type, the first enum value, empty lists)."""
    if not isinstance(node, dict):
        return None
    if "enum" in node:
        return node["enum"][0]
    if "anyOf" in node:
        return example(next((b for b in node["anyOf"] if b.get("type") != "null"), node["anyOf"][0]))
    kind = node.get("type")
    if isinstance(kind, list):
        kind = next((k for k in kind if k != "null"), "null")
    if kind == "object":
        return {k: example(v) for k, v in (node.get("properties") or {}).items()}
    if kind == "array":
        return [example(node.get("items"))] * int(node.get("minItems", 0))
    return {"string": "x", "integer": 1, "number": 1.5, "boolean": True, "null": None}.get(kind)


def wrong(node: Any) -> Any:
    kind = node.get("type") if isinstance(node, dict) else None
    if isinstance(kind, list):
        kind = kind[0]
    return {"string": 7, "integer": "seven", "number": "1.5", "boolean": "yes", "array": {"not": "a list"},
            "object": ["not", "an", "object"]}.get(kind or "", 7)


def judge(text: str, schema: dict[str, Any]) -> dict[str, Any]:
    try:
        check_final(text, schema)
        return {"ok": True}
    except ValueError as e:
        return {"ok": False, "error": str(e), "repair_note": schema_shape.repair_note(str(e))}


def repair_fixture(named: dict[str, dict[str, Any]]) -> dict[str, Any]:
    cases: list[dict[str, Any]] = []

    def add(schema: str, what: str, text: str) -> None:
        cases.append({"schema": schema, "what": what, "answer": text, **judge(text, named[schema])})
    not_json = ["", "   ", "done", "{", '{"status": "done",}', '{"status": "done"} and more', "{'status': 'done'}",
                '```json\n{"answer": "x"}\n```', '{"answer": "x"', '["a", ]', '{"a": 01}', '{"a": "line\nbreak"}',
                '{"a": "\\x"}', "NaN", "﻿{}"]
    for text in not_json:
        add("FIX_SCHEMA", "not JSON", text)
    for text in ("[]", '"text"', "null", "7", "true", "{}", '{"answer": null}', '{"answer": "x", "extra": 1}',
                 '{"answer": 5, "extra": 1}'):
        add("FIX_SCHEMA", "wrong top level or fields", text)
    chosen = ["SUPERVISOR_SCHEMA", "WORKER_SCHEMA", "WORKER_ALL_SCHEMA", "CHECKER_SCHEMA", "RECHECK_SCHEMA",
              "SUMMARY_SCHEMA", "QUESTION_SCHEMA", "ATTENTION_SCHEMA", "PROBE_SCHEMA", "PROFILE_SCHEMA",
              "DISTILL_SCHEMA", "RECALL_SCHEMA", "VERIFY_SCHEMA", "MAC_SCHEMA", "PLAYBOOK_SCHEMA", "SUGGEST_SCHEMA",
              "IMAGE_SCHEMA"]
    valid: dict[str, Any] = {}
    for name in chosen:
        schema = named[name]
        good = example(schema)
        valid[name] = {"answer": json.dumps(good, ensure_ascii=False), **judge(json.dumps(good), schema)}
        if not isinstance(good, dict) or not good:
            continue
        props = schema.get("properties") or {}
        first = next(iter(props))
        last = list(props)[-1]
        add(name, f"missing {first}", json.dumps({k: v for k, v in good.items() if k != first}))
        add(name, "an extra property", json.dumps({**good, "unasked": "café ✓"}, ensure_ascii=False))
        add(name, f"wrong type at {last}", json.dumps({**good, last: wrong(props[last])}))
        enum = next((k for k, v in props.items() if isinstance(v, dict) and "enum" in v), None)
        if enum:
            add(name, f"not in the enum at {enum}", json.dumps({**good, enum: "z" * 260}))
        arr = next((k for k, v in props.items() if isinstance(v, dict) and v.get("type") == "array"
                    and isinstance(v.get("items"), dict)), None)
        if arr:
            item = example(props[arr]["items"])
            bad = wrong(props[arr]["items"])
            add(name, f"a wrong item at {arr}/1", json.dumps({**good, arr: [item, bad]}))
            if isinstance(item, dict) and item:
                short = dict(list(item.items())[1:])
                add(name, f"an item missing a field at {arr}/0", json.dumps({**good, arr: [short]}))
        add(name, "two problems (missing and extra)", json.dumps({**{k: v for k, v in good.items() if k != last},
                                                                   "unasked": 1}))
    # answers in a provider's wire form mapped back to the core form (Shape.answer_text), then checked
    back: list[dict[str, Any]] = []
    for name in ("SUPERVISOR_SCHEMA", "WORKER_ALL_SCHEMA", "CHECKER_SCHEMA", "WORKER_WATCH_SCHEMA"):
        shape = schema_shape.for_anthropic(named[name])
        wire_good = example(shape.wire)
        for what, answer in (("smallest wire answer", wire_good),
                             ("every optional left out", {k: v for k, v in wire_good.items()
                                                          if k in shape.wire.get("required", [])}),
                             ("an unknown property kept", {**wire_good, "unasked": "é"})):
            text = json.dumps(answer, ensure_ascii=False)
            core_text = shape.answer_text(text)
            back.append({"schema": name, "mapped": shape.mapped, "what": what, "wire_answer": text,
                         "core_answer": core_text, **judge(core_text, named[name])})
    back.append({"schema": "SUPERVISOR_SCHEMA", "what": "not JSON is left as it is", "wire_answer": "oops {",
                 "core_answer": schema_shape.for_anthropic(named["SUPERVISOR_SCHEMA"]).answer_text("oops {")})
    fences = ['```json\n{"answer": "x"}\n```', '```\n{"answer": "x"}\n```', '```JSON\n{"answer": "x"}\n```',
              '```python\n{"answer": "x"}\n```', '  ```json\n{"answer": "x"}\n```  ', '{"answer": "x"}', '``````',
              '```json {"answer": "x"}```', 'Here:\n```json\n{"answer": "x"}\n```']
    return {
        "about": ("check_final (ferb/core/structured.py) on invented answers: `error` is the ValueError's text, "
                  "`repair_note` what schema_shape.repair_note makes of it (sent to the model in the stated form). "
                  "Schemas are in answer_schemas.json by name."),
        "versions": {"jsonschema": importlib.metadata.version("jsonschema"), "python": ".".join(map(str, sys.version_info[:3]))},
        "worker_side": {"not_json": "the answer is not JSON ({json.JSONDecodeError.msg})",
                        "not_object": "the answer is not a JSON object",
                        "where": "ferb/orchestrator/worker.py _run_worker_once, raised as InvalidAnswer"},
        "case_count": len(cases), "cases": cases, "valid_examples": valid, "mapped_back": back,
        "unfenced": [{"text": t, "out": schema_shape.unfenced(t)} for t in fences],
    }


# ---------------------------------------------------------------------------------------------------------------
# validation_errors.json
# ---------------------------------------------------------------------------------------------------------------

def bad_arguments(cap: Any) -> list[tuple[str, dict[str, Any]]]:
    schema = cap.Input.model_json_schema()
    props: dict[str, Any] = schema.get("properties") or {}
    required = list(schema.get("required") or [])
    out: list[tuple[str, dict[str, Any]]] = [("nothing given", {}), ("an unknown argument", {"no_such_argument": 1})]
    filled = {k: example(inline_refs({**props[k], "$defs": schema.get("$defs", {})})) for k in required}
    filled = {k: ("x" if v is None else v) for k, v in filled.items()}
    for name, prop in list(props.items())[:4]:
        flat = inline_refs({**prop, "$defs": schema.get("$defs", {})})
        branch = next((b for b in flat.get("anyOf", []) if b.get("type") != "null"), flat)
        out.append((f"wrong type for {name}", {**filled, name: wrong(branch)}))
        if "enum" in branch:
            out.append((f"{name} not one of its values", {**filled, name: "no-such-value"}))
        for key, value in (("minimum", lambda m: m - 1), ("exclusiveMinimum", lambda m: m), ("maximum", lambda m: m + 1),
                           ("maxLength", lambda m: "x" * (m + 1)), ("minLength", lambda m: ""),
                           ("maxItems", lambda m: ["x"] * (m + 1)), ("minItems", lambda m: [])):
            if key in branch and (key != "minLength" or branch[key] > 0) and (key != "minItems" or branch[key] > 0) \
                    and (key not in ("maxLength", "maxItems") or branch[key] <= 300):
                out.append((f"{name} outside {key}", {**filled, name: value(branch[key])}))
    out.append(("null for every argument", {k: None for k in list(props)[:6]}))
    if required:
        out.append(("required arguments empty", {k: "" for k in required}))
    return out


def as_model_text(reply: dict[str, Any]) -> dict[str, Any]:
    """What the model reads: ferb/cli/mcp.py call_tool and ferb/runtimes/api.py _tool both write
    json.dumps(content, ensure_ascii=False) + "\\n" + footer. `rpc_line` is the content as json.dumps(msg) carries it on
    the daemon's socket (ferb/daemon/server.py) before the bridge decodes it."""
    out: dict[str, Any] = {}
    try:
        out["model_text"] = json.dumps(reply["content"], ensure_ascii=False) + "\n" + reply["footer"]
    except (TypeError, ValueError) as e:
        out["model_text_error"] = f"{type(e).__name__}: {e}"
    try:
        out["rpc_content_json"] = json.dumps(reply["content"])
    except (TypeError, ValueError) as e:
        out["rpc_content_error"] = f"{type(e).__name__}: {e}"
    return out


async def validation_fixture(d: Daemon) -> dict[str, Any]:
    g = d.gateway
    assert g is not None and d.spans is not None
    every = set().union(*(c.scopes for c in g.caps.values()))
    grants: dict[Identity, RunGrant] = {}
    for ident in (Identity.OWNER, Identity.FERB):
        task = Task(f"task-fx-{ident.value}", None, "an invented goal", "dev", ident, set(every), "worker",
                    "req-fx", [], d.clock.now() + timedelta(minutes=10))
        root = d.spans.start("task", task_id=task.id, input={})
        d.tasks.add(TaskRecord(task, d.clock.now(), root.id))
        grants[ident] = grant(ident, every, task_id=task.id, worker=f"w-{ident.value}")
    tools: dict[str, Any] = {}
    count = 0
    from pydantic import ValidationError
    for name, cap in g.caps.items():
        ident = Identity.OWNER if Identity.OWNER in cap.identities else Identity.FERB
        generic = bool(getattr(cap, "generic_input", False)) and "destination" not in cap.Input.model_fields
        rows = []
        seen: set[str] = set()
        candidates = bad_arguments(cap)
        for what, args in candidates:
            try:
                cap.Input.model_validate(dict(args))
                continue                                   # pydantic takes it: never sent, so no tool ever runs
            except ValidationError:
                pass
            key = json.dumps(args, sort_keys=True, default=str)
            if key in seen:
                continue
            seen.add(key)
            reply = await g.call(grants[ident], name, {**args, "why": "an invented reason", "effect": "edit"})
            assert reply["is_error"] and reply["content"].get("error") == "invalid arguments", (name, args, reply)
            rows.append({"what": what, "arguments": args, "is_error": reply["is_error"],
                         "content": json.loads(json.dumps(reply["content"], default=str)),
                         "footer": reply["footer"], **as_model_text(reply)})
        if generic:                                        # the declared destination is validated after the arguments
            schema = cap.Input.model_json_schema()
            base: dict[str, Any] | None = None
            for trial in ({}, {k: example(inline_refs({**schema["properties"][k], "$defs": schema.get("$defs", {})}))
                               for k in schema.get("required", [])}):
                try:
                    cap.Input.model_validate(dict(trial))
                    base = trial
                    break
                except ValidationError:
                    continue
            if base is not None:                           # refused at the destination, before precheck: nothing runs
                for what, dest in (("destination amount below zero", {"amount": -1}),
                                   ("destination recipients not a list", {"recipients": "someone"}),
                                   ("destination with an unknown field and a long currency",
                                    {"currency": "123456789", "who": "x"})):
                    reply = await g.call(grants[ident], name, {**base, "destination": dest, "effect": "send"})
                    assert reply["is_error"] and reply["content"].get("error") == "invalid arguments", (name, reply)
                    rows.append({"what": what, "arguments": {**base, "destination": dest, "effect": "send"},
                                 "is_error": True, "content": json.loads(json.dumps(reply["content"], default=str)),
                                 "footer": reply["footer"], **as_model_text(reply)})
        count += len(rows)
        tools[name] = {"identity": ident.value, "generic_destination": generic, "cases": rows}
    # the refusals before validation, for one tool
    stray = []
    for what, g_, name in (("unknown tool", grants[Identity.OWNER], "no.such_tool"),
                           ("outside the scopes", grant(Identity.OWNER, set(), task_id="task-fx-owner"), "fs.write"),
                           ("an acting tool for the checker", grant(Identity.OWNER, every, observe_only=True,
                                                                    task_id="task-fx-owner"), "fs.write"),
                           ("another identity's tool", grants[Identity.FERB], "browser.open"),
                           ("a task that is not running", grant(Identity.OWNER, every, task_id="task-none"), "fs.read")):
        reply = await g.call(g_, name, {})
        stray.append({"what": what, "tool": name, "is_error": reply["is_error"], "content": reply["content"],
                      "footer": reply["footer"], **as_model_text(reply)})
    return {
        "about": ("Gateway._build (ferb/daemon/gateway.py): {'error': 'invalid arguments', 'details': "
                  "ValidationError.errors(include_url=False)}, redacted, with the time footer. Every case went through "
                  "the real Gateway.call; arguments pydantic accepts were never sent."),
        "versions": {"pydantic": pydantic.VERSION, "pydantic_core": pydantic_core.__version__},
        "clock": d.clock.now().isoformat(),
        "case_count": count, "tools": tools, "before_validation": stray,
    }


# ---------------------------------------------------------------------------------------------------------------
# prompt_static.json
# ---------------------------------------------------------------------------------------------------------------

RICH_SKILLS = [
    {"name": "weekly-report", "description": "Résumé of the week — “quoted”, a \"straight\" quote, a back\\slash, a /slash"},
    {"name": "日本語", "description": "tab\there, newline\nhere, emoji 😀, rupee ₹, control \x01 and \x7f, line sep  ."},
    {"name": "", "description": ""},
]
RICH_ABILITIES = [
    {"id": "invented_ability", "version": "2", "description": "An invented ability — naïve café ✓",
     "source_kind": "builtin", "origin": "ferb", "supported_interactions": ["press", "hold"],
     "required_capabilities": [], "availability": {"available": True, "missing_capabilities": [], "reason": None},
     "numbers": {"int": -7, "float": 1.5, "whole": 2.0, "small": 1e-07, "big": 1e16, "third": 0.1,
                 "large": 123456789012345678901234567890, "true": True, "false": False},
     "nested": {"empty_list": [], "empty_object": {}, "list": [1, "a", None, [2, [3]]]}},
]
RICH = {
    "facts": "- About FERB: version 9.9.9 (invented); models: planner Invented-1; worker Invented-2.\n- A second fact with “curly quotes”, ₹ and 😀.",
    "live_facts": "- The AI FERB uses now: planner Invented-1\nThe owner's schedules: none",
    "capabilities": "Installed worker capabilities:\n- Files: an invented line.\n- Time: another invented line — with a dash.",
    "identity_context": "The owner's core profile (invented):\n- Name: Sam Example\n- Speaks: English, 日本語",
    "skills_catalog": RICH_SKILLS,
    "abilities_catalog": RICH_ABILITIES,
}
SYSTEM_CALLS: list[tuple[str | None, bool, tuple[str, ...], str]] = [
    ("supervisor.md", False, (), "the planner (supervisor.py _supervise): live=False"),
    ("worker.md", True, (), "a one-step worker"),
    ("worker.md", True, ("step",), "a plan step"),
    ("worker.md", True, ("step", "inputs"), "a plan step with inputs"),
    ("worker.md", True, ("step", "proposals"), "a plan step a converse step depends on"),
    ("worker.md", True, ("step", "inputs", "proposals", "parts"), "a plan step with every step block"),
    ("worker.md", True, ("converse",), "a conversation turn"),
    ("worker.md", True, ("watch",), "a watch's run"),
    ("worker.md", True, ("loop",), "a loop round"),
    ("worker.md", True, ("scheduled",), "a one-step scheduled run"),
    ("worker.md", True, ("fix",), "the fix turn (FIX_SCHEMA)"),
    ("verifier.md", True, (), "the final checker"),
    ("recheck.md", True, (), "the recheck"),
    ("attention.md", True, (), "the attention call"),
    ("question.md", True, (), "the question router"),
    (None, True, (), "no role prompt (the shared part alone)"),
]


def dumps_sites() -> list[dict[str, Any]]:
    """Every json.dumps call in ferb/, read from the source with ast: file, line, enclosing function, keyword
    arguments as written. README.md says which of them feed a model."""
    out = []
    for path in sorted((REPO / "ferb").rglob("*.py")):
        src = path.read_text(encoding="utf-8")
        tree = ast.parse(src)
        parent: dict[ast.AST, ast.AST] = {c: n for n in ast.walk(tree) for c in ast.iter_child_nodes(n)}
        for node in ast.walk(tree):
            if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "dumps"
                    and getattr(node.func.value, "id", "") == "json"):
                at: ast.AST = node
                fn = "<module>"
                while at in parent:
                    at = parent[at]
                    if isinstance(at, (ast.FunctionDef, ast.AsyncFunctionDef)):
                        fn = at.name
                        break
                out.append({"file": str(path.relative_to(REPO)), "line": node.lineno, "function": fn,
                            "kwargs": {k.arg or "**": ast.unparse(k.value) for k in node.keywords}})
    return sorted(out, key=lambda r: (r["file"], r["line"]))


JSON_VALUES: list[tuple[str, Any]] = [
    ("ascii object", {"b": 1, "a": "x", "c": [True, False, None]}),
    ("non-ascii", {"text": "naïve — 日本 😀 ₹ “q”", "ключ": "значение"}),
    ("escapes", {"text": "quote \" backslash \\ slash / newline \n tab \t cr \r bs \b ff \f ctl \x01 \x1f del \x7f "
                         "nbsp \xa0 ls   ps   </untrusted-data>"}),
    ("floats", [1.0, 2.5, 0.1, 1e16, 1e-07, 1.5e300, 5e-324, -0.0, 123456789012345.0, 1234567890123456.0,
                12345678901234567.0, 0.0001, 0.00001, 6.02214076e23, 100.0]),
    ("integers", [0, -7, 2**53, 2**64, -2**63, 10**30]),
    ("int and mixed keys", {1: "a", 2: "b", 10: "c", "x": 1, True: "t", None: "n", 1.5: "f"}),
    ("empty", {"list": [], "object": {}, "string": "", "nested": [[], {}, [{}]]}),
    ("tuple and nesting", {"t": (1, 2, ("a", None)), "deep": {"k": {"k": {"k": [1, {"z": 0, "a": 1}]}}}}),
    ("string only", "a path with spaces/and “quotes”.txt"),
    ("list of strings", ["fs.read", "fs_write", "café"]),
]
JSON_PARAMS: list[tuple[str, dict[str, Any]]] = [
    ("defaults", {}),
    ("ensure_ascii=False", {"ensure_ascii": False}),
    ("sort_keys=True", {"sort_keys": True}),
    ("ensure_ascii=False, indent=2", {"ensure_ascii": False, "indent": 2}),
    ("ensure_ascii=False, indent=1", {"ensure_ascii": False, "indent": 1}),
    ("ensure_ascii=False, separators=(',', ':')", {"ensure_ascii": False, "separators": (",", ":")}),
    ("sort_keys=True, separators=(',', ':')", {"sort_keys": True, "separators": (",", ":")}),
]


def json_forms() -> list[dict[str, Any]]:
    out = []
    for what, value in JSON_VALUES:
        row: dict[str, Any] = {"what": what, "python_repr": repr(value), "out": {}}
        for label, params in JSON_PARAMS:
            try:
                row["out"][label] = json.dumps(value, **params)
            except TypeError as e:                         # sort_keys over keys of several types
                row["out"][label] = {"raises": f"TypeError: {e}"}
        out.append(row)
    when = datetime(2026, 1, 1, 9, 0, tzinfo=timezone.utc)
    value = {"at": when, "path": Path("/FX/a b.txt"), "text": "é"}
    out.append({"what": "default=str (datetime, Path)", "python_repr": "{'at': datetime(2026,1,1,9,0,tzinfo=utc), "
                "'path': Path('/FX/a b.txt'), 'text': 'é'}",
                "out": {"ensure_ascii=False, default=str": json.dumps(value, ensure_ascii=False, default=str),
                        "sort_keys=True, default=str": json.dumps(value, sort_keys=True, default=str),
                        "ensure_ascii=False, sort_keys=True, separators=(',', ':'), default=str":
                            json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), default=str)}})
    return out


def prompt_fixture(d: Daemon) -> dict[str, Any]:
    sup = d.supervisor
    g = d.gateway
    assert sup is not None and g is not None
    real_abilities = d.registry.abilities.catalog()
    minimal = {"facts": "", "live_facts": "", "capabilities": "Installed worker capabilities:\n- none (invented).",
               "identity_context": "", "skills_catalog": [], "abilities_catalog": real_abilities}
    variants: dict[str, Any] = {}
    for label, inputs in (("minimal", minimal), ("rich", RICH)):
        sup.facts = lambda inputs=inputs: inputs["facts"]  # type: ignore[assignment]
        sup.live_facts = lambda inputs=inputs: inputs["live_facts"]  # type: ignore[assignment]
        sup.availability = lambda inputs=inputs: SimpleNamespace(text=inputs["capabilities"])  # type: ignore[assignment]
        sup.identity_context = lambda inputs=inputs: inputs["identity_context"]  # type: ignore[assignment]
        sup.skills_catalog = lambda inputs=inputs: inputs["skills_catalog"]  # type: ignore[assignment]
        sup.abilities_catalog = lambda inputs=inputs: inputs["abilities_catalog"]  # type: ignore[assignment]
        rows = []
        for name, live, roles, what in SYSTEM_CALLS:
            text = sup._system(name, live=live, roles=roles)
            rows.append({"what": what, "name": name, "live": live, "roles": list(roles), "system": text,
                         "system_sha256": sha(text), "system_len": len(text.encode()),
                         "catalogs_appended": name not in sup_mod.NO_CATALOG_ROLES})
        variants[label] = {"inputs": inputs, "systems": rows}
    directory = REPO / "ferb" / "orchestrator" / "prompts"
    files = {}
    for path in sorted(directory.rglob("*.md")):
        raw = path.read_bytes()
        text = raw.decode("utf-8")
        files[str(path.relative_to(REPO))] = {
            "sha256_file": hashlib.sha256(raw).hexdigest(), "bytes": len(raw),
            "sha256_stripped": sha(text.strip()), "placeholders": sorted({p for p in ("{capabilities}", "{effects}",
                                                                                      "{tool_names}") if p in text})}
    # code-composed pieces around the instructions
    every = set().union(*(c.scopes for c in g.caps.values()))
    now = datetime(2026, 1, 1, 9, 0, tzinfo=timezone.utc)
    started, deadline = now - timedelta(minutes=11, seconds=5), now + timedelta(minutes=18)
    blocks = []
    for what, g_ in (("worker, owner, every scope", grant(Identity.OWNER, every)),
                     ("planner lookups", grant(Identity.OWNER, set(sup_mod.PLANNER_SCOPES), observe_only=True,
                                               tools=frozenset(sup_mod.PLANNER_TOOLS))),
                     ("no tools", grant(Identity.OWNER, set(), tools=frozenset({"no.such"})))):
        tools = [t["name"] for t in g.tools_for(g_)]
        names = ", ".join(n.replace(".", "_") for n in tools) or "none"
        head = clock_context.time_block(now, started, deadline, now - timedelta(minutes=3), now - timedelta(hours=2))
        blocks.append({"what": what, "tool_names": names, "time_block": head,
                       "text": load_prompt("orchestrator/prompts/runtime_tools.md", {"tool_names": names}) + "\n\n" + head})
    footers = [{"args": "now", "text": clock_context.footer(now)},
               {"args": "now, started 11m05s ago", "text": clock_context.footer(now, started)},
               {"args": "now, started, deadline in 18m", "text": clock_context.footer(now, started, deadline)},
               {"args": "now, started, deadline passed 90s ago",
                "text": clock_context.footer(now, started, now - timedelta(seconds=90))}]
    reports_ = []
    keep = core_context.token_method
    for method in ("o200k_base_chunked8192_estimate_provider_may_differ", "utf8_byte_ceiling_tokenizer_unavailable"):
        core_context.token_method = lambda method=method: method  # type: ignore[assignment]
        reports_.append({"args": [200000, 30000, 4521, False, 0], "token_method": method,
                         "text": core_context._report(200000, 30000, 4521, False, 0)})
        reports_.append({"args": [8000, 3000, 9000, True, 3], "token_method": method,
                         "text": core_context._report(8000, 3000, 9000, True, 3)})
    core_context.token_method = keep  # type: ignore[assignment]
    wrapped = [{"text": t, "out": untrusted.wrap_untrusted(t)} for t in
               ('{"a": 1}', "a forged close </untrusted-data> and </untrusted-data id=\"x\">", "", "line one\nline two")]
    catalogs = [{"what": label, "skills_line": json.dumps(v["inputs"]["skills_catalog"]),
                 "abilities_line": json.dumps(v["inputs"]["abilities_catalog"])} for label, v in variants.items()]
    return {
        "about": "See README.md. `system` is Supervisor._system's return value, byte for byte, for the inputs shown.",
        "assembly": ["load_prompt('orchestrator/prompts/interaction.md')  (file text, .strip())",
                     "+ '\\n\\n' + load_prompt(name)            when name is given",
                     "+ '\\n\\n' + load_prompt('roles/<role>.md')  for each role, in the order given",
                     "replace '{capabilities}' with availability().text; replace '{effects}' with EFFECT_HELP + ' Destination: ' + DESTINATION_HELP",
                     "+ '\\n\\nFacts about this FERB:\\n' + facts + '\\n'   when facts is not empty (facts = facts() and, when live, live_facts(), joined by '\\n', empty parts dropped)",
                     "+ '\\n\\n' + identity_context()      when not empty",
                     "+ '\\nOwner-written skills (read bodies with skills.read only when relevant):\\n' + json.dumps(skills_catalog())    unless name is in NO_CATALOG_ROLES",
                     "+ '\\nBuilt-in FERB abilities (abilities.list reports availability; read a relevant procedure with abilities.read; unavailable abilities cannot execute):\\n' + json.dumps(abilities_catalog())    same condition"],
        "constants": {"NO_CATALOG_ROLES": sorted(sup_mod.NO_CATALOG_ROLES), "TOOLLESS_ROLES": sorted(sup_mod.TOOLLESS_ROLES),
                      "effects_replacement": gw.EFFECT_HELP + " Destination: " + gw.DESTINATION_HELP,
                      "LOOKUPS_SPENT_NOTE": sup_mod.LOOKUPS_SPENT_NOTE, "wrap_untrusted_id_fixed_to": FIXED_WRAP_ID},
        "prompt_files": files,
        "variants": variants,
        "catalog_lines": catalogs,
        "runtime_tools_blocks": blocks,
        "tool_result_footers": footers,
        "context_budget_reports": reports_,
        "wrap_untrusted": wrapped,
        "json_forms": json_forms(),
        "json_dumps_call_sites": dumps_sites(),
    }


# ---------------------------------------------------------------------------------------------------------------

# `--only tools.json`: write that file alone (the others unchanged).
ONLY = sys.argv[sys.argv.index("--only") + 1] if "--only" in sys.argv else None


async def main() -> dict[str, int]:
    d = Daemon(home=ROOT / "home", clock=FakeClock(), ids=SequentialIds("id"), watch_owner_input=False,
               watchers=False, distill=False, retention=False, resources=False, notifications=False, snapshots=False,
               persona_scan=None, screen_at_send=False, preload_secrets=False, embeddings=False,
               data_access=lambda: False, developer_tools=lambda: [])
    d._runtime_available = lambda name: name in ("codex", "claude")  # type: ignore[method-assign]
    d._permissions = lambda: {"accessibility": True, "screen": True}  # type: ignore[method-assign]
    await d.start()
    sizes: dict[str, int] = {}
    try:
        d.use_identity_browser(StubBrowser())  # type: ignore[arg-type]
        sizes["tools.json"] = write("tools.json", await tools_fixture(d))
        if ONLY == "tools.json":
            return sizes
        answers, named = await answer_schemas_fixture(d)
        sizes["answer_schemas.json"] = write("answer_schemas.json", answers)
        sizes["repair_notes.json"] = write("repair_notes.json", repair_fixture(named))
        sizes["validation_errors.json"] = write("validation_errors.json", await validation_fixture(d))
        sizes["prompt_static.json"] = write("prompt_static.json", prompt_fixture(d))
    finally:
        await d.shutdown()
    return sizes


if __name__ == "__main__":
    try:
        done = asyncio.run(main())
    finally:
        shutil.rmtree(ROOT, ignore_errors=True)
    for name, size in done.items():
        print(f"{name}: {size} bytes")
    print(f"total: {sum(done.values())} bytes")
