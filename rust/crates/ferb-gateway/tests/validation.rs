//! Argument checks against `rust/fixtures/catalog/validation_errors.json`: 627 calls pydantic refuses, through the
//! real `Gateway.call`, and the 5 refusals before validation. Content, footer and the exact text the model reads.

use std::collections::BTreeSet;
use std::sync::Arc;

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, ToolContext};
use ferb_gateway::catalog;
use ferb_gateway::tool::{Args, Registry, Tool, ToolError, ToolOutput};
use ferb_gateway::{admit, args, reply};
use ferb_rpc::auth::{Identity, RunGrant};
use ferb_sanitize::Redactor;
use serde_json::{Map, Value};

struct Frozen(String);

impl Tool for Frozen {
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

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/catalog/validation_errors.json"
    ))
    .unwrap()
}

fn at(text: &str) -> ferb_core::when::DateTime {
    ferb_core::when::parse(text).unwrap()
}

fn every_scope() -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    for e in &catalog::catalog().entries {
        s.extend(e.scopes.iter().cloned());
    }
    s
}

fn grant(identity: Identity, scopes: BTreeSet<String>, observe_only: bool) -> RunGrant {
    RunGrant {
        task_id: "task-fx".into(),
        worker_id: "w".into(),
        scopes,
        identity,
        expires_at: 1e9,
        observe_only,
        tools: BTreeSet::new(),
        max_calls: 0,
    }
}

fn the_reply(
    content: Map<String, Value>,
    origins: Option<&BTreeSet<String>>,
    running: bool,
) -> reply::Reply {
    let now = at("2026-01-01T09:00:00+00:00");
    let deadline = at("2026-01-01T09:10:00+00:00");
    reply::reply(
        &Redactor::new(&[], &[]),
        &now,
        "UTC",
        running.then_some(&now),
        running.then_some(&deadline),
        Value::Object(content),
        true,
        origins,
        false,
    )
}

// decision: ENG-3
#[test]
fn every_refused_argument_reads_as_pydantic_wrote_it() {
    let f = fixture();
    let mut cases = 0;
    let mut fixed = 0;
    for (name, t) in f["tools"].as_object().unwrap() {
        let tool = Frozen(name.clone());
        for c in t["cases"].as_array().unwrap() {
            cases += 1;
            let given = c["arguments"].as_object().unwrap();
            let refused = args::check(&tool, given).expect_err(&format!("{name}: {}", c["what"]));
            let r = the_reply(refused, None, true);
            assert_eq!(r.content, c["content"], "{name}: {}", c["what"]);
            assert_eq!(r.footer, c["footer"].as_str().unwrap(), "{name}");
            match c.get("model_text") {
                Some(text) => assert_eq!(
                    r.model_text(),
                    text.as_str().unwrap(),
                    "{name}: {}",
                    c["what"]
                ),
                None => {
                    // Fixed against Python (catalog README, "Found in Python"): pydantic left the ValueError object
                    // in ctx.error, so json.dumps of the reply raised TypeError and the worker got no refusal. Here
                    // ctx.error is its text and the reply is written.
                    assert_eq!(
                        c["model_text_error"],
                        "TypeError: Object of type ValueError is not JSON serializable"
                    );
                    assert!(r
                        .model_text()
                        .contains("\"ctx\": {\"error\": \"give exactly one of"));
                    fixed += 1;
                }
            }
            if let Some(rpc) = c.get("rpc_content_json") {
                assert_eq!(
                    ferb_py::json::dumps(&r.content),
                    rpc.as_str().unwrap(),
                    "{name}"
                );
            }
        }
    }
    assert_eq!(cases, 627);
    assert_eq!(fixed, 5);
}

#[test]
fn refusals_before_validation() {
    let f = fixture();
    let mut reg = Registry::new();
    for e in &catalog::catalog().entries {
        reg.add(Arc::new(Frozen(e.name.clone()))).unwrap();
    }
    let every = every_scope();
    let cases = f["before_validation"].as_array().unwrap();
    assert_eq!(cases.len(), 5);
    for c in cases {
        let name = c["tool"].as_str().unwrap();
        let (g, running) = match c["what"].as_str().unwrap() {
            "unknown tool" => (grant(Identity::Owner, every.clone(), false), true),
            "outside the scopes" => (grant(Identity::Owner, BTreeSet::new(), false), true),
            "an acting tool for the checker" => (grant(Identity::Owner, every.clone(), true), true),
            "another identity's tool" => (grant(Identity::Ferb, every.clone(), false), true),
            "a task that is not running" => (grant(Identity::Owner, every.clone(), false), false),
            other => panic!("{other}"),
        };
        let tool = reg.get(name).map(|t| t.as_ref());
        let refused = admit::admit(&g, tool, name, false, running, 0).unwrap_err();
        let ferb: BTreeSet<String> = ["ferb".to_string()].into_iter().collect();
        let r = the_reply(
            refused.content,
            refused.ferb_origin.then_some(&ferb),
            running,
        );
        assert_eq!(r.content, c["content"], "{}", c["what"]);
        assert_eq!(r.footer, c["footer"].as_str().unwrap(), "{}", c["what"]);
        assert_eq!(r.model_text(), c["model_text"].as_str().unwrap());
        assert_eq!(
            ferb_py::json::dumps(&r.content),
            c["rpc_content_json"].as_str().unwrap()
        );
    }
}
