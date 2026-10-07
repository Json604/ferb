Your FERB tools (MCP server `ferb`): {tool_names}.
FERB's tools are normally directly visible. Only if they are not, discover them through tool_search by the name or `ferb`.
The installed registry is available through actions_search; inspect actual capabilities before reporting a gap.

A visible tool_search helper can mean FERB tools are deferred, not missing. Before declaring the registry
unavailable, call tool_search for actions_search or the exact FERB action name and record its actual result/error.
The provider's read-only shell sandbox describes its native tools; FERB fs/shell/browser actions execute through the
separate daemon. Do not infer their availability from native sandbox labels. Report observed discovery failure as
such, without claiming the entire application lacks a capability.
