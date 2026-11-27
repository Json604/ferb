//! The front door's six tools (kb/plans/front-door.md, step 5; ORCH-736): five take the matching part of the planner's
//! answer form (`ferb_context::schemas::supervisor_schema()`), field for field, so every check of a call stays
//! today's; the hand-off (ORCH-743) takes only its ack and goal, and the planner writes the task. The rows in
//! `catalog_additions.json` are written by hand; this derives each of the five's schema from the answer form as it is
//! now and compares the bytes, so the two can never drift apart, and compares the hand-off's with its own. And they
//! are offered and admitted only to a grant that names them (the front door's), never to a worker, the checker or the
//! planner.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, ToolContext};
use ferb_gateway::catalog::{self, Live};
use ferb_gateway::listing::tools_for;
use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use ferb_orchestrator::lookups::{FRONT_SCOPES, FRONT_TOOLS, PLANNER_SCOPES, PLANNER_TOOLS};
use ferb_rpc::auth::{Identity, RunGrant};
use serde_json::{json, Map, Value};

/// Each front tool's arguments but the hand-off's, in order: the answer form's field each one is (`question` is a
/// question's `text`), and the ones a call must give.
const ARGUMENTS: [(&str, &[&str], &[&str]); 5] = [
    ("chat.ask", &["question"], &["question"]),
    (
        "chat.control_task",
        &[
            "control",
            "answer_question",
            "conversation_changes",
            "answers_question",
        ],
        &[],
    ),
    (
        "chat.remember",
        &["remember", "forget", "answers_question"],
        &[],
    ),
    (
        "chat.schedule",
        &[
            "schedules",
            "schedule_changes",
            "show_schedules",
            "answers_question",
        ],
        &[],
    ),
    (
        "chat.set_reminder",
        &["timers", "reminder_cancels", "answers_question"],
        &[],
    ),
];

/// A front tool's `Input` schema as derived from the answer form now: each field from the form's top level or its
/// side entries (`question` is the form's `text`, a string), the form's `$defs` carried when it has any,
/// then the `why` the gateway adds to every tool.
fn derived(fields: &[&str], required: &[&str]) -> Value {
    let form = ferb_context::schemas::supervisor_schema();
    let top = form["properties"].as_object().unwrap();
    let side = form["properties"]["side_entries"]["anyOf"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["type"] == "object")
        .unwrap()["properties"]
        .as_object()
        .unwrap();
    let mut props = Map::new();
    for f in fields {
        let field = match *f {
            "question" => top["text"].clone(),
            _ => top
                .get(*f)
                .or_else(|| side.get(*f))
                .unwrap_or_else(|| panic!("{f} isn't a field of the answer form"))
                .clone(),
        };
        props.insert(f.to_string(), field);
    }
    props.insert("why".into(), catalog::why_property());
    let mut schema = Map::new();
    if let Some(defs) = form.get("$defs") {
        schema.insert("$defs".into(), defs.clone());
    }
    schema.insert("additionalProperties".into(), json!(false));
    schema.insert("properties".into(), Value::Object(props));
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    schema.insert("title".into(), json!("Input"));
    schema.insert("type".into(), json!("object"));
    Value::Object(schema)
}

/// The hand-off's `Input` schema (ORCH-743): the line the owner reads and the goal, nothing of the task form, then
/// the `why` the gateway adds to every tool.
fn dispatch_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "ack": {"type": "string"},
            "goal": {"type": "string"},
            "answers_question": {"type": "boolean", "default": false},
            "why": catalog::why_property(),
        },
        "required": ["ack", "goal"],
        "title": "Input",
        "type": "object",
    })
}

// decision: ORCH-736, ORCH-743
#[test]
fn each_front_tool_takes_the_matching_part_of_the_answer_form() {
    let mine: Vec<&catalog::Addition> = catalog::additions()
        .iter()
        .filter(|a| FRONT_TOOLS.contains(&a.name.as_str()))
        .collect();
    assert_eq!(
        mine.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        FRONT_TOOLS,
        "the six, in FRONT_TOOLS' order"
    );
    let dispatch = mine
        .iter()
        .find(|a| a.name == "chat.dispatch_task")
        .unwrap();
    assert_eq!(
        ferb_py::json::dumps(&dispatch.row["input_schema"]),
        ferb_py::json::dumps(&dispatch_schema()),
        "chat.dispatch_task: its schema is no longer the hand-off's"
    );
    let parts = ARGUMENTS.map(|(name, fields, required)| (name, Some(derived(fields, required))));
    for (name, schema) in parts.into_iter().chain([("chat.dispatch_task", None)]) {
        let a = mine.iter().find(|a| a.name == name).unwrap();
        if let Some(schema) = schema {
            assert_eq!(
                ferb_py::json::dumps(&a.row["input_schema"]),
                ferb_py::json::dumps(&schema),
                "{name}: its schema is no longer the answer form's part"
            );
        }
        assert_eq!(a.decision, "ORCH-736");
        assert_eq!(a.after, "chat.history");
        assert_eq!(a.row["identities"], json!(["owner"]));
        assert_eq!(a.row["scopes"], json!(FRONT_SCOPES));
        assert_eq!(a.row["added_by_gateway"], json!(["why"]));
        assert_eq!(a.row["acts"], json!(false));
        let f = &a.row["facts"];
        assert_eq!(f["named_only"], json!(true), "{name}");
        assert_eq!(f["observation"], json!(false), "{name}");
        assert_eq!(f["reversible"], json!(true), "{name}");
        // what it returns is FERB's own words (accepted, or why not), never content read from outside
        assert_eq!(f["origin"], json!("ferb"), "{name}");
    }
    // the catalog keeps the file's order after chat.history
    let names: Vec<&str> = catalog::catalog()
        .entries
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    let at = names.iter().position(|n| *n == "chat.history").unwrap();
    assert_eq!(names[at + 1..at + 7], FRONT_TOOLS);
}

/// A tool of the catalog, by name only.
struct Named(String);

impl Tool for Named {
    fn name(&self) -> &str {
        &self.0
    }
    fn describe(&self, _args: &Args) -> ActionSpec {
        ActionSpec::new("read")
    }
    fn run<'a>(
        &'a self,
        _args: &'a Args,
        _ctx: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async { Err(ToolError::failed("NotImplementedError", "")) })
    }
}

fn grant(scopes: &[&str], tools: &[&str], observe_only: bool) -> RunGrant {
    RunGrant {
        task_id: "t".into(),
        worker_id: "w".into(),
        scopes: scopes.iter().map(|s| s.to_string()).collect(),
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only,
        tools: tools.iter().map(|s| s.to_string()).collect(),
        max_calls: 0,
    }
}

// decision: ORCH-735
// decision: ORCH-736
#[test]
fn the_front_tools_are_offered_and_admitted_only_to_the_front_door() {
    let mut r = Registry::new();
    for e in &catalog::catalog().entries {
        r.add(Arc::new(Named(e.name.clone()))).unwrap();
    }
    let names = |g: &RunGrant| -> Vec<String> {
        tools_for(&r, g, &Live::default())
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    };
    let every_scope: BTreeSet<String> = catalog::catalog()
        .entries
        .iter()
        .flat_map(|e| e.scopes.iter().cloned())
        .collect();
    let every: Vec<&str> = every_scope.iter().map(String::as_str).collect();
    let worker = grant(&every, &[], false);
    let checker = grant(&every, &[], true);
    let planner = grant(&PLANNER_SCOPES, &PLANNER_TOOLS, true);
    let front_scopes: Vec<&str> = PLANNER_SCOPES
        .iter()
        .chain(&FRONT_SCOPES)
        .copied()
        .collect();
    let front_tools: Vec<&str> = PLANNER_TOOLS.iter().chain(&FRONT_TOOLS).copied().collect();
    let front = grant(&front_scopes, &front_tools, false);
    for g in [&worker, &checker, &planner] {
        let listed = names(g);
        for t in FRONT_TOOLS {
            assert!(!listed.iter().any(|n| n == t), "{t} offered: {listed:?}");
            let tool = r.get(t).unwrap();
            assert!(
                ferb_gateway::admit::admit(g, Some(tool.as_ref()), t, false, true, 0).is_err(),
                "{t} admitted"
            );
        }
    }
    let listed = names(&front);
    let at = listed.iter().position(|n| n == "chat.history").unwrap();
    assert_eq!(listed[at + 1..at + 7], FRONT_TOOLS, "{listed:?}");
    for t in FRONT_TOOLS {
        let tool = r.get(t).unwrap();
        assert!(ferb_gateway::admit::admit(&front, Some(tool.as_ref()), t, false, true, 0).is_ok());
        // their schema as listed: the answer form's part and `why`; nothing that acts, so no `effect`
        let schema = ferb_gateway::listing::listed_schema(tool.as_ref());
        assert!(schema["properties"].get("effect").is_none(), "{t}");
    }
    // without chat:act (the planner's own scopes) none is offered, even by name
    let unscoped = grant(&PLANNER_SCOPES, &front_tools, false);
    assert!(!names(&unscoped)
        .iter()
        .any(|n| n.starts_with("chat.") && n != "chat.history"));
}

/// The gateway's argument model takes a hand-off (ORCH-743: its ack and goal) and gives the same values back;
/// `answers_question` left out is false in what the tool is handed, and the old task form is refused.
// decision: ORCH-736, ORCH-743
#[test]
fn a_hand_off_passes_the_argument_model_unchanged() {
    let r = {
        let mut r = Registry::new();
        r.add(Arc::new(Named("chat.dispatch_task".into()))).unwrap();
        r
    };
    let tool = r.get("chat.dispatch_task").unwrap();
    let goal = "Write hello into a.txt in my Documents folder";
    let args = json!({"ack": "On it.", "goal": goal});
    let checked = ferb_gateway::args::check(tool.as_ref(), args.as_object().unwrap());
    match checked {
        Ok(c) => {
            assert_eq!(c.args["ack"], json!("On it."));
            assert_eq!(c.args["goal"], json!(goal));
            assert_eq!(c.args["answers_question"], json!(false));
        }
        Err(e) => panic!("refused: {e:?}"),
    }
    let check =
        |v: Value| ferb_gateway::args::check(tool.as_ref(), v.as_object().unwrap()).is_err();
    // a missing ack or goal is the argument model's own error
    assert!(check(json!({"goal": goal})));
    assert!(check(json!({"ack": "On it."})));
    // the task form is no longer the hand-off's
    assert!(check(
        json!({"ack": "On it.", "goal": goal, "task": {"goal": goal}})
    ));
}
