//! The pure parts of `learning.py`: how a node is hashed (`encoded`, `digest`), what search matches it by
//! (`search_text`), how a prompt shows it (`shown_text`, `shown_state`), an episode's how-note (MEM-31) and the steps
//! its recorded calls show, and the constants the store keeps.

use serde_json::{Map, Value};

use ferb_memory::pystr;
use ferb_sanitize::origin;

use crate::py;
use crate::text::query_terms;

/// learning.py:30.
pub const ID_KEYED: [&str; 3] = ["episode", "episode_annotation", "episode_intent"];
/// learning.py:35.
pub const CARD_MAKER: &str = "code:evidence-card/4";
pub const PARKED_BY_REDACTION: &str = "redaction would still change it";
pub const COPYING_CARD_MAKER: &str = "code:evidence-card/1";
pub const SOURCE_REF_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS learning_source_ref ON learning_sources(source_ref)";
/// learning.py:47.
pub const SUGGESTIONS: [&str; 3] = ["procedure", "identity_proposal", "lesson"];
/// learning.py:51.
pub const PROMPT_LEAVES_OUT: [&str; 8] = [
    "fact",
    "identity",
    "identity_proposal",
    "episode_intent",
    "procedure",
    "lesson",
    "summary",
    "entity",
];
pub const RECENCY_WEIGHT: f64 = 0.05;
pub const RECENCY_HALF_LIFE_DAYS: f64 = 30.0;
pub const SEARCH_TEXT_VERSION: &str = "content/2";
pub const REINDEX_BATCH_BYTES: usize = 64_000;
/// learning.py:136.
pub const KEPT_FAMILIES: [&str; 3] = ["episode", "journal", "memory"];
pub const HINT_CHARS: usize = 300;
pub const HOW_STEPS: usize = 12;
pub const HOW_CHARS: usize = 600;
const STEP_TARGETS: [&str; 7] = [
    "url",
    "app",
    "bundle_id",
    "path",
    "site",
    "host",
    "recipient",
];

pub use crate::py::encoded;

/// learning.py:81 `digest`.
pub fn digest(value: &Value) -> String {
    py::sha256_hex(&encoded(value))
}

/// learning.py:124 `_one_line`.
pub fn one_line(value: Option<&Value>) -> String {
    let text = match value {
        None | Some(Value::Null) => String::new(),
        Some(v) => py::py_str(v),
    };
    pystr::split_ws(&text).collect::<Vec<_>>().join(" ")
}

/// learning.py:129 `slug_words`.
pub fn slug_words(slug: &str) -> String {
    let part = slug.split_once(':').map_or(slug, |(_, rest)| rest);
    query_terms(part).join(" ")
}

fn get<'a>(body: &'a Value, key: &str) -> Option<&'a Value> {
    body.get(key)
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(ferb_py::value::truthy)
}

/// `str(x or '')`.
fn or_empty(v: Option<&Value>) -> String {
    if truthy(v) {
        py::py_str(v.expect("truthy"))
    } else {
        String::new()
    }
}

fn step_target(args: &Map<String, Value>) -> String {
    for key in STEP_TARGETS {
        let Some(Value::String(value)) = args.get(key) else {
            continue;
        };
        if pystr::strip(value).is_empty() {
            continue;
        }
        if key == "url" && value.contains("://") {
            let host = ferb_sanitize::parity::hostname(value)
                .ok()
                .flatten()
                .unwrap_or_default();
            let path = url_path(value);
            return host
                + if path.is_empty() || path == "/" {
                    ""
                } else {
                    &path
                };
        }
        if key == "path" {
            let name = py::path_name(value);
            return if name.is_empty() { value.clone() } else { name };
        }
        return value.clone();
    }
    String::new()
}

/// `urlsplit(url).path`.
fn url_path(url: &str) -> String {
    let url: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut rest = url.as_str();
    if let Some(i) = rest.find(':') {
        let ok = |c: char| c.is_ascii_alphanumeric() || "+-.".contains(c);
        if i > 0 && rest.starts_with(|c: char| c.is_ascii_alphabetic()) && rest[..i].chars().all(ok)
        {
            rest = &rest[i + 1..];
        }
    }
    if let Some(after) = rest.strip_prefix("//") {
        rest = &after[after.find(['/', '?', '#']).unwrap_or(after.len())..];
    }
    rest[..rest.find(['?', '#']).unwrap_or(rest.len())].to_owned()
}

/// learning.py:160 `recorded_steps`.
pub fn recorded_steps(actions: &[Value]) -> String {
    let mut steps: Vec<(String, usize)> = Vec::new();
    let mut failed = 0usize;
    let empty = Map::new();
    for action in actions {
        let result_failed = action
            .get("result")
            .and_then(Value::as_object)
            .is_some_and(|r| r.get("ok") == Some(&Value::Bool(false)));
        if truthy(action.get("error")) || result_failed {
            failed += 1;
            continue;
        }
        let tool = or_empty(action.get("tool"));
        let tool = tool.strip_prefix("ferb_").unwrap_or(&tool).to_owned();
        if tool.is_empty() {
            continue;
        }
        let args = action
            .get("args")
            .filter(|v| ferb_py::value::truthy(v))
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let target = step_target(args);
        let label = if target.is_empty() {
            tool
        } else {
            format!("{tool}({target})")
        };
        match steps.last_mut() {
            Some((last, n)) if *last == label => *n += 1,
            _ => steps.push((label, 1)),
        }
    }
    let shown: Vec<String> = steps
        .iter()
        .take(HOW_STEPS)
        .map(|(l, n)| {
            if *n > 1 {
                format!("{l} ×{n}")
            } else {
                l.clone()
            }
        })
        .collect();
    let more = steps.len() - shown.len();
    let mut text = shown.join(" → ");
    if more > 0 {
        text.push_str(&format!(" → … {more} more"));
    }
    if failed > 0 && !text.is_empty() {
        text.push_str(&format!(
            " ({failed} call{} failed)",
            if failed != 1 { "s" } else { "" }
        ));
    }
    text
}

/// learning.py:184 `how_note`.
pub fn how_note(body: &Value) -> String {
    let mut parts = Vec::new();
    if !pystr::strip(&or_empty(body.get("how"))).is_empty() {
        parts.push(one_line(body.get("how")));
    }
    let hints: Vec<String> = body
        .get("owner_hints")
        .filter(|v| ferb_py::value::truthy(v))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|h| !pystr::strip(&py::py_str(h)).is_empty())
        .map(|h| one_line(Some(h)))
        .collect();
    if !hints.is_empty() {
        let quoted: Vec<String> = hints.iter().map(|h| format!("“{h}”")).collect();
        parts.push(format!(
            "The owner's hints (their words): {}",
            quoted.join(" / ")
        ));
    }
    let actions = body
        .get("actions")
        .filter(|v| ferb_py::value::truthy(v))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let steps = recorded_steps(actions);
    if !steps.is_empty() {
        parts.push(format!("Recorded steps: {steps}"));
    }
    let text = parts.join(" ");
    if pystr::char_len(&text) <= HOW_CHARS {
        text
    } else {
        format!("{}…", pystr::rstrip(pystr::prefix(&text, HOW_CHARS - 1)))
    }
}

fn words(values: &[Option<&Value>]) -> String {
    values
        .iter()
        .filter_map(|v| match v {
            Some(Value::String(s)) if !pystr::strip(s).is_empty() => Some(one_line(*v)),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// learning.py:200 `search_text`.
pub fn search_text(kind: &str, key: &str, body: &Value) -> String {
    let s = |text: String| Value::String(text);
    match kind {
        "fact" => {
            let subject = s(slug_words(&or_empty(get(body, "subject"))));
            let key = s(slug_words(&or_empty(get(body, "key"))));
            words(&[get(body, "content"), Some(&subject), Some(&key)])
        }
        "entity" | "identity" | "identity_proposal" => {
            let k = s(key.to_owned());
            words(&[Some(&k), get(body, "text")])
        }
        "episode" => {
            let how = s(how_note(body));
            words(&[
                get(body, "intent"),
                get(body, "narrative"),
                get(body, "status"),
                Some(&how),
            ])
        }
        "playbook" => words(&[get(body, "title"), get(body, "text")]),
        "episode_annotation" => words(&[get(body, "reflection"), get(body, "narrative")]),
        "activity_summary" => words(&[get(body, "text")]),
        _ => String::new(),
    }
}

/// learning.py:221 `shown_text`: `node` holds `kind`, `key`, `body` and `recorded_at`.
pub fn shown_text(kind: &str, key: &str, body: &Value, recorded_at: &str) -> String {
    match kind {
        "fact" => format!("{key} — {}", one_line(body.get("content"))),
        "episode" => {
            let when_v = [body.get("ended_at"), body.get("started_at")]
                .into_iter()
                .find(|v| truthy(*v))
                .flatten()
                .map_or_else(|| recorded_at.to_owned(), py::py_str);
            let when = pystr::prefix(&when_v, 10);
            let verified = body.get("verified") == Some(&Value::Bool(true));
            let status = {
                let s = or_empty(body.get("status"));
                if s.is_empty() {
                    "unknown".to_owned()
                } else {
                    s
                }
            };
            let outcome = status + if verified { ", verified" } else { "" };
            let how = if verified {
                how_note(body)
            } else {
                String::new()
            };
            let task = {
                let t = or_empty(body.get("task_id"));
                if t.is_empty() {
                    key.to_owned()
                } else {
                    t
                }
            };
            let mut out = format!(
                "past task {task} ({when}): {} — {outcome}",
                one_line(body.get("intent"))
            );
            if !how.is_empty() {
                out.push_str(&format!(" — how it was done: {how}"));
            }
            out
        }
        "playbook" => {
            let runs = body
                .get("tasks")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!(
                "playbook “{}” (from {runs} verified runs): {}",
                one_line(body.get("title")),
                one_line(body.get("text"))
            )
        }
        "episode_annotation" => format!("about task {key}: {}", one_line(body.get("reflection"))),
        _ => {
            let text = one_line(body.get("text"));
            if text.is_empty() {
                key.to_owned()
            } else {
                format!("{key}: {text}")
            }
        }
    }
}

/// learning.py:323 `shown_state`.
pub fn shown_state(kind: &str, body: &Value, state: &str) -> String {
    if kind == "fact" && truthy(body.get("unconfirmed_hint")) {
        "unconfirmed hint".into()
    } else {
        state.into()
    }
}

/// learning.py:318 `memory_origin`.
pub fn memory_origin(label: &str) -> String {
    match label {
        "owner_stated" => origin::OWNER.into(),
        "observed" => origin::PRIVATE.into(),
        "inferred" => origin::INFERRED.into(),
        "third_party" => origin::external("memory", ""),
        _ => origin::external("unlabeled", ""),
    }
}
