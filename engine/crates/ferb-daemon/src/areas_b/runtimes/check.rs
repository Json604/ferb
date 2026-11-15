//! What Settings → AI's Check sends (ferb/runtimes/check.py; T-1069, RUN-439): the call shapes FERB really makes,
//! each kept tiny, the two tools that exist only while a check runs (they answer from memory and touch nothing), and
//! one check line as Settings shows it.

use std::sync::Mutex;

use serde_json::{json, Map, Value};

pub const PING: &str = "check.ping";
pub const ECHO: &str = "check.echo";
pub const PROMPT_PLANNER: &str = "This is FERB's connection check, not a real request. Use no tool. Answer with kind \"reply\", text \"ok\", every object field null and every list empty, and every other field empty or false.";
pub const PROMPT_WORKER: &str = "This is FERB's connection check. First call the tool check_ping (check.ping) with no arguments. Then call the tool check_echo (check.echo) with text \"hello\". Then report: status \"done\", reason \"\", question null, answer \"ok\", artifacts [], how \"\".";
pub const PROMPT_CHECKER: &str = "This is FERB's connection check. Condition 0: the answer says ok. The answer: \"ok\". Judge it: results [{index 0, met true, evidence \"the answer says ok\", how null}], next \"accept\", findings [], and every other field empty or null.";
pub const PROMPT_IMAGE: &str =
    "This is FERB's connection check. What single colour fills the picture? Answer {\"colour\": <word>}.";

/// check.py `IMAGE_SCHEMA`.
pub fn image_schema() -> Value {
    json!({"type": "object", "properties": {"colour": {"type": "string"}}, "required": ["colour"],
           "additionalProperties": false})
}

/// check.py `red_png()`: a 16x16 solid red PNG, the exact bytes Python's zlib makes of it (no screen is captured
/// for the check). Fixed bytes rather than a compressor of our own, so the request is byte-identical (fixture
/// `checks`, the image's SHA-256).
pub const RED_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x10, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x91, 0x68,
    0x36, 0x00, 0x00, 0x00, 0x17, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x40,
    0x12, 0x22, 0x4d, 0xf5, 0xa8, 0x86, 0x51, 0x0d, 0x43, 0x4a, 0x03, 0x00, 0x90, 0xf9, 0xff, 0x01,
    0xf9, 0xe1, 0xfa, 0x78, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// RUN-439: each check call is given up after this long (the check's own limit; a real call has none).
pub const LINE_LIMIT_S: f64 = 60.0;

/// A tool that exists only while a check runs: the gateway's registry lists it and runs it like any tool.
pub trait CheckTool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    /// Its input as pydantic writes the model's JSON schema (extra fields forbidden).
    fn input_schema(&self) -> Value;
    /// check.py `run`: the result's data. `args` were validated against `input_schema` by the gateway.
    fn run(&self, args: &Map<String, Value>) -> Value;
}

/// check.py `CheckPing`: no arguments, answers pong.
#[derive(Default)]
pub struct CheckPing {
    calls: Mutex<u64>,
}

impl CheckPing {
    pub fn calls(&self) -> u64 {
        *self.calls.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl CheckTool for CheckPing {
    fn name(&self) -> &'static str {
        PING
    }
    fn description(&self) -> &'static str {
        "FERB's connection check: call it with no arguments; it answers pong."
    }
    fn input_schema(&self) -> Value {
        json!({"additionalProperties": false, "properties": {}, "title": "Input", "type": "object"})
    }
    fn run(&self, _args: &Map<String, Value>) -> Value {
        *self.calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        json!({"pong": true})
    }
}

/// check.py `CheckEcho`: echoes its text.
#[derive(Default)]
pub struct CheckEcho {
    texts: Mutex<Vec<String>>,
}

impl CheckEcho {
    pub fn texts(&self) -> Vec<String> {
        self.texts.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl CheckTool for CheckEcho {
    fn name(&self) -> &'static str {
        ECHO
    }
    fn description(&self) -> &'static str {
        "FERB's connection check: call it with text; it answers with the same text."
    }
    fn input_schema(&self) -> Value {
        json!({"additionalProperties": false, "properties": {"text": {"title": "Text", "type": "string"}},
               "required": ["text"], "title": "Input", "type": "object"})
    }
    fn run(&self, args: &Map<String, Value>) -> Value {
        let text = ferb_py::value::py_str(args.get("text").unwrap_or(&Value::Null));
        self.texts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(text.clone());
        json!({"echo": text})
    }
}

/// check.py `Line`: one check's result as Settings shows it.
#[derive(Debug, Clone, Default)]
pub struct Line {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub model: String,
    pub typed: String,
    pub seconds: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: Option<f64>,
    pub part_of: String,
    /// `extra["retrying"]`: Codex's last retry message, said when the line never answers.
    pub retrying: Option<String>,
}

/// Python's `int(x or 0)` of a decoded JSON value (a float is truncated; text or anything else counts as 0 here,
/// where Python would raise: no adapter reports tokens as text).
fn int_or_zero(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n
            .as_i64()
            .unwrap_or_else(|| n.as_f64().map_or(0, |f| f as i64)),
        Some(Value::Bool(b)) => i64::from(*b),
        _ => 0,
    }
}

impl Line {
    pub fn new(name: &str) -> Line {
        Line {
            name: name.to_string(),
            ..Line::default()
        }
    }

    /// check.py `add_usage`.
    pub fn add_usage(&mut self, data: &Map<String, Value>) {
        if let Some(m) = data.get("model").filter(|v| ferb_py::value::truthy(v)) {
            self.model = ferb_py::value::py_str(m);
        }
        if let Some(t) = data.get("typed").filter(|v| ferb_py::value::truthy(v)) {
            self.typed = ferb_py::value::py_str(t);
        }
        self.input_tokens += int_or_zero(data.get("input_tokens"));
        self.output_tokens += int_or_zero(data.get("output_tokens"));
        let cost = match data.get("cost_usd") {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::Bool(b)) => Some(f64::from(u8::from(*b))),
            _ => None,
        };
        if let Some(c) = cost {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + c);
        }
    }

    /// check.py `row`.
    pub fn row(&self) -> Value {
        let mut out = Map::new();
        out.insert("name".into(), self.name.clone().into());
        out.insert("ok".into(), self.ok.into());
        out.insert("detail".into(), self.detail.clone().into());
        out.insert("model".into(), self.model.clone().into());
        let typed = if self.typed.is_empty() {
            "none"
        } else {
            &self.typed
        };
        out.insert("typed".into(), typed.into());
        out.insert("seconds".into(), json!(py_round(self.seconds, 1)));
        out.insert("input_tokens".into(), self.input_tokens.into());
        out.insert("output_tokens".into(), self.output_tokens.into());
        out.insert(
            "cost_usd".into(),
            self.cost_usd.map_or(Value::Null, |c| json!(c)),
        );
        out.insert("limit_s".into(), json!(LINE_LIMIT_S));
        if !self.part_of.is_empty() {
            out.insert("part_of".into(), self.part_of.clone().into());
        }
        Value::Object(out)
    }
}

/// Python's `round(x, digits)` for a float: the decimal correctly rounded (half to even on the exact binary value),
/// read back as the nearest float.
pub fn py_round(x: f64, digits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.digits$}").parse().unwrap_or(x)
}

/// check.py `no_answer` (RUN-439): a line given up after `limit_s`, with what the provider answered meanwhile.
pub fn no_answer(limit_s: f64, retried: &[Value], retrying: &str) -> String {
    let mut said = format!("no answer after {} s", py_g(limit_s));
    if let Some(last) = retried.last() {
        let status = int_or_zero(last.get("status"));
        let wait = last
            .get("retry_after")
            .filter(|w| ferb_py::value::truthy(w));
        let what = if status == 429 {
            "a rate or usage limit (HTTP 429)".to_string()
        } else {
            format!("HTTP {status}")
        };
        let n = retried.len();
        said.push_str(&format!(
            "; the provider answered {what} {n} time{}",
            if n != 1 { "s" } else { "" }
        ));
        if let Some(w) = wait {
            let w = w.as_f64().map_or_else(|| ferb_py::value::py_str(w), py_g);
            said.push_str(&format!(", asking to retry after {w} s"));
        }
    } else if !retrying.is_empty() {
        said.push_str(&format!("; {retrying}"));
    }
    said
}

/// Python's `f"{x:g}"`.
fn py_g(x: f64) -> String {
    ferb_runtimes::api::py_g(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_given_up_says_what_the_provider_answered() {
        assert_eq!(no_answer(60.0, &[], ""), "no answer after 60 s");
        assert_eq!(
            no_answer(60.0, &[], "invented retry"),
            "no answer after 60 s; invented retry"
        );
        let retried = [
            json!({"status": 500}),
            json!({"status": 429, "retry_after": 2.5}),
        ];
        assert_eq!(
            no_answer(60.0, &retried, "x"),
            "no answer after 60 s; the provider answered a rate or usage limit (HTTP 429) 2 times, asking to retry after 2.5 s"
        );
        assert_eq!(
            no_answer(60.0, &[json!({"status": 503})], ""),
            "no answer after 60 s; the provider answered HTTP 503 1 time"
        );
    }

    #[test]
    fn rounding_is_pythons() {
        assert_eq!(py_round(0.0021000000000000003, 6), 0.0021);
        assert_eq!(py_round(0.04, 1), 0.0);
        assert_eq!(py_round(0.25, 1), 0.2);
    }
}
