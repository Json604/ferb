//! Admission (`Gateway._admit`, `gateway.py:1069-1096`): FERB not stopped, the task running, the tool installed and
//! in the worker's scopes and identity, an acting tool never for the final checker (V3), and the planner's lookups
//! within their count (ORCH-516). Stop wins: a stopped FERB or a cancelled task refuses before anything else.

use ferb_rpc::auth::RunGrant;
use serde_json::{json, Map, Value};

use crate::listing::{identity, observes};
use crate::tool::Tool;
use crate::words::{fill, CHECKER_READ_ONLY, LOOKUPS_SPENT};

/// Why a call isn't admitted: the reply's content, whether its footer names FERB as the origin, and the reason a
/// small `tool.refused` span records (None: no span, the task isn't running or was stopped).
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub content: Map<String, Value>,
    pub ferb_origin: bool,
    pub span_reason: Option<String>,
    /// RUN-445: a call refused past `max_calls` counts toward the run's cap.
    pub over_cap: bool,
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// The admission of one call. `made`: the grant's calls so far (ORCH-516).
pub fn admit(
    grant: &RunGrant,
    tool: Option<&dyn Tool>,
    name: &str,
    stopped: bool,
    running: bool,
    made: u64,
) -> Result<(), Refusal> {
    if stopped {
        return Err(Refusal {
            content: obj(json!({"error": "task stopped by the owner"})),
            ferb_origin: false,
            span_reason: None,
            over_cap: false,
        });
    }
    if !running {
        return Err(Refusal {
            content: obj(json!({"error": "task is not running"})),
            ferb_origin: false,
            span_reason: None,
            over_cap: false,
        });
    }
    let outside = match tool {
        None => Some("no such tool"),
        Some(t) if !t.scopes().is_subset(&grant.scopes) => Some("outside this worker's scopes"),
        Some(_) if !grant.tools.is_empty() && !grant.tools.contains(name) => {
            Some("outside this worker's scopes")
        }
        _ => None,
    };
    if let Some(reason) = outside {
        return Err(Refusal {
            content: obj(json!({
                "error": format!("tool {name} is not available to this worker"),
                "next": "Inspect actions.search and report the capability gap to the orchestrator."
            })),
            ferb_origin: false,
            span_reason: Some(reason.into()),
            over_cap: false,
        });
    }
    let tool = tool.expect("checked above");
    if grant.observe_only && !observes(tool) {
        return Err(Refusal {
            content: obj(json!({"error": CHECKER_READ_ONLY})),
            ferb_origin: true,
            span_reason: Some("an acting tool: the final check is read-only".into()),
            over_cap: false,
        });
    }
    if !tool.identities().contains(identity(grant)) {
        return Err(Refusal {
            content: obj(json!({"error": "this tool is not available to this task's identity"})),
            ferb_origin: true,
            span_reason: Some("tool is outside this task's identity".into()),
            over_cap: false,
        });
    }
    if grant.max_calls > 0 && made >= grant.max_calls {
        return Err(Refusal {
            content: obj(
                json!({"error": fill(LOOKUPS_SPENT, &[("n", &grant.max_calls.to_string())])}),
            ),
            ferb_origin: true,
            span_reason: Some(format!("over the grant's {} calls", grant.max_calls)),
            over_cap: true,
        });
    }
    Ok(())
}
