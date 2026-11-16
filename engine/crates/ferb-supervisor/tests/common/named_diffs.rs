//! The deliberate differences from the Python recordings that the replay harnesses (tests/support_control,
//! tests/replay/harness.rs, ferbd's tests/chat_replay.rs) apply before comparing, each naming its decision (ENG-6, ENG-17).
#![allow(dead_code)]

use serde_json::Value;

fn s(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_default()
}

/// ORCH-650 (fixed against Python): a check reads the owner's answers to its step's questions in a block of its own,
/// which Python's checker never had. A call is compared without that block; the blocks taken out are kept, so a
/// scenario where the owner answered a step's question asserts its check had them.
// decision: ORCH-650
pub fn without_step_answers(parts: &Value, taken: &mut Vec<String>) -> Value {
    let Some(list) = parts.as_array() else {
        return parts.clone();
    };
    let (block, rest): (Vec<Value>, Vec<Value>) = list.iter().cloned().partition(|p| {
        p.as_str()
            .is_some_and(|t| t.starts_with("Layer 6 — FERB's questions to the owner in this step"))
    });
    taken.extend(block.iter().map(s));
    Value::Array(rest)
}

/// RUN-465 (fixed against Python): a worker run the owner's pause cut is recorded as paused ("paused by you",
/// cancelled, no error) and its episode step says so; Python recorded "The report from this attempt wasn't usable:
/// runtime finished without an answer." as a failure (`stop_while_paused`). A recorded row with that failure is
/// compared as the paused one when the written row is the paused one; nothing else changes.
// decision: RUN-465
pub fn paused_run(table: &str, cols: &[String], python: &mut [Value], rust: &[Value]) {
    const FAILED: &str =
        "The report from this attempt wasn't usable: runtime finished without an answer.";
    // the owner's pause, and FERB's own (a closed tab's question: `owner_closed_tab`)
    const PAUSED: [&str; 2] = ["paused by you", "paused by FERB to ask you"];
    let at = |name: &str| cols.iter().position(|c| c == name);
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    match table {
        "spans" => {
            let (Some(out), Some(err)) = (at("output_json"), at("error")) else {
                return;
            };
            let (p, r) = (text(&python[out]), text(&rust[out]));
            let paused = PAUSED
                .iter()
                .any(|w| r.contains(&format!(r#""result": "{w}", "status": "cancelled""#)));
            if p.contains(FAILED) && paused {
                python[out] = rust[out].clone();
                python[err] = Value::Null;
            }
        }
        "episodes" => {
            let Some(steps) = at("steps_json") else {
                return;
            };
            let from = format!("worker reported failed: {FAILED}");
            for w in PAUSED {
                let to = format!("stopped: {w}");
                if text(&python[steps]).contains(&from) && text(&rust[steps]).contains(&to) {
                    python[steps] = Value::String(text(&python[steps]).replace(&from, &to));
                }
            }
        }
        _ => {}
    }
}

pub const STEP_ANSWERS: &str = "Layer 6 — FERB's questions to the owner in this step";

/// ORCH-650: a text with the step-answer block taken out and the context budget's estimate (which counts it) left
/// unnumbered, for the comparison with Python's.
pub fn answers_out(t: &str) -> String {
    let mut t = t.to_string();
    if let Some(at) = t.find(STEP_ANSWERS) {
        let close = t[at..].find("</untrusted-data id=").map(|c| at + c);
        if let Some(close) = close {
            let end = t[close..].find('>').map_or(t.len(), |e| close + e + 1);
            let end = if t[end..].starts_with("\n\n") {
                end + 2
            } else {
                end
            };
            t.replace_range(at..end, "");
        }
    }
    let key = "\"content_tokens_estimate\": ";
    if let Some(at) = t.find(key) {
        let from = at + key.len();
        let to = t[from..]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(t.len(), |e| from + e);
        t.replace_range(from..to, "N");
    }
    t
}

pub fn answers_out_value(v: &Value) -> Value {
    match v {
        Value::String(t) => Value::String(answers_out(t)),
        Value::Array(a) => Value::Array(a.iter().map(answers_out_value).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), answers_out_value(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// ORCH-650: a recorded and a written span's input, both without the step-answer block, when the written one has it.
pub fn span_inputs_without_answers(python: &Value, rust: &Value) -> Option<(Value, Value)> {
    let parsed = |c: &Value| {
        c.as_str()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
    };
    let (p, r) = (parsed(python)?, parsed(rust)?);
    if !r.to_string().contains(STEP_ANSWERS) {
        return None;
    }
    let dump = |v: &Value| Value::String(ferb_py::json::dumps(&answers_out_value(v)));
    Some((dump(&p), dump(&r)))
}

/// RUN-465 (fixed against Python): a worker run the owner's stop or pause cut ends with no failure in its span; Python
/// kept the cut's error ("ValueError: runtime finished without an answer"). Recorded cancelled worker spans are compared
/// without it; every other cell as recorded.
// decision: RUN-465
pub fn stop_records_no_failure(cols: &[String], rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    let at = |name: &str| cols.iter().position(|c| c == name);
    let (Some(kind), Some(output), Some(error)) = (at("kind"), at("output_json"), at("error"))
    else {
        return rows;
    };
    rows.into_iter()
        .map(|mut r| {
            let cancelled = r[output]
                .as_str()
                .and_then(|o| serde_json::from_str::<Value>(o).ok())
                .is_some_and(|o| o["status"] == "cancelled");
            if r[kind] == "worker" && cancelled {
                r[error] = Value::Null;
            }
            r
        })
        .collect()
}

/// RUN-465: so a check the stop cut reads in the task's episode as what it is (nothing checked), not as Python's
/// "check failed: ValueError: runtime finished without an answer" (`stop_while_checking`).
// decision: RUN-465
pub fn stopped_check_in_episode(rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    let python = r#"check failed: ValueError: runtime finished without an answer", "mark": "✗""#;
    let now = r#"checked nothing — 1 condition unknown", "mark": "⚠""#;
    // the episode's problem line follows from that step, so it changes in that row only
    let swaps = [
        (python, now),
        (
            "\"✗ 1 step failed\"",
            "\"⚠ 1 check left conditions unknown\"",
        ),
    ];
    rows.into_iter()
        .map(|r| {
            let cut = r
                .iter()
                .any(|c| c.as_str().is_some_and(|t| t.contains(python)));
            r.into_iter()
                .map(|c| match c {
                    Value::String(t) if cut => {
                        Value::String(swaps.iter().fold(t, |t, (from, to)| t.replace(from, to)))
                    }
                    other => other,
                })
                .collect()
        })
        .collect()
}

/// A table's recorded rows and the written ones, each with the named differences above applied.
pub fn named_rows(
    table: &str,
    cols: &[String],
    python: Vec<Vec<Value>>,
    rust: &[Vec<Value>],
) -> (Vec<Vec<Value>>, Vec<Vec<Value>>) {
    let mut python = match table {
        "spans" => stop_records_no_failure(cols, python),
        "episodes" => stopped_check_in_episode(python),
        _ => python,
    };
    let mut rust = rust.to_vec();
    for (a, b) in python.iter_mut().zip(rust.iter_mut()) {
        paused_run(table, cols, a, b);
        if table == "spans" {
            if let Some(c) = cols.iter().position(|c| c == "input_json") {
                if let Value::String(t) = &a[c] {
                    a[c] = Value::String(attempts_last_text(t));
                }
                if let Some((x, y)) = span_inputs_without_answers(&a[c], &b[c]) {
                    a[c] = x;
                    b[c] = y;
                }
            }
        }
    }
    (python, rust)
}

/// ORCH-655 (not Python's order): a check's previous-attempts block comes last among the task's blocks, just before
/// FERB's code facts. A recorded check's parts in that order; any other call's as recorded.
// decision: ORCH-655
pub fn attempts_last(parts: &Value) -> Value {
    const ATTEMPTS: &str = "Previous attempts (data):\n";
    const FACTS: &str = "FERB's code facts about how this work was done";
    let Some(list) = parts.as_array() else {
        return parts.clone();
    };
    let starts = |p: &Value, with: &str| p.as_str().is_some_and(|t| t.starts_with(with));
    let (Some(at), Some(facts)) = (
        list.iter().position(|p| starts(p, ATTEMPTS)),
        list.iter().position(|p| starts(p, FACTS)),
    ) else {
        return parts.clone();
    };
    if at > facts {
        return parts.clone();
    }
    let mut out = list.clone();
    let block = out.remove(at);
    out.insert(facts - 1, block);
    Value::Array(out)
}

/// ORCH-655 in a recorded text that holds a check's context (a span's input, escaped once or more): the
/// previous-attempts block moved to just before FERB's code facts; any other text as recorded.
// decision: ORCH-655
pub fn attempts_last_text(t: &str) -> String {
    const FACTS: &str = "FERB's code facts about how this work was done";
    let esc = |x: &str| {
        let d = ferb_py::json::dumps(&Value::String(x.to_string()));
        d[1..d.len() - 1].to_string()
    };
    let (mut head, mut sep) = (
        "Previous attempts (data):\n".to_string(),
        "\n\n".to_string(),
    );
    for _ in 0..3 {
        if let (Some(at), Some(facts)) = (t.find(&head), t.find(FACTS)) {
            if at > facts {
                return t.to_string();
            }
            let Some(close) = t[at..].find("</untrusted-data id=").map(|c| at + c) else {
                return t.to_string();
            };
            let Some(end) = t[close..].find('>').map(|e| close + e + 1) else {
                return t.to_string();
            };
            let end = if t[end..].starts_with(&sep) {
                end + sep.len()
            } else {
                end
            };
            let block = &t[at..end];
            let rest = format!("{}{}", &t[..at], &t[end..]);
            let to = rest.find(FACTS).expect("the facts follow");
            return format!("{}{block}{}", &rest[..to], &rest[to..]);
        }
        head = esc(&head);
        sep = esc(&sep);
    }
    t.to_string()
}

/// A recorded model call and the one made, compared without a check's step-answer block (ORCH-650), which is kept
/// in `taken`; with one taken out, the budget line's estimate (which counted it) is not compared. A check's recorded
/// parts are in ORCH-655's order first.
pub fn named_call(
    mut expected: Value,
    mut actual: Value,
    taken: &mut Vec<String>,
) -> (Value, Value) {
    if let Some(parts) = expected.get("parts").cloned() {
        expected["parts"] = attempts_last(&parts);
    }
    let before = taken.len();
    if let Some(parts) = actual.get("parts").cloned() {
        actual["parts"] = without_step_answers(&parts, taken);
    }
    if taken.len() > before {
        (answers_out_value(&expected), answers_out_value(&actual))
    } else {
        (expected, actual)
    }
}
