//! Where a traced turn's time went (`ferb/trace/timing.py`; ORCH-527): FERB's work before the model, the runtime's
//! start, each model call split into its parts, the tools between them, and FERB's work after, from the timing the
//! spans recorded (ORCH-525, ORCH-526, RUN-419, RUN-420). It adds nothing the records don't say: a span recorded
//! before timing existed shows none.
//!
//! `breakdown` is the JSON `ferb trace --json` adds; `lines` is the text `ferb trace --full` ends with.

use serde_json::{Map, Value};

use crate::compose::sorted_nodes;
use crate::py::{
    g, gd, has, int_of, int_val, is_int, is_number, items, map, micros, num, obj, or, parse_time,
    round_int, str_of, string, text, total_seconds, truthy, EMPTY,
};

/// A planner turn's lookup record: its request id with this suffix (supervisor.LOOKUPS).
pub const LOOKUPS: &str = ".lookups";
const CALL_PARTS: [(&str, &str); 4] = [
    ("runtime_ms", "runtime before the request"),
    ("ttft_ms", "to the first output"),
    ("generate_ms", "generating"),
    ("tail_ms", "to its usage report"),
];

fn ms(start: &Value, end: &Value) -> Value {
    let (Some(a), Some(b)) = (
        start.as_str().filter(|s| !s.is_empty()),
        end.as_str().filter(|s| !s.is_empty()),
    ) else {
        return Value::Null;
    };
    total_seconds(a, b).map_or(Value::Null, |s| Value::from(round_int(s * 1000.0)))
}

fn timing_of(node: &Value) -> &Value {
    let t = g(g(node, "output"), "timing");
    if t.is_object() {
        t
    } else {
        &EMPTY
    }
}

fn calls(node: &Value) -> Vec<Value> {
    items(node, "children")
        .iter()
        .filter(|c| text(c, "kind") == "model.usage")
        .map(|c| {
            let mut m: Map<String, Value> = timing_of(c).as_object().cloned().unwrap_or_default();
            m.insert("reported_at".to_owned(), g(c, "started_at").clone());
            Value::Object(m)
        })
        .collect()
}

fn tools(nodes: &[&Value], task_id: &Value, start: &str, end: &Value) -> Vec<Value> {
    nodes
        .iter()
        .filter(|n| {
            text(n, "kind") == "tool.call"
                && g(n, "task_id") == task_id
                && text(n, "started_at") >= start
                && (end.is_null() || text(n, "started_at") <= str_of(end).as_str())
        })
        .map(|n| {
            map(vec![
                (
                    "tool",
                    string(&str_of(gd(obj(n, "input"), "tool", &string("?")))),
                ),
                ("ms", ms(g(n, "started_at"), g(n, "ended_at"))),
                ("at", g(n, "started_at").clone()),
            ])
        })
        .collect()
}

fn planner_turns(nodes: &[&Value]) -> Vec<Value> {
    let mut used: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for turn in nodes {
        let inp = obj(turn, "input");
        if text(turn, "kind") != "model.turn" || g(inp, "role").as_str() != Some("supervisor") {
            continue;
        }
        let request_id = str_of(or(g(inp, "request_id"), &string("")));
        let prepare = nodes.iter().rev().find(|n| {
            text(n, "kind") == "planner.prepare"
                && !used.contains(&text(n, "id"))
                && g(obj(n, "input"), "request_id").as_str() == Some(request_id.as_str())
                && text(n, "started_at") <= text(turn, "started_at")
        });
        if let Some(p) = prepare {
            used.push(text(p, "id"));
        }
        let timing = timing_of(turn);
        let call = if g(timing, "call").is_object() {
            g(timing, "call")
        } else {
            &EMPTY
        };
        let steps = obj(timing, "steps_ms");
        let call_steps = obj(call, "steps_ms");
        let reply = nodes.iter().find(|n| {
            text(n, "kind") == "planner.reply"
                && text(n, "started_at") >= text(turn, "started_at")
                && g(obj(n, "input"), "request_id").as_str() == Some(request_id.as_str())
        });
        let before = prepare.map_or(&*EMPTY, |p| obj(p, "output"));
        let first = prepare.unwrap_or(turn);
        out.push(map(vec![
            ("kind", string("planner")),
            ("request_id", string(&request_id)),
            ("at", g(first, "started_at").clone()),
            ("error", g(turn, "error").clone()),
            (
                "ferb_before",
                map(vec![
                    ("steps_ms", obj(before, "steps_ms").clone()),
                    ("total_ms", g(before, "total_ms").clone()),
                    ("in_turn_ms", g(steps, "ferb_before").clone()),
                    ("request_built_ms", g(call_steps, "prepare").clone()),
                ]),
            ),
            (
                "runtime_start",
                map(vec![
                    ("ms", g(call_steps, "start").clone()),
                    ("steps_ms", obj(call, "start_steps_ms").clone()),
                ]),
            ),
            ("model_calls", Value::Array(calls(turn))),
            (
                "tools",
                Value::Array(tools(
                    nodes,
                    &string(&format!("{request_id}{LOOKUPS}")),
                    text(turn, "started_at"),
                    g(turn, "ended_at"),
                )),
            ),
            ("stream_ms", g(call_steps, "stream").clone()),
            ("close_ms", g(call_steps, "close").clone()),
            (
                "ferb_after",
                map(vec![
                    ("in_turn_ms", g(steps, "ferb_after").clone()),
                    (
                        "to_reply_ms",
                        reply.map_or(Value::Null, |r| {
                            g(obj(r, "output"), "after_decision_ms").clone()
                        }),
                    ),
                ]),
            ),
            ("turn_ms", g(timing, "total_ms").clone()),
            ("recorded", Value::Bool(truthy(timing))),
        ]));
    }
    out
}

fn workers(nodes: &[&Value]) -> Vec<Value> {
    let mut out = Vec::new();
    for run in nodes.iter().filter(|n| text(n, "kind") == "worker") {
        let call = timing_of(run);
        let steps = obj(call, "steps_ms");
        let verifier = g(obj(run, "input"), "role").as_str() == Some("verifier");
        out.push(map(vec![
            ("kind", string(if verifier { "checker" } else { "worker" })),
            ("task_id", g(run, "task_id").clone()),
            ("at", g(run, "started_at").clone()),
            ("error", g(run, "error").clone()),
            ("request_built_ms", g(steps, "prepare").clone()),
            (
                "runtime_start",
                map(vec![
                    ("ms", g(steps, "start").clone()),
                    ("steps_ms", obj(call, "start_steps_ms").clone()),
                ]),
            ),
            ("model_calls", Value::Array(calls(run))),
            (
                "tools",
                Value::Array(tools(
                    nodes,
                    &string(&str_of(g(run, "task_id"))),
                    text(run, "started_at"),
                    g(run, "ended_at"),
                )),
            ),
            ("stream_ms", g(steps, "stream").clone()),
            ("close_ms", g(steps, "close").clone()),
            ("run_ms", ms(g(run, "started_at"), g(run, "ended_at"))),
            ("recorded", Value::Bool(truthy(call))),
        ]));
    }
    out
}

/// `breakdown(forest)`: each planner turn and worker run, in start order, with where its time went.
pub fn breakdown(forest: &[Value]) -> Vec<Value> {
    let nodes = sorted_nodes(forest);
    let mut all = planner_turns(&nodes);
    all.extend(workers(&nodes));
    all.sort_by(|a, b| text(a, "at").cmp(text(b, "at")));
    all
}

/// `_d(ms)`: "?" unless a number; milliseconds under a second, else seconds.
fn d(v: &Value) -> String {
    if !is_number(v) {
        return "?".to_owned();
    }
    let x = num(v);
    if x < 1000.0 {
        format!("{x:.0} ms")
    } else {
        format!("{:.2} s", x / 1000.0)
    }
}

fn parts(steps: &Value) -> String {
    let Some(m) = steps.as_object() else {
        return String::new();
    };
    m.iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(name, v)| format!("{} {}", name.replace('_', " "), d(v)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// When a model call began: its usage report's arrival less its total, in microseconds.
fn began(call: &Value) -> i128 {
    let reported = parse_time(text(call, "reported_at")).map_or(0, |t| micros(&t));
    reported - i128::from(int_of(or(g(call, "total_ms"), &Value::from(0)))) * 1000
}

fn call_line(i: usize, timing: &Value) -> String {
    if !has(timing, "total_ms") {
        return format!("  Model call {i}: no timing recorded");
    }
    let parts: Vec<String> = CALL_PARTS
        .iter()
        .filter(|(key, _)| has(timing, key))
        .map(|(key, label)| format!("{label} {}", d(g(timing, key))))
        .collect();
    let clock = if g(timing, "by").as_str() == Some("runtime") {
        "the runtime's own stream"
    } else {
        "arrival at FERB"
    };
    format!(
        "  Model call {i} {}: {} (by {clock})",
        d(g(timing, "total_ms")),
        parts.join(", ")
    )
}

fn run_lines(entry: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let start = g(entry, "runtime_start");
    let mut head = format!("  Runtime start {}", d(g(start, "ms")));
    if truthy(g(start, "steps_ms")) {
        head.push_str(&format!(": {}", parts(g(start, "steps_ms"))));
    }
    out.push(head);
    let model_calls = items(entry, "model_calls");
    let tool_list = items(entry, "tools");
    let tools_ms: i64 = tool_list
        .iter()
        .map(|t| int_of(or(g(t, "ms"), &Value::from(0))))
        .sum();
    let mut events: Vec<(i128, u8, String)> = Vec::new();
    for (i, c) in model_calls.iter().enumerate() {
        events.push((began(c), 0, call_line(i + 1, c)));
    }
    for t in tool_list {
        let at = parse_time(text(t, "at")).map_or(0, |t| micros(&t));
        events.push((
            at,
            1,
            format!("  Tool {} {}", str_of(g(t, "tool")), d(g(t, "ms"))),
        ));
    }
    events.sort_by_key(|e| (e.0, e.1));
    out.extend(events.into_iter().map(|e| e.2));
    if is_int(g(entry, "stream_ms")) {
        let counted: i64 = model_calls
            .iter()
            .map(|c| int_of(or(g(c, "total_ms"), &Value::from(0))))
            .sum::<i64>()
            + tools_ms;
        out.push(format!(
            "  Stream {} in all (model calls and tools {}; the rest is the runtime's own work between them, or clock skew)",
            d(g(entry, "stream_ms")),
            d(&Value::from(counted))
        ));
    }
    out.push(format!("  Runtime close {}", d(g(entry, "close_ms"))));
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// `lines(entries)`: the timing section `ferb trace --full` ends with.
pub fn lines(entries: &[Value]) -> Vec<String> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut out = vec![
        String::new(),
        "Where the time went (FERB's clock; each model call on the clock named):".to_owned(),
    ];
    for e in entries {
        let when = crate::py::hms(text(e, "at"));
        let kind = text(e, "kind");
        if !truthy(g(e, "recorded")) {
            out.push(format!(
                "{} {when}: no timing recorded (a span from before timing was kept)",
                capitalize(kind)
            ));
            continue;
        }
        let failed = if truthy(g(e, "error")) {
            " (failed)"
        } else {
            ""
        };
        if kind == "planner" {
            let (before, after) = (g(e, "ferb_before"), g(e, "ferb_after"));
            let ints = |vs: &[&Value]| {
                vs.iter()
                    .filter(|v| is_int(v))
                    .map(|v| int_val(v))
                    .sum::<i64>()
            };
            let total = ints(&[
                g(before, "total_ms"),
                g(e, "turn_ms"),
                g(after, "to_reply_ms"),
            ]);
            let to = if is_int(g(after, "to_reply_ms")) {
                "its reply"
            } else {
                "its decision"
            };
            out.push(format!(
                "Planner turn {when} (request {}): {} from its intake to {to}{failed}",
                str_of(g(e, "request_id")),
                d(&Value::from(total))
            ));
            let mut merged: Map<String, Value> = g(before, "steps_ms")
                .as_object()
                .cloned()
                .unwrap_or_default();
            merged.insert("turn setup".to_owned(), g(before, "in_turn_ms").clone());
            merged.insert(
                "request built".to_owned(),
                g(before, "request_built_ms").clone(),
            );
            let ahead = ints(&[
                g(before, "total_ms"),
                g(before, "in_turn_ms"),
                g(before, "request_built_ms"),
            ]);
            out.push(format!(
                "  FERB before the model {}: {}",
                d(&Value::from(ahead)),
                parts(&Value::Object(merged))
            ));
            out.extend(run_lines(e));
            let mut s = format!(
                "  FERB after the model: answer check {}",
                d(g(after, "in_turn_ms"))
            );
            if is_int(g(after, "to_reply_ms")) {
                s.push_str(&format!(", to the reply {}", d(g(after, "to_reply_ms"))));
            }
            out.push(s);
        } else {
            out.push(format!(
                "{} {} {when}: {}{failed}",
                capitalize(kind),
                str_of(g(e, "task_id")),
                d(g(e, "run_ms"))
            ));
            out.push(format!("  Request built {}", d(g(e, "request_built_ms"))));
            out.extend(run_lines(e));
        }
    }
    out
}
