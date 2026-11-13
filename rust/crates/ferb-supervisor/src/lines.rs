//! supervisor.py's module-level functions: the owner-facing lines composed in code (reminders, refusals, sleeping
//! and stop lines, the "which is it?" questions), the short forms the planner's history shows, and the small text
//! rules around them. Each is checked against `rust/fixtures/supervisor/units.json`.

use std::sync::LazyLock;

use ferb_clock::py::{fmt, Time};
use ferb_clock::zone::Zone;
use ferb_context::assemble::ContextBlock;
use ferb_context::tokens::Tokenizer;
use ferb_orchestrator::answer_marks::{headline, NOT_FINISHED, STOPPED, WHERE_STOOD};
use ferb_orchestrator::lines::{choices, shorten};
use ferb_orchestrator::py::{rstrip, rstrip_chars, split_ws, squeeze, strip};
use ferb_orchestrator::{Condition, Step};
use regex::Regex;
use serde_json::{Map, Value};

use crate::words::*;

/// supervisor.py:220 `sleeping_line` (ORCH-598): a sleeping task's status line.
pub fn sleeping_line(asleep: &Map<String, Value>) -> String {
    let get = |k: &str| asleep.get(k).map(py_str).unwrap_or_default();
    if truthy(asleep.get("line")) {
        return get("line");
    }
    let mut out = format!("waiting until {}", get("until"));
    if truthy(asleep.get("reason")) {
        out.push_str(&format!(": “{}”", get("reason")));
    }
    out
}

/// supervisor.py:313 `_quoted`: model-written names on one line, each in quotes.
pub fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("“{}”", squeeze(n)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// supervisor.py:318 `_with_reasons`.
pub fn with_reasons(refused: &[(String, String)]) -> String {
    let why = |t: &str| rstrip_chars(&squeeze(t), ".").to_string();
    if refused.len() == 1 {
        return format!("{}: {}", quoted(&[&refused[0].0]), why(&refused[0].1));
    }
    refused
        .iter()
        .map(|(name, reason)| format!("{} ({})", quoted(&[name]), why(reason)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// supervisor.py:327 `_not_set`.
pub fn not_set(refused: &[(String, String)]) -> String {
    if refused.is_empty() {
        return String::new();
    }
    let (what, it) = if refused.len() == 1 {
        ("the reminder", "it")
    } else {
        ("the reminders", "them")
    };
    format!(
        "I couldn't set {what} {}. Nothing is scheduled for {it}. Could you ask again?",
        with_reasons(refused)
    )
}

/// supervisor.py:376 `_not_saved`.
pub fn not_saved(refused: &[(String, String)]) -> String {
    if refused.is_empty() {
        return String::new();
    }
    format!(
        "I couldn't remember {}. {} not saved. Could you tell me again?",
        with_reasons(refused),
        if refused.len() == 1 {
            "It is"
        } else {
            "They are"
        }
    )
}

/// supervisor.py:283 `_day`: "today", "yesterday", a weekday within the week, else "3 Nov" (in `zone`).
pub fn day(at: &Time, now: &Time, zone: &Zone) -> String {
    let days = (zone.date(now) - zone.date(at)).num_days();
    if days <= 0 {
        return "today".into();
    }
    if days == 1 {
        return "yesterday".into();
    }
    let local = zone.local(at);
    if days < 7 {
        fmt(&local, "%A")
    } else {
        format!(
            "{} {}",
            chrono::Datelike::day(&local.naive_local()),
            fmt(&local, "%b")
        )
    }
}

/// A reminder or timer as the lines read it (`timer.fire_at`, `.kind`, `.message`, `.state`).
#[derive(Clone, Debug, PartialEq)]
pub struct TimerFacts {
    pub fire_at: Time,
    pub kind: String,
    pub message: String,
    pub state: String,
}

/// supervisor.py:354 `_reminder_line` (L11, ORCH-590, T-1055).
pub fn reminder_line(timer: &TimerFacts, present: bool, now: Option<&Time>, zone: &Zone) -> String {
    let local = zone.local(&timer.fire_at);
    let what = if timer.kind == "timer" {
        "Timer"
    } else {
        "Reminder"
    };
    if !present {
        return format!(
            "⏰ {what} set for {}: {}",
            fmt(&local, "%a %d %b %H:%M"),
            timer.message
        );
    }
    if let Some(now) = now {
        if timer.fire_at <= *now + chrono::TimeDelta::seconds(1) {
            return format!(
                "⏰ {what} set for the next time you're back at your Mac: {}",
                timer.message
            );
        }
    }
    let naive = local.naive_local();
    let at = if (
        chrono::Timelike::hour(&naive),
        chrono::Timelike::minute(&naive),
    ) == (0, 0)
    {
        fmt(&local, "%a %d %b")
    } else {
        fmt(&local, "%a %d %b %H:%M")
    };
    format!(
        "⏰ {what} set for {at}, when you're at your Mac: {}",
        timer.message
    )
}

/// supervisor.py:366 `_already_set`.
pub fn already_set(timer: &TimerFacts, zone: &Zone) -> String {
    let what = quoted(&[&timer.message]);
    let at = fmt(&zone.local(&timer.fire_at), "%a %d %b %H:%M");
    match timer.state.as_str() {
        "cancelled" => {
            format!("⏰ The reminder {what} for {at} was cancelled; I didn't set it again.")
        }
        "delivered" => format!("⏰ The reminder {what} already went off ({at})."),
        _ => format!("⏰ The reminder {what} is already set for {at}."),
    }
}

/// An open question as the "which is it?" lines read it (daemon/questions.py `Approval`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestionFacts {
    pub capability: String,
    pub options_only: bool,
    pub summary: String,
    pub reason: String,
    pub options: Vec<String>,
    pub labels: Vec<String>,
}

/// supervisor.py:265 `_takes_words` (ORCH-537); `ferb_checks` is steering.py's `FERB_CHECKS`.
pub fn takes_words(ap: &QuestionFacts, ferb_checks: &[&str]) -> bool {
    !ferb_checks.contains(&ap.capability.as_str()) && !ap.options_only
}

/// supervisor.py:271 `_plain_choice` (L3, GATE-566).
pub fn plain_choice(ap: &QuestionFacts) -> String {
    let question = if ap.summary.is_empty() {
        &ap.reason
    } else {
        &ap.summary
    };
    if ap.labels.iter().any(|l| !l.is_empty()) {
        let picked: Vec<&str> = ap
            .labels
            .iter()
            .zip(&ap.options)
            .map(|(l, o)| if l.is_empty() { o.as_str() } else { l.as_str() })
            .collect();
        return PLAIN_CHOICE
            .replace("{choices}", &choices(&picked))
            .replace("{question}", question);
    }
    if ap.options.is_empty() || ap.options == ["Yes", "No"] {
        return PLAIN_YES_NO.replace("{question}", question);
    }
    PLAIN_CHOICE
        .replace("{choices}", &choices(&ap.options))
        .replace("{question}", question)
}

/// supervisor.py:406 `_bare`: a reply as an option pick reads it.
pub fn bare(text: &str) -> String {
    strip(rstrip_chars(strip(text), ".!")).to_string()
}

/// supervisor.py:411 `_stored_refs`: a span's refs without the activity rows.
pub fn stored_refs(refs: &[String]) -> Vec<String> {
    refs.iter()
        .filter(|r| !r.starts_with("trail:"))
        .cloned()
        .collect()
}

/// supervisor.py:419 `_exposure_output`. `record` holds `text` (null for the conversation), optional `payload` and
/// `segments`.
pub fn exposure_output(record: &Map<String, Value>) -> Map<String, Value> {
    let text = record.get("text").cloned().unwrap_or(Value::Null);
    let payload = record
        .get("payload")
        .cloned()
        .unwrap_or_else(|| text.clone());
    let mut out = Map::new();
    if !text.is_null() {
        out.insert("text".into(), text);
    }
    out.insert("payload".into(), payload);
    if let Some(segments) = record.get("segments").filter(|s| truthy(Some(s))) {
        out.insert("segments".into(), segments.clone());
    }
    out
}

/// supervisor.py:647 `judged_kind`: the check kind of the judged condition at `index`.
pub fn judged_kind(judged: &[(usize, Condition)], index: usize) -> String {
    judged
        .iter()
        .find(|(i, _)| *i == index)
        .map(|(_, c)| c.check.kind.clone())
        .unwrap_or_default()
}

static PARAGRAPHS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n[ \t]*\n").expect("pattern"));

fn paragraphs(text: &str) -> Vec<&str> {
    PARAGRAPHS.split(text).collect()
}

/// supervisor.py:652 `without_whole` (ORCH-549).
pub fn without_whole(text: &str, piece: &str) -> String {
    let piece = strip(piece);
    if piece.is_empty()
        || (!text.split('\n').any(|l| strip(l) == piece)
            && !paragraphs(text).iter().any(|p| strip(p) == piece))
    {
        return text.to_string();
    }
    let kept: Vec<String> = paragraphs(text)
        .into_iter()
        .filter(|p| strip(p) != piece)
        .map(|p| {
            p.split('\n')
                .filter(|l| strip(l) != piece)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect();
    let mut kept = kept.join("\n\n");
    while kept.contains("\n\n\n") {
        kept = kept.replace("\n\n\n", "\n\n");
    }
    strip(&kept).to_string()
}

/// supervisor.py:666 `work_in` (HAND-204): the plan line's folder statement, `home` shown as ~.
pub fn work_in(folders: &[String], home: &str) -> String {
    let shown: Vec<String> = folders
        .iter()
        .map(|f| {
            if f == home || f.starts_with(&format!("{home}/")) {
                format!("~{}", &f[home.len()..])
            } else {
                f.clone()
            }
        })
        .collect();
    format!("Working in {}.", shown.join(", "))
}

static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:#{1,6}[ \t]+\S[^\n]*|#{1,6})$").expect("pattern"));

/// supervisor.py:692 `_heading_only` (ORCH-616).
pub fn heading_only(paragraph: &str) -> bool {
    let p = strip(paragraph);
    !p.contains('\n') && heading_match(p)
}

/// `_HEADING.fullmatch`: `.` in Python's `re` stops at "\n" only.
fn heading_match(text: &str) -> bool {
    HEADING.is_match(text) && !text.ends_with('\n')
}

/// supervisor.py:697 `_first_content_line`.
pub fn first_content_line(text: &str) -> String {
    let lines: Vec<&str> = strip(text)
        .split('\n')
        .map(strip)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .find(|l| !heading_only(l))
        .or(lines.first())
        .map(|l| l.to_string())
        .unwrap_or_default()
}

/// supervisor.py:673 `_short_report` (ORCH-303, ORCH-535, ORCH-616).
pub fn short_report(text: &str, index: bool) -> String {
    let all: Vec<&str> = strip(text)
        .split("\n\n")
        .map(strip)
        .filter(|p| !p.is_empty())
        .collect();
    if index {
        return all.first().map(|p| p.to_string()).unwrap_or_default();
    }
    let content: Vec<&str> = all.iter().copied().filter(|p| !heading_only(p)).collect();
    let paragraphs = if content.is_empty() { all } else { content };
    let mut first = paragraphs
        .first()
        .map(|p| p.to_string())
        .unwrap_or_default();
    let stood = format!("{STOPPED} {WHERE_STOOD}");
    if paragraphs.len() > 1 && (first == NOT_FINISHED || first == STOPPED || first == stood) {
        first = format!("{first} {}", paragraphs[1]);
    }
    shorten(&first, REPORT_SHORT_CHARS, false)
}

/// supervisor.py:703 `_within_tokens`: `text` cut to at most `limit` tokens, marked as cut.
pub fn within_tokens(text: &str, limit: usize, tokens: Tokenizer) -> String {
    if tokens.count(text) <= limit {
        return text.to_string();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let prefix = |n: usize| -> &str {
        if n >= chars.len() {
            text
        } else {
            &text[..chars[n].0]
        }
    };
    let (mut lo, mut hi) = (0usize, chars.len());
    let bound = limit as i64 - 20;
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if tokens.count(prefix(mid)) as i64 <= bound {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    format!(
        "{} … [cut to fit; chat.history has every entry]",
        rstrip(prefix(lo))
    )
}

/// supervisor.py:740 `sent_from`: the window block's heading when several messages are read together.
pub fn sent_from(said: &[String]) -> String {
    FRONT_WINDOW_AT.to_string()
        + &said
            .iter()
            .map(|t| format!("“{t}”"))
            .collect::<Vec<_>>()
            .join(" and ")
}

/// supervisor.py:745 `_said`: a message as a window's heading names it.
pub fn said(text: &str) -> String {
    let words = squeeze(text);
    if words.chars().count() <= SAID_CHARS {
        return words;
    }
    let head: String = words.chars().take(SAID_CHARS - 1).collect();
    format!("{}…", rstrip(&head))
}

/// supervisor.py:816 `result_reference` (ORCH-619).
pub fn result_reference(entry_id: &str, task_id: Option<&str>) -> String {
    match task_id.filter(|t| !t.is_empty()) {
        Some(t) => format!("Result id: {entry_id}; task.evidence task_id={t}"),
        None => format!("Result id: {entry_id}"),
    }
}

static HISTORY_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:\[c[0-9]+\][ \t]*|\((?:Mon|Tue|Wed|Thu|Fri|Sat|Sun) [0-9]{2} [A-Z][a-z]{2} [0-9]{2}:[0-9]{2}\)[ \t]*)")
        .expect("pattern")
});

/// supervisor.py:829 `without_history_labels` (ORCH-622). `\d` in Python's `re` is any Unicode decimal digit; the
/// tags FERB writes hold ASCII digits only, and a tag of other digits is left as it is here (ferb-py's `\d` table
/// would read it as a tag: no FERB line holds one).
pub fn without_history_labels(text: &str) -> String {
    let mut text = text.to_string();
    loop {
        let rest = text
            .trim_start_matches(ferb_orchestrator::py::is_space)
            .to_string();
        if let Some(m) = HISTORY_TAG.find(&rest) {
            text = rest[m.end()..].to_string();
        } else if let Some(label) = HISTORY_LABELS.iter().find(|l| rest.starts_with(*l)) {
            text = rest[label.len()..].to_string();
        } else {
            return text;
        }
    }
}

/// supervisor.py:1082 `_summary_blocks`: a running-summary call's context. `previous`: the latest summary's text;
/// `copied`: the entries' display lines; `today`, in the local zone.
pub fn summary_blocks(
    previous: Option<&str>,
    copied: &[String],
    today: Option<&Time>,
    zone: &Zone,
    tokens: Tokenizer,
    wrap: &dyn Fn(&str) -> String,
) -> Vec<ContextBlock> {
    let when = today
        .map(|t| format!(" Today is {}.", fmt(&zone.local(t), "%a %d %b %Y")))
        .unwrap_or_default();
    let protected = |text: String, reference: &str| ContextBlock {
        protected: true,
        ..ContextBlock::new(text, reference)
    };
    vec![
        protected(
            format!(
                "Keep the new summary within about {SUMMARY_TOKENS} tokens. Keep only the last {SUMMARY_DAYS} days \
                 of the conversation and anything still open.{when}"
            ),
            "summary bound",
        ),
        match previous {
            Some(text) => protected(
                format!(
                    "FERB's earlier running summary (data):\n{}",
                    within_tokens(text, SUMMARY_TOKENS, tokens)
                ),
                "chat.history",
            ),
            None => ContextBlock::new("", ""),
        },
        protected(
            format!(
                "Entries to fold in, oldest first (data):\n{}",
                wrap(&copied.join("\n\n"))
            ),
            "chat.history",
        ),
    ]
}

/// supervisor.py:1563 `sites_line` (D10).
pub fn sites_line(sites: &[String]) -> String {
    format!(
        "{SITES_LINE}{} site{}",
        sites.len(),
        if sites.len() == 1 { "" } else { "s" }
    )
}

/// supervisor.py:1582 `_within`: the leading lines that fit in `chars` (at least one), and how many more.
pub fn within(lines: &[String], chars: usize) -> (Vec<String>, usize) {
    let mut shown = Vec::new();
    let mut used = 0usize;
    for line in lines {
        let n = line.chars().count();
        if !shown.is_empty() && used + n > chars {
            break;
        }
        shown.push(line.clone());
        used += n + 1;
    }
    let more = lines.len() - shown.len();
    (shown, more)
}

/// supervisor.py:7600 `_stopped_plan_summary` (ORCH-626, ORCH-627). `states`: step id -> its status.
pub fn stopped_plan_summary(steps: &[Step], status_of: &dyn Fn(&str) -> Option<String>) -> String {
    let stopped = steps.iter().find(|s| {
        !matches!(
            status_of(&s.id).as_deref(),
            Some("done") | Some("pending") | Some("running")
        )
    });
    match stopped {
        None => "The plan stopped before its last step.".into(),
        Some(s) => format!(
            "The step \u{201c}{}\u{201d} couldn't be done.",
            ferb_orchestrator::plans::step_label(s)
        ),
    }
}

/// supervisor.py:7638 `_since_line` (ORCH-591).
pub fn since_line(text: &str) -> String {
    shorten(&headline(text), SINCE_LINE_CHARS, true)
}

/// supervisor.py:7651 `_sentence`: a report another sentence can follow.
pub fn sentence(text: &str) -> String {
    let t = strip(text);
    match t.chars().last() {
        None => String::new(),
        Some(c) if ".!?".contains(c) => format!("{t} "),
        Some(_) => format!("{t}. "),
    }
}

/// supervisor.py:7657 `_clipped`: the "Couldn't confirm" line's text, whole, on one line, no end period.
pub fn clipped(text: &str) -> String {
    rstrip_chars(&squeeze(text), ".").to_string()
}

/// supervisor.py:880 `_entry_payload`: an entry's own text for the sink rule (S4).
pub fn entry_payload(kind: &str, data: &Map<String, Value>) -> String {
    match kind {
        "chat.notice" => {
            // attention.py:167 `notice_payload`
            let mut out = vec![data.get("text").map(py_str).unwrap_or_default()];
            if let Some(Value::Array(quotes)) = data.get("quotes") {
                for q in quotes {
                    if let Value::Object(q) = q {
                        out.push(q.get("text").map(py_str).unwrap_or_default());
                    }
                }
            }
            out.join("\n")
        }
        "timer.delivered" => data.get("message").map(py_str).unwrap_or_default(),
        _ => data.get("text").map(py_str).unwrap_or_default(),
    }
}

/// supervisor.py:861 `_stored_values`: a derived row's stored owner values (GATE-519); None for the owner's own
/// message, a notice and a row from before stored provenance.
pub fn stored_values(kind: &str, data: &Map<String, Value>) -> Option<Value> {
    if kind == "chat.message" || kind == "chat.notice" {
        return None;
    }
    let stored = data.get("provenance")?.as_object()?;
    let values = stored.get("values")?.as_object()?;
    let mut out = Map::new();
    for (k, v) in values {
        if let Value::Array(items) = v {
            out.insert(
                k.clone(),
                Value::Array(items.iter().map(|c| Value::String(py_str(c))).collect()),
            );
        }
    }
    Some(Value::Object(out))
}

/// `str(value)` for a JSON value as Python's `str` writes it (text as is; `None`, `True`, numbers as Python).
pub fn py_str(value: &Value) -> String {
    ferb_orchestrator::py::str_of(value)
}

/// Python truthiness of an optional JSON value.
pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `"\n".split` of Python's `str.split()` with no argument, re-exported for the modules here.
pub fn words(text: &str) -> Vec<&str> {
    split_ws(text)
}

/// supervisor.py:130 `watch_condition`: a watch run's report of its condition, as a condition the check judges.
pub fn watch_condition(met: Option<bool>) -> Condition {
    let said = match met {
        Some(true) => {
            "The condition this watch checks holds now, as my report says (watch_met true)"
        }
        Some(false) => {
            "The condition this watch checks does not hold now, as my report says (watch_met false)"
        }
        None => "My report says whether the condition this watch checks holds now (it gave no watch_met)",
    };
    Condition {
        text: said.into(),
        owner_text: String::new(),
        check: ferb_orchestrator::Check::none(),
        layer: "outcome".into(),
    }
}

/// supervisor.py:140 `checked_conditions`: the planner's conditions, then (a watch's run) its own; none when the
/// plan has none.
pub fn checked_conditions(
    conditions: &[Condition],
    watch: bool,
    watch_met: Option<bool>,
) -> Vec<Condition> {
    if conditions.is_empty() {
        return Vec::new();
    }
    let mut out = conditions.to_vec();
    if watch {
        out.push(watch_condition(watch_met));
    }
    out
}
