//! The daemon's side of the intro's "Try" beat: `intro.suggest`, two look-only first tasks that fit what FERB saw
//! on this Mac (topped up with general tasks; DMN-500, DMN-542), and `intro.preview`, the old plan-only preview (no
//! longer used by FERB.app). Port of `ferb/daemon/areas/intro.py`. Must not act, start a task or record chat; both
//! are FERB.app's only (auth.APP_ONLY).

use std::sync::Arc;
use std::time::Duration;

use ferb_rpc::{BoxFuture, EventStream, HandlerResult, Server};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use super::core::{ok, refusal, Host, P};

/// Shown when the scan isn't ready or found nothing clear, and to top up a reply with fewer than two: one on the
/// Mac and its files, one in the browser (owner, 2026-11-01).
pub const GENERAL_TASKS: [&str; 2] = [
    "Find the five biggest files in my Downloads folder and tell me what each is, without changing anything.",
    "Compare three well-reviewed electric kettles on the web and tell me which is the best value.",
];
/// The one main app each general task would use, "" for the owner's default browser (APP-693).
pub const GENERAL_APPS: [&str; 2] = ["Finder", ""];
pub const APP_NAME_MAX: usize = 60;
pub const REQUEST_MAX: usize = 280;
/// The preview's planner session (`continuity.PREVIEW_SESSION` + "intro").
pub const INTRO_SESSION: &str = "preview:intro";
/// The suggestion's model call ends just before the app stops waiting (`server.LONG_WAITS["intro.suggest"]` - 5).
pub const SUGGEST_LIMIT_S: f64 = 55.0;

/// The answer schema of the suggestion call.
pub fn suggest_schema() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["requests"],
           "properties": {"requests": {"type": "array", "items": {
               "type": "object", "additionalProperties": false, "required": ["request", "app"],
               "properties": {"request": {"type": "string"}, "app": {"type": "string"}}}}}})
}

/// `_key`: two requests that differ only in case or spacing are the same.
fn key(request: &str) -> String {
    ferb_py::value::squeeze(&ferb_py_casefold(request))
}

/// Python's `str.casefold()` for the characters suggestions use (Unicode full case folding; `ß` → `ss`).
fn ferb_py_casefold(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            'ß' => "ss".chars().collect::<Vec<_>>(),
            'ẞ' => "ss".chars().collect(),
            _ => c.to_lowercase().collect(),
        })
        .collect()
}

/// `_within`: single-spaced `words` cut to at most `limit` characters at a word boundary.
pub fn within(words: &str, limit: usize) -> String {
    let chars: Vec<char> = words.chars().collect();
    if chars.len() <= limit {
        return words.to_owned();
    }
    let mut head: String = chars[..limit].iter().collect();
    if chars[limit] != ' ' && head.contains(' ') {
        head = head
            .rsplit_once(' ')
            .map(|(h, _)| h.to_owned())
            .unwrap_or(head);
    }
    head.trim_end().to_owned()
}

/// `two`: the scan's first two distinct requests, topped up from GENERAL_TASKS; and how many came from the scan.
pub fn two(from_scan: &[(String, String)]) -> (Vec<(String, String)>, usize) {
    let mut chosen: Vec<(String, String)> = vec![];
    for (request, app) in from_scan {
        if !request.is_empty()
            && !chosen.iter().any(|(c, _)| key(c) == key(request))
            && chosen.len() < 2
        {
            chosen.push((request.clone(), app.clone()));
        }
    }
    let used = chosen.len();
    for (request, app) in GENERAL_TASKS.iter().zip(GENERAL_APPS) {
        if chosen.len() < 2 && !chosen.iter().any(|(c, _)| key(c) == key(request)) {
            chosen.push((request.to_string(), app.to_string()));
        }
    }
    (chosen, used)
}

/// The suggestion call failed: the exception's type name (Python journals `type(exc).__name__`).
pub struct CallFailed(pub String);

/// A preview event, or why the preview ended: a FerbError's words (`ferb`), or the owner's line for any other
/// failure (`couldnt("show that plan", exc, redact)`, worded by the supervisor's side).
pub enum PreviewEnd {
    Ferb(String),
    Other(String),
}

/// `supervisor.preview(session, request)`: its events, then the end of the stream (or why it failed).
pub type PreviewStart =
    Arc<dyn Fn(&str, &str) -> mpsc::Receiver<Result<Map<String, Value>, PreviewEnd>> + Send + Sync>;

/// The suggestion's model call: (system, context) → the answer text.
pub type Ask =
    Arc<dyn Fn(String, String) -> BoxFuture<'static, Result<String, CallFailed>> + Send + Sync>;

pub struct Deps {
    /// `_mac_profile_service().status()` (MEM-13).
    pub scan: Arc<dyn Fn() -> Value + Send + Sync>,
    /// The suggestion's tool-less model call (`learning_call` on the orchestrator's runtime with SUGGEST_SCHEMA,
    /// role intro_suggest): (system, the redacted scan as JSON) → the answer text.
    pub ask: Ask,
    /// `load_prompt("orchestrator/prompts/intro.md", {"capabilities": supervisor.availability().text})`.
    pub system: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    pub limit: Duration,
    /// `self._shutting_down`.
    pub shutting_down: Arc<dyn Fn() -> bool + Send + Sync>,
    /// `supervisor.preview(INTRO_SESSION, request)`; None when the supervisor isn't started.
    pub preview: Option<PreviewStart>,
}

pub fn register(server: &Server, host: &Host, deps: &Arc<Deps>) {
    let (h, d) = (host.clone(), deps.clone());
    server.register_unary("intro.suggest", move |_s, _p: Map<String, Value>| {
        let (h, d) = (h.clone(), d.clone());
        async move { suggest(&h, &d).await }
    });
    let (h, d) = (host.clone(), deps.clone());
    server.register_stream(
        "intro.preview",
        move |_s, p: Map<String, Value>| -> Box<dyn EventStream> { preview(&h, &d, &p) },
    );
}

fn words(h: &Host, v: Option<&Value>) -> String {
    let text = match v {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => String::new(),
    };
    ferb_py::value::squeeze(&h.redact_text(&text))
}

/// `_intro_suggest`: never fails the app; each outcome journaled `intro.suggested` (never the summary).
async fn suggest(h: &Host, d: &Deps) -> HandlerResult {
    let seen = (d.scan)();
    let mut why = String::new();
    let (mut summary, mut facts) = (String::new(), Vec::<String>::new());
    if seen.get("state") != Some(&json!("seen")) {
        why = format!(
            "scan {}",
            seen.get("state")
                .map(ferb_py::value::py_str)
                .unwrap_or("None".into())
        );
    } else {
        summary = words(h, seen.get("summary"));
        if let Some(Value::Array(fs)) = seen.get("facts") {
            for f in fs.iter().filter_map(Value::as_object) {
                let w = ferb_py::value::squeeze(
                    &h.redact_text(
                        &f.get("text")
                            .map(ferb_py::value::py_str)
                            .unwrap_or_default(),
                    ),
                );
                if !w.is_empty() {
                    facts.push(w);
                }
            }
        }
        if summary.is_empty() && facts.is_empty() {
            why = "nothing seen".into();
        }
    }
    let mut from_scan = vec![];
    if why.is_empty() {
        match tokio::time::timeout(d.limit, ask(h, d, &summary, &facts)).await {
            Err(_) => why = "timed out".into(),
            Ok(Err(CallFailed(kind))) => why = kind,
            Ok(Ok(found)) => from_scan = found,
        }
    }
    let (chosen, used) = two(&from_scan);
    if why.is_empty() && used < 2 {
        why = if used > 0 {
            "topped up"
        } else {
            "nothing clear"
        }
        .into();
    }
    let mut row = json!({"fits": used > 0, "from_scan": used});
    if !why.is_empty() {
        row["why"] = json!(why);
    }
    h.journal("intro.suggested", row, None)?;
    let suggestions: Vec<Value> = chosen
        .iter()
        .map(|(r, a)| json!({"request": r, "app": a}))
        .collect();
    ok(json!({"suggestions": suggestions, "fits": used > 0}))
}

/// `_suggest`: the model's requests, redacted, single-spaced and cut.
async fn ask(
    h: &Host,
    d: &Deps,
    summary: &str,
    facts: &[String],
) -> Result<Vec<(String, String)>, CallFailed> {
    let system = (d.system)().map_err(|_| CallFailed("OSError".into()))?;
    let ctx = ferb_py::json::dumps_with(
        &json!({"summary": summary, "facts": facts}),
        ferb_py::json::Style {
            ascii: false,
            sort_keys: false,
            indent: Some(1),
        },
    );
    let text = (d.ask)(system, ctx).await?;
    let answer = ferb_py::json::loads(&text).map_err(|_| CallFailed("JSONDecodeError".into()))?;
    let mut found = vec![];
    if let Some(Value::Array(items)) = answer.get("requests") {
        for item in items.iter().filter(|i| i.is_object()) {
            let request = within(&words(h, item.get("request")), REQUEST_MAX);
            let app = ferb_py::value::prefix(&words(h, item.get("app")), APP_NAME_MAX).to_owned();
            found.push((request, app));
        }
    }
    Ok(found)
}

/// `_intro_preview`: the plan as the real chat shows it, every event marked `preview: true`, nothing done.
fn preview(h: &Host, d: &Deps, p: &Map<String, Value>) -> Box<dyn EventStream> {
    let fail = |text: &str| -> Box<dyn EventStream> {
        let (tx, rx) = mpsc::channel(1);
        let _ = tx.try_send(Err(refusal(text)));
        Box::new(rx)
    };
    if (d.shutting_down)() {
        return fail("daemon is shutting down; no preview was started");
    }
    let Some(start) = &d.preview else {
        return fail("daemon not started");
    };
    let request = ferb_py::value::squeeze(&h.redact_text(&P(p).text_or_empty("request")));
    if request.is_empty() {
        return fail("request must be a nonempty task");
    }
    let mut events = start(INTRO_SESSION, &request);
    let (tx, rx) = mpsc::channel(16);
    let h = h.clone();
    tokio::spawn(async move {
        while let Some(e) = events.recv().await {
            let out = match e {
                Ok(ev) => Ok(ev),
                Err(PreviewEnd::Ferb(text)) => Err(refusal(text)),
                Err(PreviewEnd::Other(line)) => Err(refusal(h.redact_text(&line))),
            };
            let end = out.is_err();
            if tx.send(out).await.is_err() || end {
                break;
            }
        }
    });
    Box::new(rx)
}
