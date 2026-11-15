//! One task's permanent record composed from its recorded spans and journal facts (`ferb/trace/compose.py`; L1,
//! P26, RQ1-RQ4). It calls no model and adds nothing the records don't say; a worker's words stay a claim.
//!
//! Input: the `trace.get` shape `{task_id, spans: [forest of span objects], outcome: SpanStore.outcome()}` as
//! `ferb_store::spans::node_json` and `SpanStore::outcome` write it.

use std::collections::{BTreeMap, BTreeSet};

use ferb_sanitize::{origin, untrusted};
use serde_json::{Map, Value};

use crate::lines::{clip, clip_value, duration, no_on_screen_action, plural, CameForward};
use crate::py::{
    self, float, g, gd, has, int_of, is_int, items, json_obj, map, num, obj, opt_float, or,
    round_digits, str_of, string, text, total_seconds, truthy, EMPTY,
};

/// The kinds of span that count as a model call. model.dispatch and model.resources are kept so an older task's
/// record still reads as it did.
pub const MODEL_KINDS: [&str; 5] = [
    "model.turn",
    "model.dispatch",
    "model.question",
    "model.resources",
    "worker",
];
pub const PLANNER_KINDS: [&str; 4] = [
    "model.turn",
    "model.dispatch",
    "model.question",
    "model.resources",
];
pub const OPEN: &str = "⏹";
pub const OPEN_TEXT: &str = "did not finish (interrupted)";
pub const RERUN: &str = "↻";
const NO_SCREEN: [&str; 2] = ["shell.", "fs."];
const OBSERVE_KINDS: [&str; 3] = ["read", "search", "mailbox_read"];
const ORIGIN_KINDS: [&str; 4] = ["task", "tool.call", "context.source", "context.exposure"];
pub const PASTED: &str = "[pasted text]";

/// `walk(nodes)`: every span, parents before their children.
pub fn walk(nodes: &[Value]) -> Vec<&Value> {
    let mut out = Vec::new();
    fn visit<'a>(nodes: &'a [Value], out: &mut Vec<&'a Value>) {
        for n in nodes {
            out.push(n);
            visit(items(n, "children"), out);
        }
    }
    visit(nodes, &mut out);
    out
}

/// The spans of a forest in start order (their start text, as Python sorts them; ties keep tree order).
pub fn sorted_nodes(forest: &[Value]) -> Vec<&Value> {
    let mut nodes = walk(forest);
    nodes.sort_by(|a, b| text(a, "started_at").cmp(text(b, "started_at")));
    nodes
}

fn nonempty(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}

/// `_secs(start, end)`: seconds to a tenth, `None` unless both ends are recorded.
pub fn secs(start: Option<&str>, end: Option<&str>) -> Option<f64> {
    let (a, b) = (
        start.filter(|s| !s.is_empty())?,
        end.filter(|s| !s.is_empty())?,
    );
    Some(round_digits(total_seconds(a, b)?, 1))
}

fn planner_text(n: &Value) -> String {
    let out = obj(n, "output");
    let empty = Value::String(String::new());
    let kind = text(n, "kind");
    let rstrip = |s: String| s.trim_end_matches([' ', '—']).to_owned();
    match kind {
        "model.turn" => {
            let d = json_obj(g(out, "text"));
            let task = g(&d, "task");
            let spec = if task.is_object() { task } else { &EMPTY };
            if g(&d, "kind").as_str() == Some("task") && truthy(g(spec, "goal")) {
                return format!("task — {}", clip_value(g(spec, "goal"), 160));
            }
            let decided = string("decided");
            rstrip(format!(
                "{} — {}",
                str_of(or(g(&d, "kind"), &decided)),
                clip_value(gd(&d, "text", &empty), 160)
            ))
        }
        "model.dispatch" => {
            let d = json_obj(g(out, "text"));
            let detail = or(
                g(&d, "instructions"),
                or(g(&d, "question"), or(g(&d, "stop_reason"), &empty)),
            );
            let decided = string("decided");
            rstrip(format!(
                "{} — {}",
                str_of(or(g(&d, "action"), &decided)),
                clip_value(detail, 160)
            ))
        }
        "model.resources" => {
            let d = json_obj(g(out, "text"));
            if !truthy(g(&d, "status")) {
                return if truthy(g(out, "text")) {
                    format!("after cleanup: {}", clip_value(g(out, "text"), 160))
                } else {
                    "after cleanup".to_owned()
                };
            }
            let mut s = format!("after cleanup: {}", str_of(g(&d, "status")));
            if truthy(g(&d, "summary")) {
                s.push_str(&format!(" — {}", clip_value(g(&d, "summary"), 160)));
            }
            s
        }
        _ => {
            let d = json_obj(g(out, "decision"));
            if truthy(g(&d, "ask_owner")) {
                return format!("asks you: {}", clip_value(gd(&d, "question", &empty), 160));
            }
            format!(
                "answered the worker: {}",
                clip_value(gd(&d, "answer", &empty), 160)
            )
        }
    }
}

fn eq_false(m: &Value) -> bool {
    match m {
        Value::Bool(b) => !*b,
        Value::Number(_) => num(m) == 0.0,
        _ => false,
    }
}

fn eq_true(m: &Value) -> bool {
    match m {
        Value::Bool(b) => *b,
        Value::Number(_) => num(m) == 1.0,
        _ => false,
    }
}

fn checker_text(n: &Value, total: i64) -> (&'static str, String) {
    if truthy(g(n, "error")) {
        return (
            "✗",
            format!("check failed: {}", clip_value(g(n, "error"), 120)),
        );
    }
    let result = json_obj(g(obj(n, "output"), "result"));
    let results = items(&result, "results");
    let met: Vec<&Value> = results.iter().map(|r| g(r, "met")).collect();
    let empty = string("");
    if !results.is_empty() && met.iter().all(|m| matches!(m, Value::Bool(true))) {
        let evidence: Vec<String> = results
            .iter()
            .map(|r| clip_value(gd(r, "evidence", &empty), 60))
            .collect();
        return ("✓", format!("verified: {}", evidence.join("; ")));
    }
    if met.iter().all(|m| m.is_null()) {
        let why = results
            .first()
            .map(|r| clip_value(gd(r, "evidence", &empty), 60))
            .unwrap_or_default();
        let count = if total != 0 {
            total
        } else {
            results.len() as i64
        };
        let mut s = format!(
            "checked nothing — {} unknown",
            plural(count, "condition", "conditions")
        );
        if !why.is_empty() {
            s.push_str(&format!(" ({why})"));
        }
        return ("⚠", s);
    }
    let bad = met.iter().filter(|m| eq_false(m)).count();
    let unknown = met.iter().filter(|m| m.is_null()).count();
    let good = met.iter().filter(|m| eq_true(m)).count();
    (
        if bad > 0 { "✗" } else { "⚠" },
        format!("{bad} not met, {unknown} unknown, {good} verified"),
    )
}

fn on_screen(action: &Value, kinds: &BTreeMap<String, Value>) -> bool {
    if has(action, "on_screen") {
        return truthy(g(action, "on_screen"));
    }
    let kind = kinds
        .get(&str_of(g(action, "span_id")))
        .unwrap_or(&Value::Null);
    let tool = str_of(gd(action, "tool", &string("")));
    let observed = kind.as_str().is_some_and(|k| OBSERVE_KINDS.contains(&k));
    !NO_SCREEN.iter().any(|p| tool.starts_with(p)) && !observed
}

fn tree_of(task_id: &str) -> &str {
    task_id.split('.').next().unwrap_or("")
}

fn sorted_names(v: &Value) -> Vec<String> {
    let mut names: Vec<String> = match v {
        Value::Object(m) => m.keys().cloned().collect(),
        Value::Array(a) => a.iter().map(str_of).collect(),
        _ => Vec::new(),
    };
    names.sort();
    names
}

fn strings(v: Vec<String>) -> Value {
    Value::Array(v.into_iter().map(Value::String).collect())
}

fn desktop(nodes: &[&Value], task_id: &str, ended: &Value) -> Value {
    let summary = nodes
        .iter()
        .rev()
        .find(|n| text(n, "kind") == "disturbance.summary")
        .copied();
    let raw = summary.map_or(&Value::Null, |s| g(obj(s, "output"), "changes"));
    let tree = tree_of(task_id);
    let mut near: Vec<Value> = Vec::new();
    let mut none_near: Vec<Value> = Vec::new();
    let mut paused: Vec<(Value, Value)> = Vec::new();
    let mut kinds: BTreeMap<String, Value> = BTreeMap::new();
    for n in nodes.iter().filter(|n| text(n, "kind") == "tool.call") {
        kinds.insert(text(n, "id").to_owned(), g(obj(n, "input"), "kind").clone());
    }
    let mut forward = CameForward::new();
    for n in nodes {
        let out = obj(n, "output");
        match text(n, "kind") {
            "disturbance.baseline" => forward.baseline(obj(out, "snapshot")),
            "disturbance.paused" => paused.push((g(n, "started_at").clone(), Value::Null)),
            "disturbance.resumed" => {
                if let Some(last) = paused.last_mut() {
                    if last.1.is_null() {
                        last.1 = g(n, "started_at").clone();
                    }
                }
            }
            "disturbance.change" => {
                let changes = obj(out, "changes");
                let actions = items(out, "actions");
                let what = forward.change(changes);
                let tab_sound = has(changes, "tab_sound");
                let screen: Vec<&Value> = actions
                    .iter()
                    .filter(|a| on_screen(a, &kinds) || tab_sound)
                    .collect();
                let task_of = |a: &Value| {
                    let fallback = string(tree);
                    let t = or(g(a, "task_id"), &fallback);
                    tree_of(&str_of(t)).to_owned()
                };
                let names = |same: bool| -> Vec<String> {
                    let set: BTreeSet<String> = screen
                        .iter()
                        .filter(|a| (task_of(a) == tree) == same)
                        .map(|a| str_of(gd(a, "tool", &string("?"))))
                        .collect();
                    set.into_iter().collect()
                };
                let (own, other) = (names(true), names(false));
                if !own.is_empty() || !other.is_empty() {
                    near.push(map(vec![
                        ("at", g(n, "started_at").clone()),
                        ("changed", strings(sorted_names(changes))),
                        ("tools", strings(own)),
                        ("other_task_tools", strings(other)),
                    ]));
                } else if !what.is_empty() && actions.iter().all(|a| has(a, "on_screen")) {
                    none_near.push(map(vec![
                        ("at", g(n, "started_at").clone()),
                        (
                            "text",
                            string(&no_on_screen_action(
                                &what,
                                g(out, "processes"),
                                task_id,
                                g(out, "on_screen_in_task"),
                                g(out, "last_on_screen_ended_s"),
                            )),
                        ),
                    ]));
                }
            }
            _ => {}
        }
    }
    if let Some(last) = paused.last_mut() {
        if last.1.is_null() {
            last.1 = summary.map_or_else(|| ended.clone(), |s| g(s, "started_at").clone());
        }
    }
    let changes = match raw {
        Value::Array(a) => a.len() as i64,
        Value::Object(m) => m.len() as i64,
        other => int_of(other),
    };
    map(vec![
        ("changes", Value::from(changes)),
        ("near_ferb", Value::Array(near)),
        ("no_on_screen_action", Value::Array(none_near)),
        (
            "paused",
            Value::Array(
                paused
                    .into_iter()
                    .map(|(at, until)| map(vec![("at", at), ("until", until)]))
                    .collect(),
            ),
        ),
    ])
}

fn worker_text(n: &Value) -> (&'static str, String) {
    let out = obj(n, "output");
    let report = json_obj(g(out, "result"));
    let failed = string("failed");
    let none = string("");
    let status = or(
        g(out, "status"),
        or(
            g(&report, "status"),
            if truthy(g(n, "error")) {
                &failed
            } else {
                &none
            },
        ),
    );
    let summary = if truthy(&report) {
        str_of(or(g(&report, "summary"), &none))
    } else {
        str_of(or(g(out, "result"), &none))
    };
    let calls = walk(items(n, "children"))
        .iter()
        .filter(|c| text(c, "kind") == "tool.call")
        .count() as i64;
    let head = if calls > 0 {
        format!("{} · ", plural(calls, "tool call", "tool calls"))
    } else {
        String::new()
    };
    let status_text = str_of(status);
    let claim = if status.as_str() == Some("cancelled") {
        "stopped: ".to_owned()
    } else if truthy(status) {
        format!("worker reported {status_text}: ")
    } else {
        "worker reported: ".to_owned()
    };
    let bad = matches!(status.as_str(), Some("failed" | "blocked" | "cancelled"))
        || truthy(g(n, "error"));
    (
        if bad { "✗" } else { "" },
        format!("{head}{claim}{}", clip(&summary, 160)),
    )
}

/// `WRAPPED.sub(repl, text)`: FERB's own wrapper (`<untrusted-data id="X">` ... `</untrusted-data id="X">`)
/// replaced wherever it appears.
pub fn replace_wrapped(text: &str, repl: &str) -> String {
    const OPEN_TAG: &str = "<untrusted-data id=\"";
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    let mut from = 0;
    while let Some(rel) = text[from..].find(OPEN_TAG) {
        let start = from + rel;
        let id_at = start + OPEN_TAG.len();
        let id_len = text[id_at..]
            .bytes()
            .take_while(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
            .count();
        let after = id_at + id_len;
        if id_len > 0 && text[after..].starts_with("\">") {
            let id = &text[id_at..after];
            let close = format!("</untrusted-data id=\"{id}\">");
            if let Some(c) = text[after + 2..].find(&close) {
                let end = after + 2 + c + close.len();
                out.push_str(&text[pos..start]);
                out.push_str(repl);
                pos = end;
                from = end;
                continue;
            }
        }
        from = start + 1;
    }
    out.push_str(&text[pos..]);
    out
}

fn squeezed(s: &str) -> String {
    py::squeeze(s)
}

/// What the owner asked (`_request_text`): the words they typed, pasted parts as a marker.
pub fn request_text(outcome: &Value) -> String {
    let request = obj(outcome, "request");
    let messages = items(request, "messages");
    let words = str_of(or(g(request, "authority_words"), &string("")));
    let typed = squeezed(&replace_wrapped(
        &untrusted::without_ferb_question(&words),
        PASTED,
    ));
    let pasted = untrusted::holds_pasted(&str_of(or(g(request, "text"), &string(""))));
    if !typed.is_empty() || (pasted && !g(request, "authority_words").is_null()) {
        return if pasted && !typed.contains(PASTED) {
            py::strip(&format!("{typed} {PASTED}")).to_owned()
        } else {
            typed
        };
    }
    let last = messages
        .last()
        .map_or(&Value::Null, |m| g(obj(m, "data"), "text"));
    let blank = string("");
    let chosen = or(
        last,
        or(
            g(request, "text"),
            or(g(obj(outcome, "task_spec"), "goal"), &blank),
        ),
    );
    squeezed(&replace_wrapped(&str_of(chosen), PASTED))
}

struct Step {
    at: Value,
    role: &'static str,
    seconds: Option<f64>,
    text: String,
    mark: String,
}

fn sum_ints(values: impl Iterator<Item = i64>) -> i64 {
    values.sum()
}

/// A sum as Python makes it: an int while every term is, else a float added left to right.
fn py_sum(values: &[&Value]) -> Value {
    if values.iter().all(|v| is_int(v)) {
        return Value::from(values.iter().map(|v| py::int_val(v)).sum::<i64>());
    }
    let mut total = 0.0;
    for v in values {
        total += num(v);
    }
    float(total)
}

/// `round(value, 6)`.
fn round6(v: Value) -> Value {
    match v {
        Value::Number(n) if n.is_f64() => float(round_digits(n.as_f64().unwrap_or(0.0), 6)),
        other => other,
    }
}

/// `compose(trace)`: the episode of one task.
pub fn compose(trace: &Value) -> Value {
    let task_id = text(trace, "task_id").to_owned();
    let forest = items(trace, "spans");
    let nodes = sorted_nodes(forest);
    let outcome = obj(trace, "outcome");
    let spec = obj(outcome, "task_spec");
    let conditions_text: Vec<Value> = items(spec, "success_conditions")
        .iter()
        .map(|c| gd(c, "text", &string("")).clone())
        .collect();
    let is_task = |n: &&Value| text(n, "kind") == "task";
    let root: &Value = forest
        .iter()
        .rev()
        .find(|n| is_task(n) && g(n, "task_id").as_str() == Some(task_id.as_str()))
        .or_else(|| forest.iter().find(is_task))
        .unwrap_or(&EMPTY);
    let result = obj(outcome, "result");
    let row: &Value = items(outcome, "tasks")
        .iter()
        .find(|t| g(t, "id").as_str() == Some(task_id.as_str()))
        .unwrap_or(&EMPTY);
    let records = items(outcome, "outcome_records");
    let checked: Option<&Vec<Value>> = records
        .iter()
        .rev()
        .find_map(|r| g(r, "verification").as_array());
    let checked_count = match checked {
        Some(c) if !c.is_empty() => c.len() as i64,
        _ => conditions_text.len() as i64,
    };

    let mut steps: Vec<Step> = Vec::new();
    let mut retry_starts: Vec<String> = Vec::new();
    let mut reruns = 0usize;
    for n in &nodes {
        let kind = text(n, "kind");
        let inp = obj(n, "input");
        if kind == "verify.rerun" {
            if let Some(last) = steps.iter_mut().rev().find(|s| s.role == "Checker") {
                last.mark = RERUN.to_owned();
                last.text = format!(
                    "its model window overflowed, so the same check ran again without the recorded calls: {}",
                    clip(&str_of(or(g(inp, "reason"), &string(""))), 120)
                );
            }
            reruns += 1;
            continue;
        }
        let role = if PLANNER_KINDS.contains(&kind)
            && (kind != "model.turn" || g(inp, "role").as_str() == Some("supervisor"))
        {
            "Planner"
        } else if kind == "worker" {
            if g(inp, "role").as_str() == Some("verifier") {
                "Checker"
            } else {
                "Worker"
            }
        } else {
            continue;
        };
        let started = text(n, "started_at");
        let (mark, body): (String, String);
        if !truthy(g(n, "ended_at")) {
            (mark, body) = (OPEN.to_owned(), OPEN_TEXT.to_owned());
        } else if role == "Planner" && truthy(g(n, "error")) {
            (mark, body) = (
                "✗".to_owned(),
                format!("failed: {}", clip_value(g(n, "error"), 120)),
            );
        } else if role == "Planner" {
            mark = String::new();
            body = planner_text(n);
            if kind == "model.dispatch"
                && g(&json_obj(g(obj(n, "output"), "text")), "action").as_str() == Some("retry")
            {
                retry_starts.push(started.to_owned());
            }
        } else if role == "Checker" {
            let (m, mut t) = checker_text(n, checked_count);
            if g(&json_obj(g(obj(n, "output"), "result")), "next").as_str() == Some("retry") {
                retry_starts.push(started.to_owned());
            }
            if steps
                .last()
                .is_some_and(|s| s.mark == RERUN && s.role == "Checker")
            {
                t = format!("rerun of the same check: {t}");
            }
            (mark, body) = (m.to_owned(), t);
        } else {
            let (m, t) = worker_text(n);
            (mark, body) = (m.to_owned(), t);
        }
        steps.push(Step {
            at: g(n, "started_at").clone(),
            role,
            seconds: secs(Some(started), nonempty(g(n, "ended_at"))),
            text: body,
            mark,
        });
    }
    for i in 0..steps.len() {
        if steps[i].role == "Worker" && steps[i].mark != OPEN {
            let next = (i + 1..steps.len())
                .find(|&j| matches!(steps[j].role, "Worker" | "Checker") && steps[j].mark != RERUN);
            if let Some(j) = next {
                if steps[j].role == "Checker" && steps[j].mark == "✓" {
                    steps[i]
                        .text
                        .push_str(&format!(" — verified in step {}", j + 1));
                }
            }
        }
    }

    let times: Vec<&str> = nodes
        .iter()
        .flat_map(|n| [g(n, "started_at"), g(n, "ended_at")])
        .filter_map(nonempty)
        .collect();
    let started = times.iter().min().copied();
    let ended = g(root, "ended_at").clone();
    let mut problems: Vec<String> = Vec::new();
    if !retry_starts.is_empty() {
        let spent = duration(secs(Some(&retry_starts[0]), nonempty(&ended)));
        problems.push(format!(
            "⚠ {} ({spent} spent after {})",
            plural(retry_starts.len() as i64, "retry", "retries"),
            if retry_starts.len() == 1 {
                "it"
            } else {
                "the first"
            }
        ));
    }
    if reruns > 0 {
        problems.push(format!(
            "{RERUN} {} ran again without the recorded calls (its model window overflowed)",
            plural(reruns as i64, "check", "checks")
        ));
    }
    let unknown = steps
        .iter()
        .filter(|s| s.role == "Checker" && s.mark == "⚠")
        .count();
    if unknown > 0 {
        problems.push(format!(
            "⚠ {} left conditions unknown",
            plural(unknown as i64, "check", "checks")
        ));
    }
    let failed = steps.iter().filter(|s| s.mark == "✗").count();
    if failed > 0 {
        problems.push(format!(
            "✗ {} failed",
            plural(failed as i64, "step", "steps")
        ));
    }
    let unfinished = steps.iter().filter(|s| s.mark == OPEN).count();
    if unfinished > 0 {
        problems.push(format!(
            "{OPEN} {} did not finish",
            plural(unfinished as i64, "step", "steps")
        ));
    }
    let refused = nodes
        .iter()
        .filter(|n| text(n, "kind") == "tool.refused")
        .count();
    if refused > 0 {
        problems.push(format!(
            "⚠ {} refused",
            plural(refused as i64, "tool call", "tool calls")
        ));
    }

    let usage_out: Vec<&Value> = nodes
        .iter()
        .filter(|n| text(n, "kind") == "model.usage")
        .map(|n| obj(n, "output"))
        .collect();
    let priced: Vec<&Value> = usage_out
        .iter()
        .map(|u| g(u, "cost_usd"))
        .filter(|c| !c.is_null())
        .collect();
    let tokens = |key: &str| {
        sum_ints(
            usage_out
                .iter()
                .map(|u| int_of(or(g(u, key), &Value::from(0)))),
        )
    };
    let model_calls = nodes
        .iter()
        .filter(|n| MODEL_KINDS.contains(&text(n, "kind")))
        .count() as i64;
    let mut unpriced: Vec<String> = usage_out
        .iter()
        .filter(|u| g(u, "cost_usd").is_null())
        .map(|u| str_of(or(g(u, "model"), &string("an unnamed model"))))
        .collect();
    unpriced.sort();
    unpriced.dedup();
    let mut billing: Vec<String> = usage_out
        .iter()
        .filter(|u| truthy(g(u, "billing")))
        .map(|u| str_of(g(u, "billing")))
        .collect();
    billing.sort();
    billing.dedup();
    let usage = map(vec![
        ("model_calls", Value::from(model_calls)),
        ("usage_records", Value::from(usage_out.len() as i64)),
        ("input_tokens", Value::from(tokens("input_tokens"))),
        (
            "cached_input_tokens",
            Value::from(tokens("cached_input_tokens")),
        ),
        (
            "cache_write_tokens",
            Value::from(tokens("cache_write_tokens")),
        ),
        (
            "cache_write_5m_tokens",
            Value::from(tokens("cache_write_5m_tokens")),
        ),
        (
            "cache_write_1h_tokens",
            Value::from(tokens("cache_write_1h_tokens")),
        ),
        ("output_tokens", Value::from(tokens("output_tokens"))),
        (
            "cost_usd",
            if priced.is_empty() {
                Value::Null
            } else {
                round6(py_sum(&priced))
            },
        ),
        ("unpriced_models", strings(unpriced)),
        ("billing", strings(billing)),
    ]);

    let mut tools_used: Map<String, Value> = Map::new();
    let mut point_checks: Map<String, Value> = Map::new();
    let mut origins: BTreeSet<String> = BTreeSet::new();
    let mut tool_origins: BTreeSet<String> = BTreeSet::new();
    for n in &nodes {
        let kind = text(n, "kind");
        if ORIGIN_KINDS.contains(&kind) {
            let stored = g(n, "origin").as_str();
            let held = if kind == "task" {
                origin::split(stored)
            } else {
                origin::recorded(stored, !g(n, "output").is_null())
            };
            if kind == "tool.call" {
                tool_origins.extend(held.iter().cloned());
            }
            origins.extend(held);
        }
        if kind == "tool.call" {
            let inp = obj(n, "input");
            let name = str_of(gd(inp, "tool", &string("?")));
            let entry = tools_used.entry(name).or_insert_with(|| {
                map(vec![("calls", Value::from(0)), ("errors", Value::from(0))])
            });
            let bump = |v: &mut Value, key: &str, by: i64| {
                if let Some(slot) = v.get_mut(key) {
                    *slot = Value::from(slot.as_i64().unwrap_or(0) + by);
                }
            };
            bump(entry, "calls", 1);
            bump(entry, "errors", i64::from(truthy(g(n, "error"))));
            for check in items(inp, "point_checks") {
                let slot = point_checks.entry(str_of(check)).or_insert(Value::from(0));
                *slot = Value::from(slot.as_i64().unwrap_or(0) + 1);
            }
        }
    }

    let conditions: Vec<Value> = match checked {
        Some(c) if !c.is_empty() => c
            .iter()
            .map(|v| {
                map(vec![
                    ("text", gd(v, "condition", &string("")).clone()),
                    ("met", g(v, "met").clone()),
                    ("evidence", gd(v, "evidence", &string("")).clone()),
                ])
            })
            .collect(),
        _ => conditions_text
            .iter()
            .map(|t| {
                map(vec![
                    ("text", t.clone()),
                    ("met", Value::Null),
                    ("evidence", string("")),
                ])
            })
            .collect(),
    };
    let answer = records
        .iter()
        .rev()
        .find(|r| has(r, "text"))
        .map(|r| str_of(g(r, "text")))
        .unwrap_or_else(|| str_of(or(g(result, "summary"), &string(""))));
    let blank = string("");
    let goal = or(g(spec, "goal"), or(g(row, "goal"), &blank));
    let status = str_of(or(
        g(result, "status"),
        or(g(row, "status"), &string("unknown")),
    ));
    let seconds = secs(started, nonempty(&ended));
    let step_values: Vec<Value> = steps
        .iter()
        .map(|s| {
            map(vec![
                ("at", s.at.clone()),
                ("role", string(s.role)),
                ("seconds", opt_float(s.seconds)),
                ("text", string(&s.text)),
                ("mark", string(&s.mark)),
            ])
        })
        .collect();
    map(vec![
        ("task_id", string(&task_id)),
        ("request_id", g(obj(outcome, "request"), "id").clone()),
        ("request_text", string(&request_text(outcome))),
        ("goal", goal.clone()),
        ("status", string(&status)),
        ("verified", g(result, "verified").clone()),
        ("started_at", started.map_or(Value::Null, string)),
        ("ended_at", ended.clone()),
        ("seconds", opt_float(seconds)),
        ("answer", string(&answer)),
        ("how", string(&str_of(or(g(result, "how"), &string(""))))),
        ("conditions", Value::Array(conditions)),
        ("steps", Value::Array(step_values)),
        ("problems", strings(problems)),
        ("tools", Value::Object(tools_used)),
        ("point_checks", Value::Object(point_checks)),
        ("origins", strings(origins.into_iter().collect())),
        ("tool_origins", strings(tool_origins.into_iter().collect())),
        ("usage", usage),
        ("desktop", desktop(&nodes, &task_id, &ended)),
    ])
}
