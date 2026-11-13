//! Replays rust/fixtures/orchestrator/texts.json (made by make.py from the Python build): every pure function of
//! these modules on the same arguments gives what Python returned, byte for byte.
//!
//! decision: ENG-3

mod common;

use std::sync::Arc;

use common::{fixture, fx, real_tempdir, same, unfx, TestPolicy};
use ferb_context::Py;
use ferb_orchestrator::loops::{self, Observed, Pacer, Reason};
use ferb_orchestrator::ports::{CapFacts, Wrap};
use ferb_orchestrator::{
    answer_marks as am, attempts, availability, browsing, language, leftovers, names, plans,
    probes, progress, reply_stream, steering, validate,
};
use serde_json::{json, Map, Value};

fn s(v: &Value) -> String {
    v.as_str()
        .map(String::from)
        .unwrap_or_else(|| panic!("a string: {v}"))
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(s).collect())
        .unwrap_or_default()
}

fn opt(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

fn findings(v: &Value) -> Vec<(String, String)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| (s(&p[0]), s(&p[1])))
        .collect()
}

fn wrap() -> Wrap {
    ferb_orchestrator::ports::wrap_with_id("0123456789ab")
}

/// What Python gave: `{"out": value}` or `{"raised": [type, text]}`.
fn expected(case: &Value) -> Value {
    case.get("out").cloned().unwrap_or(Value::Null)
}

/// The test's capability (make.py `Cap`): an Input of `tab: str = ""` and its required fields.
struct Cap {
    observation: bool,
    probe_safe: bool,
    moves_view: bool,
    holds_tab: bool,
    scopes: Vec<String>,
    probe_keys: Vec<String>,
    required: Vec<String>,
}

fn cap(name: &str) -> Option<Arc<dyn CapFacts>> {
    let base = Cap {
        observation: true,
        probe_safe: true,
        moves_view: false,
        holds_tab: false,
        scopes: vec!["browser:read".into()],
        probe_keys: vec![],
        required: vec![],
    };
    let c = match name {
        "browser.read" => base,
        "browser.snapshot" => Cap {
            required: vec!["tab_ref".into()],
            ..base
        },
        "browser.scroll" => Cap {
            moves_view: true,
            ..base
        },
        "browser.use_tab" => Cap {
            holds_tab: true,
            observation: false,
            ..base
        },
        "browser.act" => Cap {
            observation: false,
            probe_safe: false,
            ..base
        },
        "mac.read" => Cap {
            scopes: vec!["mac:read".into()],
            ..base
        },
        "media.state" => Cap {
            probe_keys: vec!["playing".into()],
            ..base
        },
        _ => return None,
    };
    Some(Arc::new(c))
}

impl CapFacts for Cap {
    fn scopes(&self) -> Vec<String> {
        self.scopes.clone()
    }
    fn identities(&self) -> Vec<String> {
        vec!["owner".into(), "ferb".into()]
    }
    fn observation(&self) -> bool {
        self.observation
    }
    fn probe_safe(&self) -> bool {
        self.probe_safe
    }
    fn moves_view(&self) -> bool {
        self.moves_view
    }
    fn on_screen(&self) -> bool {
        false
    }
    fn holds_tab(&self) -> bool {
        self.holds_tab
    }
    fn probe_keys(&self) -> Vec<String> {
        self.probe_keys.clone()
    }
    fn validate(&self, args: &Value) -> Result<Map<String, Value>, String> {
        // pydantic's words for the two ways these inputs fail (make.py's model: `tab: str = ""`, required strs)
        let obj = args.as_object().cloned().unwrap_or_default();
        let repr = ferb_context::pyvalue::repr(&Py::from_value(args));
        let mut errors = Vec::new();
        if let Some(t) = obj.get("tab") {
            if !t.is_string() {
                let r = ferb_context::pyvalue::repr(&Py::from_value(t));
                errors.push(format!(
                    "tab\n  Input should be a valid string [type=string_type, input_value={r}, input_type={}]\n    For further information visit https://errors.pydantic.dev/2.13/v/string_type",
                    match t { Value::Number(n) if n.is_f64() => "float", Value::Number(_) => "int", Value::Bool(_) => "bool", Value::Null => "NoneType", Value::Array(_) => "list", _ => "dict" }
                ));
            }
        }
        for name in &self.required {
            if !obj.contains_key(name) {
                errors.push(format!(
                    "{name}\n  Field required [type=missing, input_value={repr}, input_type=dict]\n    For further information visit https://errors.pydantic.dev/2.13/v/missing"
                ));
            }
        }
        if !errors.is_empty() {
            let n = errors.len();
            return Err(format!(
                "{n} validation error{} for Input\n{}",
                if n == 1 { "" } else { "s" },
                errors.join("\n")
            ));
        }
        let mut set = Map::new();
        for k in std::iter::once("tab".to_string()).chain(self.required.iter().cloned()) {
            if let Some(v) = obj.get(&k) {
                set.insert(k, v.clone());
            }
        }
        Ok(set)
    }
}

fn probe_of(v: &Value) -> ferb_orchestrator::Probe {
    validate::probe_validate(&Py::from_value(v)).expect("a valid probe")
}

fn py_value(v: &Value) -> Py {
    Py::from_value(v)
}

fn observed(v: &Value) -> Observed {
    if v.as_str() == Some("<MISSING>") {
        Observed::Missing
    } else {
        Observed::Value(py_value(v))
    }
}

fn as_opt(o: &Observed) -> Option<&Py> {
    match o {
        Observed::Missing => None,
        Observed::Value(v) => Some(v),
    }
}

fn run_case(section: &str, case: &Value, root: &str, home: &str) -> Option<Value> {
    let f = case["fn"].as_str().unwrap();
    let a = case.get("args").cloned().unwrap_or(json!([]));
    let policy = TestPolicy;
    let got: Value = match (section, f) {
        ("answer_marks", "headline") => json!(am::headline(&s(&a[0]))),
        ("answer_marks", "mark_unconfirmed") => json!(am::mark_unconfirmed(&s(&a[0]), &s(&a[1]))),
        ("answer_marks", "remove_quote") => json!(am::remove_quote(&s(&a[0]), &s(&a[1]))),
        ("answer_marks", "removes_or_marks") => json!(am::removes_or_marks(&s(&a[0]), &s(&a[1]))),
        ("answer_marks", "settle") => {
            let d = am::settle(&s(&a[0]), &findings(&a[1]));
            json!([
                d.answer,
                d.removed,
                d.marked,
                d.missing,
                d.kept_contradicted
            ])
        }
        ("answer_marks", "unmarked") => json!(am::unmarked(&s(&a[0]), &findings(&a[1]))),
        ("answer_marks", "sentences") => json!(am::sentences(&s(&a[0]))),
        ("answer_marks", "has_text") => json!(am::has_text(&s(&a[0]))),
        ("answer_marks", "only_quoted_changed") => {
            json!(am::only_quoted_changed(&s(&a[0]), &s(&a[1]), &strs(&a[2])))
        }
        ("answer_marks", "without_stray_closing_tags") => {
            json!(am::without_stray_closing_tags(&s(&a[0])))
        }
        ("names", "short_label") => json!(names::short_label(
            &s(&a[0]),
            a[1].as_u64().unwrap() as usize
        )),
        ("names", "shown_name") => json!(names::shown_name(opt(&a[0]).as_deref(), &s(&a[1]))),
        ("language", "internal_terms") => json!(language::internal_terms(&s(&a[0]))),
        ("attempts", "sequence") => {
            let mut at = attempts::Attempts::default();
            let mut out = Vec::new();
            for step in 0..14 {
                if step == 7 {
                    at.allow_more();
                    out.push(json!(["allow_more", at.failed, at.allowed]));
                }
                let due = at.failed_one();
                out.push(json!(["failed_one", due, at.failed, at.allowed]));
            }
            json!(out)
        }
        ("attempts", "check_in_question") => json!(attempts::check_in_question(
            a[0].as_u64().unwrap() as u32,
            &s(&a[1])
        )),
        ("attempts", "stopped_after") => json!(attempts::stopped_after(
            a[0].as_u64().unwrap() as u32,
            a[1].as_bool().unwrap()
        )),
        ("leftovers", "leftover_lines") => json!(leftovers::leftover_lines(
            a[0].as_array().unwrap(),
            &strs(&a[1])
        )),
        ("availability", "build") => {
            let reg = strs(&a[0]).into_iter().collect();
            let probs: Vec<(String, String)> = a[1]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (s(&p[0]), s(&p[1])))
                .collect();
            let av = availability::build(&reg, &probs, &s(&a[2]));
            json!([av.scopes, av.text, av.missing, av.unavailable_line()])
        }
        ("browsing", "with_label") => {
            let text = match s(&a[0]).as_str() {
                "GONE" => browsing::GONE,
                "CHOSEN" => browsing::CHOSEN,
                "GONE_RUN" => browsing::GONE_RUN,
                _ => browsing::CHOSEN_FACT,
            };
            json!(browsing::with_label(text, &s(&a[1])))
        }
        ("browsing", "question") => json!(browsing::question()),
        ("loops", "pacer") => {
            let mut p = Pacer::from_json(a[0].as_str());
            let mut out = Vec::new();
            for step in expected(case).as_array().unwrap() {
                let t = step[1].as_f64().unwrap();
                let r = match step[0].as_str().unwrap() {
                    "ended" => {
                        p.ended(t);
                        Value::Null
                    }
                    "tripped" => json!(p.tripped(t)),
                    "lasted" => json!(p.lasted(t)),
                    _ => json!(p.start_at(t)),
                };
                out.push(json!([step[0], step[1], r, p.to_json()]));
            }
            json!(out)
        }
        ("loops", "round_reason") => {
            let mut fields = a[0].as_object().cloned().unwrap();
            let obs = fields.shift_remove("observed").map(|v| observed(&v));
            json!(loops::round_reason(
                &Reason {
                    fields,
                    observed: obs
                },
                &wrap()
            ))
        }
        ("loops", "loop_line") => {
            let mut step = ferb_orchestrator::Step::new("s", "do");
            step.repeat = Some(ferb_orchestrator::Repeat {
                end_when: s(&a[0]),
                ..Default::default()
            });
            json!(loops::loop_line(&step))
        }
        ("loops", "pause_line") => json!(loops::pause_line(&s(&a[0]), &s(&a[1]))),
        ("loops", "look_only_round") => {
            let calls: Vec<Map<String, Value>> = a[0]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_object().unwrap().clone())
                .collect();
            let probe = if a[1].is_null() {
                None
            } else {
                Some(probe_of(&a[1]))
            };
            json!(loops::look_only_round(
                &calls,
                probe.as_ref(),
                &cap,
                &strs(&a[2])
            ))
        }
        ("probes", "resolve_tool") => {
            let (name, why) = probes::resolve_tool(&s(&a[0]), &strs(&a[1]));
            json!([name, why])
        }
        ("probes", "refusal") => {
            let p = probe_of(&a[0]);
            let c = cap(&p.tool);
            json!(probes::refusal(&p, c.as_deref(), &strs(&a[1]), "owner"))
        }
        ("probes", "args") | ("probes", "value") => {
            let p = ferb_orchestrator::Probe {
                tool: "x".into(),
                args_json: if f == "args" { s(&a[0]) } else { "{}".into() },
                field: "f".into(),
                op: "equals".into(),
                value_json: if f == "value" {
                    s(&a[0])
                } else {
                    "null".into()
                },
            };
            let r = if f == "args" {
                p.args()
                    .map(|m| Py::Dict(ferb_context::pyvalue::Dict(m)).to_value())
            } else {
                p.value().map(|v| v.to_value())
            };
            match r {
                Ok(v) => v,
                Err(e) => return Some(json!({"raised": e})),
            }
        }
        ("probes", "field_value") => {
            let content = py_value(&a[0]);
            match probes::field_value(&content, &s(&a[1])) {
                None => json!("<MISSING>"),
                Some(v) => v.to_value(),
            }
        }
        ("probes", "same") => json!(probes::same(
            as_opt(&observed(&a[0])),
            as_opt(&observed(&a[1]))
        )),
        ("probes", "tripped") => {
            let p = probe_of(&a[0]);
            json!(probes::tripped(&p, as_opt(&observed(&a[1])), as_opt(&observed(&a[2]))).unwrap())
        }
        ("progress", "describe") => {
            let args = a[1].as_object().cloned().unwrap();
            json!(progress::describe(
                &policy,
                &s(&a[0]),
                &args,
                &s(&a[2]),
                opt(&a[3]).as_deref(),
                opt(&a[4]).as_deref()
            ))
        }
        ("progress", "waiting_for") => json!(progress::waiting_for(
            &s(&a[0]),
            &s(&a[1]),
            a[2].as_i64().unwrap()
        )),
        ("progress", "app_name") => {
            let args = a[0].as_object().cloned().unwrap();
            let installed: Vec<(String, String)> = a[2]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (s(&p[0]), s(&p[1])))
                .collect();
            let running: Vec<(String, String, i64)> = a[3]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| (s(&p[0]), s(&p[1]), p[2].as_i64().unwrap()))
                .collect();
            json!(progress::app_name(
                &args,
                opt(&a[1]).as_deref(),
                &installed,
                &move || running.clone()
            ))
        }
        ("plans", fun) => return Some(run_plans(fun, &a, root, home)),
        ("steering", "acknowledgement") => {
            json!(steering::acknowledgement(
                &policy,
                &s(&a[0]),
                a[1].as_bool().unwrap(),
                opt(&a[2]).as_deref()
            ))
        }
        ("steering", "not_covered") => json!(steering::not_covered(a[0].as_bool().unwrap())),
        ("steering", "not_typed") => json!(steering::not_typed()),
        ("steering", "just_ended") => json!(steering::JUST_ENDED),
        ("steering", "scope_line") => {
            let scope = validate::send_scope_validate(&py_value(&a[0])).unwrap();
            json!(steering::scope_line(&scope))
        }
        ("reply_stream", "draft") | ("reply_stream", "restart") => {
            let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
            let sink = events.clone();
            let emit = Arc::new(move |e: Value| sink.lock().unwrap().push(e));
            let mut draft = reply_stream::ReplyDraft::new(
                Arc::new(ferb_sanitize::Redactor::new(&[], &[])),
                Arc::new(TestPolicy),
                emit,
                None,
            );
            if f == "restart" {
                let raw = s(&a[0]);
                draft.feed("{\"kind\": \"reply\", \"answers");
                draft.restart();
                draft.feed(&raw);
                draft.finish(&raw);
                draft.restart();
            } else {
                for piece in a[0].as_array().unwrap() {
                    draft.feed(&s(piece));
                }
                if let Some(fin) = a[1].as_str() {
                    draft.finish(fin);
                }
            }
            let out = events.lock().unwrap().clone();
            json!(out)
        }
        _ => panic!("no replay for {section}.{f}"),
    };
    Some(got)
}

fn plan_of(v: &Value) -> Option<ferb_orchestrator::Plan> {
    if v.is_null() {
        None
    } else {
        Some(validate::plan_validate(&Py::from_value(v)).expect("a valid plan"))
    }
}

fn parts_of(v: &Value) -> Vec<ferb_orchestrator::Part> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| validate::part_validate(&Py::from_value(p)).unwrap())
        .collect()
}

fn facts_of(v: &Value) -> Vec<plans::FileFact> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|f| plans::FileFact {
            path: s(&f["path"]),
            bytes: f["bytes"].as_u64().unwrap(),
            text: f["text"].as_bool().unwrap(),
            tokens_upper_bound: f["tokens_upper_bound"].as_u64().unwrap(),
        })
        .collect()
}

fn inventory_json(m: &plans::Inventory) -> Value {
    json!([
        m.files.iter().map(|f| f.to_json()).collect::<Vec<_>>(),
        m.stopped,
        m.counted,
        m.bound,
        m.parts
            .iter()
            .map(|(n, c)| json!([n, c]))
            .collect::<Vec<_>>(),
        m.unreached
    ])
}

fn run_plans(f: &str, a: &Value, root: &str, home: &str) -> Value {
    match f {
        "problems" => json!(plans::problems(
            &plan_of(&a[0]).unwrap(),
            &facts_of(&a[1]),
            a[2].as_u64().unwrap()
        )),
        "answer_steps" => json!(plans::answer_steps(
            plan_of(&a[0]).as_ref(),
            &parts_of(&a[1])
        )),
        "part_problems" => json!(plans::part_problems(
            &parts_of(&a[0]),
            plan_of(&a[1]).as_ref(),
            a[2].as_u64().unwrap() as usize
        )),
        "card" => json!(plans::card(&parts_of(&a[0]))),
        "step_label" => {
            let mut step = ferb_orchestrator::Step::new("s1", "do");
            step.label = s(&a[0]);
            step.goal = s(&a[1]);
            json!(plans::step_label(&step))
        }
        "steps_done" => {
            let (done, total) = plans::steps_done(&a[0]);
            json!({"done": done, "total": total})
        }
        "looking_at" => {
            let paths: Vec<String> = strs(&unfx(&a[0], root));
            fx(&json!(plans::looking_at(&paths, home)), root)
        }
        "inventory" => {
            let a = unfx(a, root);
            let denied = |p: &str| ferb_fs::pypath::name(p) == "b.md";
            let m = plans::inventory(
                &strs(&a[0]),
                Some(&denied),
                &strs(&a[1]),
                a[2].as_u64().unwrap() as usize,
                a[3].as_u64(),
                home,
            );
            fx(&inventory_json(&m), root)
        }
        "too_big" => {
            let i = &a[0];
            let m = plans::Inventory {
                files: facts_of(&i[0]),
                stopped: opt(&i[1]),
                counted: i[2].as_u64().unwrap(),
                bound: i[3].as_u64().unwrap(),
                parts: i[4]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| (s(&p[0]), p[1].as_u64().unwrap()))
                    .collect(),
                unreached: strs(&i[5]),
            };
            json!(plans::too_big(&m, &|p: &str| p.replace("/FX", "~")))
        }
        "walked" => {
            let a = unfx(a, root);
            let plan = plan_of(&a[0]).unwrap();
            let normal = |v: &str| ferb_fs::pypath::expanduser(v, home);
            fx(&json!(plans::walked(&plan, &strs(&a[1]), &normal)), root)
        }
        "resolve_inputs" => {
            let a = unfx(a, root);
            let mut plan = plan_of(&a[0]).unwrap();
            let normal = |v: &str| ferb_fs::pypath::expanduser(v, home);
            plans::resolve_inputs(&mut plan, &facts_of(&a[1]), &normal);
            fx(
                &json!(plan
                    .steps
                    .iter()
                    .map(|st| st.inputs.clone())
                    .collect::<Vec<_>>()),
                root,
            )
        }
        _ => panic!("no replay for plans.{f}"),
    }
}

/// make.py's invented tree, under `root` (the fixture's /FX).
fn build_tree(root: &std::path::Path) {
    use std::fs;
    let tree = root.join("tree");
    fs::create_dir_all(tree.join("notes/deep/deeper")).unwrap();
    fs::create_dir_all(tree.join("notes/empty")).unwrap();
    fs::create_dir_all(tree.join("media")).unwrap();
    fs::create_dir_all(root.join("user")).unwrap();
    fs::write(tree.join("notes/a.txt"), "hello ".repeat(50)).unwrap();
    fs::write(tree.join("notes/b.md"), "é".repeat(100)).unwrap();
    fs::write(tree.join("notes/deep/c.txt"), "x".repeat(300)).unwrap();
    fs::write(tree.join("notes/deep/deeper/d.txt"), "y".repeat(50)).unwrap();
    let mut cut = vec![b'a'; 8191];
    cut.extend("é".as_bytes());
    fs::write(tree.join("notes/cut.txt"), cut).unwrap();
    fs::write(tree.join("notes/bad.txt"), b"ok\xff\xfe").unwrap();
    fs::write(tree.join("media/v.bin"), [0u8; 64]).unwrap();
    fs::write(tree.join("media/z.txt"), "z".repeat(2000)).unwrap();
    std::os::unix::fs::symlink(tree.join("notes/a.txt"), tree.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(tree.join("media"), tree.join("notes/linkdir")).unwrap();
    fs::write(tree.join("single.txt"), "single").unwrap();
}

/// Cases whose difference from Python is deliberate (ENG-6), by section and function, with why.
fn deliberate(section: &str, f: &str, case: &Value) -> bool {
    // probes.py:83: `int('²')` raises ValueError out of `field_value` (and out of the loop's probe read); Rust reads
    // a digit that isn't decimal as MISSING, which trips the probe like any missing field.
    section == "probes" && f == "field_value" && case.get("raised").is_some()
}

#[test]
fn texts_match_python() {
    let all = fixture("texts.json");
    let (_dir, real) = real_tempdir();
    build_tree(&real);
    let root = real.to_string_lossy().into_owned();
    let home = format!("{root}/user");
    let mut total = 0;
    let mut differ = Vec::new();
    for (section, cases) in all.as_object().unwrap() {
        for case in cases.as_array().unwrap() {
            total += 1;
            let f = case["fn"].as_str().unwrap();
            if deliberate(section, f, case) {
                continue;
            }
            let got = run_case(section, case, &root, &home).unwrap();
            let want = if let Some(raised) = case.get("raised") {
                json!({"raised": raised[1]})
            } else {
                expected(case)
            };
            if !same(&got, &want) {
                differ.push(format!(
                    "{section}.{f} args={}\n  python: {want}\n  rust:   {got}",
                    case.get("args").unwrap_or(&Value::Null)
                ));
            }
        }
    }
    let shown: Vec<&String> = differ.iter().take(25).collect();
    assert!(
        differ.is_empty(),
        "{} of {total} differ:\n{}",
        differ.len(),
        shown
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    eprintln!("texts: {total} cases, none differ");
}
