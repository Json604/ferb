# Adding a built-in FERB ability

Each reviewed ability lives in its own package here. It is available from first use, without learned runs or an
owner skill installation. Owner-written skills remain under `~/.ferb/skills` and use `skills.read` independently.

1. Define an `AbilityDefinition`: stable ID, version, concise description, `ProcedureResource` anchored to an
   explicitly imported package, typed input, supported interactions and required capability names. Describe only
   implemented interactions. Set an explicit unavailable reason while the implementation is incomplete.
2. Ship `SKILL.md` beside the definition: observations, grounded decisions, verification, failure/handoff behavior
   and existing authority boundaries. Keep that body out of global prompts; metadata is always discoverable and
   `abilities.read` loads the body on demand. The package resource must also work in the built wheel/FERB.app.
3. Register the definition explicitly in `catalog.builtin_definitions`; duplicate IDs fail startup. Tool arguments
   select registered IDs, never paths, Python modules or owner skill files. There is no automatic plugin installer.
4. Add the implementation through the gateway-backed runner when that interface is available. Generic mechanics
   belong in their existing driver layer. Do not call raw drivers or add a model runtime inside an ability. Until
   the runner lands, only catalog/read adapters exist; reading a procedure cannot execute it.
5. Add tests for first-task discovery, dependencies, packaged resource reads, origin separation, execution and
   recovery. Use documented owner-controlled workflows for live acceptance and keep live results separate.

`source_kind=builtin` is metadata, not a trust label. Packaged metadata/procedures use the existing `ferb` origin;
owner skills keep `owner`. Inner web observations and model decisions must keep their `ToolResult.read_sources`
external/inferred labels: the gateway gives these precedence over an adapter's fixed origin. Do not wrap outside
content as a trusted procedure. Version changes must account for persisted checkpoints when execution is added.

Catalog dependency availability describes installed capability names, not per-task permission or a browser's
current connection. The runner must check the current grant, profile, capability features and observations at use.

## Operational guidance

`guidance/definitions.py` explicitly registers the operational modules. These are readable guidance, not runnable
abilities: `readable` describes the packaged resource independently of `availability`, which still describes
execution. `read` returns the exact body with its version and SHA-256; planner context reads and ordinary tool
reads retain that result in their traces. The permanent role contracts require relevant reads, while host-known
loop/conversation state and changed-file inspection attach their corresponding guidance directly and record the
same body/version/hash as `context.source`. No goal-word classifier selects modules.

The system index contains only ID, version, short description and readability. It never embeds every procedure,
its schema or dependency list. Planner reads use its confined context protocol, not a task grant or acting tool.
Worker/checker reads use `abilities.read` with their existing permissions. Dispatch/question/resource roles keep
their narrow contracts and do not receive a catalog they cannot retrieve. Shipped guidance remains subordinate to
the owner's request and permanent contract; external observations and historical copies do not inherit its trust.
