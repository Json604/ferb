//! `ferb logs` (ferb/cli/logs.py): the daemon's bus, live, one line per event. A `tool.finished` event's line is
//! ferb/trace/lines.py's `step_line`, which comes in through `step_line` until the traces part is ported.

use serde_json::Value;

use super::{CliError, Rpc};

/// `json.dumps(value, separators=(",", ":"), ensure_ascii=False)`.
pub fn dumps_compact(value: &Value) -> String {
    let mut out = String::new();
    write(&mut out, value);
    out
}

fn write(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(&ferb_py::json::number(n)),
        Value::String(s) => string(out, s),
        Value::Array(a) => {
            out.push('[');
            for (i, v) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(out, v);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, v)) in m.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                string(out, k);
                out.push(':');
                write(out, v);
            }
            out.push('}');
        }
    }
}

fn string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// logs.py:13. `step_line` renders a `tool.finished` event (the traces part's).
pub fn format_event(ev: &Value, step_line: &dyn Fn(&Value) -> String) -> Result<String, CliError> {
    if ev["topic"] == "tool.finished" {
        return Ok(step_line(ev));
    }
    let at = ferb_py::value::py_str(&ev["at"]);
    let t = ferb_py::time::DateTime::from_isoformat(&at).map_err(|_| CliError::Ferb {
        kind: "ValueError".into(),
        message: format!("Invalid isoformat string: {}", crate::py::py_repr_str(&at)),
    })?;
    let marker = match ev["class"].as_str() {
        Some("critical") => "!",
        Some("notice") => "*",
        _ => " ",
    };
    let topic = ferb_py::value::py_str(&ev["topic"]);
    Ok(format!(
        "{:02}:{:02}:{:02} {marker} {topic:<24} {}",
        t.hour,
        t.minute,
        t.second,
        dumps_compact(&ev["data"])
    ))
}

/// logs.py:24: the stream, printed until it ends (or `limit` events).
pub fn run(
    client: &mut dyn Rpc,
    filters: &[String],
    out: &mut dyn std::io::Write,
    limit: Option<usize>,
    step_line: &dyn Fn(&Value) -> String,
) -> Result<i32, CliError> {
    let mut n = 0;
    let mut failed = None;
    client.stream(
        "logs.subscribe",
        Some(serde_json::json!({"filters": filters})),
        &mut |ev| {
            match format_event(&ev, step_line) {
                Ok(line) => {
                    let _ = writeln!(out, "{line}");
                    let _ = out.flush();
                }
                Err(e) => {
                    failed = Some(e);
                    return false;
                }
            }
            n += 1;
            limit.is_none_or(|l| n < l)
        },
    )?;
    match failed {
        Some(e) => Err(e),
        None => Ok(0),
    }
}
