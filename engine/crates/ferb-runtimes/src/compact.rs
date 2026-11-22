//! What a tool loop sends the model again on each step (owner 2026-11-22, RUN-468..RUN-472): every request of a
//! tool loop carries every earlier tool result, so a screen read twenty steps ago is paid for on every later step.
//! A [`Compactor`] keeps one run's plan of results to shorten: a screenshot or snapshot of a window or tab that has a
//! newer one of the same kind is replaced by one line saying so; a large read older than a few steps by one line
//! with its start and how to read it again. Nothing else in the request changes.
//!
//! The provider caches the unchanged beginning of each request, so the plan grows only in batches: a compaction is
//! made when the request is past `start_tokens` and the new replacements remove at least `gain_tokens`, and the same
//! replacements (same bytes) are applied to every later request, so the prefix stays the same between compactions
//! and each compaction breaks the cache once, at its first replaced result. Results of the last `keep_steps` model
//! calls, the newest screen of each window or tab, and the results of the tools that carry the owner's words or
//! FERB's instructions are never replaced.
//!
//! The rule is shared; each wire shape's reading and rewriting lives in its adapter (AGENTS.md rule 12):
//! `api::anthropic` (the Anthropic API loop and Claude Code's requests through FERB's relay), `api::openai` and
//! `api::google`. Codex runs are not compacted for now (RUN-472). Code decides from structural facts
//! only (the tool's name, the window or tab id its result reports), never from what a result says (rule 4).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde_json::{json, Map, Value};

/// The numbers that change behaviour (the owner's: RUN-471).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// No compaction while the request is estimated below this many tokens.
    pub start_tokens: u64,
    /// A compaction is made only when it removes at least this many tokens (each one costs one cache break).
    pub gain_tokens: u64,
    /// Results of the last this-many model calls are never replaced.
    pub keep_steps: u32,
    /// A read result is "large" past this many characters...
    pub read_chars: usize,
    /// ...and is shortened once it is at least this many model calls old.
    pub read_age: u32,
}

impl Policy {
    /// The owner's numbers (RUN-471, "numbers ok"): measured on the recorded Codex run a843aa54, input tokens -43%
    /// with 7 cache breaks (engine/JOURNAL.md).
    pub const OWNERS: Policy = Policy {
        start_tokens: 40_000,
        gain_tokens: 10_000,
        keep_steps: 2,
        read_chars: 2_000,
        read_age: 4,
    };
}

/// Tokens a provider counts for one image, as estimated (gpt-5.6-sol: 2,050 measured on run a843aa54; Claude
/// counts about width x height / 750, about 2,180 for FERB's 1568 x 1044 screenshots).
pub const IMAGE_TOKENS: u64 = 2_000;
/// Characters per token for FERB's JSON results (2.5 to 4.0 measured on run a843aa54; snapshots 2.9 to 3.1).
pub const CHARS_PER_TOKEN: u64 = 3;

/// The tools whose results are never replaced: the owner's words and answers (questions, the conversation), and the
/// instructions FERB gives the model (abilities, skills, memory). Wire names, as `codex::mcp_name` writes them.
pub const KEPT_TOOLS: [&str; 9] = [
    "owner_ask",
    "chat_history",
    "conversation_read",
    "abilities_read",
    "skills_read",
    "memory_read",
    "memory_learned",
    // Codex's own question to the owner, and the answer tools
    "request_user_input",
    crate::answer_rules::ANSWER_TOOL,
];

/// One tool result as a request carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    /// The call's id in the wire (tool_use_id, call_id, or a position the wire module makes stable).
    pub call_id: String,
    /// The tool's wire name, without the `mcp__ferb__` (Claude Code) or `mcp__ferb.` (Codex) prefix.
    pub tool: String,
    pub args: Value,
    /// The result's text (FERB writes `json.dumps(content)` then a newline and the footer).
    pub text: String,
    pub images: usize,
}

impl Seen {
    pub fn tokens(&self) -> u64 {
        text_tokens(&self.text) + self.images as u64 * IMAGE_TOKENS
    }
}

/// A tool's wire name without the MCP server prefix the model programs add.
pub fn tool_name(name: &str, namespace: Option<&str>) -> String {
    let name = name
        .strip_prefix("mcp__ferb__")
        .or_else(|| name.strip_prefix("mcp__ferb."))
        .unwrap_or(name);
    match namespace {
        Some(ns) if !ns.is_empty() && ns != "mcp__ferb" && ns != "mcp__ferb__" => {
            format!("{ns}.{name}")
        }
        _ => name.to_string(),
    }
}

pub fn text_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(CHARS_PER_TOKEN)
}

/// A whole request's estimated size: every text's characters, every image at [`IMAGE_TOKENS`].
pub fn estimate(body: &Value) -> u64 {
    fn walk(v: &Value, chars: &mut u64, images: &mut u64) {
        match v {
            Value::String(s) => {
                if s.starts_with("data:image/") {
                    *images += 1;
                } else {
                    *chars += s.chars().count() as u64;
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, chars, images)),
            Value::Object(m) => {
                let kind = m.get("type").and_then(Value::as_str);
                if matches!(kind, Some("image" | "input_image")) || m.contains_key("inlineData") {
                    *images += 1;
                    return;
                }
                m.values().for_each(|x| walk(x, chars, images));
            }
            _ => {}
        }
    }
    let (mut chars, mut images) = (0, 0);
    walk(body, &mut chars, &mut images);
    chars.div_ceil(CHARS_PER_TOKEN) + images * IMAGE_TOKENS
}

/// One compaction, for the trace (`model.compacted` with `by: "ferb"`).
#[derive(Debug, Clone, PartialEq)]
pub struct Compaction {
    /// The model call (1-based) whose request was compacted.
    pub step: u32,
    pub screens: usize,
    pub reads: usize,
    pub tokens_before: u64,
    pub tokens_after: u64,
    /// How many of the request's results come before the first one replaced now: where the cached prefix ends.
    pub kept_before: usize,
}

impl Compaction {
    pub fn as_event(&self) -> Value {
        json!({"by": "ferb", "step": self.step, "screens_replaced": self.screens,
            "reads_shortened": self.reads, "estimated_tokens_before": self.tokens_before,
            "estimated_tokens_after": self.tokens_after, "results_before_first_change": self.kept_before})
    }
}

/// The clock the placeholders name a time with ("14:21:03", local).
pub type Clock = Arc<dyn Fn() -> String + Send + Sync>;

pub fn local_clock() -> Clock {
    Arc::new(|| chrono::Local::now().format("%H:%M:%S").to_string())
}

/// What one result is to the rule.
#[derive(Debug, Clone, PartialEq)]
enum Kind {
    /// A look at one window or tab: newer of the same kind and key replaces it.
    Screen {
        key: String,
        label: String,
        image: bool,
        /// Where it looked, to tell when that window or tab is gone.
        place: Place,
    },
    Other,
}

/// A window or tab a look was of.
#[derive(Debug, Clone, PartialEq)]
enum Place {
    /// (app bundle, window id): gone when the app's newest snapshot lists its windows without it.
    Window(String, String),
    /// (`browser` or `ferb_browser`, tab id): gone once that side's close of it succeeded.
    Tab(String, String),
}

fn id_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// The line holding FERB's `json.dumps(content)`: the first line that is a JSON object (Codex puts its own
/// "Wall time: ... Output:" lines before it).
fn content_line(text: &str) -> Option<(&str, Map<String, Value>)> {
    text.split('\n')
        .filter(|l| l.trim_start().starts_with('{'))
        .find_map(|l| match serde_json::from_str::<Value>(l.trim()) {
            Ok(Value::Object(m)) => Some((l, m)),
            _ => None,
        })
}

fn content(text: &str) -> Option<Map<String, Value>> {
    content_line(text).map(|(_, m)| m)
}

fn classify(seen: &Seen) -> (Kind, Option<Map<String, Value>>) {
    let c = content(&seen.text);
    let Some(m) = c.as_ref().filter(|m| !m.contains_key("error")) else {
        return (Kind::Other, c);
    };
    let arg = |k: &str| seen.args.get(k).map(id_text).unwrap_or_default();
    let field = |k: &str| m.get(k).map(id_text).unwrap_or_default();
    let tool = seen.tool.as_str();
    let kind = match tool {
        "mac_screenshot" if seen.images > 0 => {
            let (bundle, window) = (field("bundle_id"), field("window_id"));
            Kind::Screen {
                key: format!("mac-image|{bundle}|{window}"),
                label: format!(
                    "screenshot of {} (window {window})",
                    or(&field("app"), &bundle)
                ),
                image: true,
                place: Place::Window(bundle, window),
            }
        }
        // a later page of the same snapshot is a read, not a new look
        "mac_snapshot" if m.get("offset").and_then(Value::as_i64).unwrap_or(0) == 0 => {
            let bundle = field("bundle_id");
            let current = m
                .get("windows")
                .and_then(Value::as_array)
                .and_then(|ws| {
                    ws.iter()
                        .find(|w| w.get("current").and_then(Value::as_bool) == Some(true))
                })
                .and_then(|w| w.get("id"))
                .map(id_text)
                .unwrap_or_default();
            let window = [arg("window"), current]
                .into_iter()
                .find(|w| !w.is_empty())
                .unwrap_or_else(|| field("window"));
            let title = field("window");
            let label = if title.is_empty() || title == window {
                format!(
                    "snapshot of {} (window {window})",
                    or(&field("app"), &bundle)
                )
            } else {
                format!(
                    "snapshot of {} (window {window}, {})",
                    or(&field("app"), &bundle),
                    crate::pyjson::repr_str(&title)
                )
            };
            Kind::Screen {
                key: format!("mac-tree|{bundle}|{window}"),
                label,
                image: false,
                place: Place::Window(bundle, window),
            }
        }
        "browser_snapshot"
        | "ferb_browser_snapshot"
        | "browser_screenshot"
        | "ferb_browser_screenshot" => {
            let image = tool.ends_with("screenshot");
            if image && seen.images == 0 {
                return (Kind::Other, c);
            }
            let tab = arg("tab");
            let side = tool
                .trim_end_matches("_snapshot")
                .trim_end_matches("_screenshot");
            let title = field("title");
            let what = if image { "screenshot" } else { "snapshot" };
            Kind::Screen {
                key: format!("{side}-{what}|{tab}"),
                label: if title.is_empty() {
                    format!("{what} of tab {tab}")
                } else {
                    format!("{what} of tab {tab} ({})", crate::pyjson::repr_str(&title))
                },
                image,
                place: Place::Tab(side.to_string(), tab),
            }
        }
        _ => Kind::Other,
    };
    (kind, c)
}

fn or(a: &str, b: &str) -> String {
    if a.is_empty() { b } else { a }.to_string()
}

/// The windows the newest snapshot of each app lists (bundle -> ids), when it lists them.
fn listed_windows(seen: &[Seen]) -> HashMap<String, HashSet<String>> {
    let mut out = HashMap::new();
    for s in seen.iter().filter(|s| s.tool == "mac_snapshot") {
        let Some(m) = content(&s.text) else { continue };
        let Some(ws) = m.get("windows").and_then(Value::as_array) else {
            continue;
        };
        let bundle = m.get("bundle_id").map(id_text).unwrap_or_default();
        out.insert(
            bundle,
            ws.iter()
                .filter_map(|w| w.get("id").map(id_text))
                .collect::<HashSet<_>>(),
        );
    }
    out
}

/// The tabs a close took away (`browser_close` / `ferb_browser_close` with no error), as (side, tab).
fn closed_tabs(seen: &[Seen]) -> HashSet<(String, String)> {
    seen.iter()
        .filter_map(|s| {
            let side = s.tool.strip_suffix("_close")?;
            if side != "browser" && side != "ferb_browser" {
                return None;
            }
            let m = content(&s.text).filter(|m| !m.contains_key("error"))?;
            let tab = s
                .args
                .get("tab")
                .or_else(|| m.get("tab"))
                .map(id_text)
                .filter(|t| !t.is_empty())?;
            Some((side.to_string(), tab))
        })
        .collect()
}

/// The line a superseded screen becomes.
pub fn screen_line(label: &str, at: &str) -> String {
    format!("FERB: an earlier {label} taken at {at}; a newer one is below.")
}

/// The line a large old read becomes: its start, its size and how to read it again.
pub fn read_line(tool: &str, at: &str, text: &str, evidence: &str) -> String {
    let body = content_line(text)
        .map(|(l, _)| l)
        .or_else(|| text.split('\n').find(|l| !l.trim().is_empty()))
        .unwrap_or_default();
    let head: String = body.chars().take(HEAD_CHARS).collect();
    let more = if body.chars().count() > HEAD_CHARS {
        "…"
    } else {
        ""
    };
    let again = match evidence {
        "" => format!(
            "call {tool} again for a fresh read, or task_evidence with kind \"tool.call\" and query \"{}\" for this one",
            capability(tool)
        ),
        id => format!(
            "call {tool} again for a fresh read, or task_evidence with kind \"tool.call\" and query \"{id}\" for this one"
        ),
    };
    format!(
        "FERB: this {tool} result from {at} ({} characters) was shortened to save room. It began: {head}{more} To see it again, {again}.",
        text.chars().count()
    )
}

/// The capability name a wire name stands for, as FERB's records name it (`mac_menu` -> `mac.menu`,
/// `ferb_browser_read` -> `ferb_browser.read`).
pub fn capability(tool: &str) -> String {
    match tool.strip_prefix("ferb_browser_") {
        Some(rest) => format!("ferb_browser.{rest}"),
        None => tool.replacen('_', ".", 1),
    }
}

/// How much of a shortened read's start is kept.
pub const HEAD_CHARS: usize = 200;

/// One run's compaction plan.
pub struct Compactor {
    policy: Policy,
    clock: Clock,
    step: u32,
    /// call id -> (model call it first appeared in, the time it was first seen)
    first: HashMap<String, (u32, String)>,
    plan: BTreeMap<String, String>,
    made: Vec<Compaction>,
}

impl Compactor {
    pub fn new(policy: Policy, clock: Clock) -> Compactor {
        Compactor {
            policy,
            clock,
            step: 0,
            first: HashMap::new(),
            plan: BTreeMap::new(),
            made: Vec::new(),
        }
    }

    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// The replacement for one result's whole content, once planned (the same bytes on every later request).
    pub fn replacement(&self, call_id: &str) -> Option<&str> {
        self.plan.get(call_id).map(String::as_str)
    }

    pub fn plan(&self) -> &BTreeMap<String, String> {
        &self.plan
    }

    /// The compactions made so far, oldest first.
    pub fn made(&self) -> &[Compaction] {
        &self.made
    }

    /// One model request: `results` are the tool results it carries, in order, as they were (before any
    /// replacement); `tokens` the request's estimated size with the current plan applied. Returns the compaction
    /// made now, if any; the caller then applies the plan again.
    pub fn step(&mut self, results: &[Seen], tokens: u64) -> Option<Compaction> {
        self.step += 1;
        let now = self.step;
        for r in results {
            if !self.first.contains_key(&r.call_id) {
                let at = (self.clock)();
                self.first.insert(r.call_id.clone(), (now, at));
            }
        }
        if tokens < self.policy.start_tokens {
            return None;
        }
        let classified: Vec<(Kind, Option<Map<String, Value>>)> =
            results.iter().map(classify).collect();
        let mut newest: HashMap<&str, usize> = HashMap::new();
        for (i, (k, _)) in classified.iter().enumerate() {
            if let Kind::Screen { key, .. } = k {
                newest.insert(key.as_str(), i);
            }
        }
        let listed = listed_windows(results);
        let closed = closed_tabs(results);
        let gone = |place: &Place| match place {
            Place::Window(bundle, window) => listed
                .get(bundle)
                .is_some_and(|ids| !window.is_empty() && !ids.contains(window)),
            Place::Tab(side, tab) => closed.contains(&(side.clone(), tab.clone())),
        };
        let mut new: Vec<(usize, String, bool)> = Vec::new();
        let mut gain: u64 = 0;
        for (i, (r, (kind, c))) in results.iter().zip(&classified).enumerate() {
            if self.plan.contains_key(&r.call_id) || KEPT_TOOLS.contains(&r.tool.as_str()) {
                continue;
            }
            let (born, at) = self
                .first
                .get(&r.call_id)
                .cloned()
                .unwrap_or((now, String::new()));
            let age = now - born;
            if age < self.policy.keep_steps {
                continue;
            }
            let large = r.text.chars().count() > self.policy.read_chars;
            let old = age >= self.policy.read_age;
            let evidence = c
                .as_ref()
                .and_then(|m| m.get("evidence"))
                .map(id_text)
                .unwrap_or_default();
            let line = match kind {
                Kind::Screen { key, label, .. } if newest.get(key.as_str()) != Some(&i) => {
                    Some((screen_line(label, &at), true))
                }
                Kind::Screen { place, .. } if large && old && gone(place) => {
                    // the window or tab is gone: its last look is an old read
                    Some((read_line(&r.tool, &at, &r.text, &evidence), false))
                }
                Kind::Screen { .. } => None,
                Kind::Other if large && old => {
                    Some((read_line(&r.tool, &at, &r.text, &evidence), false))
                }
                Kind::Other => None,
            };
            // a line no shorter than the result would only cost a cache break
            if let Some((line, screen)) = line.filter(|(l, _)| text_tokens(l) < r.tokens()) {
                gain += r.tokens() - text_tokens(&line);
                new.push((i, line, screen));
            }
        }
        if new.is_empty() || gain < self.policy.gain_tokens {
            return None;
        }
        let made = Compaction {
            step: now,
            screens: new.iter().filter(|(_, _, s)| *s).count(),
            reads: new.iter().filter(|(_, _, s)| !*s).count(),
            tokens_before: tokens,
            tokens_after: tokens.saturating_sub(gain),
            kept_before: new.iter().map(|(i, _, _)| *i).min().unwrap_or(0),
        };
        for (i, line, _) in new {
            self.plan.insert(results[i].call_id.clone(), line);
        }
        self.made.push(made.clone());
        Some(made)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_lose_the_mcp_prefix_only() {
        assert_eq!(tool_name("mcp__ferb__mac_snapshot", None), "mac_snapshot");
        assert_eq!(tool_name("mac_snapshot", Some("mcp__ferb")), "mac_snapshot");
        assert_eq!(tool_name("mcp__ferb.mac_snapshot", None), "mac_snapshot");
        assert_eq!(tool_name("mac_snapshot", None), "mac_snapshot");
        assert_eq!(tool_name("x", Some("other")), "other.x");
    }

    #[test]
    fn the_estimate_counts_images_not_their_bytes() {
        let body = json!({"messages": [{"content": [{"type": "text", "text": "abcdef"},
            {"type": "image", "source": {"data": "A".repeat(90_000)}}]},
            {"output": [{"type": "input_image", "image_url": "data:image/jpeg;base64,AAAA"}]},
            {"parts": [{"inlineData": {"data": "B".repeat(1000)}}]}]});
        // the values "text" and "abcdef" (10 characters), three images; keys and image bytes are not counted
        assert_eq!(estimate(&body), 10u64.div_ceil(3) + 3 * IMAGE_TOKENS);
    }
}
