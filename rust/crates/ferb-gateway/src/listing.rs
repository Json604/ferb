//! Which tools a run token's grant is offered, and their listing (`Gateway.tools_for`, `gateway.py:717-733`).
//! The catalog differs by grant (identity, scopes, `observe_only`, `grant.tools`), never a tool's schema.

use ferb_rpc::auth::RunGrant;
use serde_json::{Map, Value};

use crate::catalog::{self, Live};
use crate::tool::{Registry, Tool};

/// `gateway._acts`: a tool that acts, or a generic input (W2): its schema gets `effect`.
pub fn acts(tool: &dyn Tool) -> bool {
    let f = tool.facts();
    !f.reversible || f.generic_input
}

/// `observes`: a tool that only observes (V3, W4).
pub fn observes(tool: &dyn Tool) -> bool {
    tool.facts().observation
}

/// The grant's identity as the tools name it.
pub fn identity(grant: &RunGrant) -> &'static str {
    match grant.identity {
        ferb_rpc::auth::Identity::Owner => "owner",
        ferb_rpc::auth::Identity::Ferb => "ferb",
    }
}

/// Whether the grant is offered this tool.
pub fn offered(grant: &RunGrant, tool: &dyn Tool) -> bool {
    tool.identities().contains(identity(grant))
        && tool.scopes().is_subset(&grant.scopes)
        && !(grant.observe_only && !observes(tool))
        && (grant.tools.is_empty() || grant.tools.contains(tool.name()))
}

/// A tool's schema as the models get it: its own, plus `effect`, `destination` and `why` (`gateway.py:725-731`).
pub fn listed_schema(tool: &dyn Tool) -> Value {
    let mut schema = tool.input_schema();
    if !schema.is_object() {
        schema = Value::Object(Map::new());
    }
    let obj = schema.as_object_mut().expect("an object");
    let props = obj
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(props) = props {
        if acts(tool) {
            props.insert("effect".into(), catalog::effect_property());
        }
        if tool.facts().generic_input && !props.contains_key("destination") {
            props.insert("destination".into(), catalog::destination_property());
        }
        props.insert("why".into(), catalog::why_property());
    }
    schema
}

/// `tools_for(grant)`: `[{"name", "description", "input_schema"}]` in registry order.
pub fn tools_for(registry: &Registry, grant: &RunGrant, live: &Live) -> Vec<Value> {
    registry
        .tools()
        .iter()
        .filter(|t| offered(grant, t.as_ref()))
        .map(|t| {
            let mut row = Map::new();
            row.insert("name".into(), Value::String(t.name().to_string()));
            row.insert("description".into(), Value::String(t.description(live)));
            row.insert("input_schema".into(), listed_schema(t.as_ref()));
            Value::Object(row)
        })
        .collect()
}
