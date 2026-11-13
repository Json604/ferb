//! The real call hooks (`call_hooks::RealHooks`) against `rust/fixtures/gateway/hooks.json` (made by `make_hooks.py`
//! from the real Python `CallHooks` over recording fakes): every op's result, every call a fake received in order,
//! every event and journaled problem.
#![cfg(target_os = "macos")]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ferb_core::interfaces::BoxFuture;
use ferb_core::types::{ActionSpec, Target, ToolContext, ToolResult};
use ferb_gateway::call_hooks::{
    AppShot, AppWindows, ChangeRecord, Evidence, HookParts, ImageSanitizer, Previews, RealHooks,
    Sentinel,
};
use ferb_gateway::seams::CallHooks;
use ferb_gateway::tool::{Args, Facts, Point, PreviewTarget, Shot, Tool, ToolError, ToolOutput};
use ferb_rpc::auth::{Identity, RunGrant};
use serde_json::{json, Map, Value};

type Log = Arc<Mutex<Vec<Value>>>;

fn push(log: &Log, v: Value) {
    log.lock().unwrap().push(v);
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

/// A fake's error, by its recorded name (`error_name`).
fn raised(cfg: &Value, key: &str) -> Option<String> {
    cfg.get(key).and_then(|r| s(r, "name"))
}

struct FakeSentinel(Log, Value);
impl Sentinel for FakeSentinel {
    fn call_started(&self, task: &str, span: &str, name: &str) {
        push(&self.0, json!(["sentinel.call_started", task, span, name]));
    }
    fn before_action<'a>(
        &'a self,
        task: &'a str,
        span: &'a str,
        name: &'a str,
        arguments: &'a Map<String, Value>,
        on_screen: bool,
    ) -> BoxFuture<'a, Result<(), String>> {
        push(
            &self.0,
            json!([
                "sentinel.before_action",
                task,
                span,
                name,
                arguments,
                on_screen
            ]),
        );
        let e = raised(&self.1, "before_raises");
        Box::pin(async move { e.map_or(Ok(()), Err) })
    }
    fn call_ended(&self, span: &str) {
        push(&self.0, json!(["sentinel.call_ended", span]));
    }
    fn after_action<'a>(
        &'a self,
        span: &'a str,
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>> {
        push(&self.0, json!(["sentinel.after_action", span]));
        let out = match raised(&self.1, "after_raises") {
            Some(e) => Err(e),
            None => Ok(self
                .1
                .get("after")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()),
        };
        Box::pin(async move { out })
    }
}

struct FakePreviews(Log);
impl Previews for FakePreviews {
    fn declare(&self, root: &str, task: &str, target: Option<PreviewTarget>, locks: &[String]) {
        push(
            &self.0,
            json!([
                "previews.declare",
                root,
                task,
                target.map(|t| t.scope),
                locks
            ]),
        );
    }
    fn remove(&self, task_ids: &BTreeSet<String>) {
        push(&self.0, json!(["previews.remove", task_ids]));
    }
}

struct FakeEvidence(Log, Value);
impl Evidence for FakeEvidence {
    fn put<'a>(
        &'a self,
        task: &'a str,
        span: &'a str,
        kind: &'a str,
        image: Option<&'a [u8]>,
        data: Map<String, Value>,
        origin: String,
    ) -> BoxFuture<'a, Result<String, String>> {
        let img = image.map(|b| String::from_utf8(b.to_vec()).unwrap());
        push(
            &self.0,
            json!(["evidence.put", task, span, kind, img, data, origin]),
        );
        let out = raised(&self.1, "evidence_raises").map_or(Ok("ev-1".to_string()), Err);
        Box::pin(async move { out })
    }
}

struct FakeChanges(Log, Value);
impl ChangeRecord for FakeChanges {
    fn before<'a>(
        &'a self,
        task: &'a str,
        tree: &'a str,
        paths: Vec<String>,
    ) -> BoxFuture<'a, Result<Map<String, Value>, String>> {
        push(&self.0, json!(["changes.before", task, tree, paths]));
        let n = paths.len();
        let out = match raised(&self.1, "changes_raises") {
            Some(e) => Err(e),
            None => Ok(json!({"copied": n}).as_object().cloned().unwrap()),
        };
        Box::pin(async move { out })
    }
}

struct FakeImages(Log, Value);
impl ImageSanitizer for FakeImages {
    fn sanitize<'a>(&'a self, image: &'a [u8]) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        push(
            &self.0,
            json!([
                "images.sanitize",
                String::from_utf8(image.to_vec()).unwrap()
            ]),
        );
        let fail = self.1.get("sanitize_raises").is_some();
        let mut out = image.to_vec();
        out.extend_from_slice(b"+clean");
        Box::pin(async move {
            if fail {
                Err("RuntimeError".into())
            } else {
                Ok(out)
            }
        })
    }
}

fn shot_of(v: &Value) -> Result<Option<AppShot>, Shot> {
    match v {
        Value::Null => Ok(None),
        Value::String(t) if t == "NoTarget" => Err(Shot::NoTarget),
        Value::String(t) if t == "Failed" => Err(Shot::Failed),
        Value::String(t) => Err(Shot::Withheld(t.strip_prefix("Withheld:").unwrap().into())),
        w => Ok(Some(AppShot {
            image: s(w, "image").map(String::into_bytes),
            rects: w["rects"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|r| {
                            let r: Vec<f64> = r
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|x| x.as_f64().unwrap())
                                .collect();
                            [r[0], r[1], r[2], r[3]]
                        })
                        .collect()
                })
                .unwrap_or_default(),
            facts: w["facts"].as_object().cloned().unwrap_or_default(),
            read_sources: w["reads"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|p| (p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into()))
                        .collect()
                })
                .unwrap_or_default(),
        })),
    }
}

struct FakeWindows(Log, Value);
impl AppWindows for FakeWindows {
    fn shot<'a>(
        &'a self,
        app: &'a str,
        scopes: &'a BTreeSet<String>,
    ) -> BoxFuture<'a, Result<Option<AppShot>, Shot>> {
        push(&self.0, json!(["app_window", app, scopes]));
        let out = shot_of(self.1.get("app_window").unwrap_or(&Value::Null));
        Box::pin(async move { out })
    }
    fn mask<'a>(
        &'a self,
        image: Vec<u8>,
        rects: &'a [[f64; 4]],
    ) -> BoxFuture<'a, Result<Vec<u8>, Shot>> {
        let rs: Vec<Value> = rects
            .iter()
            .map(|r| json!(r.iter().map(|x| *x as i64).collect::<Vec<_>>()))
            .collect();
        push(
            &self.0,
            json!(["mask", String::from_utf8(image.clone()).unwrap(), rs]),
        );
        let out = match s(&self.1, "mask_raises") {
            Some(e) => Err(Shot::Withheld(e)),
            None => {
                let mut i = image;
                i.extend_from_slice(b"+masked");
                Ok(i)
            }
        };
        Box::pin(async move { out })
    }
}

/// The capability from a spec (make_hooks.py's `Cap`).
struct HookTool(Value);
impl Tool for HookTool {
    fn name(&self) -> &str {
        self.0
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("probe.tool")
    }
    fn facts(&self) -> Facts {
        Facts {
            on_screen: self.0.get("on_screen").is_some_and(|v| v == true),
            observation: self.0.get("observation").is_some_and(|v| v == true),
            ..Facts::default()
        }
    }
    fn describe(&self, _args: &Args) -> ActionSpec {
        ActionSpec::new("read")
    }
    fn preview_target(
        &self,
        _args: &Args,
        _owner: &str,
        observed: Option<&Map<String, Value>>,
    ) -> Result<Option<PreviewTarget>, String> {
        let Some(p) = self.0.get("preview") else {
            return Ok(None);
        };
        let p = match p {
            Value::String(t) if t == "raise" => return Err("RuntimeError".into()),
            Value::Object(o) => o
                .get(if observed.is_some() {
                    "observed"
                } else {
                    "before"
                })
                .cloned()
                .unwrap_or(Value::Null),
            other => other.clone(),
        };
        Ok(p.as_str().map(|scope| PreviewTarget {
            scope: scope.into(),
            label: "the app".into(),
            capture: None,
        }))
    }
    fn point<'a>(
        &'a self,
        _args: &'a Args,
        _grant: &'a RunGrant,
    ) -> BoxFuture<'a, Result<Option<Point>, String>> {
        let out = match self.0.get("point") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(_)) => Err("RuntimeError".into()),
            Some(p) => Ok(Some(Point {
                kind: s(p, "kind").unwrap(),
                rect: {
                    let r: Vec<f64> = p["rect"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_f64().unwrap())
                        .collect();
                    [r[0], r[1], r[2], r[3]]
                },
                display: p["display"].as_i64().unwrap(),
            })),
        };
        Box::pin(async move { out })
    }
    fn file_paths(&self, _args: &Args) -> Option<Vec<(String, String)>> {
        self.0.get("file_paths").and_then(Value::as_array).map(|a| {
            a.iter()
                .map(|p| (p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into()))
                .collect()
        })
    }
    fn tab_picture<'a>(
        &'a self,
        _args: &'a Args,
        _task: &'a str,
    ) -> BoxFuture<'a, Result<(Vec<u8>, String), Shot>> {
        let out = match self.0.get("tab_picture") {
            None => Err(Shot::Failed),
            Some(Value::String(t)) if t == "NoTarget" => Err(Shot::NoTarget),
            Some(Value::String(t)) if t == "Failed" => Err(Shot::Failed),
            Some(Value::String(t)) => {
                Err(Shot::Withheld(t.strip_prefix("Withheld:").unwrap().into()))
            }
            Some(p) => Ok((
                s(p, "image").unwrap().into_bytes(),
                s(p, "masking").unwrap_or_default(),
            )),
        };
        Box::pin(async move { out })
    }
    fn run<'a>(
        &'a self,
        _a: &'a Args,
        _c: &'a ToolContext,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async { Ok(ToolOutput::default()) })
    }
}

fn grant_of(op: &Value) -> RunGrant {
    RunGrant {
        task_id: s(op, "task").unwrap_or_else(|| "t1".into()),
        worker_id: s(op, "worker").unwrap_or_else(|| "w1".into()),
        scopes: op
            .get("scopes")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|x| x.as_str().unwrap().to_string()).collect())
            .unwrap_or_else(|| ["probe".to_string()].into_iter().collect()),
        identity: Identity::Owner,
        expires_at: 1e9,
        observe_only: op.get("observe_only").is_some_and(|v| v == true),
        tools: BTreeSet::new(),
        max_calls: 0,
    }
}

fn spec_of(op: &Value) -> ActionSpec {
    ActionSpec {
        kind: s(op, "kind").unwrap_or_else(|| "read".into()),
        target: Target {
            site: s(op, "site"),
            path: s(op, "path"),
            ..Target::default()
        },
        content: s(op, "content"),
        ..ActionSpec::new("read")
    }
}

fn args_of(op: &Value) -> Args {
    op.get("args")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn utf8(b: &Option<Vec<u8>>) -> Value {
    b.as_ref().map_or(
        Value::Null,
        |b| json!(String::from_utf8(b.clone()).unwrap()),
    )
}

async fn replay(case: &Value) -> Value {
    let cfg = case.get("config").cloned().unwrap_or(json!({}));
    let on = |k: &str| cfg.get(k).is_none_or(|v| v != false);
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let events: Log = Arc::new(Mutex::new(Vec::new()));
    let journal: Log = Arc::new(Mutex::new(Vec::new()));
    let (ev, jr, sh) = (events.clone(), journal.clone(), log.clone());
    let shadow_fails = raised(&cfg, "shadow_raises");
    let parts = HookParts {
        sentinel: on("sentinel")
            .then(|| Arc::new(FakeSentinel(log.clone(), cfg.clone())) as Arc<dyn Sentinel>),
        previews: Some(Arc::new(FakePreviews(log.clone()))),
        evidence: on("evidence")
            .then(|| Arc::new(FakeEvidence(log.clone(), cfg.clone())) as Arc<dyn Evidence>),
        changes: on("changes")
            .then(|| Arc::new(FakeChanges(log.clone(), cfg.clone())) as Arc<dyn ChangeRecord>),
        images: Some(Arc::new(FakeImages(log.clone(), cfg.clone()))),
        app_windows: on("app_windows")
            .then(|| Arc::new(FakeWindows(log.clone(), cfg.clone())) as Arc<dyn AppWindows>),
        shadow: on("shadow").then(|| {
            let f: ferb_gateway::call_hooks::Shadow = Arc::new(move |call: Value, r: String| {
                push(&sh, json!(["shadow", call, r]));
                let e = shadow_fails.clone();
                Box::pin(async move { e.map_or(Ok(()), Err) })
            });
            f
        }),
        publish: Arc::new(move |t: &str, d: Map<String, Value>| push(&ev, json!([t, d]))),
        journal: Arc::new(move |k: &str, d: Map<String, Value>| push(&jr, json!([k, d]))),
        workspace: "/HOME/ferb/workspace".into(),
        home: "/HOME/user".into(),
    };
    let hooks = RealHooks::new(parts);
    let tree = cfg.get("tree").cloned().unwrap_or(json!({}));
    let mut results = Vec::new();
    for op in case["ops"].as_array().unwrap() {
        let tool = HookTool(op.get("tool").cloned().unwrap_or(json!({})));
        let g = grant_of(op);
        let args = args_of(op);
        let r = match op["op"].as_str().unwrap() {
            "screen_before" => {
                hooks
                    .screen_before(&g, op["span"].as_str().unwrap(), tool.name(), &args, &tool)
                    .await;
                Value::Null
            }
            "screen_after" => Value::Object(hooks.screen_after(op["span"].as_str().unwrap()).await),
            "told" => Value::Object(hooks.told(op["screen"].as_object().unwrap())),
            "shadow" => {
                let ids: Vec<String> = op["ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_str().unwrap().into())
                    .collect();
                hooks.shadow(
                    op["name"].as_str().unwrap(),
                    &spec_of(op),
                    op["target"].as_object().unwrap(),
                    &ids,
                    &g.task_id,
                    op["words"].as_str().unwrap(),
                    op["span"].as_str().unwrap(),
                );
                for _ in 0..20 {
                    tokio::task::yield_now().await;
                }
                Value::Null
            }
            "redact_image" => {
                let out = ToolOutput::from(ToolResult {
                    image: s(op, "image").map(String::into_bytes),
                    ..ToolResult::default()
                });
                let (o, facts) = hooks.redact_image(out).await;
                json!({"image": utf8(&o.result.image), "facts": facts})
            }
            "preview" => {
                let root = s(op, "tree")
                    .unwrap_or_else(|| g.task_id.split('.').next().unwrap().to_string());
                let locks: Vec<String> = op
                    .get("locks")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|x| x.as_str().unwrap().into()).collect())
                    .unwrap_or_default();
                hooks.preview(
                    &tool,
                    &args,
                    &g,
                    &root,
                    &locks,
                    op.get("observed").and_then(Value::as_object),
                );
                Value::Null
            }
            "point" => {
                let root = tree
                    .get(&g.task_id)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| g.task_id.clone());
                hooks.point(&tool, &args, &g, &root).await;
                Value::Null
            }
            "initial_image" => {
                let out = ToolOutput::from(ToolResult {
                    content: op
                        .get("content")
                        .and_then(Value::as_object)
                        .cloned()
                        .unwrap_or_else(|| json!({"ok": true}).as_object().cloned().unwrap()),
                    read_sources: op
                        .get("reads")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .map(|p| {
                                    (p[0].as_str().unwrap().into(), p[1].as_str().unwrap().into())
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    ..ToolResult::default()
                });
                let o = hooks
                    .initial_image(
                        op["name"].as_str().unwrap(),
                        &tool,
                        &args,
                        &spec_of(op),
                        &g,
                        out,
                    )
                    .await;
                let reads: Vec<Value> = o
                    .result
                    .read_sources
                    .iter()
                    .map(|(a, b)| json!([a, b]))
                    .collect();
                json!({"content": o.result.content, "image": utf8(&o.result.image), "reads": reads})
            }
            "initial_image_given" => {
                hooks.initial_image_given(&g.task_id);
                Value::Null
            }
            "image_provided" => {
                hooks.image_provided(&g.task_id);
                Value::Null
            }
            "watch_changes" => {
                let tree = s(op, "tree").unwrap_or_else(|| g.task_id.clone());
                Value::Object(
                    hooks
                        .watch_changes(&tool, &args, &spec_of(op), &g, &tree)
                        .await,
                )
            }
            "keep_image" => {
                let origins: BTreeSet<String> = op
                    .get("origins")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().map(|x| x.as_str().unwrap().into()).collect())
                    .unwrap_or_default();
                let image = s(op, "image").map(String::into_bytes);
                let site = s(op, "site");
                Value::Object(
                    hooks
                        .keep_image(
                            op["name"].as_str().unwrap(),
                            site.as_deref(),
                            &g,
                            op["span"].as_str().unwrap(),
                            op["content"].as_object().unwrap(),
                            image.as_deref(),
                            &origins,
                        )
                        .await,
                )
            }
            "forget" => {
                let ids: BTreeSet<String> = op["tasks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_str().unwrap().into())
                    .collect();
                hooks.forget(&ids);
                Value::Null
            }
            "looked" => json!(hooks.looked_at(&g.task_id, &g.worker_id)),
            other => panic!("{other}"),
        };
        results.push(r);
    }
    let calls = log.lock().unwrap().clone();
    let events = events.lock().unwrap().clone();
    let journal = journal.lock().unwrap().clone();
    json!({"results": results, "calls": calls, "events": events, "journal": journal})
}

fn same(a: &Value, b: &Value) -> bool {
    ferb_py::json::dumps(a) == ferb_py::json::dumps(b)
}

// decision: DMN-5, DMN-403, HAND-212
#[test]
fn every_hook_does_what_pythons_did() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/gateway/hooks.json"
    ))
    .unwrap();
    let d: Value = serde_json::from_str(&text).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut failed = Vec::new();
    let cases = d["cases"].as_array().unwrap();
    for case in cases {
        let got = rt.block_on(replay(case));
        for k in ["results", "calls", "events", "journal"] {
            if !same(&got[k], &case[k]) {
                failed.push(format!(
                    "{} {k}:\n  got  {}\n  want {}",
                    case["name"], got[k], case[k]
                ));
            }
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} cases differ:\n{}",
        failed.len(),
        cases.len(),
        failed.join("\n")
    );
}
